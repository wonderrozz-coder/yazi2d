use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::favorites::Favorites;
use crate::panel::{Panel, PanelMode};
use crate::preview::{ImageCache, PreviewCache};
use crate::transfer::{self, Job, JobKind, JobMsg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Panels,
    Favorites,
}

pub enum Prompt {
    Mkdir { buffer: String },
    Rename { buffer: String, target: PathBuf },
    Delete { sources: Vec<PathBuf> },
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
    pub preview_cache: PreviewCache,
    pub image_cache: ImageCache,
    pub status: String,
    pub should_quit: bool,
    /// Set when the screen must be repainted. Keeps idle CPU usage near zero.
    pub dirty: bool,
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
        }

        let status = if errors.is_empty() {
            "Ready. Tab switches panels, ? shows help.".to_string()
        } else {
            errors.join(" | ")
        };

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
            preview_cache: PreviewCache::new(),
            image_cache: ImageCache::new(),
            status,
            should_quit: false,
            dirty: true,
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

    pub fn other_mut(&mut self) -> &mut Panel {
        &mut self.panels[1 - self.active]
    }

    /// Drain worker-thread progress and finish completed jobs.
    pub fn tick(&mut self) {
        self.sync_preview_scroll();
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
            self.refresh_all();
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

    pub fn on_key(&mut self, key: KeyEvent) {
        self.dirty = true;

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

        if key.code == KeyCode::Char('q') && !key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        if matches!(key.code, KeyCode::Char('?') | KeyCode::F(1)) {
            self.help = true;
            return;
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
                    self.toggle_favorites_focus();
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
                KeyCode::Char('q') => {
                    self.toggle_preview();
                    return;
                }
                KeyCode::Char('t') => {
                    self.toggle_thumbnails();
                    return;
                }
                KeyCode::Char('r') => {
                    self.refresh_all();
                    self.sync_preview_scroll();
                    self.status = "Refreshed".to_string();
                    return;
                }
                _ => {}
            }
        }

        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Left => {
                    self.open_entry_in_panel(0);
                    return;
                }
                KeyCode::Right => {
                    self.open_entry_in_panel(1);
                    return;
                }
                _ => {}
            }
        }

        match self.focus {
            Focus::Panels => self.handle_panel_key(key),
            Focus::Favorites => self.handle_favorites_key(key),
        }
    }

    fn handle_panel_key(&mut self, key: KeyEvent) {
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
            KeyCode::Char('.') => self.toggle_hidden(),
            _ => return false,
        }
        true
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
        if matches!(self.prompt, Some(Prompt::Delete { .. })) {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(Prompt::Delete { sources }) = self.prompt.take() {
                        let count = sources.len();
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
        if let Some(Prompt::Mkdir { buffer }) | Some(Prompt::Rename { buffer, .. }) =
            self.prompt.as_mut()
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
            _ => {}
        }
    }

    fn enter_current(&mut self) {
        let Some(entry) = self.active().current_entry().cloned() else {
            return;
        };

        if !entry.is_dir {
            self.status = format!(
                "{} is a file (opening files is not part of the MVP)",
                entry.name
            );
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
        let hidden = self.hidden;
        let path = entry.path.clone();

        match self
            .active_mut()
            .change_dir(path, select.as_deref(), hidden)
        {
            Ok(()) => self.status = self.active().cwd.display().to_string(),
            Err(e) => self.status = format!("Cannot open {}: {}", entry.path.display(), e),
        }
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
        let hidden = self.hidden;

        match self
            .active_mut()
            .change_dir(parent, current_name.as_deref(), hidden)
        {
            Ok(()) => self.status = self.active().cwd.display().to_string(),
            Err(e) => self.status = format!("Cannot go up: {e}"),
        }
    }

    fn start_transfer(&mut self, kind: JobKind) {
        if self.blocked_by_preview() {
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

        let count = sources.len();
        self.job = Some(transfer::spawn(kind, sources, Some(dest.clone())));
        self.status = format!("{} {} item(s) to {}...", kind.verb(), count, dest.display());
    }

    fn begin_mkdir(&mut self) {
        if self.blocked_by_preview() {
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
        if self.blocked_by_preview() {
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
        if self.blocked_by_preview() {
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
                self.refresh_all();
                self.active_mut().select_name(name);
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
                self.refresh_all();
                self.active_mut().select_name(new_name);
                self.status = format!("Renamed to {}", new_name);
            }
            Err(e) => self.status = format!("Cannot rename: {e}"),
        }
    }

    /// Ctrl+D: reveal the sidebar (if needed) and move keyboard focus into it.
    fn focus_favorites(&mut self) {
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
        self.refresh_all();
        self.sync_preview_scroll();
        self.status = if self.hidden {
            "Hidden files: shown".to_string()
        } else {
            "Hidden files: hidden".to_string()
        };
    }

    /// Ctrl+Q: turn the active panel into a quick-view pane (TC style F3 preview).
    fn toggle_preview(&mut self) {
        if self.active().mode == PanelMode::Preview {
            self.active_mut().leave_preview();
            self.status = "Quick preview closed".to_string();
            return;
        }

        // Only one preview at a time: the other panel becomes its subject.
        if self.other().mode == PanelMode::Preview {
            self.other_mut().leave_preview();
        }

        self.active_mut().set_mode(PanelMode::Preview);
        self.focus = Focus::Panels;
        self.sync_preview_scroll();
        self.status =
            "Quick preview: ↑↓ scroll, ←→ pan, Ctrl+Q/Esc close, Ctrl+T thumbnails".to_string();
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

    /// Alt+←/→: open the folder under the active cursor in the neighbouring panel.
    fn open_entry_in_panel(&mut self, target: usize) {
        if target == self.active {
            self.status = "Alt+arrow points at the other panel".to_string();
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
        let hidden = self.hidden;

        match self.panels[target].change_dir(entry.path.clone(), select.as_deref(), hidden) {
            Ok(()) => {
                self.sync_preview_scroll();
                let label = if target == 0 { "Left" } else { "Right" };
                self.status = format!("{label}: {}", self.panels[target].cwd.display());
            }
            Err(e) => self.status = format!("Cannot open {}: {}", entry.path.display(), e),
        }
    }

    /// Keep the preview subject in sync with the opposite panel's cursor.
    pub fn sync_preview_scroll(&mut self) {
        for index in 0..self.panels.len() {
            if self.panels[index].mode != PanelMode::Preview {
                continue;
            }
            let target = self.panels[1 - index]
                .current_entry()
                .map(|entry| entry.path.clone());

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

    fn add_favorite(&mut self) {
        if self.blocked_by_preview() {
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
        if !path.is_dir() {
            self.status = format!("Favorite does not exist: {}", path.display());
            return;
        }
        let hidden = self.hidden;
        match self.active_mut().change_dir(path, None, hidden) {
            Ok(()) => {
                self.focus = Focus::Panels;
                self.status = self.active().cwd.display().to_string();
            }
            Err(e) => self.status = format!("Cannot open favorite: {e}"),
        }
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

    fn refresh_all(&mut self) {
        let hidden = self.hidden;
        let mut error = None;
        for panel in &mut self.panels {
            if let Err(e) = panel.reload(hidden) {
                error = Some(format!("{}: {}", panel.cwd.display(), e));
            }
        }
        if let Some(error) = error {
            self.status = error;
        }
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
        App::new(vec![left.to_path_buf(), right.to_path_buf()])
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.on_key(KeyEvent::new(code, modifiers));
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
    fn alt_arrow_opens_the_cursor_folder_in_the_neighbour() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let child = left.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&right).unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("child");
        press(&mut app, KeyCode::Right, KeyModifiers::ALT);

        assert_eq!(app.panels[1].cwd, child);
    }

    #[test]
    fn alt_arrow_on_a_file_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("a.txt"), "x").unwrap();

        let mut app = app_for(&left, &right);
        app.panels[0].select_name("a.txt");
        press(&mut app, KeyCode::Right, KeyModifiers::ALT);

        assert_eq!(app.panels[1].cwd, right);
        assert!(app.status.contains("not a folder"));
    }

    #[test]
    fn preview_and_thumbnails_toggle_from_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_for(tmp.path(), tmp.path());

        press(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].mode, PanelMode::Preview);
        press(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].mode, PanelMode::List);

        press(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].mode, PanelMode::Thumbnails);
        press(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(app.panels[0].mode, PanelMode::List);
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
