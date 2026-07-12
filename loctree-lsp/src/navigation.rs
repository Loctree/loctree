//! Navigation utilities for loctree LSP
//!
//! Provides word extraction at the cursor position, used by the
//! `code_action` handler to detect the symbol under the cursor.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use tower_lsp::lsp_types::Position;

/// Extract word at cursor position from document text
///
/// Returns the identifier under the cursor, handling common identifier characters.
pub fn get_word_at_position(text: &str, position: Position) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = position.line as usize;

    if line_idx >= lines.len() {
        return None;
    }

    let line = lines[line_idx];
    let char_idx = position.character as usize;

    if char_idx > line.len() {
        return None;
    }

    // Find word boundaries
    let chars: Vec<char> = line.chars().collect();

    // Check if cursor is on an identifier character
    // If not (e.g., on whitespace or punctuation), return None
    if !is_identifier_char(chars.get(char_idx).copied()) {
        return None;
    }

    // Find start of word
    let mut start = char_idx;
    while start > 0 && is_identifier_char(chars.get(start - 1).copied()) {
        start -= 1;
    }

    // Find end of word
    let mut end = char_idx;
    while end < chars.len() && is_identifier_char(chars.get(end).copied()) {
        end += 1;
    }

    if start == end {
        return None;
    }

    let word: String = chars[start..end].iter().collect();
    if word.is_empty() { None } else { Some(word) }
}

/// Check if a character is valid in an identifier
fn is_identifier_char(c: Option<char>) -> bool {
    match c {
        Some(ch) => ch.is_alphanumeric() || ch == '_' || ch == '$',
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_word_at_position_simple() {
        let text = "import { foo } from './bar'";
        let pos = Position {
            line: 0,
            character: 9,
        }; // on 'foo'
        assert_eq!(get_word_at_position(text, pos), Some("foo".to_string()));
    }

    #[test]
    fn test_get_word_at_position_start_of_word() {
        let text = "const myVariable = 42";
        let pos = Position {
            line: 0,
            character: 6,
        }; // at 'm' of myVariable
        assert_eq!(
            get_word_at_position(text, pos),
            Some("myVariable".to_string())
        );
    }

    #[test]
    fn test_get_word_at_position_end_of_word() {
        let text = "const myVariable = 42";
        let pos = Position {
            line: 0,
            character: 15,
        }; // at 'e' of myVariable
        assert_eq!(
            get_word_at_position(text, pos),
            Some("myVariable".to_string())
        );
    }

    #[test]
    fn test_get_word_at_position_on_space() {
        let text = "const foo = bar";
        let pos = Position {
            line: 0,
            character: 5,
        }; // on space
        assert_eq!(get_word_at_position(text, pos), None);
    }

    #[test]
    fn test_get_word_at_position_multiline() {
        let text = "first line\nsecond line";
        let pos = Position {
            line: 1,
            character: 0,
        }; // at 's' of second
        assert_eq!(get_word_at_position(text, pos), Some("second".to_string()));
    }

    #[test]
    fn test_get_word_with_underscore() {
        let text = "const my_var = 42";
        let pos = Position {
            line: 0,
            character: 8,
        }; // in 'my_var'
        assert_eq!(get_word_at_position(text, pos), Some("my_var".to_string()));
    }

    #[test]
    fn test_get_word_with_dollar() {
        let text = "const $element = document";
        let pos = Position {
            line: 0,
            character: 6,
        }; // on '$element'
        assert_eq!(
            get_word_at_position(text, pos),
            Some("$element".to_string())
        );
    }
}
