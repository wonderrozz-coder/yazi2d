use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
        }
    }
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
}

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
        }
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
        self.cursor = keep
            .and_then(|name| self.entries.iter().position(|e| e.name == name))
            .unwrap_or(if self.entries.len() > 1 { 1 } else { 0 })
            .min(self.entries.len().saturating_sub(1));
        self.error = None;
        self.ensure_visible();
        Ok(())
    }

    /// Change directory. `select` is a child name to put the cursor on (used when going up).
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
        self.cursor = select
            .and_then(|name| self.entries.iter().position(|e| e.name == name))
            .unwrap_or(if self.entries.len() > 1 { 1 } else { 0 })
            .min(self.entries.len().saturating_sub(1));
        self.error = None;
        self.ensure_visible();
        Ok(())
    }

    pub fn current_entry(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub fn move_cursor(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as i32 - 1;
        self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
        self.ensure_visible();
    }

    pub fn go_top(&mut self) {
        self.cursor = 0;
        self.offset = 0;
    }

    pub fn go_bottom(&mut self) {
        self.cursor = self.entries.len().saturating_sub(1);
        self.ensure_visible();
    }

    pub fn select_name(&mut self, name: &str) {
        if let Some(index) = self.entries.iter().position(|e| e.name == name) {
            self.cursor = index;
            self.ensure_visible();
        }
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
        self.move_cursor(1);
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
}

fn list_dir(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
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

        let Ok(file_type) = item.file_type() else {
            continue;
        };
        let is_symlink = file_type.is_symlink();

        // Follow symlinks for display, but never for the symlink check itself.
        let meta = if is_symlink {
            fs::metadata(item.path()).ok()
        } else {
            item.metadata().ok()
        };

        let is_dir = meta
            .as_ref()
            .map(|m| m.is_dir())
            .unwrap_or_else(|| file_type.is_dir());
        let size = meta
            .as_ref()
            .map(|m| if m.is_dir() { 0 } else { m.len() })
            .unwrap_or(0);
        let modified = meta.as_ref().and_then(|m| m.modified().ok());

        entries.push(Entry {
            name,
            path: item.path(),
            is_dir,
            is_symlink,
            is_parent: false,
            size,
            modified,
        });
    }

    // Parent first, then directories, then files, case-insensitively.
    entries.sort_by_cached_key(|e| {
        (
            if e.is_parent { 0 } else { 1 },
            if e.is_dir { 0 } else { 1 },
            e.name.to_lowercase(),
        )
    });

    Ok(entries)
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
}
