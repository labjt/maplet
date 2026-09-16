//! A line-oriented CLI for working inside a directory: `maplet run "…"`.
//!
//! Assistant prose goes to stdout and everything else — tool calls, thinking,
//! status, prompts — goes to stderr, so `maplet run "…" > notes.md` captures
//! just the answer and `maplet run "…" | grep` behaves.

use std::cell::Cell;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde_json::json;
use tokio::sync::mpsc;

use crate::agent;
use crate::config::Config;
use crate::event::{
    AcpCommand, AcpEvent, AppEvent, PermissionOptionView, PermissionOutcome, PlanEntryView,
    ToolCallUpdateView, ToolCallView,
};
use crate::proxy;

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_INTERRUPTED: i32 = 2;

#[derive(Debug, Default)]
pub struct RunArgs {
    pub prompt: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub yes: bool,
    pub quiet: bool,
    pub json: bool,
    pub think: bool,
}

pub async fn run(config: Config, args: RunArgs) -> Result<i32> {
    let cwd = args.cwd.clone().unwrap_or(std::env::current_dir()?);
    let cwd = cwd
        .canonicalize()
        .with_context(|| format!("cannot use {} as the working directory", cwd.display()))?;
    if !cwd.is_dir() {
        bail!("{} is not a directory", cwd.display());
    }
    let ui = Ui::new(&args, cwd.clone());

    // A piped stdin is either the whole prompt or extra context for one given
    // on the command line; either way it means we are not interactive.
    let piped = read_piped_stdin()?;
    let first = compose_prompt(&args.prompt.join(" "), piped);
    let interactive = first.is_none();
    if interactive && !std::io::stdin().is_terminal() {
        bail!("no prompt given (pass one as an argument, or pipe it in)");
    }

    let api_key = config.resolve_api_key()?;
    crate::breadcrumb::mark("cli: key resolved, starting proxy");
    ui.status(&format!("working in {}", cwd.display()));
    ui.status("attesting enclave…");
    let proxy_handle = proxy::start(&config, &api_key).await?;
    let mut spawn_cfg =
        agent::GooseSpawn::from_config(&config, proxy_handle.base_url(), api_key.clone())?;
    if let Some(model) = &args.model {
        spawn_cfg.model = model.clone();
    }
    spawn_cfg.approve = !args.yes;
    // Maple retires model ids, and asking for a dead one only surfaces as an
    // opaque 400 once a turn is already under way. Check up front instead.
    crate::breadcrumb::mark("cli: proxy listening");
    let client = crate::chat::ChatClient::new(proxy_handle.base_url(), api_key);
    match client.list_models().await {
        Ok(models) if !models.iter().any(|m| *m == spawn_cfg.model) => {
            bail!(
                "model {} is not available on this account.\navailable: {}",
                spawn_cfg.model,
                models.join(", ")
            );
        }
        Ok(_) => {}
        Err(e) => ui.status(&format!("could not verify the model list ({e})")),
    }
    ui.status(&format!("model {}", spawn_cfg.model));

    crate::breadcrumb::mark("cli: spawning goose");
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let handle = agent::spawn(spawn_cfg, cwd, tx);

    // The agent has to finish starting before it will accept a prompt.
    loop {
        match rx.recv().await {
            Some(AppEvent::Acp(AcpEvent::SessionReady { .. })) => break,
            Some(AppEvent::Acp(AcpEvent::Exited(reason))) => {
                ui.error(&format!("agent exited before starting: {reason}"));
                return Ok(EXIT_ERROR);
            }
            Some(AppEvent::Acp(AcpEvent::AuthRequired(hint))) => {
                ui.error(&format!("agent auth problem: {hint}"));
                return Ok(EXIT_ERROR);
            }
            Some(_) => continue,
            None => return Ok(EXIT_ERROR),
        }
    }
    if interactive {
        ui.status("ready — type a request, Ctrl+D to leave");
    }

    let mut pending = first;
    let mut code = EXIT_OK;
    loop {
        let prompt = match pending.take() {
            Some(p) => p,
            None if interactive => match read_line("\nyou › ").await? {
                Some(line) if !line.trim().is_empty() => line,
                Some(_) => continue,
                None => break,
            },
            None => break,
        };
        if matches!(prompt.trim(), "exit" | "quit" | "/exit" | "/quit") {
            break;
        }

        ui.start_turn();
        handle.cmd_tx.send(AcpCommand::Prompt { text: prompt })?;
        match drive_turn(&mut rx, &handle, &ui, &args).await {
            Turn::Ended(reason) => {
                if reason != "end_turn" {
                    ui.status(&format!("turn ended: {reason}"));
                }
                if reason == "cancelled" {
                    code = EXIT_INTERRUPTED;
                }
            }
            Turn::Error(msg) => {
                ui.error(&msg);
                code = EXIT_ERROR;
                if !interactive {
                    break;
                }
            }
            Turn::Exited(reason) => {
                ui.error(&format!("agent exited: {reason}"));
                return Ok(EXIT_ERROR);
            }
        }
        if !interactive {
            break;
        }
    }

    let _ = handle.cmd_tx.send(AcpCommand::Shutdown);
    crate::breadcrumb::mark(&format!("cli: exit {code}"));
    Ok(code)
}

