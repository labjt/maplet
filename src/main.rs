mod agent;
mod app;
mod chat;
mod config;
mod event;
mod logging;
mod proxy;
mod session;
mod theme;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app::{App, Effect};
use crate::chat::types::ChatMessage;
use crate::chat::ChatClient;
use crate::config::Config;
use crate::event::{AppEvent, ChatEvent};
use crate::theme::Theme;

#[derive(Parser)]
#[command(name = "maplet", about = "Omarchy-native TUI for Maple AI", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Plain stdin/stdout chat REPL (no TUI) — plumbing check.
    Repl {
        /// Model id (default from config, else glm-5-2)
        #[arg(long)]
        model: Option<String>,
    },
    /// List available chat models.
    Models,
    /// Run one agent turn headlessly, printing every ACP event (plumbing check).
    AgentOneshot {
        prompt: String,
        /// Working directory for the agent session (default: current dir)
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    let _log_guard = logging::init()?;

    match cli.command {
        Some(Command::Repl { model }) => repl(config, model).await,
        Some(Command::Models) => models(config).await,
        Some(Command::AgentOneshot { prompt, cwd }) => agent_oneshot(config, prompt, cwd).await,
        None => tui(config).await,
    }
}

async fn start_proxy(config: &Config) -> Result<(proxy::ProxyHandle, ChatClient)> {
    let api_key = config.resolve_api_key()?;
    eprintln!("starting embedded maple-proxy (attesting enclave)...");
    let handle = proxy::start(config, &api_key).await?;
    let client = ChatClient::new(handle.base_url(), api_key);
    Ok((handle, client))
}

async fn tui(config: Config) -> Result<()> {
    let (proxy_handle, client) = start_proxy(&config).await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Terminal input pump.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut events = crossterm::event::EventStream::new();
            while let Some(Ok(ev)) = events.next().await {
                if tx.send(AppEvent::Term(ev)).is_err() {
                    break;
                }
            }
        });
    }
    theme::spawn_watcher(tx.clone());
    spawn_health_pinger(proxy_handle.base_url(), tx.clone());
    // Model list arrives whenever the backend answers; UI works without it.
    {
        let client = client.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            match client.list_models().await {
                Ok(models) => {
                    let _ = tx.send(AppEvent::Models(models));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Chat(ChatEvent::Error {
                        turn: 0,
                        message: format!("could not list models: {e:#}"),
                    }));
                }
            }
        });
    }

    let mut terminal = ratatui::init();
    let mut app = App::new(&config, Theme::load());
    let mut chat_cancel: Option<CancellationToken> = None;
    let mut agent_handle: Option<agent::AgentHandle> = None;
    let api_key = config.resolve_api_key()?;

    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        let Some(first) = rx.recv().await else { break };
        let mut effects = app.handle(first);
        // Coalesce whatever else is queued before redrawing.
        while let Ok(ev) = rx.try_recv() {
            effects.extend(app.handle(ev));
        }
        for effect in effects {
            match effect {
                Effect::Quit => {
                    ratatui::restore();
                    return Ok(());
                }
                Effect::SendChat { turn, model, messages } => {
                    let cancel = CancellationToken::new();
                    chat_cancel = Some(cancel.clone());
                    spawn_chat_turn(client.clone(), tx.clone(), turn, model, messages, cancel);
                }
                Effect::CancelChat => {
                    if let Some(cancel) = chat_cancel.take() {
                        cancel.cancel();
                    }
                }
                Effect::SaveSession => {
                    if let Err(e) =
                        session::save(&app.session_id, &app.chat_model, &app.chat_history)
                    {
                        tracing::warn!("session save failed: {e:#}");
                    }
                }
                Effect::Acp(cmd) => {
                    if agent_handle.is_none() {
                        match agent::GooseSpawn::from_config(
                            &config,
                            proxy_handle.base_url(),
                            api_key.clone(),
                        ) {
                            Ok(spawn_cfg) => {
                                let cwd = config::expand_tilde(config.agent.cwd.clone())
                                    .canonicalize()
                                    .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
                                agent_handle = Some(agent::spawn(spawn_cfg, cwd, tx.clone()));
                            }
                            Err(e) => {
                                let _ = tx.send(AppEvent::Acp(event::AcpEvent::Error(
                                    format!("{e:#}"),
                                )));
                                continue;
                            }
                        }
                    }
                    if let Some(handle) = &agent_handle {
                        if handle.cmd_tx.send(cmd).is_err() {
                            // Thread died; drop the handle so next use respawns.
                            agent_handle = None;
                            let _ = tx.send(AppEvent::Acp(event::AcpEvent::Error(
                                "agent connection lost; try again to respawn".into(),
                            )));
                        }
                    }
                }
            }
        }
    }
    ratatui::restore();
    Ok(())
}

