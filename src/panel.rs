use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Instant, SystemTime};

use crate::logging;

/// Directory loads slower than this are logged as `WARN`.
const SLOW_LOAD_MS: u128 = 250;

/// One visible row in a panel. The synthetic `..` entry is also an `Entry`.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_parent: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// False until a background worker has stat'ed this entry.
    pub stats_loaded: bool,
}

impl Entry {
    fn parent(path: PathBuf) -> Self {
        Self {
            name: "..".to_string(),
            path,
            is_dir: true,
            is_symlink: false,
            is_parent: true,
            size: 0,
            modified: None,
            stats_loaded: true,
        }
    }
}

/// Sort order for a panel listing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Extension,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "date",
            SortKey::Extension => "type",
        }
    }
}

/// One batch of stat results from the background enrichment worker.
pub struct StatsUpdate {
    pub generation: u64,
    pub stats: Vec<(usize, u64, Option<SystemTime>)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelMode {
    List,
    Thumbnails,
    Preview,
}

/// Whether applying a load should behave like navigation or like a refresh.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadKind {
    Navigate,
    Refresh,
}

/// Result of a background directory listing.
pub struct LoadResult {
    pub generation: u64,
    pub kind: LoadKind,
    pub path: PathBuf,
    pub select: Option<String>,
    pub entries: io::Result<Vec<Entry>>,
    pub elapsed: std::time::Duration,
}

pub struct Panel {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub offset: usize,
    pub selected: HashSet<PathBuf>,
    /// Visible list height, updated by the renderer and used for PgUp/PgDn.
    pub page_size: usize,
    pub error: Option<String>,
    pub mode: PanelMode,
    /// Mode to restore when leaving Preview.
    pub mode_before_preview: PanelMode,
    pub preview_path: Option<PathBuf>,
    pub preview_scroll: u16,
    pub preview_scroll_x: u16,
    pub preview_max_scroll: u16,
    pub preview_max_scroll_x: u16,
    /// Grid geometry, updated by the thumbnail renderer.
    pub grid_cols: usize,
    pub grid_rows: usize,
    /// True while a background directory load is in flight.
    pub loading: bool,
    /// Path of the in-flight load, shown in the sub-header.
    pub loading_path: Option<PathBuf>,
    /// Monotonic id of the newest load request; stale results are ignored.
    pub generation: u64,
    /// Current sort order; re-applied whenever a listing arrives.
    pub sort_key: SortKey,
    pub sort_reverse: bool,
    /// True while background stat workers are still filling in sizes/dates.
    pub enriching: bool,
}

/// Work queue and result channel handed to the stat worker.
pub type EnrichmentStart = (
    mpsc::Sender<StatsUpdate>,
    Receiver<StatsUpdate>,
    Vec<(usize, PathBuf)>,
    u64,
);

