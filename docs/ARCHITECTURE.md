# Architecture

Yazi2D is a single-binary Rust TUI built on `ratatui` + `crossterm`. This
document explains how the pieces fit and where to extend them.

## 1. Runtime shape

```
main::run
  ├─ event::poll(80ms)
  │    ├─ KeyEvent  ──► App::on_key ──► state changes, App::dirty = true
  │    ├─ Mouse     ──► scroll moves the cursor; double tap opens an entry
  │    └─ Resize    ──► App::dirty = true
  ├─ App::tick      ──► poll directory loads, sync preview, drain Job::rx,
  │                     finish jobs, refresh panels
  └─ if App::dirty  ──► ui::draw(frame, &mut App)
```

Mouse capture is enabled and keyboard enhancement is pushed (see ADR-022), so
the terminal reports key releases. `KeyEventKind::Press | Repeat` both drive
`App::on_key`; `Release` only clears held modifiers. `ScrollUp`/`ScrollDown`
call `App::on_scroll` (delta inverted, so the list follows the fingers).
`ScrollLeft`/`ScrollRight` are dropped: macOS emits them inside vertical
gestures. Left clicks call `App::on_pointer_tap`, and a second tap within
`DOUBLE_TAP_WINDOW` opens the entry under the cursor through
`open_current_entry`.

There is exactly one UI thread, at most one background `Job`, and at most one
pending directory load per panel. There is no async runtime and no `unsafe`
code. `App::dirty` prevents idle repaints: a static screen produces no terminal
output and negligible CPU.

Terminal setup/teardown lives in `main.rs`. A panic hook disables raw mode and
leaves the alternate screen so a crash never leaves the shell unusable; it also
writes the panic to the log.

## 2. Module map

### `main.rs`

- Parses `--help`, `--version`, and up to two positional directories.
- Initializes logging (`logging::init`), raw mode + alternate screen, panic hook.
- Owns the event loop and the dirty-render check.
- Restores the terminal on all normal exits.

### `logging.rs`

Tiny append-only file logger, no dependencies:

- `init()` resolves the sink once: `YAZI2D_LOG=off` disables it,
  `YAZI2D_LOG=/path` selects a file, anything else (or unset) uses
  `config_dir()/yazi2d.log`. Rotates to `.1` above 4 MB.
- `info` / `warn` / `error` queue `timestamp LEVEL message` for a background
  writer. The bounded queue can drop messages when full; I/O errors are ignored.