fn spawn_chat_turn(
    client: ChatClient,
    tx: mpsc::UnboundedSender<AppEvent>,
    turn: u64,
    model: String,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let result = client
            .stream_chat(&model, messages, cancel, |delta| {
                let _ = tx.send(AppEvent::Chat(ChatEvent::Delta {
                    turn,
                    text: delta.to_string(),
                }));
            })
            .await;
        let event = match result {
            Ok(_) => ChatEvent::Done { turn },
            Err(e) => ChatEvent::Error { turn, message: format!("{e:#}") },
        };
        let _ = tx.send(AppEvent::Chat(event));
    });
}

fn spawn_health_pinger(base_url: String, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let healthy = client
                .get(format!("{base_url}/health"))
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if tx.send(AppEvent::ProxyHealth(healthy)).is_err() {
                break;
            }
        }
    });
}

async fn agent_oneshot(
    config: Config,
    prompt: String,
    cwd: Option<std::path::PathBuf>,
) -> Result<()> {
    use crate::event::{AcpCommand, AcpEvent, PermissionOutcome};

    let api_key = config.resolve_api_key()?;
    eprintln!("starting embedded maple-proxy (attesting enclave)...");
    let proxy_handle = proxy::start(&config, &api_key).await?;
    let cwd = match cwd {
        Some(c) => c.canonicalize()?,
        None => std::env::current_dir()?,
    };
    let spawn_cfg = agent::GooseSpawn::from_config(&config, proxy_handle.base_url(), api_key)?;
    eprintln!("spawning goose acp (model {}, cwd {})...", spawn_cfg.model, cwd.display());

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let handle = agent::spawn(spawn_cfg, cwd, tx);

    let mut prompted = false;
    while let Some(event) = rx.recv().await {
        let AppEvent::Acp(event) = event else { continue };
        match event {
            AcpEvent::SessionReady { session_id } => {
                println!("session-ready {session_id}");
                if !prompted {
                    prompted = true;
                    handle.cmd_tx.send(AcpCommand::Prompt { text: prompt.clone() })?;
                }
            }
            AcpEvent::MessageChunk(text) => {
                print!("{text}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            AcpEvent::ThoughtChunk(text) => print!("[thought] {text}"),
            AcpEvent::ToolCall(tc) => {
                println!("\ntool-call {} {} · {} · {}", tc.id, tc.kind, tc.title, tc.status)
            }
            AcpEvent::ToolCallUpdate(up) => {
                println!("tool-update {} · {}", up.id, up.status.as_deref().unwrap_or("?"))
            }
            AcpEvent::Plan(entries) => {
                println!("plan:");
                for e in entries {
                    println!("  [{}] {}", e.status, e.content);
                }
            }
            AcpEvent::PermissionRequest { title, options, reply } => {
                println!("\npermission: {title}");
                for (i, o) in options.iter().enumerate() {
                    println!("  {i}: {} ({})", o.name, o.kind);
                }
                eprint!("allow? [y/n] ");
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                let pick = |kind: &str| options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());
                let outcome = if line.trim().eq_ignore_ascii_case("y") {
                    pick("allow_once").or_else(|| options.first().map(|o| o.id.clone()))
                } else {
                    pick("reject_once")
                };
                let _ = reply.send(match outcome {
                    Some(option_id) => PermissionOutcome::Selected { option_id },
                    None => PermissionOutcome::Cancelled,
                });
            }
            AcpEvent::TurnEnded { stop_reason } => {
                println!("\nturn-ended {stop_reason}");
                let _ = handle.cmd_tx.send(AcpCommand::Shutdown);
            }
            AcpEvent::Error(e) => println!("\nerror: {e}"),
            AcpEvent::AuthRequired(e) => {
                println!("\nauth-required: {e}");
                let _ = handle.cmd_tx.send(AcpCommand::Shutdown);
            }
            AcpEvent::Exited(reason) => {
                println!("agent-exited: {reason}");
                break;
            }
        }
    }
    Ok(())
}

async fn models(config: Config) -> Result<()> {
    let (_proxy, client) = start_proxy(&config).await?;
    for id in client.list_models().await? {
        println!("{id}");
    }
    Ok(())
}

async fn repl(config: Config, model: Option<String>) -> Result<()> {
    let model = model.unwrap_or_else(|| config.chat.model.clone());
    let (_proxy, client) = start_proxy(&config).await?;
    eprintln!("model: {model} — type a message, Ctrl+D to exit\n");

    let mut history: Vec<ChatMessage> = Vec::new();
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        eprint!("you ▸ ");
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        history.push(ChatMessage::user(prompt));

        eprint!("maple ▸ ");
        let mut reply = String::new();
        let result = client
            .stream_chat(&model, history.clone(), CancellationToken::new(), |delta| {
                print!("{delta}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                reply.push_str(delta);
            })
            .await;
        println!();
        match result {
            Ok(_) => history.push(ChatMessage::assistant(reply)),
            Err(e) => {
                history.pop();
                eprintln!("error: {e:#}");
            }
        }
    }
}
