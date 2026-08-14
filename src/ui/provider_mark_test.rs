use super::*;
use crate::ui::theme::ThemeName;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

#[test]
fn provider_marks_are_distinct_single_cell_glyphs() {
    assert_ne!(CLAUDE, CODEX);
    assert_eq!(UnicodeWidthStr::width(CLAUDE), 1);
    assert_eq!(UnicodeWidthStr::width(CODEX), 1);
}

#[test]
fn provider_marks_use_the_brand_palette_without_losing_row_state() {
    let theme = ThemeName::Mudi.palette();
    let base = Style::default()
        .bg(theme.selected_background)
        .add_modifier(Modifier::BOLD);
    let line = colorize(
        Line::from(Span::styled(format!("{CLAUDE} {CODEX}"), base)),
        theme,
    );

    assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(217, 119, 87)));
    assert_eq!(line.spans[2].style.fg, Some(Color::Rgb(122, 157, 255)));
    for mark in [&line.spans[0], &line.spans[2]] {
        assert_eq!(mark.style.bg, Some(theme.selected_background));
        assert!(mark.style.add_modifier.contains(Modifier::BOLD));
    }
}

#[test]
fn light_theme_uses_readable_colors_from_the_same_brand_palettes() {
    let theme = ThemeName::Light.palette();

    assert_eq!(
        style(Provider::ClaudeCode, theme, Style::default()).fg,
        Some(Color::Rgb(161, 75, 52))
    );
    assert_eq!(
        style(Provider::CodexCli, theme, Style::default()).fg,
        Some(Color::Rgb(57, 65, 255))
    );
}
