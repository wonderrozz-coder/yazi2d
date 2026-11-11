use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct FavoritesFile {
    #[serde(default)]
    favorites: Vec<String>,
}

/// Persistent bookmark list stored in a small TOML file.
pub struct Favorites {
    pub paths: Vec<PathBuf>,
    file: PathBuf,
}

impl Favorites {
    /// Load the user's favorites, falling back to well-known folders on first run.
    pub fn load_or_default() -> Self {
        let file = config_file();
        let mut favorites = Self::load_from_file(file.clone());
        if favorites.paths.is_empty() {
            favorites.paths = default_paths();
        }
        favorites.file = file;
        favorites
    }

    /// Load from an explicit config directory (used by tests).
    #[cfg(test)]
    pub fn load_from_dir(dir: &Path) -> Self {
        let file = dir.join("favorites.toml");
        let mut favorites = Self::load_from_file(file.clone());
        favorites.file = file;
        favorites
    }

    fn load_from_file(file: PathBuf) -> Self {
        let paths = fs::read_to_string(&file)
            .ok()
            .and_then(|raw| toml::from_str::<FavoritesFile>(&raw).ok())
            .map(|data| data.favorites.into_iter().map(PathBuf::from).collect())
            .unwrap_or_default();
        Self { paths, file }
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = FavoritesFile {
            favorites: self
                .paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
        };
        let raw = toml::to_string_pretty(&data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&self.file, raw)
    }

    /// Add a path (canonicalized when possible). Returns false if it is already present.
    pub fn add(&mut self, path: PathBuf) -> bool {
        let path = path.canonicalize().unwrap_or(path);
        if self.contains(&path) {
            return false;
        }
        self.paths.push(path);
        true
    }

    pub fn remove(&mut self, index: usize) -> Option<PathBuf> {
        if index < self.paths.len() {
            Some(self.paths.remove(index))
        } else {
            None
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|p| p == path)
    }

    #[cfg(test)]
    pub fn file_path(&self) -> &Path {
        &self.file
    }
}

pub fn config_file() -> PathBuf {
    if let Ok(dir) = std::env::var("YAZI2D_CONFIG_DIR") {
        return PathBuf::from(dir).join("favorites.toml");
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("yazi2d")
        .join("favorites.toml")
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

    #[test]
    fn add_dedupe_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = Favorites::load_from_dir(tmp.path());

        assert!(favorites.add(PathBuf::from("/tmp")));
        assert!(!favorites.add(PathBuf::from("/tmp")));
        assert_eq!(favorites.paths.len(), 1);

        favorites.save().unwrap();
        let loaded = Favorites::load_from_dir(tmp.path());
        assert_eq!(loaded.paths, favorites.paths);
        assert_eq!(loaded.file_path(), tmp.path().join("favorites.toml"));
    }

    #[test]
    fn remove_returns_path_and_ignores_out_of_bounds() {
        let tmp = tempfile::tempdir().unwrap();
        let mut favorites = Favorites::load_from_dir(tmp.path());
        favorites.paths.push(PathBuf::from("/a"));
        favorites.paths.push(PathBuf::from("/b"));

        assert_eq!(favorites.remove(0), Some(PathBuf::from("/a")));
        assert_eq!(favorites.remove(99), None);
        assert_eq!(favorites.paths, vec![PathBuf::from("/b")]);
    }
}
