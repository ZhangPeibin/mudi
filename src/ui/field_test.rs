use super::*;
use crate::ui::ThemeName;

#[test]
fn explicit_label_width_aligns_and_pads_a_narrow_value() {
    let line = FieldLine::new("Model", "gpt")
        .label_width(10)
        .render(18, ThemeName::SessionMesh.palette())
        .to_string();

    assert_eq!(line, "Model      gpt    ");
}

#[test]
fn keep_end_preserves_complete_double_width_characters() {
    let line = FieldLine::new("模型", "abcdef文档整理")
        .label_width(6)
        .keep_end()
        .render(17, ThemeName::SessionMesh.palette())
        .to_string();

    assert_eq!(line, "模型   ...档整理 ");
}

#[test]
fn field_line_never_exceeds_a_width_narrower_than_its_label() {
    let line = FieldLine::new("Model", "gpt")
        .label_width(10)
        .render(4, ThemeName::SessionMesh.palette())
        .to_string();

    assert_eq!(line, "M...");
}

#[test]
fn default_truncation_preserves_the_start_of_the_value() {
    let line = FieldLine::new("Model", "abcdefghij")
        .label_width(5)
        .render(12, ThemeName::SessionMesh.palette())
        .to_string();

    assert_eq!(line, "Model abc...");
}
