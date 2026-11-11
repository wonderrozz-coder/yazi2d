# AGENTS.md — Yazi2D handoff

Read this first. It is the single entry point for any agent or developer who
continues this project from any machine.

## Mission

**Yazi2D** is a fast, dual-pane terminal file manager: Yazi's speed and TUI
aesthetics with a deliberately simplified Total Commander workflow.

The product must stay small, predictable and fast. Three features are
non-negotiable and define the project:

1. Two independent panels with copy and move **between** them.
2. A current-path bar at the top of **each** panel.
3. A persistent favorites sidebar for folders the user bookmarked.

Non-goals for now: full Yazi feature parity, plugin system, tabs, remote
filesystems, a GUI, sixel/Kitty image protocols.

## Current status (2026-11-11)

Version `0.3.9` — MVP + quick view, thumbnails, background loading, logging,
sorting, type colours, bulk rename, trackpad gestures, a live footer, and a
polished thumbnail grid with a separate extension column.

- [x] Two panels, independent cwd/cursor/selection, active-panel highlighting.
- [x] Path bar at the top of both panels (left-truncated, tail always visible).
- [x] Favorites sidebar: toggle, focus (`Shift+Tab`/`Ctrl+D`), add current dir, navigate, remove, TOML persistence.
- [x] Copy/move selection (or cursor item) to the other panel, recursive, symlink-aware.
- [x] Background operations with progress; UI remains navigable.
- [x] Mkdir, rename, delete confirmed by `Enter` (or `y`), multi-select, hidden files (`Ctrl+H`/`.`).
- [x] Quick preview: text, images (half-block truecolor), directories; follows the other panel's cursor; `Ctrl+Q`/`Ctrl+P` always target the opposite panel.
- [x] Thumbnail grid mode with cached image tiles, adaptive size and Lanczos3 resampling.
- [x] Cross-panel keys: `Ctrl+O` (and `Alt+←/→`) open the cursor folder in the neighbour; `Ctrl+←/→` preview there.
- [x] Background directory loading with `Loading <path>…`, `Esc` cancel, generation checks.
- [x] File log with load timings and `WARN` for slow paths (`YAZI2D_LOG=off` disables).
- [x] Safe conflict policy: unique suffix instead of overwrite.
- [x] Multi-select, select-all (`Ctrl+A`), bulk rename (`Ctrl+G`), sorting (`s`),
  file-type colours with a menu (`Ctrl+K`).
- [x] `§` opens favorites; trackpad scroll + double-tap open; live footer.
- [x] Folder/file thumbnail glyphs, `Ext` column in the list.
- [x] `F9` chord: `F9` + arrows for cross-panel open / thumbnails.
- [x] Key repeat and debounced trackpad swipes.
- [x] Layout-independent shortcuts (`src/keys.rs`), `Enter` opens files.
- [x] Block-art icons per file group in the thumbnail grid.
- [x] 69 unit tests + a real-PTY end-to-end test, all green.
- [x] `cargo fmt`, `cargo clippy -D warnings`, release build (≈2 MB binary).

Known limitations and the next steps are in `docs/STATUS.md` and
`docs/ROADMAP.md`. Keep both updated when behavior changes.

## Quick start

```sh
# Rust stable is required (rust-toolchain.toml pins the channel).
source "$HOME/.cargo/env"

cargo run --release                     # left: cwd, right: home
cargo run --release -- ~/Projects /Volumes/Backup

# Quality gates — run all of them before claiming a task is done:
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release

# Real terminal integration test (needs python3, no extra packages):
python3 scripts/e2e_pty.py target/debug/yazi2d
```

If the repo lives in a synced folder (Nextcloud/Dropbox), set
`CARGO_TARGET_DIR` outside it to avoid syncing build artifacts:

```sh
export CARGO_TARGET_DIR="$HOME/.cache/yazi2d-target"
```

## Repository map

```
Cargo.toml                 dependencies + release profile
rust-toolchain.toml        stable toolchain
src/main.rs                CLI, terminal setup/restore, event loop, --help
src/app.rs                 App state, key dispatch, prompts, jobs, favorites actions
src/panel.rs               Panel state, modes, navigation, background directory loads
src/preview.rs             async preview/image caches, image resampling
src/colors.rs              file-type groups, colour presets, persisted theme
src/keys.rs                Cyrillic→Latin key mapping so shortcuts ignore layout
src/icons.rs               block-art icons per FileGroup for the thumbnail grid
src/logging.rs             append-only file log (timings, slow loads, panics)
src/transfer.rs            background copy/move/delete engine + progress channel
src/favorites.rs           favorites model and TOML persistence
src/ui.rs                  ratatui rendering, layout, overlays, preview/thumbnails, UI tests
scripts/e2e_pty.py         end-to-end test in a real pseudo-terminal
docs/CONCEPT.md            product concept and UX spec (Russian)
docs/ARCHITECTURE.md       module/data-flow/design details (English)
docs/DECISIONS.md          ADR-style decisions and rationale
docs/ROADMAP.md            staged plan (Russian)
docs/STATUS.md             current state, known issues, next tasks
README.md                  user-facing docs, keys, config (Russian)
```

