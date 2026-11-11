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

## ADR-019 — Directory listing never stats entries

**Status:** accepted (2026-11-11, `0.3.1`)

**Context:** A user log showed one folder taking 13 059 ms for 75 entries
(`Work/+Portfolio/3D`, 50–90 MB TIFFs on a Nextcloud mount). Re-running the same
directory took 0.1 s. The cause is that `fs::read_dir` plus one `metadata()` per
entry forces the macOS File Provider to materialise every cloud placeholder; the
first pass is arbitrarily slow and the work sat on the UI thread.

**Decision:**

1. `list_dir` reads only names and `DirEntry::file_type()` (which comes from the
   directory entry itself and does not stat the file). Entries start with
   `stats_loaded = false`.
2. Size and mtime are filled in later by `stat_entries`, a worker thread that
   sends `StatsUpdate` batches of 16 entries. The UI keeps rendering and shows
   `…` in the size column until a row is enriched.
3. Sorting by size/date re-runs when stats arrive, so the order converges
   without blocking.
4. Only the initial startup listing is enriched synchronously, because the very
   first frame must be complete.

**Consequences:** A 75-entry cloud folder now lists in 0 ms instead of 13 s.
Sorting by size is briefly approximated until stats land. A cancelled or
superseded load drops its stats by generation number.

## ADR-020 — Previews and image decoding run off the UI thread

**Status:** accepted (2026-11-11, `0.3.1`)

**Context:** `PreviewCache::get` read files and listed directories inside
`ui::draw`, and the directory branch called `fs::metadata` for every child. Even
after ADR-019, opening a preview of a cloud folder froze the TUI for seconds and
`Esc` could not be processed because the event loop was inside the draw call.

**Decision:**

1. `PreviewCache::get` returns `Option<&PreviewContent>`. On a miss it starts one
   worker for the requested path and returns `None`; the renderer shows
   `Loading…`. Finished content is merged on the next `tick`.
2. The directory preview uses `DirEntry::file_type()` only and reports names, so
   it never stats children.
3. `ImageCache::render` is asynchronous too, with at most four decodes in flight.
   Failures are remembered so a broken file is not retried every frame.
4. `App::tick` marks the frame dirty while a preview or decode is pending, so the
   result appears without any keypress.

**Consequences:** The UI thread only ever touches cached data. Fast scrolling
through a large folder no longer stacks up decodes. Content is one frame late,
which is invisible at 80 ms polling.

## ADR-021 — Folder previews are thumbnail grids; sorting, type colours, bulk rename

**Status:** accepted (2026-11-11, `0.3.1`)

**Context:** The user asked for the preview pane to show thumbnails when a
folder is the subject, plus sorting, colour groups with a menu, bulk rename,
`Ctrl+A`, and a cursor that stays near the deleted row.

**Decision:**

1. A preview of a folder renders the same adaptive tile grid as `Ctrl+T`
   (`draw_folder_grid`); files keep text/image/binary previews.
2. `SortKey` (`Name`, `Size`, `Modified`, `Extension`) with a reverse flag on
   `Panel`; `s`/`Ctrl+S` opens a menu, `r` flips the direction. `..` is always
   the first row and directories stay grouped before files.
3. `colors.rs` classifies entries into `FileGroup` (directory, image, video,
   audio, text, archive, temporary, model, code, other), applies one of three
   presets, and persists the choice to `$YAZI2D_CONFIG_DIR/theme`. `Ctrl+K`
   opens a preview of all groups.
4. `Ctrl+G` bulk rename supports `{name}`, `{ext}`, `{n}` and `%e=new`.
5. `Ctrl+A` selects every non-parent entry; `Esc` clears the selection.
6. After a delete the cursor is restored to the row index it had, so it stays
   next to the removed entry instead of jumping to the top.

**Consequences:** Six user requests are covered without adding dependencies.
Colours are presets, not per-extension rules; bulk rename has no preview step.

## ADR-022 — Double Tab, trackpad gestures, modifier-aware footer

**Status:** accepted (2026-11-11, `0.3.2`)

**Context:** The user wanted a `Tab`-based way into favorites without a
modifier, natural (inverted) two-finger scrolling, horizontal swipes that mirror
the arrows, and a footer that explains the chords of whichever modifier is held.

**Decision:**

1. Two `Tab` presses within 280 ms open favorites; slower presses keep switching
   panels. The window is deliberately short so bouncing between panels with
   repeated `Tab` does not get hijacked. `Shift+Tab` and `Ctrl+D` stay.
2. Mouse capture is always enabled. `ScrollUp`/`ScrollDown` move the cursor,
   with the sign inverted so the gesture drags the list. `ScrollLeft`/
   `ScrollRight` mirror `←`/`→` per mode: list → parent/open, preview → pan,
   grid → move.
3. `PushKeyboardEnhancementFlags(REPORT_EVENT_TYPES)` asks for key-release
   events. A release of a bare modifier clears it from `held_modifiers`, and the
   footer then prints only Ctrl/Alt/Shift chords in that colour. Support is
   probed by simply sending the sequence: `supports_keyboard_enhancement()`
   blocks for up to two seconds waiting for a reply, which stalled startup.

