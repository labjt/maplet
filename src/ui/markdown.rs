use std::collections::HashMap;

use ratatui::text::{Line, Span, Text};

/// tui-markdown borrows from the source string; transcript items live in App,
/// so renders are converted to owned Text and cached per (item, revision).
#[derive(Default)]
pub struct MarkdownCache {
    entries: HashMap<u64, (u64, Text<'static>)>,
}

impl MarkdownCache {
    pub fn render(&mut self, item_id: u64, revision: u64, source: &str) -> Text<'static> {
        if let Some((rev, text)) = self.entries.get(&item_id) {
            if *rev == revision {
                return text.clone();
            }
        }
        let text = to_owned(tui_markdown::from_str(source));
        self.entries.insert(item_id, (revision, text.clone()));
        text
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn to_owned(text: Text<'_>) -> Text<'static> {
    Text {
        lines: text
            .lines
            .into_iter()
            .map(|line| Line {
                spans: line
                    .spans
                    .into_iter()
                    .map(|span| Span {
                        content: std::borrow::Cow::Owned(span.content.into_owned()),
                        style: span.style,
                    })
                    .collect(),
                style: line.style,
                alignment: line.alignment,
            })
            .collect(),
        style: text.style,
        alignment: text.alignment,
    }
}
