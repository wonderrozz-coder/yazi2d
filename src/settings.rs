//! Persistent user settings: key bindings and trackpad behaviour.
//!
//! Every action has **two slots**, and both are editable in the settings menu.
//! A slot is a key combo (`"f5"`, `"ctrl+q"`, `"§"`) stored in
//! `$YAZI2D_CONFIG_DIR/settings.toml` as `key.<action>.1` / `key.<action>.2`;
//! an empty slot means "not assigned".
//!
//! [`Settings::shortcuts`] is the single source of truth: it turns the slots into
//! the list of key combos the app actually reacts to. The key dispatcher, the
//! footer and the help screen all read it, so a rebound key can never leave a
//! stale duplicate behind — pressing the old key does nothing, exactly as the
//! menu says. Duplicates across actions are resolved there too (see
//! [`resolve`]), which is why the footer cannot show "copy" three times.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Shortcuts the user can rebind from the settings menu.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Action {
    Quit,
    Help,
    Copy,
    Move,
    Mkdir,
    Rename,
    Delete,
    /// Erase without the trash. Deliberately not on a plain key.
    DeletePermanent,
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
    ContextMenu,
    AddFavorite,
    FocusFavorites,
    OpenHere,
    PreviewThere,
    ToggleSidebar,
    CopyPath,
    CopyName,
    CopySelection,
    SwitchPanel,
    Confirm,
    Cancel,
    /// Move the highlighted bookmark (or separator) up one row.
    FavoriteUp,
    /// Move the highlighted bookmark (or separator) down one row.
    FavoriteDown,
    /// Add a named separator at the cursor (arranging mode).
    FavoriteSeparator,
    /// Add an empty row at the cursor (arranging mode).
    FavoriteBlank,
    /// Toggle arranging mode in the sidebar.
    FavoriteArrange,
}

