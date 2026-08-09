//! Shared compact key/value rows for adjacent Session panes.

use super::{text, theme::Theme};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub(super) struct FieldLine<'a> {
    label: &'a str,
    value: &'a str,
    label_width: usize,
    keep_end: bool,
    muted_value: bool,
    value_style: Option<Style>,
}

impl<'a> FieldLine<'a> {
    pub(super) fn new(label: &'a str, value: &'a str) -> Self {
        Self {
            label,
            value,
            label_width: UnicodeWidthStr::width(label),
            keep_end: false,
            muted_value: false,
            value_style: None,
        }
    }

    pub(super) fn label_width(mut self, label_width: usize) -> Self {
        self.label_width = label_width;
        self
    }

    pub(super) fn keep_end(mut self) -> Self {
        self.keep_end = true;
        self
    }

    pub(super) fn muted_value(mut self) -> Self {
        self.muted_value = true;
        self
    }

    pub(super) fn value_style(mut self, style: Style) -> Self {
        self.value_style = Some(style);
        self
    }

    pub(super) fn render(self, content_width: usize, theme: Theme) -> Line<'static> {
        let label_width = self.label_width.min(content_width);
        let separator = if label_width > 0 && content_width > label_width {
            " "
        } else {
            ""
        };
        let label = format!("{}{separator}", text::fit(self.label, label_width, false));
        let value_width = content_width.saturating_sub(UnicodeWidthStr::width(label.as_str()));
        let value = text::fit(self.value, value_width, self.keep_end);
        let label_style = Style::default().fg(theme.muted);
        let value_style = self.value_style.unwrap_or_else(|| {
            if self.muted_value {
                Style::default().fg(theme.muted)
            } else {
                Style::default().fg(theme.foreground)
            }
        });
        Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(value, value_style),
        ])
    }
}

#[cfg(test)]
#[path = "field_test.rs"]
mod tests;
