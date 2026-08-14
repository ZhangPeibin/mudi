use super::*;
use crate::ui::ThemeName;
use ratatui::style::{Color, Modifier, Style};

#[test]
fn explicit_label_width_aligns_and_pads_a_narrow_value() {
    let line = FieldLine::new("Model", "gpt")
        .label_width(10)
        .render(18, ThemeName::Mudi.palette())
        .to_string();

    assert_eq!(line, "Model      gpt    ");
}

#[test]
fn keep_end_preserves_complete_double_width_characters() {
    let line = FieldLine::new("模型", "abcdef文档整理")
        .label_width(6)
        .keep_end()
        .render(17, ThemeName::Mudi.palette())
        .to_string();

    assert_eq!(line, "模型   ...档整理 ");
}

#[test]
fn field_line_never_exceeds_a_width_narrower_than_its_label() {
    let line = FieldLine::new("Model", "gpt")
        .label_width(10)
        .render(4, ThemeName::Mudi.palette())
        .to_string();

    assert_eq!(line, "M...");
}

#[test]
fn default_truncation_preserves_the_start_of_the_value() {
    let line = FieldLine::new("Model", "abcdefghij")
        .label_width(5)
        .render(12, ThemeName::Mudi.palette())
        .to_string();

    assert_eq!(line, "Model abc...");
}

#[test]
fn explicit_value_style_preserves_semantic_emphasis() {
    let expected = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let line = FieldLine::new("Attention", "Critical Context")
        .value_style(expected)
        .render(40, ThemeName::Mudi.palette());

    assert_eq!(line.spans[1].style, expected);
}
