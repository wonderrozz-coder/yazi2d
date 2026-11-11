mod app;
mod colors;
mod favorites;
mod logging;
mod panel;
mod preview;
mod transfer;
mod ui;

use std::io::{self, stdout};
use std::panic;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
    // can light up while a modifier is held. Terminals without kitty-protocol
    // support silently ignore the sequence; releases then simply never arrive.
    // We deliberately do not probe support first: the probe blocks for up to
    // two seconds waiting for a reply.
    let keyboard_enhanced = execute!(
        out,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
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

        if app.dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.dirty = false;
        }

        if app.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(key) => match key.kind {
                    KeyEventKind::Press => {
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
                // Trackpad two-finger gestures arrive as scroll events. Vertical
                // scrolling moves the list (inverted, see `on_scroll`); horizontal
                // swipes mirror the left/right arrows.
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => app.on_scroll(-1),
                    MouseEventKind::ScrollDown => app.on_scroll(1),
                    MouseEventKind::ScrollLeft => app.on_horizontal_gesture(true),
                    MouseEventKind::ScrollRight => app.on_horizontal_gesture(false),
                    _ => {}
                },
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
         \x20 Shift+Tab       focus favorites / back to panels\n\
         \x20 Arrows / hjkl   navigate\n\
         \x20 Enter / l       open directory\n\
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
         \x20 Ctrl+Q / Ctrl+P quick preview in the OTHER panel\n\
         \x20 Ctrl+T          thumbnail grid in the active panel\n\
         \x20 Ctrl+O          open cursor folder in the other panel\n\
         \x20 Ctrl+Left/Right preview in that panel (may be OS-bound)\n\
         \x20 Alt+Left/Right  open cursor folder in that panel\n\
         \x20 Ctrl+B / a      add current directory to favorites\n\
         \x20 Ctrl+D          focus favorites / back to panels\n\
         \x20 Ctrl+F / f      toggle favorites sidebar\n\
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
