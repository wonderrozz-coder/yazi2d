# Architecture

Yazi2D is a single-binary Rust TUI built on `ratatui` + `crossterm`. This
document explains how the pieces fit and where to extend them.

## 1. Runtime shape

```
main::run
  ├─ event::poll(80ms)
  │    ├─ KeyEvent  ──► App::on_key ──► state changes, App::dirty = true
  │    └─ Resize    ──► App::dirty = true
  ├─ App::tick      ──► drain Job::rx, finish jobs, refresh panels
  └─ if App::dirty  ──► ui::draw(frame, &mut App)
```

There is exactly one UI thread and at most one background `Job`. There is no
async runtime and no `unsafe` code. `App::dirty` prevents idle repaints: a
static screen produces no terminal output and negligible CPU.

Terminal setup/teardown lives in `main.rs`. A panic hook disables raw mode and
leaves the alternate screen so a crash never leaves the shell unusable.

## 2. Module map

### `main.rs`

- Parses `--help`, `--version`, and up to two positional directories.
- Initializes raw mode + alternate screen, installs the panic hook.
- Owns the event loop and the dirty-render check.
- Restores the terminal on all normal exits.

### `app.rs`

`App` is the root state machine:

```rust
pub struct App {
    pub panels: [Panel; 2],
    pub active: usize,          // 0 = left, 1 = right
    pub focus: Focus,           // Panels | Favorites
    pub favorites: Favorites,
    pub show_favorites: bool,
    pub fav_cursor: usize,
    pub hidden: bool,
    pub prompt: Option<Prompt>, // Mkdir | Rename | Delete
    pub help: bool,
    pub job: Option<Job>,
    pub status: String,
    pub should_quit: bool,
    pub dirty: bool,
}
```

Responsibilities:

- key routing by precedence: global quit → help → prompt → global shortcuts →
  focus-specific handler;
- panel navigation (`enter_current`, `go_parent`);
- file operation orchestration (`start_transfer`, `begin_*`, `do_*`);
- favorites actions (add/remove/open/toggle/save);
- draining job progress in `tick` and refreshing panels on completion.

`Prompt` is a modal enum. Delete uses a `y/N` confirmation; mkdir/rename use a
single-line text buffer with `Esc`/`Enter`/`Backspace` handling.

### `panel.rs`

`Panel` is a pure-ish model of one pane:

```rust
pub struct Panel {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,    // includes synthetic ".." at index 0 (non-root)
    pub cursor: usize,
    pub offset: usize,
    pub selected: HashSet<PathBuf>,
    pub page_size: usize,       // set by renderer for PgUp/PgDn
    pub error: Option<String>,
}
```

`Entry` stores name, path, `is_dir`, `is_symlink`, `is_parent`, size, mtime.

Listing (`list_dir`):

- inserts `..` when a parent exists;
- skips dotfiles unless `show_hidden`;
- follows symlinks only to decide `is_dir`/size/mtime, while retaining
  `is_symlink` for display and transfer;
- sorts by `(parent?, dir?, lowercase name)`.

Cursor semantics:

- entering a directory puts the cursor on the first real entry;
- going up restores the cursor to the child directory name;
- refresh preserves the current name when it still exists;
- selection is cleared on directory change and pruned on refresh.

`selection_or_current` is the single source of truth for operation sources:
selected entries in display order, else the cursor entry; `..` is always
excluded.

### `transfer.rs`

The filesystem engine. `spawn(kind, sources, dest)` returns immediately with a
`Job`:

```rust
pub struct Job {
    pub kind: JobKind,          // Copy | Move | Delete
    pub rx: Receiver<JobMsg>,
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub errors: Vec<String>,
    pub finished: bool,
}
```

Worker flow for copy/move:

1. Deduplicate sources; if a parent is selected, nested children are dropped.
2. Create the destination directory if needed.
3. Scan each top-level source into a `Group` of pre-order `Item`s
   (`Dir`, `File`, `Symlink`) and count files.
4. Allocate a unique destination name per group (`name (1).ext`), tracking
   planned names in a `HashSet`.
5. For move: try `fs::rename` per group (same-filesystem fast path). If it
   fails, fall back to copy + remove.
6. For copy: create dirs, `fs::copy` files, recreate symlinks on Unix, preserve
   permissions where cheap.
7. Emit `Progress { done, total, current }` messages; collect per-item errors.
8. Always emit `Done { done, errors }`.