- Used for startup, directory loads (with duration; slow loads are `WARN`),
  image decodes, jobs, preview loads, key events slower than 100 ms and panics.

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
    pub loads: [Option<Receiver<LoadResult>>; 2],
    pub preview_cache: PreviewCache,
    pub image_cache: ImageCache,
    pub status: String,
    pub should_quit: bool,
    pub dirty: bool,
}
```

Responsibilities:

- key routing by precedence: global quit → help → prompt → global shortcuts →
  focus-specific handler;
- panel navigation (`enter_current`, `go_parent`, `open_favorite`,
  `open_entry_in_panel`) starts a background directory load;
- polling loads in `tick`, applying the newest generation only;
- file operation orchestration (`start_transfer`, `begin_*`, `do_*`);
- favorites actions (add/remove/open/focus/toggle/save);
- view modes: `toggle_preview` (`Ctrl+Q`/`Ctrl+P`, always the *other* panel),
  `preview_panel` (`Ctrl+←/→`), `toggle_thumbnails` (`Ctrl+T`),
  `open_entry_in_panel` (`Ctrl+O`, `Alt+←/→`);
- draining job progress in `tick` and refreshing panels on completion.

`Prompt` is a modal enum. Delete confirms on `Enter` or `y`, cancels on
`n`/`Esc`; mkdir/rename use a single-line text buffer with
`Esc`/`Enter`/`Backspace` handling.

`sync_preview_scroll()` runs at the start of every `tick` (and after directory
changes). For each panel in `Preview` mode it stores the opposite panel's
cursor path in `panel.preview_path`; when the subject changes it resets
vertical/horizontal scroll and marks the app dirty. If the subject panel is
still loading, the preview shows `Loading…` until the load is applied. This is
the only place that decides what a preview shows.

`blocked_by_preview()` rejects file operations (`copy`, `move`, `mkdir`,
`rename`, `delete`, add-favorite) while the active panel is a preview;
`blocked_by_loading()` does the same while the active panel is loading a
directory.

### `panel.rs`

`Panel` is a pure-ish model of one pane:

```rust
pub struct Panel {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,    // includes synthetic ".." at index 0 (non-root)
    pub cursor: usize,
    pub offset: usize,          // first visible row (list) or grid row (thumbnails)
    pub selected: HashSet<PathBuf>,
    pub page_size: usize,       // set by renderer for PgUp/PgDn
    pub error: Option<String>,
    pub mode: PanelMode,        // List | Thumbnails | Preview
    pub mode_before_preview: PanelMode,
    pub preview_path: Option<PathBuf>,
    pub preview_scroll: u16,
    pub preview_scroll_x: u16,
    pub preview_max_scroll: u16,
    pub preview_max_scroll_x: u16,
    pub grid_cols: usize,
    pub grid_rows: usize,
    /// True while a background directory load is in flight.
    pub loading: bool,
    /// Path of the in-flight load, shown in the sub-header.
    pub loading_path: Option<PathBuf>,
    /// Monotonic id of the latest load request; stale results are ignored.
    pub generation: u64,
}
```

`Entry` stores name, path, `is_dir`, `is_symlink`, `is_parent`, size, mtime.

Listing (`list_dir`, runs on a worker thread for navigation):

- inserts `..` when a parent exists;
- skips dotfiles unless `show_hidden`;
- follows symlinks only to decide `is_dir`/size/mtime, while retaining
  `is_symlink` for display and transfer;
- sorts by `(parent?, dir?, lowercase name)`.

`list_dir` reads only names and `DirEntry::file_type()` — it never stats an
entry. `Panel::spawn_load(path, select, show_hidden)` bumps `generation`, sets
`loading`, and returns an `mpsc::Receiver<LoadResult>` from a `std::thread`
that runs `list_dir`. Once the listing is applied, `App::start_stat_worker`
runs `panel::stat_entries` to fill sizes/dates in batches of 16; the size column
shows `…` until a row is enriched. Sorting by size/date re-applies when batches
arrive. Startup enriches synchronously so the first frame is complete. `Panel::apply_load(result)` accepts only a result whose
generation matches, then replaces `cwd`/`entries`, clears selection, restores
the cursor (by name when possible), and clears `loading`. `Panel::reload` and
`Panel::change_dir` remain synchronous and are used for the initial listing
and by tests.

Cursor semantics:

- entering a directory puts the cursor on the first real entry;
- going up restores the cursor to the child directory name;
- refresh preserves the current name when it still exists;
- selection is cleared on directory change and pruned on refresh.

Mode semantics:

- `set_mode(Preview)` remembers the previous mode in `mode_before_preview` and
  clears the preview subject/scroll;
- `leave_preview()` restores that mode and clears all preview state;
- `move_grid` clamps within `entries` and keeps the cursor row visible using
  `grid_cols`/`grid_rows`;
- `ensure_mode_visible` dispatches to list or grid scrolling.

`selection_or_current` is the single source of truth for operation sources:
selected entries in display order, else the cursor entry; `..` is always
excluded.

### `colors.rs`

`FileGroup::classify(path, is_dir)` maps an entry to one of ten groups
(directory, image, video, audio, text, archive, temporary, model, code, other).
`Colors::color(group)` resolves it through one of three presets (`default`,
`vivid`, `mono`). The preset is persisted in `$YAZI2D_CONFIG_DIR/theme`; `Ctrl+K`
renders a preview of every group.

### `preview.rs`

Two caches, both keyed by path and validated by mtime + size:

- `PreviewCache::get(path, show_hidden)` → `Option<&PreviewContent>`: returns
  `None` and starts a worker on a miss, so a preview never blocks a draw.
  - `Text { lines, truncated, size }` — reads at most 512 KiB, at most 5000
    lines, expands tabs, detects NUL bytes as binary;
  - `Binary { size, sample }` — hex sample of the first bytes;
  - `Directory { children, total }` — first 64 children from `read_dir`,
    directories first, flags image children, never stats them;
  - `Error(String)`.
- `ImageCache::render(path, max_width, max_height)` → `Option<Arc<RgbImage>>`:
  - returns `None` and decodes on a worker thread (at most four in flight);
  - rejects files larger than 64 MiB and remembers failures;
  - decodes with `image` (PNG/JPEG/GIF/WebP/BMP/TIFF) once and caches the
    decoded source;
  - pre-scales the source so its longest side is at most 1600 px (`Triangle`),
    then resizes to the exact target with `Lanczos3`, keeping `2 * height`
    pixel rows; resized buffers are cached per `(width, height)`;
  - composites alpha over the dark UI background so half blocks look right;
  - logs decode time and the source/target sizes.

Both caches are bounded: when they grow past 64 entries they are cleared
wholesale (simple, predictable, avoids an LRU dependency).

`is_image_path()` is the single extension check used by previews and tiles.

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
6. For copy: create dirs, `fs::copy` files, recreate symlinks on Unix; apply
   directory permissions after contents (deepest first).
7. Emit `Progress { done, total, current }` messages; collect per-item errors.
8. Always emit `Done { done, errors }`.

Delete scans top-level sources, counts files for progress, then uses
`remove_dir_all` for real directories and `remove_file` for files/symlinks.
Symlinks are never followed for deletion.

Safety rules enforced in `App::start_transfer`:

- destination must be the other panel's cwd and a directory;
- move to the source's parent is rejected;
- destination inside a source directory is rejected;
- the active panel must not be in `Preview` mode.

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

Pure rendering; it reads `App`/`Panel` and mutates only layout-derived state:
`page_size`, `offset`, `grid_cols`, `grid_rows`, preview scroll limits.

Layout:

- vertical: body (`Min(3)`), status (1), footer (1);
- body with favorites: `Length(26) | Min(20) | Min(20)`, only when width ≥ 72;
- body without favorites: `Min(20) | Min(20)`.

Panel layout:

1. bordered block, title `Left`/`Right` plus `· preview` / `· thumbnails`;
2. inner vertical: path bar (1), sub-header (1), content (rest).

`draw_panel` dispatches on `PanelMode`:

- `List` — column header + `List` widget with `ListState`; while a directory
  load is in flight the header says `Loading <path>…` and the previous list is
  kept visible;
- `Thumbnails` — tile grid. Tiles are adaptive: 3 columns on wide panels, 2 on
  narrow ones, tile width 16–32 cells and height `(width-2)/2 + 3` so the image
  area is roughly square in pixels. Directories draw a multi-row folder glyph
  (`FOLDER_ART`), non-image files draw a bordered extension badge, and each tile
  prints the stem on one row and the extension on the next. The renderer sets `grid_cols`/`grid_rows`,
  then draws visible tiles only. Image tiles call `ImageCache::render` and
  `render_halfblocks`; directories show `▸`, other files show their extension.
  The focused tile gets a background highlight.
- `Preview` — subject is the other panel's cursor path (passed in from `draw`).
  Directories render as an adaptive thumbnail grid (`draw_folder_grid`); text
  renders as a scrollable `Paragraph`; images render with `render_halfblocks`;
  binary/error content renders as a centered message. While loading, the pane
  shows `Loading…`.
  The second row is a caption with name/kind/size.

`render_halfblocks` writes directly into `frame.buffer_mut()`: each cell uses
`▀` with the top pixel as foreground and the bottom pixel as background, which
gives two vertical pixels per character cell in truecolor.

The path bar left-truncates with `…` using Unicode display width. List rows use
the same width calculation for name/size/date columns and hide details on
narrow panels. Overlays (`Prompt`, help) are centered, drawn after the main
screen with `Clear` underneath. The input prompt positions the terminal cursor
after the buffer.

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

### Navigate into a directory (background load)

```
Enter (active = left)
  → App::enter_current
      entry = left.current_entry(); must be a directory
      rx = left.spawn_load(path, select, hidden)   // generation++, loading = true
      loads[0] = Some(rx); status = "Loading <path>…"
  → App::tick (80 ms poll)
      loads[0].try_recv() → LoadResult
      left.apply_load(result) if result.generation == left.generation
          cwd/entries replaced, selection cleared, cursor restored, loading = false
      sync_preview_scroll(); dirty = true
