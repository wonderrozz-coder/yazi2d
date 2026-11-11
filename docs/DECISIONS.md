# Architecture decisions (ADR)

Short records of decisions that shape the project. Add a new entry when a
choice would be expensive to reverse.

## ADR-001 — Standalone Rust + ratatui instead of patching Yazi's source

**Status:** accepted (2026-11-11)

**Context:** Upstream Yazi is ~82k lines of Rust with a deep single-pane
architecture (parent/current/preview) and a large plugin/config surface. The
requested product is a simplified two-pane Total Commander, not Yazi with one
extra pane. Retrofitting a second independent pane would touch tabs, layout,
rendering, watchers, preview and event routing.

**Decision:** Build Yazi2D as an independent crate using the same foundations
(Rust, ratatui, crossterm) and Yazi's design lessons, without vendoring Yazi
code.

**Consequences:** Full control and a tiny, auditable codebase; no free access
to Yazi features (preview, plugins, VFS, SSH). Upstream remains a reference and
a source of ideas, not a merge target.

## ADR-002 — Two independent panels, no preview/parent columns

**Status:** accepted

**Decision:** Each panel has its own cwd, cursor, selection and scroll offset.
The right panel is a first-class pane, never a preview. Navigation history and
tabs are out of scope for the MVP.

**Consequences:** Copy/move destination is always the other panel's cwd, which
matches Total Commander muscle memory. A preview pane was later added as a
per-panel mode (`0.2.0`, see ADR-011), not as a third column, so the two-pane
operation model is unchanged.

## ADR-003 — Never overwrite on name conflict (MVP)

**Status:** accepted

**Context:** A real overwrite/skip/rename dialog needs per-item interaction and
complicates the background job protocol.

**Decision:** Generate `name (1).ext`, `name (2).ext`, … when the destination
name already exists or is already planned in the same job. The suffix preserves
the extension.

**Consequences:** No data loss and no blocking prompts. Users cannot choose
"replace" yet; a conflict dialog is a roadmap item. This rule is an invariant:
do not silently overwrite.

## ADR-004 — Worker thread + `mpsc`, no async runtime

**Status:** accepted

**Decision:** Run copy/move/delete on a `std::thread`, report
`Progress`/`Done` through `std::sync::mpsc`. The UI polls the channel in
`App::tick`.

**Consequences:** No Tokio, no futures, no cancellation plumbing beyond an
`AtomicBool` when needed. Exactly one job at a time keeps state simple. Progress
is item-based, not byte-based; byte progress/ETA is a future enhancement.

## ADR-005 — Render only when dirty

**Status:** accepted

**Decision:** `App::dirty` is set on key input, resize, job progress and job
completion. `main::run` draws only when it is set.

**Consequences:** Idle CPU and terminal output are near zero. Any new state
mutation must set `dirty` (or be followed by a redraw-triggering event).
Background job messages set it in `tick`.

## ADR-006 — Favorites in a flat TOML file

**Status:** accepted

**Decision:** Store an ordered list of absolute paths in
`$YAZI2D_CONFIG_DIR/favorites.toml` or `dirs::config_dir()/yazi2d/favorites.toml`.
Seed with existing `~`, `Desktop`, `Documents`, `Downloads` on first run.

**Consequences:** Human-editable and trivial to test. No names, icons, groups or
ordering UI yet; duplicates are prevented by canonicalized path comparison.

## ADR-007 — F-keys plus single-letter alternatives

**Status:** accepted

**Decision:** Every important action has both a Total Commander-style F-key and
a letter: copy `F5/c`, move `F6/m`, mkdir `F7/n`, delete `F8/d`, rename `F2/r`,
favorites `Ctrl+B/a`, sidebar `Ctrl+F/f`.

**Consequences:** Usable on macOS keyboards where F-keys require `Fn` or are
captured by the system. Letter bindings must not conflict with navigation
(`h/j/k/l`, `g/G`, `q`, `?`, `.`).

## ADR-008 — Permanent delete with confirmation (MVP)

**Status:** accepted

**Decision:** `F8/d/Del` asks `Delete N item(s)? (y/N)` and then removes the
entries permanently. Symlinks are removed as links.

**Consequences:** No trash integration and no undo. A future trash backend must
keep the confirmation and report failures per item.

