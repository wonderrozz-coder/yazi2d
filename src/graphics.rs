//! Terminal image protocols.
//!
//! Half blocks (`▀` with a foreground and a background colour) work in every
//! terminal, but their resolution is one cell per two pixels: a photo in an
//! 80-cell preview is 80 pixels wide, which is why it looked blocky.
//!
//! Native graphics protocols solve that. The terminal draws real pixels, so the
//! same preview carries 800–1000 pixels across and stays sharp:
//!
//! * **Kitty** — Ghostty, kitty, WezTerm, foot, Konsole;
//! * **iTerm2** — iTerm2, WezTerm, Konsole;
//! * **Sixel** — foot, WezTerm, mlterm, xterm with `-ti vt340`.
//!
//! `ratatui-image` speaks all of them; this module decides which one to use and
//! keeps that decision in one place.
//!
//! ## Why the probe does not use `Picker::from_query_stdio`
//!
//! That helper enables raw mode, writes the queries, reads replies and restores
//! the previous terminal state — and when the terminal does not answer (a
//! multiplexer that hides the sequences, a terminal without those protocols, a
//! pty in a test) it gives up after a timeout and leaves a worker thread blocked
//! in `read()`. That thread keeps stealing keystrokes from the event loop, which
//! in practice froze the whole app. The probe below only inspects environment
//! variables and a terminfo query, so it never touches stdin.

use std::cell::RefCell;

use image::DynamicImage;
use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;

/// Terminal we are drawing into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    /// Ghostty, kitty, WezTerm, foot — 24-bit colour half blocks look right.
    TrueColor,
    /// macOS Terminal and other 256-colour terminals: half blocks would be
    /// dithered, so a block icon is shown instead of a "pixelated" image.
    Limited,
}

/// Graphics support, resolved once at startup.
struct Graphics {
    picker: Option<Picker>,
    /// Protocol chosen for this terminal; `None` until [`init`] runs.
    protocol: Option<ProtocolType>,
}

thread_local! {
    static GRAPHICS: RefCell<Graphics> = const {
        RefCell::new(Graphics {
            picker: None,
            protocol: None,
        })
    };
}

/// Start-up probe.
///
/// Kept as a module so the call site reads like "begin detecting": the probe
/// itself is synchronous but does no terminal I/O, so it costs microseconds.
pub mod probe {
    /// Detect the image protocol from the environment and configure the cache.
    pub fn start() {
        super::init();
    }
}

/// Detect the terminal's graphics support.
///
/// The decision is made from environment variables only — no stdin reads, no
/// escape-sequence queries. Every protocol this app supports is advertised by
/// its terminal through `TERM`, `TERM_PROGRAM` or an explicit opt-in, and the
/// escape-sequence query is exactly what broke input handling before.
pub fn init() {
    let protocol = forced_protocol().unwrap_or_else(detect_protocol);
    let picker = Picker::from_fontsize((8, 16));
    let mut picker = picker;
    picker.set_protocol_type(protocol);

    crate::logging::info(format!(
        "image protocol: {} (TERM={:?}, TERM_PROGRAM={:?})",
        protocol_name(protocol),
        std::env::var("TERM").unwrap_or_default(),
        std::env::var("TERM_PROGRAM").unwrap_or_default(),
    ));

    GRAPHICS.with(|graphics| {
        let mut graphics = graphics.borrow_mut();
        graphics.picker = Some(picker);
        graphics.protocol = Some(protocol);
    });
}

/// Protocol requested through `YAZI2D_IMAGE_PROTOCOL`, if any.
///
/// This is the escape hatch for a multiplexer that hides the terminal's
/// identity, and the way the tests pin a protocol.
fn forced_protocol() -> Option<ProtocolType> {
    let value = std::env::var("YAZI2D_IMAGE_PROTOCOL").ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "halfblocks" | "half" | "blocks" => Some(ProtocolType::Halfblocks),
        "kitty" => Some(ProtocolType::Kitty),
        "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
        "sixel" => Some(ProtocolType::Sixel),
        _ => None,
    }
}

