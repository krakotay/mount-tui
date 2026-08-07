use super::*;

/// Shared single-line editor used by every mount form.
pub(super) fn handle_line_editor_key(
    value: &mut String,
    cursor: &mut usize,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Left => *cursor = cursor.saturating_sub(1),
        KeyCode::Right => *cursor = (*cursor + 1).min(value.chars().count()),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = value.chars().count(),
        KeyCode::Backspace => remove_char_before(value, cursor),
        KeyCode::Delete => remove_char_at(value, *cursor),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            value.clear();
            *cursor = 0;
        }
        KeyCode::Char(ch)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            insert_char_at(value, cursor, ch);
        }
        _ => return false,
    }
    true
}
