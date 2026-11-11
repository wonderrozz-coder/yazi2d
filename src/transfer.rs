use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobKind {
    Copy,
    Move,
    Delete,
}

impl JobKind {
    /// Verb used while the job is running.
    pub fn verb(self) -> &'static str {
        match self {
            JobKind::Copy => "Copying",
            JobKind::Move => "Moving",
            JobKind::Delete => "Deleting",
        }
    }

    /// Verb used in the final status message.
    pub fn done_verb(self) -> &'static str {
        match self {
            JobKind::Copy => "Copied",
            JobKind::Move => "Moved",
            JobKind::Delete => "Deleted",
        }
    }
}

#[derive(Debug)]
pub enum JobMsg {
    Progress {
        done: usize,
        total: usize,
        current: String,
    },
    Done {
        done: usize,
        errors: Vec<String>,
    },
}

pub struct Job {
    pub kind: JobKind,
    pub rx: Receiver<JobMsg>,
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub errors: Vec<String>,
    pub finished: bool,
}

/// Start a filesystem operation on a worker thread. The UI stays responsive and
/// polls progress through `Job::rx`.
pub fn spawn(kind: JobKind, sources: Vec<PathBuf>, dest: Option<PathBuf>) -> Job {
    let (tx, rx) = mpsc::channel();
    let sources = dedupe_sources(sources);
    let dest = dest.unwrap_or_default();

    thread::spawn(move || {
        let (done, errors) = match kind {
            JobKind::Delete => run_delete(&sources, &tx),
            JobKind::Copy | JobKind::Move => run_transfer(kind, &sources, &dest, &tx),
        };
        let _ = tx.send(JobMsg::Done { done, errors });
    });

    Job {
        kind,
        rx,
        done: 0,
        total: 0,
        current: String::new(),
        errors: Vec::new(),
        finished: false,
    }
}

struct Item {
    src: PathBuf,
    dst: PathBuf,
    kind: ItemKind,
}

enum ItemKind {
    Dir,
    File,
    Symlink,
}

struct Group {
    src: PathBuf,
    dst: PathBuf,
    items: Vec<Item>,
    files: usize,
    is_real_dir: bool,
}

impl Group {
    fn units(&self) -> usize {
        self.files.max(1)
    }
}

fn run_transfer(
    kind: JobKind,
    sources: &[PathBuf],
    dest: &Path,
    tx: &Sender<JobMsg>,
) -> (usize, Vec<String>) {
    let mut errors = Vec::new();

    if let Err(e) = fs::create_dir_all(dest) {
        return (0, vec![format!("{}: {}", dest.display(), e)]);
    }

    let mut taken = HashSet::new();
    let mut groups = Vec::new();
    for source in sources {
        match Group::scan(source, dest, &mut taken) {
            Ok(group) => groups.push(group),
            Err(e) => push_error(&mut errors, e),
        }
    }

    let total: usize = groups.iter().map(|g| g.units()).sum::<usize>().max(1);
    let mut done = 0;
    let _ = tx.send(JobMsg::Progress {
        done,
        total,
        current: String::new(),
    });

    for group in groups {
        let _ = tx.send(JobMsg::Progress {
            done,
            total,
            current: group.src.display().to_string(),
        });

        if kind == JobKind::Move {
            // Fast path: a single rename is atomic and instant on the same filesystem.
            if fs::rename(&group.src, &group.dst).is_ok() {
                done += group.units();
                let _ = tx.send(JobMsg::Progress {
                    done,
                    total,
                    current: group.dst.display().to_string(),
                });
                continue;
            }
        }

        let errors_before = errors.len();
        for item in &group.items {
            let _ = tx.send(JobMsg::Progress {
                done,
                total,
                current: item.src.display().to_string(),
            });

            match copy_item(item) {
                Ok(()) => {
                    if !matches!(item.kind, ItemKind::Dir) {
                        done += 1;
                    }
                }
                Err(e) => push_error(&mut errors, format!("{}: {}", item.src.display(), e)),
            }
        }

        if group.files == 0 {
            done += 1;
        }

        // Apply directory permissions after the contents are written, so that
        // read-only source directories do not block their own children.
        apply_dir_permissions(&group.items);

        if kind == JobKind::Move && errors.len() == errors_before {
            let removed = if group.is_real_dir {
                fs::remove_dir_all(&group.src)
            } else {
                fs::remove_file(&group.src)
            };
            if let Err(e) = removed {
                push_error(&mut errors, format!("{}: {}", group.src.display(), e));
            }
        }
    }

    (done, errors)
}