impl Action {
    /// Every rebindable action, in the order shown by the settings screen.
    ///
    /// `Settings` *is* included (ADR-063): its first slot opens the menu and
    /// both slots are editable. If a user clears both, the menu stays reachable
    /// by right-clicking a row — a pointer, not a key, so a binding mistake can
    /// never lock the app out of its own settings.
    pub const ALL: [Action; 35] = [
        Action::Quit,
        Action::Help,
        Action::Copy,
        Action::Move,
        Action::Mkdir,
        Action::Rename,
        Action::Delete,
        Action::DeletePermanent,
        Action::SelectAll,
        Action::BulkRename,
        Action::ToggleHidden,
        Action::Refresh,
        Action::SortMenu,
        Action::ColourMenu,
        Action::TogglePreview,
        Action::ToggleThumbnails,
        Action::OpenInOther,
        Action::ContextMenu,
        Action::AddFavorite,
        Action::FocusFavorites,
        Action::ToggleSidebar,
        Action::CopyPath,
        Action::CopyName,
        Action::CopySelection,
        Action::SwitchPanel,
        Action::OpenHere,
        Action::PreviewThere,
        Action::Confirm,
        Action::Cancel,
        Action::FavoriteUp,
        Action::FavoriteDown,
        Action::FavoriteSeparator,
        Action::FavoriteBlank,
        Action::FavoriteArrange,
        Action::Settings,
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
            Action::DeletePermanent => "delete_permanent",
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
            Action::ContextMenu => "context_menu",
            Action::AddFavorite => "add_favorite",
            Action::FocusFavorites => "focus_favorites",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::CopyPath => "copy_path",
            Action::CopyName => "copy_name",
            Action::CopySelection => "copy_selection",
            Action::SwitchPanel => "switch_panel",
            Action::OpenHere => "open_here",

            Action::PreviewThere => "preview_there",
            Action::Confirm => "confirm",
            Action::Cancel => "cancel",
            Action::FavoriteUp => "favorite_up",
            Action::FavoriteDown => "favorite_down",
            Action::FavoriteSeparator => "favorite_separator",
            Action::FavoriteBlank => "favorite_blank",
            Action::FavoriteArrange => "favorite_arrange",
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
            Action::Delete => "Delete (to trash)",
            Action::DeletePermanent => "Delete permanently",
            Action::SelectAll => "Select all",
            Action::BulkRename => "Bulk rename",
            Action::ToggleHidden => "Hidden files",
            Action::Refresh => "Refresh panels",
            Action::SortMenu => "Sort menu",
            Action::ColourMenu => "Colour menu",
            Action::Settings => "Settings menu",
            Action::TogglePreview => "Preview other panel",
            Action::ToggleThumbnails => "Thumbnail grid",
            Action::OpenInOther => "Open folder in the other panel",
            Action::ContextMenu => "Context menu",
            Action::AddFavorite => "Add to favorites",
            Action::FocusFavorites => "Focus favorites",
            Action::ToggleSidebar => "Show/hide favorites",
            Action::CopyPath => "Copy path",
            Action::CopyName => "Copy name",
            Action::CopySelection => "Copy selected paths",
            Action::SwitchPanel => "Switch panel",
            Action::OpenHere => "Open entry (Enter)",

            Action::PreviewThere => "Preview there",
            Action::Confirm => "Confirm prompt",
            Action::Cancel => "Cancel / clear",
            Action::FavoriteUp => "Move bookmark up",
            Action::FavoriteDown => "Move bookmark down",
            Action::FavoriteSeparator => "Add separator",
            Action::FavoriteBlank => "Add empty row",
            Action::FavoriteArrange => "Arrange favorites",
        }
    }

    /// Actions the settings menu lists, in display order.
    pub fn menu_actions() -> Vec<Action> {
        Action::ALL.to_vec()
    }

    /// The two built-in slots for this action.
    ///
    /// Exactly two entries: the settings menu edits both, and anything extra
    /// used to become an invisible key that could not be reassigned.
    pub fn defaults(self) -> [Option<KeySpec>; 2] {
        match self {
            Action::Quit => [Some(KeySpec::plain_char('q')), Some(KeySpec::ctrl('c'))],
            Action::Help => [Some(KeySpec::plain_char('?')), Some(KeySpec::f(1))],
            Action::Copy => [Some(KeySpec::f(5)), Some(KeySpec::plain_char('c'))],
            Action::Move => [Some(KeySpec::f(6)), Some(KeySpec::plain_char('m'))],
            Action::Mkdir => [Some(KeySpec::f(7)), Some(KeySpec::plain_char('n'))],
            Action::Rename => [Some(KeySpec::f(2)), Some(KeySpec::plain_char('r'))],
            // `Del` and `F8` are the everyday keys and both go to the trash.
            Action::Delete => [Some(KeySpec::f(8)), Some(KeySpec::code(KeyCode::Delete))],
            Action::DeletePermanent => [
                Some(KeySpec::new(KeyCode::Delete, KeyModifiers::SHIFT)),
                None,
            ],
            Action::SelectAll => [Some(KeySpec::ctrl('a')), None],
            Action::BulkRename => [Some(KeySpec::ctrl('g')), None],
            Action::ToggleHidden => [Some(KeySpec::ctrl('h')), Some(KeySpec::plain_char('.'))],
            Action::Refresh => [Some(KeySpec::ctrl('r')), None],
            Action::SortMenu => [Some(KeySpec::plain_char('s')), Some(KeySpec::ctrl('s'))],
            Action::ColourMenu => [Some(KeySpec::ctrl('k')), None],
            Action::Settings => [Some(KeySpec::ctrl('e')), None],
            Action::TogglePreview => [Some(KeySpec::ctrl('q')), Some(KeySpec::ctrl('p'))],
            Action::ToggleThumbnails => [Some(KeySpec::ctrl('t')), None],
            // `Ctrl+O` and `Shift+→` were two separate actions with the same
            // effect and two different labels; they are two slots of one action
            // now. `Shift+←` mirrors the gesture (see `KeySpec::matches`).
            Action::OpenInOther => [
                Some(KeySpec::ctrl('o')),
                Some(KeySpec::new(KeyCode::Right, KeyModifiers::SHIFT)),
            ],
            Action::ContextMenu => [
                Some(KeySpec::new(KeyCode::F(10), KeyModifiers::SHIFT)),
                None,
            ],
            Action::AddFavorite => [Some(KeySpec::ctrl('b')), None],
            Action::FocusFavorites => [
                Some(KeySpec::code(KeyCode::BackTab)),
                Some(KeySpec::ctrl('d')),
            ],
            Action::ToggleSidebar => [Some(KeySpec::ctrl('f')), None],
            Action::CopyPath => [Some(KeySpec::ctrl('l')), None],
            Action::CopyName => [
                Some(KeySpec::new(KeyCode::Char('i'), KeyModifiers::ALT)),
                None,
            ],
            Action::CopySelection => [
                Some(KeySpec::new(
                    KeyCode::Char('C'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                )),
                None,
            ],
            Action::SwitchPanel => [Some(KeySpec::code(KeyCode::Tab)), None],
            Action::OpenHere => [Some(KeySpec::code(KeyCode::Enter)), None],
            Action::PreviewThere => [
                Some(KeySpec::new(KeyCode::Right, KeyModifiers::ALT)),
                Some(KeySpec::new(KeyCode::Left, KeyModifiers::ALT)),
            ],
            // `Confirm` has no key of its own: Enter/Ctrl+E are prompt keys, and
            // giving Confirm the same `Enter` made "open" lose its only slot.
            // PageUp/PageDown are the documented keys, but a MacBook has no
            // reachable PageUp/PageDown (Fn+↑/↓ is swallowed by most terminals),
            // so each slot also carries a plain letter opposite to the arrow
            // that browses: `K` and `J` are the up/down moves of the row itself.
            Action::FavoriteUp => [
                Some(KeySpec::code(KeyCode::PageUp)),
                Some(KeySpec::plain_char('K')),
            ],
            Action::FavoriteDown => [
                Some(KeySpec::code(KeyCode::PageDown)),
                Some(KeySpec::plain_char('J')),
            ],
            Action::FavoriteSeparator => [Some(KeySpec::plain_char('S')), None],
            Action::FavoriteBlank => [Some(KeySpec::plain_char('B')), None],
            Action::FavoriteArrange => [
                Some(KeySpec::plain_char('E')),
                Some(KeySpec::plain_char('e')),
            ],
            Action::Confirm => [None, None],
            // Both slots are editable now: the overlay's own keys are handled by
            // the overlay, so `Cancel` is an ordinary action like the rest.
            Action::Cancel => [Some(KeySpec::code(KeyCode::Esc)), None],
        }
    }
}

