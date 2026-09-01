pub mod markdown;
pub mod modal;
pub mod statusline;
pub mod transcript;

use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let input_height = (app.input.lines().len() as u16).clamp(1, 6) + 2;
    let plan_height = if app.plan.is_empty() { 0 } else { (app.plan.len() as u16).min(5) + 1 };
    let chunks = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(plan_height),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    // Transcript with scroll/follow.
    let text = transcript::build(app);
    let transcript_area = chunks[0];
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let total = paragraph.line_count(transcript_area.width) as u16;
    let max_scroll = total.saturating_sub(transcript_area.height);
    if app.follow {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
        if app.scroll == max_scroll {
            app.follow = true;
        }
    }
    frame.render_widget(
        paragraph.style(app.theme.body()).scroll((app.scroll, 0)),
        transcript_area,
    );

    // Plan checklist (agent mode), pinned above the input.
    if plan_height > 0 {
        let theme = app.theme.clone();
        let mut lines = vec![Line::from(Span::styled("plan", theme.hint()))];
        for entry in app.plan.iter().take(plan_height as usize - 1) {
            let (glyph, style) = match entry.status.as_str() {
                "completed" => ("☑", ratatui::style::Style::default().fg(theme.green)),
                "in_progress" => ("◐", ratatui::style::Style::default().fg(theme.yellow)),
                _ => ("☐", theme.hint()),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{glyph} "), style),
                Span::styled(entry.content.clone(), theme.body()),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), chunks[1]);
    }

    // Input.
    let border = if app.busy { app.theme.border_idle() } else { app.theme.border_active() };
    app.input.set_block(Block::default().borders(Borders::ALL).border_style(border));
    app.input.set_style(app.theme.body());
    frame.render_widget(&app.input, chunks[2]);

    statusline::draw(frame, app, chunks[3]);
    modal::draw(frame, app, area);
}
