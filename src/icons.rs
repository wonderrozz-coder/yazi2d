//! Large block icons for the thumbnail grid.
//!
//! Terminals cannot scale fonts, so each icon is a small piece of block art
//! (six rows, nine columns) drawn in the colour of its file group. This is what
//! turns a non-previewable file from a "black hole" into something recognisable.

use crate::colors::FileGroup;

/// Every icon is this many cells wide.
pub const ICON_WIDTH: usize = 9;
/// Every icon is this many cells tall.
pub const ICON_HEIGHT: usize = 6;

/// Block art for one group.
pub fn icon(group: FileGroup) -> &'static [&'static str; ICON_HEIGHT] {
    match group {
        FileGroup::Directory => &FOLDER,
        FileGroup::Image => &IMAGE,
        FileGroup::Video => &VIDEO,
        FileGroup::Audio => &AUDIO,
        FileGroup::Text => &TEXT,
        FileGroup::Archive => &ARCHIVE,
        FileGroup::Temporary => &TEMPORARY,
        FileGroup::Model => &MODEL,
        FileGroup::Code => &CODE,
        FileGroup::Other => &OTHER,
    }
}

/// A single character fallback when the tile is smaller than the full icon.
pub fn glyph(group: FileGroup) -> char {
    match group {
        FileGroup::Directory => '▸',
        FileGroup::Image => '▣',
        FileGroup::Video => '▶',
        FileGroup::Audio => '♪',
        FileGroup::Text => '≡',
        FileGroup::Archive => '▤',
        FileGroup::Temporary => '·',
        FileGroup::Model => '◈',
        FileGroup::Code => '‹',
        FileGroup::Other => '▪',
    }
}

const FOLDER: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│█▄     │",
    "│██████ │",
    "│██████ │",
    "│██████ │",
    "└───────┘",
];

const IMAGE: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│ ▄▄▄▄▄ │",
    "│ █   █ │",
    "│ █ ▄ █ │",
    "│ ▀▀▀▀▀ │",
    "└───────┘",
];

const VIDEO: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│   █   │",
    "│   ██  │",
    "│   ███ │",
    "│   ██  │",
    "└── █───┘",
];

const AUDIO: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│    ██ │",
    "│    ██ │",
    "│    ██ │",
    "│  ████ │",
    "└─████──┘",
];

const TEXT: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│▒▒▒▒▒▒▒│",
    "│▒▒▒▒▒▒▒│",
    "│▒▒▒▒▒  │",
    "│▒▒▒▒▒▒▒│",
    "└───────┘",
];

const ARCHIVE: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│   █   │",
    "│  █ █  │",
    "│   █   │",
    "│  █ █  │",
    "└───────┘",
];

const TEMPORARY: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│       │",
    "│  ░░░  │",
    "│  ░░░  │",
    "│       │",
    "└───────┘",
];

const MODEL: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│  ▄▄▄  │",
    "│ █▀▀▀█ │",
    "│ █   █ │",
    "│ ▀▄▄▄▀ │",
    "└───────┘",
];

const CODE: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│  ███  │",
    "│ █   █ │",
    "│ █   █ │",
    "│  ███  │",
    "└───────┘",
];

const OTHER: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│       │",
    "│   ▪   │",
    "│       │",
    "│       │",
    "└───────┘",
];

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn every_icon_has_the_same_shape() {
        for group in FileGroup::ALL {
            let art = icon(group);
            assert_eq!(art.len(), ICON_HEIGHT, "{group:?} has the wrong height");
            for line in art {
                assert_eq!(
                    line.width(),
                    ICON_WIDTH,
                    "{group:?} line has the wrong width: {line:?}"
                );
            }
        }
    }

    #[test]
    fn icons_are_distinct_per_group() {
        let folder = icon(FileGroup::Directory);
        let archive = icon(FileGroup::Archive);
        assert_ne!(folder, archive, "groups must not share artwork");
    }
}