**Consequences:** Terminals without the kitty protocol ignore the push, never
send releases, and fall back to the static footer. Mouse capture means the
terminal no longer handles selection/copy itself while yazi2d runs, which is the
normal trade-off for TUI mouse support.

## ADR-023 — Thumbnail tiles get real art; the extension moves to its own column

**Status:** accepted (2026-11-11, `0.3.3`)

**Context:** In the thumbnail grid a folder showed a single `▸` in a large black
box, and any file that is not an image showed only a small grey extension label —
the tiles looked broken. In the list the extension was glued to the name
(`report.pdf`), so the name column could not be scanned by stem alone.

**Decision:**

1. Tiles paint a card background (`TILE_BG`) instead of `Color::Reset`, so empty
   space reads as a card rather than a hole.
2. Folders draw a four-row ASCII folder glyph (`FOLDER_ART`) centred in the body;
   if the tile is too small it falls back to `▸`.
3. Non-image files draw a coloured extension badge between two faint rules, in
   the colour of their `FileGroup`.
4. Each tile prints the stem on one row and the extension on the row below.
5. The list gains an `Ext` column before `Size`, shown from 40 columns wide. When
   the panel is narrower, the extension is appended to the name again so the
   file type is never lost. `header_line` switched from a fixed three-column
   layout to the same shared `build_row`.

**Consequences:** The grid is legible without any image decoding, and the list
can be scanned by name. `build_row` now takes six positional strings, which is
the upper limit before a struct argument becomes worthwhile.

## ADR-024 — `F9` chord as a fallback for swallowed arrow combinations

**Status:** accepted (2026-11-11, `0.3.4`)

**Context:** On macOS both `Ctrl+←/→` (Mission Control) and, in some terminals,
`Alt+←/→` never arrive. `Ctrl+O` and `Ctrl+T` work, but the user asked for the
same actions on an arrow plus a function key, mirroring the panel layout.

**Decision:**

1. `F9` arms a chord (`App::leader = Instant::now()`).
2. The next key is then resolved as: `←`/`→` open the cursor folder in the left
   or right panel, `↑`/`↓` toggle the thumbnail grid on the active panel.
3. `Esc`, any other key, or a `LEADER_WINDOW` (2 s) timeout cancels the chord.
4. The status line explains the pending chord, and the footer shows it while
   armed, so the mode is never invisible.

**Consequences:** Every cross-panel action now has a plain-keyboard path that no
window manager intercepts. `F9` was unused; macOS may require `Fn` for it, and
the `Ctrl+O`/`Ctrl+T` bindings remain the primary ones.

## ADR-025 — Key repeat and debounced horizontal swipes

**Status:** accepted (2026-11-11, `0.3.5`)

**Context:** Two regressions appeared after the gesture work. Holding an arrow
stopped scrolling: enabling `REPORT_EVENT_TYPES` makes a held key arrive as
`KeyEventKind::Repeat`, and the event loop only matched `Press`. Separately,
macOS emits `ScrollLeft`/`ScrollRight` inside largely vertical two-finger
gestures, so the panels dove into folders the user never meant to open.

**Decision:**

1. `main::run` handles `Press | Repeat` identically; `Release` only clears held
   modifiers.
2. `App::on_horizontal_scroll` accumulates signed distance and acts only after
   `SWIPE_THRESHOLD` (4) events, then blocks further horizontal events for
   `SWIPE_COOLDOWN` (450 ms) so one long swipe performs one action.
3. `App::on_scroll` records `last_scroll`; a horizontal event within
   `SWIPE_VERTICAL_SUPPRESS` (220 ms) of vertical scrolling resets the
   accumulator and does nothing. Stale accumulations expire after
   `SWIPE_RESET_AFTER` (350 ms).
4. The action itself moved to `act_on_swipe`, keeping the detector independent
   of what the swipe means in each panel mode.

**Consequences:** Key repeat works again, and a swipe needs a deliberate
horizontal movement. A very slow, long swipe may need a second attempt, which is
preferable to accidental navigation.

## ADR-026 — `§` replaces the double-Tab favorites gesture

**Status:** accepted (2026-11-11, `0.3.6`)

**Context:** Double `Tab` was implemented with a 280 ms window, but it competed
with the primary `Tab` action (switch panel). Pressing `Tab` twice quickly to
bounce between panels could land in the favorites sidebar instead, which felt
unreliable.

**Decision:** Remove the timing-based gesture entirely. `§` (the section sign on
the key left of `1` on most layouts) opens and closes favorites with a single
press. `Shift+Tab` and `Ctrl+D` remain as aliases, and `Tab` is once again only
"switch panel".

**Consequences:** No timing heuristics and no hidden mode. `§` may require a
layout-specific key, so the two existing aliases stay documented everywhere.

## ADR-027 — Horizontal gestures removed; double tap opens entries

**Status:** accepted (2026-11-11, `0.3.7`)

