use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Mode};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let mode = match app.mode {
        Mode::Chat => "CHAT",
        Mode::Agent => "AGENT",
    };
    let health = if app.proxy_healthy {
        Span::styled("● enclave", ratatui::style::Style::default().fg(theme.green))
    } else {
        Span::styled("● enclave?", theme.error())
    };
    let busy = if app.busy {
        Span::styled(" · working… (Esc cancels)", theme.hint())
    } else {
        Span::raw("")
    };
    let left = Line::from(vec![
        Span::styled(format!(" {mode} "), theme.selected()),
        Span::raw(" "),
        Span::styled(app.model().to_string(), theme.accent_style()),
        Span::raw(" · "),
        health,
        Span::raw(" · "),
        Span::styled(app.theme.name.clone(), theme.hint()),
        busy,
    ]);
    let hints = "/ commands · ? help ";
    let pad = (area.width as usize)
        .saturating_sub(line_width(&left))
        .saturating_sub(hints.len());
    let line = Line::from(
        left.spans
            .into_iter()
            .chain([Span::raw(" ".repeat(pad)), Span::styled(hints.to_string(), theme.hint())])
            .collect::<Vec<_>>(),
    );
    frame.render_widget(Paragraph::new(line), area);
}

fn line_width(line: &Line) -> usize {
    line.spans.iter().map(|s| s.content.chars().count()).sum()
}
