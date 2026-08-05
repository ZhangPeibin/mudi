use super::*;
use crate::ui::theme::ThemeName;

#[test]
fn wrapped_detail_fields_align_continuation_lines() {
    let mut lines = Vec::new();
    let theme = ThemeName::default().palette();

    push_wrapped_detail_field(&mut lines, "Field", "abcd", 16, theme);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].spans[0].content, "Field         ");
    assert_eq!(lines[1].spans[0].content, "              ");
}
