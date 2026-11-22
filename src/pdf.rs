//! PDF previews, rendered the way Yazi does it.
//!
//! There is no PDF rasteriser in the Rust ecosystem that is both tiny and fast
//! enough for an interactive preview, so Yazi shells out to `pdftoppm` (Poppler)
//! and pipes the result back into its normal image pipeline. There is no reason
//! to do it differently: the terminal already knows how to draw a JPEG, so a
//! PDF page becomes a cached JPEG and every other code path stays untouched.
//!
//! The rendered page lives in the user's cache directory, keyed by the file's
//! identity (path, size, mtime) and the page number, so scrolling back and
//! forth over a document costs one `pdftoppm` call per page, not per frame.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

/// Longest edge of a rendered page, in pixels.
///
/// A page is drawn into a pane a few hundred cells wide, so more than this is
/// thrown away by the terminal's own downscaling while making the JPEG bigger
/// and slower to decode.
const PAGE_LONG_EDGE: u32 = 1600;

/// True when `pdftoppm` is on `PATH`, so callers can fall back to a text
/// message instead of showing a broken preview.
pub fn available() -> bool {
    which("pdftoppm").is_some()
}

/// Path to a cached JPEG of `page` (1-based) of the PDF at `path`.
///
/// Renders the page on first use. Returns `None` when Poppler is missing, the
/// page is out of range, or the conversion failed.
pub fn page_image(path: &Path, page: usize) -> Option<PathBuf> {
    if page == 0 {
        return None;
    }
    // Fast path: a cached page needs no subprocess at all. This runs on every
    // frame the preview is drawn, so it must stay allocation-only.
    let cached = cache_file(path, page)?;
    if cached.is_file() {
        return Some(cached);
    }

    // Clamp before rendering rather than trusting every caller: `pdftoppm -f 99`
    // on a two-page document exits successfully while writing nothing, which
    // would turn a harmless "page past the end" into a blank preview.
    let page = match page_count(path) {
        Some(total) if total > 0 => page.min(total),
        _ => page,
    };
    let cache = cache_file(path, page)?;
    if cache.is_file() {
        return Some(cache);
    }

    let cache_root = cache.parent()?;
    fs::create_dir_all(cache_root).ok()?;

    let stem = format!("{}", cache.file_stem()?.to_string_lossy());
    let output = cache_root.join(&stem);

    let pdftoppm = which("pdftoppm")?;
    let status = Command::new(&pdftoppm)
        .arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg("-singlefile")
        .arg("-jpeg")
        .arg("-jpegopt")
        .arg("quality=85")
        // Fit the page's long edge, without ever upscaling a small page into a
        // huge JPEG.
        .arg("-scale-to")
        .arg(PAGE_LONG_EDGE.to_string())
        .arg(path)
        .arg(&output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;

    if !status.success() {
        return None;
    }

    // `pdftoppm` appends `.jpg` itself.
    let produced = output.with_extension("jpg");
    if produced.is_file() {
        // Always address the cache path we computed: on a rebuild of the same
        // document the two names agree, and a stale file would otherwise be
        // reused.
        if produced != cache {
            fs::rename(&produced, &cache).ok()?;
        }
        Some(cache)
    } else {
        None
    }
}

/// Number of pages in a PDF, or `None` when it cannot be determined.
///
/// Spawning `pdfinfo` costs ~10 ms, and the preview asks for the count on
/// every frame it draws, so answers are memoised per path.
pub fn page_count(path: &Path) -> Option<usize> {
    static COUNTS: OnceLock<Mutex<HashMap<PathBuf, Option<usize>>>> = OnceLock::new();
    let counts = COUNTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = counts.lock().ok()?.get(path).copied() {
        return cached;
    }

    let pdfinfo = which("pdfinfo")?;
    let output = Command::new(&pdfinfo)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let pages = if !output.status.success() {
        None
    } else {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("Pages:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
    };

    if let Ok(mut guard) = counts.lock() {
        guard.insert(path.to_path_buf(), pages);
    }
    pages
}

/// Cache location for one rendered page.
///
/// The name includes the file's size and mtime so editing a document does not
/// serve a picture of its old content.
fn cache_file(path: &Path, page: usize) -> Option<PathBuf> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_secs())
        .unwrap_or(0);
    let stamp = format!("{:x}", fnv1a(path.to_string_lossy().as_bytes()));
    Some(
        cache_root()
            .join("pdf")
            .join(format!("{stamp}-{:x}-{mtime}-p{page}.jpg", meta.len())),
    )
}

fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("YAZI2D_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("yazi2d")
}

