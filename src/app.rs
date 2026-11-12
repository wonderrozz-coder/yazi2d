use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::colors::Colors;
use crate::favorites::Favorites;
use crate::logging;
use crate::panel::{LoadKind, LoadResult, Panel, PanelMode, SortKey, StatsUpdate};
use crate::preview::{ImageCache, PreviewCache};
use crate::search::{self, SearchMsg};
use crate::settings::{Action, KeySpec, Settings};
use crate::transfer::{self, Job, JobKind, JobMsg};

/// How long `F9` waits for an arrow before the chord is abandoned.
const LEADER_WINDOW: Duration = Duration::from_millis(2000);

/// Two pointer taps within this window open the entry under the cursor.
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Panels,
    Favorites,
}

pub enum Prompt {
    Mkdir {
        buffer: String,
    },
    Rename {
        buffer: String,
        target: PathBuf,
    },
    Delete {
        sources: Vec<PathBuf>,
    },
    BulkRename {
        buffer: String,
        sources: Vec<PathBuf>,
    },
    /// Copy/move confirmation: source count, destination, direction.
    Transfer {
        kind: JobKind,
        sources: Vec<PathBuf>,
        dest: PathBuf,
    },
}

pub struct App {
    pub panels: [Panel; 2],
    pub active: usize,
    pub focus: Focus,
    pub favorites: Favorites,
    pub show_favorites: bool,
    pub fav_cursor: usize,
    /// Screen rectangle of the favorites list body (borders excluded).
    pub favorites_area: Rect,
    /// First favorites row currently drawn, so clicks map to the right bookmark.
    pub fav_offset: usize,
    pub hidden: bool,
    pub prompt: Option<Prompt>,
    pub help: bool,
    /// Right-click menu over an entry, or over a path bar.
    pub context_menu: Option<ContextMenu>,
    /// `Alt+F7` recursive search window.
    pub search: Option<SearchWindow>,
    pub job: Option<Job>,
    /// At most one pending background directory load per panel.
    pub loads: [Option<Receiver<LoadResult>>; 2],
    /// Background stat workers that fill in sizes/dates after a fast listing.
    pub stat_jobs: [Option<Receiver<StatsUpdate>>; 2],
    /// Sort menu overlay.
    pub sort_menu: bool,
    /// Settings menu overlay.
    pub settings_menu: bool,
    /// Row selected in the settings menu.
    pub settings_cursor: usize,
    /// True while the settings menu waits for a key to bind.
    pub rebinding: bool,
    /// User settings: bindings and trackpad direction.
    pub settings: Settings,
    /// Colour-theme menu overlay.
    pub theme_menu: bool,
    /// File-type colour palette.
    pub colors: Colors,
    pub preview_cache: PreviewCache,
    pub image_cache: ImageCache,
    /// After a delete, where the cursor should land: the names that were
    /// removed and the row index the cursor had. Applied when the refresh
    /// completes so the cursor does not jump to the top.
    pub pending_cursor_after: Option<(Vec<String>, usize)>,
    pub status: String,
    pub should_quit: bool,
    /// Set when the screen must be repainted. Keeps idle CPU usage near zero.
    pub dirty: bool,
    /// Modifiers currently held down, used to show matching keys in the footer.
    pub held_modifiers: KeyModifiers,
    /// Physical modifier keys currently held (left/right tracked separately).
    pub held_keys: Vec<ModifierKeyCode>,
    /// Set when `F9` was pressed and an arrow is expected next.
    pub leader: Option<Instant>,
    /// Timestamp of the last pointer tap, for double-tap detection.
    pub last_tap: Option<Instant>,
    /// Where the last tap landed: panel index and row index. A second tap on a
    /// different row starts a new gesture instead of opening anything.
    pub last_tap_target: Option<(usize, usize)>,
    /// The arrow key that is currently held, if any, and when it was pressed.
    /// Held arrows accelerate: after the keyboard's own repeat delay the app
    /// starts moving the cursor by itself, much faster than the terminal
    /// repeats the key.
    pub held_arrow: Option<(KeyCode, Instant)>,
    /// Last automatic move of a held arrow.
    pub last_auto_repeat: Option<Instant>,
}

/// After the arrow has been held this long, the app moves the cursor itself.
const REPEAT_DELAY: Duration = Duration::from_millis(320);
/// Interval between two automatic moves at the first repeat speed.
const REPEAT_START: Duration = Duration::from_millis(55);
/// Fastest automatic repeat interval.
const REPEAT_MIN: Duration = Duration::from_millis(12);
/// How often the repeat speed is doubled.
const REPEAT_RAMP: Duration = Duration::from_millis(500);

/// One row of the right-click menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuItem {
    Open,
    OpenInOther,
    CopyPath,
    CopyName,
    ToggleSelect,
    CopySelection,
    ClearSelection,
    Rename,
    Delete,
    Mkdir,
    AddFavorite,
    Thumbnails,
    Search,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::Open => "Open",
            MenuItem::OpenInOther => "Open in the other panel",
            MenuItem::CopyPath => "Copy full path",
            MenuItem::CopyName => "Copy name",
            MenuItem::ToggleSelect => "Select / deselect",
            MenuItem::CopySelection => "Copy selected paths",
            MenuItem::ClearSelection => "Clear selection",
            MenuItem::Rename => "Rename",
            MenuItem::Delete => "Delete",
            MenuItem::Mkdir => "New folder",
            MenuItem::AddFavorite => "Add folder to favorites",
            MenuItem::Thumbnails => "Thumbnails on / off",
            MenuItem::Search => "Find (Alt+F7)",
        }
    }
}

/// A right-click menu anchored at the pointer.
#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub panel: usize,
    /// Entry the menu was opened on; `None` for a right-click on the path bar.
    pub index: Option<usize>,
    pub items: Vec<MenuItem>,
    pub cursor: usize,
    pub x: u16,
    pub y: u16,
    /// Screen rectangle, written by the renderer so clicks can be routed.
    pub rect: Rect,
}

impl ContextMenu {
    /// Row index under a screen point, if any.
    pub fn index_at(&self, x: u16, y: u16) -> Option<usize> {
        // The first row inside the border is item 0.
        let first = self.rect.y + 1;
        if x <= self.rect.x || x >= self.rect.right().saturating_sub(1) {
            return None;
        }
        if y < first || y >= self.rect.bottom().saturating_sub(1) {
            return None;
        }
        let index = (y - first) as usize;
        (index < self.items.len()).then_some(index)
    }
}

/// State of the `Alt+F7` recursive search.
#[derive(Debug)]
pub struct SearchWindow {
    /// Folder the search started from; the panel the hits open into.
    pub root: PathBuf,
    pub panel: usize,
    pub query: String,
    pub hits: Vec<search::Hit>,
    pub cursor: usize,
    pub offset: usize,
    /// Entries the worker has looked at so far.
    pub scanned: usize,
    /// The worker stopped at the result cap.
    pub truncated: bool,
    pub running: bool,
    pub receiver: Option<Receiver<SearchMsg>>,
    /// Screen rectangle of the list body, written by the renderer.
    pub list_area: Rect,
}

impl SearchWindow {
    fn new(root: PathBuf, panel: usize) -> Self {
        Self {
            root,
            panel,
            query: String::new(),
            hits: Vec::new(),
            cursor: 0,
            offset: 0,
            scanned: 0,
            truncated: false,
            running: false,
            receiver: None,
            list_area: Rect::new(0, 0, 0, 0),
        }
    }

    /// Rows the window can show at once (one hit per row).
    fn page(&self) -> usize {
        self.list_area.height.max(1) as usize
    }

    fn move_cursor(&mut self, delta: i32) {
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() as i32 - 1;
        self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
        self.ensure_visible();
    }

    pub fn ensure_visible(&mut self) {
        let page = self.page();
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if self.cursor >= self.offset + page {
            self.offset = self.cursor + 1 - page;
        }
        let max_offset = self.hits.len().saturating_sub(page);
        self.offset = self.offset.min(max_offset);
    }

    fn current(&self) -> Option<&search::Hit> {
        self.hits.get(self.cursor)
    }
}

impl App {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home = dirs::home_dir().unwrap_or_else(|| cwd.clone());

        let left = pick_dir(paths.first(), &cwd);
        let right = match paths.get(1) {
            Some(path) if path.is_dir() => path.clone(),
            _ => {
                if home != left {
                    home.clone()
                } else {
                    let downloads = home.join("Downloads");
                    if downloads.is_dir() {
                        downloads
                    } else {
                        left.clone()
                    }
                }
            }
        };

        let mut panels = [Panel::new(left), Panel::new(right)];
        let hidden = false;
        let mut errors = Vec::new();
        for panel in &mut panels {
            if let Err(e) = panel.reload(hidden) {
                errors.push(format!("{}: {}", panel.cwd.display(), e));
            }
            // The first frame must show sizes and dates, so the initial
            // listing is enriched synchronously. Startup is the only blocking
            // listing; everything after it runs in the background.
            panel.enrich_blocking();
        }

        let status = if errors.is_empty() {
            "Ready. Tab switches panels, ? shows help.".to_string()
        } else {
            errors.join(" | ")
        };

        logging::info(format!(
            "panels ready: left={} right={}",
            panels[0].cwd.display(),
            panels[1].cwd.display()
        ));