Upstream Yazi is **not vendored** and must not be copied wholesale. It is an
inspiration: https://github.com/sxyazi/yazi.

## Architecture in one screen

- Single-threaded UI loop: `main::run` polls crossterm events every 80 ms,
  calls `App::tick`, and redraws only when `App::dirty` is set.
- `App` owns `[Panel; 2]`, `active`, `focus`, favorites, prompt/help state,
  `PreviewCache`, `ImageCache`, at most one `Job`, and at most one pending
  directory load per panel.
- `Panel` owns cwd, `Vec<Entry>` (with a synthetic `..` row), cursor, scroll
  offset, selected path set, visible page size, and a `PanelMode`
  (`List`, `Thumbnails`, `Preview`) with grid/preview scroll state, plus
  `loading`/`loading_path`/`generation` for background listing.
- Directory navigation calls `Panel::spawn_load`, which runs `list_dir` on a
  `std::thread` and returns an `mpsc::Receiver<LoadResult>`; `App::tick` polls
  it and applies only the newest `generation`. The previous list stays visible
  and the header shows `Loading <path>…`; `Esc` cancels. Initial startup
  listing is synchronous.
- A panel in `Preview` shows the **other panel's** cursor entry. `App::sync_preview_scroll`
  keeps the subject path in sync and resets scroll when it changes; only one
  preview panel exists at a time. Preview is read-only: file operations are
  rejected while the active panel is a preview.
- `preview.rs` never blocks: `PreviewCache` and `ImageCache` return `None` on a
  miss, load on a worker thread, and the next `tick` picks the result up. Images
  are pre-scaled to ≤1600 px, then resized with Lanczos3 into cached
  half-block-sized RGB buffers (two pixel rows per terminal cell). No external
  viewer or sixel/Kitty protocol is used.
- Directory listing reads only names and `file_type()`; sizes/dates are filled
  in by `panel::stat_entries` in batches. This is what keeps Nextcloud folders
  fast (ADR-019). Never introduce a `metadata()` call inside a listing or inside
  `ui::draw`.
- `colors.rs` maps paths to `FileGroup` and a `Preset`; the chosen preset lives
  in `$YAZI2D_CONFIG_DIR/theme`.
- File operations never run on the UI thread. `transfer::spawn` returns a
  `Job` with an `mpsc::Receiver<JobMsg>`; `App::tick` drains it and refreshes
  panels on completion.
- `logging.rs` queues lines for a background writer to `yazi2d.log`; includes
  load timings, slow-path `WARN`s, image decodes and panics. The bounded queue
  may drop messages; `YAZI2D_LOG=off` disables logging.
- Favorites are stored as a flat path list in TOML at
  `$YAZI2D_CONFIG_DIR/favorites.toml` or the platform config dir.
- The UI is composed with ratatui: body (favorites + two panels), status line,
  footer line; prompt/help are centered overlays. Panel rendering dispatches on
  `PanelMode`; thumbnails and previews draw with `frame.buffer_mut()`.

Detailed invariants and data flow: `docs/ARCHITECTURE.md`.

## Behavior that must not regress

- Copy/move destination is **always the other panel's cwd**, never a prompt.
- Existing destination names are **never overwritten**: generate `name (1).ext`.
- Moving to the source's own parent directory is rejected.
- Copying/moving a folder into itself is rejected.
- `..` is a display row; it is never selected, copied, moved, renamed or deleted.
- When a selection exists, only selected entries are transferred; otherwise the
  cursor entry is used.
- The path bar must remain the first inner row of each panel.
- The UI thread must never perform bulk filesystem work; directory listings run
  on a worker thread.
- Idle redraws should not happen (`dirty` flag).
- Terminal state must be restored on normal exit and on panic.
- Only one panel may be in `Preview` mode; it always shows the other panel's
  cursor entry and never performs file operations.
- Preview/thumbnail decoding stays on the UI thread only for the visible items
  and must go through `PreviewCache`/`ImageCache` (never re-read per frame).
- `Ctrl+Q`/`Ctrl+P` target the **opposite** panel; `Ctrl+O`/`Alt+←/→` open the
  active cursor folder in that neighbour. Do not reuse these for history.
- Only the newest directory load generation may change a panel; cancelled or
  superseded loads are ignored.
- Logging must never panic; lines are flushed immediately so the last one
  survives a hang.
- No `stat`/`metadata` call may run inside `list_dir`, `PreviewCache::get` or any
  draw function: on cloud mounts those block for hundreds of milliseconds.
- Previews and image decoding must stay asynchronous; the UI thread only reads
  caches.
- `KeyEventKind::Repeat` must be handled exactly like `Press`: with keyboard
  enhancement enabled, held keys arrive as `Repeat` and dropping them breaks
  key repeat.