/// Which protocol the environment says the terminal can draw.
fn detect_protocol() -> ProtocolType {
    let term = std::env::var("TERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();

    // Kitty graphics: Ghostty, kitty, WezTerm and foot all implement it, and it
    // is the sharpest and cheapest of the three.
    let kitty = program.contains("ghostty")
        || program.contains("kitty")
        || program.contains("wezterm")
        || term.contains("ghostty")
        || term.starts_with("xterm-kitty")
        || term.starts_with("foot")
        || term.contains("wezterm");
    if kitty {
        return ProtocolType::Kitty;
    }

    // Sixel: foot and mlterm prefer it; WezTerm supports it too but kitty is
    // chosen above. `TERM` is the only hint available here.
    if term.contains("sixel") || term.contains("mlterm") {
        return ProtocolType::Sixel;
    }

    // iTerm2 inline images.
    if program.contains("iterm") {
        return ProtocolType::Iterm2;
    }

    ProtocolType::Halfblocks
}

/// Which protocol the terminal will be drawn with.
pub fn protocol() -> ProtocolType {
    GRAPHICS.with(|graphics| {
        graphics
            .borrow()
            .protocol
            .unwrap_or(ProtocolType::Halfblocks)
    })
}

/// True when the terminal draws native graphics for us.
pub fn native() -> bool {
    protocol() != ProtocolType::Halfblocks
}

/// Resize a decoded image into a protocol ready to be rendered.
///
/// Returns `None` when graphics were never initialised (tests), so callers can
/// fall back to the half-block renderer.
pub fn render(image: DynamicImage, area: Rect) -> Option<Protocol> {
    GRAPHICS.with(|graphics| {
        let graphics = graphics.borrow();
        let picker = graphics.picker.as_ref()?;
        picker.new_protocol(image, area, Resize::Fit(None)).ok()
    })
}

/// What the terminal can do with truecolor half blocks.
pub fn terminal() -> Terminal {
    if native() {
        return Terminal::TrueColor;
    }
    if std::env::var("COLORTERM")
        .map(|value| value.contains("truecolor") || value.contains("24bit"))
        .unwrap_or(false)
    {
        return Terminal::TrueColor;
    }

    // TERM_PROGRAM is set by GUI terminals even when COLORTERM is not forwarded
    // through ssh or a multiplexer.
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if matches!(
        program.as_str(),
        "ghostty" | "Ghostty" | "iTerm.app" | "WezTerm" | "vscode" | "kitty"
    ) {
        return Terminal::TrueColor;
    }

    // 256-colour terminals without any of the markers above (macOS
    // Terminal.app, plain xterm) get a warning instead of dithered pixels.
    Terminal::Limited
}

/// Human-readable protocol name for the log.
fn protocol_name(protocol: ProtocolType) -> &'static str {
    match protocol {
        ProtocolType::Kitty => "kitty graphics",
        ProtocolType::Iterm2 => "iTerm2 inline images",
        ProtocolType::Sixel => "sixel",
        ProtocolType::Halfblocks => "half blocks",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// The probe reads process-wide environment variables, and `cargo test` runs
    /// these in parallel. This lock serialises them so one test cannot see the
    /// variables another one just set.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn image() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(120, 80, Rgb([10, 200, 40])))
    }

    #[test]
    fn a_picker_produces_a_protocol_for_an_area() {
        let _guard = lock_env();
        init();
        let encoded = render(image(), Rect::new(0, 0, 40, 20));
        assert!(encoded.is_some(), "no protocol even for half blocks");
    }

    #[test]
    fn half_blocks_encode_a_fitted_area() {
        let _guard = lock_env();
        std::env::set_var("YAZI2D_IMAGE_PROTOCOL", "halfblocks");
        init();
        std::env::remove_var("YAZI2D_IMAGE_PROTOCOL");
        // The source is 120×80 (3:2). Fitted into 10×5 cells the result keeps
        // that ratio, so it fills the width and is 4 rows tall, not 5.
        let encoded = render(image(), Rect::new(0, 0, 10, 5)).unwrap();
        assert_eq!(encoded.area().width, 10);
        assert_eq!(encoded.area().height, 4);
        assert!(encoded.area().height <= 5);
    }

    #[test]
    fn kitty_is_reported_as_native() {
        let _guard = lock_env();
        std::env::set_var("YAZI2D_IMAGE_PROTOCOL", "kitty");
        init();
        std::env::remove_var("YAZI2D_IMAGE_PROTOCOL");
        assert_eq!(protocol(), ProtocolType::Kitty);
        assert!(native());
    }

    #[test]
    fn ghostty_and_kitty_terminals_are_detected() {
        let _guard = lock_env();
        std::env::set_var("YAZI2D_IMAGE_PROTOCOL", "halfblocks");
        std::env::set_var("TERM_PROGRAM", "ghostty");
        init();
        // The explicit override still wins, which is what the variable is for.
        assert_eq!(protocol(), ProtocolType::Halfblocks);

        std::env::remove_var("YAZI2D_IMAGE_PROTOCOL");
        init();
        assert_eq!(protocol(), ProtocolType::Kitty);

        std::env::remove_var("TERM_PROGRAM");
        init();
    }

    #[test]
    fn truecolor_is_detected_from_the_environment() {
        let _guard = lock_env();
        let saved = std::env::var("COLORTERM").ok();
        std::env::set_var("COLORTERM", "truecolor");
        std::env::set_var("YAZI2D_IMAGE_PROTOCOL", "halfblocks");
        init();
        std::env::remove_var("YAZI2D_IMAGE_PROTOCOL");
        assert_eq!(terminal(), Terminal::TrueColor);
        match saved {
            Some(value) => std::env::set_var("COLORTERM", value),
            None => std::env::remove_var("COLORTERM"),
        }
    }
}