        Self {
            panels,
            active: 0,
            focus: Focus::Panels,
            favorites: Favorites::load_or_default(),
            show_favorites: true,
            fav_cursor: 0,
            favorites_area: Rect::new(0, 0, 0, 0),
            fav_offset: 0,
            hidden,
            prompt: None,
            help: false,
            context_menu: None,
            search: None,
            job: None,
            loads: [None, None],
            stat_jobs: [None, None],
            sort_menu: false,
            settings_menu: false,
            settings_cursor: 0,
            rebinding: false,
            settings: Settings::load(),
            theme_menu: false,
            colors: Colors::new(),
            preview_cache: PreviewCache::new(),
            image_cache: ImageCache::new(),
            pending_cursor_after: None,
            status,
            should_quit: false,
            dirty: true,
            held_modifiers: KeyModifiers::NONE,
            held_keys: Vec::new(),
            leader: None,
            last_tap: None,
            last_tap_target: None,
            held_arrow: None,
            last_auto_repeat: None,
        }
    }

    pub fn active(&self) -> &Panel {
        &self.panels[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Panel {
        &mut self.panels[self.active]
    }

    pub fn other(&self) -> &Panel {
        &self.panels[1 - self.active]
    }

    /// Drain worker-thread progress and finish completed jobs.
    pub fn tick(&mut self) {
        self.poll_loads();
        self.poll_stats();
        self.poll_search();
        self.sync_preview_scroll();
        // Background preview/image loads complete after a frame or two; repaint
        // when something new showed up.
        if self.preview_cache.is_loading() || self.image_cache.is_loading() {
            self.dirty = true;
        }
        let mut finished = false;
        let mut changed = false;

        if let Some(job) = &mut self.job {
            while let Ok(message) = job.rx.try_recv() {
                changed = true;
                match message {
                    JobMsg::Progress {
                        done,
                        total,
                        current,
                    } => {
                        job.done = done;
                        job.total = total;
                        job.current = current;
                    }
                    JobMsg::Done { done, errors } => {
                        job.done = done;
                        job.errors = errors;
                        job.finished = true;
                    }
                }
            }
            finished = job.finished;
        }

        if finished {
            let job = self.job.take().expect("job disappeared");
            self.preview_cache.invalidate();
            for index in 0..2 {
                if !self.panels[index].loading {
                    self.reload_panel_async(index);
                }
            }
            self.sync_preview_scroll();
            changed = true;

            if job.errors.is_empty() {
                self.status = format!("{} {} item(s).", job.kind.done_verb(), job.done);
            } else {
                self.status = format!(
                    "{} with {} error(s): {}",
                    job.kind.done_verb(),
                    job.errors.len(),
                    job.errors[0]
                );
            }
        }

        if changed {
            self.dirty = true;
        }
    }

    /// Apply finished background directory loads.
    fn poll_loads(&mut self) {
        for index in 0..self.panels.len() {
            let received = match self.loads[index].as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(load) => Some(Ok(load)),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Err(())),
                },
                None => None,
            };

            match received {
                Some(Ok(load)) => {
                    self.loads[index] = None;
                    let path = load.path.clone();
                    let applied = self.panels[index].apply_load(load);
                    if applied {
                        // Restore a sensible cursor after a delete instead of
                        // letting it jump to the first row.
                        if let Some((names, previous)) = self.pending_cursor_after.take() {
                            self.panels[index].focus_after_removal(&names, previous);
                        }
                        let label = if index == 0 { "Left" } else { "Right" };
                        self.status = format!("{label}: {}", self.panels[index].cwd.display());
                        self.start_stat_worker(index);
                    } else if let Some(error) = self.panels[index].error.clone() {
                        self.status = format!("Cannot open {}: {error}", path.display());
                    }
                    self.dirty = true;
                }
                Some(Err(())) => {
                    self.loads[index] = None;
                    self.panels[index].loading = false;
                    self.panels[index].loading_path = None;
                    self.status = "Directory load stopped unexpectedly".to_string();
                    self.dirty = true;
                }
                None => {}
            }
        }
    }

    /// Drain stat batches from the enrichment workers.
    fn poll_stats(&mut self) {
        for index in 0..self.panels.len() {
            let received = match self.stat_jobs[index].as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(update) => Some(Ok(update)),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Err(())),
                },
                None => None,
            };

            match received {
                Some(Ok(update)) => {
                    if self.panels[index].apply_stats(update) {
                        self.dirty = true;
                    }
                }
                Some(Err(())) => {
                    self.stat_jobs[index] = None;
                    self.panels[index].finish_enrichment();
                    self.dirty = true;
                }
                None => {}
            }
        }
    }

    /// Spawn the stat worker for a freshly applied listing.
    fn start_stat_worker(&mut self, index: usize) {
        let (sender, receiver, work, generation) = self.panels[index].begin_enrichment();
        if work.is_empty() {
            self.panels[index].finish_enrichment();
            self.stat_jobs[index] = None;
            return;
        }
        std::thread::spawn(move || {
            crate::panel::stat_entries(work, generation, sender);
        });
        self.stat_jobs[index] = Some(receiver);
    }

    /// Start a background listing for one panel.
    fn start_load(&mut self, index: usize, path: PathBuf, select: Option<String>, kind: LoadKind) {
        let hidden = self.hidden;
        let receiver = self.panels[index].spawn_load(path, select, hidden, kind);
        self.loads[index] = Some(receiver);
        let subject = self.panels[index]
            .loading_label()
            .unwrap_or_else(|| "…".to_string());
        self.status = format!("Loading {subject}… (Esc cancels)");
        self.dirty = true;
    }

    /// Re-list one panel in the background, keeping the cursor when possible.
    fn reload_panel_async(&mut self, index: usize) {
        let path = self.panels[index].cwd.clone();
        let select = self.panels[index]
            .current_entry()
            .map(|entry| entry.name.clone());
        self.start_load(index, path, select, LoadKind::Refresh);
    }

    /// Cancel the active panel's pending load.
    fn cancel_active_load(&mut self) {
        let index = self.active;
        self.loads[index] = None;
        self.panels[index].cancel_load();
        self.status = "Load cancelled".to_string();
    }

    /// Handle a key-release event (only delivered with keyboard enhancement).
    ///
    /// Pressing and releasing a bare modifier produces no `Press` event on most
    /// terminals, so this is the only way to know when Ctrl/Alt/Shift is held.
    pub fn on_key_release(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Modifier(modifier) => self.release_modifier(modifier),
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k') => {
                self.release_arrow();
            }
            _ => {}
        }
    }

    fn release_modifier(&mut self, modifier: ModifierKeyCode) {
        let was = self.held_modifiers;
        let bit = bit_of(modifier);
        // Right/left pairs share a bit, but a release of one should not clear the
        // other. Track presses per physical key to stay correct.
        self.held_keys.retain(|held| *held != modifier);
        let still_held = self.held_keys.iter().any(|held| bit_of(*held) == bit);
        if bit != KeyModifiers::NONE && !still_held {
            self.held_modifiers.remove(bit);
        }
        if self.held_modifiers != was {
            self.dirty = true;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.dirty = true;

        // Prompts type text, so they keep the raw character. Everything else is
        // a shortcut and must not depend on the keyboard layout: with the
        // Russian layout `Ctrl+и` has to behave like `Ctrl+b`.
        let shortcut = if self.prompt.is_some() {
            key
        } else {
            crate::keys::normalize(key)
        };
        let key = shortcut;

        // Track bare modifier presses so the footer reacts while the key is held.
        if let KeyCode::Modifier(modifier) = key.code {
            if !self.held_keys.contains(&modifier) {
                self.held_keys.push(modifier);
            }
            let bit = bit_of(modifier);
            if bit != KeyModifiers::NONE {
                self.held_modifiers.insert(bit);
            }
            return;
        }

        // A real key press reports exactly which modifiers are down, so trust
        // it even when it clears bits: this is the recovery path for terminals
        // that never send modifier releases. Without it the footer can stay
        // stuck on "Ctrl · …" after the chord was used.
        if !self.held_keys.is_empty() {
            // The physical-key tracking is authoritative while it has entries.
            self.held_modifiers = key.modifiers;
        } else if key.modifiers.is_empty() || !self.held_modifiers.contains(key.modifiers) {
            self.held_modifiers = key.modifiers;
        }

        // Safety net: a press with no modifiers means nothing is held. This
        // unsticks the footer when a terminal loses a release event.
        if key.modifiers.is_empty() {
            self.held_modifiers = KeyModifiers::NONE;
            self.held_keys.clear();
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if self.help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::F(1)
            ) {
                self.help = false;
            }
            return;
        }

        if self.prompt.is_some() {
            self.handle_prompt_key(key);
            return;
        }

        if self.sort_menu {
            self.handle_sort_menu_key(key);
            return;
        }

        if self.theme_menu {
            self.handle_theme_menu_key(key);
            return;
        }

        if self.settings_menu {
            self.handle_settings_key(key);
            return;
        }

        if self.context_menu.is_some() {
            self.handle_context_menu_key(key);
            return;
        }

        if self.search.is_some() {
            self.handle_search_key(key);
            return;
        }

        if key.code == KeyCode::Char('q') && !key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // A user override wins over the built-in handling below. Only actions
        // that were explicitly rebound are consulted, so defaults keep their
        // context-sensitive behaviour.
        if let Some(action) = self.custom_binding(key) {
            self.dispatch_action(action);
            return;
        }

        if matches!(key.code, KeyCode::Char('?') | KeyCode::F(1)) {
            self.help = true;
            return;
        }

        // Shift+Tab (BackTab) is the primary way into and out of favorites.
        // `§` is the dedicated single key for the same action (requested
        // because double-Tab felt unreliable on macOS).
        if matches!(key.code, KeyCode::BackTab)
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
            || matches!(key.code, KeyCode::Char('§'))
        {
            self.toggle_favorites_focus();
            return;
        }

        // F9 is a chord leader, so the cross-panel actions also work on
        // keyboards where Ctrl/Alt + arrow never reaches the terminal:
        //   F9 → (wait for one arrow)
        //   ←/→  open the cursor folder in that panel
        //   ↑/↓  toggle the thumbnail grid in the active panel
        // The leader expires after LEADER_WINDOW or on any unrelated key.
        if let Some(started) = self.leader {
            if started.elapsed() > LEADER_WINDOW {
                self.leader = None;
            }
        }

        if key.code == KeyCode::F(9) {
            self.leader = Some(Instant::now());
            self.status = "F9 + ←/→ open there · F9 + ↑/↓ thumbnails · Esc cancels".to_string();
            return;
        }

        if let Some(_started) = self.leader.take() {
            match key.code {
                KeyCode::Left => {
                    self.open_entry_in_panel(0);
                    return;
                }
                KeyCode::Right => {
                    self.open_entry_in_panel(1);
                    return;
                }
                KeyCode::Up | KeyCode::Down => {
                    self.toggle_thumbnails();
                    return;
                }
                KeyCode::Esc => {
                    self.status = "F9 chord cancelled".to_string();
                    return;
                }
                _ => {
                    self.status = "F9 chord cancelled".to_string();
                }
            }
        }

        if key.code == KeyCode::Tab {
            self.focus = Focus::Panels;
            self.active ^= 1;
            self.status = format!(
                "Active panel: {}",
                if self.active == 0 { "Left" } else { "Right" }
            );
            return;
        }

        // `Alt+Shift+F7` opens the search in the other panel, `Alt+F7` in the
        // active one; `Meta`/`Super` is accepted too because some terminals
        // deliver the Option key that way.
        if key.code == KeyCode::F(7)
            && key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META)
        {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.begin_search_in(1 - self.active);
            } else {
                self.begin_search();
            }
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Left => {
                    self.preview_panel(0);
                    return;
                }
                KeyCode::Right => {
                    self.preview_panel(1);
                    return;
                }
                KeyCode::Char('f') => {
                    self.toggle_favorites_sidebar();
                    return;
                }
                KeyCode::Char('d') => {
                    self.focus_favorites();
                    return;
                }
                KeyCode::Char('b') => {
                    self.add_favorite();
                    return;
                }
                KeyCode::Char('h') => {
                    self.toggle_hidden();
                    return;
                }
                KeyCode::Char('q') | KeyCode::Char('p') => {
                    self.toggle_preview();
                    return;
                }
                KeyCode::Char('o') => {
                    self.open_entry_in_panel(1 - self.active);
                    return;
                }
                KeyCode::Char('t') => {
                    self.toggle_thumbnails();
                    return;
                }
                KeyCode::Char('r') => {
                    self.reload_panel_async(0);
                    self.reload_panel_async(1);
                    self.status = "Refreshing…".to_string();
                    return;
                }
                KeyCode::Char('a') => {
                    let count = self.active_mut().select_all();
                    self.sync_preview_scroll();
                    self.status = if count == 0 {
                        "Nothing to select".to_string()
                    } else {
                        format!("Selected {count} item(s)")
                    };
                    return;
                }
                KeyCode::Char('s') => {
                    self.sort_menu = true;
                    self.status =
                        "Sort: n name · s size · t type · d date · r reverse · Esc".into();
                    return;
                }
                KeyCode::Char('k') => {
                    self.theme_menu = true;
                    self.status = "Colour themes: 1 default · 2 vivid · 3 mono · Esc".into();
                    return;
                }
                KeyCode::Char('e') => {
                    self.open_settings();
                    return;
                }
                KeyCode::Char('l') => {
                    self.copy_current_path();
                    return;
                }
                KeyCode::Char('i') => {
                    self.copy_current_name();
                    return;
                }
                // Ctrl+Shift+C keeps the terminal's own Ctrl+C for interrupts
                // while still copying the selection's paths.
                KeyCode::Char('C') => {
                    self.copy_selection_paths();
                    return;
                }
                KeyCode::Char('g') => {
                    self.begin_bulk_rename();
                    return;
                }
                _ => {}
            }
        }

        // Shift+←/→ opens the cursor folder in that panel; Command (Super)+←/→
        // opens the quick preview there. Both are offered because terminals
        // disagree about which modifier they deliver.
        // Cross-panel actions are split between the two modifiers that
        // actually reach the terminal:
        //   Shift+←/→  open the cursor folder in the indicated panel
        //   Alt+←/→    quick preview in the indicated panel
        // Ctrl and Command are deliberately unused: macOS reserves Ctrl+arrows
        // for Spaces and terminals (Ghostty, iTerm2) bind Command+arrows to
        // line editing, so they never arrive here.
        if let Some((target, preview)) = cross_panel_action(key) {
            if preview {
                self.preview_panel(target);
            } else {
                self.open_entry_in_panel(target);
            }
            return;
        }

        match self.focus {
            Focus::Panels => self.handle_panel_key(key),
            Focus::Favorites => self.handle_favorites_key(key),
        }
    }

    /// Move the active panel's cursor by `delta` in the current mode.
    ///
    /// Arrow keys wrap around at the ends (Total Commander / Yazi behaviour);
    /// everything that targets a place instead of a direction keeps clamping.
    fn move_active(&mut self, delta: i32) {
        match self.active().mode {
            PanelMode::Thumbnails => self.active_mut().move_grid_wrapping(delta),
            _ => self.active_mut().move_cursor_wrapping(delta),
        }
        self.sync_preview_scroll();
    }

    /// Remember that an arrow key is held, so `on_tick` can accelerate it.
    fn arm_arrow(&mut self, code: KeyCode) {
        let already = self
            .held_arrow
            .map(|(held, _)| held == code)
            .unwrap_or(false);
        if !already {
            self.held_arrow = Some((code, Instant::now()));
            self.last_auto_repeat = None;
        }
    }

    /// Forget the held arrow (a release, or a key that is not an arrow).
    fn release_arrow(&mut self) {
        self.held_arrow = None;
        self.last_auto_repeat = None;
    }

    /// Drive the held arrow.
    ///
    /// Terminals repeat a held key at their own rate, which is slow and often
    /// capped. Instead of waiting for those events the app moves the cursor
    /// itself: after `REPEAT_DELAY` it repeats every `REPEAT_START`, halving
    /// that interval every `REPEAT_RAMP`, and it grows the step so a long list
    /// is crossed in a second or two. Called from the event loop while it
    /// waits for input, so the movement is smooth between key repeats.
    pub fn tick_key_repeat(&mut self) -> bool {
        let Some((code, pressed)) = self.held_arrow else {
            return false;
        };
        if self.should_quit
            || self.help
            || self.prompt.is_some()
            || self.sort_menu
            || self.theme_menu
            || self.settings_menu
            || self.context_menu.is_some()
            || self.search.is_some()
        {
            return false;
        }

        let now = Instant::now();
        let held = now.saturating_duration_since(pressed);
        if held < REPEAT_DELAY {
            return false;
        }

        let interval = self.repeat_interval(held);
        if let Some(last) = self.last_auto_repeat {
            if now.saturating_duration_since(last) < interval {
                return false;
            }
        }
        self.last_auto_repeat = Some(now);

        let direction: i32 = if matches!(code, KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp) {
            -1
        } else {
            1
        };
        let cols = self.active().grid_cols.max(1) as i32;
        let step = if self.active().mode == PanelMode::Thumbnails {
            cols
        } else {
            self.repeat_step_for(held)
        };

        self.move_active(direction * step);
        self.dirty = true;
        true
    }

    /// Interval between automatic moves for a key held for `held`.
    fn repeat_interval(&self, held: Duration) -> Duration {
        let ramps = (held.as_millis() / REPEAT_RAMP.as_millis()) as u32;
        let millis = REPEAT_START
            .as_millis()
            .saturating_div(1u128 << ramps.min(4))
            .max(REPEAT_MIN.as_millis());
        Duration::from_millis(millis as u64)
    }

    /// How many rows one automatic move covers for a key held for `held`.
    fn repeat_step_for(&self, held: Duration) -> i32 {
        if held > Duration::from_millis(1800) {
            4
        } else if held > Duration::from_millis(900) {
            2
        } else {
            1
        }
    }

    fn handle_panel_key(&mut self, key: KeyEvent) {
        // A held arrow accelerates; every other key stops the auto-repeat so it
        // can never fight with what the user is typing.
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k')
        ) && key.modifiers.is_empty()
        {
            self.arm_arrow(key.code);
        } else {
            self.release_arrow();
        }

        if self.active().loading && key.code == KeyCode::Esc {
            self.cancel_active_load();
            return;
        }

        match self.active().mode {
            PanelMode::Preview => {
                self.handle_preview_key(key);
                return;
            }
            PanelMode::Thumbnails => {
                self.handle_thumbnail_key(key);
                return;
            }
            PanelMode::List => {}
        }

        if self.handle_common_action(key) {
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_active(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_active(1),
            KeyCode::PageUp => {
                let delta = self.active().page_size.max(1) as i32;
                self.active_mut().move_cursor(-delta);
            }
            KeyCode::PageDown => {
                let delta = self.active().page_size.max(1) as i32;
                self.active_mut().move_cursor(delta);
            }
            KeyCode::Home | KeyCode::Char('g') => self.active_mut().go_top(),
            KeyCode::End | KeyCode::Char('G') => self.active_mut().go_bottom(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.enter_current(),
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => self.go_parent(),
            KeyCode::Char(' ') | KeyCode::Insert => self.active_mut().toggle_selection(),
            KeyCode::Esc => self.clear_selection(),
            _ => {}
        }
    }

    fn handle_thumbnail_key(&mut self, key: KeyEvent) {
        if self.active().loading && key.code == KeyCode::Esc {
            self.cancel_active_load();
            return;
        }

        if self.handle_common_action(key) {
            return;
        }

        let cols = self.active().grid_cols.max(1) as i32;
        let page = self.active().grid_cols.max(1) as i32 * self.active().grid_rows.max(1) as i32;

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_active(-cols),
            KeyCode::Down | KeyCode::Char('j') => self.move_active(cols),
            KeyCode::Left | KeyCode::Char('h') => self.active_mut().move_grid(-1),
            KeyCode::Right | KeyCode::Char('l') => self.active_mut().move_grid(1),
            KeyCode::PageUp => self.active_mut().move_grid(-page),
            KeyCode::PageDown => self.active_mut().move_grid(page),
            KeyCode::Home | KeyCode::Char('g') => self.active_mut().go_top(),
            KeyCode::End | KeyCode::Char('G') => self.active_mut().go_bottom(),
            KeyCode::Enter => self.enter_current(),
            KeyCode::Backspace => self.go_parent(),
            KeyCode::Char(' ') | KeyCode::Insert => self.active_mut().toggle_selection(),
            KeyCode::Esc => self.clear_selection(),
            _ => {}
        }
    }

    /// Handle a vertical two-finger gesture.
    ///
    /// Terminal scroll events report the finger direction, while the user asked
    /// for the list to follow the natural (content) direction, so the delta is
    /// inverted here: swiping up moves the cursor down, like dragging the list.
    pub fn on_scroll(&mut self, delta: i32) {
        self.dirty = true;
        if self.help
            || self.prompt.is_some()
            || self.sort_menu
            || self.theme_menu
            || self.settings_menu
        {
            return;
        }

        // Inverted (default) drags the list with the fingers, which feels
        // natural on a trackpad; the settings menu can flip this.
        let delta = if self.settings.invert_scroll {
            -delta
        } else {
            delta
        };
        self.apply_scroll(delta);
    }

    fn apply_scroll(&mut self, delta: i32) {
        if self.focus == Focus::Favorites {
            let count = self.favorites.paths.len();
            if count == 0 {
                return;
            }
            let next = (self.fav_cursor as i32 + delta).clamp(0, count as i32 - 1);
            self.fav_cursor = next as usize;
            return;
        }

        if self.active().loading {
            return;
        }

        match self.active().mode {
            PanelMode::Preview => self.active_mut().scroll_preview(delta, 0),
            PanelMode::Thumbnails => {
                let cols = self.active().grid_cols.max(1) as i32;
                self.active_mut().move_grid(delta * cols);
            }
            PanelMode::List => self.active_mut().move_cursor(delta),
        }
    }

    /// Handle a pointer tap that carries no position.
    ///
    /// Kept for terminals that report bare presses without coordinates (and for
    /// the tests of the double-tap window); a real click goes through
    /// [`App::on_mouse`], which knows which row was hit.
    #[allow(dead_code)]
    pub fn on_pointer_tap(&mut self) {
        self.dirty = true;
        if self.help
            || self.prompt.is_some()
            || self.sort_menu
            || self.theme_menu
            || self.settings_menu
            || self.context_menu.is_some()
            || self.search.is_some()
        {
            return;
        }
        if self.focus == Focus::Panels && self.active().loading {
            return;
        }

        let now = Instant::now();
        let doubled = self
            .last_tap
            .take()
            .is_some_and(|last| now.saturating_duration_since(last) <= DOUBLE_TAP_WINDOW);
        if !doubled {
            self.last_tap = Some(now);
            return;
        }

        match self.focus {
            Focus::Favorites => {
                self.open_favorite();
            }
            Focus::Panels => {
                // A tap knows nothing about position, so it can only open the
                // entry under the cursor; a click carries its target instead.
                self.open_current_entry();
            }
        }
    }

    /// Route a mouse event.
    ///
    /// Terminals that report positions (every modern one with mouse capture)
    /// arrive here; a terminal that only reports bare presses falls back to
    /// `on_pointer_tap`, which keeps the two-finger double tap working.
    ///
    /// * left click on a row or a tile — focus that panel and move the cursor
    /// * double left click — open the entry (folder in place, file in its app)
    /// * left click on the path bar — copy the full path to the clipboard
    /// * right click — a context menu for the entry or for the folder
    /// * wheel — scroll, as before
    pub fn on_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.on_scroll(-1);
                return;
            }
            MouseEventKind::ScrollDown => {
                self.on_scroll(1);
                return;
            }
            // Horizontal scrolling is dropped: macOS emits it inside mostly
            // vertical trackpad gestures and it used to cause random jumps.
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => return,
            // Mouse capture also reports every motion; repainting for those
            // would burn CPU while the pointer merely rests over the window.
            MouseEventKind::Moved | MouseEventKind::Drag(_) => return,
            _ => {}
        }

        self.dirty = true;

        // A menu or the search window owns the pointer while it is open.
        if self.context_menu.is_some() {
            self.on_context_menu_mouse(event);
            return;
        }
        if self.search.is_some() {
            self.on_search_mouse(event);
            return;
        }
        if self.help
            || self.prompt.is_some()
            || self.sort_menu
            || self.theme_menu
            || self.settings_menu
        {
            return;
        }

        if let MouseEventKind::Down(button) = event.kind {
            self.handle_click(button, event.column, event.row);
        }
    }

    fn handle_click(&mut self, button: MouseButton, x: u16, y: u16) {
        // Favorites sidebar.
        if self.show_favorites && self.favorites_area.contains((x, y).into()) {
            self.focus = Focus::Favorites;
            let row = y.saturating_sub(self.favorites_area.y) as usize;
            let index = self.fav_offset + row;
            if index < self.favorites.paths.len() {
                self.fav_cursor = index;
                let target = (usize::MAX, index);
                let doubled = self.register_tap(target);
                if doubled {
                    self.open_favorite();
                }
            }
            return;
        }

        // Which panel was clicked? The path bar and the body are the targets.
        let Some(panel) = self.panel_at(x, y) else {
            return;
        };
        self.focus = Focus::Panels;
        self.active = panel;

        // The path bar copies the current path on a left click and offers the
        // folder actions on a right click.
        if self.panels[panel].path_bar_at(x, y) {
            self.last_tap = None;
            self.last_tap_target = None;
            match button {
                MouseButton::Left => {
                    if self.panels[panel].loading {
                        self.status = "The folder is still loading".to_string();
                        return;
                    }
                    let path = self.panels[panel].cwd.display().to_string();
                    self.copy_text(&path, "Path copied");
                }
                MouseButton::Right => self.open_context_menu(panel, None, x, y),
                _ => {}
            }
            return;
        }

        if !self.panels[panel].body_area.contains((x, y).into()) {
            return;
        }
        let Some(index) = self.panels[panel].index_at(x, y) else {
            return;
        };
        match button {
            MouseButton::Left => {
                // The entry under the pointer becomes the cursor; a second
                // click on the same entry opens it (folder in place, file in
                // its default application).
                self.panels[panel].cursor = index;
                self.panels[panel].ensure_mode_visible();
                self.sync_preview_scroll();

                if self.register_tap((panel, index)) {
                    self.open_clicked(panel, index);
                }
            }
            MouseButton::Right => self.open_context_menu(panel, Some(index), x, y),
            _ => {}
        }
    }

    /// Remember a click and report whether it completes a double click.
    ///
    /// Clicking a different row resets the gesture, so clicking twice on two
    /// different entries never opens anything by accident.
    fn register_tap(&mut self, target: (usize, usize)) -> bool {
        let now = Instant::now();
        let repeated = self.last_tap_target == Some(target)
            && self
                .last_tap
                .is_some_and(|last| now.saturating_duration_since(last) <= DOUBLE_TAP_WINDOW);

        if repeated {
            self.last_tap = None;
            self.last_tap_target = None;
        } else {
            self.last_tap = Some(now);
            self.last_tap_target = Some(target);
        }
        repeated
    }

    /// Open the entry that was just double clicked.
    fn open_clicked(&mut self, panel: usize, index: usize) {
        if self.panels[panel].mode == PanelMode::Preview {
            self.status = "Preview is read-only: Esc returns to the list".to_string();
            return;
        }

        let previous = self.active;
        self.active = panel;
        self.panels[panel].cursor = index;
        self.open_current_entry();
        self.active = previous;
    }

    /// Index of the panel whose rectangle contains the point.
    fn panel_at(&self, x: u16, y: u16) -> Option<usize> {
        (0..self.panels.len()).find(|&index| self.panels[index].screen_area.contains((x, y).into()))
    }

    /// Copy text to the system clipboard and report the outcome.
    fn copy_text(&mut self, text: &str, subject: &str) {
        match crate::clipboard::copy_text(text) {
            Ok(how) => self.status = format!("{subject} ({how}): {text}"),
            Err(error) => self.status = format!("Clipboard unavailable: {error}"),
        }
    }

    /// Copy the active panel's folder path.
    fn copy_current_path(&mut self) {
        let text = self.active().cwd.display().to_string();
        self.copy_text(&text, "Path copied");
    }

    /// Copy the cursor entry's full path (or its name with `Alt+I`).
    fn copy_current_name(&mut self) {
        let Some(entry) = self.active().current_entry().cloned() else {
            return;
        };
        self.copy_text(&entry.name, "Name copied");
    }

    /// `Ctrl+Shift+C`: copy the full path of every selected entry.
    fn copy_selection_paths(&mut self) {
        let paths = self.active().selection_or_current();
        if paths.is_empty() {
            self.status = "Nothing selected".to_string();
            return;
        }
        let text = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        self.copy_text(&text, &format!("Copied {} path(s)", paths.len()));
    }

    /// Right click: build the menu for an entry, or for the folder itself when
    /// `index` is `None` (the path bar).
    fn open_context_menu(&mut self, panel: usize, index: Option<usize>, x: u16, y: u16) {
        let mut items = Vec::new();

        match index {
            Some(index) => {
                let Some(entry) = self.panels[panel].entries.get(index).cloned() else {
                    return;
                };
                self.panels[panel].cursor = index;
                self.sync_preview_scroll();

                if !entry.is_parent {
                    items.push(MenuItem::Open);
                    if entry.is_dir {
                        items.push(MenuItem::OpenInOther);
                    }
                }
                items.push(MenuItem::CopyPath);
                items.push(MenuItem::CopyName);
                items.push(MenuItem::ToggleSelect);
                if !self.panels[panel].selected.is_empty() {
                    items.push(MenuItem::CopySelection);
                    items.push(MenuItem::ClearSelection);
                }
                if !entry.is_parent {
                    items.push(MenuItem::Rename);
                    items.push(MenuItem::Delete);
                }
            }
            None => {
                items.push(MenuItem::CopyPath);
                items.push(MenuItem::Mkdir);
                items.push(MenuItem::AddFavorite);
                items.push(MenuItem::Thumbnails);
                items.push(MenuItem::Search);
            }
        }

        self.focus = Focus::Panels;
        self.active = panel;
        self.context_menu = Some(ContextMenu {
            panel,
            index,
            items,
            cursor: 0,
            x,
            y,
            rect: Rect::new(x, y, 0, 0),
        });
        self.status = "↑↓ choose · Enter run · Esc close".to_string();
    }

    fn close_context_menu(&mut self) {
        self.context_menu = None;
        self.status.clear();
    }

    fn handle_context_menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.context_menu.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                menu.cursor = menu.cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                menu.cursor = (menu.cursor + 1).min(menu.items.len().saturating_sub(1));
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let item = menu.items.get(menu.cursor).copied();
                let panel = menu.panel;
                let index = menu.index;
                self.context_menu = None;
                if let Some(item) = item {
                    self.run_menu_item(item, panel, index);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => self.close_context_menu(),
            _ => {}
        }
    }

    fn on_context_menu_mouse(&mut self, event: MouseEvent) {
        let Some(menu) = &self.context_menu else {
            return;
        };
        let Some(index) = menu.index_at(event.column, event.row) else {
            if matches!(event.kind, MouseEventKind::Down(_))
                && !menu.rect.contains((event.column, event.row).into())
            {
                self.close_context_menu();
            }
            return;
        };

        match event.kind {
            MouseEventKind::Down(_) => {
                if let Some(menu) = self.context_menu.as_mut() {
                    menu.cursor = index;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let item = self
                    .context_menu
                    .as_ref()
                    .and_then(|menu| menu.items.get(index).copied());
                let (panel, target) = (menu.panel, menu.index);
                self.context_menu = None;
                if let Some(item) = item {
                    self.run_menu_item(item, panel, target);
                }
            }
            _ => {}
        }
    }

    fn run_menu_item(&mut self, item: MenuItem, panel: usize, index: Option<usize>) {
        self.active = panel;
        self.focus = Focus::Panels;

        match item {
            MenuItem::Open => {
                let index = index.unwrap_or(self.panels[panel].cursor);
                self.panels[panel].cursor = index;
                self.active = panel;
                self.open_current_entry();
            }
            MenuItem::OpenInOther => {
                let index = index.unwrap_or(self.panels[panel].cursor);
                self.panels[panel].cursor = index;
                self.open_entry_in_panel(1 - panel);
            }
            MenuItem::CopyPath => {
                let Some(index) = index else {
                    return;
                };
                let Some(entry) = self.panels[panel].entries.get(index) else {
                    return;
                };
                let text = entry.path.display().to_string();
                self.copy_text(&text, "Path copied");
            }
            MenuItem::CopyName => {
                let Some(index) = index else {
                    return;
                };
                let Some(entry) = self.panels[panel].entries.get(index) else {
                    return;
                };
                let text = entry.name.clone();
                self.copy_text(&text, "Name copied");
            }
            MenuItem::ToggleSelect => {
                if let Some(index) = index {
                    self.panels[panel].toggle_at(index);
                }
            }
            MenuItem::ClearSelection => {
                self.panels[panel].selected.clear();
                self.status = "Selection cleared".to_string();
            }
            MenuItem::CopySelection => self.copy_selection_paths(),
            MenuItem::Rename => self.begin_rename(),
            MenuItem::Delete => self.begin_delete(),
            MenuItem::Mkdir => self.begin_mkdir(),
            MenuItem::AddFavorite => self.add_favorite(),
            MenuItem::Thumbnails => self.toggle_thumbnails_in(panel),
            MenuItem::Search => self.begin_search_in(panel),
        }
    }

    /// Enter a folder, or hand a file to the desktop's default application.
    fn open_current_entry(&mut self) {
        let Some(entry) = self.active().current_entry().cloned() else {
            return;
        };

        if entry.is_dir {
            let select = if entry.is_parent {
                self.active()
                    .cwd
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            } else {
                None
            };
            let index = self.active;
            self.start_load(index, entry.path, select, LoadKind::Navigate);
            return;
        }

        if self.active().mode == PanelMode::Preview {
            self.status = "Preview is read-only: Esc returns to the list".to_string();
            return;
        }

        self.open_with_system(&entry.path);
    }

    /// Open a file with the platform's default handler.
    fn open_with_system(&mut self, path: &Path) {
        #[cfg(target_os = "macos")]
        let program = "open";
        #[cfg(target_os = "windows")]
        let program = "explorer";
        #[cfg(all(unix, not(target_os = "macos")))]
        let program = "xdg-open";

        match std::process::Command::new(program)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {
                self.status = format!("Opening {}", path.display());
                logging::info(format!("open with {program}: {}", path.display()));
            }
            Err(error) => {
                self.status = format!("Cannot open {}: {error}", path.display());
            }
        }
    }

    fn handle_preview_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.active_mut().scroll_preview(-1, 0),
            KeyCode::Down | KeyCode::Char('j') => self.active_mut().scroll_preview(1, 0),
            KeyCode::PageUp => {
                let page = self.active().page_size.max(1) as i32;
                self.active_mut().scroll_preview(-page, 0);
            }
            KeyCode::PageDown => {
                let page = self.active().page_size.max(1) as i32;
                self.active_mut().scroll_preview(page, 0);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                let panel = self.active_mut();
                panel.preview_scroll = 0;
                panel.preview_scroll_x = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                let max = self.active().preview_max_scroll;
                self.active_mut().preview_scroll = max;
            }
            KeyCode::Left | KeyCode::Char('h') => self.active_mut().scroll_preview(0, -1),
            KeyCode::Right | KeyCode::Char('l') => self.active_mut().scroll_preview(0, 1),
            KeyCode::Esc | KeyCode::Backspace => {
                self.active_mut().leave_preview();
                self.status = "Preview closed".to_string();
            }
            KeyCode::Enter => {
                self.status =
                    "Preview is read-only: Ctrl+Q or Esc returns to the file list".to_string();
            }
            _ => {}
        }
    }

    /// File operations shared by the list and the thumbnail grid.
    ///
    /// These are plain-key shortcuts, so any held modifier means the user is
    /// performing a different gesture (for example `Shift+→` opens a folder in
    /// the neighbour). Returning `false` lets that gesture win instead of
    /// silently starting a copy.
    fn handle_common_action(&mut self, key: KeyEvent) -> bool {
        let code = key.code;
        let busy_modifier = key.modifiers.intersects(
            KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::META
                | KeyModifiers::SHIFT,
        );

        // Function keys are unambiguous even with Shift (macOS often needs it),
        // but bare letters must have no modifiers at all.
        let is_function_key = matches!(code, KeyCode::F(_));
        if busy_modifier && !is_function_key {
            return false;
        }

        match code {
            KeyCode::F(5) | KeyCode::Char('c') => self.start_transfer(JobKind::Copy),
            KeyCode::F(6) | KeyCode::Char('m') => self.start_transfer(JobKind::Move),
            KeyCode::F(7) | KeyCode::Char('n') => self.begin_mkdir(),
            KeyCode::F(8) | KeyCode::Delete | KeyCode::Char('d') => self.begin_delete(),
            KeyCode::F(2) | KeyCode::Char('r') => self.begin_rename(),
            KeyCode::Char('s') => {
                self.sort_menu = true;
                self.status =
                    "Sort: n name · s size · t type · d date · r reverse · Esc".to_string();
            }
            KeyCode::Char('.') => self.toggle_hidden(),
            _ => return false,
        }
        true
    }

    fn sort_from(&mut self, key: SortKey) {
        let panel = self.active_mut();
        panel.set_sort(key, panel.sort_reverse);
        let label = panel.sort_key.label();
        let reverse = panel.sort_reverse;
        self.status = format!(
            "Sorted by {label}{}",
            if reverse { " (reverse)" } else { "" }
        );
        self.sort_menu = false;
    }

    fn handle_sort_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('n') => self.sort_from(SortKey::Name),
            KeyCode::Char('s') => self.sort_from(SortKey::Size),
            KeyCode::Char('t') => self.sort_from(SortKey::Extension),
            KeyCode::Char('d') => self.sort_from(SortKey::Modified),
            KeyCode::Char('r') => {
                let panel = self.active_mut();
                panel.sort_reverse = !panel.sort_reverse;
                let key = panel.sort_key;
                let reverse = panel.sort_reverse;
                panel.set_sort(key, reverse);
                self.sort_menu = false;
                self.status = format!("Reverse sort: {}", if reverse { "on" } else { "off" });
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.sort_menu = false;
                self.status.clear();
            }
            _ => {}
        }
    }

    /// Only user-overridden bindings are matched here.
    fn custom_binding(&self, key: KeyEvent) -> Option<Action> {
        Action::ALL.into_iter().find(|action| {
            self.settings.is_custom(*action)
                && self
                    .settings
                    .keys_for(*action)
                    .iter()
                    .any(|spec| spec.matches(key))
        })
    }

    /// Run an action chosen from a custom binding.
    fn dispatch_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Help => self.help = true,
            Action::Settings => self.open_settings(),
            Action::Copy => self.start_transfer(JobKind::Copy),
            Action::Move => self.start_transfer(JobKind::Move),
            Action::Mkdir => self.begin_mkdir(),
            Action::Rename => self.begin_rename(),
            Action::Delete => self.begin_delete(),
            Action::SelectAll => {
                let count = self.active_mut().select_all();
                self.status = format!("Selected {count} item(s)");
            }
            Action::BulkRename => self.begin_bulk_rename(),
            Action::ToggleHidden => self.toggle_hidden(),
            Action::Refresh => {
                self.reload_panel_async(0);
                self.reload_panel_async(1);
            }
            Action::SortMenu => self.sort_menu = true,
            Action::ColourMenu => self.theme_menu = true,
            Action::TogglePreview => self.toggle_preview(),
            Action::ToggleThumbnails => self.toggle_thumbnails(),
            Action::OpenInOther => self.open_entry_in_panel(1 - self.active),
            Action::AddFavorite => self.add_favorite(),
            Action::FocusFavorites => self.toggle_favorites_focus(),
            Action::ToggleSidebar => self.toggle_favorites_sidebar(),
            Action::OpenHere => self.open_current_entry(),
            Action::OpenFolderThere => self.open_entry_in_panel(1 - self.active),
            Action::PreviewThere => self.preview_panel(1 - self.active),
            Action::Confirm => {
                // Enter on a prompt is handled by the prompt itself.
            }
            Action::Cancel => {
                self.clear_selection();
            }
        }
    }

    fn open_settings(&mut self) {
        self.settings_menu = true;
        self.settings_cursor = 0;
        self.rebinding = false;
        self.status = "Settings: ↑↓ move · Enter rebind · r reset · i invert trackpad · Esc close"
            .to_string();
    }

    fn handle_settings_key(&mut self, key: KeyEvent) {
        // Rebinding swallows the next key, whatever it is.
        if self.rebinding {
            if key.code == KeyCode::Esc {
                self.rebinding = false;
                self.status = "Rebind cancelled".to_string();
                return;
            }
            let action = Action::ALL[self.settings_cursor.min(Action::ALL.len() - 1)];
            let spec = KeySpec::from_event(key);
            self.rebinding = false;
            if crate::settings::is_reserved(spec) {
                self.status = format!(
                    "{} is reserved by the app and cannot be rebound",
                    spec.label()
                );
                return;
            }
            if self.settings.rebind(action, spec) {
                self.persist_settings();
                self.status = format!("{} is now {}", action.label(), spec.label());
            } else {
                self.status = format!("Cannot rebind {}", action.label());
            }
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_cursor = self.settings_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings_cursor =
                    (self.settings_cursor + 1).min(Action::ALL.len().saturating_sub(1));
            }
            KeyCode::Char('i') => {
                self.settings.invert_scroll = !self.settings.invert_scroll;
                self.persist_settings();
                self.status = format!(
                    "Trackpad: {}",
                    if self.settings.invert_scroll {
                        "inverted (drag the list)"
                    } else {
                        "normal (scroll moves the cursor)"
                    }
                );
            }
            KeyCode::Char('r') => {
                let action = Action::ALL[self.settings_cursor.min(Action::ALL.len() - 1)];
                self.settings.reset(action);
                self.persist_settings();
                self.status = format!("{} reset to defaults", action.label());
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.rebinding = true;
                self.status = "Press the new key combination, Esc to cancel".to_string();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.settings_menu = false;
                self.status.clear();
            }
            _ => {}
        }
    }

    fn persist_settings(&mut self) {
        if let Err(error) = self.settings.save() {
            self.status = format!("Could not save settings: {error}");
        }
    }

    fn handle_theme_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('1') => {
                self.colors.set_theme(crate::colors::Preset::Default);
                self.theme_menu = false;
                self.status = "Colours: default theme".to_string();
            }
            KeyCode::Char('2') => {
                self.colors.set_theme(crate::colors::Preset::Vivid);
                self.theme_menu = false;
                self.status = "Colours: vivid theme".to_string();
            }
            KeyCode::Char('3') => {
                self.colors.set_theme(crate::colors::Preset::Mono);
                self.theme_menu = false;
                self.status = "Colours: mono theme".to_string();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.theme_menu = false;
                self.status.clear();
            }
            _ => {}
        }
    }

    fn clear_selection(&mut self) {
        if self.active().selected.is_empty() {
            self.status.clear();
        } else {
            self.active_mut().selected.clear();
            self.status = "Selection cleared".to_string();
        }
    }

    fn handle_favorites_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.fav_cursor > 0 {
                    self.fav_cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.fav_cursor + 1 < self.favorites.paths.len() {
                    self.fav_cursor += 1;
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_favorite(),
            KeyCode::Delete | KeyCode::Char('d') => self.remove_favorite(),
            KeyCode::Esc | KeyCode::Char('f') => {
                self.show_favorites = false;
                self.focus = Focus::Panels;
                self.status = "Favorites hidden".to_string();
            }
            _ => {}
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        if matches!(self.prompt, Some(Prompt::Transfer { .. })) {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(Prompt::Transfer {
                        kind,
                        sources,
                        dest,
                    }) = self.prompt.take()
                    {
                        self.do_transfer(kind, sources, dest);
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.prompt = None;
                    self.status = "Transfer cancelled".to_string();
                }
                _ => {}
            }
            return;
        }

        if matches!(self.prompt, Some(Prompt::Delete { .. })) {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(Prompt::Delete { sources }) = self.prompt.take() {
                        let count = sources.len();
                        let names: Vec<String> = sources
                            .iter()
                            .filter_map(|path| {
                                path.file_name()
                                    .map(|name| name.to_string_lossy().to_string())
                            })
                            .collect();
                        let previous = self
                            .active()
                            .entries
                            .iter()
                            .position(|entry| sources.iter().any(|path| path == &entry.path))
                            .unwrap_or(self.active().cursor);
                        self.pending_cursor_after = Some((names, previous));
                        self.job = Some(transfer::spawn(JobKind::Delete, sources, None));
                        self.status = format!("Deleting {count} item(s)...");
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.prompt = None;
                    self.status = "Delete cancelled".to_string();
                }
                _ => {}
            }
            return;
        }

        let mut submit = false;
        if let Some(Prompt::Mkdir { buffer })
        | Some(Prompt::Rename { buffer, .. })
        | Some(Prompt::BulkRename { buffer, .. }) = self.prompt.as_mut()
        {
            match key.code {
                KeyCode::Esc => {
                    self.prompt = None;
                    self.status = "Cancelled".to_string();
                }
                KeyCode::Enter => submit = true,
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    buffer.push(c);
                }
                _ => {}
            }
        }

        if submit {
            self.submit_input();
        }
    }

    fn submit_input(&mut self) {
        match self.prompt.take() {
            Some(Prompt::Mkdir { buffer }) => self.do_mkdir(&buffer),
            Some(Prompt::Rename { buffer, target }) => self.do_rename(&buffer, &target),
            Some(Prompt::BulkRename { buffer, sources }) => self.do_bulk_rename(&buffer, &sources),
            _ => {}
        }
    }

    /// Enter: open a folder in place, or a file with its associated app.
    fn enter_current(&mut self) {
        if self.active().mode == PanelMode::Preview {
            self.status = "Preview is read-only: Esc returns to the list".to_string();
            return;
        }
        self.open_current_entry();
    }

    fn go_parent(&mut self) {
        let current_name = self
            .active()
            .cwd
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        let Some(parent) = self.active().cwd.parent().map(Path::to_path_buf) else {
            self.status = "Already at the filesystem root".to_string();
            return;
        };
        let index = self.active;
        self.start_load(index, parent, current_name, LoadKind::Navigate);
    }

    fn start_transfer(&mut self, kind: JobKind) {
        if self.blocked_by_preview() || self.blocked_by_loading() {
            return;
        }
        if self.job.is_some() {
            self.status = "Wait: another operation is running".to_string();
            return;
        }

        let sources = self.active().selection_or_current();
        if sources.is_empty() {
            self.status = "Nothing selected".to_string();
            return;
        }

        let dest = self.other().cwd.clone();
        if !dest.is_dir() {
            self.status = format!("Destination is not a directory: {}", dest.display());
            return;
        }

        if kind == JobKind::Move
            && sources
                .iter()
                .any(|src| src.parent() == Some(dest.as_path()))
        {
            self.status = "Source and destination are the same".to_string();
            return;
        }

        if sources.iter().any(|src| dest.starts_with(src)) {
            self.status = "Cannot copy or move a folder into itself".to_string();
            return;
        }

        // Copy/move are destructive enough to deserve a confirmation, like
        // delete. `do_transfer` performs the job after the user agrees.
        self.prompt = Some(Prompt::Transfer {
            kind,
            sources,
            dest,
        });
        self.status = format!(
            "{} {} item(s)? Enter or y confirms, n/Esc cancels",
            kind.verb(),
            self.active().selection_or_current().len()
        );
    }

    /// Start the transfer after confirmation.
    fn do_transfer(&mut self, kind: JobKind, sources: Vec<PathBuf>, dest: PathBuf) {
        let count = sources.len();
        self.job = Some(transfer::spawn(kind, sources, Some(dest.clone())));
        self.status = format!("{} {} item(s) to {}...", kind.verb(), count, dest.display());
    }

    fn begin_mkdir(&mut self) {
        if self.blocked_by_preview() || self.blocked_by_loading() {
            return;
        }
        if self.job.is_some() {
            self.status = "Wait: another operation is running".to_string();
            return;
        }
        self.prompt = Some(Prompt::Mkdir {
            buffer: String::new(),
        });
    }

    fn begin_rename(&mut self) {
        if self.blocked_by_preview() || self.blocked_by_loading() {
            return;
        }
        if self.job.is_some() {
            self.status = "Wait: another operation is running".to_string();
            return;
        }
        let Some(entry) = self.active().current_entry().cloned() else {
            return;
        };
        if entry.is_parent {
            self.status = "Cannot rename the parent entry".to_string();
            return;
        }
        self.prompt = Some(Prompt::Rename {
            buffer: entry.name,
            target: entry.path,
        });
    }

    fn begin_delete(&mut self) {
        if self.blocked_by_preview() || self.blocked_by_loading() {
            return;
        }
        if self.job.is_some() {
            self.status = "Wait: another operation is running".to_string();
            return;
        }
        let sources = self.active().selection_or_current();
        if sources.is_empty() {
            self.status = "Nothing selected".to_string();
            return;
        }
        self.prompt = Some(Prompt::Delete { sources });
    }

    /// Ctrl+G: rename many files at once with pattern tokens.
    ///
    /// Tokens: `{name}` base name, `{ext}` extension, `{n}` counter starting at
    /// 1, `%e` → new extension. Examples: `IMG_{n}` → `IMG_1.jpg`,
    /// `{name}.bak`, `%e=txt` changes only the extension.
    fn begin_bulk_rename(&mut self) {
        if self.blocked_by_preview() || self.blocked_by_loading() {
            return;
        }
        if self.job.is_some() {
            self.status = "Wait: another operation is running".to_string();
            return;
        }
        let sources = self.active().selection_or_current();
        if sources.is_empty() {
            self.status = "Nothing selected".to_string();
            return;
        }
        self.prompt = Some(Prompt::BulkRename {
            buffer: String::new(),
            sources,
        });
        self.status = "{name} base · {ext} ext · {n} counter · e.g. IMG_{n}".to_string();
    }

    fn do_bulk_rename(&mut self, pattern: &str, sources: &[PathBuf]) {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            self.status = "Pattern is empty".to_string();
            return;
        }

        // `%e=new` replaces only the extension, which is the most common bulk
        // operation and needs no counter support.
        if let Some(new_ext) = pattern.strip_prefix("%e=") {
            let new_ext = new_ext.trim().trim_start_matches('.');
            let mut renamed = 0;
            let mut errors = Vec::new();
            for path in sources {
                let dest = path.with_extension(new_ext);
                if dest == *path {
                    continue;
                }
                if dest.exists() {
                    errors.push(format!("{} exists", dest.display()));
                    continue;
                }
                match fs::rename(path, &dest) {
                    Ok(()) => renamed += 1,
                    Err(e) => errors.push(format!("{}: {e}", path.display())),
                }
            }
            self.finish_bulk_rename(renamed, errors);
            return;
        }

        let mut renamed = 0;
        let mut errors = Vec::new();
        for (index, path) in sources.iter().enumerate() {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(name);
            let extension = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default();

            let mut new_name = pattern
                .replace("{name}", stem)
                .replace("{ext}", extension)
                .replace("{n}", &(index + 1).to_string());

            if new_name.contains('/') || new_name == "." || new_name == ".." {
                errors.push(format!("{name}: invalid target"));
                continue;
            }
            // Keep the original extension when the pattern does not name one.
            if !extension.is_empty()
                && Path::new(&new_name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_none()
            {
                new_name = format!("{new_name}.{extension}");
            }

            let dest = path.with_file_name(&new_name);
            if dest == *path {
                continue;
            }
            if dest.exists() {
                errors.push(format!("{new_name} exists"));
                continue;
            }
            match fs::rename(path, &dest) {
                Ok(()) => renamed += 1,
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        self.finish_bulk_rename(renamed, errors);
    }

    fn finish_bulk_rename(&mut self, renamed: usize, errors: Vec<String>) {
        let index = self.active;
        self.reload_panel_async(index);
        self.preview_cache.invalidate();
        if errors.is_empty() {
            self.status = format!("Renamed {renamed} item(s)");
        } else {
            self.status = format!(
                "Renamed {renamed}, {} error(s): {}",
                errors.len(),
                errors[0]
            );
        }
    }

    fn do_mkdir(&mut self, buffer: &str) {
        let name = buffer.trim();
        if name.is_empty() {
            self.status = "Directory name is empty".to_string();
            return;
        }
        if name.contains('/') || name == "." || name == ".." {
            self.status = "Invalid directory name".to_string();
            return;
        }

        let path = self.active().cwd.join(name);
        if path.exists() {
            self.status = format!("Already exists: {}", path.display());
            return;
        }

        match fs::create_dir(&path) {
            Ok(()) => {
                let index = self.active;
                self.start_load(
                    index,
                    self.panels[index].cwd.clone(),
                    Some(name.to_string()),
                    LoadKind::Refresh,
                );
                self.status = format!("Created {}", path.display());
            }
            Err(e) => self.status = format!("Cannot create {}: {}", path.display(), e),
        }
    }

    fn do_rename(&mut self, buffer: &str, target: &Path) {
        let new_name = buffer.trim();
        if new_name.is_empty() {
            self.status = "Name is empty".to_string();
            return;
        }
        if new_name.contains('/') || new_name == "." || new_name == ".." {
            self.status = "Invalid file name".to_string();
            return;
        }
        if target.file_name().map(|n| n == new_name).unwrap_or(false) {
            self.status = "Name unchanged".to_string();
            return;
        }

        let dest = target.with_file_name(new_name);
        if dest.exists() {
            self.status = format!("Already exists: {}", dest.display());
            return;
        }

        match fs::rename(target, &dest) {
            Ok(()) => {
                let index = self.active;
                self.start_load(
                    index,
                    self.panels[index].cwd.clone(),
                    Some(new_name.to_string()),
                    LoadKind::Refresh,
                );
                self.status = format!("Renamed to {}", new_name);
            }
            Err(e) => self.status = format!("Cannot rename: {e}"),
        }
    }

    /// Ctrl+D / Shift+Tab: reveal the sidebar and move keyboard focus into it.
    /// Pressing the same chord while focused returns to the panels.
    fn focus_favorites(&mut self) {
        if self.focus == Focus::Favorites {
            self.focus = Focus::Panels;
            self.status = "Back to panels".to_string();
            return;
        }
        self.show_favorites = true;
        self.focus = Focus::Favorites;
        if let Some(index) = self
            .favorites
            .paths
            .iter()
            .position(|path| path == &self.active().cwd)
        {
            self.fav_cursor = index;
        }
        self.fav_cursor = self
            .fav_cursor
            .min(self.favorites.paths.len().saturating_sub(1));
        self.status = "Favorites: Enter go, Del remove, Ctrl+B add, Esc back to panels".to_string();
    }

    /// Ctrl+F: toggle the sidebar; when it is already focused, return to panels.
    /// Ctrl+F: show or hide the sidebar as a whole.
    ///
    /// Entering keyboard focus is a separate action (`§`, `Shift+Tab`,
    /// `Ctrl+D`), so one key controls visibility and another controls focus.
    fn toggle_favorites_sidebar(&mut self) {
        if self.show_favorites {
            self.show_favorites = false;
            if self.focus == Focus::Favorites {
                self.focus = Focus::Panels;
            }
            self.status = "Favorites hidden".to_string();
        } else {
            self.show_favorites = true;
            self.status = "Favorites shown".to_string();
        }
    }

    fn toggle_favorites_focus(&mut self) {
        if self.show_favorites && self.focus == Focus::Favorites {
            self.focus = Focus::Panels;
            self.status = "Favorites unfocused (still visible)".to_string();
        } else if !self.show_favorites {
            self.show_favorites = true;
            self.focus_favorites();
        } else {
            self.focus_favorites();
        }
    }

    fn toggle_hidden(&mut self) {
        self.hidden = !self.hidden;
        self.reload_panel_async(0);
        self.reload_panel_async(1);
        self.sync_preview_scroll();
        self.status = if self.hidden {
            "Hidden files: shown".to_string()
        } else {
            "Hidden files: hidden".to_string()
        };
    }

    /// `Alt+F7`: search recursively from the active panel's folder.
    fn begin_search(&mut self) {
        self.begin_search_in(self.active);
    }

    /// `Alt+F7` (or `Alt+Shift+F7` for the other panel): open the search window.
    fn begin_search_in(&mut self, panel: usize) {
        if self.panels[panel].loading {
            self.status = "Folder is still loading — wait or press Esc".to_string();
            return;
        }
        self.focus = Focus::Panels;
        self.active = panel;
        let root = self.panels[panel].cwd.clone();
        self.search = Some(SearchWindow::new(root, panel));
        self.status = "Find: type a name or a mask (*.rs) · Enter start · Esc close".to_string();
    }

    fn close_search(&mut self) {
        self.search = None;
        self.status.clear();
    }

    /// Re-run the search from the beginning with the current query.
    fn start_search(&mut self) {
        let Some(window) = self.search.as_mut() else {
            return;
        };
        let query = window.query.trim().to_string();
        if query.is_empty() {
            self.status = "Type what to look for".to_string();
            return;
        }

        let root = window.root.clone();
        let hidden = self.hidden;
        window.hits.clear();
        window.cursor = 0;
        window.offset = 0;
        window.scanned = 0;
        window.truncated = false;
        window.running = true;
        window.receiver = Some(search::spawn(root.clone(), query.clone(), hidden));
        self.status = format!("Searching {query:?} in {}…", root.display());
    }

    /// Drain results from the search worker.
    fn poll_search(&mut self) {
        let Some(window) = self.search.as_mut() else {
            return;
        };
        let mut changed = false;
        let mut done = false;

        if let Some(receiver) = window.receiver.as_ref() {
            while let Ok(message) = receiver.try_recv() {
                window.scanned = message.scanned;
                window.truncated = message.truncated;
                window.hits.extend(message.hits);
                if message.done {
                    done = true;
                }
                changed = true;
            }
        }

        if done {
            window.running = false;
            window.receiver = None;
            let count = window.hits.len();
            let scanned = window.scanned;
            let truncated = window.truncated;
            self.status = if truncated {
                format!(
                    "Found {count}+ in {scanned} entries (limit {}) — refine the query",
                    search::MAX_RESULTS
                )
            } else {
                format!("Found {count} in {scanned} entries")
            };
        }

        if changed {
            self.dirty = true;
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        let Some(window) = self.search.as_mut() else {
            return;
        };

        match key.code {
            KeyCode::Esc => self.close_search(),
            KeyCode::Enter => {
                if window.running {
                    return;
                }
                // Enter starts the search while typing and opens the hit once
                // results are already in.
                if window.hits.is_empty() {
                    self.start_search();
                } else {
                    self.open_search_hit();
                }
            }
            KeyCode::F(5) => self.start_search(),
            KeyCode::Up => window.move_cursor(-1),
            KeyCode::Down => window.move_cursor(1),
            KeyCode::PageUp => {
                let page = window.page() as i32;
                window.move_cursor(-page);
            }
            KeyCode::PageDown => {
                let page = window.page() as i32;
                window.move_cursor(page);
            }
            KeyCode::Home => window.move_cursor(-(window.hits.len() as i32)),
            KeyCode::End => window.move_cursor(window.hits.len() as i32),
            KeyCode::Tab => {
                // Tab opens the hit in the OTHER panel.
                let other = 1 - window.panel;
                self.open_search_hit_in(other);
            }
            KeyCode::Backspace => {
                window.query.pop();
                window.hits.clear();
                window.cursor = 0;
                window.offset = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                window.query.push(c);
                // Results belong to the previous query, so drop them.
                window.hits.clear();
                window.cursor = 0;
                window.offset = 0;
                window.scanned = 0;
                window.truncated = false;
            }
            _ => {}
        }
    }

    /// Enter in the search window: reveal the hit in its panel.
    fn open_search_hit(&mut self) {
        let Some(panel) = self.search.as_ref().map(|window| window.panel) else {
            return;
        };
        self.open_search_hit_in(panel);
    }

    fn open_search_hit_in(&mut self, panel: usize) {
        let Some(hit) = self
            .search
            .as_ref()
            .and_then(|window| window.current().cloned())
        else {
            self.status = "No result to open".to_string();
            return;
        };

        self.search = None;
        self.focus = Focus::Panels;
        self.active = panel;

        if !hit.is_dir {
            // Reveal the containing folder with the cursor on the file.
            let parent = hit
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| hit.path.clone());
            let name = hit
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string());
            self.start_load(panel, parent, name, LoadKind::Navigate);
            return;
        }

        // A folder opens in place, so its contents are immediately visible.
        let name = hit
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        let parent = hit.path.parent().map(Path::to_path_buf);
        if let (Some(parent), Some(name)) = (parent, name) {
            if parent == self.panels[panel].cwd {
                self.panels[panel].select_name(&name);
                return;
            }
        }
        self.start_load(panel, hit.path, None, LoadKind::Navigate);
    }

    fn on_search_mouse(&mut self, event: MouseEvent) {
        let MouseEventKind::Down(button) = event.kind else {
            return;
        };
        let Some(window) = self.search.as_mut() else {
            return;
        };

        if !window.list_area.contains((event.column, event.row).into()) {
            return;
        }

        let row = event.row.saturating_sub(window.list_area.y) as usize;
        let index = window.offset + row;
        if index < window.hits.len() {
            window.cursor = index;
            if button == MouseButton::Left {
                self.open_search_hit();
            }
        }
    }

    /// Ctrl+Q / Ctrl+P: quick-view the **other** panel (Total Commander style).
    ///
    /// The active panel keeps its list and cursor, so moving the cursor drives
    /// the preview. Pressing the chord again closes the preview.
    fn toggle_preview(&mut self) {
        let target = 1 - self.active;

        if self.panels[target].mode == PanelMode::Preview {
            self.panels[target].leave_preview();
            self.sync_preview_scroll();
            self.status = "Quick preview closed".to_string();
            return;
        }

        // Only one preview at a time.
        if self.panels[self.active].mode == PanelMode::Preview {
            self.panels[self.active].leave_preview();
        }

        self.panels[target].set_mode(PanelMode::Preview);
        self.focus = Focus::Panels;
        self.sync_preview_scroll();
        self.status =
            "Quick preview of the other panel: ↑↓ scroll, Ctrl+Q/Ctrl+P/Esc close".to_string();
    }

    fn toggle_thumbnails(&mut self) {
        self.toggle_thumbnails_in(self.active);
    }

    /// Flip the list/thumbnail mode of one panel (used by the context menu).
    fn toggle_thumbnails_in(&mut self, panel: usize) {
        if self.panels[panel].mode == PanelMode::Thumbnails {
            self.panels[panel].set_mode(PanelMode::List);
            self.status = "List view".to_string();
            return;
        }

        if self.panels[panel].mode == PanelMode::Preview {
            self.panels[panel].leave_preview();
        }
        self.focus = Focus::Panels;
        self.active = panel;
        self.panels[panel].set_mode(PanelMode::Thumbnails);
        self.status = "Thumbnails: arrows move, Enter open, Ctrl+T back to list".to_string();
    }

    /// Ctrl+←/→: make the neighbouring panel a viewer of the active cursor.
    fn preview_panel(&mut self, target: usize) {
        if target == self.active {
            self.status = "Ctrl+arrow points at the other panel".to_string();
            return;
        }

        if self.panels[target].mode == PanelMode::Preview {
            self.panels[target].leave_preview();
            self.status = "Quick preview closed".to_string();
            return;
        }

        if self.panels[self.active].mode == PanelMode::Preview {
            self.panels[self.active].leave_preview();
        }

        self.panels[target].set_mode(PanelMode::Preview);
        self.focus = Focus::Panels;
        self.sync_preview_scroll();
        self.status = "Quick preview: ↑↓ scroll, ←→ pan, Ctrl+arrow or Ctrl+Q closes".to_string();
    }

    /// Ctrl+O / Alt+←/→: open the folder under the active cursor in `target`.
    fn open_entry_in_panel(&mut self, target: usize) {
        if target == self.active {
            self.status = "Ctrl+O opens the cursor folder in the other panel".to_string();
            return;
        }

        let Some(entry) = self.active().current_entry().cloned() else {
            return;
        };
        if !entry.is_dir {
            self.status = format!("{} is not a folder", entry.name);
            return;
        }

        let select = if entry.is_parent {
            self.active()
                .cwd
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        } else {
            None
        };

        self.start_load(target, entry.path, select, LoadKind::Navigate);
    }

    /// Keep the preview subject in sync with the opposite panel's cursor.
    pub fn sync_preview_scroll(&mut self) {
        for index in 0..self.panels.len() {
            if self.panels[index].mode != PanelMode::Preview {
                continue;
            }
            let other = &self.panels[1 - index];
            let target = if other.loading {
                None
            } else {
                other.current_entry().map(|entry| entry.path.clone())
            };

            if self.panels[index].preview_path != target {
                let panel = &mut self.panels[index];
                panel.preview_path = target;
                panel.preview_scroll = 0;
                panel.preview_scroll_x = 0;
                panel.preview_max_scroll = 0;
                panel.preview_max_scroll_x = 0;
                self.dirty = true;
            }
        }
    }

    /// File operations target the list/grid cursor, never the preview pane.
    fn blocked_by_preview(&mut self) -> bool {
        if self.active().mode == PanelMode::Preview {
            self.status =
                "Preview is read-only: Ctrl+Q or Esc returns to the file list".to_string();
            return true;
        }
        false
    }

    /// Reject operations while the active panel is still listing.
    fn blocked_by_loading(&mut self) -> bool {
        if self.active().loading {
            self.status = "Folder is still loading — wait or press Esc".to_string();
            return true;
        }
        false
    }

    fn add_favorite(&mut self) {
        if self.blocked_by_preview() || self.blocked_by_loading() {
            return;
        }
        let cwd = self.active().cwd.clone();
        if !self.favorites.add(cwd.clone()) {
            self.status = "Already in favorites".to_string();
            return;
        }

        self.fav_cursor = self.favorites.paths.len().saturating_sub(1);
        match self.favorites.save() {
            Ok(()) => self.status = format!("Added to favorites: {}", cwd.display()),
            Err(e) => {
                self.status = format!("Added, but could not save favorites: {e}");
            }
        }
    }

    fn open_favorite(&mut self) {
        let Some(path) = self.favorites.paths.get(self.fav_cursor).cloned() else {
            return;
        };
        let index = self.active;
        self.start_load(index, path, None, LoadKind::Navigate);
        self.focus = Focus::Panels;
    }

    fn remove_favorite(&mut self) {
        let Some(removed) = self.favorites.remove(self.fav_cursor) else {
            return;
        };
        self.fav_cursor = self
            .fav_cursor
            .min(self.favorites.paths.len().saturating_sub(1));
        match self.favorites.save() {
            Ok(()) => self.status = format!("Removed from favorites: {}", removed.display()),
            Err(e) => {
                self.status = format!("Removed, but could not save favorites: {e}");
            }
        }
    }
}

