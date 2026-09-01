use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, McpServer, NewSessionRequest,
    PermissionOptionKind, PlanEntryStatus, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, SessionUpdate, StopReason, TextContent, ToolCallStatus, ToolKind,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, ConnectionTo};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::event::{
    AcpCommand, AcpEvent, AppEvent, PermissionOptionView, PermissionOutcome, PlanEntryView,
    ToolCallUpdateView, ToolCallView,
};

use super::goose::GooseSpawn;

/// Handle owned by the main loop; dropping cmd_tx shuts the agent down.
pub struct AgentHandle {
    pub cmd_tx: mpsc::UnboundedSender<AcpCommand>,
}

/// Spawn the ACP client on its own OS thread with a current-thread runtime.
/// This makes no Send assumptions about the ACP connection and isolates a
/// goose crash from the TUI.
pub fn spawn(
    spawn_cfg: GooseSpawn,
    cwd: PathBuf,
    tx: mpsc::UnboundedSender<AppEvent>,
) -> AgentHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AcpCommand>();
    std::thread::Builder::new()
        .name("acp".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(AppEvent::Acp(AcpEvent::Exited(format!(
                        "failed to build ACP runtime: {e}"
                    ))));
                    return;
                }
            };
            let result = rt.block_on(run(spawn_cfg, cwd, tx.clone(), cmd_rx));
            let reason = match result {
                Ok(()) => "shut down".to_string(),
                Err(e) => format!("{e:#}"),
            };
            let _ = tx.send(AppEvent::Acp(AcpEvent::Exited(reason)));
        })
        .expect("spawning acp thread");
    AgentHandle { cmd_tx }
}

async fn run(
    spawn_cfg: GooseSpawn,
    cwd: PathBuf,
    tx: mpsc::UnboundedSender<AppEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<AcpCommand>,
) -> anyhow::Result<()> {
    let agent = spawn_cfg.agent();
    let notify_tx = tx.clone();
    let perm_tx = tx.clone();

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                for event in map_update(notification.update) {
                    let _ = notify_tx.send(AppEvent::Acp(event));
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let title = request.tool_call.fields.title.clone().unwrap_or_else(|| {
                    "The agent wants to run a tool".to_string()
                });
                let options = request
                    .options
                    .iter()
                    .map(|o| PermissionOptionView {
                        id: o.option_id.0.to_string(),
                        name: o.name.clone(),
                        kind: permission_kind_str(&o.kind).to_string(),
                    })
                    .collect();
                let _ = perm_tx.send(AppEvent::Acp(AcpEvent::PermissionRequest {
                    title,
                    options,
                    reply: reply_tx,
                }));
                let outcome = match reply_rx.await {
                    Ok(PermissionOutcome::Selected { option_id }) => {
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            option_id,
                        ))
                    }
                    Ok(PermissionOutcome::Cancelled) | Err(_) => {
                        RequestPermissionOutcome::Cancelled
                    }
                };
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let mut new_session = NewSessionRequest::new(cwd.clone());
            new_session.mcp_servers = map_mcp_servers(&spawn_cfg.mcp_servers);
            let session = connection.send_request(new_session).block_task().await?;
            let session_id = session.session_id.clone();
            let _ = tx.send(AppEvent::Acp(AcpEvent::SessionReady {
                session_id: session_id.0.to_string(),
            }));

            let mut turns = FuturesUnordered::new();
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => match cmd {
                        Some(AcpCommand::Prompt { text }) => {
                            let request = PromptRequest::new(
                                session_id.clone(),
                                vec![ContentBlock::Text(TextContent::new(text))],
                            );
                            let fut = connection.send_request(request);
                            turns.push(async move { fut.block_task().await });
                        }
                        Some(AcpCommand::Cancel) => {
                            let _ = connection
                                .send_notification(CancelNotification::new(session_id.clone()));
                        }
                        Some(AcpCommand::Shutdown) | None => break,
                    },
                    Some(result) = turns.next(), if !turns.is_empty() => {
                        let event = match result {
                            Ok(response) => AcpEvent::TurnEnded {
                                stop_reason: stop_reason_str(&response.stop_reason).to_string(),
                            },
                            Err(e) => classify_error(e),
                        };
                        let _ = tx.send(AppEvent::Acp(event));
                    }
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

