use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// Rows the popup wants, so the layout can reserve space above the input.
pub fn height(app: &App) -> u16 {
    match &app.completion {
        Some(c) => (c.items.len() as u16).min(8) + 2,
        None => 0,
    }
}

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Some(comp) = &app.completion else { return };
    let theme = &app.theme;
    let visible = (area.height.saturating_sub(2)) as usize;
    // keep the highlighted row on screen when the list is longer than the popup
    let start = comp.selected.saturating_sub(visible.saturating_sub(1));
    let width = comp
        .items
        .iter()
        .map(|c| c.name.len() + c.args.len())
        .max()
        .unwrap_or(8)
        + 3;

    let lines: Vec<Line> = comp
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, c)| {
            let selected = i == comp.selected;
            let marker = if selected { "› " } else { "  " };
            let name = if c.args.is_empty() {
                format!("/{}", c.name)
            } else {
                format!("/{} {}", c.name, c.args)
            };
            let name_style = if selected {
                theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                theme.accent_style()
            };
            // pad to the full inner width so the selected row reads as one bar
            let used = marker.len() + width.max(name.len()) + c.help.len();
            let pad = (area.width as usize).saturating_sub(2).saturating_sub(used);
            let line = Line::from(vec![
                Span::styled(marker.to_string(), theme.accent_style()),
                Span::styled(format!("{name:<width$}"), name_style),
                Span::styled(c.help.to_string(), theme.hint()),
                Span::raw(" ".repeat(pad)),
            ]);
            if selected {
                line.style(ratatui::style::Style::default().bg(theme.selection))
            } else {
                line
            }
        })
        .collect();

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).style(theme.card_bg()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_active())
                .title(Span::styled(" commands ", theme.accent_style())),
        ),
        area,
    );
}
