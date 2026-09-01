use std::path::PathBuf;

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

/// Semantic palette parsed from an omarchy theme's colors.toml.
/// Falls back to ANSI-16 so maplet still looks right off-omarchy.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub accent: Color,
    pub selection: Color,
    pub muted: Color,
    pub background: Color,
    pub darker_background: Color,
    pub lighter_background: Color,
    pub foreground: Color,
    pub dark_foreground: Color,
    pub bright_foreground: Color,
    pub red: Color,
    pub yellow: Color,
    pub green: Color,
    pub magenta: Color,
    pub cyan: Color,
}

#[derive(Debug, Deserialize)]
struct ColorsToml {
    accent: Option<String>,
    selection: Option<String>,
    muted: Option<String>,
    background: Option<String>,
    darker_background: Option<String>,
    lighter_background: Option<String>,
    foreground: Option<String>,
    dark_foreground: Option<String>,
    bright_foreground: Option<String>,
    red: Option<String>,
    yellow: Option<String>,
    green: Option<String>,
    magenta: Option<String>,
    cyan: Option<String>,
}

fn omarchy_current_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| PathBuf::from(".")).join("omarchy/current")
}

fn hex(s: &Option<String>, fallback: Color) -> Color {
    let Some(s) = s else { return fallback };
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return fallback;
    }
    match u32::from_str_radix(h, 16) {
        Ok(v) => Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
        Err(_) => fallback,
    }
}

impl Theme {
    /// ANSI-16 fallback theme (inherits the terminal's own palette).
    pub fn ansi() -> Self {
        Self {
            name: "terminal".into(),
            accent: Color::Blue,
            selection: Color::DarkGray,
            muted: Color::DarkGray,
            background: Color::Reset,
            darker_background: Color::Reset,
            lighter_background: Color::Reset,
            foreground: Color::Reset,
            dark_foreground: Color::DarkGray,
            bright_foreground: Color::White,
            red: Color::Red,
            yellow: Color::Yellow,
            green: Color::Green,
            magenta: Color::Magenta,
            cyan: Color::Cyan,
        }
    }

    /// Load the active omarchy theme, or the ANSI fallback.
    pub fn load() -> Self {
        let dir = omarchy_current_dir();
        let name = std::fs::read_to_string(dir.join("theme.name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "terminal".into());
        let Ok(raw) = std::fs::read_to_string(dir.join("theme/colors.toml")) else {
            return Self::ansi();
        };
        let Ok(c) = toml::from_str::<ColorsToml>(&raw) else {
            tracing::warn!("unparseable omarchy colors.toml; using ANSI fallback");
            return Self::ansi();
        };
        let f = Self::ansi();
        Self {
            name,
            accent: hex(&c.accent, f.accent),
            selection: hex(&c.selection, f.selection),
            muted: hex(&c.muted, f.muted),
            background: hex(&c.background, f.background),
            darker_background: hex(&c.darker_background, f.darker_background),
            lighter_background: hex(&c.lighter_background, f.lighter_background),
            foreground: hex(&c.foreground, f.foreground),
            dark_foreground: hex(&c.dark_foreground, f.dark_foreground),
            bright_foreground: hex(&c.bright_foreground, f.bright_foreground),
            red: hex(&c.red, f.red),
            yellow: hex(&c.yellow, f.yellow),
            green: hex(&c.green, f.green),
            magenta: hex(&c.magenta, f.magenta),
            cyan: hex(&c.cyan, f.cyan),
        }
    }

    // Role styles — the UI never touches raw palette fields directly.

    pub fn body(&self) -> Style {
        Style::default().fg(self.foreground)
    }

    pub fn user(&self) -> Style {
        Style::default().fg(self.bright_foreground).add_modifier(Modifier::BOLD)
    }

    pub fn hint(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn border_active(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn border_idle(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn error(&self) -> Style {
        Style::default().fg(self.red)
    }

    pub fn thought(&self) -> Style {
        Style::default().fg(self.magenta).add_modifier(Modifier::ITALIC | Modifier::DIM)
    }

    pub fn selected(&self) -> Style {
        Style::default().bg(self.selection).fg(self.bright_foreground)
    }

    pub fn card_bg(&self) -> Style {
        Style::default().bg(self.lighter_background)
    }
}

/// Watch omarchy's current-theme dir; sends a reloaded Theme on switches.
pub fn spawn_watcher(tx: tokio::sync::mpsc::UnboundedSender<crate::event::AppEvent>) {
    use notify::{RecursiveMode, Watcher};
    let dir = omarchy_current_dir();
    if !dir.exists() {
        return;
    }
    std::thread::spawn(move || {
        let (raw_tx, raw_rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(raw_tx) else { return };
        // Watch recursively: theme switches replace files under current/theme/.
        if watcher.watch(&dir, RecursiveMode::Recursive).is_err() {
            return;
        }
        while raw_rx.recv().is_ok() {
            // Debounce the burst of events a theme switch produces.
            std::thread::sleep(std::time::Duration::from_millis(200));
            while raw_rx.try_recv().is_ok() {}
            if tx.send(crate::event::AppEvent::ThemeChanged(Theme::load())).is_err() {
                return;
            }
        }
    });
}