enum Turn {
    Ended(String),
    Error(String),
    Exited(String),
}

async fn drive_turn(
    rx: &mut mpsc::UnboundedReceiver<AppEvent>,
    handle: &agent::AgentHandle,
    ui: &Ui,
    args: &RunArgs,
) -> Turn {
    let mut cancelled = false;
    loop {
        let event = tokio::select! {
            event = rx.recv() => event,
            _ = tokio::signal::ctrl_c(), if !cancelled => {
                cancelled = true;
                ui.status("cancelling…");
                let _ = handle.cmd_tx.send(AcpCommand::Cancel);
                continue;
            }
        };
        let Some(AppEvent::Acp(event)) = event else {
            match event {
                Some(_) => continue,
                None => return Turn::Exited("event channel closed".into()),
            }
        };
        match event {
            AcpEvent::MessageChunk(text) => ui.answer(&text),
            AcpEvent::ThoughtChunk(text) => ui.thought(&text),
            AcpEvent::ToolCall(tc) => ui.tool_started(&tc),
            AcpEvent::ToolCallUpdate(update) => ui.tool_updated(&update),
            AcpEvent::Plan(entries) => ui.plan(&entries),
            AcpEvent::PermissionRequest { title, options, reply } => {
                let outcome = decide_permission(&title, &options, ui, args).await;
                let _ = reply.send(outcome);
            }
            AcpEvent::TurnEnded { stop_reason } => {
                ui.finish_answer();
                ui.turn_ended(&stop_reason);
                return Turn::Ended(stop_reason);
            }
            AcpEvent::Error(msg) | AcpEvent::AuthRequired(msg) => {
                ui.finish_answer();
                return Turn::Error(msg);
            }
            AcpEvent::Exited(reason) => {
                ui.finish_answer();
                return Turn::Exited(reason);
            }
            AcpEvent::SessionReady { .. } => {}
        }
    }
}

async fn decide_permission(
    title: &str,
    options: &[PermissionOptionView],
    ui: &Ui,
    args: &RunArgs,
) -> PermissionOutcome {
    let pick = |kind: &str| options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());

    if args.yes {
        ui.status(&format!("allowing: {title}"));
        return match pick("allow_once").or_else(|| options.first().map(|o| o.id.clone())) {
            Some(option_id) => PermissionOutcome::Selected { option_id },
            None => PermissionOutcome::Cancelled,
        };
    }
    // Nothing to prompt on: refuse rather than hang waiting for a tty.
    if !std::io::stdin().is_terminal() {
        ui.error(&format!("refused (no tty to ask on; pass --yes to allow): {title}"));
        return match pick("reject_once") {
            Some(option_id) => PermissionOutcome::Selected { option_id },
            None => PermissionOutcome::Cancelled,
        };
    }

    ui.permission_prompt(title, options);
    // Enter means the shown default (allow once); losing stdin must NOT.
    let answer = match read_line("allow? [Y/a/n] ").await {
        Ok(Some(line)) => line,
        _ => {
            ui.error("input closed before answering — refusing");
            return match pick("reject_once") {
                Some(option_id) => PermissionOutcome::Selected { option_id },
                None => PermissionOutcome::Cancelled,
            };
        }
    };
    let chosen = choose_permission(&answer, options);
    match chosen.or_else(|| options.first().map(|o| o.id.clone())) {
        Some(option_id) => PermissionOutcome::Selected { option_id },
        None => PermissionOutcome::Cancelled,
    }
}

