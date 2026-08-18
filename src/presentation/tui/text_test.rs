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
fn fit_right_pads_short_values_and_keeps_a_fixed_terminal_width() {
    assert_eq!(fit_right("5%", 4, true), "  5%");
    assert_eq!(fit_right("100%", 4, true), "100%");
    assert_eq!(
        UnicodeWidthStr::width(fit_right("你好世界", 5, true).as_str()),
        5
    );
}

#[test]
fn shortening_can_preserve_either_identity_edge() {
    assert_eq!(shorten_start("abcdefgh", 6), "abc...");
    assert_eq!(shorten_end("abcdefgh", 6), "...fgh");
}
