use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use image::{DynamicImage, RgbImage};

use crate::logging;

/// How much of a text file we are willing to read for a preview.
pub const TEXT_LIMIT: usize = 512 * 1024;
/// Upper bound on rendered text lines, to keep scrolling cheap.
pub const MAX_TEXT_LINES: usize = 5_000;
/// Upper bound on directory children kept for a preview.
pub const MAX_DIR_CHILDREN: usize = 64;
/// Do not try to decode absurdly large images on the UI thread.
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Pre-scale large sources before the final (expensive) Lanczos3 pass.
pub const MAX_SOURCE_EDGE: u32 = 1600;

#[derive(Clone, Debug)]
pub struct DirChild {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_image: bool,
}

pub enum PreviewContent {
    Text {
        lines: Vec<String>,
        truncated: bool,
        size: u64,
    },
    Binary {
        size: u64,
        sample: String,
    },
    Directory {
        children: Vec<DirChild>,
        total: usize,
    },
    Error(String),
}

/// Result of a background preview load.
pub struct PreviewLoaded {
    pub path: PathBuf,
    pub content: PreviewContent,
}

/// Caches previews and loads them off the UI thread.
///
/// Directory listings and file reads used to run inside `ui::draw`, which froze
/// the whole TUI on slow shares: a `stat` on a Nextcloud placeholder can block
/// for hundreds of milliseconds. Now every miss returns `None` immediately,
/// starts one worker, and the next frame shows the cached result.
pub struct PreviewCache {
    entries: HashMap<PathBuf, PreviewContent>,
    pending: Option<(PathBuf, bool, Receiver<PreviewLoaded>)>,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            pending: None,
        }
    }

    /// Return cached content, starting a background load on a miss.
    ///
    /// Never blocks: the caller renders a "loading" placeholder for one or two
    /// frames and then gets the content.
    pub fn get(&mut self, path: &Path, show_hidden: bool) -> Option<&PreviewContent> {
        self.poll();

        let pending_matches = self
            .pending
            .as_ref()
            .map(|(pending, hidden, _)| pending == path && *hidden == show_hidden)
            .unwrap_or(false);

        if let Some(content) = self.entries.get(path) {
            return Some(content);
        }
        if pending_matches {
            return None;
        }

        let (sender, receiver) = mpsc::channel();
        let thread_path = path.to_path_buf();
        thread::spawn(move || {
            let content = if thread_path.is_dir() {
                load_directory(&thread_path, show_hidden)
            } else {
                load_file(&thread_path)
            };
            let _ = sender.send(PreviewLoaded {
                path: thread_path,
                content,
            });
        });
        self.pending = Some((path.to_path_buf(), show_hidden, receiver));
        None
    }

    /// True when a preview load is currently in flight.
    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    /// Forget every cached preview (used after renames/deletes).
    pub fn invalidate(&mut self) {
        self.entries.clear();
        self.pending = None;
    }

    fn poll(&mut self) {
        let received = match self.pending.as_ref() {
            Some((_, _, receiver)) => match receiver.try_recv() {
                Ok(loaded) => Some(Ok(loaded)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(())),
            },
            None => None,
        };

        match received {
            Some(Ok(loaded)) => {
                if self.entries.len() > 64 {
                    self.entries.clear();
                }
                logging::info(format!("preview ready: {}", loaded.path.display()));
                self.entries.insert(loaded.path, loaded.content);
                self.pending = None;
            }
            Some(Err(())) => self.pending = None,
            None => {}
        }
    }
}

