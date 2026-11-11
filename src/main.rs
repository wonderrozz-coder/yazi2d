mod app;
mod favorites;
mod panel;
mod transfer;
mod ui;

use std::io::{self, stdout};
use std::panic;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

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
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    // Restore the terminal even if a panic happens deep inside a widget.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    let mut app = App::new(paths);
    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> io::Result<()> {
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
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Resize(_, _) => app.dirty = true,
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
         \x20 Arrows / hjkl   navigate\n\
         \x20 Enter / l       open directory\n\
         \x20 Backspace / h   go to parent directory\n\
         \x20 Space / Insert  select entry (and move down)\n\
         \x20 F5 / c          copy selection to the other panel\n\
         \x20 F6 / m          move selection to the other panel\n\
         \x20 F7 / n          create directory\n\
         \x20 F8 / d / Del    delete selection\n\
         \x20 F2 / r          rename entry\n\
         \x20 Ctrl+B / a      add current directory to favorites\n\
         \x20 Ctrl+F / f      toggle favorites sidebar\n\
         \x20 Ctrl+R          refresh both panels\n\
         \x20 .               toggle hidden files\n\
         \x20 ? / F1          help\n\
         \x20 q / Ctrl+C      quit\n\
         \n\
         CONFIG:\n\
         \x20 $YAZI2D_CONFIG_DIR/favorites.toml\n\
         \x20 (default: ~/.config/yazi2d/favorites.toml)\n",
        env!("CARGO_PKG_VERSION")
    );
}