fn map_mcp_servers(decls: &[crate::config::McpServerDecl]) -> Vec<McpServer> {
    use agent_client_protocol::schema::v1::{EnvVariable, HttpHeader, McpServerHttp, McpServerStdio};
    decls
        .iter()
        .filter_map(|d| match d.transport.as_str() {
            "stdio" => {
                let command = d.command.clone()?;
                let mut server = McpServerStdio::new(d.name.clone(), command);
                server.args = d.args.clone();
                server.env = d
                    .env
                    .iter()
                    .map(|(k, v)| EnvVariable::new(k.clone(), v.clone()))
                    .collect();
                Some(McpServer::Stdio(server))
            }
            "http" => {
                let url = d.url.clone()?;
                let mut server = McpServerHttp::new(d.name.clone(), url);
                server.headers = d
                    .env
                    .iter()
                    .map(|(k, v)| HttpHeader::new(k.clone(), v.clone()))
                    .collect();
                Some(McpServer::Http(server))
            }
            other => {
                tracing::warn!("ignoring MCP server {} with unknown transport {other}", d.name);
                None
            }
        })
        .collect()
}

fn classify_error(e: agent_client_protocol::Error) -> AcpEvent {
    let text = e.to_string();
    if text.to_lowercase().contains("auth") {
        AcpEvent::AuthRequired(text)
    } else {
        AcpEvent::Error(format!("agent turn failed: {text}"))
    }
}

fn map_update(update: SessionUpdate) -> Vec<AcpEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            content_text(&chunk.content).map(AcpEvent::MessageChunk).into_iter().collect()
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            content_text(&chunk.content).map(AcpEvent::ThoughtChunk).into_iter().collect()
        }
        SessionUpdate::ToolCall(tc) => vec![AcpEvent::ToolCall(ToolCallView {
            id: tc.tool_call_id.0.to_string(),
            title: tc.title,
            kind: tool_kind_str(&tc.kind).to_string(),
            status: tool_status_str(&tc.status).to_string(),
            raw_input: tc.raw_input.map(|v| pretty_json(&v)),
            raw_output: tc.raw_output.map(|v| pretty_json(&v)),
        })],
        SessionUpdate::ToolCallUpdate(up) => {
            vec![AcpEvent::ToolCallUpdate(ToolCallUpdateView {
                id: up.tool_call_id.0.to_string(),
                title: up.fields.title,
                status: up.fields.status.map(|s| tool_status_str(&s).to_string()),
                raw_input: up.fields.raw_input.map(|v| pretty_json(&v)),
                raw_output: up.fields.raw_output.map(|v| pretty_json(&v)),
            })]
        }
        SessionUpdate::Plan(plan) => vec![AcpEvent::Plan(
            plan.entries
                .into_iter()
                .map(|e| PlanEntryView {
                    content: e.content,
                    status: plan_status_str(&e.status).to_string(),
                })
                .collect(),
        )],
        _ => vec![],
    }
}

fn content_text(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(t) => Some(t.text.clone()),
        _ => None,
    }
}

fn pretty_json(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn permission_kind_str(kind: &PermissionOptionKind) -> &'static str {
    match kind {
        PermissionOptionKind::AllowOnce => "allow_once",
        PermissionOptionKind::AllowAlways => "allow_always",
        PermissionOptionKind::RejectOnce => "reject_once",
        PermissionOptionKind::RejectAlways => "reject_always",
        _ => "other",
    }
}

fn stop_reason_str(reason: &StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        _ => "unknown",
    }
}

fn tool_kind_str(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        _ => "other",
    }
}

fn tool_status_str(status: &ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "pending",
        ToolCallStatus::InProgress => "in_progress",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Failed => "failed",
        _ => "unknown",
    }
}

fn plan_status_str(status: &PlanEntryStatus) -> &'static str {
    match status {
        PlanEntryStatus::Pending => "pending",
        PlanEntryStatus::InProgress => "in_progress",
        PlanEntryStatus::Completed => "completed",
        _ => "unknown",
    }
}