fn load_file(path: &Path) -> PreviewContent {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) => return PreviewContent::Error(e.to_string()),
    };

    let size = meta.len();
    let mut bytes = Vec::new();
    match File::open(path) {
        Ok(file) => {
            if let Err(e) = file.take(TEXT_LIMIT as u64 + 1).read_to_end(&mut bytes) {
                return PreviewContent::Error(e.to_string());
            }
        }
        Err(e) => return PreviewContent::Error(e.to_string()),
    }

    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return PreviewContent::Binary {
            size,
            sample: hex_sample(&bytes),
        };
    }

    let truncated = bytes.len() > TEXT_LIMIT || size > TEXT_LIMIT as u64;
    bytes.truncate(TEXT_LIMIT);

    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text
        .lines()
        .take(MAX_TEXT_LINES)
        .map(|line| line.replace('\t', "    "))
        .collect();

    if lines.len() >= MAX_TEXT_LINES || truncated {
        lines.push("… (truncated)".to_string());
    }

    PreviewContent::Text {
        lines,
        truncated,
        size,
    }
}

fn load_directory(path: &Path, show_hidden: bool) -> PreviewContent {
    let started = Instant::now();
    let read_dir = match fs::read_dir(path) {
        Ok(read_dir) => read_dir,
        Err(e) => return PreviewContent::Error(e.to_string()),
    };

    let mut children = Vec::new();
    let mut total = 0;

    for item in read_dir.flatten() {
        let name = item.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        total += 1;
        if children.len() >= MAX_DIR_CHILDREN {
            continue;
        }

        // Use the directory-entry type only. `fs::metadata()` here would stat
        // every child, which is exactly what made previews hang on Nextcloud
        // and iCloud: each stat can materialise a cloud placeholder.
        let is_dir = item.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let child_path = item.path();
        let is_image = !is_dir && is_image_path(&child_path);

        children.push(DirChild {
            name,
            path: child_path,
            is_image,
            is_dir,
        });
    }

    children
        .sort_by_cached_key(|child| (if child.is_dir { 0 } else { 1 }, child.name.to_lowercase()));

    logging::info(format!(
        "directory preview in {} ms: {} ({} entries)",
        started.elapsed().as_millis(),
        path.display(),
        total
    ));

    PreviewContent::Directory { children, total }
}

fn hex_sample(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(48)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn is_image_path(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico"
    )
}

struct ImageEntry {
    source: Option<DynamicImage>,
    resized: HashMap<(u32, u32), Arc<RgbImage>>,
}

/// Outcome of a background decode.
struct DecodedImage {
    path: PathBuf,
    source: Option<DynamicImage>,
}

/// Decodes images and caches half-block-sized RGB thumbnails.
///
/// Decoding big TIFFs (tens of megabytes) on the UI thread stalled the whole
/// TUI. Misses now return `None` and decode on a worker thread; the final
/// resize to the requested cell size stays cached and cheap.
pub struct ImageCache {
    entries: HashMap<PathBuf, ImageEntry>,
    /// Decodes in flight, ordered oldest first. A small cap keeps a fast scroll
    /// from spawning a thread per cell while still filling a grid progressively.
    pending: Vec<(PathBuf, Receiver<DecodedImage>)>,
    /// Paths that failed to decode, so we do not retry every frame.
    failed: HashSet<PathBuf>,
}

