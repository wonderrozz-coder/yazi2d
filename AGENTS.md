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

Non-goals for now: full Yazi feature parity, plugin system, image previews,
tabs, remote filesystems, a GUI.

## Current status (2026-11-11)

Version `0.1.0` — working MVP.

- [x] Two panels, independent cwd/cursor/selection, active-panel highlighting.
- [x] Path bar at the top of both panels (left-truncated, tail always visible).
- [x] Favorites sidebar: toggle, add current dir, navigate, remove, TOML persistence.
- [x] Copy/move selection (or cursor item) to the other panel, recursive, symlink-aware.
- [x] Background operations with progress; UI remains navigable.
- [x] Mkdir, rename, delete with confirmation, multi-select, hidden files.
- [x] Safe conflict policy: unique suffix instead of overwrite.
- [x] 14 unit tests + a real-PTY end-to-end test, all green.
- [x] `cargo fmt`, `cargo clippy -D warnings`, release build (≈1 MB binary).

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
src/panel.rs               Panel state, directory listing, cursor, selection
src/transfer.rs            background copy/move/delete engine + progress channel
src/favorites.rs           favorites model and TOML persistence
src/ui.rs                  ratatui rendering, layout, overlays, UI tests
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
  and at most one `Job`.
- `Panel` owns cwd, `Vec<Entry>` (with a synthetic `..` row), cursor, scroll
  offset, selected path set, and visible page size (set by the renderer).
- File operations never run on the UI thread. `transfer::spawn` returns a
  `Job` with an `mpsc::Receiver<JobMsg>`; `App::tick` drains it and refreshes
  panels on completion.
- Favorites are stored as a flat path list in TOML at
  `$YAZI2D_CONFIG_DIR/favorites.toml` or the platform config dir.
- The UI is composed with ratatui: body (favorites + two panels), status line,
  footer line; prompt/help are centered overlays.

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
- The UI thread must never perform bulk filesystem work.
- Idle redraws should not happen (`dirty` flag).
- Terminal state must be restored on normal exit and on panic.

## Key handling summary

Panel mode: `Tab` switches panel; arrows/`hjkl` navigate; `Enter`/`l` opens a
directory; `Backspace`/`h` goes up; `Space` selects and advances; `F5`/`c`
copies; `F6`/`m` moves; `F7`/`n` mkdir; `F2`/`r` renames; `F8`/`d`/`Del`
deletes; `Ctrl+B`/`a` favorites; `Ctrl+F`/`f` sidebar; `Ctrl+R` refresh;
`.` hidden files; `?` help; `q` quit.

Favorites focus: arrows/`jk`, `Enter` navigate, `Del`/`d` remove, `Esc` close.

Global: `Ctrl+C` quits, `q` quits outside prompts/help, `?`/`F1` help.

macOS F-keys may require `Fn`; letter alternatives are intentional and must be
kept.

## Testing strategy

- Unit tests live next to modules (`#[cfg(test)]`). Use `tempfile` for filesystem
  tests; never touch the user's real home/config.
- `transfer` tests wait on `Job::rx` with a timeout.
- `ui` tests render with `ratatui::backend::TestBackend` and assert buffer text.
- `scripts/e2e_pty.py` drives the real binary in a PTY: checks both panels,
  favorites, copy, help, mkdir, and clean `q` exit. It drains the PTY in a
  background thread — keep that, or the app can block on a full PTY buffer.

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