Esc while loading
  → loads[0] = None; left.loading = false; stale result dropped
```

### Quick preview

```
Ctrl+Q / Ctrl+P (active = left)
  → App::toggle_preview
      target = other panel (right)
      right not in Preview? right.set_mode(Preview)
      sync_preview_scroll()
          right.preview_path = left.current_entry().path
  → ui::draw
      right panel renders preview of left's cursor entry
Left cursor moves (↓)
  → on_key sets dirty
  → App::tick → sync_preview_scroll() sees a new subject, resets scroll
  → ui::draw re-renders the preview
```

### Cross-panel open

```
Ctrl+O / Alt+Right (active = left)
  → App::open_entry_in_panel(1)
      entry = left.current_entry()
      entry.is_dir? else status message
      rx = right.spawn_load(entry.path, select, hidden)
```

### Favorites add

```
Ctrl+B
  → App::add_favorite
      favorites.add(active.cwd)   // canonicalize + dedupe
      favorites.save()
      status = "Added to favorites: ..."
```

## 4. Persistence and environment

| What | Where |
|---|---|
| Favorites | `$YAZI2D_CONFIG_DIR/favorites.toml` |
| Log | `$YAZI2D_CONFIG_DIR/yazi2d.log` (or `$YAZI2D_LOG`) |
| Default config dir | `dirs::config_dir()/yazi2d` |
| Start directories | CLI args; fallbacks: cwd / home / Downloads |
| Build artifacts | `target/` (override with `CARGO_TARGET_DIR`) |

No other files are written. User-facing messages go to the status line; the
diagnostic log is separate and never required for correctness.

## 5. Testing

- Module tests use `tempfile`; no test touches real user data.
- Transfer tests consume `Job::rx` until `Done` with a 10-second timeout.
- Preview tests build tiny text/binary/image fixtures and assert cache output.
- App tests drive `App::on_key` with `KeyEvent`s: mode transitions,
  `Ctrl+Q`/`Ctrl+P` targeting the other panel, `Ctrl+O`, `Alt+←/→`,
  `Shift+Tab`, preview-follows-cursor, Enter-confirmed delete. Navigation
  tests call `wait_for_loads()` (repeated `App::tick` with a timeout).
- `logging` tests check line formatting and that disabled logging is a no-op.
- UI tests render into `TestBackend` and assert text fragments (paths, names,
  overlays, preview content, thumbnail titles, `Loading…` state).
- `scripts/e2e_pty.py` runs the real binary in a PTY with a 120×30 window,
  continuously drains output in a thread, and verifies copy, move, favorites,
  hidden files, quick preview (`Ctrl+P`), thumbnails, cross-panel open
  (`Ctrl+O`), favorites focus (`Shift+Tab`), help, mkdir, delete with `Enter`,
  and clean exit.

## 6. Extension points

- **New operation**: add `JobKind`, implement in `transfer.rs`, add key route in
  `app.rs`, show it in status/help/README.
- **New overlay**: add a `Prompt` variant + `ui::draw_prompt` arm; keep input
  handling inside `handle_prompt_key`.
- **New panel mode**: add a `PanelMode` variant, a key toggle in `app.rs`, and a
  `draw_panel` branch; keep `mode_before_preview` semantics intact.
- **New preview kind**: add a `PreviewContent` variant in `preview.rs`, render
  it in `ui::draw_preview`, and extend the cache tests.
- **New panel column**: extend `Entry`, `list_dir`, and `build_row`; keep the
  width math in one place.
- **Config file**: extend `favorites.rs` into a `config` module or add a new
  TOML file; avoid global mutable state.
- **Cancellation**: add an `Arc<AtomicBool>` to `Job`, check between items, and
  send a `Cancelled` message. Never block the UI thread waiting for a worker.

## 7. Invariants

1. The UI thread never performs recursive/bulk I/O; directory listings run on a
   worker thread.
2. `..` is navigation only.
3. Destination names are never silently overwritten.
4. The path bar is always the first inner row of a panel.
5. A panel's cwd only changes after a successful directory listing.
6. Only one job at a time; navigation remains available during a job.
7. Idle frames are not rendered.
8. Terminal state is restored on exit and panic.
9. Only one panel is in `Preview`; it mirrors the other panel's cursor and is
   read-only.
10. Preview/thumbnail decoding goes through the caches, only for visible items.
11. `Ctrl+Q`/`Ctrl+P` target the *other* panel; `Ctrl+O`/`Alt+←/→` open the
    active cursor folder there. These bindings are not navigation history.
12. Only the newest directory load generation is applied; a superseded or
    cancelled load never changes a panel.
13. Logging must never panic and must never block the UI on I/O.
