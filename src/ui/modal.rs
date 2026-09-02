use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Modal};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Some(modal) = &app.modal else { return };
    let theme = &app.theme;
    match modal {
        Modal::Help => {
            let rect = centered(area, 62, 24);
            frame.render_widget(Clear, rect);
            let mut lines: Vec<Line> = HELP
                .iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!("{k:>12}  "), theme.accent_style()),
                        Span::styled(*v, theme.body()),
                    ])
                })
                .collect();
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "  commands (type / for completion)",
                theme.hint(),
            )));
            for c in crate::commands::COMMANDS {
                let name = if c.args.is_empty() {
                    format!("/{}", c.name)
                } else {
                    format!("/{} {}", c.name, c.args)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{name:>12}  "), theme.accent_style()),
                    Span::styled(c.help, theme.body()),
                ]));
            }
            frame.render_widget(
                Paragraph::new(lines).style(theme.card_bg()).block(titled_block("help", app)),
                rect,
            );
        }
        Modal::ModelPicker { items, selected } => {
            let height = (items.len() as u16 + 2).min(14);
            let rect = centered(area, 44, height);
            frame.render_widget(Clear, rect);
            let list = List::new(items.iter().map(|m| ListItem::new(m.clone())))
                .style(theme.card_bg())
                .highlight_style(theme.selected())
                .highlight_symbol("› ")
                .block(titled_block("model", app));
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, rect, &mut state);
        }
        Modal::SessionPicker { items, selected } => {
            let height = (items.len() as u16 + 2).min(16);
            let rect = centered(area, 64, height);
            frame.render_widget(Clear, rect);
            let list = List::new(items.iter().map(|s| ListItem::new(s.title.clone())))
                .style(theme.card_bg())
                .highlight_style(theme.selected())
                .highlight_symbol("› ")
                .block(titled_block("sessions", app));
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, rect, &mut state);
        }
        Modal::Permission { title, options, selected, .. } => {
            let height = (options.len() as u16 + 5).min(14);
            let rect = centered(area, 64, height);
            frame.render_widget(Clear, rect);
            let block = titled_block("permission", app);
            let inner = block.inner(rect);
            frame.render_widget(Clear, rect);
            frame.render_widget(block.style(theme.card_bg()), rect);
            let chunks = Layout::vertical([Constraint::Min(2), Constraint::Length(options.len() as u16 + 1)])
                .split(inner);
            frame.render_widget(
                Paragraph::new(title.clone()).style(theme.body()).wrap(Wrap { trim: true }),
                chunks[0],
            );
            let list = List::new(options.iter().map(|o| {
                let key = match o.kind.as_str() {
                    "allow_once" => "y",
                    "allow_always" => "a",
                    "reject_once" => "n",
                    "reject_always" => "N",
                    _ => " ",
                };
                ListItem::new(format!("[{key}] {}", o.name))
            }))
            .highlight_style(theme.selected())
            .highlight_symbol("› ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, chunks[1], &mut state);
        }
    }
}

fn titled_block<'a>(title: &'a str, app: &App) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border_active())
        .title(Span::styled(format!(" {title} "), app.theme.accent_style()))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [rect] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(rect);
    rect
}

const HELP: &[(&str, &str)] = &[
    ("Enter", "send"),
    ("Alt+Enter", "newline (also Ctrl+J)"),
    ("Esc", "cancel turn / close modal"),
    ("Ctrl+C", "cancel if busy, else quit"),
    ("Ctrl+D", "quit (empty input)"),
    ("Ctrl+T", "toggle chat/agent mode"),
    ("Ctrl+O", "model picker"),
    ("Ctrl+P", "session picker"),
    ("Ctrl+N", "new session"),
    ("Tab", "focus tool cards / expand"),
    ("PgUp/PgDn", "scroll transcript"),
    ("End", "follow live output"),
    ("y/a/n/N", "answer permission prompts"),
];