**Context:** After the debounce work the horizontal swipe still felt wrong: even
a deliberate swipe to "enter" a folder is less natural on a trackpad than a tap,
and macOS keeps injecting `ScrollLeft`/`ScrollRight` into vertical gestures. The
user asked to remove horizontal movement entirely and make a double tap with two
fingers the entry gesture.

**Decision:**

1. `MouseEventKind::ScrollLeft | ScrollRight` are ignored in `main::run`; the
   swipe detector, its thresholds and `act_on_swipe` were deleted.
2. `MouseEventKind::Down(MouseButton::Left)` calls `App::on_pointer_tap`. The
   first tap only records `last_tap`; a second one within `DOUBLE_TAP_WINDOW`
   (400 ms) calls `open_current_entry`.
3. `open_current_entry` navigates into folders and, for files, spawns the
   platform opener (`open` on macOS, `xdg-open` elsewhere, `explorer` on
   Windows) with null stdio. Failures land in the status line.

**Consequences:** No gesture can change directory accidentally. Double tap needs
mouse reporting from the terminal, which `EnableMouseCapture` already provides.
Opening files is new behaviour: it is user-initiated only and never touches the
UI thread.

## ADR-028 — Layout-independent shortcuts, Enter opens files, taps in favorites

**Status:** accepted (2026-11-11, `0.3.8`)

**Context:** Three problems shared one root: keyboard and pointer events arrive in
a form the app did not expect. With the Russian layout macOS reports `Ctrl+и`
instead of `Ctrl+B`, so every letter binding died. `Enter` on a file only printed
"not part of the MVP". And a two-finger tap produced no reaction at all, because
macOS maps it to a **right** button click while only left presses were handled.

**Decision:**

1. `src/keys.rs` maps Cyrillic characters to the Latin character on the same
   physical key (`и`→`b`, `ы`→`s`, `ё`→`` ` ``), preserving case. `App::on_key`
   normalizes before dispatch unless a prompt is open; prompts keep the raw
   character so Russian file names still work.
2. `enter_current` delegates to `open_current_entry`: folders load in place,
   files spawn the platform opener. Preview mode still rejects the action.
3. `on_pointer_tap` handles both focuses: in the sidebar a double tap opens the
   bookmark under the cursor, in the panels it opens the entry. `main::run`
   routes `MouseEventKind::Down(_)` — any button — to that handler.

**Consequences:** One set of bindings works in both layouts, files open from the
keyboard, and the trackpad gesture works on machines where a two-finger tap is a
right click. macOS does not expose "double tap with two fingers" as a distinct
event, so the gesture remains "two taps", not a single two-finger touch.

## ADR-029 — Block-art icons per file group in the thumbnail grid

**Status:** accepted (2026-11-11, `0.3.9`)

**Context:** Only images produced a thumbnail. Folders had a small glyph and every
other file drew two faint rules around a short extension label, which left most
of the tile as a black hole — the user described them as "black spots".

**Decision:**

1. New `src/icons.rs` holds a 9×6 block-art icon per `FileGroup` (folder, image,
   video, audio, text, archive, temporary, model, code, other).
2. `draw_group_icon` centres that art in the tile body and falls back to a single
   coloured glyph when the tile is smaller than the artwork.
3. Non-image files use `draw_file_icon`, which draws the group icon plus the
   extension on the last row, in the group colour.
4. A unit test asserts every icon is exactly `ICON_WIDTH × ICON_HEIGHT` in
   terminal cells, so adding a group cannot silently break the grid.

**Consequences:** Every tile is visually distinct without decoding anything, so
the grid is fast even for folders full of archives and models. Icon shapes are
ASCII/box-drawing only, which any monospace font renders consistently.

## ADR-030 — Settings menu with rebindable keys and trackpad inversion

**Status:** accepted (2026-11-11, `0.4.0`)

**Context:** Bindings were hard-coded, the trackpad direction was not adjustable,
and the folder preview (`Ctrl+Q`) still drew a single `·` for non-image entries
while the `Ctrl+T` grid already had block icons.

**Decision:**

1. `Ctrl+Q` folder tiles call `draw_group_icon`, the same helper the thumbnail
   grid uses, so both views agree.
2. `src/settings.rs` introduces `Action` (19 rebindable commands) and `KeySpec`
   (code + modifiers with `label()`/`parse()`), stored in
   `$YAZI2D_CONFIG_DIR/settings.toml` as `key.<action> = "<spec>"` plus
   `invert_scroll`. A small flat `key = value` parser avoids a TOML dependency
   for a file that only ever holds scalars.
3. An override *replaces* all default keys for that action. `App::on_key`
   consults overrides before the built-in match, so defaults keep their
   context-sensitive handling while custom bindings win outright.
4. An override is matched against the event, so the settings menu needs no
   lookup table inside every handler.
5. `App::on_scroll` applies `settings.invert_scroll` before moving, and the menu
   writes the setting back to disk immediately.

**Consequences:** Users can fix conflicts without editing source, and the
trackpad direction is personal. Rebound keys are matched greedily, so binding a
plain letter steals that letter from list navigation — the menu shows the
current binding in green to make that visible.