impl Panel {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            entries: Vec::new(),
            cursor: 0,
            offset: 0,
            selected: HashSet::new(),
            page_size: 10,
            error: None,
            mode: PanelMode::List,
            mode_before_preview: PanelMode::List,
            preview_path: None,
            preview_scroll: 0,
            preview_scroll_x: 0,
            preview_max_scroll: 0,
            preview_max_scroll_x: 0,
            grid_cols: 1,
            grid_rows: 1,
            loading: false,
            loading_path: None,
            generation: 0,
            sort_key: SortKey::Name,
            sort_reverse: false,
            enriching: false,
        }
    }

    pub fn set_mode(&mut self, mode: PanelMode) {
        if mode == PanelMode::Preview {
            if self.mode != PanelMode::Preview {
                self.mode_before_preview = self.mode;
            }
            self.preview_path = None;
            self.preview_scroll = 0;
            self.preview_scroll_x = 0;
            self.preview_max_scroll = 0;
            self.preview_max_scroll_x = 0;
        } else {
            self.offset = 0;
            self.mode = mode;
            if mode == PanelMode::List {
                self.ensure_visible();
            } else {
                self.ensure_grid_visible();
            }
        }
        self.mode = mode;
    }

    pub fn leave_preview(&mut self) {
        if self.mode != PanelMode::Preview {
            return;
        }
        let mode = match self.mode_before_preview {
            PanelMode::Preview => PanelMode::List,
            mode => mode,
        };
        self.set_mode(mode);
        self.preview_path = None;
        self.preview_scroll = 0;
        self.preview_scroll_x = 0;
        self.preview_max_scroll = 0;
        self.preview_max_scroll_x = 0;
    }

    pub fn scroll_preview(&mut self, delta_y: i32, delta_x: i32) {
        self.preview_scroll =
            (self.preview_scroll as i32 + delta_y).clamp(0, self.preview_max_scroll as i32) as u16;
        self.preview_scroll_x = (self.preview_scroll_x as i32 + delta_x)
            .clamp(0, self.preview_max_scroll_x as i32) as u16;
    }

    /// Re-read the current directory while keeping the cursor and selection when possible.
    pub fn reload(&mut self, show_hidden: bool) -> io::Result<()> {
        let keep = self.current_entry().map(|e| e.name.clone());
        let entries = list_dir(&self.cwd, show_hidden)?;

        let valid: HashSet<PathBuf> = entries
            .iter()
            .filter(|e| !e.is_parent)
            .map(|e| e.path.clone())
            .collect();
        self.selected.retain(|p| valid.contains(p));

        self.entries = entries;
        self.error = None;
        self.apply_sort();
        self.cursor = keep
            .and_then(|name| self.entries.iter().position(|e| e.name == name))
            .unwrap_or(if self.entries.len() > 1 { 1 } else { 0 })
            .min(self.entries.len().saturating_sub(1));
        self.ensure_mode_visible();
        Ok(())
    }

    /// Change directory synchronously (initial listing and tests).
    ///
    /// Navigation in the app goes through `spawn_load`/`apply_load` so the UI
    /// never blocks; this helper remains for callers that already have the
    /// listing result on hand.
    #[allow(dead_code)]
    pub fn change_dir(
        &mut self,
        path: PathBuf,
        select: Option<&str>,
        show_hidden: bool,
    ) -> io::Result<()> {
        let entries = list_dir(&path, show_hidden)?;

        self.cwd = path;
        self.entries = entries;
        self.selected.clear();
        self.offset = 0;
        self.apply_sort();
        self.cursor = select
            .and_then(|name| self.entries.iter().position(|e| e.name == name))
            .unwrap_or(if self.entries.len() > 1 { 1 } else { 0 })
            .min(self.entries.len().saturating_sub(1));
        self.error = None;
        self.ensure_mode_visible();
        Ok(())
    }

    pub fn current_entry(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    /// Start listing `path` on a worker thread.
    ///
    /// The panel keeps its previous list until `apply_load` is called with the
    /// matching generation, so the UI stays navigable and never freezes on a
    /// slow filesystem.
    pub fn spawn_load(
        &mut self,
        path: PathBuf,
        select: Option<String>,
        show_hidden: bool,
        kind: LoadKind,
    ) -> Receiver<LoadResult> {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.loading_path = Some(path.clone());
        self.error = None;

        let (sender, receiver) = mpsc::channel();
        let thread_path = path.clone();
        let thread_select = select.clone();
        thread::spawn(move || {
            logging::info(format!("load start: {}", thread_path.display()));
            let started = Instant::now();
            let entries = list_dir(&thread_path, show_hidden);
            let elapsed = started.elapsed();
            let _ = sender.send(LoadResult {
                generation,
                kind,
                path: thread_path,
                select: thread_select,
                entries,
                elapsed,
            });
        });

        receiver
    }

    /// Collect the stat work for the current entries and mark the panel as
    /// enriching. Returns the work queue and a channel for the batches.
    pub fn begin_enrichment(&mut self) -> EnrichmentStart {
        self.enriching = true;
        let (sender, receiver) = mpsc::channel();
        let work = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_parent && !entry.stats_loaded)
            .map(|(index, entry)| (index, entry.path.clone()))
            .collect();
        (sender, receiver, work, self.generation)
    }

    /// Fill in size/date synchronously. Used only for the initial startup
    /// listing so the first frame is complete; navigation uses `stat_entries`.
    pub fn enrich_blocking(&mut self) {
        let paths: Vec<(usize, PathBuf)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_parent && !entry.stats_loaded)
            .map(|(index, entry)| (index, entry.path.clone()))
            .collect();

        for (index, path) in paths {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if let Some(entry) = self.entries.get_mut(index) {
                entry.size = if meta.is_dir() { 0 } else { meta.len() };
                entry.modified = meta.modified().ok();
                entry.stats_loaded = true;
            }
        }
        if self.sort_key != SortKey::Name {
            self.apply_sort();
        }
    }

    /// Mark enrichment finished once the worker channel is closed.
    pub fn finish_enrichment(&mut self) {
        self.enriching = false;
    }

    /// Apply a background load. Returns `true` when the directory was replaced.
    ///
    /// Results from superseded or cancelled loads are ignored.
    pub fn apply_load(&mut self, load: LoadResult) -> bool {
        if load.generation != self.generation {
            logging::info(format!(
                "load ignored (stale generation {}): {}",
                load.generation,
                load.path.display()
            ));
            return false;
        }

        self.loading = false;
        self.loading_path = None;

        match load.entries {
            Ok(entries) => {
                let valid: HashSet<PathBuf> = entries
                    .iter()
                    .filter(|entry| !entry.is_parent)
                    .map(|entry| entry.path.clone())
                    .collect();

                self.cwd = load.path;
                self.entries = entries;

                match load.kind {
                    LoadKind::Navigate => {
                        self.selected.clear();
                        self.offset = 0;
                    }
                    LoadKind::Refresh => {
                        self.selected.retain(|path| valid.contains(path));
                    }
                }

                self.apply_sort();
                self.cursor = load
                    .select
                    .and_then(|name| self.entries.iter().position(|entry| entry.name == name))
                    .unwrap_or(if self.entries.len() > 1 { 1 } else { 0 })
                    .min(self.entries.len().saturating_sub(1));
                self.error = None;
                self.enriching = false;
                self.ensure_mode_visible();
                true
            }
            Err(error) => {
                logging::warn(format!(
                    "load failed in {} ms: {}: {error}",
                    load.elapsed.as_millis(),
                    load.path.display()
                ));
                self.error = Some(error.to_string());
                false
            }
        }
    }

    /// Drop the pending load without applying it.
    pub fn cancel_load(&mut self) {
        self.generation += 1;
        self.loading = false;
        self.loading_path = None;
    }

    /// Human-readable subject of the pending load.
    pub fn loading_label(&self) -> Option<String> {
        self.loading_path.as_ref().map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string())
        })
    }

    /// Sort the visible entries, keeping `..` first and the cursor on its entry.
    pub fn apply_sort(&mut self) {
        let selected_name = self.current_entry().map(|entry| entry.name.clone());
        sort_entries(&mut self.entries, self.sort_key, self.sort_reverse);

        if let Some(name) = selected_name {
            if let Some(index) = self.entries.iter().position(|entry| entry.name == name) {
                self.cursor = index;
            }
        }
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        self.ensure_mode_visible();
    }

    /// Cycle the sort key, or flip direction when the key does not change.
    pub fn set_sort(&mut self, key: SortKey, reverse: bool) {
        self.sort_key = key;
        self.sort_reverse = reverse;
        self.apply_sort();
    }

    /// Merge a batch of stat results. Returns true when anything changed.
    pub fn apply_stats(&mut self, update: StatsUpdate) -> bool {
        if update.generation != self.generation {
            return false;
        }
        let mut changed = false;
        for (index, size, modified) in update.stats {
            if let Some(entry) = self.entries.get_mut(index) {
                if entry.is_parent {
                    continue;
                }
                entry.size = size;
                entry.modified = modified;
                entry.stats_loaded = true;
                changed = true;
            }
        }
        if changed && self.sort_key != SortKey::Name {
            self.apply_sort();
        }
        changed
    }

    pub fn move_cursor(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as i32 - 1;
        self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
        self.ensure_visible();
    }

    /// Move in the thumbnail grid. `delta` is measured in tiles.
    pub fn move_grid(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as i32 - 1;
        self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
        self.ensure_grid_visible();
    }

    pub fn go_top(&mut self) {
        self.cursor = 0;
        self.offset = 0;
    }

    pub fn go_bottom(&mut self) {
        self.cursor = self.entries.len().saturating_sub(1);
        self.ensure_mode_visible();
    }

    /// Move the cursor to a visible entry by name (tests and sync callers).
    #[allow(dead_code)]
    pub fn select_name(&mut self, name: &str) {
        if let Some(index) = self.entries.iter().position(|e| e.name == name) {
            self.cursor = index;
            self.ensure_mode_visible();
        }
    }

    /// Ctrl+A: select every real entry in the panel.
    pub fn select_all(&mut self) -> usize {
        self.selected = self
            .entries
            .iter()
            .filter(|entry| !entry.is_parent)
            .map(|entry| entry.path.clone())
            .collect();
        self.selected.len()
    }

    /// Put the cursor on `name` if it still exists, otherwise on the nearest
    /// remaining row at or above `previous` (used after deleting entries).
    pub fn focus_after_removal(&mut self, names: &[String], previous: usize) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| names.contains(&entry.name))
        {
            self.cursor = index;
        } else {
            self.cursor = previous.min(self.entries.len().saturating_sub(1));
        }
        self.ensure_mode_visible();
    }

    /// Space/Insert: toggle the current entry and advance like Total Commander.
    pub fn toggle_selection(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor) {
            if entry.is_parent {
                return;
            }
            let path = entry.path.clone();
            if !self.selected.remove(&path) {
                self.selected.insert(path);
            }
        }
        self.move_grid_or_cursor(1);
    }

    /// Selected entries in display order, or the cursor entry when nothing is selected.
    pub fn selection_or_current(&self) -> Vec<PathBuf> {
        if !self.selected.is_empty() {
            self.entries
                .iter()
                .filter(|e| !e.is_parent && self.selected.contains(&e.path))
                .map(|e| e.path.clone())
                .collect()
        } else {
            self.current_entry()
                .filter(|e| !e.is_parent)
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        }
    }

    pub fn ensure_visible(&mut self) {
        let page = self.page_size.max(1);
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if self.cursor >= self.offset + page {
            self.offset = self.cursor + 1 - page;
        }
        let max_offset = self.entries.len().saturating_sub(page);
        self.offset = self.offset.min(max_offset);
    }

    pub fn ensure_grid_visible(&mut self) {
        let cols = self.grid_cols.max(1);
        let rows = self.grid_rows.max(1);
        let row = self.cursor / cols;

        if row < self.offset {
            self.offset = row;
        } else if row >= self.offset + rows {
            self.offset = row + 1 - rows;
        }

        let max_offset =
            (self.entries.len().saturating_sub(1) / cols).saturating_sub(rows.saturating_sub(1));
        self.offset = self.offset.min(max_offset);
    }

    fn ensure_mode_visible(&mut self) {
        match self.mode {
            PanelMode::Thumbnails => self.ensure_grid_visible(),
            _ => self.ensure_visible(),
        }
    }

    pub fn move_grid_or_cursor(&mut self, delta: i32) {
        match self.mode {
            PanelMode::Thumbnails => self.move_grid(delta),
            _ => self.move_cursor(delta),
        }
    }
}