- Shortcuts must be layout-independent: `App::on_key` normalizes Cyrillic
  characters through `keys::normalize` **before** dispatch, but never while a
  prompt is open (prompts need the raw character).

## Key handling summary

Panel mode: `Tab` switches panel; `§` focuses favorites; `Shift+Tab` does
the same; arrows/`hjkl`
navigate; `Enter`/`l` opens a directory; `Backspace`/`h` goes up; `Space`
selects and advances; `F5`/`c` copies; `F6`/`m` moves; `F7`/`n` mkdir;
`F2`/`r` renames; `F8`/`d`/`Del` deletes (confirm with `Enter` or `y`);
`Ctrl+B`/`a` favorites; `Ctrl+D` focus favorites; `Ctrl+F`/`f` toggle
favorites; `Ctrl+R` refresh; `Ctrl+H`/`.` hidden files; `?` help; `q` quit.
`Esc` cancels a pending directory load.

View: `Ctrl+Q`/`Ctrl+P` previews the **other** panel (a folder subject renders
as a thumbnail grid, a file as text/image/binary); `Ctrl+T` toggles
thumbnails; `Ctrl+O` opens the cursor folder in the other panel;
`Ctrl+←/→` previews the indicated panel; `Alt+←/→` opens the cursor folder
there. Preview: arrows/`hjkl` scroll, `Esc`/`Ctrl+Q`/`Ctrl+P` closes.
Thumbnails: arrows/`hjkl` move, `Enter` opens, `Ctrl+T` returns to the list.

Favorites focus: arrows/`jk`, `Enter` navigate, `Del`/`d` remove,
`Esc`/`Shift+Tab` close.

Chord: `F9` arms a leader; the next arrow opens the cursor folder in that
panel (`←`/`→`) or toggles thumbnails (`↑`/`↓`). `Esc` or an unrelated key
cancels it. This is the fallback for terminals that swallow Ctrl/Alt+arrow.

Selection & tools: `Ctrl+A` selects all, `Ctrl+G` bulk rename (`{name}`,
`{ext}`, `{n}`, `%e=ext`), `s`/`Ctrl+S` sort menu, `Ctrl+K` colour menu.

Gestures: two-finger scroll moves the cursor (vertical, inverted). A double tap
with two fingers opens the entry under the cursor (folder → enter, file → system
default app) and also works in the favorites sidebar. Any mouse button counts,
because macOS reports a two-finger tap as a right click. Single taps and horizontal scroll are deliberately inert: macOS
emits `ScrollLeft`/`ScrollRight` inside vertical gestures, and acting on them
caused random navigation. The footer shows Ctrl/Alt/Shift chords while the
modifier is held when the terminal supports key-release events.

Global: `Ctrl+C` quits, `q` quits outside prompts/help, `?`/`F1` help.

macOS F-keys may require `Fn`; letter alternatives are intentional and must be
kept. `Ctrl+←/→` and `Alt+←/→` may be intercepted by macOS/terminals — every
cross-panel action must have a `Ctrl+letter` alternative (`Ctrl+P`, `Ctrl+O`,
`Shift+Tab`).

## Testing strategy

- Unit tests live next to modules (`#[cfg(test)]`). Use `tempfile` for filesystem
  tests; never touch the user's real home/config.
- `transfer` tests wait on `Job::rx` with a timeout.
- `preview` tests build tiny text/binary/image fixtures in tempdirs.
- `ui` tests render with `ratatui::backend::TestBackend` and assert buffer text,
  including preview and thumbnail modes.
- `app` tests drive `App::on_key` with real `KeyEvent`s (modes, `Ctrl+←/→`,
  `Alt+←/→`, Enter-confirmed delete).
- `scripts/e2e_pty.py` drives the real binary in a PTY: checks both panels,
  favorites, copy, move, hidden files, quick preview, thumbnails, cross-panel
  open, help, mkdir, delete with `Enter`, and clean `q` exit. It drains the PTY
  in a background thread — keep that, or the app can block on a full PTY buffer.

## Conventions

- Rust 2021, no `unsafe`, no async runtime. A thread + mpsc is enough.
- Keep dependencies minimal; justify new ones in `docs/DECISIONS.md`.
- UI text is English; user docs are Russian. Code comments are English.
- Never add a key binding without updating `README.md`, the in-app help, and
  `AGENTS.md` if it changes the interaction model.
- When adding a feature: update `docs/STATUS.md`, tick the relevant
  `docs/ROADMAP.md` item, add tests, run all quality gates.

## Handoff checklist

1. `source "$HOME/.cargo/env"` and run the quality gates above.
2. Run `python3 scripts/e2e_pty.py target/debug/yazi2d`.
3. Read `docs/STATUS.md` for the immediate next tasks.
4. Keep the three non-negotiable features working; do not trade them for scope.
5. Commit with a focused message; do not commit `target/`.
