pub mod completion;
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
    let completion_height = completion::height(app);
    let chunks = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(plan_height),
        Constraint::Length(completion_height),
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
                "completed" => ("●", ratatui::style::Style::default().fg(theme.green)),
                "in_progress" => ("◌", ratatui::style::Style::default().fg(theme.yellow)),
                _ => ("○", theme.hint()),
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
    if completion_height > 0 {
        completion::draw(frame, app, chunks[2]);
    }
    frame.render_widget(&app.input, chunks[3]);

    statusline::draw(frame, app, chunks[4]);
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

    /// Slash commands offer completion, and Enter runs the highlighted one.
    #[test]
    fn slash_command_completion_and_dispatch() {
        use crossterm::event::{KeyCode, KeyEvent};
        let mut app = App::new(&Config::default(), Theme::ansi());
        for ch in "/mod".chars() {
            app.handle(crate::event::AppEvent::Term(crossterm::event::Event::Key(
                KeyEvent::from(KeyCode::Char(ch)),
            )));
        }
        let comp = app.completion.as_ref().expect("popup open while typing a command");
        assert_eq!(comp.selected().name, "model");
        let screen = render(&mut app, 80, 24);
        assert!(screen.contains("/model"), "completion popup not drawn");

        // Enter runs the highlighted command rather than sending it as a message
        app.handle(crate::event::AppEvent::Term(crossterm::event::Event::Key(
            KeyEvent::from(KeyCode::Enter),
        )));
        assert!(app.completion.is_none(), "popup should close after running");
        assert!(app.chat_history.is_empty(), "a command must not be sent to the model");
    }

    /// Every glyph the UI draws must exist in ordinary monospace fonts, so it
    /// does not fall back (misaligning the grid) or render as tofu elsewhere.
    #[test]
    fn only_portable_glyphs_are_drawn() {
        use crate::event::{PlanEntryView, ToolCallView};
        const ALLOWED: &str = "±·»×…›→↓≡▌○◌●┌┐└┘─│";
        let mut app = App::new(&Config::default(), Theme::ansi());
        app.items.push(Item::User { text: "hi".into() });
        app.items.push(Item::Thought { text: "pondering".into(), collapsed: false });
        for (i, kind) in ["read", "edit", "delete", "move", "search", "execute", "think", "fetch", "other"]
            .iter()
            .enumerate()
        {
            app.items.push(Item::Tool(crate::app::ToolCard {
                view: ToolCallView {
                    id: format!("t{i}"),
                    title: "thing".into(),
                    kind: (*kind).into(),
                    status: ["pending", "in_progress", "completed", "failed"][i % 4].into(),
                    raw_input: None,
                    raw_output: None,
                },
                expanded: false,
            }));
        }
        for status in ["pending", "in_progress", "completed"] {
            app.plan.push(PlanEntryView { content: "step".into(), status: status.into() });
        }
        let screen = render(&mut app, 100, 40);
        let bad: Vec<char> = screen
            .chars()
            .filter(|c| !c.is_ascii() && !ALLOWED.contains(*c))
            .collect();
        assert!(bad.is_empty(), "non-portable glyphs drawn: {bad:?}");
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
