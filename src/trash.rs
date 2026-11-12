//! Moving entries to the system trash instead of deleting them outright.
//!
//! A file manager that erases a folder on a single key press is dangerous, and
//! that is exactly how a real project folder was lost here. Deletion now moves
//! the entry to the trash, where the desktop's own restore UI can bring it back:
//!
//! * **Linux** — the [freedesktop.org trash specification]: `$XDG_DATA_HOME/Trash`
//!   for files on the same filesystem, `$topdir/.Trash/$uid` on the volume
//!   itself when the entry lives on another mount (moving across filesystems is
//!   a copy, which we avoid). A `.trashinfo` file is written next to the payload
//!   so every file manager shows the original path and deletion date.
//! * **macOS** — `~/.Trash`, which Finder reads; the same volume rule applies.
//! * **Windows** — PowerShell's `Shell.Application` recycle-bin API, so entries
//!   land in the real `$RECYCLE.BIN` with their original path.
//!
//! When no trash is usable, [`trash`] returns an error and the caller falls back
//! to a plain delete, which the UI still confirms.
//!
//! [freedesktop.org trash specification]: https://specifications.freedesktop.org/trash-spec/latest/

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Optional override, mostly for tests: `YAZI2D_TRASH_DIR=/tmp/x`.
const TRASH_DIR_ENV: &str = "YAZI2D_TRASH_DIR";

/// Result of a successful trash move.
#[derive(Debug)]
pub struct Trashed {
    /// Where the entry now lives inside the trash.
    #[cfg_attr(not(test), allow(dead_code))]
    pub path: PathBuf,
}

/// Move `source` to the trash.
///
/// Returns an error when the platform trash cannot be used; the caller then
/// decides whether to fall back to a hard delete.
pub fn trash(source: &Path) -> Result<Trashed, String> {
    if std::env::var_os(TRASH_DIR_ENV).is_some() {
        return trash_into(&trash_dir_for(source)?, source, source);
    }

    #[cfg(target_os = "windows")]
    {
        return trash_windows(source);
    }

    #[cfg(not(target_os = "windows"))]
    {
        trash_into(&trash_dir_for(source)?, source, source)
    }
}

/// Root of the trash that can hold `source` on the same filesystem.
fn trash_dir_for(source: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os(TRASH_DIR_ENV) {
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        return Ok(dir);
    }

    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().ok_or("no home directory")?;
        Ok(home.join(".Trash"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // A rename only works inside one filesystem, so an external drive needs
        // its own `.Trash`. The spec puts it at `$topdir/.Trash/$uid`, falling
        // back to `$topdir/.Trash-$uid` when that is not writable.
        if let Some(mount) = mount_point_of(source) {
            let home_mount = dirs::home_dir()
                .as_deref()
                .and_then(mount_point_of)
                .unwrap_or_else(|| PathBuf::from("/"));
            if mount != home_mount {
                let uid = unsafe { libc_uid() };
                let shared = mount.join(".Trash").join(uid.to_string());
                if fs::create_dir_all(&shared).is_ok() && is_writable(&shared) {
                    return Ok(shared);
                }
                let private = mount.join(format!(".Trash-{uid}"));
                fs::create_dir_all(&private).map_err(|error| error.to_string())?;
                return Ok(private);
            }
        }

        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")))
            .ok_or("no data directory")?;
        let trash = data.join("Trash");
        fs::create_dir_all(trash.join("files")).map_err(|error| error.to_string())?;
        fs::create_dir_all(trash.join("info")).map_err(|error| error.to_string())?;
        Ok(trash)
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        Err("no trash implementation for this platform".to_string())
    }
}

/// The mount point `path` belongs to, by walking up until the device changes.
fn mount_point_of(path: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let mut current = path.canonicalize().ok()?;
        while !current.is_dir() {
            current = current.parent()?.to_path_buf();
        }
        let mut device = fs::metadata(&current).ok()?.dev();
        while let Some(parent) = current.parent() {
            let parent_device = match fs::metadata(parent) {
                Ok(meta) => meta.dev(),
                Err(_) => break,
            };
            if parent_device != device {
                break;
            }
            device = parent_device;
            current = parent.to_path_buf();
        }
        Some(current)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_writable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    // The sticky `.Trash` directory must be owned by the user to be usable.
    meta.uid() == unsafe { libc_uid() }
}

#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn libc_uid() -> u32 {
    // `geteuid` has no safe wrapper in std and is never called on Windows.
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

/// Move one entry into `trash_root`, writing a freedesktop `.trashinfo` when the
/// layout calls for one.
fn trash_into(trash_root: &Path, source: &Path, original: &Path) -> Result<Trashed, String> {
    // macOS `~/.Trash` keeps payloads directly inside the directory; the
    // freedesktop layout uses a `files/` subdirectory next to `info/`. The
    // override counts as a freedesktop trash, which is what tests exercise.
    let freedesktop =
        cfg!(all(unix, not(target_os = "macos"))) || std::env::var_os(TRASH_DIR_ENV).is_some();
    let files = if freedesktop {
        trash_root.join("files")
    } else {
        trash_root.to_path_buf()
    };
    fs::create_dir_all(&files).map_err(|error| error.to_string())?;

    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or("cannot trash a path without a name")?;

    let mut counter = 0usize;
    let target = loop {
        let candidate = if counter == 0 {
            name.clone()
        } else {
            match name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => format!("{stem}.{counter}.{ext}"),
                _ => format!("{name}.{counter}"),
            }
        };
        let path = files.join(&candidate);
        if !path.exists() {
            break path;
        }
        counter += 1;
        if counter > 10_000 {
            return Err(format!("cannot find a free name for {name} in the trash"));
        }
    };

    fs::rename(source, &target).map_err(|error| error.to_string())?;

    // The spec's info file is what lets a desktop file manager show the original
    // location and restore the entry to it.
    if freedesktop {
        let info = trash_root.join("info").join(format!(
            "{}.trashinfo",
            target
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        ));
        let _ = fs::create_dir_all(trash_root.join("info"));
        let _ = fs::write(
            info,
            format!(
                "[Trash Info]\nPath={}\nDeletionDate={}\n",
                original.display(),
                deletion_date()
            ),
        );
    }

    Ok(Trashed { path: target })
}

