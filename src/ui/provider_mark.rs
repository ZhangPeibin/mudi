//! Terminal-safe Provider marks derived from the LobeHub Claude and Codex silhouettes.

use super::theme::Theme;
use crate::domain::Provider;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

pub(super) const CLAUDE: &str = "✳";
pub(super) const CODEX: &str = "⬡";

pub(super) fn icon(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode => CLAUDE,
        Provider::CodexCli => CODEX,
    }
}

pub(super) fn style(provider: Provider, theme: Theme, base: Style) -> Style {
    base.fg(match provider {
        Provider::ClaudeCode => theme.provider_claude,
        Provider::CodexCli => theme.provider_codex,
    })
}

pub(super) fn colorize(line: Line<'static>, theme: Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for span in line.spans {
        let mut plain = String::new();
        for character in span.content.chars() {
            let provider = match character {
                '✳' => Some(Provider::ClaudeCode),
                '⬡' => Some(Provider::CodexCli),
                _ => None,
            };
            let Some(provider) = provider else {
                plain.push(character);
                continue;
            };
            if !plain.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut plain), span.style));
            }
            spans.push(Span::styled(
                character.to_string(),
                style(provider, theme, span.style),
            ));
        }
        if !plain.is_empty() {
            spans.push(Span::styled(plain, span.style));
        }
    }
    Line {
        style: line.style,
        alignment: line.alignment,
        spans,
    }
}

#[cfg(test)]
#[path = "provider_mark_test.rs"]
mod tests;