fn extension_of(entry: &Entry) -> String {
    entry
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Sort in place: `..` first, then directories, then the chosen key.
fn sort_entries(entries: &mut [Entry], key: SortKey, reverse: bool) {
    entries.sort_by(|a, b| {
        let rank = |entry: &Entry| u8::from(!entry.is_parent);
        let rank_order = rank(a).cmp(&rank(b));
        if rank_order != std::cmp::Ordering::Equal {
            return rank_order;
        }

        // Directories stay grouped before files so navigation stays predictable.
        let dir_order = b.is_dir.cmp(&a.is_dir);
        if dir_order != std::cmp::Ordering::Equal {
            return dir_order;
        }

        let order = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Extension => extension_of(a)
                .cmp(&extension_of(b))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        };
        if reverse {
            order.reverse()
        } else {
            order
        }
    });
}

fn list_dir(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let started = Instant::now();
    let result = list_dir_inner(path, show_hidden);
    let elapsed = started.elapsed();

    match &result {
        Ok(entries) => {
            let message = format!(
                "load done: {} entries in {} ms: {}",
                entries.len().saturating_sub(1),
                elapsed.as_millis(),
                path.display()
            );
            if elapsed.as_millis() > SLOW_LOAD_MS {
                logging::warn(format!("slow directory load — {message}"));
            } else {
                logging::info(message);
            }
        }
        Err(error) => logging::warn(format!(
            "load failed in {} ms: {}: {error}",
            elapsed.as_millis(),
            path.display()
        )),
    }

    result
}