/// How many image decodes may run at once.
const MAX_PENDING_DECODES: usize = 4;

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            pending: Vec::new(),
            failed: HashSet::new(),
        }
    }

    /// Return an image fitted into `max_width` cells by `max_height` cells.
    ///
    /// The returned image has `2 * height` pixel rows (two pixels per terminal
    /// cell). The caller centers it and draws it with half blocks. Returns
    /// `None` while the background decode is still running.
    pub fn render(
        &mut self,
        path: &Path,
        max_width: u16,
        max_height: u16,
    ) -> Option<Arc<RgbImage>> {
        if max_width == 0 || max_height == 0 {
            return None;
        }

        self.poll();

        if !self.entries.contains_key(path) {
            if self.failed.contains(path) {
                return None;
            }
            let pending_matches = self.pending.iter().any(|(pending, _)| pending == path);
            if !pending_matches && self.pending.len() < MAX_PENDING_DECODES {
                self.spawn_decode(path);
            }
            return None;
        }

        let entry = self.entries.get_mut(path)?;
        let source = entry.source.as_ref()?;
        let (width, height) = fit_dimensions(
            source.width(),
            source.height(),
            max_width as u32,
            max_height as u32 * 2,
        );
        let key = (width, height);

        entry.resized.entry(key).or_insert_with(|| {
            let rgba = source.to_rgba8();
            let resized = image::imageops::resize(
                &rgba,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
            );
            Arc::new(composite_on_dark(&resized))
        });

        entry.resized.get(&key).cloned()
    }

    /// True when at least one decode is in flight.
    pub fn is_loading(&self) -> bool {
        !self.pending.is_empty()
    }

    fn spawn_decode(&mut self, path: &Path) {
        let (sender, receiver) = mpsc::channel();
        let thread_path = path.to_path_buf();
        thread::spawn(move || {
            let started = Instant::now();
            logging::info(format!("image decode start: {}", thread_path.display()));
            let source = image::ImageReader::open(&thread_path)
                .ok()
                .and_then(|reader| reader.with_guessed_format().ok())
                .and_then(|reader| reader.decode().ok())
                .filter(|image| {
                    let pixels = u64::from(image.width()) * u64::from(image.height());
                    pixels.saturating_mul(4) <= MAX_IMAGE_BYTES
                })
                .map(shrink_source);
            match &source {
                Some(image) => logging::info(format!(
                    "image decode done in {} ms: {} ({}×{})",
                    started.elapsed().as_millis(),
                    thread_path.display(),
                    image.width(),
                    image.height()
                )),
                None => logging::warn(format!(
                    "image decode failed in {} ms: {}",
                    started.elapsed().as_millis(),
                    thread_path.display()
                )),
            }
            let _ = sender.send(DecodedImage {
                path: thread_path,
                source,
            });
        });
        self.pending.push((path.to_path_buf(), receiver));
    }

    fn poll(&mut self) {
        let mut finished = Vec::new();
        let mut disconnected = Vec::new();

        for (index, (path, receiver)) in self.pending.iter().enumerate() {
            match receiver.try_recv() {
                Ok(decoded) => finished.push(decoded),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => disconnected.push((index, path.clone())),
            }
        }

        for (index, path) in disconnected.into_iter().rev() {
            self.pending.remove(index);
            self.failed.insert(path);
        }

        for decoded in finished {
            self.pending.retain(|(path, _)| path != &decoded.path);
            if self.entries.len() > 64 {
                self.entries.clear();
            }
            if decoded.source.is_none() {
                self.failed.insert(decoded.path);
            } else {
                self.failed.remove(&decoded.path);
                self.entries.insert(
                    decoded.path,
                    ImageEntry {
                        source: decoded.source,
                        resized: HashMap::new(),
                    },
                );
            }
        }
    }
}

/// Bound the decoded source so the final Lanczos3 pass stays cheap.
fn shrink_source(image: DynamicImage) -> DynamicImage {
    let (width, height) = (image.width(), image.height());
    let longest = width.max(height);
    if longest <= MAX_SOURCE_EDGE {
        return image;
    }

    let scale = MAX_SOURCE_EDGE as f64 / longest as f64;
    let target_width = ((width as f64 * scale).round() as u32).max(1);
    let target_height = ((height as f64 * scale).round() as u32).max(1);
    image.resize_exact(
        target_width,
        target_height,
        image::imageops::FilterType::Triangle,
    )
}

fn fit_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 || max_width == 0 || max_height == 0 {
        return (1, 1);
    }

    let scale_x = max_width as f64 / width as f64;
    let scale_y = max_height as f64 / height as f64;
    let scale = scale_x.min(scale_y);

    let fitted_width = ((width as f64 * scale).round() as u32).clamp(1, max_width);
    let fitted_height = ((height as f64 * scale).round() as u32).clamp(1, max_height);

    (fitted_width, fitted_height)
}