## ADR-009 — `..` is a synthetic display row

**Status:** accepted

**Decision:** `list_dir` prepends an `Entry { name: "..", is_parent: true }`
when a parent exists. All operation-source logic excludes `is_parent`.

**Consequences:** Total Commander-style navigation is visible and clickable in
the future. Risk: new features must remember to filter `is_parent`; use
`selection_or_current` and explicit checks.

## ADR-010 — Files are not opened in the MVP

**Status:** accepted, partially superseded by ADR-011

**Decision:** `Enter` only enters directories. On a file it shows a status
message.

**Consequences:** No accidental external programs, no editor/viewer coupling.
`F4` editor remains a roadmap item. Quick *viewing* is provided by the built-in
preview mode (`Ctrl+Q`), which never launches an external process.

## ADR-011 — Panel modes (`List` / `Thumbnails` / `Preview`) instead of a third pane

**Status:** accepted (2026-11-11, `0.2.0`)

**Context:** Total Commander's quick view turns the *other* panel into a viewer
that follows the cursor. Adding a third column would break the two-panel
operation model and shrink both lists.

**Decision:** A panel has a `PanelMode`. `Preview` shows the opposite panel's
cursor entry and is read-only. Only one panel can be in `Preview`; enabling a
second one disables the first. `mode_before_preview` restores the previous mode
when the preview closes. `Thumbnails` is a grid over the same entries.

**Consequences:** Preview follows the cursor naturally, copy/move destination is
still the other panel's cwd, and no layout changes are needed. File operations
must be explicitly blocked while the active panel is a preview. Two panels can
never both be previews, which keeps the "other panel" subject unambiguous.

## ADR-012 — Half-block truecolor rendering, no external image viewer

**Status:** accepted (2026-11-11, `0.2.0`)

**Context:** Yazi relies on terminal graphics protocols (sixel/Kitty/iTerm2) and
external helpers. That requires feature detection, escape-sequence plumbing and
terminal-specific fallbacks.

**Decision:** Decode images with the `image` crate, fit them into the panel, and
draw two pixels per cell using `▀` with foreground/background truecolor. Cache
resized buffers per path and per `(width, height)`.

**Consequences:** Works in any truecolor terminal with no external process and
no protocol detection; the binary grows to ≈2 MB. Quality is limited by cell
aspect ratio and the font. sixel/Kitty support can be added later behind the
same `ImageCache` API.

## ADR-013 — Cross-panel keys: `Ctrl+←/→` previews, `Alt+←/→` opens

**Status:** accepted (2026-11-11)

**Context:** The user wanted panel-to-panel navigation without leaving the
active panel, and accepted that this conflicts with the earlier idea of
back/forward history on the same keys.

**Decision:** The arrow points at the target panel:

- `Ctrl+←` / `Ctrl+→` — the left/right panel becomes the quick viewer of the
  active panel's cursor (pressing again closes it);
- `Alt+←` / `Alt+→` — the left/right panel opens the folder under the active
  panel's cursor (`..` is allowed; files are rejected with a status message).

**Consequences:** Fast "drill into the same folder on both sides" workflow.
History back/forward must use different bindings if it is ever added. Unit and
PTY tests cover both chords.

## ADR-014 — `Ctrl+H` toggles hidden files, `Enter` confirms deletion

**Status:** accepted (2026-11-11)

**Decision:** `Ctrl+H` is the primary hidden-files toggle, with `.` kept as an
alias for terminals that cannot report the chord. Delete confirmation accepts
`Enter` or `y` and cancels on `n`/`Esc`; the prompt text says so.

**Consequences:** Matches common file-manager muscle memory. crossterm maps the
`0x08` byte to `Char('h') + CONTROL`, so `Ctrl+H` is distinguishable from
`Backspace` (`0x7F`) in raw mode; `.` remains a fallback.

## ADR-015 — Preview is strictly read-only

**Status:** accepted (2026-11-11)

**Decision:** While the active panel is in `Preview` mode, copy/move/mkdir/
rename/delete/add-favorite are rejected with a status hint. Preview scrolling
uses arrows/`hjkl`; `Esc`, `Ctrl+Q` or the same `Ctrl+←/→` chord returns to the
previous mode.

**Consequences:** The preview subject can never be mutated by accident, and the
cursor/selection model stays tied to the list or grid.
