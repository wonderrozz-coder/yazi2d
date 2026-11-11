//! Layout-independent shortcuts.
//!
//! On macOS the terminal reports the character produced by the active layout.
//! With the Russian (ЙЦУКЕН) layout every letter shortcut stops matching its
//! Latin binding, so `Ctrl+B` becomes `Ctrl+и` and `s` becomes `ы`. This module
//! maps a Cyrillic character back to the Latin character on the same physical
//! key, so the same key works in both layouts.
//!
//! The mapping is applied to shortcut handling only. Text prompts keep the raw
//! character, so typing Russian file names still works.

use crossterm::event::{KeyCode, KeyEvent};

/// `(Cyrillic, Latin)` pairs for the standard ЙЦУКЕН layout.
const RU_TO_LATIN: [(char, char); 33] = [
    ('й', 'q'),
    ('ц', 'w'),
    ('у', 'e'),
    ('к', 'r'),
    ('е', 't'),
    ('н', 'y'),
    ('г', 'u'),
    ('ш', 'i'),
    ('щ', 'o'),
    ('з', 'p'),
    ('х', '['),
    ('ъ', ']'),
    ('ф', 'a'),
    ('ы', 's'),
    ('в', 'd'),
    ('а', 'f'),
    ('п', 'g'),
    ('р', 'h'),
    ('о', 'j'),
    ('л', 'k'),
    ('д', 'l'),
    ('ж', ';'),
    ('э', '\''),
    ('я', 'z'),
    ('ч', 'x'),
    ('с', 'c'),
    ('м', 'v'),
    ('и', 'b'),
    ('т', 'n'),
    ('ь', 'm'),
    ('б', ','),
    ('ю', '.'),
    ('ё', '`'),
];

/// Latin character on the same physical key, preserving case.
pub fn latin_equivalent(c: char) -> Option<char> {
    let lower = c.to_lowercase().next()?;
    let latin = RU_TO_LATIN
        .iter()
        .find_map(|(ru, en)| (*ru == lower).then_some(*en))?;

    if c.is_uppercase() {
        Some(latin.to_ascii_uppercase())
    } else {
        Some(latin)
    }
}

/// Rewrite a Cyrillic character key into its Latin equivalent.
///
/// Modifiers are preserved, so `Ctrl+и` becomes `Ctrl+b`.
pub fn normalize(key: KeyEvent) -> KeyEvent {
    let KeyCode::Char(c) = key.code else {
        return key;
    };
    match latin_equivalent(c) {
        Some(latin) => KeyEvent {
            code: KeyCode::Char(latin),
            ..key
        },
        None => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn maps_russian_letters_to_their_physical_keys() {
        assert_eq!(latin_equivalent('и'), Some('b'));
        assert_eq!(latin_equivalent('ы'), Some('s'));
        assert_eq!(latin_equivalent('в'), Some('d'));
        assert_eq!(latin_equivalent('п'), Some('g'));
        assert_eq!(latin_equivalent('л'), Some('k'));
        assert_eq!(latin_equivalent('я'), Some('z'));
        assert_eq!(latin_equivalent('ё'), Some('`'));
    }

    #[test]
    fn keeps_case() {
        assert_eq!(latin_equivalent('И'), Some('B'));
        assert_eq!(latin_equivalent('П'), Some('G'));
        assert_eq!(latin_equivalent('Л'), Some('K'));
    }

    #[test]
    fn leaves_latin_and_symbols_alone() {
        assert_eq!(latin_equivalent('b'), None);
        assert_eq!(latin_equivalent('G'), None);
        assert_eq!(latin_equivalent('?'), None);
        assert_eq!(latin_equivalent('§'), None);
    }

    #[test]
    fn normalizes_ctrl_chords() {
        let key = KeyEvent::new(KeyCode::Char('и'), KeyModifiers::CONTROL);
        let normalized = normalize(key);
        assert_eq!(normalized.code, KeyCode::Char('b'));
        assert_eq!(normalized.modifiers, KeyModifiers::CONTROL);
    }
}
