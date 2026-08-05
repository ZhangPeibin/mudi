use super::*;

#[test]
fn one_hundred_percent_preserves_the_complete_area() {
    let area = Rect::new(7, 11, 120, 30);

    assert_eq!(centered_rect(100, 100, area), area);
}

#[test]
fn fifty_percent_is_centered_on_both_axes() {
    let area = Rect::new(0, 0, 100, 40);

    assert_eq!(centered_rect(50, 50, area), Rect::new(25, 10, 50, 20));
}