/// A prompt given as arguments, piped in, or both (arguments first, piped
/// text as its context). None means there is nothing to run yet.
fn compose_prompt(inline: &str, piped: Option<String>) -> Option<String> {
    let inline = inline.trim();
    match (inline.is_empty(), piped) {
        (true, None) => None,
        (true, Some(piped)) => Some(piped),
        (false, None) => Some(inline.to_string()),
        (false, Some(piped)) => Some(format!("{inline}\n\n{piped}")),
    }
}

/// Map a typed answer onto one of the options the agent offered. Enter takes
/// the default shown in the prompt; anything unrecognised refuses.
fn choose_permission(answer: &str, options: &[PermissionOptionView]) -> Option<String> {
    let pick = |kind: &str| options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());
    match answer.trim() {
        "y" | "Y" | "" => pick("allow_once"),
        "a" | "A" => pick("allow_always").or_else(|| pick("allow_once")),
        _ => pick("reject_once"),
    }
}

fn read_piped_stdin() -> Result<Option<String>> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).context("reading stdin")?;
    let buf = buf.trim().to_string();
    Ok((!buf.is_empty()).then_some(buf))
}

/// Read one line from the tty without blocking the runtime.
async fn read_line(prompt: &str) -> Result<Option<String>> {
    let prompt = prompt.to_string();
    tokio::task::spawn_blocking(move || {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line)? {
            0 => Ok(None),
            _ => Ok(Some(line.trim_end().to_string())),
        }
    })
    .await?
}

struct Ui {
    json: bool,
    quiet: bool,
    color: bool,
    think: bool,
    cwd: PathBuf,
    mid_answer: Cell<bool>,
    mid_thought: Cell<bool>,
    said_thinking: Cell<bool>,
}

impl Ui {
    fn new(args: &RunArgs, cwd: PathBuf) -> Self {
        let color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Self {
            json: args.json,
            quiet: args.quiet,
            color,
            think: args.think,
            cwd,
            mid_answer: Cell::new(false),
            mid_thought: Cell::new(false),
            said_thinking: Cell::new(false),
        }
    }

    fn start_turn(&self) {
        self.said_thinking.set(false);
    }

    /// Close whichever stream is mid-line so the next output starts clean.
    fn break_line(&self) {
        if self.mid_answer.get() {
            println!();
            self.mid_answer.set(false);
        }
        if self.mid_thought.get() {
            if self.color {
                eprint!("\x1b[0m");
            }
            eprintln!();
            self.mid_thought.set(false);
        }
    }