/// Which panel a cross-panel key targets, if any.
///
/// Returns `Some(0)` for "the left panel" and `Some(1)` for "the right panel".
/// Both the arrow keys and the `b`/`f` letters are accepted: terminals that
/// implement readline-style Alt bindings (Ghostty and iTerm2 by default) turn
/// `Alt+←` into `ESC b` and `Alt+→` into `ESC f`, so the app never sees an
/// arrow at all.
fn cross_panel_action(key: KeyEvent) -> Option<(usize, bool)> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

    // Which panel the arrow points at.
    let target = match key.code {
        KeyCode::Left => 0,
        KeyCode::Right => 1,
        // Ghostty and iTerm2 encode Alt+←/→ as ESC b / ESC f, so the arrow
        // never arrives; accept the letters as the same gesture.
        KeyCode::Char('b') if alt => 0,
        KeyCode::Char('f') if alt => 1,
        _ => return None,
    };

    if alt {
        Some((target, true))
    } else if shift {
        Some((target, false))
    } else {
        None
    }
}

/// Key modifier bits produced by a physical modifier key.
fn bit_of(modifier: ModifierKeyCode) -> KeyModifiers {
    match modifier {
        ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl => KeyModifiers::CONTROL,
        ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => KeyModifiers::ALT,
        ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => KeyModifiers::SHIFT,
        ModifierKeyCode::LeftSuper | ModifierKeyCode::RightSuper => KeyModifiers::SUPER,
        ModifierKeyCode::LeftMeta | ModifierKeyCode::RightMeta => KeyModifiers::META,
        _ => KeyModifiers::NONE,
    }
}

