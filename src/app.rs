use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, ModifierKeyCode};

use crate::colors::Colors;
use crate::favorites::Favorites;
use crate::logging;
use crate::panel::{LoadKind, LoadResult, Panel, PanelMode, SortKey, StatsUpdate};
use crate::preview::{ImageCache, PreviewCache};
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
    pub hidden: bool,
    pub prompt: Option<Prompt>,
    pub help: bool,
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
            hidden,
            prompt: None,
            help: false,
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
        if let KeyCode::Modifier(modifier) = key.code {
            self.release_modifier(modifier);
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

    fn handle_panel_key(&mut self, key: KeyEvent) {
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

        if self.handle_common_action(key.code) {
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.active_mut().move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.active_mut().move_cursor(1),
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

        if self.handle_common_action(key.code) {
            return;
        }

        let cols = self.active().grid_cols.max(1) as i32;
        let page = self.active().grid_cols.max(1) as i32 * self.active().grid_rows.max(1) as i32;

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.active_mut().move_grid(-cols),
            KeyCode::Down | KeyCode::Char('j') => self.active_mut().move_grid(cols),
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

    /// Handle a pointer tap (keyboard-reporting terminals send taps as clicks).
    ///
    /// A double tap with two fingers is the gesture for "open the entry under
    /// the cursor": folders are entered, files are opened in the system app.
    /// Single taps do nothing, so a stray click never navigates.
    pub fn on_pointer_tap(&mut self) {
        self.dirty = true;
        if self.help || self.prompt.is_some() || self.sort_menu || self.theme_menu {
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
            Focus::Favorites => self.open_favorite(),
            Focus::Panels => self.open_current_entry(),
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

    fn handle_common_action(&mut self, code: KeyCode) -> bool {
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
        if self.active().mode == PanelMode::Thumbnails {
            self.active_mut().set_mode(PanelMode::List);
            self.status = "List view".to_string();
            return;
        }

        if self.active().mode == PanelMode::Preview {
            self.active_mut().leave_preview();
        }
        self.focus = Focus::Panels;
        self.active_mut().set_mode(PanelMode::Thumbnails);
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
}
