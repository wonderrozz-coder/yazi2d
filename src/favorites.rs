//! Persistent bookmark list: folders, files and named separators.
//!
//! Entries are stored as a list in `$YAZI2D_CONFIG_DIR/favorites.toml`, in the
//! order the sidebar shows them:
//!
//! ```toml
//! favorites = ["/home/me", "/home/me/Projects"]
//!
//! [[group]]
//! title = "Work"
//!
//! [[group]]
//! kind = "path"
//! path = "/srv/repo"
//!
//! [[group]]
//! kind = "path"
//! path = "/home/me/notes.md"
//! ```
//!
//! The flat `favorites = [...]` list is still read (that is what older versions
//! wrote), so an existing file keeps working; new data uses `[[group]]` entries,
//! which is what makes separators and file bookmarks possible. Both a folder and
//! a file can be bookmarked — the sidebar shows the type icon and revealing a
//! file selects it in its own folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One row of the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A bookmarked folder or file.
    Path(PathBuf),
    /// A named separator, drawn as a caption. Not selectable as a folder.
    Separator(String),
}

impl Entry {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Entry::Path(path) => Some(path),
            Entry::Separator(_) => None,
        }
    }

    /// Display name: the file name, or the separator title.
    pub fn label(&self) -> String {
        match self {
            Entry::Path(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            Entry::Separator(title) => title.clone(),
        }
    }

    pub fn is_separator(&self) -> bool {
        matches!(self, Entry::Separator(_))
    }
}

#[derive(Default, Serialize, Deserialize)]
struct FavoritesFile {
    /// Legacy flat list; read for compatibility, never written any more.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    favorites: Vec<String>,
    /// Current format: an ordered list of rows.
    #[serde(default, rename = "group", skip_serializing_if = "Vec::is_empty")]
    group: Vec<GroupRow>,
}

#[derive(Serialize, Deserialize)]
struct GroupRow {
    /// `"path"` or `"separator"`.
    #[serde(default = "default_kind", skip_serializing_if = "is_path_kind")]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

fn default_kind() -> String {
    "path".to_string()
}

fn is_path_kind(kind: &String) -> bool {
    kind == "path"
}

/// Persistent bookmark list.
pub struct Favorites {
    pub entries: Vec<Entry>,
    file: PathBuf,
}

impl Favorites {
    /// Load the user's favorites, falling back to well-known folders on first run.
    pub fn load_or_default() -> Self {
        let file = config_file();
        let mut favorites = Self::load_from_file(file.clone());
        if favorites.entries.is_empty() {
            favorites.entries = default_paths().into_iter().map(Entry::Path).collect();
        }
        favorites.file = file;
        favorites
    }

    /// Load from an explicit config directory (used by tests).
    #[cfg(test)]
    pub fn load_from_dir(dir: &Path) -> Self {
        let file = dir.join("favorites.toml");
        Self::load_from_file(file)
    }

    fn load_from_file(file: PathBuf) -> Self {
        let data = fs::read_to_string(&file)
            .ok()
            .and_then(|raw| toml::from_str::<FavoritesFile>(&raw).ok())
            .unwrap_or_default();

        let mut entries: Vec<Entry> = data
            .group
            .into_iter()
            .filter_map(|row| {
                if row.kind == "separator" {
                    let title = row.title.unwrap_or_default();
                    return (!title.trim().is_empty()).then_some(Entry::Separator(title));
                }
                row.path.map(|path| Entry::Path(PathBuf::from(path)))
            })
            .collect();

        // A file written by an older version has only the flat list.
        if entries.is_empty() {
            entries = data
                .favorites
                .into_iter()
                .map(|p| Entry::Path(PathBuf::from(p)))
                .collect();
        }

        Self { entries, file }
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let group = self
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Path(path) => GroupRow {
                    kind: default_kind(),
                    path: Some(path.to_string_lossy().to_string()),
                    title: None,
                },
                Entry::Separator(title) => GroupRow {
                    kind: "separator".to_string(),
                    path: None,
                    title: Some(title.clone()),
                },
            })
            .collect();

