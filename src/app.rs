use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::{CursorMove, TextArea};
use tokio::sync::oneshot;

use crate::chat::types::ChatMessage;
use crate::commands::{self, Completion, Parsed};
use crate::config::Config;
use crate::event::{
    AcpCommand, AcpEvent, AppEvent, ChatEvent, PermissionOptionView, PermissionOutcome,
    PlanEntryView, ToolCallUpdateView, ToolCallView,
};
use crate::theme::Theme;
use crate::ui::markdown::MarkdownCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    Agent,
}

pub struct ToolCard {
    pub view: ToolCallView,
    pub expanded: bool,
}

pub enum Item {
    User { text: String },
    Assistant { id: u64, md: String, revision: u64, streaming: bool },
    Thought { text: String, collapsed: bool },
    Tool(ToolCard),
    Info(String),
    Error(String),
}

pub enum Modal {
    ModelPicker { items: Vec<String>, selected: usize },
    SessionPicker { items: Vec<crate::session::SessionMeta>, selected: usize },
    Permission {
        title: String,
        options: Vec<PermissionOptionView>,
        selected: usize,
        reply: Option<oneshot::Sender<PermissionOutcome>>,
    },
    Help,
}

/// Side effects the reducer asks the main loop to perform.
pub enum Effect {
    SendChat { turn: u64, model: String, messages: Vec<ChatMessage> },
    CancelChat,
    Acp(AcpCommand),
    SaveSession,
    Quit,
}

pub struct App {
    pub mode: Mode,
    pub items: Vec<Item>,
    pub chat_history: Vec<ChatMessage>,
    pub input: TextArea<'static>,
    pub theme: Theme,
    pub busy: bool,
    pub turn: u64,
    pub chat_model: String,
    pub agent_model: String,
    pub available_models: Vec<String>,
    pub scroll: u16,
    pub follow: bool,
    pub modal: Option<Modal>,
    pub plan: Vec<PlanEntryView>,
    pub proxy_healthy: bool,
    pub agent_session: Option<String>,
    pub focused_tool: Option<usize>,
    pub markdown: MarkdownCache,
    pub completion: Option<Completion>,
    pub session_id: String,
    next_id: u64,
}

