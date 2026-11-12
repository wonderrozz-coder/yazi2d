//! Recursive file search behind `Alt+F7`.
//!
//! The walk runs on a worker thread and streams batches of hits back to the UI,
//! so a search inside a huge folder never freezes the panels. Matching is a
//! case-insensitive substring, unless the query contains `*` or `?`, in which
//! case it is treated as a wildcard mask (`*.rs`, `shot*.png`).
//!
//! Symlinked directories are never followed (they would loop), hidden entries
//! are skipped unless hidden files are enabled, and both the result count and
//! the depth are capped so a search cannot exhaust memory.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use crate::logging;

/// Stop collecting once this many hits were found.
pub const MAX_RESULTS: usize = 1000;
/// Never descend deeper than this.
const MAX_DEPTH: usize = 24;
/// Hits are sent to the UI in batches to keep redraws cheap.
const BATCH: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// One progress update from the search worker.
pub struct SearchMsg {
    pub hits: Vec<Hit>,
    pub scanned: usize,
    pub truncated: bool,
    pub done: bool,
}

/// Start a recursive search under `root`. The receiver yields batches until a
/// message with `done = true` arrives (or the channel closes).
pub fn spawn(root: PathBuf, query: String, show_hidden: bool) -> Receiver<SearchMsg> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || walk(root, query, show_hidden, sender));
    receiver
}

fn walk(root: PathBuf, query: String, show_hidden: bool, sender: Sender<SearchMsg>) {
    let started = Instant::now();
    logging::info(format!("search start: {query:?} in {}", root.display()));

    let mut scanned = 0usize;
    let mut truncated = false;
    let mut batch: Vec<Hit> = Vec::new();
    let mut found = 0usize;
    let mut stack = vec![(root.clone(), 0usize)];

    'walk: while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let is_dir = file_type.is_dir();
            scanned += 1;

            if matches(&query, &name) {
                if found >= MAX_RESULTS {
                    truncated = true;
                    break 'walk;
                }
                found += 1;
                batch.push(Hit {
                    path: path.clone(),
                    is_dir,
                });
                if batch.len() >= BATCH && !send(&sender, &mut batch, scanned, truncated) {
                    return;
                }
            }

            // Never follow a symlink: it may point back up the tree.
            if is_dir && !file_type.is_symlink() {
                stack.push((path, depth + 1));
            }
        }
    }

    let _ = sender.send(SearchMsg {
        hits: std::mem::take(&mut batch),
        scanned,
        truncated,
        done: true,
    });
    logging::info(format!(
        "search done: {found} hit(s), {scanned} scanned in {} ms{}",
        started.elapsed().as_millis(),
        if truncated { " (truncated)" } else { "" }
    ));
}

/// Send a batch; `false` means the UI closed the search window.
fn send(sender: &Sender<SearchMsg>, batch: &mut Vec<Hit>, scanned: usize, truncated: bool) -> bool {
    let message = SearchMsg {
        hits: std::mem::take(batch),
        scanned,
        truncated,
        done: false,
    };
    sender.send(message).is_ok()
}

/// Does `name` match the query?
///
/// No wildcard characters means a case-insensitive substring match; otherwise
/// the whole name must match the mask.
pub fn matches(query: &str, name: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_lowercase();
    let name = name.to_lowercase();

    if query.contains('*') || query.contains('?') {
        wildcard(query.chars().collect::<Vec<_>>().as_slice(), &name)
    } else {
        name.contains(&query)
    }
}

/// Classic two-pointer wildcard match with `?` (one character) and `*`.
fn wildcard(pattern: &[char], text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;

    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(position) = star {
            p = position + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plain_queries_match_as_substrings() {
        assert!(matches("read", "README.md"));
        assert!(matches("README", "readme.md"));
        assert!(matches("папка", "Моя папка"));
        assert!(!matches("readme", "license.txt"));
    }

    #[test]
    fn masks_match_the_whole_name() {
        assert!(matches("*.rs", "main.rs"));
        assert!(!matches("*.rs", "main.rs.bak"));
        assert!(matches("shot*.png", "shot-1.png"));
        assert!(matches("s?ot.png", "shot.png"));
        assert!(!matches("s?ot.png", "shoot.png"));
        assert!(matches("*a*b*", "xaaybzz"));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches("", "anything"));
    }

    #[test]
    fn finds_files_recursively_and_reports_done() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("nested/deeper")).unwrap();
        fs::write(tmp.path().join("nested/alpha.txt"), "x").unwrap();
        fs::write(tmp.path().join("nested/deeper/beta.txt"), "x").unwrap();
        fs::write(tmp.path().join("gamma.md"), "x").unwrap();

        let receiver = spawn(tmp.path().to_path_buf(), "*.txt".to_string(), false);
        let mut hits = Vec::new();
        let mut done = false;
        while let Ok(message) = receiver.recv() {
            hits.extend(message.hits);
            done = message.done;
            if done {
                break;
            }
        }

        assert!(done, "worker never reported completion");
        let names: Vec<String> = hits
            .iter()
            .map(|hit| hit.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"alpha.txt".to_string()));
        assert!(names.contains(&"beta.txt".to_string()));
        assert!(!names.contains(&"gamma.md".to_string()));
    }

    #[test]
    fn hidden_entries_are_skipped_unless_requested() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".secret.txt"), "x").unwrap();

        let collect = |show_hidden: bool| {
            let receiver = spawn(tmp.path().to_path_buf(), ".secret".to_string(), show_hidden);
            let mut hits = 0;
            while let Ok(message) = receiver.recv() {
                hits += message.hits.len();
                if message.done {
                    break;
                }
            }
            hits
        };

        assert_eq!(collect(false), 0);
        assert_eq!(collect(true), 1);
    }

    #[test]
    fn directories_are_reported_with_their_flag() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("projects")).unwrap();

        let receiver = spawn(tmp.path().to_path_buf(), "projects".to_string(), false);
        let mut hits = Vec::new();
        while let Ok(message) = receiver.recv() {
            hits.extend(message.hits);
            if message.done {
                break;
            }
        }
        assert_eq!(hits.len(), 1);
        assert!(hits[0].is_dir);
    }
}
