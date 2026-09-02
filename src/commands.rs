//! Slash commands, following the conventions of Claude Code, opencode and
//! friends: type `/` at the start of the input to get a filtered completion
//! list, Tab to complete, Enter to run.

pub struct Command {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Argument hint shown in the completion list, empty if the command takes none.
    pub args: &'static str,
    pub help: &'static str,
}

pub const COMMANDS: &[Command] = &[
    Command { name: "help", aliases: &["?"], args: "", help: "show keys and commands" },
    Command { name: "model", aliases: &["models"], args: "[id]", help: "pick a model, or set one by id" },
    Command { name: "new", aliases: &["clear"], args: "", help: "start a new session" },
    Command { name: "sessions", aliases: &["resume"], args: "", help: "reopen a saved session" },
    Command { name: "chat", aliases: &[], args: "", help: "switch to chat mode" },
    Command { name: "agent", aliases: &[], args: "", help: "switch to agent mode" },
    Command { name: "quit", aliases: &["exit", "q"], args: "", help: "leave maplet" },
];

pub fn lookup(name: &str) -> Option<&'static Command> {
    let name = name.trim_start_matches('/');
    COMMANDS
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name))
}

/// Commands whose name or aliases start with `prefix` (the text after `/`).
pub fn matching(prefix: &str) -> Vec<&'static Command> {
    COMMANDS
        .iter()
        .filter(|c| {
            c.name.starts_with(prefix) || c.aliases.iter().any(|a| a.starts_with(prefix))
        })
        .collect()
}

pub enum Parsed {
    Known(&'static Command, String),
    Unknown(String),
}

/// Parse input as a slash command. Returns None when it is an ordinary message.
pub fn parse(input: &str) -> Option<Parsed> {
    let input = input.trim();
    let rest = input.strip_prefix('/')?;
    if rest.is_empty() {
        return None;
    }
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim().to_string()),
        None => (rest, String::new()),
    };
    Some(match lookup(name) {
        Some(cmd) => Parsed::Known(cmd, args),
        None => Parsed::Unknown(name.to_string()),
    })
}

/// The completion popup shown while typing a command name.
pub struct Completion {
    pub items: Vec<&'static Command>,
    pub selected: usize,
}

impl Completion {
    /// Some(..) while the input is a bare `/name` fragment with no argument yet.
    pub fn for_input(input: &str, previous: Option<&Completion>) -> Option<Self> {
        let rest = input.strip_prefix('/')?;
        if rest.contains(char::is_whitespace) {
            return None; // typing arguments now, not a command name
        }
        let items = matching(rest);
        if items.is_empty() {
            return None;
        }
        // keep the highlighted entry where it was if it still matches
        let selected = previous
            .and_then(|p| p.items.get(p.selected))
            .and_then(|prev| items.iter().position(|c| c.name == prev.name))
            .unwrap_or(0);
        Some(Self { items, selected })
    }

    pub fn selected(&self) -> &'static Command {
        self.items[self.selected]
    }
}