fn composite_on_dark(image: &image::RgbaImage) -> RgbImage {
    let mut output = RgbImage::new(image.width(), image.height());
    for (x, y, pixel) in image.enumerate_pixels() {
        let alpha = pixel[3] as f32 / 255.0;
        let blend = |channel: u8, background: u8| -> u8 {
            (channel as f32 * alpha + background as f32 * (1.0 - alpha)).round() as u8
        };
        output.put_pixel(
            x,
            y,
            image::Rgb([
                blend(pixel[0], 18),
                blend(pixel[1], 18),
                blend(pixel[2], 24),
            ]),
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    /// Poll the async preview cache until content arrives or the deadline hits.
    fn wait_for(cache: &mut PreviewCache, path: &Path, show_hidden: bool) -> PreviewContent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(content) = cache.get(path, show_hidden) {
                return match content {
                    PreviewContent::Text {
                        lines,
                        truncated,
                        size,
                    } => PreviewContent::Text {
                        lines: lines.clone(),
                        truncated: *truncated,
                        size: *size,
                    },
                    PreviewContent::Binary { size, sample } => PreviewContent::Binary {
                        size: *size,
                        sample: sample.clone(),
                    },
                    PreviewContent::Directory { children, total } => PreviewContent::Directory {
                        children: children.clone(),
                        total: *total,
                    },
                    PreviewContent::Error(error) => PreviewContent::Error(error.clone()),
                };
            }
            assert!(Instant::now() < deadline, "preview did not load in time");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn text_preview_expands_tabs_and_reports_size() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.txt");
        fs::write(&path, "hello\n\tworld\n").unwrap();

        let mut cache = PreviewCache::new();
        match wait_for(&mut cache, &path, false) {
            PreviewContent::Text { lines, size, .. } => {
                assert_eq!(lines, vec!["hello".to_string(), "    world".to_string()]);
                assert_eq!(size, 13);
            }
            _ => panic!("expected text preview"),
        }
    }

    #[test]
    fn binary_preview_detects_nul_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("data.bin");
        fs::write(&path, [0u8, 1, 2, 3, 255]).unwrap();

        let mut cache = PreviewCache::new();
        assert!(matches!(
            wait_for(&mut cache, &path, false),
            PreviewContent::Binary { .. }
        ));
    }

    #[test]
    fn directory_preview_sorts_directories_first_and_hides_dotfiles() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("beta")).unwrap();
        fs::write(tmp.path().join("alpha.txt"), "x").unwrap();
        fs::write(tmp.path().join(".secret"), "x").unwrap();

        let mut cache = PreviewCache::new();
        match wait_for(&mut cache, tmp.path(), false) {
            PreviewContent::Directory { children, total } => {
                assert_eq!(total, 2);
                assert_eq!(children[0].name, "beta");
                assert!(children[0].is_dir);
                assert_eq!(children[1].name, "alpha.txt");
            }
            _ => panic!("expected directory preview"),
        }
    }

    #[test]
    fn image_cache_is_async_and_fills_in() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("red.png");
        let image = RgbImage::from_pixel(4, 2, image::Rgb([255, 0, 0]));
        image.save(&path).unwrap();

        let mut cache = ImageCache::new();
        // First call starts the decode and must not block the UI.
        assert!(cache.render(&path, 10, 5).is_none());

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(rendered) = cache.render(&path, 10, 5) {
                assert!(rendered.width() <= 10);
                assert!(rendered.height() <= 10);
                assert!(rendered.width() > 0);
                break;
            }
            assert!(Instant::now() < deadline, "image decode did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn image_extensions_are_recognized_case_insensitively() {
        assert!(is_image_path(Path::new("photo.JPG")));
        assert!(is_image_path(Path::new("anim.webp")));
        assert!(!is_image_path(Path::new("notes.txt")));
        assert!(!is_image_path(Path::new("no-extension")));
    }
}