/// `YYYY-MM-DDThh:mm:ss`, the format the trash specification asks for.
fn deletion_date() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    chrono::Local
        .timestamp_opt(seconds, 0)
        .single()
        .map(|moment| moment.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn trash_windows(source: &Path) -> Result<Trashed, String> {
    use std::process::Command;

    // PowerShell is present on every supported Windows; Shell.Application is the
    // only API that puts an entry into the real recycle bin with its metadata.
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; \
         [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{}','OnlyErrorDialogs','SendToRecycleBin')",
        source.display()
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(Trashed {
            path: source.to_path_buf(),
        })
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

use chrono::TimeZone;

#[cfg(test)]
mod tests {
    use super::*;

    fn with_trash<T>(dir: &Path, run: impl FnOnce() -> T) -> T {
        // The override is process-wide, so these tests are serialised by `cargo
        // test`'s default single-threaded-per-module? No: use a mutex.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        std::env::set_var(TRASH_DIR_ENV, dir);
        let result = run();
        std::env::remove_var(TRASH_DIR_ENV);
        result
    }

    #[test]
    fn a_file_moves_to_the_trash_with_an_info_file() {
        let tmp = tempfile::tempdir().unwrap();
        let trash_root = tmp.path().join("Trash");
        let work = tmp.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let file = work.join("notes.txt");
        fs::write(&file, "hello").unwrap();

        let trashed = with_trash(&trash_root, || trash(&file)).expect("trash failed");

        assert!(!file.exists(), "the original must be gone");
        assert_eq!(fs::read_to_string(&trashed.path).unwrap(), "hello");

        let info = fs::read_dir(trash_root.join("info"))
            .unwrap()
            .flatten()
            .next()
            .expect("no .trashinfo written");
        let text = fs::read_to_string(info.path()).unwrap();
        assert!(text.contains("[Trash Info]"));
        // The recorded path is the one the entry had; a temp dir may be behind a
        // symlink (`/tmp` → `/private/tmp`), so the file name is what matters.
        assert!(text.contains("Path="), "no Path= in {text:?}");
        assert!(
            text.contains("notes.txt"),
            "the original name is not recorded: {text:?}"
        );
        assert!(text.contains("DeletionDate="));
    }

    #[test]
    fn a_directory_moves_with_all_of_its_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let trash_root = tmp.path().join("Trash");
        let folder = tmp.path().join("project");
        fs::create_dir_all(folder.join("nested")).unwrap();
        fs::write(folder.join("nested/file.txt"), "x").unwrap();

        let trashed = with_trash(&trash_root, || trash(&folder)).expect("trash failed");

        assert!(!folder.exists());
        assert!(trashed.path.join("nested/file.txt").is_file());
    }

    #[test]
    fn a_name_collision_gets_a_suffix_instead_of_overwriting() {
        let tmp = tempfile::tempdir().unwrap();
        let trash_root = tmp.path().join("Trash");
        let work = tmp.path().join("work");
        fs::create_dir_all(&work).unwrap();

        let first = work.join("same.txt");
        fs::write(&first, "one").unwrap();
        let first_trashed = with_trash(&trash_root, || trash(&first)).unwrap();

        let second = work.join("same.txt");
        fs::write(&second, "two").unwrap();
        let second_trashed = with_trash(&trash_root, || trash(&second)).unwrap();

        assert_ne!(first_trashed.path, second_trashed.path);
        assert_eq!(fs::read_to_string(&first_trashed.path).unwrap(), "one");
        assert_eq!(fs::read_to_string(&second_trashed.path).unwrap(), "two");
    }

    #[test]
    fn trashing_a_missing_path_reports_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let trash_root = tmp.path().join("Trash");
        let missing = tmp.path().join("nope.txt");
        assert!(with_trash(&trash_root, || trash(&missing)).is_err());
    }
}
