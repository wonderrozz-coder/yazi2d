//! Persistent user settings: key bindings and trackpad behaviour.
//!
//! Bindings are stored as action id → key spec (`"ctrl+b"`, `"f5"`, `"§"`) in
//! `$YAZI2D_CONFIG_DIR/settings.toml`. An action may have several default keys;
//! a user override replaces all of them, which is what the settings menu edits.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Shortcuts the user can rebind from the settings menu.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Action {
    Quit,
    Help,
    Copy,
    Move,
    Mkdir,
    Rename,
    Delete,
    SelectAll,
    BulkRename,
    ToggleHidden,
    Refresh,
    SortMenu,
    ColourMenu,
    Settings,
    TogglePreview,
    ToggleThumbnails,
    OpenInOther,
    AddFavorite,
    FocusFavorites,
}

impl Action {
    /// Every rebindable action, in the order shown by the settings screen.
    ///
    /// `Settings` is deliberately absent: rebinding the key that opens the
    /// settings menu would make the menu unreachable.
    pub const ALL: [Action; 18] = [
        Action::Quit,
        Action::Help,
        Action::Copy,
        Action::Move,
        Action::Mkdir,
        Action::Rename,
        Action::Delete,
        Action::SelectAll,
        Action::BulkRename,
        Action::ToggleHidden,
        Action::Refresh,
        Action::SortMenu,
        Action::ColourMenu,
        Action::TogglePreview,
        Action::ToggleThumbnails,
        Action::OpenInOther,
        Action::AddFavorite,
        Action::FocusFavorites,
    ];

    /// Stable identifier used in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Help => "help",
            Action::Copy => "copy",
            Action::Move => "move",
            Action::Mkdir => "mkdir",
            Action::Rename => "rename",
            Action::Delete => "delete",
            Action::SelectAll => "select_all",
            Action::BulkRename => "bulk_rename",
            Action::ToggleHidden => "toggle_hidden",
            Action::Refresh => "refresh",
            Action::SortMenu => "sort_menu",
            Action::ColourMenu => "colour_menu",
            Action::Settings => "settings",
            Action::TogglePreview => "toggle_preview",
            Action::ToggleThumbnails => "toggle_thumbnails",
            Action::OpenInOther => "open_in_other",
            Action::AddFavorite => "add_favorite",
            Action::FocusFavorites => "focus_favorites",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Quit => "Quit",
            Action::Help => "Help",
            Action::Copy => "Copy to other panel",
            Action::Move => "Move to other panel",
            Action::Mkdir => "Create folder",
            Action::Rename => "Rename",
            Action::Delete => "Delete",
            Action::SelectAll => "Select all",
            Action::BulkRename => "Bulk rename",
            Action::ToggleHidden => "Hidden files",
            Action::Refresh => "Refresh panels",
            Action::SortMenu => "Sort menu",
            Action::ColourMenu => "Colour menu",
            Action::Settings => "Settings menu",
            Action::TogglePreview => "Preview other panel",
            Action::ToggleThumbnails => "Thumbnail grid",
            Action::OpenInOther => "Open folder there",
            Action::AddFavorite => "Add to favorites",
            Action::FocusFavorites => "Focus favorites",
        }
    }

    /// Built-in keys for this action.
    pub fn defaults(self) -> Vec<KeySpec> {
        match self {
            Action::Quit => vec![KeySpec::ctrl('c'), KeySpec::plain_char('q')],
            Action::Help => vec![KeySpec::plain_char('?'), KeySpec::f(1)],
            Action::Copy => vec![KeySpec::f(5), KeySpec::plain_char('c')],
            Action::Move => vec![KeySpec::f(6), KeySpec::plain_char('m')],
            Action::Mkdir => vec![KeySpec::f(7), KeySpec::plain_char('n')],
            Action::Rename => vec![KeySpec::f(2), KeySpec::plain_char('r')],
            Action::Delete => vec![
                KeySpec::f(8),
                KeySpec::plain_char('d'),
                KeySpec::code(KeyCode::Delete),
            ],
            Action::SelectAll => vec![KeySpec::ctrl('a')],
            Action::BulkRename => vec![KeySpec::ctrl('g')],
            Action::ToggleHidden => vec![KeySpec::ctrl('h'), KeySpec::plain_char('.')],
            Action::Refresh => vec![KeySpec::ctrl('r')],
            Action::SortMenu => vec![KeySpec::plain_char('s'), KeySpec::ctrl('s')],
            Action::ColourMenu => vec![KeySpec::ctrl('k')],
            Action::Settings => vec![KeySpec::ctrl('e')],
            Action::TogglePreview => vec![KeySpec::ctrl('q'), KeySpec::ctrl('p')],
            Action::ToggleThumbnails => vec![KeySpec::ctrl('t')],
            Action::OpenInOther => vec![KeySpec::ctrl('o')],
            Action::AddFavorite => vec![KeySpec::ctrl('b')],
            Action::FocusFavorites => vec![
                KeySpec::ctrl('d'),
                KeySpec::code(KeyCode::BackTab),
                KeySpec::plain_char('§'),
            ],
        }
    }
}

