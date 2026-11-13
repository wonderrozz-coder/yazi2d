use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
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
    Separator {
        buffer: String,
    },
    Delete {
        sources: Vec<PathBuf>,
        /// `false` for the trash (the default), `true` for a hard delete.
        permanent: bool,
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
    /// True while the sidebar is in arranging mode (`E`).
    ///
    /// Browsing and arranging are different jobs, so they get different modes:
    /// normally the cursor skips separators and blanks and lands only on
    /// bookmarks, which is what makes `Enter` predictable. In edit mode every
    /// row (including separators and blanks) is a stop, so it can be moved and
    /// deleted, and `S` inserts a separator where the cursor is.
    pub fav_edit: bool,
    /// Screen rectangle of the favorites list body (borders excluded).
    pub favorites_area: Rect,
    /// First favorites row currently drawn, so clicks map to the right bookmark.
    pub fav_offset: usize,
    /// Mounted volumes shown under the bookmarks.
    pub disks: Vec<crate::disks::Disk>,
    /// True while the cursor is in the Disks section, not the bookmarks.
    pub disks_focus: bool,
    /// Cursor row inside the Disks section.
    pub disk_cursor: usize,
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
    /// Which of the two slots of `settings_cursor` is being edited.
    pub settings_slot: usize,
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
    /// Last time the held arrow produced any key event.
    ///
    /// A release event is the normal way to end an auto-repeat, but it can be
    /// lost or never sent at all (Terminal.app does not report releases). This
    /// timestamp is the watchdog that stops the cursor moving on its own.
    pub last_arrow_activity: Option<Instant>,
    /// Set once the terminal has delivered a key-release event.
    ///
    /// The app moves the cursor itself for smoother acceleration, which is only
    /// safe when the terminal reports releases — otherwise nothing tells it that
    /// the key was let go and the cursor keeps walking. Until a release has
    /// actually been seen, the terminal's own key repeat drives the movement.
    pub releases_seen: bool,
    /// Last automatic move of a held arrow.
    pub last_auto_repeat: Option<Instant>,
    /// Folder sizes measured on demand (Space on a folder). Keyed by path so a
    /// measured size survives navigation and re-listing.
    pub folder_sizes: HashMap<PathBuf, u64>,
    /// Folders whose size is being measured right now.
    pub folder_size_pending: Vec<(PathBuf, Receiver<(PathBuf, u64)>)>,
    /// A row of the favorites sidebar waiting to be revealed as a folder.
    pub favorites_preview: Option<(usize, PathBuf)>,
    /// Bookmarked file to put the cursor on once its folder has loaded.
    pub favorite_file: Option<PathBuf>,
}

/// After the arrow has been held this long, the app moves the cursor itself.
const REPEAT_DELAY: Duration = Duration::from_millis(320);

/// How many spacer rows may pile up below the last group.
///
/// The free space under the bookmarks is what lets a row be dragged down and
/// left there; a handful of rows is enough to arrange groups, and without a cap
/// a held `↓` would append blanks forever.
const MAX_TRAILING_BLANKS: usize = 3;
/// Interval between two automatic moves at the first repeat speed.
const REPEAT_START: Duration = Duration::from_millis(55);
/// Fastest automatic repeat interval.
const REPEAT_MIN: Duration = Duration::from_millis(12);
/// How often the repeat speed is doubled.
const REPEAT_RAMP: Duration = Duration::from_millis(500);

/// Silence after which a held arrow counts as released.
///
/// The app moves the cursor itself instead of waiting for the terminal's slow
/// repeat, so it needs to know when the key is let go. A release event only
/// arrives with keyboard enhancement, and even then one can be lost (a dropped
/// read, a multiplexer, a terminal that keeps repeat switched off). A held key
/// produces events every few tens of milliseconds, so a whole second of silence
/// means the key is up: without this the cursor walked on its own until another
/// key was pressed.
const LOST_KEY_RELEASE: Duration = Duration::from_millis(600);

