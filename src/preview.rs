use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
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

    /// Collect a finished background load.
    ///
    /// Returns true when something arrived, so the caller repaints once instead
    /// of on every tick while the load is still running.
    pub fn poll_background(&mut self) -> bool {
        let was_pending = self.pending.is_some();
        let before = self.entries.len();
        self.poll();
        was_pending && (self.pending.is_none() || self.entries.len() != before)
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
                // Evict a single old entry rather than clearing the map: a
                // wipe re-decoded and re-encoded everything visible, which the
                // user saw as the preview blinking.
                if !self.entries.contains_key(&loaded.path)
                    && self.entries.len() >= MAX_CACHED_IMAGES
                {
                    if let Some(oldest) = self.entries.keys().next().cloned() {
                        self.entries.remove(&oldest);
                    }
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
    /// The picture's own pixel size, before the decoded copy was shrunk.
    dimensions: (u32, u32),
    resized: HashMap<(u32, u32), Arc<RgbImage>>,
    /// Encoded native-graphics payloads (kitty/iTerm2/sixel), keyed by the
    /// terminal area and the fitting mode. Encoding is expensive, so it is done
    /// once per size.
    protocols: HashMap<(u16, u16, bool), Rc<ratatui_image::protocol::Protocol>>,
    /// Logical clock value of the last use, for LRU eviction.
    used: u64,
}

/// Outcome of a background decode.
struct DecodedImage {
    path: PathBuf,
    source: Option<DynamicImage>,
    /// The picture's own pixel size, captured before `shrink_source` crops the
    /// decoded copy. The caption reports this, so it stays the file's size.
    dimensions: Option<(u32, u32)>,
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
    /// Monotonic tick used to age cache entries for LRU eviction.
    age: u64,
    /// The image the preview pane is currently showing.
    ///
    /// It must survive eviction: a pane-sized payload is megabytes and a
    /// neighbouring thumbnail grid otherwise pushes it out of the cache, so the
    /// preview re-decoded and re-encoded on every frame.
    pinned: Option<PathBuf>,
    /// Encodes in flight, keyed by their cache key. Encoding a pane-sized
    /// payload costs 50–80 ms; doing it inline blocked the whole UI on every new
    /// file, which is what made previews feel slow. Yazi encodes in a worker
    /// task too and simply shows the previous frame meanwhile.
    pending_encodes: Vec<PendingEncode>,
    /// `(path, key)` pairs currently being encoded, so the next frame does not
    /// start a duplicate job for the same picture and size.
    encoding: HashSet<(PathBuf, (u16, u16, bool))>,
}

/// How many image decodes may run at once.
const MAX_PENDING_DECODES: usize = 4;

/// How many decoded images are kept. Old ones are evicted one by one, never
/// all at once: wiping the map re-decoded and re-encoded every visible tile,
/// which showed up as the grid blinking.
const MAX_CACHED_IMAGES: usize = 96;

/// How many encoded payloads are kept per image (one per distinct size).
const MAX_PROTOCOL_SIZES: usize = 4;

/// An encode running on a worker thread: which image and size it is for, and
/// the channel that hands back the finished protocol.
type PendingEncode = (
    (PathBuf, (u16, u16, bool)),
    Receiver<Option<ratatui_image::protocol::Protocol>>,
);

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
            age: 0,
            pinned: None,
            pending_encodes: Vec::new(),
            encoding: HashSet::new(),
        }
    }

    /// Keep `path` out of LRU eviction while the preview pane shows it.
    pub fn pin(&mut self, path: &Path) {
        self.pinned = Some(path.to_path_buf());
        self.touch(path);
    }

    /// Advance the logical clock and stamp an entry as most recently used.
    fn touch(&mut self, path: &Path) {
        self.age = self.age.wrapping_add(1);
        let age = self.age;
        if let Some(entry) = self.entries.get_mut(path) {
            entry.used = age;
        }
    }

    /// Drop the least recently used entry, never the pinned preview.
    ///
    /// Scanning the whole map is fine: it holds at most a few dozen entries and
    /// only runs on a cache miss.
    fn evict_one(&mut self) {
        let victim = self
            .entries
            .iter()
            .filter(|(path, _)| Some(path.as_path()) != self.pinned.as_deref())
            .min_by_key(|(_, entry)| entry.used)
            .map(|(path, _)| path.clone());
        if let Some(victim) = victim {
            self.entries.remove(&victim);
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
        entry.used = self.age;
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

    /// Ask for a decode without waiting for it.
    ///
    /// The renderer calls this for every image it is about to show (a preview,
    /// a grid of thumbnails), so the worker starts one frame earlier and the
    /// `…` placeholder disappears sooner. A no-op when the image is cached, in
    /// flight, or already known to be undecodable.
    pub fn prefetch(&mut self, path: &Path) {
        if self.entries.contains_key(path) || self.failed.contains(path) {
            return;
        }
        if self.pending.iter().any(|(pending, _)| pending == path)
            || self.pending.len() >= MAX_PENDING_DECODES
        {
            return;
        }
        self.spawn_decode(path);
    }

    /// True when the image has been decoded and is ready to draw.
    ///
    /// The caller then chooses between a native graphics protocol and half
    /// blocks, which needs the original aspect ratio rather than the
    /// pre-resized half-block image.
    pub fn source_dimensions(&mut self, path: &Path) -> Option<(u32, u32)> {
        self.poll();
        // Start the decode on a miss, exactly like `render` and `protocol` do.
        // A PDF page goes through here first, so without this the pane stayed on
        // "Rendering page…" forever: nothing was ever queued, nothing landed,
        // and the frame was never repainted.
        if !self.entries.contains_key(path) {
            if self.failed.contains(path) {
                return None;
            }
            let already = self.pending.iter().any(|(pending, _)| pending == path);
            if !already && self.pending.len() < MAX_PENDING_DECODES {
                self.spawn_decode(path);
            }
            return None;
        }
        self.age = self.age.wrapping_add(1);
        let age = self.age;
        let entry = self.entries.get_mut(path)?;
        entry.used = age;
        entry.source.as_ref()?;
        Some(entry.dimensions)
    }

    /// Encoded native protocol for `area`, built once per terminal size.
    ///
    /// Returns `None` when the terminal has no native protocol, so the caller
    /// draws half blocks instead.
    pub fn protocol(
        &mut self,
        path: &Path,
        area: ratatui::layout::Rect,
        fit: crate::graphics::Fit,
    ) -> Option<Rc<ratatui_image::protocol::Protocol>> {
        self.poll();
        if !crate::graphics::native() || area.width == 0 || area.height == 0 {
            return None;
        }

        if !self.entries.contains_key(path) {
            if self.failed.contains(path) {
                return None;
            }
            if !self.pending.iter().any(|(pending, _)| pending == path)
                && self.pending.len() < MAX_PENDING_DECODES
            {
                self.spawn_decode(path);
            }
            return None;
        }

        let key = (area.width, area.height, fit == crate::graphics::Fit::Fill);
        self.age = self.age.wrapping_add(1);
        let age = self.age;
        let entry = self.entries.get_mut(path)?;
        entry.used = age;
        // Check the cache before touching the bitmap: cloning a multi-megapixel
        // `DynamicImage` per frame and per tile was the main reason previews
        // felt slow.
        if let Some(protocol) = entry.protocols.get(&key) {
            return Some(protocol.clone());
        }
        let source = entry.source.clone()?;

        // Encoding a pane-sized payload takes 50–80 ms. It used to run right
        // here, on the UI thread, which is why scrolling through photos felt
        // stuck. Start a worker instead and let the caller draw the previous
        // frame; the next `poll_background` picks the result up.
        let job = (path.to_path_buf(), key);
        if self.encoding.contains(&job) {
            return None;
        }
        let encoder = crate::graphics::encoder(fit)?;
        self.encoding.insert(job);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let started = Instant::now();
            let protocol = encoder.encode(source, area, fit);
            let elapsed = started.elapsed();
            if elapsed.as_millis() >= 50 {
                logging::info(format!(
                    "image encode took {} ms: {} ({}×{} cells, {}, background)",
                    elapsed.as_millis(),
                    "",
                    area.width,
                    area.height,
                    if fit == crate::graphics::Fit::Fill {
                        "fill"
                    } else {
                        "inside"
                    }
                ));
            }
            let _ = sender.send(protocol);
        });
        self.pending_encodes
            .push(((path.to_path_buf(), key), receiver));
        None
    }

    /// Store a finished encode, honouring the per-image payload cap.
    fn store_protocol(
        &mut self,
        path: &Path,
        key: (u16, u16, bool),
        protocol: Rc<ratatui_image::protocol::Protocol>,
    ) {
        let Some(entry) = self.entries.get_mut(path) else {
            return;
        };
        // A fill payload covers the whole pane at real pixel size, which is
        // megabytes, so a preview keeps a couple of sizes at most while tiles
        // (a few hundred bytes each) may keep more.
        let limit = if key.2 { 1 } else { MAX_PROTOCOL_SIZES };
        if entry.protocols.len() >= limit {
            entry.protocols.clear();
        }
        entry.protocols.insert(key, protocol);
        if key.2 {
            // Only the image on screen is worth a full-pane payload: scrolling
            // through previews must not accumulate one per file.
            for (other_path, other) in self.entries.iter_mut() {
                if other_path != path {
                    other.protocols.retain(|size, _| !size.2);
                }
            }
        }
    }

    /// Drop the encoded payloads of every image, keeping the decodes.
    ///
    /// Called when the terminal's cell size changed: a payload is a function of
    /// the pane size *and* the cell size, and re-encoding is much cheaper than
    /// decoding a photo again.
    pub fn invalidate_protocols(&mut self) {
        for entry in self.entries.values_mut() {
            entry.protocols.clear();
        }
    }

    /// True when at least one decode or encode is in flight.
    pub fn is_loading(&self) -> bool {
        !self.pending.is_empty() || !self.pending_encodes.is_empty()
    }

    fn spawn_decode(&mut self, path: &Path) {
        let (sender, receiver) = mpsc::channel();
        let thread_path = path.to_path_buf();
        thread::spawn(move || {
            let started = Instant::now();
            logging::info(format!("image decode start: {}", thread_path.display()));
            let decoded = image::ImageReader::open(&thread_path)
                .ok()
                .and_then(|reader| reader.with_guessed_format().ok())
                .and_then(|reader| reader.decode().ok())
                .filter(|image| {
                    let pixels = u64::from(image.width()) * u64::from(image.height());
                    pixels.saturating_mul(4) <= MAX_IMAGE_BYTES
                });
            // Capture the file's own size before shrinking: the caption must
            // report the picture, not the copy this cache keeps.
            let dimensions = decoded
                .as_ref()
                .map(|image| (image.width(), image.height()));
            let source = decoded.map(shrink_source);
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
                dimensions,
            });
        });
        self.pending.push((path.to_path_buf(), receiver));
    }

    /// Collect finished background decodes.
    ///
    /// Returns true when at least one entry changed, so the caller can repaint
    /// exactly once per finished decode. Repainting every tick while something
    /// was still decoding made the grid blink.
    pub fn poll_background(&mut self) -> bool {
        let before = self.entries.len();
        let had_pending = !self.pending.is_empty();
        let had_encodes = !self.pending_encodes.is_empty();
        self.poll();
        self.poll_encodes();
        self.entries.len() != before
            || (had_pending && self.pending.is_empty())
            || (had_encodes && self.pending_encodes.is_empty())
    }

    /// Collect finished background encodes.
    fn poll_encodes(&mut self) {
        let mut done = Vec::new();
        let mut disconnected = Vec::new();
        for (index, (job, receiver)) in self.pending_encodes.iter().enumerate() {
            match receiver.try_recv() {
                Ok(protocol) => done.push((job.clone(), protocol)),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => disconnected.push(index),
            }
        }
        for index in disconnected.into_iter().rev() {
            let (job, _) = self.pending_encodes.remove(index);
            self.encoding.remove(&job);
        }
        for (job, protocol) in done {
            let (path, key) = job;
            self.encoding.remove(&(path.clone(), key));
            if let Some(protocol) = protocol {
                self.store_protocol(&path, key, Rc::new(protocol));
            }
        }
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
            if decoded.source.is_none() {
                self.failed.insert(decoded.path);
            } else {
                self.failed.remove(&decoded.path);
                // Evict the least recently used entry instead of clearing the
                // whole map: the grid keeps its already-encoded tiles, so it
                // does not blink.
                if !self.entries.contains_key(&decoded.path)
                    && self.entries.len() >= MAX_CACHED_IMAGES
                {
                    self.evict_one();
                }
                self.age = self.age.wrapping_add(1);
                self.entries.insert(
                    decoded.path,
                    ImageEntry {
                        source: decoded.source,
                        dimensions: decoded.dimensions.unwrap_or((0, 0)),
                        resized: HashMap::new(),
                        protocols: HashMap::new(),
                        used: self.age,
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

    /// `source_dimensions` is the first call a PDF page makes, and it used to
    /// only read the cache. Nothing was queued, so "Rendering page…" never went
    /// away. It must start the decode itself, like `render` and `protocol`.
    #[test]
    fn asking_for_dimensions_starts_a_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.jpg");
        // A real 2×2 PNG so the decode succeeds on the worker thread.
        let mut png = std::fs::File::create(&path).unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        drop(png);

        let mut cache = ImageCache::new();
        assert!(
            cache.source_dimensions(&path).is_none(),
            "the first call must miss and queue a decode"
        );
        assert!(
            cache.is_loading() || !cache.pending.is_empty(),
            "no decode was started, so the pane would wait forever"
        );
    }

    fn image_entry(used: u64) -> ImageEntry {
        ImageEntry {
            source: None,
            dimensions: (0, 0),
            resized: HashMap::new(),
            protocols: HashMap::new(),
            used,
        }
    }

    /// The cache must drop the *least* recently used entry, never whatever
    /// `HashMap` happens to yield first. With the old behaviour a folder of
    /// photos re-decoded the visible file hundreds of times.
    #[test]
    fn eviction_drops_the_least_recently_used_entry() {
        let mut cache = ImageCache::new();
        let paths: Vec<PathBuf> = (0..3)
            .map(|i| PathBuf::from(format!("/tmp/img-{i}.png")))
            .collect();
        // Distinct, increasing stamps: the clock must be past all of them so a
        // touch really moves an entry to the front.
        for (i, path) in paths.iter().enumerate() {
            cache
                .entries
                .insert(path.clone(), image_entry(10 * (i as u64 + 1)));
        }
        cache.age = 100;

        // Touch the oldest entry: it is now the newest.
        cache.touch(&paths[0]);
        cache.evict_one();

        assert!(
            cache.entries.contains_key(&paths[0]),
            "a just-used entry must survive eviction"
        );
        assert!(
            !cache.entries.contains_key(&paths[1]),
            "the least recently used entry is the one dropped"
        );
    }

    /// The preview pane's file is pinned, so a burst of grid decodes cannot
    /// evict the picture the user is actually looking at.
    #[test]
    fn a_pinned_entry_is_never_evicted() {
        let mut cache = ImageCache::new();
        let pinned = PathBuf::from("/tmp/pinned.png");
        let other = PathBuf::from("/tmp/other.png");
        cache.entries.insert(pinned.clone(), image_entry(0));
        cache.entries.insert(other.clone(), image_entry(5));
        cache.pin(&pinned);

        for _ in 0..10 {
            cache.evict_one();
        }

        assert!(cache.entries.contains_key(&pinned));
        assert!(!cache.entries.contains_key(&other));
    }
}

#[cfg(test)]
mod bench_tmp {
    #[test]
    fn measure_encode_cost() {
        use image::{DynamicImage, RgbImage};
        use ratatui::layout::Rect;
        use std::time::Instant;

        fn photo(w: u32, h: u32) -> DynamicImage {
            let mut img = RgbImage::new(w, h);
            for (x, y, p) in img.enumerate_pixels_mut() {
                *p = image::Rgb([(x % 251) as u8, (y % 253) as u8, ((x + y) % 247) as u8]);
            }
            DynamicImage::ImageRgb8(img)
        }

        let img = photo(1872, 1036);
        let area = Rect::new(0, 0, 74, 46);
        for (label, fit) in [
            ("Fill", crate::graphics::Fit::Fill),
            ("Inside", crate::graphics::Fit::Inside),
        ] {
            let started = Instant::now();
            let proto = crate::graphics::render(img.clone(), area, fit);
            let elapsed = started.elapsed();
            match proto {
                Some(proto) => println!(
                    "{label}: {:?}, drawn area {:?}, whole pane {:?}",
                    elapsed,
                    proto.area(),
                    area
                ),
                None => println!("{label}: none"),
            }
        }
    }
}
