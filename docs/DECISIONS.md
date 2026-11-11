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

**Status:** accepted (2026-11-11), superseded in practice by ADR-017
(macOS does not deliver these chords reliably; they remain as aliases)

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

## ADR-016 — Directory listing runs on a worker thread; optional file log

**Status:** accepted (2026-11-11, `0.3.0`)

**Context:** `Panel::reload`/`change_dir` called `fs::read_dir` plus one
`metadata()` per entry on the UI thread. On large directories, network shares and
cloud placeholders (iCloud/Nextcloud) a single `metadata()` can block for
seconds; the whole TUI freezes and there was no way to find out which path was
responsible.

**Decision:**

1. Navigation (`Enter`, `Backspace`, favorites, `Ctrl+O`, `Alt+←/→`, refresh)
   starts a background load: `Panel::spawn_load` runs `list_dir` on a thread and
   returns an `mpsc::Receiver<LoadResult>`. `App::tick` polls it and applies the
   result only if its generation matches the panel's latest request.
2. While a load is in flight the panel keeps its previous list, shows
   `Loading <path>…` in the sub-header, and rejects file operations. `Esc`
   cancels the pending load. Stale results are dropped.
3. A lightweight append-only log records startup, every directory load with its
   duration, slow loads (> 250 ms) as `WARN`, image decodes, jobs, panics and
   slow key handling. It is written to `$YAZI2D_CONFIG_DIR/yazi2d.log` (or the
   platform config dir) and is disabled with `YAZI2D_LOG=off`; a custom path can
   be set with `YAZI2D_LOG=/path/to/file`. The file is rotated to `yazi2d.log.1`
   above 4 MB at startup. A bounded queue feeds a background writer; messages
   can be dropped when it is full or the process exits abruptly.
4. Startup (`App::new`) still lists the two initial directories synchronously so
   the first frame is complete; it is logged like any other load.

**Consequences:** The UI stays responsive and a hang becomes a visible
`Loading…` state plus a log line naming the slow path. Loads can be superseded
cheaply by starting a new one; only the newest generation is applied. Tests
drive `App::tick` until loads settle (`wait_for_loads`). Preview mode shows
"Loading…" while the subject panel is still listing.

## ADR-017 — macOS-safe keys: `Ctrl+Q` previews the other panel, `Ctrl+P`/`Ctrl+O` aliases, `Shift+Tab` favorites

**Status:** accepted (2026-11-11, `0.3.0`)

**Context:** On macOS `Ctrl+←/→` is often grabbed by Mission Control and
`Alt+←/→` is not delivered by several terminals, so the cross-panel workflow was
unusable. The user also asked for a `Tab`-based way into favorites and for
`Ctrl+Q` to always preview the *opposite* panel (Total Commander semantics).

**Decision:**

- `Ctrl+Q` (alias `Ctrl+P`, because some terminals consume `Ctrl+Q` for XOFF)
  toggles a quick preview of the **other** panel; the active panel keeps its
  list/grid and its cursor, so moving it drives the preview.
- `Ctrl+O` opens the folder under the active cursor in the other panel
  (alias of the old `Alt+←/→`; `Alt+←/→` and `Ctrl+←/→` remain as aliases for
  terminals that do deliver them).
- `Shift+Tab` (`BackTab`) toggles favorites focus; `Ctrl+D` does the same, and
  pressing either key again returns to the panels. `Tab` still switches panels.
- The arrow chords are documented as "may be intercepted by the OS/terminal";
  every cross-panel action has a letter/Ctrl alternative.

**Consequences:** The whole two-panel workflow works with plain `Ctrl+letter`
chords, which terminals deliver reliably. `Ctrl+Q` no longer changes the active
panel, so the preview always mirrors the cursor the user is moving. Tests and
the PTY script use `Ctrl+P`/`Ctrl+O`/`Shift+Tab` as the primary bindings.

## ADR-018 — Image quality: Lanczos3, bounded pre-scale, adaptive tiles

**Status:** accepted (2026-11-11, `0.3.0`)

**Context:** Thumbnails and previews looked blocky. Half-block rendering gives
only one pixel per cell column and two per row, and the old code used a cheap
`Triangle` filter into a fixed 20×11 tile.

**Decision:**

1. Decode once, then immediately shrink the source so its longest side is at
   most 1600 px (`Triangle`, cheap) — this bounds the cost of later filtering.
2. Resize that bounded image to the exact target with `Lanczos3`, which keeps
   edges and text legible at small sizes. Resized buffers stay cached per
   `(path, width, height)`.
3. Thumbnail tiles are adaptive: 3 columns on wide panels, 2 on narrow ones,
   with a tile width of 22–32 cells and a height chosen so the image area is
   roughly square in pixels (`(width-2)/2 + 3` cells).
4. Keep the half-block renderer as the fallback; sixel/Kitty remain roadmap.

**Consequences:** Noticeably crisper thumbnails and previews at the same
terminal resolution; decoding cost is bounded (pre-scale is linear, Lanczos3
runs on a ≤1600 px image). The binary and memory limits are unchanged.