/// FNV-1a, so cache names are stable across runs and do not need a hasher
/// dependency. Chinese or emoji paths hash fine because this works on bytes.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Directories worth checking even when they are not on `PATH`.
///
/// A `.app` launched from Finder (or from our own `open -na Ghostty.app`)
/// inherits launchd's minimal environment, whose `PATH` is
/// `/usr/gnu/bin:/usr/local/bin:/bin:/usr/bin:.` — Homebrew's prefix is absent,
/// so a perfectly installed Poppler looked missing and the preview wrongly
/// claimed it was not there. Searching the well-known prefixes fixes that
/// without requiring the user to edit anything.
const EXTRA_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",          // Homebrew on Apple Silicon
    "/usr/local/bin",             // Homebrew on Intel, manual builds
    "/opt/local/bin",             // MacPorts
    "/run/current-system/sw/bin", // NixOS
    "/nix/var/nix/profiles/default/bin",
];

fn which(program: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path));
    }
    candidates.extend(EXTRA_BIN_DIRS.iter().map(PathBuf::from));

    candidates
        .into_iter()
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// True when the extension is one Poppler can rasterise.
pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_pdf_extensions() {
        assert!(is_pdf_path(Path::new("/tmp/a.pdf")));
        assert!(is_pdf_path(Path::new("/tmp/a.PDF")));
        assert!(!is_pdf_path(Path::new("/tmp/a.png")));
        assert!(!is_pdf_path(Path::new("/tmp/pdf")));
    }

    /// A `.app` started from Finder inherits launchd's minimal `PATH`, which has
    /// no `/opt/homebrew/bin` — the reported "PDF preview needs pdftoppm" error
    /// on a machine where Poppler *was* installed. The search must therefore
    /// consult the usual Homebrew/MacPorts prefixes regardless of `PATH`.
    #[test]
    fn finds_a_tool_that_is_missing_from_path() {
        let _env = crate::testutil::env_lock();
        // An empty PATH: nothing can be found through it.
        std::env::set_var("PATH", "");
        let found = which("pdftoppm");
        std::env::remove_var("PATH");

        // Either Poppler is installed in a known prefix, or it is not installed
        // at all — what must never happen is a "missing from PATH" failure when
        // the binary does exist in one of those prefixes.
        let in_known_dir = EXTRA_BIN_DIRS
            .iter()
            .any(|dir| Path::new(dir).join("pdftoppm").is_file());
        assert_eq!(
            found.is_some(),
            in_known_dir,
            "search ignored the known prefixes"
        );
        if let Some(path) = found {
            assert!(path.is_absolute(), "returned a relative path");
        }
    }

    #[test]
    fn cache_names_differ_per_page_and_change_with_the_file() {
        let dir = std::env::temp_dir().join(format!("yazi2d-pdf-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("doc.pdf");
        fs::write(&file, b"%PDF-1.4 test").unwrap();

        let p1 = cache_file(&file, 1).unwrap();
        let p2 = cache_file(&file, 2).unwrap();
        assert_ne!(p1, p2, "pages must not share a cache file");

        fs::write(&file, b"%PDF-1.4 edited, longer body").unwrap();
        let edited = cache_file(&file, 1).unwrap();
        assert_ne!(p1, edited, "an edited file must not reuse the old page");

        let _ = fs::remove_dir_all(&dir);
    }

    /// A real render, skipped when Poppler is not installed so the suite stays
    /// runnable on a bare machine.
    #[test]
    fn renders_distinct_pages_and_clamps_past_the_end() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("yazi2d-pdf-render-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("two-pages.pdf");
        fs::write(&pdf, TWO_PAGE_PDF).unwrap();

        assert_eq!(page_count(&pdf), Some(2));

        let first = page_image(&pdf, 1).expect("page 1 renders");
        let second = page_image(&pdf, 2).expect("page 2 renders");
        assert_ne!(
            fs::read(&first).unwrap(),
            fs::read(&second).unwrap(),
            "pages must rasterise differently"
        );

        // Asking past the end yields the last real page, not a blank preview.
        let past = page_image(&pdf, 99).expect("clamped");
        assert_eq!(past, second, "page 99 must clamp to page 2");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Minimal two-page PDF: one page paints a black square, the other a white
    /// one, so the two renders differ byte for byte.
    const TWO_PAGE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R 5 0 R]/Count 2>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]/Contents 4 0 R>>endobj\n\
4 0 obj<</Length 31>>stream\n0 0 0 rg 20 20 160 160 re f\nendstream endobj\n\
5 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]/Contents 6 0 R>>endobj\n\
6 0 obj<</Length 39>>stream\n1 1 1 rg 20 20 160 160 re f 0 0 0 rg\nendstream endobj\n\
trailer<</Root 1 0 R/Size 7>>\n%%EOF\n";
}
