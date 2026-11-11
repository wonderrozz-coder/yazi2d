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
matches Total Commander muscle memory. A preview pane can later be added as an
optional third column, but must not change the two-pane operation model.

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

**Status:** accepted

**Decision:** `Enter` only enters directories. On a file it shows a status
message.

**Consequences:** No accidental external programs, no editor/viewer coupling.
`F3` viewer and `F4` editor are roadmap items and should be explicit.