/// One key combination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeySpec {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeySpec {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub const fn plain_char(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub const fn ctrl(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    pub const fn f(n: u8) -> Self {
        Self::new(KeyCode::F(n), KeyModifiers::NONE)
    }

    pub const fn code(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }

    /// Does this spec describe the pressed key?
    ///
    /// Only the modifiers a terminal actually reports matter: on macOS `Shift`
    /// is folded into the character, so a plain `?` arrives without a modifier
    /// and must still match a `?` binding.
    pub fn matches(self, key: KeyEvent) -> bool {
        if self.code != key.code {
            return false;
        }
        if self.modifiers == KeyModifiers::NONE {
            // Accept stray SHIFT on characters, but not CONTROL/ALT.
            return !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        }
        let wanted = self.modifiers.difference(KeyModifiers::SHIFT);
        key.modifiers.contains(wanted)
    }

    /// Build a spec from a pressed key, dropping modifiers we cannot persist.
    pub fn from_event(key: KeyEvent) -> Self {
        let mut modifiers = KeyModifiers::NONE;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            modifiers |= KeyModifiers::CONTROL;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            modifiers |= KeyModifiers::ALT;
        }
        Self::new(key.code, modifiers)
    }

    /// Human-readable form, also used as the settings file value.
    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift".to_string());
        }
        parts.push(code_name(self.code));
        parts.join("+")
    }

    /// Parse the label form back into a spec.
    pub fn parse(text: &str) -> Option<Self> {
        let mut modifiers = KeyModifiers::NONE;
        let mut code = None;

        for part in text.trim().split('+') {
            let lower = part.trim().to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "alt" | "option" | "opt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                _ => {
                    if code.is_some() {
                        return None;
                    }
                    code = Some(parse_code(part.trim())?);
                }
            }
        }
        Some(Self::new(code?, modifiers))
    }
}

fn code_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "backtab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pgup".to_string(),
        KeyCode::PageDown => "pgdn".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

fn parse_code(text: &str) -> Option<KeyCode> {
    let lower = text.to_ascii_lowercase();
    let code = match lower.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "esc" | "escape" => KeyCode::Esc,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "space" => KeyCode::Char(' '),
        _ => {
            if let Some(number) = lower.strip_prefix('f') {
                if let Ok(n) = number.parse::<u8>() {
                    return (1..=24).contains(&n).then_some(KeyCode::F(n));
                }
            }
            let mut chars = text.chars();
            let first = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(first)
        }
    };
    Some(code)
}

/// User settings persisted between runs.
pub struct Settings {
    /// Reverse the two-finger vertical direction.
    pub invert_scroll: bool,
    /// Per-action overrides; an empty map means "use the defaults".
    pub bindings: BTreeMap<String, KeySpec>,
    file: PathBuf,
}

impl Settings {
    /// Load settings, falling back to defaults when the file is missing.
    pub fn load() -> Self {
        let file = settings_file();
        let text = fs::read_to_string(&file).unwrap_or_default();
        let mut invert_scroll = true;
        let mut bindings = BTreeMap::new();

        for (key, value) in parse_toml(&text) {
            if key == "invert_scroll" {
                invert_scroll = matches!(value.as_str(), "true" | "1" | "on" | "yes");
            } else if let Some(action) = key.strip_prefix("key.") {
                if let Some(spec) = KeySpec::parse(&value) {
                    bindings.insert(action.to_string(), spec);
                }
            }
        }

        Self {
            invert_scroll,
            bindings,
            file,
        }
    }