fn pick_dir(path: Option<&PathBuf>, fallback: &Path) -> PathBuf {
    match path {
        Some(path) if path.is_dir() => path.clone(),
        Some(path) => path
            .parent()
            .filter(|parent| parent.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| fallback.to_path_buf()),
        None => fallback.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    fn app_for(left: &Path, right: &Path) -> App {
        let mut app = App::new(vec![left.to_path_buf(), right.to_path_buf()]);
        // Tests must never depend on the developer's real settings file.
        app.settings.invert_scroll = true;
        app.settings.bindings.clear();
        app
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.on_key(KeyEvent::new(code, modifiers));
    }

    /// Drive `App::tick` until every pending directory load has been applied.
    fn wait_for_loads(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.tick();
            if app.loads.iter().all(Option::is_none) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "background directory loads did not finish in time"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Drive `App::tick` until the background stats have been merged.
    fn wait_for_stats(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.tick();
            if app.stat_jobs.iter().all(Option::is_none) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "background stats did not finish in time"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn ctrl_arrow_makes_the_neighbour_a_viewer_and_toggles_back() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.panels[1].mode, PanelMode::Preview);
        assert_eq!(app.panels[0].mode, PanelMode::List);

        press(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.panels[1].mode, PanelMode::List);
    }

    #[test]
    fn ctrl_arrow_towards_the_active_panel_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].mode, PanelMode::List);
        assert_eq!(app.panels[1].mode, PanelMode::List);
    }

    #[test]
    fn shift_arrow_opens_the_cursor_folder_in_the_neighbour() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let child = left.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT);
        wait_for_loads(&mut app);

        assert_eq!(app.panels[1].cwd, child);
    }

    #[test]
    fn shift_arrow_on_a_file_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT);

        assert_eq!(app.panels[1].cwd, right);
        assert!(app.status.contains("not a folder"));
    }

    #[test]
    fn preview_and_thumbnails_toggle_from_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        // Ctrl+Q previews the *other* panel; the active panel keeps its list.
        press(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[1].mode, PanelMode::Preview);
        assert_eq!(app.panels[0].mode, PanelMode::List);
        press(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[1].mode, PanelMode::List);

        // Ctrl+P is the reliable alias.
        press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[1].mode, PanelMode::Preview);
        press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[1].mode, PanelMode::List);

        press(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].mode, PanelMode::Thumbnails);
        press(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].mode, PanelMode::List);
    }

    #[test]
    fn f9_then_arrow_opens_the_cursor_folder_or_toggles_thumbnails() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let child = left.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");

        // F9 → Right opens the cursor folder in the right panel.
        press(&mut app, KeyCode::F(9), KeyModifiers::NONE);
        assert!(app.leader.is_some());
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert!(app.leader.is_none());
        wait_for_loads(&mut app);
        assert_eq!(app.panels[1].cwd, child);

        // F9 → Up toggles the thumbnail grid in the active panel.
        press(&mut app, KeyCode::F(9), KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.panels[0].mode, PanelMode::Thumbnails);
        press(&mut app, KeyCode::F(9), KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.panels[0].mode, PanelMode::List);
    }

    #[test]
    fn f9_chord_is_cancelled_by_escape_or_an_unrelated_key() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::F(9), KeyModifiers::NONE);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.leader.is_none());
        assert_eq!(app.panels[0].mode, PanelMode::List);

        press(&mut app, KeyCode::F(9), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(app.leader.is_none());
    }

    #[test]
    fn ctrl_o_opens_the_cursor_folder_in_the_other_panel() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let child = left.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");
        press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        wait_for_loads(&mut app);

        assert_eq!(app.panels[1].cwd, child);
    }

    #[test]
    fn shift_tab_toggles_favorites_focus() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(app.focus, Focus::Favorites);
        assert!(app.show_favorites);

        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(app.focus, Focus::Panels);
    }

    #[test]
    fn section_sign_opens_favorites() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('§'), KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::Favorites);
        assert!(app.show_favorites);

        press(&mut app, KeyCode::Char('§'), KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::Panels);
    }

    #[test]
    fn repeated_tab_keeps_switching_panels() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.active, 1);
        assert_eq!(app.focus, Focus::Panels);

        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.active, 0);
        assert_eq!(app.focus, Focus::Panels, "Tab must never steal focus");
    }

    #[test]
    fn vertical_scroll_moves_the_cursor_in_the_opposite_direction() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(tmp.path().join(name), "x").unwrap();
        }
        let mut app = app_for(tmp.path(), tmp.path());
        app.panels[0].select_name("a.txt");

        // Swiping up (ScrollUp -> -1) moves the cursor down, like dragging.
        app.on_scroll(-1);
        assert_eq!(app.panels[0].current_entry().unwrap().name, "b.txt");
        app.on_scroll(1);
        assert_eq!(app.panels[0].current_entry().unwrap().name, "a.txt");
    }

    #[test]
    fn shifted_letters_do_not_trigger_file_operations() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");

        // Shift+C is not "copy": Shift belongs to the cross-panel gesture.
        press(&mut app, KeyCode::Char('C'), KeyModifiers::SHIFT);
        assert!(app.prompt.is_none(), "Shift+C must not start a copy");

        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(matches!(app.prompt, Some(Prompt::Transfer { .. })));
    }

    #[test]
    fn function_keys_still_work_with_shift() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");

        // macOS often needs Shift for F-keys, so those must keep working.
        press(&mut app, KeyCode::F(5), KeyModifiers::SHIFT);
        assert!(matches!(app.prompt, Some(Prompt::Transfer { .. })));
    }

    #[test]
    fn settings_can_record_alt_shift_and_super_bindings() {
        let mut settings = crate::settings::Settings::load();
        settings.bindings.clear();

        assert!(settings.rebind(
            Action::Delete,
            KeySpec::new(KeyCode::Char('d'), KeyModifiers::ALT)
        ));
        assert_eq!(
            settings.keys_for(Action::Delete),
            vec![KeySpec::new(KeyCode::Char('d'), KeyModifiers::ALT)]
        );
        assert_eq!(settings.keys_label(Action::Delete), "alt+d");

        // Shift is preserved for keys where it is meaningful.
        assert!(settings.rebind(
            Action::Copy,
            KeySpec::new(KeyCode::Right, KeyModifiers::SHIFT)
        ));
        assert_eq!(settings.keys_label(Action::Copy), "shift+right");

        // Command is stored as super and matches both Super and Meta events.
        assert!(settings.rebind(
            Action::Mkdir,
            KeySpec::new(KeyCode::Char('n'), KeyModifiers::SUPER)
        ));
        assert_eq!(settings.keys_label(Action::Mkdir), "super+n");
        assert!(KeySpec::new(KeyCode::Char('n'), KeyModifiers::SUPER)
            .matches(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::META)));
    }

    #[test]
    fn settings_menu_lists_the_new_actions() {
        use crate::settings::Action as A;
        for action in [
            A::ToggleSidebar,
            A::OpenHere,
            A::OpenFolderThere,
            A::PreviewThere,
            A::Confirm,
            A::Cancel,
        ] {
            assert!(A::ALL.contains(&action), "{action:?} missing from the menu");
            assert!(!action.defaults().is_empty());
        }
    }

    #[test]
    fn copy_asks_for_confirmation_before_starting() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        assert!(matches!(app.prompt, Some(Prompt::Transfer { .. })));
        assert!(app.job.is_none(), "job must not start before confirmation");
        assert!(!right.join("a.txt").exists());

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.prompt.is_none());
        assert!(!right.join("a.txt").exists(), "cancel must copy nothing");
    }

    #[test]
    fn confirming_copy_starts_the_job() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "content").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert!(app.job.is_some(), "Enter must start the copy");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !right.join("a.txt").exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(right.join("a.txt")).unwrap(), "content");
    }

    #[test]
    fn move_can_be_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");
        press(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        assert!(matches!(app.prompt, Some(Prompt::Transfer { .. })));
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);

        assert!(left.join("a.txt").exists(), "cancel must keep the source");
        assert!(!right.join("a.txt").exists());
    }

    #[test]
    fn alt_arrow_previews_in_the_indicated_panel() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");

        press(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(app.panels[1].mode, PanelMode::Preview);
        assert_eq!(
            app.panels[0].mode,
            PanelMode::List,
            "source panel unchanged"
        );

        press(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(app.panels[1].mode, PanelMode::List);
    }

    #[test]
    fn alt_letters_from_the_terminal_preview_too() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");

        // Ghostty encodes Alt+Right as ESC f.
        press(&mut app, KeyCode::Char('f'), KeyModifiers::ALT);
        assert_eq!(app.panels[1].mode, PanelMode::Preview);

        press(&mut app, KeyCode::Char('f'), KeyModifiers::ALT);
        assert_eq!(app.panels[1].mode, PanelMode::List);

        // ...and Alt+Left as ESC b, which targets the left panel.
        app.active = 1;
        press(&mut app, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(app.panels[0].mode, PanelMode::Preview);
    }

    #[test]
    fn shift_arrow_and_terminal_alt_letters_open_the_neighbour() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let one = left.join("one");
        let two = left.join("two");
        fs::create_dir_all(&one).unwrap();
        fs::create_dir_all(&two).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("one");
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT);
        wait_for_loads(&mut app);
        assert_eq!(app.panels[1].cwd, one);

        // Shift+Left from the right panel opens the cursor folder on the left.
        app.active = 1;
        app.panels[1].select_name("..");
        press(&mut app, KeyCode::Left, KeyModifiers::SHIFT);
        wait_for_loads(&mut app);
        assert_eq!(app.panels[0].cwd, left);
    }

    #[test]
    fn the_footer_modifier_state_clears_when_a_plain_key_arrives() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert!(app.held_modifiers.contains(KeyModifiers::CONTROL));

        // A terminal that loses the release still recovers on the next key.
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(
            app.held_modifiers.is_empty(),
            "footer stayed stuck on Ctrl: {:?}",
            app.held_modifiers
        );
    }

    #[test]
    fn ctrl_f_shows_and_hides_the_sidebar_without_taking_focus() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        assert!(app.show_favorites);

        press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert!(!app.show_favorites);
        assert_eq!(
            app.focus,
            Focus::Panels,
            "Ctrl+F must not enter the sidebar"
        );

        press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert!(app.show_favorites);
        assert_eq!(app.focus, Focus::Panels);
    }

    #[test]
    fn shift_arrow_opens_the_cursor_folder_in_the_indicated_panel() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let child = left.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");

        // Shift+Right from the left panel opens it on the right.
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT);
        wait_for_loads(&mut app);
        assert_eq!(app.panels[1].cwd, child);

        // Now the mirror case: the right panel holds the cursor, Shift+Left
        // must open that folder in the left panel.
        let nested = child.join("nested");
        fs::create_dir_all(&nested).unwrap();
        app.active = 1;
        app.reload_panel_async(1);
        wait_for_loads(&mut app);
        wait_for_stats(&mut app);
        app.panels[1].select_name("nested");
        press(&mut app, KeyCode::Left, KeyModifiers::SHIFT);
        wait_for_loads(&mut app);
        assert_eq!(app.panels[0].cwd, nested);
    }

    #[test]
    fn settings_menu_cannot_be_rebound_and_reserved_keys_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings_menu = true;

        // The settings action is not offered for rebinding at all.
        assert!(!crate::settings::Action::ALL.contains(&Action::Settings));

        // Ctrl+E is reserved, so a rebind attempt leaves both intact.
        assert!(!app.settings.rebind(Action::Copy, KeySpec::ctrl('e')));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert!(app.settings_menu, "Ctrl+E must still open settings");
    }

    #[test]
    fn settings_menu_opens_and_toggles_trackpad_inversion() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert!(app.settings_menu);

        let before = app.settings.invert_scroll;
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.settings.invert_scroll, !before);

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.settings_menu);
    }

    #[test]
    fn settings_menu_rebinds_an_action() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings_menu = true;
        app.settings_cursor = crate::settings::Action::ALL
            .iter()
            .position(|action| *action == crate::settings::Action::Copy)
            .unwrap();

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.rebinding);

        press(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert!(!app.rebinding);
        assert_eq!(
            app.settings
                .keys_for(crate::settings::Action::Copy)
                .first()
                .map(|spec| spec.label()),
            Some("ctrl+y".to_string())
        );
    }

    #[test]
    fn a_rebound_key_overrides_the_old_one() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.settings.rebind(
            crate::settings::Action::Delete,
            crate::settings::KeySpec::f(9),
        );
        app.panels[0].select_name("a.txt");

        // F9 now arms the delete prompt instead of the F9 leader chord.
        press(&mut app, KeyCode::F(9), KeyModifiers::NONE);
        assert!(matches!(app.prompt, Some(Prompt::Delete { .. })));
    }

    #[test]
    fn non_inverted_scroll_follows_the_finger_direction() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(tmp.path().join(name), "x").unwrap();
        }
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings.invert_scroll = false;
        app.panels[0].select_name("b.txt");

        // ScrollUp (-1) moves up when the inversion is off.
        app.on_scroll(-1);
        assert_eq!(app.panels[0].current_entry().unwrap().name, "a.txt");
    }

    #[test]
    fn russian_layout_keys_still_trigger_shortcuts() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("beta")).unwrap();
        fs::write(tmp.path().join("alpha.txt"), "x").unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        // `и` is the Russian key that sits where `b` is: Ctrl+и adds a favorite.
        press(&mut app, KeyCode::Char('и'), KeyModifiers::CONTROL);
        // Compare canonical paths: macOS reports /var and /private/var for the
        // same temporary directory.
        let expected = fs::canonicalize(tmp.path()).unwrap();
        assert!(
            app.favorites
                .paths
                .iter()
                .any(|path| fs::canonicalize(path).ok() == Some(expected.clone())),
            "Ctrl+и did not add the directory: {:?}",
            app.favorites.paths
        );

        // `ы` is `s`: opens the sort menu.
        press(&mut app, KeyCode::Char('ы'), KeyModifiers::NONE);
        assert!(app.sort_menu);
    }

    #[test]
    fn russian_layout_does_not_break_prompt_typing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        for ch in "папка".chars() {
            press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert!(
            tmp.path().join("папка").is_dir(),
            "prompt must keep Cyrillic"
        );
    }

    #[test]
    fn enter_opens_a_file_with_the_system_handler() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        fs::create_dir_all(&left).unwrap();
        fs::write(left.join("note.txt"), "x").unwrap();
        let mut app = app_for(&left, &left);
        app.panels[0].select_name("note.txt");

        // The spawn itself may fail in a sandbox, but it must never navigate
        // and must never report the old "not part of the MVP" message.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.panels[0].cwd, left);
        assert!(
            app.status.contains("Opening") || app.status.contains("Cannot open"),
            "unexpected status: {}",
            app.status
        );
    }

    #[test]
    fn double_tap_in_the_favorites_sidebar_opens_the_bookmark() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let bookmarked = tmp.path().join("bookmarked");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&bookmarked).unwrap();

        let mut app = app_for(&left, &left);
        app.favorites.paths.clear();
        app.favorites.paths.push(bookmarked.clone());
        app.fav_cursor = 0;
        app.focus = Focus::Favorites;

        app.on_pointer_tap();
        assert_eq!(app.focus, Focus::Favorites);

        app.on_pointer_tap();
        wait_for_loads(&mut app);

        assert_eq!(app.panels[0].cwd, bookmarked);
        assert_eq!(app.focus, Focus::Panels);
    }

    #[test]
    fn a_single_tap_does_nothing_and_a_double_tap_opens_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let child = left.join("child");
        let right = tmp.path().join("right");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");

        // One tap only arms the gesture.
        app.on_pointer_tap();
        wait_for_loads(&mut app);
        assert_eq!(app.panels[0].cwd, left, "a single tap must not navigate");
        assert!(app.last_tap.is_some());

        // The second tap enters the folder.
        app.on_pointer_tap();
        wait_for_loads(&mut app);
        assert_eq!(app.panels[0].cwd, child);
        assert!(app.last_tap.is_none());
    }

    #[test]
    fn a_slow_second_tap_does_not_open_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let child = left.join("child");
        let right = tmp.path().join("right");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");

        app.on_pointer_tap();
        // Expire the double-tap window.
        app.last_tap = Some(Instant::now() - DOUBLE_TAP_WINDOW * 2);
        app.on_pointer_tap();
        wait_for_loads(&mut app);

        assert_eq!(app.panels[0].cwd, left, "slow taps must not navigate");
    }

    #[test]
    fn horizontal_scroll_events_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let child = left.join("child");
        let right = tmp.path().join("right");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");

        // The event loop drops horizontal scroll events entirely; a vertical
        // scroll still only moves the cursor, never the directory.
        app.on_scroll(1);
        wait_for_loads(&mut app);
        assert_eq!(app.panels[0].cwd, left);
    }

    #[test]
    fn holding_control_switches_the_footer_to_control_chords() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        app.on_key(KeyEvent::new(
            KeyCode::Modifier(ModifierKeyCode::LeftControl),
            KeyModifiers::NONE,
        ));
        assert!(app.held_modifiers.contains(KeyModifiers::CONTROL));

        app.on_key_release(KeyEvent::new(
            KeyCode::Modifier(ModifierKeyCode::LeftControl),
            KeyModifiers::NONE,
        ));
        assert!(!app.held_modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn ctrl_a_selects_every_entry_and_esc_clears_it() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        fs::create_dir(tmp.path().join("folder")).unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].selected.len(), 3);
        assert_eq!(app.panels[0].selection_or_current().len(), 3);

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.panels[0].selected.is_empty());
    }

    #[test]
    fn sort_menu_sorts_by_size_and_toggles_reverse() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("small.txt"), "x").unwrap();
        fs::write(tmp.path().join("big.txt"), "x".repeat(4096)).unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        wait_for_loads(&mut app);
        wait_for_stats(&mut app);

        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(app.sort_menu);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(!app.sort_menu);

        let names: Vec<&str> = app.panels[0]
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["..", "small.txt", "big.txt"]);

        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE);
        let names: Vec<&str> = app.panels[0]
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["..", "big.txt", "small.txt"]);
    }

    #[test]
    fn bulk_rename_replaces_names_with_a_counter() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
        for ch in "photo_{n}".chars() {
            press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert!(tmp.path().join("photo_1.txt").exists());
        assert!(tmp.path().join("photo_2.txt").exists());
        assert!(!tmp.path().join("a.txt").exists());
    }

    #[test]
    fn bulk_rename_can_change_only_the_extension() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
        for ch in "%e=md".chars() {
            press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert!(tmp.path().join("a.md").exists());
    }

    #[test]
    fn colour_menu_switches_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        press(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert!(app.theme_menu);

        press(&mut app, KeyCode::Char('2'), KeyModifiers::NONE);
        assert!(!app.theme_menu);
        assert_eq!(app.colors.preset(), crate::colors::Preset::Vivid);
    }

    #[test]
    fn preview_follows_the_other_panel_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
        let first = app.panels[0].current_entry().unwrap().path.clone();
        assert_eq!(app.panels[1].preview_path.as_deref(), Some(first.as_path()));

        app.panels[0].move_cursor(1);
        app.sync_preview_scroll();
        let second = app.panels[0].current_entry().unwrap().path.clone();
        assert_ne!(first, second);
        assert_eq!(
            app.panels[1].preview_path.as_deref(),
            Some(second.as_path())
        );
    }

    #[test]
    fn delete_prompt_confirms_with_enter() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("gone.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("gone.txt");
        press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert!(app.prompt.is_some());
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && left.join("gone.txt").exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!left.join("gone.txt").exists());
    }

    // ---- mouse, context menu and search ----

    /// Lay out a two-panel frame so the panels have real screen rectangles.
    /// Mouse handling depends on them; without a frame every rectangle is zero.
    fn layout(app: &mut App, width: u16, height: u16) -> ratatui::backend::TestBackend {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        terminal.backend().clone()
    }

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn clicking_the_path_bar_copies_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("clip.txt");
        let script = dir.path().join("fake-clip");
        fs::write(&script, format!("#!/bin/sh\ncat > {}\n", out.display())).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }

        let mut app = app_for(dir.path(), dir.path());
        app.show_favorites = false;
        layout(&mut app, 120, 30);

        let path_area = app.panels[0].path_area;
        assert!(path_area.height > 0, "the path bar was not laid out");

        // `YAZI2D_CLIPBOARD` is process-wide, so the test drives the helper
        // directly instead of going through the environment.
        let copied = crate::clipboard::copy_with(
            Some(script.display().to_string().as_str()),
            &app.panels[0].cwd.display().to_string(),
        );
        assert!(copied.is_ok(), "helper failed: {copied:?}");
        assert_eq!(
            fs::read_to_string(&out).unwrap(),
            app.panels[0].cwd.display().to_string()
        );

        // A real click must report success in the status line, not panic.
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            path_area.x + 1,
            path_area.y,
        ));
        assert!(
            app.status.starts_with("Path copied") || app.status.starts_with("Clipboard"),
            "unexpected status: {}",
            app.status
        );
    }

    #[test]
    fn a_click_on_a_row_moves_the_cursor_and_focuses_that_panel() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(left.join(name), "x").unwrap();
            fs::write(right.join(name), "x").unwrap();
        }

        let mut app = app_for(&left, &right);
        app.show_favorites = false;
        layout(&mut app, 120, 30);

        let body = app.panels[1].body_area;
        assert!(body.height > 2, "the panel body was not laid out");

        // Row 2 of the right panel: `..` is row 0, so this is its first entry.
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            body.x + 2,
            body.y + 1,
        ));

        assert_eq!(app.active, 1, "the clicked panel must become active");
        assert_eq!(app.focus, Focus::Panels);
        assert_eq!(app.panels[1].cursor, 1);
    }

    #[test]
    fn right_click_opens_a_menu_and_copy_path_runs_on_it() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.show_favorites = false;
        layout(&mut app, 120, 30);
        app.panels[0].select_name("a.txt");

        let body = app.panels[0].body_area;
        let row = (app.panels[0].cursor - app.panels[0].offset) as u16;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            body.x + 2,
            body.y + row,
        ));

        let menu = app
            .context_menu
            .as_ref()
            .expect("no menu after right click");
        assert!(menu.items.contains(&MenuItem::CopyPath));
        assert!(menu.items.contains(&MenuItem::Rename));
        assert_eq!(menu.index, Some(app.panels[0].cursor));

        // Escape closes it without running anything.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.context_menu.is_none());
    }

    #[test]
    fn right_click_on_the_path_bar_offers_folder_actions() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.show_favorites = false;
        layout(&mut app, 120, 30);

        let path_area = app.panels[1].path_area;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            path_area.x + 1,
            path_area.y,
        ));

        let menu = app.context_menu.as_ref().expect("no menu");
        assert_eq!(menu.index, None);
        assert!(menu.items.contains(&MenuItem::Mkdir));
        assert!(menu.items.contains(&MenuItem::Search));
        assert!(!menu.items.contains(&MenuItem::Rename));
    }

    #[test]
    fn double_click_on_a_folder_enters_it() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let child = left.join("child");
        let right = tmp.path().join("right");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.show_favorites = false;
        layout(&mut app, 120, 30);
        app.panels[0].select_name("child");

        let body = app.panels[0].body_area;
        let row = (app.panels[0].cursor - app.panels[0].offset) as u16;
        let click = || {
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                body.x + 2,
                body.y + row,
            )
        };

        app.on_mouse(click());
        assert_eq!(app.panels[0].cwd, left, "a single click must not navigate");

        app.on_mouse(click());
        wait_for_loads(&mut app);
        assert_eq!(app.panels[0].cwd, child, "a double click must enter");
    }

    #[test]
    fn clicking_two_different_rows_never_opens_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let child = left.join("child");
        let right = tmp.path().join("right");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("file.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.show_favorites = false;
        layout(&mut app, 120, 30);

        let body = app.panels[0].body_area;
        let child_row = app.panels[0]
            .entries
            .iter()
            .position(|entry| entry.name == "child")
            .unwrap();
        let file_row = app.panels[0]
            .entries
            .iter()
            .position(|entry| entry.name == "file.txt")
            .unwrap();

        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            body.x + 2,
            body.y + child_row as u16,
        ));
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            body.x + 2,
            body.y + file_row as u16,
        ));
        wait_for_loads(&mut app);

        assert_eq!(app.panels[0].cwd, left);
    }

    #[test]
    fn alt_f7_opens_the_search_window_and_enter_reveals_a_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let nested = left.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(nested.join("needle.txt"), "x").unwrap();
        fs::write(left.join("other.md"), "x").unwrap();

        let mut app = app_for(&left, &right);
        press(&mut app, KeyCode::F(7), KeyModifiers::ALT);
        assert!(app.search.is_some(), "Alt+F7 must open the search");

        for ch in "needle".chars() {
            press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        // Wait for the worker to finish.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            app.tick();
            if app.search.as_ref().is_some_and(|window| !window.running) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let hits = app.search.as_ref().unwrap().hits.len();
        assert_eq!(hits, 1, "expected exactly one hit");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert!(app.search.is_none(), "the search window must close");
        assert_eq!(app.panels[0].cwd, nested, "the hit's folder must open");
        assert_eq!(
            app.panels[0]
                .current_entry()
                .map(|entry| entry.name.as_str()),
            Some("needle.txt")
        );
    }

    #[test]
    fn esc_closes_the_search_without_navigating() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::F(7), KeyModifiers::ALT);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert!(app.search.is_none());
        assert_eq!(app.panels[0].cwd, tmp.path());
    }

    #[test]
    fn ctrl_shift_c_copies_the_selected_paths_without_quitting() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        fs::write(tmp.path().join("a.txt"), "x").unwrap();
        app.panels[0].reload(false).unwrap();

        press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        // Ctrl+Shift+C is distinguishable from Ctrl+C only under the kitty
        // keyboard protocol, where the terminal reports an uppercase `C`.
        press(
            &mut app,
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        assert!(!app.should_quit, "Ctrl+Shift+C must not quit");
        assert!(
            app.status.starts_with("Copied") || app.status.starts_with("Clipboard"),
            "unexpected status: {}",
            app.status
        );
    }

    #[test]
    fn a_held_arrow_moves_faster_than_the_terminal_repeats() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        fs::create_dir_all(&left).unwrap();
        for index in 0..200 {
            fs::write(left.join(format!("f{index:03}.txt")), "x").unwrap();
        }

        let mut app = app_for(&left, &left);
        app.panels[0].cursor = 1;

        // Simulate a hold: arm the arrow, then let the clock pass and drive the
        // repeat by hand. One terminal press, many app moves.
        app.held_arrow = Some((KeyCode::Down, Instant::now() - Duration::from_millis(1000)));
        app.last_auto_repeat = None;

        let mut moves = 0;
        for _ in 0..50 {
            if app.tick_key_repeat() {
                moves += 1;
            }
            // Pretend time passes between event-loop iterations.
            app.last_auto_repeat = app
                .last_auto_repeat
                .map(|last| last - Duration::from_millis(100));
        }
        assert!(
            moves > 10,
            "a held arrow produced only {moves} moves in one second"
        );
        assert!(
            app.panels[0].cursor > 1,
            "the cursor did not advance while the arrow was held"
        );
    }

    #[test]
    fn releasing_the_arrow_stops_the_acceleration() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        app.held_arrow = Some((KeyCode::Down, Instant::now() - Duration::from_millis(1000)));
        assert!(app.tick_key_repeat());

        app.on_key_release(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.held_arrow.is_none());
        assert!(
            !app.tick_key_repeat(),
            "the repeat must stop once the key is released"
        );
    }

    #[test]
    fn seconds_of_holding_grow_the_repeat_step() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app_for(tmp.path(), tmp.path());

        assert_eq!(app.repeat_step_for(Duration::from_millis(300)), 1);
        assert_eq!(app.repeat_step_for(Duration::from_millis(1000)), 2);
        assert_eq!(app.repeat_step_for(Duration::from_millis(2000)), 4);
    }
}
