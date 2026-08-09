use super::*;

#[test]
fn selectable_detail_field_keeps_its_complete_copy_value() {
    let field = DetailLine::field("Session ID", "native-session-with-full-identity", true);

    assert_eq!(
        field.copy_value(),
        Some(("Session ID", "native-session-with-full-identity".into()))
    );
}

#[test]
fn unavailable_detail_field_is_not_selectable() {
    let field = DetailLine::unavailable("Worktree", "N/A (non-Git Project)");

    assert!(!field.is_selectable());
    assert_eq!(field.copy_value(), None);
}
