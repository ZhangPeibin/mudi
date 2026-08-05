use super::*;
use crate::ui::theme::ThemeName;

#[test]
fn each_guide_category_starts_with_its_visible_label() {
    let theme = ThemeName::default().palette();

    for category in GuideCategory::ALL {
        let lines = guide_content(category, theme, "sessionmesh");
        assert_eq!(lines[0].spans[0].content, category.label());
    }
}