fn run_delete(sources: &[PathBuf], tx: &Sender<JobMsg>) -> (usize, Vec<String>) {
    let mut errors = Vec::new();
    let mut groups: Vec<(PathBuf, bool, usize)> = Vec::new();

    for source in sources {
        match fs::symlink_metadata(source) {
            Ok(meta) => {
                let units = count_files(source).max(1);
                groups.push((source.clone(), meta.is_dir(), units));
            }
            Err(e) => push_error(&mut errors, format!("{}: {}", source.display(), e)),
        }
    }

    let total: usize = groups
        .iter()
        .map(|(_, _, units)| *units)
        .sum::<usize>()
        .max(1);
    let mut done = 0;
    let _ = tx.send(JobMsg::Progress {
        done,
        total,
        current: String::new(),
    });

    for (source, is_real_dir, units) in groups {
        let _ = tx.send(JobMsg::Progress {
            done,
            total,
            current: source.display().to_string(),
        });

        let removed = if is_real_dir {
            fs::remove_dir_all(&source)
        } else {
            fs::remove_file(&source)
        };

        match removed {
            Ok(()) => done += units,
            Err(e) => push_error(&mut errors, format!("{}: {}", source.display(), e)),
        }
    }

    (done, errors)
}

impl Group {
    fn scan(source: &Path, dest: &Path, taken: &mut HashSet<PathBuf>) -> Result<Self, String> {
        let meta =
            fs::symlink_metadata(source).map_err(|e| format!("{}: {}", source.display(), e))?;
        let name = source
            .file_name()
            .ok_or_else(|| format!("Invalid source path: {}", source.display()))?;

        let dst = unique_dest(dest, name, taken);
        taken.insert(dst.clone());

        let mut items = Vec::new();
        let mut files = 0;
        walk(source, &dst, &mut items, &mut files)?;

        Ok(Self {
            src: source.to_path_buf(),
            dst,
            items,
            files,
            is_real_dir: meta.is_dir(),
        })
    }
}

fn walk(src: &Path, dst: &Path, items: &mut Vec<Item>, files: &mut usize) -> Result<(), String> {
    let meta = fs::symlink_metadata(src).map_err(|e| format!("{}: {}", src.display(), e))?;

    if meta.file_type().is_symlink() {
        items.push(Item {
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
            kind: ItemKind::Symlink,
        });
        *files += 1;
    } else if meta.is_dir() {
        items.push(Item {
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
            kind: ItemKind::Dir,
        });

        let mut children: Vec<_> = fs::read_dir(src)
            .map_err(|e| format!("{}: {}", src.display(), e))?
            .filter_map(Result::ok)
            .collect();
        children.sort_by_key(|entry| entry.file_name());

        for child in children {
            walk(&child.path(), &dst.join(child.file_name()), items, files)?;
        }
    } else {
        items.push(Item {
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
            kind: ItemKind::File,
        });
        *files += 1;
    }

    Ok(())
}

fn copy_item(item: &Item) -> io::Result<()> {
    match item.kind {
        ItemKind::Dir => {
            fs::create_dir_all(&item.dst)?;
            Ok(())
        }
        ItemKind::File => {
            if let Some(parent) = item.dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&item.src, &item.dst)?;
            Ok(())
        }
        ItemKind::Symlink => {
            if let Some(parent) = item.dst.parent() {
                fs::create_dir_all(parent)?;
            }
            let target = fs::read_link(&item.src)?;
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, &item.dst)?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                let _ = target;
                fs::copy(&item.src, &item.dst)?;
                Ok(())
            }
        }
    }
}

fn apply_dir_permissions(items: &[Item]) {
    // Deepest directories first: a read-only parent must be set last.
    for item in items.iter().rev() {
        if matches!(item.kind, ItemKind::Dir) {
            if let Ok(meta) = fs::metadata(&item.src) {
                let _ = fs::set_permissions(&item.dst, meta.permissions());
            }
        }
    }
}