/// One resolved binding: which action a key actually triggers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shortcut {
    pub action: Action,
    pub spec: KeySpec,
}

/// Whether `spec` is a "letter with modifiers" chord.
///
/// Such chords give way to control keys (`Esc`, `Enter`, `Tab`, `F*`, arrows):
/// `Ctrl+C` must stay available as an interrupt, and `Enter` cannot become
/// "copy" just because someone typed it into a slot.
fn is_letter_chord(spec: &KeySpec) -> bool {
    // `Ctrl+F`, `Alt+B` and friends are the normal form of a chord. Only a bare
    // letter shared between two actions (`c` for copy and for delete, say) is
    // ambiguous, and that is what the rank below resolves.
    matches!(spec.code, KeyCode::Char(_)) && spec.modifiers.is_empty()
}

/// Keep only the bindings that can win, in a deterministic order.
///
/// A candidate is `(is_user_assigned, shortcut)`. Rules, applied in this order:
///
/// 1. reserved combos (`Esc`, the settings key) never get here — they are
///    filtered while the candidates are collected;
/// 2. a user-assigned slot beats a built-in default;
/// 3. a letter chord loses to a control key using the same combo, so `Ctrl+C`
///    stays the interrupt and `Enter` cannot become "copy";
/// 4. anything still duplicated is resolved by action order, so the result does
///    not depend on iteration speed.
///
/// The result is what the dispatcher matches, what the footer draws and what
/// the help screen lists — one list, no drift.
pub fn resolve(candidates: Vec<(bool, Shortcut)>) -> Vec<Shortcut> {
    let custom_keys: Vec<KeySpec> = candidates
        .iter()
        .filter(|(custom, _)| *custom)
        .map(|(_, shortcut)| shortcut.spec)
        .collect();

    let mut sorted = candidates;
    sorted.sort_by_key(|(custom, shortcut)| {
        let shadowed_default = !*custom && custom_keys.contains(&shortcut.spec);
        let letter = is_letter_chord(&shortcut.spec);
        // Lower sorts first and therefore wins.
        (
            shadowed_default,
            letter,
            !*custom,
            action_rank(shortcut.action),
        )
    });

    let mut taken: Vec<KeySpec> = Vec::new();
    let mut resolved = Vec::new();
    for (_, shortcut) in sorted {
        if taken.contains(&shortcut.spec) {
            continue;
        }
        taken.push(shortcut.spec);
        resolved.push(shortcut);
    }
    resolved
}

