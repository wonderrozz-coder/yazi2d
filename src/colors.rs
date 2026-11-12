//! File-type colouring.
//!
//! Every entry is classified into a coarse group (folder, image, video, text,
//! archive, temporary, 3D model, audio, code, other) and drawn with that group's
//! colour. Three presets are built in; the choice is persisted next to the
//! favorites file so it survives restarts.

use std::fs;
use std::path::{Path, PathBuf};

use ratatui::style::Color;

/// Coarse file groups used for colouring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileGroup {
    Directory,
    Image,
    Video,
    Audio,
    Text,
    Archive,
    Temporary,
    Model,
    Code,
    Other,
}

impl FileGroup {
    /// Every group, in the order shown by the colour menu.
    pub const ALL: [FileGroup; 10] = [
        FileGroup::Directory,
        FileGroup::Image,
        FileGroup::Video,
        FileGroup::Audio,
        FileGroup::Text,
        FileGroup::Archive,
        FileGroup::Temporary,
        FileGroup::Model,
        FileGroup::Code,
        FileGroup::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FileGroup::Directory => "folders",
            FileGroup::Image => "images",
            FileGroup::Video => "video",
            FileGroup::Audio => "audio",
            FileGroup::Text => "text",
            FileGroup::Archive => "archives",
            FileGroup::Temporary => "temporary",
            FileGroup::Model => "3D models",
            FileGroup::Code => "code",
            FileGroup::Other => "other",
        }
    }

    /// Classify a path. Extension matching is case-insensitive.
    pub fn classify(path: &Path, is_dir: bool) -> FileGroup {
        if is_dir {
            return FileGroup::Directory;
        }

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        if is_temporary(name) {
            return FileGroup::Temporary;
        }

        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();

        match extension.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "svg"
            | "heic" | "raw" | "psd" => FileGroup::Image,
            "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "mpg" | "mpeg" | "wmv" => {
                FileGroup::Video
            }
            "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus" | "aiff" => FileGroup::Audio,
            "txt" | "md" | "markdown" | "rst" | "pdf" | "doc" | "docx" | "odt" | "rtf" | "epub"
            | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx" | "odp" => FileGroup::Text,
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "dmg" | "iso" => {
                FileGroup::Archive
            }
            "obj" | "fbx" | "stl" | "blend" | "gltf" | "glb" | "dae" | "3ds" | "max" | "c4d"
            | "usd" | "usdz" | "ply" | "step" | "stp" | "iges" | "igs" | "ma" | "mb" => {
                FileGroup::Model
            }
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "hpp" | "go"
            | "java" | "rb" | "sh" | "zsh" | "bash" | "toml" | "yaml" | "yml" | "json" | "html"
            | "css" | "sql" | "lua" | "php" | "swift" | "kt" => FileGroup::Code,
            _ => FileGroup::Other,
        }
    }
}

/// Temporary/editor droppings: `foo~`, `.DS_Store`, `~$report.docx`, `*.tmp`.
fn is_temporary(name: &str) -> bool {
    name.ends_with('~')
        || name.starts_with(".~")
        || name.starts_with("~$")
        || name == ".DS_Store"
        || name == "Thumbs.db"
        || name.ends_with(".tmp")
        || name.ends_with(".temp")
        || name.ends_with(".swp")
        || name.ends_with(".swo")
        || name.ends_with(".part")
        || name.ends_with(".crdownload")
}

/// Built-in palettes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Default,
    Vivid,
    Mono,
}

impl Preset {
    pub fn label(self) -> &'static str {
        match self {
            Preset::Default => "default",
            Preset::Vivid => "vivid",
            Preset::Mono => "mono",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Preset::Default),
            "vivid" => Some(Preset::Vivid),
            "mono" | "monochrome" => Some(Preset::Mono),
            _ => None,
        }
    }
}

/// Colour assignment for the current preset.
#[derive(Clone, Copy)]
pub struct Colors {
    preset: Preset,
}

impl Default for Colors {
    fn default() -> Self {
        Self::new()
    }
}

