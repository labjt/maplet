use ratatui::text::{Line, Span, Text};

use crate::app::{App, Item, ToolCard};
use crate::theme::Theme;

const RAW_PREVIEW_LIMIT: usize = 1200;

fn kind_glyph(kind: &str) -> &'static str {
    match kind {
        "read" => "≡",
        "edit" => "±",
        "delete" => "×",
        "move" => "→",
        "search" => "○",
        "execute" => "»",
        "think" => "~",
        "fetch" => "↓",
        _ => "·",
    }
}

fn status_span(status: &str, theme: &Theme) -> Span<'static> {
    match status {
        "completed" => Span::styled("● completed", ratatui::style::Style::default().fg(theme.green)),
        "failed" => Span::styled("× failed", theme.error()),
        "in_progress" => {
            Span::styled("◌ running", ratatui::style::Style::default().fg(theme.yellow))
        }
        _ => Span::styled("○ pending", theme.hint()),
    }
}

/// Build the whole transcript as one wrapped Text. Item ids key the markdown
/// cache, so only the streaming tail re-renders per frame.
pub fn build(app: &mut App) -> Text<'static> {
    let theme = app.theme.clone();
    let focused = app.focused_tool;
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Split borrows: cache and items.
    let App { items, markdown, .. } = app;
    for (idx, item) in items.iter().enumerate() {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        match item {
            Item::User { text, .. } => {
                for (i, l) in text.lines().enumerate() {
                    let prefix = if i == 0 { "you › " } else { "      " };
                    lines.push(Line::from(vec![
                        Span::styled(prefix.to_string(), theme.accent_style()),
                        Span::styled(l.to_string(), theme.user()),
                    ]));
                }
            }
            Item::Assistant { id, md, revision, .. } => {
                let rendered = markdown.render(*id, *revision, md);
                lines.extend(rendered.lines);
            }
            Item::Thought { text, collapsed, .. } => {
                if *collapsed {
                    lines.push(Line::from(Span::styled("~ thought…".to_string(), theme.thought())));
                } else {
                    for l in text.lines() {
                        lines.push(Line::from(Span::styled(format!("~ {l}"), theme.thought())));
                    }
                }
            }
            Item::Tool(card) => {
                lines.extend(tool_lines(card, focused == Some(idx), &theme));
            }
            Item::Info(text) => {
                lines.push(Line::from(Span::styled(format!("· {text}"), theme.hint())));
            }
            Item::Error(text) => {
                for (i, l) in text.lines().enumerate() {
                    let prefix = if i == 0 { "! " } else { "  " };
                    lines.push(Line::from(Span::styled(format!("{prefix}{l}"), theme.error())));
                }
            }
        }
    }
    Text::from(lines)
}

fn tool_lines(card: &ToolCard, focused: bool, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let marker = if focused { "▌" } else { " " };
    // goose reports most tools as kind "other", so naming it adds nothing —
    // show the label only when it actually says something.
    let label = match card.view.kind.as_str() {
        "other" | "unknown" => format!("{} ", kind_glyph(&card.view.kind)),
        kind => format!("{} {} · ", kind_glyph(kind), kind),
    };
    let mut header = vec![
        Span::styled(marker.to_string(), theme.accent_style()),
        Span::styled(label, theme.accent_style()),
        Span::styled(card.view.title.clone(), theme.body()),
        Span::raw(" · "),
        status_span(&card.view.status, theme),
    ];
    if !card.expanded && (card.view.raw_input.is_some() || card.view.raw_output.is_some()) {
        header.push(Span::styled(" [Tab: details]", theme.hint()));
    }
    lines.push(Line::from(header));
    if card.expanded {
        for (label, raw) in
            [("input", &card.view.raw_input), ("output", &card.view.raw_output)]
        {
            if let Some(raw) = raw {
                lines.push(Line::from(Span::styled(format!("  {label}:"), theme.hint())));
                let mut preview: String = raw.chars().take(RAW_PREVIEW_LIMIT).collect();
                if raw.chars().count() > RAW_PREVIEW_LIMIT {
                    preview.push('…');
                }
                for l in preview.lines() {
                    lines.push(Line::from(Span::styled(format!("  {l}"), theme.body()))
                        .style(theme.card_bg()));
                }
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::Config;

    #[test]
    fn transcript_line_count_matches_rendered_lines() {
        let mut app = App::new(&Config::default(), Theme::ansi());
        app.items.push(Item::User { text: "Reply with only a fenced python code block.".into() });
        app.items.push(Item::Assistant {
            id: 1,
            md: "```python\nprint(\"hello\")\n```".into(),
            revision: 0,
            streaming: false,
        });
        let text = build(&mut app);
        let rendered = text.lines.len();
        let counted = ratatui::widgets::Paragraph::new(text)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .line_count(100);
        eprintln!("rendered Text lines = {rendered}, line_count(100) = {counted}");
        assert_eq!(rendered, counted, "line_count disagrees with actual line count");
    }
}