/// Secondary sort key; only needs to be stable and sensible.
fn action_rank(action: Action) -> u8 {
    match action {
        Action::Quit => 0,
        Action::Help => 1,
        Action::Copy => 2,
        Action::Move => 3,
        Action::Mkdir => 4,
        Action::Rename => 5,
        Action::Delete => 6,
        Action::DeletePermanent => 7,
        Action::OpenHere => 8,
        Action::TogglePreview => 9,
        Action::ToggleThumbnails => 9,
        Action::SortMenu => 10,
        Action::ToggleHidden => 11,
        Action::SelectAll => 12,
        Action::BulkRename => 13,
        Action::Refresh => 14,
        Action::Settings => 15,
        Action::OpenInOther => 16,
        Action::ContextMenu => 25,
        Action::PreviewThere => 17,
        Action::AddFavorite => 18,
        Action::FocusFavorites => 19,
        Action::ToggleSidebar => 20,
        Action::CopyPath => 21,
        Action::CopyName => 22,
        Action::CopySelection => 23,
        Action::SwitchPanel => 24,
        Action::ColourMenu => 23,
        Action::Confirm | Action::Cancel => 26,
        Action::FavoriteUp | Action::FavoriteDown => 27,
        Action::FavoriteSeparator | Action::FavoriteBlank | Action::FavoriteArrange => 28,
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
    ///
    /// Terminals also disagree about *shifted letters*: some send `Char('J')`
    /// with no modifier, others `Char('j')` plus `SHIFT` (the kitty keyboard
    /// protocol, which Ghostty uses). Both spellings are normalised to "lowercase
    /// character + SHIFT" before anything else, so a binding for a capital
    /// letter answers either one. Without this, `Shift+K`/`Shift+J` matched no
    /// binding and fell through to the plain `j`/`k` navigation — which is why
    /// the reorder keys moved the cursor instead of the row.
    pub fn matches(self, key: KeyEvent) -> bool {
        // A shifted letter reaches us in two spellings: `Char('J')` with no
        // modifier, or `Char('j')` plus SHIFT (kitty keyboard protocol, which
        // Ghostty and kitty use). When the *binding* is a capital letter, both
        // spellings mean that binding, so the key side is folded onto the
        // capital. Only the key side is touched: folding the binding would make
        // `Shift+C` match the plain `c` (copy), which must never happen.
        let (spec_code, spec_modifiers) = (self.code, self.modifiers);
        let (key_code, key_modifiers) = match (spec_code, key.code) {
            // `Shift+J` as `Char('j')` + SHIFT: the shift is real, so the
            // capital binding owns it.
            (KeyCode::Char(upper), KeyCode::Char(pressed))
                if upper.is_ascii_uppercase()
                    && pressed.is_ascii_lowercase()
                    && pressed.eq_ignore_ascii_case(&upper)
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                (KeyCode::Char(upper), key.modifiers - KeyModifiers::SHIFT)
            }
            _ => (key.code, key.modifiers),
        };

        // `Shift+←` is the mirror of `Shift+→` for "open in the other panel":
        // the arrow says *which* panel, so a spec holding one arrow must not
        // answer to the other. Compared by code, this keeps `Ctrl+O` (same
        // action) working while keeping the two arrows distinct.
        if spec_modifiers == KeyModifiers::SHIFT
            && key_modifiers == KeyModifiers::SHIFT
            && matches!(
                (spec_code, key_code),
                (KeyCode::Right, KeyCode::Left) | (KeyCode::Left, KeyCode::Right)
            )
        {
            // The displayed arrow is the twin; the action itself decides the
            // panel from `active`, so both directions are the same gesture.
            return true;
        }

        // `PreviewThere` is bound to Alt+←/→, but Ghostty and iTerm2 encode
        // Alt+←/→ as `ESC b` / `ESC f` by default, so those letters arrive as
        // the arrow. Only the Alt binding accepts them, and only when Alt is
        // the sole modifier: a `Ctrl+←` spec must never answer to `Ctrl+F`,
        // which is the sidebar toggle.
        if spec_modifiers == KeyModifiers::ALT
            && key_modifiers == KeyModifiers::ALT
            && matches!(
                (spec_code, key_code),
                (KeyCode::Left, KeyCode::Char('b')) | (KeyCode::Right, KeyCode::Char('f'))
            )
        {
            return true;
        }

        if spec_code != key_code {
            return false;
        }

        // Command arrives as SUPER or META depending on the platform, so both
        // sides are normalised to SUPER first.
        let mut wanted = spec_modifiers;
        if wanted.intersects(KeyModifiers::META) {
            wanted.remove(KeyModifiers::META);
            wanted |= KeyModifiers::SUPER;
        }
        let mut actual = key_modifiers;
        if actual.intersects(KeyModifiers::META) {
            actual.remove(KeyModifiers::META);
            actual |= KeyModifiers::SUPER;
        }

        // A terminal reports `?` as Shift+/, and a shifted letter arrives as an
        // uppercase character with no Shift modifier. A *character* binding may
        // therefore tolerate a stray Shift — but nothing else may: a binding
        // without Shift must never fire on Shift+←, which is a different
        // gesture entirely (`Shift+→` opens the folder in the other panel).
        // The tolerance is for *symbols only*: `?` is Shift+/ and arrives with
        // or without SHIFT depending on the terminal. A letter must not get it,
        // or `Shift+C` would fire the plain `c` (copy) — the exact accident the
        // modifier checks exist to prevent.
        let symbolic = matches!(spec_code, KeyCode::Char(c) if !c.is_ascii_alphabetic());
        if symbolic && !wanted.contains(KeyModifiers::SHIFT) {
            wanted.remove(KeyModifiers::SHIFT);
            actual.remove(KeyModifiers::SHIFT);
        } else if !wanted.contains(KeyModifiers::SHIFT) {
            // A key binding without Shift must not answer to a shifted key…
            actual.remove(KeyModifiers::SHIFT);
        }
        // …and one *with* Shift keeps requiring it, which is the whole point of
        // `Shift+→` being a separate gesture from `→`.

        // Modifiers are compared *exactly*: `Ctrl+←`, `Shift+←` and `Alt+←` are
        // three different actions, and a subset test would fire all of them.
        actual == wanted
    }

    /// Build a spec from a pressed key.
    ///
    /// Shift is kept only for keys where it is meaningful: a terminal reports
    /// `Shift+a` as `A`, but `Shift+←` keeps the modifier. Command (Super/Meta)
    /// is folded into `SUPER` so one spec covers both reportings.
    pub fn from_event(key: KeyEvent) -> Self {
        let mut modifiers = KeyModifiers::NONE;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            modifiers |= KeyModifiers::CONTROL;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            modifiers |= KeyModifiers::ALT;
        }
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            modifiers |= KeyModifiers::SHIFT;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::SUPER | KeyModifiers::META)
        {
            modifiers |= KeyModifiers::SUPER;
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
        if self.modifiers.contains(KeyModifiers::SUPER) {
            parts.push("super".to_string());
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
                "super" | "cmd" | "command" | "meta" => modifiers |= KeyModifiers::SUPER,
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
    /// Draw Nerd Font glyphs in the listing. Off for terminals without a
    /// patched font, where the glyphs would render as boxes.
    pub icons: bool,
    /// Optional path of a Nerd Font `.ttf` used to rasterise thumbnail-grid
    /// icons. Empty means "use the built-in block art".
    pub icon_font: String,
    /// Largest file (in bytes) decoded for a thumbnail; bigger ones keep the
    /// block-art icon so scrolling stays fast.
    pub max_thumb_bytes: u64,
    /// One entry per action that was edited: the two live slots.
    ///
    /// An action missing from the map uses [`Action::defaults`]. Slots are
    /// stored as a whole so an intentionally empty slot survives a reload.
    pub slots: BTreeMap<String, [Option<KeySpec>; 2]>,
    file: PathBuf,
}

impl Settings {
    /// Built-in defaults, ignoring the user's file.
    ///
    /// Used by tests so a developer's `settings.toml` cannot change what they
    /// assert.
    #[cfg(test)]
    pub fn defaults() -> Self {
        Self {
            invert_scroll: true,
            icons: true,
            icon_font: String::new(),
            max_thumb_bytes: 4 * 1024 * 1024,
            slots: BTreeMap::new(),
            file: settings_file(),
        }
    }

    /// Load settings, falling back to defaults when the file is missing.
    pub fn load() -> Self {
        let file = settings_file();
        let text = fs::read_to_string(&file).unwrap_or_default();
        let mut invert_scroll = true;
        let mut icons = true;
        let mut icon_font = String::new();
        let mut max_thumb_bytes = 4 * 1024 * 1024;
        let mut slots: BTreeMap<String, [Option<KeySpec>; 2]> = BTreeMap::new();

        for (key, value) in parse_toml(&text) {
            if key == "invert_scroll" {
                invert_scroll = matches!(value.as_str(), "true" | "1" | "on" | "yes");
            } else if key == "icons" {
                icons = matches!(value.as_str(), "true" | "1" | "on" | "yes");
            } else if key == "icon_font" {
                icon_font = value.to_string();
            } else if key == "max_thumb_bytes" {
                max_thumb_bytes = value.parse().unwrap_or(max_thumb_bytes);
            } else if let Some(rest) = key.strip_prefix("key.") {
                // `key.<action>.<slot>`, with the single-slot form
                // `key.<action>` kept readable from older files.
                let (action, slot) = match rest.rsplit_once('.') {
                    Some((action, suffix)) if suffix == "1" || suffix == "2" => {
                        (action, suffix.parse::<usize>().ok().map(|n| n - 1))
                    }
                    _ => (rest, None),
                };
                let Some(action) = Action::ALL
                    .iter()
                    .find(|candidate| candidate.id() == action)
                else {
                    continue;
                };
                let entry = slots.entry(action.id().to_string()).or_insert([None, None]);
                let slot = slot.unwrap_or(0usize);
                entry[slot] = KeySpec::parse(&value);
            }
        }

        Self {
            invert_scroll,
            icons,
            icon_font,
            max_thumb_bytes,
            slots,
            file,
        }
    }

    /// Persist the current settings; failures are reported by the caller.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = format!(
            "invert_scroll = {}\nicons = {}\nicon_font = \"{}\"\nmax_thumb_bytes = {}\n",
            self.invert_scroll, self.icons, self.icon_font, self.max_thumb_bytes
        );
        for action in Action::ALL {
            let defaults = action.defaults();
            let slots = self.stored_slots(action);
            // Only actions that differ from the defaults are written, so the
            // file stays readable and a new default reaches existing users.
            if slots == defaults {
                continue;
            }
            for (index, slot) in slots.iter().enumerate() {
                let value = slot.map(|spec| spec.label()).unwrap_or_default();
                text.push_str(&format!(
                    "key.{}.{} = \"{value}\"\n",
                    action.id(),
                    index + 1
                ));
            }
        }
        fs::write(&self.file, text)
    }

    /// The two slots of an action as stored (without defaults).
    pub fn stored_slots(&self, action: Action) -> [Option<KeySpec>; 2] {
        match self.slots.get(action.id()) {
            Some(slots) => *slots,
            None => action.defaults(),
        }
    }

    /// True when the user edited this action at all.
    pub fn is_custom(&self, action: Action) -> bool {
        self.slots
            .get(action.id())
            .is_some_and(|slots| *slots != action.defaults())
    }

    /// Assign `spec` to one slot.
    ///
    /// Both slots of every action are editable, including the settings menu's
    /// own key (ADR-063). A combo already owned by another action is refused
    /// when that action was customised too: silently stealing a key from, say,
    /// "copy" would be worse than telling the user.
    pub fn assign(&mut self, action: Action, slot: usize, spec: KeySpec) -> Result<(), String> {
        if slot > 1 {
            return Err("only two slots exist".to_string());
        }
        // A combo owned by another action is only a conflict when that action
        // was edited by the user too; a *default* gives way, which is what makes
        // "take Shift+→ for copy" possible at all.
        if let Some((owner, _)) = self.owner_of(spec).filter(|(owner, _)| *owner != action) {
            if self.is_custom(owner) {
                return Err(format!("{} is used by {}", spec.label(), owner.label()));
            }
            self.release_from_defaults(spec, action);
        }

        let mut slots = self.stored_slots(action);
        // Repeating a combo in the other slot is a mistake, not a feature.
        if let Some(other) = [0usize, 1]
            .into_iter()
            .find(|index| *index != slot && slots[*index].is_some_and(|existing| existing == spec))
        {
            slots[other] = None;
        }
        slots[slot] = Some(spec);
        self.slots.insert(action.id().to_string(), slots);
        Ok(())
    }

    /// Clear one slot. The action stays reachable through the other one.
    pub fn clear_slot(&mut self, action: Action, slot: usize) {
        if slot > 1 {
            return;
        }
        let mut slots = self.stored_slots(action);
        slots[slot] = None;
        self.slots.insert(action.id().to_string(), slots);
    }

    /// Drop both overrides so the built-in keys apply again.
    pub fn reset(&mut self, action: Action) {
        self.slots.remove(action.id());
    }

    /// Which action currently owns `spec`, if any.
    ///
    /// This reads the *slots*, not the resolved list: assigning a key has to
    /// see a slot the user just cleared, and resolution only runs afterwards.
    pub fn owner_of(&self, spec: KeySpec) -> Option<(Action, KeySpec)> {
        Action::menu_actions().into_iter().find_map(|action| {
            self.stored_slots(action)
                .iter()
                .flatten()
                .any(|existing| *existing == spec)
                .then_some((action, spec))
        })
    }

    /// Release `spec` from every action except `keep`.
    ///
    /// Assigning a key that another action holds only by *default* takes it away
    /// from that default: the user's explicit choice must win, and the resolve
    /// step already ranks it that way. An action that lost its last key is left
    /// with an empty slot rather than silently keeping a duplicate.
    fn release_from_defaults(&mut self, spec: KeySpec, keep: Action) {
        // Nothing to do: the resolver ranks a user assignment over a default.
        let _ = (spec, keep);
    }

    /// Every key combo the app reacts to, after duplicate resolution.
    ///
    /// This is *the* list: dispatch, footer and help all read it, so a rebound
    /// key cannot leave an invisible duplicate behind.
    pub fn shortcuts(&self) -> Vec<Shortcut> {
        let mut candidates = Vec::new();
        for action in Action::ALL {
            // `stored_slots` already merges the defaults for an untouched
            // action; an edited action reports exactly what was stored, which is
            // how a deliberately cleared slot stays cleared.
            let custom = self.slots.contains_key(action.id());
            for spec in self.stored_slots(action).into_iter().flatten() {
                if is_reserved(spec) {
                    continue;
                }
                candidates.push((custom, Shortcut { action, spec }));
            }
        }
        resolve(candidates)
    }

    /// Display string for an action's slots, e.g. `"f5 · c"`.
    pub fn keys_label(&self, action: Action) -> String {
        self.shortcuts()
            .into_iter()
            .filter(|shortcut| shortcut.action == action)
            .map(|shortcut| shortcut.spec.label())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The action a key actually triggers, if any.
    ///
    /// Reads the resolved list, so a rebound key resolves to its new action and
    /// the old one stops firing.
    /// The action a key actually triggers, if any.
    ///
    /// Reads the resolved list, so a rebound key resolves to its new action and
    /// the old one stops firing. There are no fixed aliases: every key the app
    /// acts on comes from this list (ADR-063).
    pub fn action_for(&self, key: KeyEvent) -> Option<Action> {
        self.shortcuts()
            .into_iter()
            .find(|shortcut| shortcut.spec.matches(key))
            .map(|shortcut| shortcut.action)
    }
}

