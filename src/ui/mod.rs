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

    // Transcript with scroll/follow. A short conversation is padded from the
    // top so it rests just above the input, the way a terminal fills up,
    // rather than stranding the messages at the top of an empty pane.
    let mut text = transcript::build(app);
    let transcript_area = chunks[0];
    let measured = Paragraph::new(text.clone())
        .wrap(Wrap { trim: false })
        .line_count(transcript_area.width) as u16;
    if measured < transcript_area.height {
        let mut lines = vec![Line::default(); (transcript_area.height - measured) as usize];
        lines.append(&mut text.lines);
        text = ratatui::text::Text::from(lines);
    }
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

#[cfg(test)]
mod tests {
    use crate::app::{App, Item};
    use crate::config::Config;
    use crate::theme::Theme;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| super::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The user's own message must stay visible after the reply arrives.
    #[test]
    fn user_message_visible_after_reply() {
        let mut app = App::new(&Config::default(), Theme::ansi());
        app.items.push(Item::User { text: "Reply with a fenced python block.".into() });
        app.items.push(Item::Assistant {
            id: 1,
            md: "```python\nprint(\"hello\")\n```".into(),
            revision: 1,
            streaming: false,
        });
        let screen = render(&mut app, 100, 30);
        eprintln!("--- rendered ---\n{screen}\n--- end ---");
        assert!(screen.contains("you"), "user message scrolled out of view");
    }

    /// A short conversation rests just above the input, not at the top of an
    /// empty pane, so the transcript fills like a terminal.
    #[test]
    fn short_conversation_is_bottom_anchored() {
        let mut app = App::new(&Config::default(), Theme::ansi());
        app.items.push(Item::User { text: "hello".into() });
        let screen = render(&mut app, 80, 24);
        let rows: Vec<&str> = screen.lines().collect();
        let msg = rows.iter().position(|r| r.contains("hello")).expect("message rendered");
        // input box occupies the last few rows; the message should sit just above it
        assert!(msg > rows.len() / 2, "message at row {msg} of {}, expected near the bottom", rows.len());
    }
}