fn list_dir_inner(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();

    if let Some(parent) = path.parent() {
        entries.push(Entry::parent(parent.to_path_buf()));
    }

    for item in fs::read_dir(path)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().to_string();

        if !show_hidden && name.starts_with('.') {
            continue;
        }

        // `file_type()` comes from the directory entry itself on every platform
        // we care about, so it does not need a `stat`. This matters a lot on
        // cloud shares (Nextcloud/iCloud), where a `stat` can materialise the
        // file and take hundreds of milliseconds.
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        let is_symlink = file_type.is_symlink();
        let is_dir = file_type.is_dir();

        entries.push(Entry {
            name,
            path: item.path(),
            is_dir,
            is_symlink,
            is_parent: false,
            size: 0,
            modified: None,
            stats_loaded: false,
        });
    }

    Ok(entries)
}

/// Fill in size/date for entries that do not have them yet.
///
/// Runs on a worker thread and reports batches so the UI can update
/// incrementally. Symlinks are stat'ed through their target to report the real
/// kind, exactly like the synchronous listing used to do.
pub fn stat_entries(
    paths: Vec<(usize, PathBuf)>,
    generation: u64,
    sender: mpsc::Sender<StatsUpdate>,
) {
    const BATCH: usize = 16;

    let mut batch = Vec::with_capacity(BATCH);
    for (index, path) in paths {
        let meta = fs::metadata(&path).ok();
        let size = meta
            .as_ref()
            .map(|m| if m.is_dir() { 0 } else { m.len() })
            .unwrap_or(0);
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        batch.push((index, size, modified));

        if batch.len() >= BATCH {
            if sender
                .send(StatsUpdate {
                    generation,
                    stats: std::mem::take(&mut batch),
                })
                .is_err()
            {
                return;
            }
            batch.reserve(BATCH);
        }
    }

    if !batch.is_empty() {
        let _ = sender.send(StatsUpdate {
            generation,
            stats: batch,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_parent_dirs_then_files_and_hides_dotfiles() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("beta")).unwrap();
        fs::write(tmp.path().join("alpha.txt"), "hello").unwrap();
        fs::write(tmp.path().join(".secret"), "hidden").unwrap();

        let mut panel = Panel::new(tmp.path().to_path_buf());
        panel.reload(false).unwrap();

        assert_eq!(panel.entries[0].name, "..");
        assert!(panel.entries[0].is_parent);
        assert_eq!(panel.entries[1].name, "beta");
        assert!(panel.entries[1].is_dir);
        assert_eq!(panel.entries[2].name, "alpha.txt");
        assert!(!panel.entries.iter().any(|e| e.name == ".secret"));

        panel.reload(true).unwrap();
        assert!(panel.entries.iter().any(|e| e.name == ".secret"));
    }

    #[test]
    fn enters_directory_and_returns_with_cursor_on_child() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("inside.txt"), "x").unwrap();

        let mut panel = Panel::new(tmp.path().to_path_buf());
        panel.reload(false).unwrap();
        panel.select_name("child");
        assert!(panel.current_entry().unwrap().is_dir);

        panel.change_dir(child.clone(), None, false).unwrap();
        assert_eq!(panel.cwd, child);
        assert_eq!(panel.current_entry().unwrap().name, "inside.txt");

        panel
            .change_dir(tmp.path().to_path_buf(), Some("child"), false)
            .unwrap();
        assert_eq!(panel.current_entry().unwrap().name, "child");
    }

    #[test]
    fn selection_or_current_uses_selection_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();

        let mut panel = Panel::new(tmp.path().to_path_buf());
        panel.reload(false).unwrap();
        panel.select_name("a.txt");
        assert_eq!(panel.selection_or_current(), vec![tmp.path().join("a.txt")]);

        panel.selected.insert(tmp.path().join("b.txt"));
        let selected = panel.selection_or_current();
        assert_eq!(selected, vec![tmp.path().join("b.txt")]);
    }

    #[test]
    fn thumbnail_grid_navigation_scrolls_by_rows() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c", "d", "e", "f"] {
            fs::create_dir(tmp.path().join(name)).unwrap();
        }

        let mut panel = Panel::new(tmp.path().to_path_buf());
        panel.reload(false).unwrap();
        panel.set_mode(PanelMode::Thumbnails);
        panel.grid_cols = 2;
        panel.grid_rows = 2;
        panel.cursor = 1;
        panel.ensure_grid_visible();
        assert_eq!(panel.offset, 0);

        panel.move_grid(2);
        assert_eq!(panel.cursor, 3);
        panel.move_grid(2);
        assert_eq!(panel.cursor, 5);
        assert_eq!(panel.offset, 1);

        panel.move_grid(-4);
        assert_eq!(panel.cursor, 1);
        assert_eq!(panel.offset, 0);
    }

    #[test]
    fn leaving_preview_restores_the_previous_mode_and_clears_the_subject() {
        let mut panel = Panel::new(PathBuf::from("/tmp"));
        panel.set_mode(PanelMode::Thumbnails);
        panel.set_mode(PanelMode::Preview);
        panel.preview_path = Some(PathBuf::from("/tmp/photo.png"));
        panel.preview_scroll = 7;

        panel.leave_preview();

        assert_eq!(panel.mode, PanelMode::Thumbnails);
        assert!(panel.preview_path.is_none());
        assert_eq!(panel.preview_scroll, 0);
    }

    #[test]
    fn background_load_applies_only_the_newest_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("inside.txt"), "x").unwrap();

        let mut panel = Panel::new(tmp.path().to_path_buf());
        let stale = panel.spawn_load(first.clone(), None, false, LoadKind::Navigate);
        let newest = panel.spawn_load(second.clone(), None, false, LoadKind::Navigate);
        assert!(panel.loading);
        assert_eq!(panel.loading_path.as_deref(), Some(second.as_path()));

        // Drain both workers; the first result is stale and must be ignored.
        let stale_result = stale.recv().unwrap();
        let newest_result = newest.recv().unwrap();
        assert!(!panel.apply_load(stale_result));
        assert!(panel.apply_load(newest_result));

        assert_eq!(panel.cwd, second);
        assert!(!panel.loading);
        assert!(panel.loading_path.is_none());
        assert!(panel.entries.iter().any(|entry| entry.name == "inside.txt"));
    }

    #[test]
    fn cancel_load_drops_the_pending_result() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();

        let mut panel = Panel::new(tmp.path().to_path_buf());
        let receiver = panel.spawn_load(tmp.path().to_path_buf(), None, false, LoadKind::Refresh);
        panel.cancel_load();

        assert!(!panel.loading);
        assert!(panel.loading_path.is_none());
        assert!(!panel.apply_load(receiver.recv().unwrap()));
        assert_eq!(panel.cwd, tmp.path());
    }
}