fn unique_dest(dir: &Path, name: &OsStr, taken: &HashSet<PathBuf>) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() && !taken.contains(&candidate) {
        return candidate;
    }

    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string_lossy().to_string());
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());

    for index in 1..10_000 {
        let file_name = match &ext {
            Some(ext) => format!("{stem} ({index}).{ext}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = dir.join(file_name);
        if !candidate.exists() && !taken.contains(&candidate) {
            return candidate;
        }
    }

    dir.join(format!("{stem} (copy)"))
}

fn dedupe_sources(sources: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut sorted = sources;
    sorted.sort();
    sorted.dedup();

    let mut result: Vec<PathBuf> = Vec::new();
    for path in sorted {
        // If a parent is already being processed, its children come along for free.
        if !result.iter().any(|parent| path.starts_with(parent)) {
            result.push(path);
        }
    }
    result
}

fn count_files(path: &Path) -> usize {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return 1;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| count_files(&entry.path()))
        .sum()
}

fn push_error(errors: &mut Vec<String>, message: String) {
    if errors.len() < 100 {
        errors.push(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait(job: Job) -> (usize, Vec<String>) {
        loop {
            match job.rx.recv_timeout(Duration::from_secs(10)) {
                Ok(JobMsg::Progress { .. }) => {}
                Ok(JobMsg::Done { done, errors }) => return (done, errors),
                Err(e) => panic!("job did not finish: {e:?}"),
            }
        }
    }

    #[test]
    fn copies_file_and_avoids_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        let dst_dir = tmp.path().join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        let file = src_dir.join("a.txt");
        fs::write(&file, "hello").unwrap();

        wait(spawn(
            JobKind::Copy,
            vec![file.clone()],
            Some(dst_dir.clone()),
        ));
        assert_eq!(fs::read_to_string(dst_dir.join("a.txt")).unwrap(), "hello");

        wait(spawn(JobKind::Copy, vec![file], Some(dst_dir.clone())));
        assert_eq!(
            fs::read_to_string(dst_dir.join("a (1).txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn copies_directory_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("tree");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();
        fs::write(src.join("nested").join("child.txt"), "child").unwrap();
        fs::create_dir_all(&dst).unwrap();

        let (done, errors) = wait(spawn(JobKind::Copy, vec![src], Some(dst.clone())));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(done >= 2);
        assert_eq!(
            fs::read_to_string(dst.join("tree").join("nested").join("child.txt")).unwrap(),
            "child"
        );
    }

    #[test]
    fn moves_file_and_removes_source() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("move-me.txt");
        let dst = tmp.path().join("dst");
        fs::write(&src, "data").unwrap();
        fs::create_dir_all(&dst).unwrap();

        let (_, errors) = wait(spawn(JobKind::Move, vec![src.clone()], Some(dst.clone())));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(dst.join("move-me.txt")).unwrap(), "data");
    }

    #[test]
    fn deletes_directory_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let victim = tmp.path().join("victim");
        fs::create_dir_all(victim.join("nested")).unwrap();
        fs::write(victim.join("nested").join("x.txt"), "x").unwrap();

        let (done, errors) = wait(spawn(JobKind::Delete, vec![victim.clone()], None));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!victim.exists());
        assert!(done >= 1);
    }

    #[cfg(unix)]
    #[test]
    fn copies_symlink_as_symlink() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("target.txt"), "target").unwrap();
        let link = src.join("link");
        symlink(src.join("target.txt"), &link).unwrap();

        let (_, errors) = wait(spawn(JobKind::Copy, vec![link], Some(dst.clone())));
        assert!(errors.is_empty(), "{errors:?}");

        let copied = dst.join("link");
        let meta = fs::symlink_metadata(&copied).unwrap();
        assert!(meta.file_type().is_symlink());
        assert_eq!(fs::read_link(&copied).unwrap(), src.join("target.txt"));
    }

    #[test]
    fn nested_sources_are_deduplicated() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent");
        let child = parent.join("child.txt");
        fs::create_dir_all(&parent).unwrap();
        fs::write(&child, "x").unwrap();

        let sources = dedupe_sources(vec![child, parent.clone(), parent.clone()]);
        assert_eq!(sources, vec![parent]);
    }
}
