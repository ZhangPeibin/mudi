use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn dispatch(
    editor: &mut PropertyEditor,
    code: KeyCode,
    activation: PropertyActivation,
) -> PropertyEditorEvent {
    editor.handle_key(key(code), activation)
}

#[test]
fn browsing_owns_focus_submit_and_close_commands() {
    let mut editor = PropertyEditor::new(4);

    assert_eq!(
        dispatch(&mut editor, KeyCode::Up, PropertyActivation::Action),
        PropertyEditorEvent::None
    );
    assert_eq!(editor.focused(), 3);
    assert_eq!(
        dispatch(&mut editor, KeyCode::Enter, PropertyActivation::Action),
        PropertyEditorEvent::Action { field: 3 }
    );
    assert_eq!(
        dispatch(&mut editor, KeyCode::Char('y'), PropertyActivation::Action),
        PropertyEditorEvent::Submit
    );
    assert_eq!(
        dispatch(&mut editor, KeyCode::Char('q'), PropertyActivation::Action),
        PropertyEditorEvent::Close
    );
}

#[test]
fn browsing_supports_vim_focus_navigation() {
    let mut editor = PropertyEditor::new(3);

    dispatch(&mut editor, KeyCode::Char('j'), PropertyActivation::Action);
    assert_eq!(editor.focused(), 1);
    dispatch(&mut editor, KeyCode::Char('k'), PropertyActivation::Action);
    assert_eq!(editor.focused(), 0);
    dispatch(&mut editor, KeyCode::Char('k'), PropertyActivation::Action);
    assert_eq!(editor.focused(), 2);
}

#[test]
fn text_edit_commit_and_cancel_do_not_change_outer_focus() {
    let mut editor = PropertyEditor::new(2);
    editor.focus(1);
    dispatch(
        &mut editor,
        KeyCode::Enter,
        PropertyActivation::Text("before".into()),
    );
    for character in " yq".chars() {
        dispatch(
            &mut editor,
            KeyCode::Char(character),
            PropertyActivation::Action,
        );
    }
    assert_eq!(
        dispatch(&mut editor, KeyCode::Enter, PropertyActivation::Action),
        PropertyEditorEvent::TextCommitted {
            field: 1,
            value: "before yq".into(),
        }
    );
    assert!(editor.is_browsing());
    assert_eq!(editor.focused(), 1);

    dispatch(
        &mut editor,
        KeyCode::Enter,
        PropertyActivation::Text("kept".into()),
    );
    dispatch(&mut editor, KeyCode::Char('x'), PropertyActivation::Action);
    assert_eq!(
        dispatch(&mut editor, KeyCode::Esc, PropertyActivation::Action),
        PropertyEditorEvent::TextCancelled { field: 1 }
    );
    assert!(editor.is_browsing());
}

#[test]
fn unicode_cursor_backspace_and_delete_keep_valid_text() {
    let mut editor = PropertyEditor::new(1);
    dispatch(
        &mut editor,
        KeyCode::Enter,
        PropertyActivation::Text("甲乙c".into()),
    );
    dispatch(&mut editor, KeyCode::Left, PropertyActivation::Action);
    dispatch(&mut editor, KeyCode::Backspace, PropertyActivation::Action);
    dispatch(&mut editor, KeyCode::Char('文'), PropertyActivation::Action);
    dispatch(&mut editor, KeyCode::Home, PropertyActivation::Action);
    dispatch(&mut editor, KeyCode::Delete, PropertyActivation::Action);

    assert_eq!(
        dispatch(&mut editor, KeyCode::Enter, PropertyActivation::Action),
        PropertyEditorEvent::TextCommitted {
            field: 0,
            value: "文c".into(),
        }
    );
}

#[test]
fn choice_dialog_consumes_shortcuts_until_confirmed_or_cancelled() {
    let mut editor = PropertyEditor::new(1);
    dispatch(
        &mut editor,
        KeyCode::Enter,
        PropertyActivation::Choice {
            option_count: 2,
            cursor: 0,
        },
    );
    assert_eq!(editor.choice().unwrap().cursor, 0);
    assert_eq!(
        dispatch(&mut editor, KeyCode::Char('q'), PropertyActivation::Action),
        PropertyEditorEvent::None
    );
    dispatch(&mut editor, KeyCode::Down, PropertyActivation::Action);
    assert_eq!(
        dispatch(&mut editor, KeyCode::Enter, PropertyActivation::Action),
        PropertyEditorEvent::ChoiceConfirmed {
            field: 0,
            choice: 1,
        }
    );
    assert!(editor.is_browsing());

    dispatch(
        &mut editor,
        KeyCode::Enter,
        PropertyActivation::Choice {
            option_count: 2,
            cursor: 1,
        },
    );
    assert_eq!(
        dispatch(&mut editor, KeyCode::Esc, PropertyActivation::Action),
        PropertyEditorEvent::ChoiceCancelled { field: 0 }
    );
    assert!(editor.is_browsing());
}