impl Colors {
    /// Load the persisted preset, falling back to the default one.
    pub fn new() -> Self {
        let preset = fs::read_to_string(theme_file())
            .ok()
            .and_then(|text| Preset::from_str(&text))
            .unwrap_or(Preset::Default);
        Self { preset }
    }

    pub fn preset(&self) -> Preset {
        self.preset
    }

    /// Switch preset and persist the choice; save errors are not fatal.
    pub fn set_theme(&mut self, preset: Preset) {
        self.preset = preset;
        let file = theme_file();
        if let Some(parent) = file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(file, format!("{}\n", preset.label()));
    }

    pub fn color(&self, group: FileGroup) -> Color {
        match self.preset {
            Preset::Default => match group {
                FileGroup::Directory => Color::LightBlue,
                FileGroup::Image => Color::LightMagenta,
                FileGroup::Video => Color::LightRed,
                FileGroup::Audio => Color::LightCyan,
                FileGroup::Text => Color::LightGreen,
                FileGroup::Archive => Color::LightYellow,
                FileGroup::Temporary => Color::DarkGray,
                FileGroup::Model => Color::Yellow,
                FileGroup::Code => Color::Cyan,
                FileGroup::Other => Color::Gray,
            },
            Preset::Vivid => match group {
                FileGroup::Directory => Color::Rgb(90, 170, 255),
                FileGroup::Image => Color::Rgb(255, 130, 220),
                FileGroup::Video => Color::Rgb(255, 110, 110),
                FileGroup::Audio => Color::Rgb(120, 230, 255),
                FileGroup::Text => Color::Rgb(140, 240, 150),
                FileGroup::Archive => Color::Rgb(255, 205, 90),
                FileGroup::Temporary => Color::Rgb(120, 120, 130),
                FileGroup::Model => Color::Rgb(255, 170, 60),
                FileGroup::Code => Color::Rgb(120, 200, 255),
                FileGroup::Other => Color::Rgb(200, 200, 205),
            },
            Preset::Mono => match group {
                FileGroup::Directory => Color::White,
                FileGroup::Temporary => Color::DarkGray,
                FileGroup::Other => Color::Gray,
                _ => Color::White,
            },
        }
    }
}

fn theme_file() -> PathBuf {
    crate::favorites::config_dir().join("theme")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_extensions() {
        assert_eq!(
            FileGroup::classify(Path::new("shot.PNG"), false),
            FileGroup::Image
        );
        assert_eq!(
            FileGroup::classify(Path::new("clip.mp4"), false),
            FileGroup::Video
        );
        assert_eq!(
            FileGroup::classify(Path::new("model.blend"), false),
            FileGroup::Model
        );
        assert_eq!(
            FileGroup::classify(Path::new("backup.zip"), false),
            FileGroup::Archive
        );
        assert_eq!(
            FileGroup::classify(Path::new("main.rs"), false),
            FileGroup::Code
        );
        assert_eq!(
            FileGroup::classify(Path::new("notes.md"), false),
            FileGroup::Text
        );
        assert_eq!(
            FileGroup::classify(Path::new("folder"), true),
            FileGroup::Directory
        );
    }

    #[test]
    fn detects_temporary_files() {
        assert_eq!(
            FileGroup::classify(Path::new("report.docx~"), false),
            FileGroup::Temporary
        );
        assert_eq!(
            FileGroup::classify(Path::new(".DS_Store"), false),
            FileGroup::Temporary
        );
        assert_eq!(
            FileGroup::classify(Path::new("~$budget.xlsx"), false),
            FileGroup::Temporary
        );
        assert_eq!(
            FileGroup::classify(Path::new("download.part"), false),
            FileGroup::Temporary
        );
    }

    #[test]
    fn presets_have_distinct_colours_for_key_groups() {
        let colors = Colors {
            preset: Preset::Default,
        };
        assert_ne!(
            colors.color(FileGroup::Directory),
            colors.color(FileGroup::Image)
        );
    }
}