impl App {
    pub fn new(config: &Config, theme: Theme) -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(ratatui::style::Style::default());
        Self {
            mode: Mode::Chat,
            items: Vec::new(),
            chat_history: Vec::new(),
            input,
            theme,
            busy: false,
            turn: 0,
            chat_model: config.chat.model.clone(),
            agent_model: config.agent.model.clone(),
            available_models: Vec::new(),
            scroll: 0,
            follow: true,
            modal: None,
            plan: Vec::new(),
            proxy_healthy: true,
            agent_session: None,
            focused_tool: None,
            markdown: MarkdownCache::default(),
            completion: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            next_id: 0,
        }
    }

    pub fn model(&self) -> &str {
        match self.mode {
            Mode::Chat => &self.chat_model,
            Mode::Agent => &self.agent_model,
        }
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn push_info(&mut self, text: impl Into<String>) {
        self.items.push(Item::Info(text.into()));
    }

    fn push_error(&mut self, text: impl Into<String>) {
        self.items.push(Item::Error(text.into()));
    }

    /// The streaming assistant item, if the last item is one.
    fn streaming_item(&mut self) -> Option<&mut Item> {
        match self.items.last_mut() {
            Some(item @ Item::Assistant { streaming: true, .. }) => Some(item),
            _ => None,
        }
    }

    fn ensure_streaming_item(&mut self) -> &mut Item {
        if self.streaming_item().is_none() {
            let id = self.next_id();
            self.items.push(Item::Assistant { id, md: String::new(), revision: 0, streaming: true });
        }
        self.items.last_mut().unwrap()
    }

    fn finish_streaming_item(&mut self) {
        if let Some(Item::Assistant { streaming, .. }) = self.items.last_mut() {
            *streaming = false;
        }
    }

    pub fn handle(&mut self, event: AppEvent) -> Vec<Effect> {
        match event {
            AppEvent::Term(Event::Key(key)) if key.is_press() => self.handle_key(key),
            AppEvent::Term(Event::Resize(..)) => vec![],
            AppEvent::Term(_) => vec![],
            AppEvent::Chat(e) => self.handle_chat(e),
            AppEvent::Acp(e) => self.handle_acp(e),
            AppEvent::ThemeChanged(theme) => {
                self.theme = theme;
                vec![]
            }
            AppEvent::ProxyHealth(ok) => {
                self.proxy_healthy = ok;
                vec![]
            }
            AppEvent::Models(models) => {
                self.available_models = models;
                vec![]
            }
        }
    }

    fn handle_chat(&mut self, event: ChatEvent) -> Vec<Effect> {
        match event {
            ChatEvent::Delta { turn, text } if turn == self.turn => {
                if let Item::Assistant { md, revision, .. } = self.ensure_streaming_item() {
                    md.push_str(&text);
                    *revision += 1;
                }
            }
            ChatEvent::Done { turn } if turn == self.turn => {
                self.busy = false;
                if let Some(Item::Assistant { md, .. }) = self.streaming_item() {
                    let reply = md.clone();
                    self.chat_history.push(ChatMessage::assistant(reply));
                }
                self.finish_streaming_item();
                return vec![Effect::SaveSession];
            }
            ChatEvent::Error { turn, message } if turn == self.turn => {
                self.busy = false;
                self.finish_streaming_item();
                self.chat_history.pop();
                self.push_error(message);
            }
            _ => {} // stale turn
        }
        vec![]
    }

    fn handle_acp(&mut self, event: AcpEvent) -> Vec<Effect> {
        match event {
            AcpEvent::SessionReady { session_id } => {
                self.agent_session = Some(session_id);
                self.push_info("agent session ready");
            }
            AcpEvent::MessageChunk(text) => {
                // Collapse the preceding thought once real output starts.
                if let Some(Item::Thought { collapsed, .. }) = self.items.last_mut() {
                    *collapsed = true;
                }
                if let Item::Assistant { md, revision, .. } = self.ensure_streaming_item() {
                    md.push_str(&text);
                    *revision += 1;
                }
            }
            AcpEvent::ThoughtChunk(text) => match self.items.last_mut() {
                Some(Item::Thought { text: t, collapsed: false, .. }) => t.push_str(&text),
                _ => {
                    self.finish_streaming_item();
                    self.items.push(Item::Thought { text, collapsed: false });
                }
            },
            AcpEvent::ToolCall(view) => {
                self.finish_streaming_item();
                self.items.push(Item::Tool(ToolCard { view, expanded: false }));
            }
            AcpEvent::ToolCallUpdate(update) => self.merge_tool_update(update),
            AcpEvent::Plan(entries) => self.plan = entries,
            AcpEvent::PermissionRequest { title, options, reply } => {
                self.modal = Some(Modal::Permission {
                    title,
                    options,
                    selected: 0,
                    reply: Some(reply),
                });
            }
            AcpEvent::TurnEnded { stop_reason } => {
                self.busy = false;
                self.finish_streaming_item();
                self.plan.clear();
                if stop_reason != "end_turn" {
                    self.push_info(format!("turn ended: {stop_reason}"));
                }
            }
            AcpEvent::Error(message) => {
                self.busy = false;
                self.finish_streaming_item();
                self.push_error(message);
            }
            AcpEvent::AuthRequired(hint) => {
                self.busy = false;
                self.push_error(format!("goose provider auth problem: {hint}"));
            }
            AcpEvent::Exited(reason) => {
                self.busy = false;
                self.agent_session = None;
                self.push_error(format!("agent exited: {reason} (Ctrl+T twice respawns)"));
            }
        }
        vec![]
    }

    fn merge_tool_update(&mut self, update: ToolCallUpdateView) {
        for item in self.items.iter_mut().rev() {
            if let Item::Tool(card) = item {
                if card.view.id == update.id {
                    if let Some(t) = update.title {
                        card.view.title = t;
                    }
                    if let Some(s) = update.status {
                        card.view.status = s;
                    }
                    if update.raw_input.is_some() {
                        card.view.raw_input = update.raw_input;
                    }
                    if update.raw_output.is_some() {
                        card.view.raw_output = update.raw_output;
                    }
                    return;
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }
        if let Some(effects) = self.handle_completion_key(key) {
            return effects;
        }
        let effects = self.handle_input_key(key);
        self.refresh_completion();
        effects
    }

    /// Keys owned by the slash-command popup while it is open.
    fn handle_completion_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let comp = self.completion.as_ref()?;
        let (len, sel) = (comp.items.len(), comp.selected);
        let chosen = comp.selected().name;
        let takes_args = !comp.selected().args.is_empty();
        match key.code {
            KeyCode::Up => {
                self.completion.as_mut()?.selected = sel.saturating_sub(1);
                Some(vec![])
            }
            KeyCode::Down => {
                self.completion.as_mut()?.selected = (sel + 1).min(len - 1);
                Some(vec![])
            }
            KeyCode::Tab => {
                let filled = if takes_args { format!("/{chosen} ") } else { format!("/{chosen}") };
                self.set_input(&filled);
                self.refresh_completion();
                Some(vec![])
            }
            KeyCode::Enter => {
                self.clear_input();
                Some(self.run_command(chosen, ""))
            }
            KeyCode::Esc => {
                self.completion = None;
                Some(vec![])
            }
            _ => None,
        }
    }

    fn set_input(&mut self, text: &str) {
        let mut input = TextArea::new(vec![text.to_string()]);
        input.set_cursor_line_style(ratatui::style::Style::default());
        input.move_cursor(CursorMove::End);
        self.input = input;
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match (key.code, ctrl, alt) {
            (KeyCode::Char('c'), true, _) => {
                if self.busy {
                    self.cancel_turn()
                } else {
                    vec![Effect::Quit]
                }
            }
            (KeyCode::Char('d'), true, _) if self.input_is_empty() => vec![Effect::Quit],
            (KeyCode::Esc, ..) => {
                if self.busy {
                    self.cancel_turn()
                } else {
                    self.focused_tool = None;
                    vec![]
                }
            }
            (KeyCode::Enter, false, true) | (KeyCode::Char('j'), true, false) => {
                self.input.insert_newline();
                vec![]
            }
            (KeyCode::Enter, false, false) => self.submit(),
            (KeyCode::Char('t'), true, false) if !self.busy => {
                self.toggle_mode();
                vec![]
            }
            (KeyCode::Char('o'), true, false) if !self.busy => {
                self.open_model_picker();
                vec![]
            }
            (KeyCode::Char('p'), true, false) if !self.busy => {
                let items = crate::session::list();
                if items.is_empty() {
                    self.push_info("no saved sessions yet");
                } else {
                    self.modal = Some(Modal::SessionPicker { items, selected: 0 });
                }
                vec![]
            }
            (KeyCode::Char('n'), true, false) if !self.busy => {
                self.new_session();
                vec![]
            }
            (KeyCode::Char('?'), ..) if self.input_is_empty() => {
                self.modal = Some(Modal::Help);
                vec![]
            }
            (KeyCode::PageUp, ..) => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(10);
                vec![]
            }
            (KeyCode::PageDown, ..) => {
                self.scroll = self.scroll.saturating_add(10);
                vec![]
            }
            (KeyCode::End, ..) if self.input_is_empty() => {
                self.follow = true;
                vec![]
            }
            (KeyCode::Tab, false, false) => {
                self.cycle_tool_focus(1);
                vec![]
            }
            (KeyCode::BackTab, ..) => {
                self.cycle_tool_focus(-1);
                vec![]
            }
            _ => {
                self.input.input(key);
                vec![]
            }
        }
    }

    fn input_is_empty(&self) -> bool {
        self.input.lines().iter().all(|l| l.trim().is_empty())
    }

    fn cancel_turn(&mut self) -> Vec<Effect> {
        match self.mode {
            Mode::Chat => vec![Effect::CancelChat],
            Mode::Agent => vec![Effect::Acp(AcpCommand::Cancel)],
        }
    }

    fn submit(&mut self) -> Vec<Effect> {
        if self.busy {
            return vec![];
        }
        let text = self.input.lines().join("\n").trim().to_string();
        if text.is_empty() {
            return vec![];
        }
        if let Some(parsed) = commands::parse(&text) {
            self.clear_input();
            return match parsed {
                Parsed::Known(cmd, args) => self.run_command(cmd.name, &args),
                Parsed::Unknown(name) => {
                    self.push_error(format!("unknown command /{name} — try /help"));
                    vec![]
                }
            };
        }
        self.clear_input();
        self.items.push(Item::User { text: text.clone() });
        self.busy = true;
        self.follow = true;
        self.turn += 1;
        match self.mode {
            Mode::Chat => {
                self.chat_history.push(ChatMessage::user(text));
                vec![Effect::SendChat {
                    turn: self.turn,
                    model: self.chat_model.clone(),
                    messages: self.chat_history.clone(),
                }]
            }
            Mode::Agent => vec![Effect::Acp(AcpCommand::Prompt { text })],
        }
    }

    fn clear_input(&mut self) {
        self.input = TextArea::default();
        self.input.set_cursor_line_style(ratatui::style::Style::default());
        self.completion = None;
    }

    /// Recompute the command completion popup from the current input.
    fn refresh_completion(&mut self) {
        let line = self.input.lines().first().cloned().unwrap_or_default();
        let single_line = self.input.lines().len() == 1;
        self.completion = if single_line {
            Completion::for_input(&line, self.completion.as_ref())
        } else {
            None
        };
    }

    fn run_command(&mut self, name: &str, args: &str) -> Vec<Effect> {
        match name {
            "help" => self.modal = Some(Modal::Help),
            "new" => self.new_session(),
            "sessions" => {
                let items = crate::session::list();
                if items.is_empty() {
                    self.push_info("no saved sessions yet");
                } else {
                    self.modal = Some(Modal::SessionPicker { items, selected: 0 });
                }
            }
            "chat" | "agent" => {
                let want = if name == "chat" { Mode::Chat } else { Mode::Agent };
                if self.mode == want {
                    self.push_info(format!("already in {name} mode"));
                } else {
                    self.mode = want;
                    self.push_info(format!("{name} mode"));
                }
            }
            "model" => {
                if args.is_empty() {
                    self.open_model_picker();
                } else {
                    self.set_model(args.to_string());
                }
            }
            "quit" => return vec![Effect::Quit],
            other => self.push_error(format!("unknown command /{other}")),
        }
        vec![]
    }

    fn set_model(&mut self, model: String) {
        let known = self.available_models.is_empty() || self.available_models.contains(&model);
        match self.mode {
            Mode::Chat => self.chat_model = model.clone(),
            Mode::Agent => self.agent_model = model.clone(),
        }
        if known {
            self.push_info(format!("model: {model}"));
        } else {
            self.push_info(format!("model: {model} (not in this account's list)"));
        }
        if self.mode == Mode::Agent {
            self.push_info("agent model applies to the next session");
        }
    }

    fn new_session(&mut self) {
        self.items.clear();
        self.chat_history.clear();
        self.plan.clear();
        self.markdown.clear();
        self.session_id = uuid::Uuid::new_v4().to_string();
        self.focused_tool = None;
        self.follow = true;
        self.push_info("new session");
    }

    fn load_session(&mut self, id: &str) {
        match crate::session::load(id) {
            Ok(session) => {
                self.new_session();
                self.items.clear();
                self.session_id = session.id;
                self.chat_model = session.model;
                self.mode = Mode::Chat;
                for msg in &session.messages {
                    match msg.role.as_str() {
                        "user" => self.items.push(Item::User { text: msg.content.clone() }),
                        _ => {
                            let id = self.next_id();
                            self.items.push(Item::Assistant {
                                id,
                                md: msg.content.clone(),
                                revision: 0,
                                streaming: false,
                            });
                        }
                    }
                }
                self.chat_history = session.messages;
            }
            Err(e) => self.push_error(format!("could not load session: {e:#}")),
        }
    }

    fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Chat => Mode::Agent,
            Mode::Agent => Mode::Chat,
        };
        let label = match self.mode {
            Mode::Chat => "chat mode",
            Mode::Agent => "agent mode",
        };
        self.push_info(label);
    }

    fn open_model_picker(&mut self) {
        if self.available_models.is_empty() {
            self.push_info("model list not loaded yet");
            return;
        }
        let current = self.model().to_string();
        let selected =
            self.available_models.iter().position(|m| *m == current).unwrap_or(0);
        self.modal =
            Some(Modal::ModelPicker { items: self.available_models.clone(), selected });
    }

    fn cycle_tool_focus(&mut self, dir: i64) {
        let tools: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, Item::Tool(_)))
            .map(|(idx, _)| idx)
            .collect();
        if tools.is_empty() {
            return;
        }
        let next = match self.focused_tool {
            Some(cur) => {
                if let Some(pos) = tools.iter().position(|&t| t == cur) {
                    // Second Tab on the same card toggles expansion.
                    if dir > 0 && pos == tools.len() - 1 {
                        if let Item::Tool(card) = &mut self.items[cur] {
                            card.expanded = !card.expanded;
                        }
                        return;
                    }
                    let next_pos = (pos as i64 + dir).rem_euclid(tools.len() as i64);
                    tools[next_pos as usize]
                } else {
                    tools[tools.len() - 1]
                }
            }
            None => tools[tools.len() - 1],
        };
        if Some(next) == self.focused_tool {
            if let Item::Tool(card) = &mut self.items[next] {
                card.expanded = !card.expanded;
            }
        }
        self.focused_tool = Some(next);
        self.follow = false;
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(modal) = &mut self.modal else { return vec![] };
        match modal {
            Modal::Help => {
                self.modal = None;
                vec![]
            }
            Modal::ModelPicker { items, selected } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = selected.saturating_sub(1);
                    vec![]
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1).min(items.len().saturating_sub(1));
                    vec![]
                }
                KeyCode::Enter => {
                    let model = items[*selected].clone();
                    self.modal = None;
                    self.set_model(model);
                    vec![]
                }
                KeyCode::Esc => {
                    self.modal = None;
                    vec![]
                }
                _ => vec![],
            },
            Modal::SessionPicker { items, selected } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = selected.saturating_sub(1);
                    vec![]
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1).min(items.len().saturating_sub(1));
                    vec![]
                }
                KeyCode::Enter => {
                    let id = items[*selected].id.clone();
                    self.modal = None;
                    self.load_session(&id);
                    vec![]
                }
                KeyCode::Esc => {
                    self.modal = None;
                    vec![]
                }
                _ => vec![],
            },
            Modal::Permission { options, selected, reply, .. } => {
                let outcome = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        *selected = selected.saturating_sub(1);
                        None
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        *selected = (*selected + 1).min(options.len().saturating_sub(1));
                        None
                    }
                    KeyCode::Enter => options.get(*selected).map(|o| o.id.clone()),
                    KeyCode::Char('y') => pick_kind(options, "allow_once"),
                    KeyCode::Char('a') => pick_kind(options, "allow_always"),
                    KeyCode::Char('n') => pick_kind(options, "reject_once"),
                    KeyCode::Char('N') => pick_kind(options, "reject_always"),
                    KeyCode::Esc => {
                        if let Some(tx) = reply.take() {
                            let _ = tx.send(PermissionOutcome::Cancelled);
                        }
                        self.modal = None;
                        return vec![];
                    }
                    _ => None,
                };
                if let Some(option_id) = outcome {
                    if let Some(tx) = reply.take() {
                        let _ = tx.send(PermissionOutcome::Selected { option_id });
                    }
                    self.modal = None;
                }
                vec![]
            }
        }
    }
}

fn pick_kind(options: &[PermissionOptionView], kind: &str) -> Option<String> {
    options.iter().find(|o| o.kind == kind).map(|o| o.id.clone())
}
