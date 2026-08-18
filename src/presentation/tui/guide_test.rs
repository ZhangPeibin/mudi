use super::*;
use crate::presentation::tui::provider_mark;
use crate::presentation::tui::theme::ThemeName;

#[test]
fn each_guide_category_starts_with_its_visible_label() {
    let theme = ThemeName::default().palette();

    for category in GuideCategory::ALL {
        let lines = guide_content(category, theme, "mudi");
        assert_eq!(lines[0].spans[0].content, category.label());
    }
}

#[test]
fn provider_marks_use_their_brand_colors_in_guide_examples() {
    let theme = ThemeName::default().palette();
    let lines = guide_content(GuideCategory::Sessions, theme, "mudi");
    let spans = lines
        .iter()
        .flat_map(|line| &line.spans)
        .collect::<Vec<_>>();

    assert_eq!(
        spans
            .iter()
            .find(|span| span.content == provider_mark::CLAUDE)
            .unwrap()
            .style
            .fg,
        Some(theme.provider_claude)
    );
    assert_eq!(
        spans
            .iter()
            .find(|span| span.content == provider_mark::CODEX)
            .unwrap()
            .style
            .fg,
        Some(theme.provider_codex)
    );
}