    /// Persist the current settings; failures are reported by the caller.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = format!("invert_scroll = {}\n", self.invert_scroll);
        for (action, spec) in &self.bindings {
            text.push_str(&format!("key.{action} = \"{}\"\n", spec.label()));
        }
        fs::write(&self.file, text)
    }

    /// Keys currently bound to an action, override first.
    pub fn keys_for(&self, action: Action) -> Vec<KeySpec> {
        match self.bindings.get(action.id()) {
            Some(spec) => vec![*spec],
            None => action.defaults(),
        }
    }

    /// Replace every binding of an action with a single key.
    ///
    /// Returns `false` when the combination is reserved, so the menu can
    /// explain why nothing changed.
    pub fn rebind(&mut self, action: Action, spec: KeySpec) -> bool {
        if is_reserved(spec) || action == Action::Settings {
            return false;
        }
        self.bindings.insert(action.id().to_string(), spec);
        true
    }

    /// Drop the override so the built-in keys apply again.
    pub fn reset(&mut self, action: Action) {
        self.bindings.remove(action.id());
    }

    pub fn is_custom(&self, action: Action) -> bool {
        self.bindings.contains_key(action.id())
    }

    /// Find the action bound to a key (used by tests and future key hints).
    #[allow(dead_code)]
    pub fn action_for(&self, key: KeyEvent) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|action| self.keys_for(*action).iter().any(|spec| spec.matches(key)))
    }

    /// Display string for the current binding of an action.
    pub fn keys_label(&self, action: Action) -> String {
        self.keys_for(action)
            .iter()
            .map(|spec| spec.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Keys that the app relies on being always available.
pub fn is_reserved(spec: KeySpec) -> bool {
    matches!(
        (spec.code, spec.modifiers),
        (KeyCode::Char('e'), m) if m.contains(KeyModifiers::CONTROL)
    ) || spec.code == KeyCode::Esc
}

fn settings_file() -> PathBuf {
    crate::favorites::config_dir().join("settings.toml")
}

/// Minimal `key = value` reader.
///
/// The settings file only ever contains flat string/bool pairs, so a full TOML
/// dependency would be overkill; the same parser is used for the tests.
fn parse_toml(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        pairs.push((key.trim().to_string(), value.to_string()));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_key_specs() {
        let spec = KeySpec::parse("ctrl+b").unwrap();
        assert_eq!(spec, KeySpec::ctrl('b'));
        assert_eq!(spec.label(), "ctrl+b");

        let f5 = KeySpec::parse("f5").unwrap();
        assert_eq!(f5, KeySpec::f(5));
        assert_eq!(f5.label(), "f5");

        let section = KeySpec::parse("§").unwrap();
        assert_eq!(section, KeySpec::plain_char('§'));
        assert_eq!(section.label(), "§");

        let backtab = KeySpec::parse("backtab").unwrap();
        assert_eq!(backtab.code, KeyCode::BackTab);
    }

    #[test]
    fn matching_ignores_folded_shift_but_not_ctrl() {
        let question = KeySpec::plain_char('?');
        assert!(question.matches(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)));
        assert!(question.matches(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)));
        assert!(!question.matches(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::CONTROL)));

        let ctrl_b = KeySpec::ctrl('b');
        assert!(ctrl_b.matches(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)));
        assert!(!ctrl_b.matches(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)));
    }

    #[test]
    fn settings_round_trip_through_the_file_writer() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.toml");
        let text = "invert_scroll = false\nkey.copy = \"ctrl+y\"\nkey.delete = \"f9\"\n";
        let pairs = parse_toml(text);
        assert_eq!(
            pairs,
            vec![
                ("invert_scroll".to_string(), "false".to_string()),
                ("key.copy".to_string(), "ctrl+y".to_string()),
                ("key.delete".to_string(), "f9".to_string()),
            ]
        );
        fs::write(&file, text).unwrap();
        assert!(file.exists());
    }

    #[test]
    fn overrides_replace_the_default_keys() {
        let mut settings = Settings {
            invert_scroll: true,
            bindings: BTreeMap::new(),
            file: PathBuf::from("/dev/null"),
        };
        assert!(settings
            .keys_for(Action::Copy)
            .iter()
            .any(|spec| *spec == KeySpec::f(5)));

        assert!(settings.rebind(Action::Copy, KeySpec::ctrl('y')));
        assert_eq!(settings.keys_for(Action::Copy), vec![KeySpec::ctrl('y')]);
        assert!(settings.is_custom(Action::Copy));

        settings.reset(Action::Copy);
        assert!(!settings.is_custom(Action::Copy));
        assert_eq!(settings.keys_for(Action::Copy), Action::Copy.defaults());
    }

    #[test]
    fn finds_the_action_for_a_key() {
        let settings = Settings {
            invert_scroll: true,
            bindings: BTreeMap::new(),
            file: PathBuf::from("/dev/null"),
        };
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            Some(Action::Copy)
        );
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn every_action_has_a_distinct_id_and_default() {
        let mut ids = std::collections::BTreeSet::new();
        for action in Action::ALL {
            assert!(ids.insert(action.id()), "duplicate id: {}", action.id());
            assert!(!action.defaults().is_empty());
            assert!(!action.label().is_empty());
        }
    }
}