        let data = FavoritesFile {
            favorites: Vec::new(),
            group,
        };
        let raw = toml::to_string_pretty(&data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&self.file, raw)
    }

    /// Every row, in sidebar order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Number of selectable rows (separators are captions, not targets).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Path of a row, if it is a path row.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn path_at(&self, index: usize) -> Option<&Path> {
        self.entries.get(index)?.path()
    }

    /// Every bookmarked path, skipping separators.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.entries.iter().filter_map(Entry::path)
    }

    /// Add a path (canonicalized when possible). Returns false if it is already present.
    pub fn add(&mut self, path: PathBuf) -> bool {
        let path = path.canonicalize().unwrap_or(path);
        if self.contains(&path) {
            return false;
        }
        self.entries.push(Entry::Path(path));
        true
    }

    /// Add a named separator after the cursor row.
    pub fn add_separator(&mut self, title: String, after: Option<usize>) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        let index = after
            .map(|index| (index + 1).min(self.entries.len()))
            .unwrap_or(self.entries.len());
        self.entries.insert(index, Entry::Separator(title));
    }

    pub fn remove(&mut self, index: usize) -> Option<Entry> {
        if index < self.entries.len() {
            Some(self.entries.remove(index))
        } else {
            None
        }
    }

    /// Move the row at `index` by `delta`, clamped to the list.
    ///
    /// Returns the new index, so the caller can keep the cursor on the row it
    /// just moved.
    pub fn move_row(&mut self, index: usize, delta: i32) -> Option<usize> {
        if index >= self.entries.len() || delta == 0 {
            return None;
        }
        let last = self.entries.len() as i32 - 1;
        let target = (index as i32 + delta).clamp(0, last) as usize;
        if target == index {
            return None;
        }
        let entry = self.entries.remove(index);
        self.entries.insert(target, entry);
        Some(target)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.paths().any(|existing| existing == path)
    }

    #[cfg(test)]
    pub fn file_path(&self) -> &Path {
        &self.file
    }

    /// Point the store at a given file (tests only).
    #[cfg(test)]
    pub fn set_file(&mut self, file: PathBuf) {
        self.file = file;
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("YAZI2D_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("yazi2d")
}

pub fn config_file() -> PathBuf {
    config_dir().join("favorites.toml")
}

fn default_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    let mut paths = vec![home.clone()];
    for name in ["Desktop", "Documents", "Downloads"] {
        let path = home.join(name);
        if path.is_dir() {
            paths.push(path);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(dir: &Path) -> Favorites {
        Favorites {
            entries: Vec::new(),
            file: dir.join("favorites.toml"),
        }
    }

    #[test]
    fn add_dedupe_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = empty(tmp.path());

        assert!(favorites.add(PathBuf::from("/tmp")));
        assert!(!favorites.add(PathBuf::from("/tmp")));
        assert_eq!(favorites.len(), 1);

        favorites.save().unwrap();
        let loaded = Favorites::load_from_dir(tmp.path());
        assert_eq!(loaded.entries, favorites.entries);
        assert_eq!(loaded.file_path(), tmp.path().join("favorites.toml"));
    }

    #[test]
    fn remove_returns_path_and_ignores_out_of_bounds() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = empty(tmp.path());
        favorites.entries.push(Entry::Path(PathBuf::from("/a")));
        favorites.entries.push(Entry::Path(PathBuf::from("/b")));

        assert_eq!(favorites.remove(0), Some(Entry::Path(PathBuf::from("/a"))));
        assert_eq!(favorites.remove(99), None);
        assert_eq!(favorites.entries, vec![Entry::Path(PathBuf::from("/b"))]);
    }

    #[test]
    fn separators_round_trip_and_keep_their_place() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = empty(tmp.path());
        favorites.add(PathBuf::from("/work"));
        favorites.add_separator("Personal".to_string(), Some(0));
        favorites.add(PathBuf::from("/music"));

        assert_eq!(
            favorites.entries,
            vec![
                Entry::Path(PathBuf::from("/work")),
                Entry::Separator("Personal".to_string()),
                Entry::Path(PathBuf::from("/music")),
            ]
        );

        favorites.save().unwrap();
        let loaded = Favorites::load_from_dir(tmp.path());
        assert_eq!(loaded.entries, favorites.entries);
    }

    #[test]
    fn an_empty_separator_title_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = empty(tmp.path());
        favorites.add_separator("   ".to_string(), None);
        assert!(favorites.entries.is_empty());
    }

    #[test]
    fn files_can_be_bookmarked_too() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = empty(tmp.path());
        let file = tmp.path().join("notes.md");
        fs::write(&file, "x").unwrap();

        assert!(favorites.add(file.clone()));
        // `add` canonicalizes, and on macOS a temp dir is behind the `/var`
        // symlink, so the expected path has to be canonicalized as well.
        let expected = file.canonicalize().unwrap();
        assert_eq!(favorites.path_at(0), Some(expected.as_path()));
        assert!(!favorites.entries[0].is_separator());
    }

    #[test]
    fn rows_move_up_and_down_and_stay_clamped() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = empty(tmp.path());
        for name in ["a", "b", "c"] {
            favorites.add(tmp.path().join(name));
        }

        // b (index 1) moves above a.
        assert_eq!(favorites.move_row(1, -1), Some(0));
        assert_eq!(favorites.entries[0].label(), "b");

        // Moving the first row up does nothing.
        assert_eq!(favorites.move_row(0, -1), None);

        // The last row cannot fall off the end.
        assert_eq!(favorites.move_row(2, 5), None);
    }

    #[test]
    fn an_old_flat_file_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("favorites.toml");
        fs::write(&file, "favorites = [\"/one\", \"/two\"]\n").unwrap();

        let loaded = Favorites::load_from_dir(tmp.path());
        assert_eq!(
            loaded.entries,
            vec![
                Entry::Path(PathBuf::from("/one")),
                Entry::Path(PathBuf::from("/two"))
            ]
        );
    }
}
