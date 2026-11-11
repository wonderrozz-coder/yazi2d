use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use image::{DynamicImage, RgbImage};

/// How much of a text file we are willing to read for a preview.
pub const TEXT_LIMIT: usize = 512 * 1024;
/// Upper bound on rendered text lines, to keep scrolling cheap.
pub const MAX_TEXT_LINES: usize = 5_000;
/// Upper bound on directory children kept for a preview.
pub const MAX_DIR_CHILDREN: usize = 64;
/// Do not try to decode absurdly large images on the UI thread.
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DirChild {
    pub name: String,
    pub is_dir: bool,
    pub is_image: bool,
    pub size: u64,
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

struct PreviewEntry {
    mtime: Option<SystemTime>,
    len: u64,
    hidden: bool,
    content: PreviewContent,
}

/// Caches text and directory previews keyed by path, mtime and size.
pub struct PreviewCache {
    entries: HashMap<PathBuf, PreviewEntry>,
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
        }
    }

    pub fn get(&mut self, path: &Path, show_hidden: bool) -> &PreviewContent {
        let meta = fs::metadata(path).ok();
        let mtime = meta.as_ref().and_then(|m| m.modified().ok());
        let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);

        let stale = match self.entries.get(path) {
            Some(entry) => {
                entry.mtime != mtime
                    || entry.len != len
                    || (matches!(&entry.content, PreviewContent::Directory { .. })
                        && entry.hidden != show_hidden)
            }
            None => true,
        };

        if stale {
            let content = if path.is_dir() {
                load_directory(path, show_hidden)
            } else {
                load_file(path)
            };
            if self.entries.len() > 64 {
                self.entries.clear();
            }
            self.entries.insert(
                path.to_path_buf(),
                PreviewEntry {
                    mtime,
                    len,
                    hidden: show_hidden,
                    content,
                },
            );
        }

        &self.entries[path].content
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

        let child_path = item.path();
        let meta = fs::metadata(&child_path).ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

        children.push(DirChild {
            name,
            is_image: !is_dir && is_image_path(&child_path),
            is_dir,
            size,
        });
    }

    children
        .sort_by_cached_key(|child| (if child.is_dir { 0 } else { 1 }, child.name.to_lowercase()));

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
    mtime: Option<SystemTime>,
    len: u64,
    source: Option<DynamicImage>,
    resized: HashMap<(u32, u32), Arc<RgbImage>>,
}

/// Decodes images and caches half-block-sized RGB thumbnails.
pub struct ImageCache {
    entries: HashMap<PathBuf, ImageEntry>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Return an image fitted into `max_width` cells by `max_height` cells.
    ///
    /// The returned image has `2 * height` pixel rows (two pixels per terminal
    /// cell). The caller centers it and draws it with half blocks.
    pub fn render(
        &mut self,
        path: &Path,
        max_width: u16,
        max_height: u16,
    ) -> Option<Arc<RgbImage>> {
        if max_width == 0 || max_height == 0 {
            return None;
        }

        let meta = fs::metadata(path).ok()?;
        if meta.len() > MAX_IMAGE_BYTES {
            return None;
        }
        let mtime = meta.modified().ok();
        let len = meta.len();

        let stale = match self.entries.get(path) {
            Some(entry) => entry.mtime != mtime || entry.len != len,
            None => true,
        };

        if stale {
            let source = image::ImageReader::open(path)
                .ok()
                .and_then(|reader| reader.with_guessed_format().ok())
                .and_then(|reader| reader.decode().ok());

            if self.entries.len() > 64 {
                self.entries.clear();
            }
            self.entries.insert(
                path.to_path_buf(),
                ImageEntry {
                    mtime,
                    len,
                    source,
                    resized: HashMap::new(),
                },
            );
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
                image::imageops::FilterType::Triangle,
            );
            Arc::new(composite_on_dark(&resized))
        });

        entry.resized.get(&key).cloned()
    }
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

    #[test]
    fn text_preview_expands_tabs_and_reports_size() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.txt");
        fs::write(&path, "hello\n\tworld\n").unwrap();

        let mut cache = PreviewCache::new();
        match cache.get(&path, false) {
            PreviewContent::Text { lines, size, .. } => {
                assert_eq!(lines, &vec!["hello".to_string(), "    world".to_string()]);
                assert_eq!(*size, 13);
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
            cache.get(&path, false),
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
        match cache.get(tmp.path(), false) {
            PreviewContent::Directory { children, total } => {
                assert_eq!(*total, 2);
                assert_eq!(children[0].name, "beta");
                assert!(children[0].is_dir);
                assert_eq!(children[1].name, "alpha.txt");
            }
            _ => panic!("expected directory preview"),
        }
    }

    #[test]
    fn image_cache_decodes_and_fits() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("red.png");
        let image = RgbImage::from_pixel(4, 2, image::Rgb([255, 0, 0]));
        image.save(&path).unwrap();

        let mut cache = ImageCache::new();
        let rendered = cache.render(&path, 10, 5).expect("image should decode");
        assert!(rendered.width() <= 10);
        assert!(rendered.height() <= 10);
        assert!(rendered.width() > 0);
        assert!(rendered.height() > 0);
    }

    #[test]
    fn image_extensions_are_recognized_case_insensitively() {
        assert!(is_image_path(Path::new("photo.JPG")));
        assert!(is_image_path(Path::new("anim.webp")));
        assert!(!is_image_path(Path::new("notes.txt")));
        assert!(!is_image_path(Path::new("no-extension")));
    }
}