Delete scans top-level sources, counts files for progress, then uses
`remove_dir_all` for real directories and `remove_file` for files/symlinks.
Symlinks are never followed for deletion.

Safety rules enforced in `App::start_transfer`:

- destination must be the other panel's cwd and a directory;
- move to the source's parent is rejected;
- destination inside a source directory is rejected.

### `favorites.rs`

Flat, ordered path list with TOML persistence:

```toml
favorites = ["/Users/me", "/Users/me/Projects"]
```

- `load_or_default()` uses `$YAZI2D_CONFIG_DIR/favorites.toml` or the platform
  config dir, seeding `~`, `Desktop`, `Documents`, `Downloads` on first run.
- `add()` canonicalizes when possible and deduplicates.
- `save()` writes pretty TOML, creating the config directory.
- Tests use `load_from_dir()` so they never touch the real config.

### `ui.rs`

Pure rendering; it reads `App`/`Panel` and only mutates `panel.page_size` and
`panel.offset` as a consequence of layout.

Layout:

- vertical: body (`Min(3)`), status (1), footer (1);
- body with favorites: `Length(26) | Min(20) | Min(20)`, only when width ≥ 72;
- body without favorites: `Min(20) | Min(20)`.

Panel layout:

1. bordered block, title `Left`/`Right`, active border cyan;
2. inner vertical: path bar (1), column header (1), list (rest).

The path bar left-truncates with `…` using Unicode display width. List rows use
the same width calculation for name/size/date columns and hide details on
narrow panels. `ListState` drives scrolling; the resulting offset is written
back to the panel.

Overlays (`Prompt`, help) are centered, drawn after the main screen with
`Clear` underneath. The input prompt positions the terminal cursor after the
buffer.

## 3. Data flow examples

### Copy

```
Key F5
  → App::start_transfer(Copy)
      sources = active.selection_or_current()
      dest    = other.cwd
      job     = transfer::spawn(...)
      dirty   = true
  → App::tick (later)
      job.rx: Progress ... → job.done/total/current
      job.rx: Done         → job.finished
  → refresh_all(), status = "Copied N item(s)."
```

### Favorites add

```
Ctrl+B
  → App::add_favorite
      favorites.add(active.cwd)   // canonicalize + dedupe
      favorites.save()
      status = "Added to favorites: ..."
```

### Enter directory

```
Enter
  → App::enter_current
      entry.is_dir?
      Panel::change_dir(path, select = None, hidden)
          list_dir() into a temporary Vec
          commit cwd/entries only on success
          clear selection, cursor = first real entry
```

## 4. Persistence and environment

| What | Where |
|---|---|
| Favorites | `$YAZI2D_CONFIG_DIR/favorites.toml` |
| Default config dir | `dirs::config_dir()/yazi2d` |
| Start directories | CLI args; fallbacks: cwd / home / Downloads |
| Build artifacts | `target/` (override with `CARGO_TARGET_DIR`) |

No other files are written. Logs go to the status line only.

## 5. Testing

- Module tests use `tempfile`; no test touches real user data.
- Transfer tests consume `Job::rx` until `Done` with a 10-second timeout.
- UI tests render into `TestBackend` and assert text fragments (paths, names,
  overlays).
- `scripts/e2e_pty.py` runs the real binary in a PTY with a 120×30 window,
  continuously drains output in a thread, and verifies copy, favorites,
  help, mkdir, and clean exit.

## 6. Extension points

- **New operation**: add `JobKind`, implement in `transfer.rs`, add key route in
  `app.rs`, show it in status/help/README.
- **New overlay**: add a `Prompt` variant + `ui::draw_prompt` arm; keep input
  handling inside `handle_prompt_key`.
- **New panel column**: extend `Entry`, `list_dir`, and `build_row`; keep the
  width math in one place.
- **Config file**: extend `favorites.rs` into a `config` module or add a new
  TOML file; avoid global mutable state.
- **Cancellation**: add an `Arc<AtomicBool>` to `Job`, check between items, and
  send a `Cancelled` message. Never block the UI thread waiting for a worker.

## 7. Invariants

1. The UI thread never performs recursive/bulk I/O.
2. `..` is navigation only.
3. Destination names are never silently overwritten.
4. The path bar is always the first inner row of a panel.
5. A panel's cwd only changes after a successful directory listing.
6. Only one job at a time; navigation remains available during a job.
7. Idle frames are not rendered.
8. Terminal state is restored on exit and panic.
