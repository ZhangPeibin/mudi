use super::*;

#[test]
fn fit_preserves_terminal_width_for_ascii_and_wide_unicode() {
    assert_eq!(UnicodeWidthStr::width(fit("title", 8, false).as_str()), 8);
    assert_eq!(
        UnicodeWidthStr::width(fit("你好世界", 7, false).as_str()),
        7
    );
}

#[test]
fn visible_input_tail_keeps_the_latest_complete_characters() {
    assert_eq!(visible_input_tail("ab你好", 5), "b你好");
    assert_eq!(visible_input_tail("ab你好", 4), "你好");
}

#[test]
fn shortening_can_preserve_either_identity_edge() {
    assert_eq!(shorten_start("abcdefgh", 6), "abc...");
    assert_eq!(shorten_end("abcdefgh", 6), "...fgh");
}
