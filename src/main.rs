mod app;
mod clipboard;
mod colors;
mod disks;
mod favorites;
mod graphics;
mod icons;
mod keys;
mod logging;
mod panel;
mod preview;
mod search;
mod settings;
mod transfer;
mod trash;
mod ui;

/// Test-only helpers (environment lock, temporary env vars).
#[cfg(test)]
mod testutil;

use std::io::{self, stdout};
use std::panic;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;

fn main() -> io::Result<()> {
    logging::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    logging::info(format!("args: {args:?}"));

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Yazi2D {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let paths: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    // Ask the terminal to report key press *and release* events, so the footer
    // can light up while a modifier is held. `REPORT_ALL_KEYS_AS_ESCAPE_CODES`
    // is what makes the terminal report the modifier keys themselves: with only
    // event types, Ctrl produces nothing until a letter is pressed, and the
    // footer could never switch on a bare Ctrl. Terminals without kitty-protocol
    // support silently ignore the sequence; the footer then simply keeps its
    // default screen. We deliberately do not probe support first: the probe
    // blocks for up to two seconds waiting for a reply.
    let keyboard_enhanced = execute!(
        out,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    )
    .is_ok();
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    // Restore the terminal even if a panic happens deep inside a widget.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| text.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "?".to_string());
        logging::error(format!("panic at {location}: {message}"));
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    // Ask the terminal which graphics protocol it speaks (kitty, iTerm2,
    // sixel). The probe is driven from the event loop rather than from a
    // blocking call here, because `ratatui-image`'s own query leaves a thread
    // reading stdin and swallows every keystroke when the terminal is silent.
    graphics::probe::start();

    let mut app = App::new(paths);
    if logging::enabled() {
        logging::info(format!(
            "log file: {}",
            logging::path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "disabled".to_string())
        ));
    }
    let result = run(&mut terminal, &mut app, keyboard_enhanced);

    disable_raw_mode()?;
    if keyboard_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    logging::info("exit");
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    keyboard_enhanced: bool,
) -> io::Result<()> {
    loop {
        app.tick();
        // A held arrow accelerates: while waiting for the next key event the app
        // moves the cursor itself, faster than the terminal repeats the key.
        app.tick_key_repeat();

        if app.dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.dirty = false;
        }

        if app.should_quit {
            return Ok(());
        }

        // Short timeout while an arrow is held, so the automatic repeat keeps a
        // steady pace; otherwise a long one keeps idle CPU near zero.
        let timeout = if app.held_arrow.is_some() {
            Duration::from_millis(10)
        } else {
            Duration::from_millis(80)
        };

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => match key.kind {
                    // `Repeat` is what a held key sends once keyboard
                    // enhancement is on; ignoring it would break key repeat.
                    KeyEventKind::Press | KeyEventKind::Repeat => {
                        let started = Instant::now();
                        app.on_key(key);
                        let elapsed = started.elapsed();
                        if elapsed.as_millis() > 100 {
                            logging::warn(format!(
                                "slow key handling ({} ms): {:?} {:?}",
                                elapsed.as_millis(),
                                key.code,
                                key.modifiers
                            ));
                        }
                    }
                    // With keyboard enhancement the terminal also reports
                    // releases, which is exactly what the footer needs to know
                    // when a modifier is let go.
                    KeyEventKind::Release if keyboard_enhanced => app.on_key_release(key),
                    _ => {}
                },
                Event::Resize(_, _) => app.dirty = true,
                // Trackpad scrolling moves the list, and the pointer is fully
                // routed now: a click selects a row, a double click opens it, a
                // click on the path bar copies the path, and a right click opens
                // the context menu. `on_mouse` handles all of it; a terminal
                // that reports only bare presses still falls back to the
                // two-finger double tap.
                Event::Mouse(mouse) => app.on_mouse(mouse),
                _ => {}
            }
        }
    }
}

fn print_help() {
    println!(
        "Yazi2D {} — fast dual-pane terminal file manager\n\
         \n\
         USAGE:\n\
         \x20 yazi2d [LEFT_DIR] [RIGHT_DIR]\n\
         \n\
         If a directory is omitted, the current directory and your home directory are used.\n\
         \n\
         KEYS:\n\
         \x20 Tab             switch active panel\n\
         \x20 § / Shift+Tab   focus favorites / back to panels\n\
         \x20 Arrows / hjkl   navigate\n\
         \x20 Enter / l       open folder, or file in its associated app\n\
         \x20 Backspace / h   go to parent directory\n\
         \x20 Space / Insert  select entry (and move down)\n\
         \x20 F5 / c          copy selection to the other panel\n\
         \x20 F6 / m          move selection to the other panel\n\
         \x20 F7 / n          create directory\n\
         \x20 F8 / d / Del    delete selection (Enter or y confirms)\n\
         \x20 F2 / r          rename entry\n\
         \x20 Ctrl+A          select everything\n\
         \x20 Ctrl+G          bulk rename ({{name}} {{ext}} {{n}} or %e=ext)\n\
         \x20 s / Ctrl+S      sort menu (name/size/type/date, r reverse)\n\
         \x20 Ctrl+K          colour menu for file types\n\
         \x20 Ctrl+E          settings: key bindings and trackpad\n\
         \x20 Alt+F7          find files recursively (Shift: other panel)\n\
         \x20 Ctrl+L          copy the current path to the clipboard\n\
         \x20 Alt+I           copy the cursor name to the clipboard\n\
         \x20 Ctrl+Shift+C    copy the selected paths\n\
         \x20 Ctrl+Q / Ctrl+P quick preview in the OTHER panel\n\
         \x20 Ctrl+T          thumbnail grid in the active panel\n\
         \x20 Ctrl+O          open cursor folder in the other panel\n\
         \x20 Shift+Left/Right open cursor folder in that panel\n\
         \x20 Alt+Left/Right  quick preview in that panel\n\
         \x20 Ctrl+Left/Right preview in that panel (alias)\n\
         \
         \x20 Ctrl+B / a      add current directory to favorites\n\
         \x20 Ctrl+D          focus favorites / back to panels\n\
         \x20 Ctrl+F          show / hide the favorites sidebar\n\
         \x20 Ctrl+R          refresh both panels\n\
         \x20 Ctrl+H / .      toggle hidden files\n\
         \x20 ? / F1          help\n\
         \x20 q / Ctrl+C      quit\n\
         \n\
         CONFIG & LOG:\n\
         \x20 $YAZI2D_CONFIG_DIR/favorites.toml\n\
         \x20 (default: ~/.config/yazi2d/favorites.toml)\n\
         \x20 $YAZI2D_CONFIG_DIR/yazi2d.log  (YAZI2D_LOG=off disables)\n",
        env!("CARGO_PKG_VERSION")
    );
}
