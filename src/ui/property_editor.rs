//! Reusable interaction state for compact property editors.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PropertyEditor {
    field_count: usize,
    focused: usize,
    interaction: Interaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Interaction {
    Browsing,
    Editing(TextDraft),
    Choosing(ChoiceState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextDraft {
    field: usize,
    value: String,
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChoiceState {
    field: usize,
    cursor: usize,
    option_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PropertyActivation {
    Text(String),
    Choice {
        option_count: usize,
        cursor: usize,
    },
    #[allow(dead_code)]
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PropertyEditorEvent {
    None,
    Close,
    Submit,
    Action { field: usize },
    TextCommitted { field: usize, value: String },
    TextCancelled { field: usize },
    ChoiceConfirmed { field: usize, choice: usize },
    ChoiceCancelled { field: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ChoiceView {
    pub(super) field: usize,
    pub(super) cursor: usize,
    pub(super) option_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DraftWindow<'a> {
    draft: &'a TextDraft,
}

impl PropertyEditor {
    pub(super) fn new(field_count: usize) -> Self {
        assert!(
            field_count > 0,
            "a property editor needs at least one field"
        );
        Self {
            field_count,
            focused: 0,
            interaction: Interaction::Browsing,
        }
    }

    pub(super) fn focused(&self) -> usize {
        self.focused
    }

    pub(super) fn focus(&mut self, field: usize) {
        self.focused = field.min(self.field_count - 1);
        self.interaction = Interaction::Browsing;
    }

    #[cfg(test)]
    pub(super) fn is_browsing(&self) -> bool {
        matches!(self.interaction, Interaction::Browsing)
    }

    pub(super) fn editing(&self) -> Option<DraftWindow<'_>> {
        match &self.interaction {
            Interaction::Editing(draft) => Some(DraftWindow { draft }),
            _ => None,
        }
    }

    pub(super) fn choice(&self) -> Option<ChoiceView> {
        match self.interaction {
            Interaction::Choosing(choice) => Some(ChoiceView {
                field: choice.field,
                cursor: choice.cursor,
                option_count: choice.option_count,
            }),
            _ => None,
        }
    }

    pub(super) fn handle_key(
        &mut self,
        key: KeyEvent,
        activation: PropertyActivation,
    ) -> PropertyEditorEvent {
        let event = match &mut self.interaction {
            Interaction::Browsing => self.handle_browsing_key(key, activation),
            Interaction::Editing(draft) => Self::handle_editing_key(key, draft),
            Interaction::Choosing(choice) => Self::handle_choice_key(key, choice),
        };
        if matches!(
            event,
            PropertyEditorEvent::TextCommitted { .. }
                | PropertyEditorEvent::TextCancelled { .. }
                | PropertyEditorEvent::ChoiceConfirmed { .. }
                | PropertyEditorEvent::ChoiceCancelled { .. }
        ) {
            self.interaction = Interaction::Browsing;
        }
        event
    }

    fn handle_browsing_key(
        &mut self,
        key: KeyEvent,
        activation: PropertyActivation,
    ) -> PropertyEditorEvent {
        match key.code {
            KeyCode::Up => {
                self.focused = self.focused.checked_sub(1).unwrap_or(self.field_count - 1);
                PropertyEditorEvent::None
            }
            KeyCode::Down => {
                self.focused = (self.focused + 1) % self.field_count;
                PropertyEditorEvent::None
            }
            KeyCode::Enter => match activation {
                PropertyActivation::Text(value) => {
                    let cursor = value.len();
                    self.interaction = Interaction::Editing(TextDraft {
                        field: self.focused,
                        value,
                        cursor,
                    });
                    PropertyEditorEvent::None
                }
                PropertyActivation::Choice {
                    option_count,
                    cursor,
                } if option_count > 0 => {
                    self.interaction = Interaction::Choosing(ChoiceState {
                        field: self.focused,
                        cursor: cursor.min(option_count - 1),
                        option_count,
                    });
                    PropertyEditorEvent::None
                }
                PropertyActivation::Choice { .. } => PropertyEditorEvent::None,
                PropertyActivation::Action => PropertyEditorEvent::Action {
                    field: self.focused,
                },
            },
            KeyCode::Char('y') if unmodified_command(key.modifiers) => PropertyEditorEvent::Submit,
            KeyCode::Esc | KeyCode::Char('q') if unmodified_command(key.modifiers) => {
                PropertyEditorEvent::Close
            }
            _ => PropertyEditorEvent::None,
        }
    }

    fn handle_editing_key(key: KeyEvent, draft: &mut TextDraft) -> PropertyEditorEvent {
        match key.code {
            KeyCode::Enter => PropertyEditorEvent::TextCommitted {
                field: draft.field,
                value: draft.value.clone(),
            },
            KeyCode::Esc => PropertyEditorEvent::TextCancelled { field: draft.field },
            KeyCode::Left => {
                draft.cursor = previous_boundary(&draft.value, draft.cursor);
                PropertyEditorEvent::None
            }
            KeyCode::Right => {
                draft.cursor = next_boundary(&draft.value, draft.cursor);
                PropertyEditorEvent::None
            }
            KeyCode::Home => {
                draft.cursor = 0;
                PropertyEditorEvent::None
            }
            KeyCode::End => {
                draft.cursor = draft.value.len();
                PropertyEditorEvent::None
            }
            KeyCode::Backspace => {
                let previous = previous_boundary(&draft.value, draft.cursor);
                if previous != draft.cursor {
                    draft.value.replace_range(previous..draft.cursor, "");
                    draft.cursor = previous;
                }
                PropertyEditorEvent::None
            }
            KeyCode::Delete => {
                let next = next_boundary(&draft.value, draft.cursor);
                if next != draft.cursor {
                    draft.value.replace_range(draft.cursor..next, "");
                }
                PropertyEditorEvent::None
            }
            KeyCode::Char(character)
                if !character.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                draft.value.insert(draft.cursor, character);
                draft.cursor += character.len_utf8();
                PropertyEditorEvent::None
            }
            _ => PropertyEditorEvent::None,
        }
    }

    fn handle_choice_key(key: KeyEvent, choice: &mut ChoiceState) -> PropertyEditorEvent {
        match key.code {
            KeyCode::Up => {
                choice.cursor = choice
                    .cursor
                    .checked_sub(1)
                    .unwrap_or(choice.option_count - 1);
                PropertyEditorEvent::None
            }
            KeyCode::Down => {
                choice.cursor = (choice.cursor + 1) % choice.option_count;
                PropertyEditorEvent::None
            }
            KeyCode::Enter => PropertyEditorEvent::ChoiceConfirmed {
                field: choice.field,
                choice: choice.cursor,
            },
            KeyCode::Esc => PropertyEditorEvent::ChoiceCancelled {
                field: choice.field,
            },
            _ => PropertyEditorEvent::None,
        }
    }
}

impl DraftWindow<'_> {
    pub(super) fn field(self) -> usize {
        self.draft.field
    }

    pub(super) fn visible(self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }

        let before = &self.draft.value[..self.draft.cursor];
        let after = &self.draft.value[self.draft.cursor..];
        let mut visible_before = Vec::new();
        let mut before_width = 0;
        for character in before.chars().rev() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if before_width + character_width > width {
                break;
            }
            visible_before.push(character);
            before_width += character_width;
        }
        visible_before.reverse();

        let mut visible_after = String::new();
        let mut remaining = width.saturating_sub(before_width);
        for character in after.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if character_width > remaining {
                break;
            }
            visible_after.push(character);
            remaining -= character_width;
        }

        (
            format!(
                "{}{}",
                visible_before.into_iter().collect::<String>(),
                visible_after
            ),
            before_width,
        )
    }
}

fn unmodified_command(modifiers: KeyModifiers) -> bool {
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn previous_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map_or(cursor, |(index, _)| index)
}

fn next_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .chars()
        .next()
        .map_or(cursor, |character| cursor + character.len_utf8())
}

#[cfg(test)]
#[path = "property_editor_test.rs"]
mod tests;