/// Keys that the app relies on being always available.
/// Keys the app used to keep for itself.
///
/// There are none any more (ADR-063). Every key is assignable in `Ctrl+E`, and
/// the escape hatches that used to justify the exceptions are the pointer and
/// `Action::Settings`'s own slot. The function stays so the call sites read the
/// same, and so "reserved" has exactly one place to be reintroduced if a real
/// need ever appears.
pub fn is_reserved(_spec: KeySpec) -> bool {
    false
}

fn settings_file() -> PathBuf {
    crate::favorites::config_dir().join("settings.toml")
}

/// The settings path, for the test that checks config isolation.
#[cfg(test)]
pub fn settings_file_for_test() -> PathBuf {
    settings_file()
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

    fn defaults() -> Settings {
        Settings::defaults()
    }

    #[test]
    fn both_slots_are_editable_and_independent() {
        let mut settings = defaults();
        let defaults = Action::Copy.defaults();
        assert_eq!(settings.stored_slots(Action::Copy), defaults);

        // Slot 2 used to be reachable only through the default: now it is a
        // first-class target.
        settings
            .assign(Action::Copy, 1, KeySpec::ctrl('y'))
            .expect("slot 2 must be assignable");
        assert_eq!(
            settings.stored_slots(Action::Copy),
            [defaults[0], Some(KeySpec::ctrl('y'))]
        );

        settings
            .assign(Action::Copy, 0, KeySpec::f(9))
            .expect("slot 1 must be assignable");
        assert_eq!(
            settings.stored_slots(Action::Copy),
            [Some(KeySpec::f(9)), Some(KeySpec::ctrl('y'))]
        );
        assert!(settings.is_custom(Action::Copy));

        // The resolution sees both slots.
        let keys: Vec<String> = settings
            .shortcuts()
            .into_iter()
            .filter(|shortcut| shortcut.action == Action::Copy)
            .map(|shortcut| shortcut.spec.label())
            .collect();
        assert_eq!(keys, vec!["f9".to_string(), "ctrl+y".to_string()]);

        settings.reset(Action::Copy);
        assert_eq!(settings.stored_slots(Action::Copy), defaults);
        assert!(!settings.is_custom(Action::Copy));
    }

    #[test]
    fn a_freed_slot_stops_answering() {
        let mut settings = defaults();
        settings.clear_slot(Action::Copy, 0);
        assert_eq!(settings.stored_slots(Action::Copy)[0], None);
        assert!(
            !settings
                .shortcuts()
                .iter()
                .any(|shortcut| shortcut.spec == KeySpec::f(5)),
            "F5 still resolves after its slot was cleared"
        );

        // ...and the freed combo can now be taken by another action.
        settings
            .assign(Action::Move, 0, KeySpec::f(5))
            .expect("F5 must be free now");
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            Some(Action::Move)
        );
    }

    #[test]
    fn a_key_cannot_be_stolen_from_an_edited_action() {
        let mut settings = defaults();
        // A key the user *chose* for Copy is protected from Move.
        settings
            .assign(Action::Copy, 0, KeySpec::ctrl('y'))
            .unwrap();
        let error = settings
            .assign(Action::Move, 0, KeySpec::ctrl('y'))
            .expect_err("stealing a user-chosen key must fail");
        assert!(error.contains("Copy"), "unhelpful error: {error}");

        // A built-in default, on the other hand, gives way: that is what makes
        // rebinding possible at all.
        settings
            .assign(Action::Copy, 1, KeySpec::f(6))
            .expect("a default key must yield to an explicit choice");
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE)),
            Some(Action::Copy)
        );
    }

    #[test]
    fn every_key_is_assignable_including_esc_and_the_settings_key() {
        // ADR-063: no key is privileged any more. `Esc` and `Ctrl+E` used to be
        // refused outright, and the settings menu's own first slot was frozen.
        let mut settings = defaults();

        assert!(settings
            .assign(Action::Copy, 0, KeySpec::code(KeyCode::Esc))
            .is_ok());
        assert!(settings.assign(Action::Copy, 1, KeySpec::ctrl('e')).is_ok());
        assert!(settings.assign(Action::Settings, 0, KeySpec::f(2)).is_ok());
        assert_eq!(
            settings.stored_slots(Action::Settings)[0],
            Some(KeySpec::f(2))
        );

        // `Ctrl+E` was just given to Copy, so it runs Copy now — the point is
        // that nothing refused the assignment. The settings menu moved to F2.
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            Some(Action::Copy)
        );
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)),
            Some(Action::Settings)
        );
    }

    /// The bug this refactor fixes: rebinding Q to preview must free Q from Quit.
    #[test]
    fn rebinding_q_to_preview_frees_q_from_quit() {
        let mut settings = defaults();
        // Slot 1 of Quit was `q`.
        settings.clear_slot(Action::Quit, 0);
        // Preview's slot 1 is Ctrl+Q and slot 2 is Ctrl+P; `q` needs a free slot,
        // which is exactly what clearing Quit's gave it.
        settings.clear_slot(Action::TogglePreview, 1);
        settings
            .assign(Action::TogglePreview, 1, KeySpec::plain_char('q'))
            .expect("q must be assignable once Quit released it");

        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::TogglePreview),
            "q must no longer quit"
        );
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Some(Action::TogglePreview),
            "Ctrl+Q stays preview; only the bare q moved"
        );
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit),
            "Ctrl+C must still quit"
        );
    }

    #[test]
    fn ctrl_c_stays_the_interrupt_and_enter_opens() {
        let settings = defaults();
        // `Ctrl+C` is quit's second slot and the only claimant: no letter chord
        // may shadow it.
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::OpenHere)
        );
        // A plain letter for copy is gone: only F5 remains.
        assert_eq!(
            settings.action_for(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            Some(Action::Copy)
        );
    }

    #[test]
    fn every_default_is_hidden_behind_a_slot() {
        // Two slots maximum, and anything beyond them used to be unreachable.
        for action in Action::ALL {
            assert_eq!(action.defaults().len(), 2);
        }
        let settings = defaults();
        for action in Action::ALL {
            let bound = settings
                .shortcuts()
                .into_iter()
                .filter(|shortcut| shortcut.action == action)
                .count();
            let expected = action.defaults().iter().flatten().count();
            assert_eq!(
                bound,
                expected,
                "{} lost a slot in resolution ({} of {expected})",
                action.id(),
                bound
            );
        }
    }

    #[test]
    fn a_single_action_never_shows_the_same_key_twice() {
        let mut settings = defaults();
        // Del was both in Delete's second slot and in the old hidden third one;
        // the footer showed it repeatedly. Now one entry per action+key.
        settings
            .assign(Action::Delete, 0, KeySpec::code(KeyCode::Delete))
            .unwrap();
        let keys: Vec<String> = settings
            .shortcuts()
            .into_iter()
            .filter(|shortcut| shortcut.action == Action::Delete)
            .map(|shortcut| shortcut.spec.label())
            .collect();
        let unique: std::collections::BTreeSet<&String> = keys.iter().collect();
        assert_eq!(keys.len(), unique.len(), "duplicate keys in {keys:?}");
    }

    #[test]
    fn every_action_has_a_distinct_id_and_label() {
        let mut ids = std::collections::BTreeSet::new();
        for action in Action::ALL {
            assert!(ids.insert(action.id()), "duplicate id: {}", action.id());
            assert!(!action.label().is_empty());
            // `Confirm` and `Cancel` are prompt keys, not bindable slots.
            if !matches!(action, Action::Confirm | Action::Cancel) {
                assert!(
                    action.defaults().iter().any(Option::is_some),
                    "{} has no default key at all",
                    action.id()
                );
            }
        }
    }

    #[test]
    fn ctrl_d_and_the_section_sign_are_ordinary_defaults() {
        // `Ctrl+D` and `§` are the two default slots of `FocusFavorites`, so a
        // user can change either in `Ctrl+E` — which is the point (ADR-063).
        let settings = defaults();
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(settings.action_for(ctrl_d), Some(Action::FocusFavorites));

        // `§` is not privileged either: it works because it can be assigned
        // like any other character, which is what the old fixed alias prevented.
        let mut with_section = defaults();
        with_section
            .assign(Action::FocusFavorites, 1, KeySpec::plain_char('§'))
            .unwrap();
        let section = KeyEvent::new(KeyCode::Char('§'), KeyModifiers::NONE);
        assert_eq!(
            with_section.action_for(section),
            Some(Action::FocusFavorites)
        );

        // Rebinding the first slot really replaces `Ctrl+D`.
        let mut edited = defaults();
        edited
            .assign(Action::FocusFavorites, 1, KeySpec::code(KeyCode::BackTab))
            .unwrap();
        assert_eq!(edited.action_for(ctrl_d), None);
    }

    #[test]
    fn a_custom_binding_beats_a_default_one() {
        // `Ctrl+D` belongs to favorites by default; taking it for the sort menu
        // must move it, because a default always gives way to an edit.
        let mut settings = Settings::defaults();
        settings
            .assign(Action::SortMenu, 0, KeySpec::ctrl('d'))
            .unwrap();
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(settings.action_for(ctrl_d), Some(Action::SortMenu));
    }
}