/// One row of the right-click menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuItem {
    Open,
    /// Only for folders: it is a different action from `Open` (which enters the
    /// folder in the same panel) and used to be labelled identically.
    OpenFolderInOther,
    /// Only for files: hand it to the desktop's default application.
    OpenWithApp,
    CopyPath,
    CopyName,
    ToggleSelect,
    CopySelection,
    ClearSelection,
    Rename,
    Delete,
    DeletePermanent,
    Mkdir,
    AddFavorite,
    AddSeparator,
    Thumbnails,
    Search,
    /// Always present: the pointer is the escape hatch when both settings-menu
    /// slots have been cleared (ADR-063).
    Settings,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::Open => "Open (folders: enter, files: preview)",
            MenuItem::OpenFolderInOther => "Open folder in the other panel",
            MenuItem::OpenWithApp => "Open with the default application",
            MenuItem::CopyPath => "Copy full path",
            MenuItem::CopyName => "Copy name",
            MenuItem::ToggleSelect => "Select / deselect",
            MenuItem::CopySelection => "Copy selected paths",
            MenuItem::ClearSelection => "Clear selection",
            MenuItem::Rename => "Rename",
            MenuItem::Delete => "Delete (to the trash)",
            MenuItem::DeletePermanent => "Delete permanently",
            MenuItem::Mkdir => "New folder",
            MenuItem::AddFavorite => "Add to favorites",
            MenuItem::AddSeparator => "Add a separator to favorites",
            MenuItem::Thumbnails => "Thumbnails on / off",
            MenuItem::Search => "Find (Alt+F7)",
            MenuItem::Settings => "Settings (key bindings)",
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
            fav_edit: false,
            favorites_area: Rect::new(0, 0, 0, 0),
            fav_offset: 0,
            disks: crate::disks::list(),
            disks_focus: false,
            disk_cursor: 0,
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
            settings_slot: 0,
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
            last_tap: None,
            last_tap_target: None,
            held_arrow: None,
            last_arrow_activity: None,
            releases_seen: false,
            last_auto_repeat: None,
            folder_sizes: HashMap::new(),
            folder_size_pending: Vec::new(),
            favorites_preview: None,
            favorite_file: None,
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
        self.poll_folder_sizes();
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
                self.status = match job.kind {
                    JobKind::Delete => format!(
                        "{} {} item(s) — restore from the system trash",
                        job.kind.done_verb(),
                        job.done
                    ),
                    _ => format!("{} {} item(s).", job.kind.done_verb(), job.done),
                };
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
                        // A bookmarked file was waiting for its folder: put the
                        // cursor on it now so the preview pane picks it up.
                        if let Some(file) = self.favorite_file.take() {
                            if let Some(position) = self.panels[index]
                                .entries
                                .iter()
                                .position(|entry| entry.path == file)
                            {
                                self.panels[index].cursor = position;
                                self.panels[index].ensure_visible();
                            }
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

    /// Collect finished folder-size measurements.
    fn poll_folder_sizes(&mut self) {
        let mut finished = Vec::new();
        let mut still_running = Vec::new();

        for (path, receiver) in self.folder_size_pending.drain(..) {
            match receiver.try_recv() {
                Ok((path, size)) => {
                    finished.push((path, size));
                }
                Err(TryRecvError::Empty) => still_running.push((path, receiver)),
                Err(TryRecvError::Disconnected) => {}
            }
        }
        self.folder_size_pending = still_running;

        for (path, size) in finished {
            self.folder_sizes.insert(path.clone(), size);
            self.status = format!("{}: {}", path.display(), crate::ui::human_size(size));
            self.dirty = true;
        }
    }

    /// Start measuring a folder's size in the background (Space on a folder).
    ///
    /// The result lands in `folder_sizes`, which the renderer prefers over the
    /// `"<DIR>"` placeholder.
    fn measure_folder(&mut self, path: &Path) {
        if self.folder_sizes.contains_key(path)
            || self
                .folder_size_pending
                .iter()
                .any(|(pending, _)| pending == path)
        {
            return;
        }

        let (sender, receiver) = mpsc::channel();
        let thread_path = path.to_path_buf();
        std::thread::spawn(move || {
            let started = Instant::now();
            let size = crate::panel::folder_size(&thread_path);
            logging::info(format!(
                "folder size in {} ms: {} = {size} bytes",
                started.elapsed().as_millis(),
                thread_path.display()
            ));
            let _ = sender.send((thread_path, size));
        });
        self.folder_size_pending
            .push((path.to_path_buf(), receiver));
        self.status = format!("Measuring {}…", path.display());
        self.dirty = true;
    }

    /// Size of a folder if it was measured, `None` while unknown.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn folder_size(&self, path: &Path) -> Option<u64> {
        self.folder_sizes.get(path).copied()
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
        // A release proves this terminal reports them, which is what makes the
        // app's own key acceleration safe. See `releases_seen`.
        self.releases_seen = true;
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

        // `Ctrl+C` is the terminal's interrupt and cannot be taken away from it:
        // it is the one key that must work even when the UI is confused. Every
        // other key, including quit, comes from the binding list (ADR-063).
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
            // No key is special here either: while rebinding, the next key is
            // the new binding (ADR-063). Help is reachable by its own binding.
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

        // `Ctrl+←/→` is an alias of "preview in that panel": some terminals
        // deliver the modifier, some turn it into `Alt+←/→`, and the alias costs
        // nothing. It is *not* a rebindable entry, so it cannot shadow anything.
        if key.modifiers == KeyModifiers::CONTROL
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            self.preview_panel(if key.code == KeyCode::Left { 0 } else { 1 });
            return;
        }

        // Help is an ordinary action now: `?` and `F1` are its two default
        // slots, and both can be changed in `Ctrl+E` (ADR-063).
        if let Some(action) = self.settings.action_for(key) {
            if action == Action::Help {
                self.help = true;
                return;
            }
        }

        // The settings menu's own slot is checked by the general dispatch below
        // like every other action; it needs no special case.

        // The favorites sidebar has its own key map: focus decides what a key
        // means, so `Del` removes a bookmark there and a file in the panels.
        // Dispatch must therefore happen *after* this branch, never before it —
        // a global `Del` was exactly what deleted a real folder while the
        // cursor was in the sidebar.
        if self.focus == Focus::Favorites {
            self.handle_favorites_key(key);
            return;
        }

        // Everything that is a rebindable shortcut is dispatched from the single
        // resolved list. Nothing below may hard-code one of those keys again:
        // that is exactly how `q` kept quitting after it was rebound.
        if let Some(action) = self.settings.action_for(key) {
            self.dispatch_action(action);
            return;
        }

        self.handle_panel_key(key);
    }

    /// Move the active panel's cursor by `delta` in the current mode.
    ///
    /// Arrow keys wrap around at the ends (Total Commander / Yazi behaviour);
    /// everything that targets a place instead of a direction keeps clamping.
    /// Space/Insert: select the entry and, for a folder, measure its size.
    ///
    /// The measurement runs on a worker thread, so selecting a 200 GB folder
    /// never blocks the UI; the number appears in the `Dir` column and in the
    /// status line when it is ready.
    fn toggle_selection_and_measure(&mut self) {
        let entry = self.active().current_entry().cloned();
        self.active_mut().toggle_selection();
        if let Some(entry) = entry {
            if entry.is_dir && !entry.is_parent {
                self.measure_folder(&entry.path);
            }
        }
        self.sync_preview_scroll();
    }

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
        // Any event for the arrow proves the key is still down, whatever the
        // acceleration timer says. See `LOST_KEY_RELEASE`.
        self.last_arrow_activity = Some(Instant::now());
    }

    /// Forget the held arrow (a release, or a key that is not an arrow).
    fn release_arrow(&mut self) {
        self.held_arrow = None;
        self.last_arrow_activity = None;
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

        // Only accelerate when the terminal has told us it reports releases.
        // Otherwise nothing can end the repeat reliably (Terminal.app sends no
        // releases at all), and the cursor would keep walking after the key is
        // up. The terminal's own repeat still moves the cursor in that case.
        if !self.releases_seen {
            return false;
        }

        // A lost release must not leave the cursor walking on its own: a key
        // that is really down keeps sending events, so a long silence means it
        // was let go.
        if let Some(activity) = self.last_arrow_activity {
            if now.saturating_duration_since(activity) > LOST_KEY_RELEASE {
                self.release_arrow();
                return false;
            }
        } else {
            self.release_arrow();
            return false;
        }

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
        // The sidebar has its own cursor, so a held arrow there must move it —
        // the panels must not scroll while the pointer is on a bookmark.
        if self.focus == Focus::Favorites {
            let step = self.repeat_step_for(held);
            self.move_favorite(direction * step);
            self.dirty = true;
            return true;
        }
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
        // `S` is the sidebar's separator key. In the panels it used to do
        // nothing at all, which read as "the feature is broken"; say where the
        // key works instead.
        if key.code == KeyCode::Char('S') && key.modifiers.is_empty() {
            self.status =
                "Separators live in the sidebar: press § or Ctrl+D to focus Favorites, then S"
                    .to_string();
            return;
        }

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
            // Only Enter opens a file in an external application. `→`/`l` are
            // listed here to enter a folder, but a file is never launched by a
            // key that also means "move right".
            KeyCode::Enter => self.enter_current(),
            KeyCode::Right | KeyCode::Char('l') => self.enter_folder_only(),
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => self.go_parent(),
            KeyCode::Char(' ') | KeyCode::Insert => self.toggle_selection_and_measure(),
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
            KeyCode::Char(' ') | KeyCode::Insert => self.toggle_selection_and_measure(),
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
            let count = self.favorites.len();
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
            if index < self.favorites.len() {
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

    /// Open the context menu for the cursor entry from the keyboard.
    ///
    /// The menu used to be reachable only with a right click, which left
    /// keyboard-only users (and terminals without mouse reporting) unable to
    /// use it. It is a rebindable action now, so it also shows up in the
    /// settings list and in `F1`.
    fn open_context_menu_for_cursor(&mut self) {
        let panel = self.active;
        let index = self.panels[panel].cursor;
        // Anchor it under the cursor row when we know where the list is; the
        // renderer clamps it to the screen either way.
        let area = self.panels[panel].body_area;
        let x = area.x + 2;
        let y = area.y + (index.saturating_sub(self.panels[panel].offset)) as u16;
        self.open_context_menu(panel, Some(index), x, y);
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
                        items.push(MenuItem::OpenFolderInOther);
                    } else {
                        items.push(MenuItem::OpenWithApp);
                    }
                    // Files are bookmarks too, so this is not folder-only.
                    items.push(MenuItem::AddFavorite);
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
                    items.push(MenuItem::DeletePermanent);
                }
                items.push(MenuItem::Settings);
            }
            None => {
                items.push(MenuItem::CopyPath);
                items.push(MenuItem::Mkdir);
                items.push(MenuItem::AddFavorite);
                items.push(MenuItem::AddSeparator);
                items.push(MenuItem::Thumbnails);
                items.push(MenuItem::Search);
                items.push(MenuItem::Settings);
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
            MenuItem::OpenFolderInOther => {
                let index = index.unwrap_or(self.panels[panel].cursor);
                self.panels[panel].cursor = index;
                self.open_entry_in_panel(1 - panel);
            }
            MenuItem::OpenWithApp => {
                let index = index.unwrap_or(self.panels[panel].cursor);
                self.panels[panel].cursor = index;
                if let Some(entry) = self.panels[panel].current_entry().cloned() {
                    self.open_with_system(&entry.path);
                }
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
            MenuItem::DeletePermanent => self.begin_delete_kind(true),
            MenuItem::Mkdir => self.begin_mkdir(),
            MenuItem::AddFavorite => {
                if let Some(index) = index {
                    self.panels[panel].cursor = index;
                }
                self.add_favorite();
            }
            MenuItem::AddSeparator => {
                self.focus = Focus::Favorites;
                self.begin_separator();
            }
            MenuItem::Thumbnails => self.toggle_thumbnails_in(panel),
            MenuItem::Search => self.begin_search_in(panel),
            MenuItem::Settings => self.open_settings(),
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

    /// Run an action chosen by a key binding.
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
            Action::DeletePermanent => self.begin_delete_kind(true),
            Action::SelectAll => {
                let count = self.active_mut().select_all();
                self.status = format!("Selected {count} item(s)");
            }
            Action::BulkRename => self.begin_bulk_rename(),
            Action::ToggleHidden => self.toggle_hidden(),
            Action::Refresh => {
                self.reload_panel_async(0);
                self.reload_panel_async(1);
                self.refresh_disks();
            }
            Action::SortMenu => self.sort_menu = true,
            Action::ColourMenu => self.theme_menu = true,
            Action::ContextMenu => self.open_context_menu_for_cursor(),
            Action::TogglePreview => self.toggle_preview(),
            Action::ToggleThumbnails => self.toggle_thumbnails(),
            Action::OpenInOther => self.open_entry_in_panel(1 - self.active),
            Action::AddFavorite => self.add_favorite(),
            Action::FocusFavorites => self.toggle_favorites_focus(),
            Action::ToggleSidebar => self.toggle_favorites_sidebar(),
            Action::CopyPath => self.copy_current_path(),
            Action::CopyName => self.copy_current_name(),
            Action::CopySelection => self.copy_selection_paths(),
            Action::SwitchPanel => {
                self.focus = Focus::Panels;
                self.active ^= 1;
                self.status = format!(
                    "Active panel: {}",
                    if self.active == 0 { "Left" } else { "Right" }
                );
            }
            Action::OpenHere => self.open_current_entry(),
            Action::PreviewThere => self.preview_panel(1 - self.active),
            Action::Confirm => {
                // Enter on a prompt is handled by the prompt itself.
            }
            Action::Cancel => {
                self.clear_selection();
            }
            // Reordering belongs to the sidebar; in a panel these keys do
            // nothing so they cannot move a bookmark by accident.
            Action::FavoriteSeparator => {
                if self.focus == Focus::Favorites && self.fav_edit {
                    self.begin_separator();
                }
            }
            Action::FavoriteBlank => {
                if self.focus == Focus::Favorites && self.fav_edit {
                    self.insert_blank_after_cursor();
                }
            }
            Action::FavoriteArrange => {
                if self.focus == Focus::Favorites && !self.disks_focus {
                    self.toggle_fav_edit();
                }
            }
            Action::FavoriteUp => {
                if self.focus == Focus::Favorites && !self.disks_focus {
                    self.shift_favorite(-1);
                }
            }
            Action::FavoriteDown => {
                if self.focus == Focus::Favorites && !self.disks_focus {
                    self.shift_favorite(1);
                }
            }
        }
    }

    fn open_settings(&mut self) {
        self.settings_menu = true;
        self.settings_cursor = 0;
        self.settings_slot = 0;
        self.rebinding = false;
        self.status = "Settings: ↑↓ action · ←→ slot · Enter assign · Del clear · r reset · i trackpad · Esc close"
            .to_string();
    }

    /// The action currently under the settings cursor.
    fn settings_action(&self) -> Action {
        let actions = Action::menu_actions();
        actions[self.settings_cursor.min(actions.len().saturating_sub(1))]
    }

    fn handle_settings_key(&mut self, key: KeyEvent) {
        // Assigning swallows the next key, whatever it is.
        if self.rebinding {
            if key.code == KeyCode::Esc {
                self.rebinding = false;
                self.status = "Rebind cancelled".to_string();
                return;
            }
            let action = self.settings_action();
            let slot = self.settings_slot;
            let spec = KeySpec::from_event(key);
            self.rebinding = false;

            match self.settings.assign(action, slot, spec) {
                Ok(()) => {
                    self.persist_settings();
                    self.status = format!(
                        "{}: slot {} is now {}",
                        action.label(),
                        slot + 1,
                        spec.label()
                    );
                }
                Err(reason) => self.status = format!("Cannot assign: {reason}"),
            }
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_cursor = self.settings_cursor.saturating_sub(1);
                self.settings_slot = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let last = Action::menu_actions().len().saturating_sub(1);
                self.settings_cursor = (self.settings_cursor + 1).min(last);
                self.settings_slot = 0;
            }
            // The two slots are equal citizens: ←/→ picks which one to edit.
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                self.settings_slot ^= 1;
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
            KeyCode::Char('g') => {
                self.settings.icons = !self.settings.icons;
                self.persist_settings();
                self.status = format!(
                    "Nerd Font icons: {}",
                    if self.settings.icons { "on" } else { "off" }
                );
            }
            KeyCode::Char('r') => {
                let action = self.settings_action();
                self.settings.reset(action);
                self.persist_settings();
                self.status = format!("{} reset to defaults", action.label());
            }
            // Delete/Backspace empties the focused slot, which is how a second
            // key that is not wanted is removed.
            KeyCode::Delete | KeyCode::Backspace => {
                let action = self.settings_action();
                let slot = self.settings_slot;
                self.settings.clear_slot(action, slot);
                self.persist_settings();
                self.status = format!("{}: slot {} cleared", action.label(), slot + 1);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.rebinding = true;
                self.status = format!(
                    "Press the key for slot {} · Esc cancels",
                    self.settings_slot + 1
                );
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
        // The sidebar needs the same held-arrow acceleration as the panels:
        // without it a held `↓` only moved at the terminal's own repeat rate,
        // which is why the cursor felt much slower here than in a panel.
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k')
        ) && key.modifiers.is_empty()
        {
            self.arm_arrow(key.code);
        } else {
            self.release_arrow();
        }

        // `Esc` deliberately does nothing here: the favorites panel is left with
        // the key that opened it, so a stray Esc cannot hide it mid-browse.
        if let Some(action) = self.settings.action_for(key) {
            match action {
                Action::FocusFavorites => {
                    self.toggle_favorites_focus();
                    return;
                }
                Action::AddFavorite => {
                    self.add_favorite();
                    return;
                }
                Action::ToggleSidebar => {
                    self.toggle_favorites_sidebar();
                    return;
                }
                Action::OpenHere => {
                    // `Enter` is the same "open what the cursor is on" gesture
                    // as the `Enter/→` branch below, so it has to honour the
                    // Disks section too. Calling `open_favorite` directly made
                    // a disk row unopenable: the cursor was visibly on the
                    // volume, and Enter jumped to the last bookmark instead.
                    if self.disks_focus {
                        self.open_disk();
                    } else {
                        self.open_favorite();
                    }
                    return;
                }
                Action::FavoriteUp => {
                    if !self.disks_focus {
                        self.shift_favorite(-1);
                    }
                    return;
                }
                Action::FavoriteDown => {
                    if !self.disks_focus {
                        self.shift_favorite(1);
                    }
                    return;
                }
                Action::Delete => {
                    // Arranging owns deletion: in browse mode `Del` on a
                    // bookmark would be a surprise in a list where the cursor
                    // is meant to browse, so it is edit-only (ADR-068).
                    if self.fav_edit {
                        self.remove_favorite();
                    } else {
                        self.status = "Deletion lives in arranging mode: press E".to_string();
                    }
                    return;
                }
                Action::Quit => {
                    self.should_quit = true;
                    return;
                }
                Action::Help => {
                    self.help = true;
                    return;
                }
                Action::FavoriteArrange => {
                    if !self.disks_focus {
                        self.toggle_fav_edit();
                    }
                    return;
                }
                Action::FavoriteSeparator => {
                    if self.fav_edit {
                        self.begin_separator();
                    } else {
                        self.status = "Separators live in arranging mode: press E".to_string();
                    }
                    return;
                }
                Action::FavoriteBlank => {
                    if self.fav_edit {
                        self.insert_blank_after_cursor();
                    } else {
                        self.status = "Empty rows live in arranging mode: press E".to_string();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Separators are captions: the cursor skips them, so a bookmark is
        // always what Enter acts on.
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_favorite(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_favorite(1),
            // PgUp/PgDn now come through `Action::FavoriteUp/Down`, so they are
            // rebindable and a Mac (which has no reachable PgUp/PgDn) can use
            // `K`/`J`. `Home`/`End` stay hardcoded: they are navigation, not a
            // reorder action.
            KeyCode::Home | KeyCode::Char('g') => {
                self.fav_cursor = 0;
                self.select_favorite_row();
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.fav_cursor = self.favorites.len().saturating_sub(1);
                self.select_favorite_row();
            }
            // `S`, `B` and `E` are ordinary actions (`Add separator`,
            // `Add empty row`, `Arrange favorites`), so they are dispatched from
            // the binding list above and their chips in the footer match
            // whatever the user assigned.
            // Enter *and* `→`/`l` open the bookmark: `→` used to work and must
            // keep working, `Enter` is the discoverable one.
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if self.disks_focus {
                    self.open_disk();
                } else {
                    self.open_favorite();
                }
            }
            _ => {}
        }
    }

    /// Move the favorites cursor by `delta`, skipping separators, and reveal the
    /// row in the active panel as the cursor lands on it.
    ///
    /// This is the "browse without committing" behaviour: the panel follows the
    /// cursor (a folder is opened, a file is previewed), while Enter chooses.
    fn move_favorite(&mut self, delta: i32) {
        if delta < 0 && self.disks_focus && self.disk_cursor == 0 {
            self.disks_focus = false;
            // Land on the last row that is not padding: it is what the user
            // was looking at before stepping into the volumes.
            let last_real = self
                .favorites
                .entries
                .iter()
                .rposition(|entry| !entry.is_blank())
                .unwrap_or(0);
            self.fav_cursor = last_real.min(self.favorites.len().saturating_sub(1));
            self.select_favorite_row();
            return;
        }
        if self.disks_focus {
            let last = self.disks.len().saturating_sub(1);
            let next = (self.disk_cursor as i32 + delta).clamp(0, last as i32) as usize;
            if next != self.disk_cursor {
                self.disk_cursor = next;
                self.reveal_disk();
            }
            return;
        }

        if self.fav_edit {
            self.move_favorite_edit(delta);
        } else {
            self.move_favorite_browse(delta);
        }
    }

    /// Browsing: the cursor visits only bookmarks, so `Enter` always has a
    /// target. Separators and blanks are skipped (ADR-068).
    fn move_favorite_browse(&mut self, delta: i32) {
        let len = self.favorites.len() as i32;
        if len == 0 {
            return;
        }
        if delta > 0 {
            // Into the Disks section once there is nothing below.
            let next = (self.fav_cursor as i32 + 1)..len;
            let found =
                next.filter(|index| self.favorites.entries[*index as usize].path().is_some());
            match found.into_iter().next() {
                Some(index) => {
                    self.fav_cursor = index as usize;
                    self.select_favorite_row();
                }
                None => self.enter_disks_section(),
            }
            return;
        }

        let mut index = self.fav_cursor as i32 - 1;
        while index >= 0 {
            if self.favorites.entries[index as usize].path().is_some() {
                self.fav_cursor = index as usize;
                self.select_favorite_row();
                return;
            }
            index -= 1;
        }
    }

    /// Arranging: every row is a stop, including separators and blanks.
    fn move_favorite_edit(&mut self, delta: i32) {
        let len = self.favorites.len() as i32;
        let next = self.fav_cursor as i32 + delta;
        if next < 0 {
            return;
        }
        if next >= len {
            // The free space under the list: a blank row is opened there, up to
            // a small cap, so a row can be dragged down and left.
            let blanks_below = self
                .favorites
                .entries
                .iter()
                .rev()
                .take_while(|entry| entry.is_blank())
                .count();
            if blanks_below >= MAX_TRAILING_BLANKS {
                self.enter_disks_section();
                return;
            }
            self.favorites.push_blank();
            self.fav_cursor = self.favorites.len() - 1;
            self.status.clear();
            return;
        }
        self.fav_cursor = next as usize;
        self.select_favorite_row();
    }

    /// Step into the mounted volumes, dropping the padding that was only there
    /// for arranging.
    fn enter_disks_section(&mut self) {
        if self.disks.is_empty() {
            return;
        }
        self.favorites.trim_trailing_blanks();
        self.fav_cursor = self.fav_cursor.min(self.favorites.len().saturating_sub(1));
        self.disks_focus = true;
        self.disk_cursor = 0;
        self.reveal_disk();
    }

    /// `E`: switch between browsing and arranging the sidebar.
    fn toggle_fav_edit(&mut self) {
        self.fav_edit = !self.fav_edit;
        if self.fav_edit {
            self.status = "Arranging: ↑↓ every row · K/J move · S separator · Del remove · E done"
                .to_string();
        } else {
            self.status = "Browsing: ↑↓ bookmarks · E to arrange".to_string();
            // Leaving edit mode must leave the cursor on something Enter can
            // act on, so a separator or blank under it is pushed aside.
            self.snap_cursor_to_target();
        }
    }

    /// Move the cursor to the nearest bookmark, if it is not already on one.
    fn snap_cursor_to_target(&mut self) {
        if self.favorites.entries.is_empty() {
            self.fav_cursor = 0;
            return;
        }
        if self
            .favorites
            .entries
            .get(self.fav_cursor)
            .is_some_and(|entry| entry.path().is_some())
        {
            return;
        }
        // Prefer the row above, then below: that is where the user came from.
        let above = (0..self.fav_cursor)
            .rev()
            .find(|index| self.favorites.entries[*index].path().is_some());
        let below = ((self.fav_cursor + 1)..self.favorites.len())
            .find(|index| self.favorites.entries[*index].path().is_some());
        if let Some(index) = above.or(below) {
            self.fav_cursor = index;
            self.select_favorite_row();
        }
    }

    /// Show the highlighted disk in the active panel, keeping the sidebar focus.
    fn reveal_disk(&mut self) {
        let Some(disk) = self.disks.get(self.disk_cursor).cloned() else {
            return;
        };
        let panel = self.active;
        if self.panels[panel].cwd != disk.mount {
            self.start_load(panel, disk.mount, None, LoadKind::Navigate);
        }
    }

    /// Enter/`→` on a disk: hand the panel over to it.
    fn open_disk(&mut self) {
        let Some(disk) = self.disks.get(self.disk_cursor).cloned() else {
            return;
        };
        let panel = self.active;
        self.focus = Focus::Panels;
        let mount = disk.mount.clone();
        self.status = format!("{} ({})", disk.label, mount.display());
        self.start_load(panel, disk.mount, None, LoadKind::Navigate);
    }

    /// Reveal the cursor row in the active panel without leaving the sidebar.
    fn select_favorite_row(&mut self) {
        let Some(entry) = self.favorites.entries.get(self.fav_cursor).cloned() else {
            return;
        };
        let Some(path) = entry.path() else {
            return;
        };
        let path = path.to_path_buf();
        let panel = self.active;

        if path.is_dir() {
            // Load the folder so the panel shows its contents; the sidebar keeps
            // the focus, which is what makes arrow-browsing possible.
            self.favorites_preview = Some((panel, path));
            self.run_favorites_preview();
            return;
        }

        // A bookmarked file is *previewed* where it already is: only the panel
        // cursor moves, so the list stays put and the preview pane picks the
        // file up. Navigating to its folder would lose the user's place for a
        // single keystroke — that is what `Enter` is for.
        if self.panels[panel].cwd == path.parent().unwrap_or(Path::new("")) {
            if let Some(index) = self.panels[panel]
                .entries
                .iter()
                .position(|entry| entry.path == path)
            {
                self.panels[panel].cursor = index;
                self.panels[panel].ensure_visible();
                self.sync_preview_scroll();
            }
            return;
        }

        // The panel is somewhere else, so reveal the folder once and let the
        // load finish with the cursor on the file.
        self.favorite_file = Some(path.clone());
        if let Some(parent) = path.parent() {
            self.start_load(
                panel,
                parent.to_path_buf(),
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string()),
                LoadKind::Navigate,
            );
        }
    }

    /// Apply a queued reveal once the sidebar has settled on a row.
    fn run_favorites_preview(&mut self) {
        let Some((panel, target)) = self.favorites_preview.clone() else {
            return;
        };
        self.favorites_preview = None;

        // For a file the stored path is `<parent>/<name>`; navigate to the
        // parent and select the name.
        let (dir, select) = if target.is_dir() {
            (target.clone(), None)
        } else {
            (
                target.parent().map(Path::to_path_buf).unwrap_or_default(),
                target
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string()),
            )
        };

        if self.panels[panel].loading {
            return;
        }
        self.start_load(panel, dir, select, LoadKind::Navigate);
    }

    /// `PageUp`/`PageDown`: move the cursor row itself, not the view.
    fn shift_favorite(&mut self, delta: i32) {
        if !self.fav_edit {
            self.status = "Moving lives in arranging mode: press E".to_string();
            return;
        }
        // A row can be moved past the end of the list: that is what drags a
        // bookmark down into the free space and opens a gap between groups. The
        // blank row is appended first, then the entry is moved into it.
        if delta > 0 && self.fav_cursor + 1 >= self.favorites.len() {
            self.favorites.push_blank();
        }
        let Some(index) = self.favorites.move_row(self.fav_cursor, delta) else {
            self.status = "Already at the edge of the list".to_string();
            return;
        };
        self.fav_cursor = index;
        self.persist_favorites();
        let label = self.favorites.entries[index].label();
        self.status = if label.is_empty() {
            format!("Empty row: moved {}", if delta < 0 { "up" } else { "down" })
        } else {
            format!("{label}: moved {}", if delta < 0 { "up" } else { "down" })
        };
    }

    fn begin_separator(&mut self) {
        self.prompt = Some(Prompt::Separator {
            buffer: String::new(),
        });
        self.status = "Separator title · Enter to add after the cursor · Esc cancels".to_string();
    }

    /// Insert an empty row right after the cursor.
    ///
    /// This is the direct way to open a gap: dragging a bookmark down is the
    /// other one, but that requires knowing dragging exists. A blank is a row
    /// like any other, so it can then be moved or deleted with the same keys.
    fn insert_blank_after_cursor(&mut self) {
        let at = (self.fav_cursor + 1).min(self.favorites.len());
        self.favorites
            .entries
            .insert(at, crate::favorites::Entry::Blank);
        self.fav_cursor = at;
        self.persist_favorites();
        self.status = "Empty row added · K/J moves it, Del removes it".to_string();
    }

    fn do_add_separator(&mut self, title: &str) {
        if title.trim().is_empty() {
            self.status = "Separator title is empty".to_string();
            return;
        }
        self.favorites
            .add_separator(title.to_string(), Some(self.fav_cursor));
        self.fav_cursor = (self.fav_cursor + 1).min(self.favorites.len().saturating_sub(1));
        self.persist_favorites();
        self.status = format!("Separator added: {title}");
    }

    /// Re-read the mount table (disks appear and disappear as they are plugged).
    pub fn refresh_disks(&mut self) {
        self.disks = crate::disks::list();
        self.disk_cursor = self.disk_cursor.min(self.disks.len().saturating_sub(1));
    }

    fn persist_favorites(&mut self) {
        if let Err(error) = self.favorites.save() {
            self.status = format!("Could not save favorites: {error}");
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
                    if let Some(Prompt::Delete { sources, permanent }) = self.prompt.take() {
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
                        let kind = if permanent {
                            JobKind::Shred
                        } else {
                            JobKind::Delete
                        };
                        self.job = Some(transfer::spawn(kind, sources, None));
                        self.status = if permanent {
                            format!("Deleting {count} item(s) permanently...")
                        } else {
                            format!("Moving {count} item(s) to the trash...")
                        };
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
        | Some(Prompt::Separator { buffer })
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
                    // Terminals disagree about a shifted letter: most send
                    // `Char('P')`, the kitty protocol (Ghostty, kitty) sends
                    // `Char('p')` plus SHIFT. The second form used to type a
                    // lowercase letter, so a separator could never start with a
                    // capital.
                    let c = if key.modifiers.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase()
                    {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    };
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
            Some(Prompt::Separator { buffer }) => self.do_add_separator(&buffer),
            Some(Prompt::Rename { buffer, target }) => self.do_rename(&buffer, &target),
            Some(Prompt::BulkRename { buffer, sources }) => self.do_bulk_rename(&buffer, &sources),
            _ => {}
        }
    }

    /// `→`/`l`: enter a folder, but never launch a file in an external app.
    fn enter_folder_only(&mut self) {
        if self.active().mode == PanelMode::Preview {
            self.status = "Preview is read-only: Esc returns to the list".to_string();
            return;
        }
        match self.active().current_entry() {
            Some(entry) if entry.is_dir => self.open_current_entry(),
            Some(entry) => {
                self.status = format!("{} is a file — press Enter to open it", entry.name);
            }
            None => {}
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

    /// Delete: move the selection to the system trash.
    ///
    /// A permanent erase needs `Shift+Del` (or the "Delete permanently" menu
    /// entry) and says so in the prompt. The trash is what makes a wrong key
    /// press recoverable, which a single-key delete in a file manager is not.
    fn begin_delete(&mut self) {
        self.begin_delete_kind(false);
    }

    fn begin_delete_kind(&mut self, permanent: bool) {
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
        self.prompt = Some(Prompt::Delete { sources, permanent });
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
            .entries
            .iter()
            .position(|entry| entry.path() == Some(self.active().cwd.as_path()))
        {
            self.fav_cursor = index;
        }

        // With no bookmarks the sidebar still has a Disks section, and that is
        // what the cursor has to land on: otherwise `j` past an empty list does
        // nothing and the volumes look dead.
        if self.favorites.entries.is_empty() && !self.disks.is_empty() {
            self.disks_focus = true;
            self.disk_cursor = self.disk_cursor.min(self.disks.len().saturating_sub(1));
            self.reveal_disk();
        } else {
            self.fav_cursor = self.fav_cursor.min(self.favorites.len().saturating_sub(1));
        }
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
        // Opening the sidebar re-reads the mount table, so a freshly plugged
        // volume appears without a restart.
        self.refresh_disks();
        if self.show_favorites && self.focus == Focus::Favorites {
            self.focus = Focus::Panels;
            // Leaving the sidebar hands the keys back to the panel, so the
            // Disks section must stop claiming the cursor: otherwise the next
            // visit starts on a volume instead of on the bookmarks.
            self.disks_focus = false;
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

    /// Bookmark what the panel is showing: the folder at the top, or the file
    /// under the cursor when there is one.
    ///
    /// Files are bookmarked as files, not as their parent folder — that is what
    /// makes a shortcut to one document useful.
    fn add_favorite(&mut self) {
        if self.blocked_by_preview() || self.blocked_by_loading() {
            return;
        }
        let cwd = self.active().cwd.clone();
        let cursor_entry = self.active().current_entry().cloned();

        // A selected file is the more specific target; a folder in the listing
        // is bookmarked too, but `..` never is.
        let target = match &cursor_entry {
            Some(entry) if !entry.is_parent => entry.path.clone(),
            _ => cwd.clone(),
        };

        if !self.favorites.add(target.clone()) {
            self.status = "Already in favorites".to_string();
            return;
        }

        self.fav_cursor = self.favorites.len().saturating_sub(1);
        self.persist_favorites();
        self.status = format!("Added to favorites: {}", target.display());
    }

    fn open_favorite(&mut self) {
        let Some(entry) = self.favorites.entries.get(self.fav_cursor).cloned() else {
            return;
        };
        let Some(path) = entry.path() else {
            // A separator and a blank are not targets. Say so instead of doing
            // nothing, because the cursor can rest on them now.
            self.status = if entry.is_blank() {
                "An empty row: move a bookmark here with Shift+J, or Del to drop it".to_string()
            } else {
                "A separator: move bookmarks around it, or Del to remove it".to_string()
            };
            return;
        };
        let path = path.to_path_buf();
        let index = self.active;
        self.focus = Focus::Panels;

        if path.is_dir() {
            self.start_load(index, path, None, LoadKind::Navigate);
            return;
        }

        // A file bookmark opens its folder with the cursor on the file.
        let parent = path.parent().map(Path::to_path_buf);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        if let Some(parent) = parent {
            self.start_load(index, parent, name, LoadKind::Navigate);
        }
    }

    fn remove_favorite(&mut self) {
        let Some(removed) = self.favorites.remove(self.fav_cursor) else {
            return;
        };
        self.fav_cursor = self.fav_cursor.min(self.favorites.len().saturating_sub(1));
        self.persist_favorites();
        self.status = format!("Removed from favorites: {}", removed.label());
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
        app.settings = crate::settings::Settings::defaults();
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
    fn section_sign_focuses_the_sidebar_when_bound() {
        // `§` is not privileged: it works because anyone can assign it in
        // `Ctrl+E`, which is the whole point of ADR-063.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings
            .assign(Action::FocusFavorites, 1, KeySpec::plain_char('§'))
            .unwrap();
        press(&mut app, KeyCode::Char('§'), KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::Favorites);
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
        let mut settings = crate::settings::Settings::defaults();

        settings
            .assign(
                Action::Delete,
                0,
                KeySpec::new(KeyCode::Char('d'), KeyModifiers::ALT),
            )
            .unwrap();
        assert_eq!(settings.keys_label(Action::Delete), "alt+d · delete");

        // Shift is preserved for keys where it is meaningful.
        settings
            .assign(
                Action::Copy,
                0,
                KeySpec::new(KeyCode::Right, KeyModifiers::SHIFT),
            )
            .unwrap();
        assert_eq!(settings.keys_label(Action::Copy), "shift+right · c");

        // Command is stored as super and matches both Super and Meta events.
        settings
            .assign(
                Action::Mkdir,
                0,
                KeySpec::new(KeyCode::Char('n'), KeyModifiers::SUPER),
            )
            .unwrap();
        assert_eq!(settings.keys_label(Action::Mkdir), "super+n · n");
        assert!(KeySpec::new(KeyCode::Char('n'), KeyModifiers::SUPER)
            .matches(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::META)));
    }

    #[test]
    fn settings_menu_lists_the_new_actions() {
        use crate::settings::Action as A;
        for action in [
            A::ToggleSidebar,
            A::OpenHere,
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
    fn every_key_is_rebindable_in_the_settings_menu() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings_menu = true;

        // ADR-063: the settings key itself is editable now…
        assert!(app
            .settings
            .assign(Action::Settings, 0, KeySpec::f(2))
            .is_ok());
        // …and so is anything else, including `Esc` and `Ctrl+E`.
        assert!(app
            .settings
            .assign(Action::Copy, 0, KeySpec::ctrl('e'))
            .is_ok());
        assert!(app
            .settings
            .assign(Action::Copy, 1, KeySpec::code(KeyCode::Esc))
            .is_ok());
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
        // Enter edits slot 1; slot 2 keeps its default.
        assert_eq!(
            app.settings.stored_slots(crate::settings::Action::Copy),
            [
                Some(crate::settings::KeySpec::ctrl('y')),
                Some(crate::settings::KeySpec::plain_char('c'))
            ]
        );
    }

    #[test]
    fn the_settings_menu_edits_the_second_slot_too() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings_menu = true;
        app.settings_cursor = crate::settings::Action::menu_actions()
            .iter()
            .position(|action| *action == crate::settings::Action::Move)
            .unwrap();

        // → moves the cursor to slot 2, Enter assigns it.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(app.settings_slot, 1);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);

        assert_eq!(
            app.settings.stored_slots(crate::settings::Action::Move),
            [
                Some(crate::settings::KeySpec::f(6)),
                Some(crate::settings::KeySpec::ctrl('y'))
            ]
        );
    }

    #[test]
    fn a_slot_can_be_cleared_from_the_settings_menu() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings_menu = true;
        app.settings_cursor = crate::settings::Action::menu_actions()
            .iter()
            .position(|action| *action == crate::settings::Action::Delete)
            .unwrap();

        press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(
            app.settings.stored_slots(crate::settings::Action::Delete)[0],
            None
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
        app.settings
            .assign(
                crate::settings::Action::Delete,
                0,
                crate::settings::KeySpec::f(9),
            )
            .unwrap();
        app.panels[0].select_name("a.txt");

        // F9 is a normal, rebindable key now: it arms the delete prompt.
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

        // `и` is the Russian key that sits where `b` is: Ctrl+и bookmarks the
        // entry under the cursor. The cursor starts on the first row, which is
        // `beta` here.
        app.panels[0].select_name("beta");
        press(&mut app, KeyCode::Char('и'), KeyModifiers::CONTROL);
        // Compare canonical paths: macOS reports /var and /private/var for the
        // same temporary directory.
        let expected = fs::canonicalize(tmp.path().join("beta")).unwrap();
        assert!(
            app.favorites
                .entries
                .iter()
                .filter_map(crate::favorites::Entry::path)
                .any(|path| fs::canonicalize(path).ok() == Some(expected.clone())),
            "Ctrl+и did not add the directory: {:?}",
            app.favorites.entries
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
        app.favorites.entries.clear();
        app.favorites
            .entries
            .push(crate::favorites::Entry::Path(bookmarked.clone()));
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
        // The helper is not waited for (see the clipboard module), so the test
        // waits for the text instead of assuming it is already on disk.
        assert_eq!(
            crate::testutil::read_when_written(&out, &app.panels[0].cwd.display().to_string()),
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
        // `Ctrl+Shift+C` is the copy path/name/selection chord; under the kitty
        // protocol an uppercase `C` with Ctrl is exactly that.
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
        //
        // Acceleration only runs when the terminal reports key releases; that
        // is what makes it safe to move the cursor on our own.
        app.releases_seen = true;
        app.held_arrow = Some((KeyCode::Down, Instant::now() - Duration::from_millis(1000)));
        app.last_arrow_activity = Some(Instant::now());
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

        app.releases_seen = true;
        app.held_arrow = Some((KeyCode::Down, Instant::now() - Duration::from_millis(1000)));
        app.last_arrow_activity = Some(Instant::now());
        assert!(app.tick_key_repeat());

        app.on_key_release(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.held_arrow.is_none());
        assert!(
            !app.tick_key_repeat(),
            "the repeat must stop once the key is released"
        );
    }

    #[test]
    fn a_terminal_without_key_releases_never_moves_the_cursor_itself() {
        // Terminal.app sends no key-release events at all. If the app still ran
        // its own repeat there, nothing could ever stop it: the cursor walked
        // on until another key was pressed, wrapping onto `..` and making the
        // next file operation report "Nothing selected".
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        fs::create_dir_all(&left).unwrap();
        for index in 0..20 {
            fs::write(left.join(format!("f{index:02}.txt")), "x").unwrap();
        }

        let mut app = app_for(&left, &left);
        assert!(!app.releases_seen);
        app.panels[0].cursor = 1;

        // A press, then no release ever arrives.
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        app.held_arrow = Some((KeyCode::Char('j'), Instant::now() - Duration::from_secs(5)));
        app.last_arrow_activity = Some(Instant::now());

        for _ in 0..50 {
            assert!(
                !app.tick_key_repeat(),
                "the app must not move the cursor without key releases"
            );
        }
        assert_eq!(app.panels[0].cursor, 2, "the cursor moved on its own");
    }

    #[test]
    fn a_lost_key_release_stops_the_repeat_after_a_silence() {
        // A release can be lost even on a terminal that reports them. A key
        // that is still down keeps sending events, so silence means it is up.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        app.releases_seen = true;
        app.held_arrow = Some((KeyCode::Down, Instant::now() - Duration::from_millis(1000)));
        app.last_arrow_activity =
            Some(Instant::now() - LOST_KEY_RELEASE - Duration::from_millis(50));

        assert!(!app.tick_key_repeat(), "the watchdog did not fire");
        assert!(app.held_arrow.is_none(), "the held arrow was not cleared");
    }

    #[test]
    fn seconds_of_holding_grow_the_repeat_step() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app_for(tmp.path(), tmp.path());

        assert_eq!(app.repeat_step_for(Duration::from_millis(300)), 1);
        assert_eq!(app.repeat_step_for(Duration::from_millis(1000)), 2);
        assert_eq!(app.repeat_step_for(Duration::from_millis(2000)), 4);
    }

    #[test]
    fn space_on_a_folder_starts_measuring_its_size() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let folder = left.join("big");
        fs::create_dir_all(folder.join("nested")).unwrap();
        fs::write(folder.join("data.bin"), vec![b'x'; 4096]).unwrap();
        fs::write(folder.join("nested/more.bin"), vec![b'x'; 4096]).unwrap();

        let mut app = app_for(&left, &left);
        app.panels[0].select_name("big");
        assert!(app.folder_size(&folder).is_none());

        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);

        // Space still selects, and the measurement is queued.
        assert!(app.panels[0].selected.contains(&folder));
        assert!(
            !app.folder_size_pending.is_empty() || app.folder_size(&folder).is_some(),
            "no measurement was started"
        );

        // Drive the event loop until the worker answers.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && app.folder_size(&folder).is_none() {
            app.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.folder_size(&folder), Some(8192));
    }

    #[test]
    fn space_on_a_file_measures_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        fs::create_dir_all(&left).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &left);
        app.panels[0].select_name("a.txt");
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);

        assert!(app.panels[0].selected.contains(&left.join("a.txt")));
        assert!(app.folder_size_pending.is_empty());
    }

    #[test]
    fn favourites_keys_are_not_hijacked_by_file_actions() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let bookmarked = tmp.path().join("bookmarked");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&bookmarked).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &left);
        app.favorites.entries.clear();
        app.favorites
            .entries
            .push(crate::favorites::Entry::Path(bookmarked.clone()));
        app.fav_cursor = 0;
        app.fav_edit = true; // deletion lives in arranging mode (ADR-068)
        app.focus = Focus::Favorites;
        app.panels[0].select_name("a.txt");

        // Delete/Del must remove the bookmark, never the file under the panel
        // cursor. This is the accident that deleted a real folder.
        press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert!(
            app.prompt.is_none(),
            "a delete prompt is not a bookmark action"
        );
        assert_eq!(app.favorites.len(), 0, "bookmark was not removed");
        assert!(
            left.join("a.txt").exists(),
            "the panel file must be untouched"
        );
    }

    #[test]
    fn enter_in_favourites_opens_the_bookmark_not_the_panel_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let bookmarked = tmp.path().join("bookmarked");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&bookmarked).unwrap();

        let mut app = app_for(&left, &left);
        app.favorites.entries.clear();
        app.favorites
            .entries
            .push(crate::favorites::Entry::Path(bookmarked.clone()));
        app.fav_cursor = 0;
        app.focus = Focus::Favorites;

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert_eq!(app.panels[0].cwd, bookmarked);
        assert_eq!(app.focus, Focus::Panels);
    }

    #[test]
    fn arrows_move_inside_favourites() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.favorites.entries.clear();
        for name in ["one", "two", "three"] {
            let path = tmp.path().join(name);
            fs::create_dir_all(&path).unwrap();
            app.favorites
                .entries
                .push(crate::favorites::Entry::Path(path));
        }
        app.fav_cursor = 0;
        app.focus = Focus::Favorites;

        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 1);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 2);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 1);
    }

    #[test]
    fn a_shift_arrow_binding_never_fires_on_a_plain_arrow() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app_for(tmp.path(), tmp.path());
        // Shift+Right belongs to "open folder there" by default.
        let plain = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(
            app.settings.action_for(plain),
            None,
            "a plain Right must not resolve to the Shift+Right action"
        );
        assert_eq!(
            app.settings
                .action_for(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
            Some(Action::OpenInOther)
        );
    }

    // ---- favorites: separators, files, reordering, browsing ----

    fn favourites_app(tmp: &Path) -> App {
        let mut app = app_for(tmp, tmp);
        app.favorites.entries.clear();
        app.focus = Focus::Favorites;
        app
    }

    #[test]
    fn arrows_browse_and_reveal_the_folder_without_leaving_the_sidebar() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("inner.txt"), "x").unwrap();

        let mut app = favourites_app(tmp.path());
        for path in [&first, &second] {
            app.favorites
                .entries
                .push(crate::favorites::Entry::Path(path.clone()));
        }
        app.fav_cursor = 0;

        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert_eq!(
            app.focus,
            Focus::Favorites,
            "browsing must stay in the list"
        );
        assert_eq!(app.fav_cursor, 1);
        assert_eq!(
            app.panels[0].cwd, second,
            "the panel must follow the cursor"
        );
        assert!(
            app.panels[0]
                .entries
                .iter()
                .any(|entry| entry.name == "inner.txt"),
            "the revealed folder must be loaded"
        );
    }

    #[test]
    fn right_arrow_opens_the_bookmark_just_like_enter() {
        let tmp = tempfile::tempdir().unwrap();
        let bookmarked = tmp.path().join("bookmarked");
        fs::create_dir_all(&bookmarked).unwrap();

        let mut app = favourites_app(tmp.path());
        app.favorites
            .entries
            .push(crate::favorites::Entry::Path(bookmarked.clone()));
        app.fav_cursor = 0;

        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert_eq!(app.panels[0].cwd, bookmarked);
        assert_eq!(app.focus, Focus::Panels, "opening leaves the sidebar");
    }

    #[test]
    fn a_bookmarked_file_reveals_itself_and_opens_on_enter() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("docs");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("note.md");
        fs::write(&file, "hello").unwrap();

        let mut app = favourites_app(tmp.path());
        app.favorites
            .entries
            .push(crate::favorites::Entry::Path(file.clone()));
        app.fav_cursor = 0;
        app.active = 0;

        // Browsing to a file shows its folder with the cursor on it. The cursor
        // is already on the only bookmark, so no Down is sent: a step past the
        // last bookmark now lands in the Disks section on purpose.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert_eq!(app.panels[0].cwd, folder);
        assert_eq!(
            app.panels[0]
                .current_entry()
                .map(|entry| entry.name.as_str()),
            Some("note.md")
        );
    }

    #[test]
    fn ctrl_b_bookmarks_the_cursor_file_not_only_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("docs");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("note.md");
        fs::write(&file, "hello").unwrap();

        let mut app = app_for(&folder, &folder);
        app.favorites.entries.clear();
        app.panels[0].select_name("note.md");
        press(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);

        assert_eq!(app.favorites.len(), 1);
        // Bookmarks are canonicalized; a macOS temp dir is reached through the
        // `/var` symlink, so compare the real path.
        assert_eq!(
            app.favorites.path_at(0),
            Some(file.canonicalize().unwrap().as_path())
        );
    }

    #[test]
    fn page_keys_reorder_the_sidebar_and_survive_a_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        for name in ["a", "b", "c"] {
            let path = tmp.path().join(name);
            fs::create_dir_all(&path).unwrap();
            app.favorites
                .entries
                .push(crate::favorites::Entry::Path(path));
        }
        app.favorites.set_file(tmp.path().join("favorites.toml"));
        app.fav_cursor = 1;
        app.fav_edit = true;

        press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 0);
        assert_eq!(app.favorites.entries[0].label(), "b");

        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 2);
        assert_eq!(app.favorites.entries[2].label(), "b");

        // The order is what gets saved.
        app.persist_favorites();
        let reloaded = crate::favorites::Favorites::load_from_dir(tmp.path());
        assert_eq!(reloaded.entries, app.favorites.entries);
    }

    #[test]
    fn s_adds_a_named_separator_after_the_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        for name in ["work", "music"] {
            let path = tmp.path().join(name);
            fs::create_dir_all(&path).unwrap();
            app.favorites
                .entries
                .push(crate::favorites::Entry::Path(path));
        }
        app.favorites.set_file(tmp.path().join("favorites.toml"));
        app.fav_cursor = 0;
        app.fav_edit = true; // separators are added while arranging

        press(&mut app, KeyCode::Char('S'), KeyModifiers::NONE);
        assert!(matches!(app.prompt, Some(Prompt::Separator { .. })));
        for ch in "Personal".chars() {
            press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            app.favorites.entries[1],
            crate::favorites::Entry::Separator("Personal".to_string())
        );
        let reloaded = crate::favorites::Favorites::load_from_dir(tmp.path());
        assert_eq!(reloaded.entries, app.favorites.entries);
    }

    #[test]
    fn browsing_skips_a_separator_and_arranging_stops_on_it() {
        // The cursor used to jump onto every caption, which is what made it feel
        // like it was "skipping around". Browsing now visits bookmarks only;
        // arranging (`E`) stops on every row so it can be moved (ADR-068).
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        for entry in [
            crate::favorites::Entry::Path(tmp.path().to_path_buf()),
            crate::favorites::Entry::Separator("Group".to_string()),
            crate::favorites::Entry::Path(tmp.path().to_path_buf()),
        ] {
            app.favorites.entries.push(entry);
        }
        app.fav_cursor = 0;

        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 2, "browsing must skip the separator");

        press(&mut app, KeyCode::Char('E'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 1, "arranging stops on the separator");

        // `Enter` on it explains instead of silently doing nothing.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.status.contains("separator"), "status: {}", app.status);
    }

    #[test]
    fn a_separator_can_be_moved_with_the_reorder_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        for entry in [
            crate::favorites::Entry::Path(tmp.path().join("a")),
            crate::favorites::Entry::Separator("Group".to_string()),
            crate::favorites::Entry::Path(tmp.path().join("b")),
        ] {
            app.favorites.entries.push(entry);
        }
        app.fav_cursor = 1;
        app.fav_edit = true;

        // `FavoriteDown` is `PgDn`/`J`: the separator swaps with the row below.
        press(&mut app, KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_eq!(
            app.favorites.entries[2],
            crate::favorites::Entry::Separator("Group".to_string())
        );
        assert_eq!(app.fav_cursor, 2, "the cursor follows the moved row");
    }

    #[test]
    fn moving_a_row_past_the_end_opens_a_gap() {
        // The free space below the last row is what lets groups be pulled apart
        // without inventing a separator title.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        let path = tmp.path().join("only");
        fs::create_dir_all(&path).unwrap();
        app.favorites
            .entries
            .push(crate::favorites::Entry::Path(path));
        app.favorites.set_file(tmp.path().join("favorites.toml"));
        app.fav_cursor = 0;
        app.fav_edit = true;

        press(&mut app, KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_eq!(app.favorites.len(), 2, "no gap was opened");
        assert!(app.favorites.entries[0].is_blank());
        assert_eq!(app.fav_cursor, 1, "the row did not move down");

        // The gap survives a reload.
        app.persist_favorites();
        let reloaded = crate::favorites::Favorites::load_from_dir(tmp.path());
        assert_eq!(reloaded.entries, app.favorites.entries);
    }

    #[test]
    fn the_context_menu_offers_favorites_for_entries_too() {
        let tmp = tempfile::tempdir().unwrap();
        // A real entry is needed: an empty folder only has `..`.
        fs::create_dir(tmp.path().join("somedir")).unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.show_favorites = false;
        app.settings = crate::settings::Settings::defaults();
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        // Row 1, not row 0: row 0 is the synthetic `..`, which has no actions.
        let body = app.panels[0].body_area;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            body.x + 2,
            body.y + 1,
        ));

        let menu = app.context_menu.as_ref().expect("no menu");
        assert_eq!(
            menu.index,
            Some(1),
            "the click must land on a real entry, not on .."
        );
        assert!(menu.items.contains(&MenuItem::AddFavorite));
        // Folder and "open in the other panel" read differently now.
        assert!(menu.items.contains(&MenuItem::OpenFolderInOther));
        let labels: Vec<&str> = menu.items.iter().map(|item| item.label()).collect();
        let unique: std::collections::BTreeSet<&&str> = labels.iter().collect();
        assert_eq!(
            labels.len(),
            unique.len(),
            "duplicate menu labels: {labels:?}"
        );
    }

    #[test]
    fn the_separator_key_does_nothing_in_a_panel() {
        // `S` is `Add separator`, a sidebar action. In a panel it is inert, which
        // is what the settings menu implies; the sidebar footer is the place
        // that mentions it.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        press(&mut app, KeyCode::Char('S'), KeyModifiers::NONE);
        assert!(app.prompt.is_none(), "S opened a prompt in a panel");
    }

    // ---- regression: focus keys, disks, sidebar targets ----

    /// Two fake volumes for the sidebar tests.
    ///
    /// The mounts are created as real directories: opening one runs a real
    /// `list_dir`, and a missing path would fail the load and leave the panel
    /// on its previous folder.
    fn fake_disks(tmp: &Path) -> Vec<crate::disks::Disk> {
        let one = tmp.join("volume-one");
        let two = tmp.join("volume-two");
        fs::create_dir_all(&two).unwrap();
        fs::create_dir_all(&one).unwrap();
        vec![
            crate::disks::Disk {
                mount: one,
                label: "One".to_string(),
                kind: "apfs".to_string(),
                free: Some(64 * 1024 * 1024 * 1024),
                total: Some(228 * 1024 * 1024 * 1024),
            },
            crate::disks::Disk {
                mount: two,
                label: "Two".to_string(),
                kind: "apfs".to_string(),
                free: Some(10 * 1024 * 1024 * 1024),
                total: Some(20 * 1024 * 1024 * 1024),
            },
        ]
    }

    #[test]
    fn ctrl_d_focuses_the_sidebar() {
        // `Ctrl+D` is printed in the README, `--help`, the in-app help and the
        // status hint, but the action only carried `Shift+Tab` and `§` — the key
        // the app advertises everywhere did nothing.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.show_favorites = false;

        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(app.show_favorites, "Ctrl+D did not open the sidebar");
        assert_eq!(app.focus, Focus::Favorites, "Ctrl+D did not focus it");

        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.focus, Focus::Panels, "Ctrl+D did not come back");
    }

    #[test]
    fn the_section_sign_focuses_the_sidebar_when_bound() {
        // `§` is an ordinary binding now (ADR-063): it is not in the default
        // slots, but assigning it works and nothing else claims it.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings
            .assign(Action::FocusFavorites, 1, KeySpec::plain_char('§'))
            .unwrap();
        press(&mut app, KeyCode::Char('§'), KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::Favorites);
    }

    #[test]
    fn a_separator_can_be_added_with_either_case_and_layout() {
        // The sidebar keys go through `keys::normalize`, so the physical key
        // matters, not the letter the layout produces. Lowercase `ы` is the
        // Russian layout's `S`; it used to do nothing at all.
        for (label, key) in [
            // Uppercase is the separator; lowercase `s` stays the sort menu.
            // A Cyrillic `Ы` is the physical `S` key on a Russian layout, and
            // `keys::normalize` maps it before dispatch.
            ("latin S", KeyCode::Char('S')),
            ("russian Ы", KeyCode::Char('Ы')),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let mut app = favourites_app(tmp.path());
            app.favorites.set_file(tmp.path().join("favorites.toml"));
            app.fav_cursor = 0;
            app.fav_edit = true;

            press(&mut app, key, KeyModifiers::NONE);
            assert!(
                matches!(app.prompt, Some(Prompt::Separator { .. })),
                "{label} did not start the separator prompt"
            );
        }
    }

    #[test]
    fn the_cursor_reaches_the_disks_past_the_free_space() {
        // Reaching the volumes is a deliberate step now: `j` below the last row
        // opens a gap (that is the arranging space), and only after the gap is
        // full does the next step enter the Disks section.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        for name in ["a", "b"] {
            let path = tmp.path().join(name);
            fs::create_dir_all(&path).unwrap();
            app.favorites
                .entries
                .push(crate::favorites::Entry::Path(path));
        }
        app.disks = fake_disks(tmp.path());
        app.fav_cursor = 1;
        // Reaching Disks is a deliberate step: browse mode goes there directly
        // from the last bookmark, arranging mode only after the gap is full.
        app.fav_edit = true;

        for _ in 0..=(MAX_TRAILING_BLANKS) {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        }
        assert!(
            app.disks_focus,
            "the cursor did not enter the Disks section"
        );
        assert_eq!(app.disk_cursor, 0);
        assert_eq!(
            app.favorites.entries.len(),
            2,
            "the trailing gap must be trimmed when the section is entered"
        );

        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.disk_cursor, 1, "the cursor did not move between disks");

        // Two steps back: one to the first volume, one out of the section.
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.disk_cursor, 0);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(
            !app.disks_focus,
            "the cursor did not leave the Disks section"
        );
        // It lands on the last real row, not on padding.
        assert_eq!(
            app.fav_cursor,
            app.favorites.len()
                - 1
                - app
                    .favorites
                    .entries
                    .iter()
                    .rev()
                    .take_while(|entry| entry.is_blank())
                    .count()
        );
    }

    #[test]
    fn enter_on_a_disk_opens_it_in_the_active_panel() {
        // `Enter` is `Action::OpenHere`, which called `open_favorite()`
        // directly and ignored the Disks section: the row was visibly
        // highlighted and Enter jumped to the last bookmark instead.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        app.disks = fake_disks(tmp.path());
        app.disks_focus = true;
        app.disk_cursor = 1;
        let expected = app.disks[1].mount.clone();

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        wait_for_loads(&mut app);

        assert_eq!(app.panels[0].cwd, expected);
        assert_eq!(app.focus, Focus::Panels, "the panel did not take the keys");
    }

    #[test]
    fn with_no_bookmarks_the_sidebar_starts_on_the_disks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.favorites.entries.clear();
        app.disks = fake_disks(tmp.path());
        app.show_favorites = false;

        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.focus, Focus::Favorites);
        assert!(app.disks_focus, "an empty sidebar left the cursor nowhere");
    }

    #[test]
    fn pgeup_pagedown_and_the_mac_letters_reorder_bookmarks() {
        // A MacBook has no reachable PageUp/PageDown, so each slot also carries
        // a plain letter. Both are rebindable like every other action.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        for name in ["a", "b", "c"] {
            let path = tmp.path().join(name);
            fs::create_dir_all(&path).unwrap();
            app.favorites
                .entries
                .push(crate::favorites::Entry::Path(path));
        }
        app.fav_cursor = 2;
        app.fav_edit = true;

        press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 1, "PageUp did not move the row");

        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 2, "PageDown did not move the row");

        // The Mac alternative, `K`/`J` (uppercase, opposite the browsing keys).
        press(&mut app, KeyCode::Char('K'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 1, "K did not move the row");

        press(&mut app, KeyCode::Char('J'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 2, "J did not move the row");
    }

    #[test]
    fn the_context_menu_reaches_settings_when_both_slots_are_cleared() {
        // ADR-063 removed every privileged key, including the settings key. The
        // safety net is the pointer: with both slots empty, the only way back
        // into `Ctrl+E` is the right-click menu, which must offer Settings.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());
        app.settings.clear_slot(Action::Settings, 0);
        app.settings.clear_slot(Action::Settings, 1);
        assert_eq!(
            app.settings
                .action_for(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            None,
            "the settings key must really be gone for this test to mean anything"
        );

        app.open_context_menu(0, None, 5, 5);
        let items = app
            .context_menu
            .as_ref()
            .expect("no context menu")
            .items
            .clone();
        assert!(
            items.contains(&MenuItem::Settings),
            "the pointer cannot reopen settings: {items:?}"
        );

        app.run_menu_item(MenuItem::Settings, 0, None);
        assert!(app.settings_menu, "the menu did not open");
    }

    // ---- arranging mode (ADR-068) ----

    /// Sidebar with a bookmark, a separator, a blank and a bookmark.
    fn arranging_app(tmp: &Path) -> App {
        let mut app = favourites_app(tmp);
        app.favorites.entries.clear();
        for entry in [
            crate::favorites::Entry::Path(tmp.join("a")),
            crate::favorites::Entry::Separator("Group".to_string()),
            crate::favorites::Entry::Blank,
            crate::favorites::Entry::Path(tmp.join("b")),
        ] {
            app.favorites.entries.push(entry);
        }
        app.favorites.set_file(tmp.join("favorites.toml"));
        app.fav_cursor = 0;
        app
    }

    #[test]
    fn browsing_skips_separators_and_blanks() {
        // The cursor jumping onto every caption was the complaint: in browse
        // mode it must visit bookmarks only, so Enter always has a target.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = arranging_app(tmp.path());

        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 3, "the cursor must land on the bookmark");

        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 0, "back to the first bookmark");
    }

    #[test]
    fn arranging_visits_every_row() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = arranging_app(tmp.path());

        press(&mut app, KeyCode::Char('E'), KeyModifiers::NONE);
        assert!(app.fav_edit, "E did not enter arranging mode");
        assert!(app.status.contains("Arranging"));

        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 1, "the separator is a row in edit mode");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 2, "the blank is a row in edit mode");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 3);

        // Leaving edit mode moves the cursor off a non-target.
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('E'), KeyModifiers::NONE);
        assert!(!app.fav_edit, "E did not leave arranging mode");
        assert!(
            app.favorites.entries[app.fav_cursor].path().is_some(),
            "the cursor stayed on a non-target: {:?}",
            app.favorites.entries[app.fav_cursor]
        );
    }

    #[test]
    fn moving_and_deleting_are_arranging_only() {
        // In browse mode `Del` on a bookmark would be a surprise in a list that
        // is meant for navigating, so both actions say where they live.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = arranging_app(tmp.path());
        let before = app.favorites.entries.clone();

        press(&mut app, KeyCode::Char('J'), KeyModifiers::SHIFT); // FavoriteDown
        assert_eq!(app.favorites.entries, before, "a row moved in browse mode");
        assert!(app.status.contains("arranging"), "status: {}", app.status);

        press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.favorites.entries, before, "a row was removed");
        assert!(app.status.contains("arranging"), "status: {}", app.status);

        // In arranging mode both work.
        press(&mut app, KeyCode::Char('E'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_ne!(
            app.favorites.entries, before,
            "a row did not move in arranging mode"
        );
        press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.favorites.len(), before.len() - 1);
    }

    #[test]
    fn the_sidebar_cursor_accelerates_like_a_panel() {
        // The sidebar never armed the held-arrow timer, so a held `j` only moved
        // at the terminal's own repeat rate and felt much slower than a panel.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        app.favorites.entries.clear();
        for index in 0..40 {
            let path = tmp.path().join(format!("d{index:02}"));
            fs::create_dir_all(&path).unwrap();
            app.favorites
                .entries
                .push(crate::favorites::Entry::Path(path));
        }
        app.releases_seen = true;
        app.fav_cursor = 0;

        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.fav_cursor, 1, "one press is one step");
        assert!(app.held_arrow.is_some(), "the arrow was not armed");

        // Pretend the key stays down.
        app.held_arrow = Some((
            KeyCode::Char('j'),
            Instant::now() - Duration::from_millis(500),
        ));
        app.last_arrow_activity = Some(Instant::now());
        let mut moves = 0;
        for _ in 0..30 {
            if app.tick_key_repeat() {
                moves += 1;
            }
            app.last_auto_repeat = app
                .last_auto_repeat
                .map(|last| last - Duration::from_millis(100));
        }
        assert!(
            moves > 5,
            "the sidebar cursor produced only {moves} repeats"
        );
        assert!(app.fav_cursor > 1, "the sidebar cursor did not advance");
    }

    #[test]
    fn the_sidebar_repeat_does_not_scroll_the_panels() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = favourites_app(tmp.path());
        app.releases_seen = true;
        let panel_cursor = app.panels[0].cursor;
        app.held_arrow = Some((KeyCode::Down, Instant::now() - Duration::from_millis(500)));
        app.last_arrow_activity = Some(Instant::now());

        app.tick_key_repeat();

        assert_eq!(
            app.panels[0].cursor, panel_cursor,
            "a held arrow in the sidebar moved the panel cursor"
        );
    }

    #[test]
    fn a_blank_row_can_be_added_directly() {
        // Opening a gap used to require dragging a row down and noticing that a
        // blank appeared. `Add empty row` makes it explicit.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = arranging_app(tmp.path());
        app.fav_edit = true;
        app.fav_cursor = 0;
        let before = app.favorites.len();

        press(&mut app, KeyCode::Char('B'), KeyModifiers::NONE);

        assert_eq!(app.favorites.len(), before + 1);
        assert!(
            app.favorites.entries[1].is_blank(),
            "the blank was not inserted after the cursor: {:?}",
            app.favorites.entries
        );
        assert_eq!(app.fav_cursor, 1, "the cursor did not follow the new row");
        assert!(app.status.contains("Empty row"), "status: {}", app.status);

        // And it is a normal row: movable and removable.
        press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.favorites.len(), before);
    }

    #[test]
    fn adding_a_blank_needs_arranging_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = arranging_app(tmp.path());
        let before = app.favorites.len();

        press(&mut app, KeyCode::Char('B'), KeyModifiers::NONE);
        assert_eq!(
            app.favorites.len(),
            before,
            "a blank was added while browsing"
        );
        assert!(app.status.contains("arranging"), "status: {}", app.status);
    }

    #[test]
    fn adding_a_separator_needs_arranging_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = arranging_app(tmp.path());

        press(&mut app, KeyCode::Char('S'), KeyModifiers::NONE);
        assert!(app.prompt.is_none(), "S opened a prompt while browsing");
        assert!(app.status.contains("arranging"), "status: {}", app.status);

        press(&mut app, KeyCode::Char('E'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('S'), KeyModifiers::NONE);
        assert!(matches!(app.prompt, Some(Prompt::Separator { .. })));
    }

    #[test]
    fn the_sidebar_keys_are_rebindable_actions() {
        // The chips in the sidebar footer were hard-coded strings, so they kept
        // showing `K`/`J` long after the keys had been rebound. Every sidebar
        // command is a real action now.
        use crate::settings::{Action, KeySpec};
        let tmp = tempfile::tempdir().unwrap();
        let mut app = arranging_app(tmp.path());
        app.settings
            .assign(Action::FavoriteDown, 1, KeySpec::plain_char('n'))
            .unwrap();
        app.settings
            .assign(Action::FavoriteBlank, 0, KeySpec::plain_char('z'))
            .unwrap();
        app.fav_edit = true;
        app.fav_cursor = 0;

        // The rebound key works…
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_ne!(
            app.favorites.entries[0].label(),
            "a",
            "n did not move the row"
        );
        // …and the old one no longer fires.
        let before = app.favorites.entries.clone();
        press(&mut app, KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_eq!(app.favorites.entries, before, "J still moved the row");

        // The new blank key works too.
        press(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(app.favorites.entries.iter().any(|entry| entry.is_blank()));
    }
}