    /// Paths inside the working directory read better relative to it.
    fn shorten(&self, text: &str) -> String {
        let cwd = format!("{}/", self.cwd.display());
        text.replace(&cwd, "")
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn emit(&self, value: serde_json::Value) {
        println!("{value}");
        let _ = std::io::stdout().flush();
    }

    fn note(&self, text: &str) {
        // keep side-channel output off the same line as streaming prose
        self.break_line();
        eprintln!("{text}");
    }

    fn status(&self, msg: &str) {
        if self.json {
            self.emit(json!({"type": "status", "message": msg}));
            return;
        }
        if self.quiet {
            return;
        }
        self.note(&self.paint("2", &format!("· {msg}")));
    }

    fn turn_ended(&self, reason: &str) {
        if self.json {
            self.emit(json!({"type": "turn_ended", "stop_reason": reason}));
        }
    }

    fn error(&self, msg: &str) {
        if self.json {
            self.emit(json!({"type": "error", "message": msg}));
            return;
        }
        self.note(&self.paint("31", &format!("! {msg}")));
    }

    fn answer(&self, text: &str) {
        if self.json {
            self.emit(json!({"type": "message", "text": text}));
            return;
        }
        if self.mid_thought.get() {
            self.break_line();
        }
        print!("{text}");
        let _ = std::io::stdout().flush();
        self.mid_answer.set(true);
    }

    fn finish_answer(&self) {
        self.break_line();
    }

    fn thought(&self, text: &str) {
        if self.json {
            self.emit(json!({"type": "thought", "text": text}));
            return;
        }
        if self.quiet {
            return;
        }
        // Reasoning arrives a few tokens at a time. Without --think just say
        // it is happening once; with it, stream as continuous prose rather
        // than one line per fragment.
        if !self.think {
            if !self.said_thinking.get() {
                self.said_thinking.set(true);
                self.note(&self.paint("2", "~ thinking…"));
            }
            return;
        }
        if self.mid_answer.get() {
            self.break_line();
        }
        if !self.mid_thought.get() {
            self.mid_thought.set(true);
            if self.color {
                eprint!("\x1b[2;3m");
            }
            eprint!("~ ");
        }
        eprint!("{}", text.replace('\n', " "));
        let _ = std::io::stderr().flush();
    }

    fn tool_started(&self, tc: &ToolCallView) {
        if self.json {
            self.emit(json!({
                "type": "tool", "id": tc.id, "kind": tc.kind,
                "title": tc.title, "status": tc.status,
            }));
            return;
        }
        if self.quiet {
            return;
        }
        let kind = if tc.kind == "other" { String::new() } else { format!("{} ", tc.kind) };
        self.note(&self.paint("2", &format!("› {kind}{}", self.shorten(&tc.title))));
    }

    fn tool_updated(&self, update: &ToolCallUpdateView) {
        if self.json {
            self.emit(json!({
                "type": "tool_update", "id": update.id,
                "status": update.status, "title": update.title,
            }));
            return;
        }
        // Successes are implied by the run continuing; only failures need saying.
        if update.status.as_deref() == Some("failed") {
            let what = update.title.clone().unwrap_or_else(|| update.id.clone());
            self.note(&self.paint("31", &format!("× {} failed", self.shorten(&what))));
        }
    }

    fn plan(&self, entries: &[PlanEntryView]) {
        if self.json {
            let items: Vec<_> = entries
                .iter()
                .map(|e| json!({"content": e.content, "status": e.status}))
                .collect();
            self.emit(json!({"type": "plan", "entries": items}));
            return;
        }
        if self.quiet {
            return;
        }
        for entry in entries {
            let glyph = match entry.status.as_str() {
                "completed" => "●",
                "in_progress" => "◌",
                _ => "○",
            };
            self.note(&self.paint("2", &format!("  {glyph} {}", entry.content)));
        }
    }

    fn permission_prompt(&self, title: &str, options: &[PermissionOptionView]) {
        if self.json {
            self.emit(json!({"type": "permission_request", "title": title}));
            return;
        }
        self.note(&self.paint("1;33", &format!("? {}", self.shorten(title))));
        let offered: Vec<&str> = options
            .iter()
            .filter_map(|o| match o.kind.as_str() {
                "allow_once" => Some("y allow once"),
                "allow_always" => Some("a allow always"),
                "reject_once" => Some("n reject"),
                _ => None,
            })
            .collect();
        if !offered.is_empty() {
            eprintln!("{}", self.paint("2", &format!("  {}", offered.join("  ·  "))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<PermissionOptionView> {
        ["allow_once", "allow_always", "reject_once"]
            .iter()
            .map(|k| PermissionOptionView {
                id: format!("id-{k}"),
                name: k.to_string(),
                kind: k.to_string(),
            })
            .collect()
    }

    #[test]
    fn prompt_sources_combine() {
        assert_eq!(compose_prompt("", None), None);
        assert_eq!(compose_prompt("  ", None), None);
        assert_eq!(compose_prompt("tidy up", None).as_deref(), Some("tidy up"));
        assert_eq!(compose_prompt("", Some("piped".into())).as_deref(), Some("piped"));
        // an argument prompt keeps piped text as its context, in that order
        assert_eq!(
            compose_prompt("explain", Some("a log line".into())).as_deref(),
            Some("explain\n\na log line")
        );
    }

    #[test]
    fn permission_answers_map_to_options() {
        let o = opts();
        assert_eq!(choose_permission("y", &o).as_deref(), Some("id-allow_once"));
        assert_eq!(choose_permission("", &o).as_deref(), Some("id-allow_once"));
        assert_eq!(choose_permission("a", &o).as_deref(), Some("id-allow_always"));
        assert_eq!(choose_permission("n", &o).as_deref(), Some("id-reject_once"));
        // anything unrecognised must refuse rather than fall through to allow
        assert_eq!(choose_permission("maybe", &o).as_deref(), Some("id-reject_once"));
    }

    #[test]
    fn allow_always_falls_back_when_not_offered() {
        let limited: Vec<_> = opts().into_iter().filter(|o| o.kind != "allow_always").collect();
        assert_eq!(choose_permission("a", &limited).as_deref(), Some("id-allow_once"));
    }
}
