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

## ADR-031 — Command/Super support, Ctrl+F sidebar toggle, protected bindings

**Status:** accepted (2026-11-11, `0.4.1`)

**Context:** Four gaps surfaced in use. The footer ignored Command because the
modifier was never added to `held_modifiers`. `Ctrl+F` moved focus into the
sidebar instead of toggling it, so hiding required `Esc`. The settings menu
allowed rebinding `Ctrl+E` itself, which could lock the user out. And the
cross-panel arrows used Ctrl/Alt, both of which macOS often swallows.

**Decision:**

1. `bit_of(ModifierKeyCode)` maps every physical modifier to its bit, including
   `SUPER`/`META`; `on_key` and `on_key_release` both use it, so Command works in
   the footer and cannot get stuck.
2. `modifier_hint` builds the footer text for Command, Ctrl, Alt and Shift; it
   lists only chords the app implements.
3. `Ctrl+F` calls `toggle_favorites_sidebar` (visibility only). Focus stays on
   `§`, `Shift+Tab` and `Ctrl+D`.
4. `Shift+←/→` opens the cursor folder in the named panel; `Cmd+←/→` (Super or
   Meta) previews there. Ctrl/Alt arrows stay as aliases.
5. `Action::Settings` is removed from `Action::ALL` and `is_reserved` rejects
   `Ctrl+E` and `Esc`, so a rebind attempt reports "reserved" and changes nothing.

**Consequences:** Every modifier the terminal reports drives the footer, the
sidebar has separate visibility and focus keys, and the settings menu can never
make itself unreachable.

## ADR-032 — Confirm transfers, Alt-based cross-panel keys, footer recovery

**Status:** accepted (2026-11-11, `0.5.0`)

**Context:** Three usability problems. Copy/move started immediately while delete
asked for confirmation, which felt inconsistent for equally destructive actions.
`Cmd+←/→` never reached the app on Ghostty: the terminal binds them to
`Ctrl+A`/`Ctrl+E` ("natural text editing"), and `Option+←/→` become `ESC b`/
`ESC f`. And the footer could stay stuck on a modifier hint because a lost
release leaves `held_modifiers` set.

**Decision:**

1. `start_transfer` validates the request exactly as before, then stores
   `Prompt::Transfer`. `do_transfer` runs the job only after `Enter`/`y`;
   `n`/`Esc` cancels. The prompt shows verb, item count and destination.
2. `cross_panel_target` accepts `Alt`+arrows **and** `Alt+b`/`Alt+f`, the
   readline encodings Ghostty and iTerm2 produce. `Shift`+arrows duplicate the
   action; `Ctrl`/`Super`+arrows preview. `Alt` is the primary binding, since it
   is the one that survives on all three platforms (`Alt`/`Option`/`Alt`).
3. `App::on_key` treats a key press with no modifiers as authoritative: it
   clears `held_modifiers` and `held_keys`. That recovers from terminals that
   never deliver a release, without needing the kitty protocol.

**Consequences:** No file operation starts without explicit confirmation.
Cross-panel navigation works on Ghostty and iTerm2 out of the box, and users who
want `Cmd` can unbind Ghostty's defaults. The footer always returns to its
default text on the next keystroke.

## ADR-033 — Shift opens, Alt previews; Ctrl/Command dropped

**Status:** accepted (2026-11-11, `0.5.1`)

**Context:** ADR-032 made both modifiers do the same thing, so the two actions
could not be told apart. Ctrl+arrows switch macOS Spaces, and Ghostty rewrites
`Cmd+←/→` into `Ctrl+A`/`Ctrl+E` before the app sees them, so neither can carry
an in-app action.

**Decision:** `cross_panel_action` returns `(panel, preview)`:

- `Shift+←/→` → open the cursor folder in that panel (`preview = false`)
- `Alt+←/→`, and `Alt+b`/`Alt+f` (the readline encodings terminals emit) →
  quick preview there (`preview = true`)

`Ctrl` and `Command` are no longer consulted for cross-panel navigation.
`Ctrl+←/→` still reaches `preview_panel` through the older control-key branch as
an alias, but it is no longer documented as a primary binding.

**Consequences:** The two actions have distinct, reliable modifiers on all three
platforms. `Alt+b`/`Alt+f` remain accepted so macOS terminals work unmodified.
The footer advertises exactly this split under Shift and Option.

## ADR-034 — Modifier-aware file operations, complete key menu, open source

**Status:** accepted (2026-11-11, `0.5.2`)

**Context:** A bare Shift press appeared to start a copy. The real cause was
`handle_common_action`: it matched on `KeyCode` alone, so `Shift+C` (reported as
`Char('C')` with `SHIFT`) hit the `c` arm and began a transfer. The settings menu
also silently dropped Alt, Shift and Super because `KeySpec::from_event` kept
only Ctrl and Alt, `matches` discarded Shift, and `label`/`parse` had no `super`.
Finally the menu listed only older actions.

**Decision:**

1. `handle_common_action` takes the whole `KeyEvent`. Any held modifier makes a
   plain-letter shortcut inapplicable, so Shift/Alt/Ctrl gestures win. Function
   keys keep working with Shift, because macOS requires it for F-keys.
2. `KeySpec::from_event` records Shift and folds Super/Meta into `SUPER`;
   `matches` requires Shift when the spec names it and accepts both Super and
   Meta for a `super` spec; `label`/`parse` round-trip `super`/`cmd`.
3. Six actions were added so the menu covers every binding: `ToggleSidebar`
   (`Ctrl+F`), `OpenHere` (`Enter`/`l`), `OpenFolderThere` (`Shift+arrows`),
   `PreviewThere` (`Alt+arrows`), `Confirm` (`Enter`) and `Cancel` (`Esc`).
4. The repository gains `LICENSE` (MIT) and a `.gitignore` that excludes build
   artifacts, logs and editor droppings, so it can be published as-is.

**Consequences:** Modifiers are never mistaken for plain keys, rebinding works
for every modifier the terminal reports, and the key menu is a complete map of
the app. Publishing requires the user's own GitHub credentials; the tree is
already clean (`git status` shows no artifacts).

## ADR-035 — macOS app bundle with a generated icon

**Status:** accepted (2026-11-11, `0.5.3`)

**Context:** The user asked for a launchable app with an icon. Yazi2D is a
terminal UI: a `.app` that executes the binary directly has no PTY attached, so
it cannot start. Bitmap generation via the image tool was unavailable, and
shipping a binary blob that nobody can regenerate is poor practice.

**Decision:**

1. `packaging/render-icon.py` draws `packaging/icon-source.png` (1024×1024) with
   only the standard library: rounded gradient background, two file-list
   panels, a folder glyph and a cyan accent. The art is reproducible and
   reviewable as code.
2. `packaging/macos/build-app.sh` builds the release binary, converts the PNG
   into an `.icns` through `sips` + `iconutil`, writes `Info.plist`, ad-hoc signs
   the bundle and installs it to `/Applications`.
3. `Contents/MacOS/Yazi2D` is a small shell launcher that asks Terminal.app to
   run `Contents/Resources/bin/yazi2d` through `osascript`. That gives the
   program a real PTY, which is what a curses-style UI requires.
4. `LSUIElement` is set: the bundle needs no window of its own, only the
   Terminal window it opens.

**Consequences:** Double-clicking the icon starts Yazi2D in Terminal, and the app
appears in Launchpad, Spotlight and the Dock with its icon. `dist/` is
git-ignored because the bundle is reproducible from source. On Linux/Windows the
plain binary remains the way to run it.

## ADR-036 — The app bundle prefers a truecolor terminal

**Status:** accepted (2026-11-11, `0.5.3`)

**Context:** The first bundle launcher always used Terminal.app, which has no
truecolor support (`COLORTERM` is unset and it renders 24-bit escapes as flat
colours). Image previews and thumbnails therefore appeared as black blocks —
the same build looked correct in Ghostty.

**Decision:** `packaging/macos/launcher.sh` probes for terminals in order of
preference and launches the first one present:

```
Ghostty → kitty → WezTerm → iTerm2 → Terminal.app
```

Ghostty is started with `open -na Ghostty.app --args -e <binary>`, which the
Ghostty docs give as the way to launch the macOS build with a command. kitty and
WezTerm use their own CLI forms, iTerm2 uses AppleScript, and Terminal.app stays
as a last resort that posts a notification explaining the missing truecolor.
The launcher lives in its own file so it can be syntax-checked with `bash -n`
and reviewed without nested heredoc quoting.

**Consequences:** Double-clicking the app now opens a truecolor terminal when one
is installed, so previews work. Users without any of those terminals still get a
working app, with a hint about why images look flat.

## ADR-037 — Mouse: hit-testing in the renderer, decisions in `App`

**Status:** accepted (2026-11-12, `0.6.0`)

**Context:** Mouse capture was already enabled for trackpad scrolling and the
two-finger double tap, but clicks carried no meaning: the app only knew *that* a
pointer was pressed, never *where*. Mapping a `(column, row)` to an entry needs
the layout, and the layout only exists while a frame is being drawn.

**Decision:** The renderer records the screen rectangles it used — the whole
panel, the path-bar row, the list body, the tile size of the thumbnail grid, and
the favorites list — into `Panel`/`App`. `Panel::index_at(x, y)` then converts a
point into an entry index for the active mode; `App::on_mouse` makes the
decision. Layout is written every frame, so a resize can never leave a stale
rectangle behind.

Gestures:

* left click — focus that panel, put the cursor on the row;
* double left click — open (folder in place, file in its application);
* left click on the path bar — copy the folder path;
* right click — context menu for the entry, or for the folder on the path bar;
* wheel — unchanged scrolling.

A double click is detected on the *press* with a 400 ms window and requires the
same `(panel, row)` target, so clicking two different rows never opens anything.
`MouseEventKind::Moved`/`Drag` are dropped before touching `dirty`: capture
reports every pointer move, and repainting for those would burn CPU while the
pointer merely rests over the window.

**Consequences:** The mouse is fully usable and no key binding lost its meaning.
The rectangles are the only coupling between `ui` and `app`, and they are plain
integers, so the tests can lay out a frame and then synthesise clicks.

## ADR-038 — Clipboard through helpers, with OSC 52 as the fallback

**Status:** accepted (2026-11-12, `0.6.0`)

**Context:** Copying a path is the single most requested mouse interaction, but
a TUI owns no window and therefore no clipboard. Options were a clipboard crate
that pulls a platform dependency tree, OSC 52 only, or platform helpers.

**Decision:** `src/clipboard.rs` tries, in order: `YAZI2D_CLIPBOARD` (a custom
program reading stdin, used by tests and for overrides), the platform helpers
(`pbcopy`; `wl-copy` then `xclip` then `xsel`; `clip.exe` then PowerShell), and
finally OSC 52. Helpers are preferred because they work in a plain `conhost`
window; OSC 52 is emitted last because terminals without support ignore it
silently.

A helper is waited for up to 400 ms: `wl-copy` and `xclip` stay alive on purpose
to serve the selection, so a helper still running after that grace period is
reaped on a worker thread instead of being killed — killing it would empty the
clipboard.

**Consequences:** No new dependency, `cargo build` still produces a single
binary, and copying works on macOS, Wayland, X11 and Windows. The cost is that
the helper name is part of the status message (`Path copied (wl-copy): …`), which
makes a broken clipboard obvious instead of silent.

## ADR-039 — Search on a worker thread, streamed in batches

**Status:** accepted (2026-11-12, `0.6.0`)

**Context:** A recursive search can touch hundreds of thousands of entries. Doing
it on the UI thread would freeze the panels, and building the full result list
before showing anything would make the window feel broken.

**Decision:** `src/search.rs` walks the tree on a dedicated thread and sends
`SearchMsg` batches of up to 24 hits over an `mpsc` channel; `App::poll_search`
drains them from `tick`. The walk is a plain DFS with an explicit stack (no
recursion, no depth-limit stack overflow), never follows symlinked directories,
and stops at 1000 hits with a `truncated` flag. Matching is a case-insensitive
substring, or a wildcard mask when the query contains `*`/`?`.

The query is editable: typing clears the previous results, `Enter` starts the
search and, once results exist, opens the cursor hit — folder in place, file by
opening its parent with the cursor on the file. `Tab` opens the hit in the other
panel.

**Consequences:** Searching a huge folder keeps the UI responsive and shows
results as they arrive. The 1000-hit cap keeps memory bounded; the status line
says when it was reached, so the user can refine the query instead of trusting a
silently short list.

## ADR-040 — Windows: two targets, `-Z build-std` only for Windows 7

**Status:** accepted (2026-11-12, `0.6.0`)

**Context:** Windows 7 support was requested alongside 10/11. Rust dropped
Windows 7 as a tier-1 platform; the `*-win7-windows-gnu` target family still
builds for it, but rustup ships no prebuilt `std`, and the GNU linking step needs
MinGW-w64 import libraries.

**Decision:** One script, two artefacts:

* `x86_64-pc-windows-gnu` on a stable toolchain → Windows 10/11;
* `x86_64-win7-windows-gnu` with `+nightly -Z build-std=std,panic_abort` →
  Windows 7/8/8.1, with `-L native=<windows_x86_64_gnu crate>/lib` because that
  target imports `windows.0.48.5` from the crate source.

Both are static: no MSVC runtime, no DLLs. The produced EXEs were checked with
`objdump -p`: the win7 build imports only `kernel32`, `user32`, `advapi32`,
`ole32`, `shell32` and `msvcrt`, all present since Windows 7.

**Consequences:** Users on Windows 7 are not cut off, while the common build
stays on stable. The cost is one nightly-only build in the release process, and
the win7 build has to recompile `std` (about two minutes here). Ghostty cannot be
bundled yet: it has no official Windows build.

## ADR-041 — Native image protocols instead of half-block "pixels"

**Status:** accepted (2026-11-12, `0.6.1`)

**Context:** Half blocks (`▀` with two colours per cell) work everywhere, but
their resolution is one pixel per cell: a preview 80 cells wide is 80 pixels
wide, which is why photos looked like huge pixels. Yazi draws photographs with
the terminal's own graphics protocol and is visibly sharp.

**Decision:** `src/graphics.rs` wraps `ratatui-image` and picks a protocol from
the environment:

| Signals | Protocol |
|---|---|
| `TERM_PROGRAM=ghostty/kitty/wezterm`, `TERM=xterm-kitty/foot*` | kitty graphics |
| `TERM` mentions `sixel`/`mlterm` | sixel |
| `TERM_PROGRAM=iterm*` | iTerm2 inline images |
| anything else | half blocks |

Encoded protocols are cached per image and per terminal size (`ImageCache`),
because encoding a 1200×800 image on every frame would dominate a redraw.

**Consequences:** In Ghostty a preview frame grew from ~5 KB to ~88 KB of
escape data — i.e. real pixels instead of 80×40 half-block cells — while
scrolling stays smooth thanks to the per-size cache. Terminals without graphics
still work through half blocks.

## ADR-042 — Never query stdin for graphics support

**Status:** accepted (2026-11-12, `0.6.1`)

**Context:** The obvious way to detect a graphics protocol is to ask the
terminal: `Picker::from_query_stdio` writes DA1/XTGETTCAP queries and reads the
replies. It enables raw mode for the query and always ends by restoring the
previous state — which turns raw mode **off**. Worse, when the terminal does not
answer in time it returns an error while its worker thread stays blocked in
`read()`, so that thread competes with the event loop for every keystroke.

In a pty test the app became completely unresponsive: the first frame drew and
then no key ever arrived. Restoring raw mode after the call did not help,
because the stuck reader had already consumed the input.

**Decision:** The protocol is detected from environment variables only. No
escape-sequence query touches stdin, ever. `YAZI2D_IMAGE_PROTOCOL` overrides the
guess (`kitty`, `iterm2`, `sixel`, `halfblocks`), which also covers a
multiplexer that hides the outer terminal's identity.

**Consequences:** Detection is dumber — a terminal that supports kitty graphics
but sets none of the usual variables needs the env override — but input handling
can no longer be broken by an image feature, which is the far worse failure. The
trade is documented in the README and in the log line
(`image protocol: kitty graphics (TERM=…, TERM_PROGRAM=…)`).

## ADR-043 — Icons from Yazi's theme, with an ASCII escape hatch

**Status:** accepted (2026-11-12, `0.6.1`)

**Context:** The listing used ASCII markers (`▸`, `▣`) and the grid used
hand-drawn block art. Yazi instead shows a Nerd Font glyph per file name and
extension, coloured by type, which makes a listing readable at a glance.

**Decision:** `src/icons.rs` embeds Yazi's `[icon] dirs/files/exts` tables (14,
214 and 485 entries) as static arrays and resolves them in Yazi's order: exact
file name, then extension, then the generic folder/file glyph. Colours are the
`#rrggbb` strings from Yazi's theme. The block art stays for the thumbnail grid,
where one glyph is too small to recognise.

**Consequences:** With a Nerd Font installed the listing matches Yazi's look.
Without one the code points render as boxes, so `icons = false` in
`settings.toml` switches back to ASCII — the table itself is never a hard
dependency.

## ADR-044 — Folder facts in the listing: sub-folder count and measured size

**Status:** accepted (2026-11-12, `0.6.2`)

**Context:** A dual-pane manager is used to decide where to go next, and the two
numbers that drive that decision are "how many folders are inside" and "how big
is it". Total Commander shows `<DIR>` and makes the size an explicit action; a
plain `stat` cannot answer the second question at all, and walking a tree during
a listing would reintroduce the multi-second freezes that ADR-015 removed.

**Decision:** Two separate mechanisms, both off the UI thread:

* **Sub-folder count** — filled by the existing stat worker. `count_subdirs`
  reads only the directory entries and never `stat`s them, so it costs one
  `read_dir` per folder and lands in the same batched `StatsUpdate`. The count
  is drawn in the `Ext` column, which is otherwise meaningless for a folder.
* **Folder size** — measured on demand. `Space` (the select key, so the two
  actions cannot disagree about which row is meant) queues `panel::folder_size`,
  a depth-limited walk that skips symlinks, and the result is cached in
  `App::folder_sizes` keyed by path. The `Dir` column shows `human_size` instead
  of `<DIR>` once the number is known, and the status line repeats it.

**Consequences:** A listing never walks a tree; a user who wants a size asks for
it, and the answer survives re-listing because the cache is keyed by path. The
count distinguishes "20 folders" from "empty" at a glance, which is what makes
the `Ext` column worth its width.

## ADR-045 — Two editable slots, one resolved shortcut list

**Status:** accepted (2026-11-12, `0.6.3`)

**Context:** Key handling had grown three layers that disagreed. The settings
menu edited one key per action, `Action::defaults()` often returned three or
four, and `App::on_key` hard-coded a third set *before* the user overrides were
consulted. The consequences were visible and reported:

* rebinding `Q` to the quick preview and quitting to `Ctrl+Q` left `Q` still
  quitting, because `if key.code == Char('q') { quit }` ran before
  `custom_binding`;
* `Delete` had three defaults (`F8`, `d`, `Del`) and only one was editable, so
  two kept working with no way to change them;
* the footer showed "copy" three times, once for every duplicate;
* `F1` listed chords that did not exist (`F9 + ↑` toggles thumbnails) and missed
  the ones that did;
* `Ctrl+C` was a special case rather than an action, so it could not be rebound
  together with the rest.

**Decision:**

1. **Two slots per action**, `key.<action>.1` / `.2`, both assigned from the
   settings menu (`←→` picks the slot, `Enter` assigns, `Del` clears).
2. **`Settings::shortcuts()` is the only source of truth.** Dispatch, footer and
   help screen all render from it. `App::on_key` no longer hard-codes any
   rebindable key; when a key is not in the list it simply does nothing.
3. **Duplicate resolution lives in `settings::resolve`.** A user assignment
   beats a built-in default, a bare letter loses to a control key on the same
   combo, and the rest is resolved by a fixed action rank. Taking a key that
   another action holds only by default is allowed — that default gives way;
   taking a key the user chose for another action is refused with a message.
4. **Only two keys are not rebindable:** `F1` (help, always available, even from
   the settings menu) and the first slot of *Settings menu* (`Ctrl+E`, the way
   into the menu). Everything else, including quit and the F9 chord, is a slot.
5. **`Ctrl+C` is quit's second slot**, so `Ctrl+Q` can be freed for "quit" while
   `Ctrl+C` stays the interrupt. A bare letter and a control key are ranked, so
   `Ctrl+C` can never be stolen by a copy action.

**Consequences:** Rebinding cannot leave a stale key behind, the footer shows one
chip per action with all of its keys (`F5/C copy`), and the help screen is
generated from the same list, so it cannot drift again. The cost is that a key
which is currently assigned nowhere does nothing at all — there is no hidden
fallback, which is exactly the point.

## ADR-046 — Delete moves to the trash; erasing needs Shift

**Status:** accepted (2026-11-12, `0.6.4`)

**Context:** `Del` called `fs::remove_dir_all` directly. In this project that was
not hypothetical: browsing the favorites sidebar and pressing `Del` to remove a
bookmark deleted a real folder instead, with no way back — the system trash was
never involved and the folder was not in it either.

**Decision:** Two separate jobs:

* `JobKind::Delete` → `src/trash.rs`, which implements the
  [freedesktop.org trash spec](https://specifications.freedesktop.org/trash-spec/latest/)
  on Linux (`$XDG_DATA_HOME/Trash/files` + an `info/*.trashinfo` recording the
  original path and the deletion date; `$topdir/.Trash/$uid` for files on another
  mount, so the move stays a rename), `~/.Trash` on macOS, and
  `Shell.Application`'s recycle-bin API on Windows. This is what `Del` and `F8`
  do, and what the confirmation prompt says.
* `JobKind::Shred` → a hard delete, reachable only from `Shift+Del` or the
  "Delete permanently" context-menu entry, with a prompt that says it cannot be
  undone.

If the platform trash refuses (a read-only volume, a full trash), the job reports
an error instead of silently erasing: losing data must never be the fallback.

**Consequences:** A wrong key press now costs nothing — the entry is in the
desktop's trash UI with its original path. The cost is one extra syscall per
delete (rename instead of unlink) and a `.trashinfo` file per trashed entry, which
is the price the spec asks for.

## ADR-047 — Focus owns the key map

**Status:** accepted (2026-11-12, `0.6.4`)

**Context:** After moving dispatch into one resolved list (ADR-045), `Del` was
looked up *before* the focus branch, so with the favorites sidebar focused it
still started a file operation on the panel cursor — the exact accident that
deleted the folder. `Enter` and the arrows were swallowed the same way, which
broke navigating favorites entirely.

**Decision:** `App::on_key` resolves the focus first: `Focus::Favorites` goes to
`handle_favorites_key` and nothing else sees the key. Global dispatch runs only
for `Focus::Panels`. The favorites map now uses the same actions
(`FocusFavorites`, `AddFavorite`, `ToggleSidebar`, `OpenHere`, `Delete`, `Quit`,
`Help`) plus unbound arrows, so the same key keeps meaning "remove a bookmark"
there and "move to the trash" in the panels.

`Esc` deliberately does nothing in the sidebar: a stray Esc must not hide the
panel while browsing, which is why the panel is left with its own key.

**Consequences:** The meaning of every key now depends on where the cursor is,
which is what a dual-pane manager with a sidebar should do. A test locks the
delete case in place (`favourites_keys_are_not_hijacked_by_file_actions`).

## ADR-048 — Favorites are a list of typed rows, not a list of paths

**Status:** accepted (2026-11-12, `0.6.5`)

**Context:** The sidebar was a flat `Vec<PathBuf>`. That made four requested
things impossible without hacks: named separators (a path has no title), file
bookmarks (a path with a file works, but "reveal the file" needs a different
action from "enter the folder"), reordering (no stable row identity to keep the
cursor on) and browsing (the cursor had no way to say "show this in the panel
but do not commit").

**Decision:** `favorites::Entry { Path(PathBuf), Separator(String) }`, stored as
ordered `[[group]]` rows (`kind = "path" | "separator"`, plus `path` / `title`).
The old flat `favorites = [...]` key is still read and only written by versions
before this one, so nobody loses their list.

Behaviour follows from the row type:

* **`↑↓`** moves the cursor, skipping separators, and *reveals* the row in the
  active panel — a folder is loaded, a file is revealed with the cursor on it so
  the preview shows it. The sidebar keeps the focus, which is what makes it
  browsing rather than navigation.
* **`Enter` and `→`** commit: a folder is entered, a file opens its folder with
  the cursor on the file.
* **`PageUp/PageDown`** reorder the cursor row (the only keys free for it) and
  persist immediately, so the order the user built survives a restart.
* **`S`** adds a separator after the cursor, asking for its title.
* **`Ctrl+B`** bookmarks the cursor entry, so a file is a first-class favorite.

**Consequences:** The sidebar is now a real outline: groups with captions, rows
that can be rearranged, and entries that may point at a single document. The
footer and the settings menu read the same `Action` enum, so the sidebar's
`Del` still means "remove a bookmark" and never a file operation (ADR-047).

## ADR-049 — One "open in the other panel" action; F9 chord removed

**Status:** accepted (2026-11-13, `0.6.6`)

**Context:** Three separate things claimed to do the same job and showed up in
the settings list as three rows: `Open folder there` (`Ctrl+O`),
`Open folder there` (`Shift+→/←`, a different action with an identical label)
and the `F9` chord leader, which was not in the menu at all but worked and was
printed in the footer.

**Decision:** `Action::OpenInOther` is the only action. Its two slots are
`Ctrl+O` and `Shift+→`; `Shift+←` is accepted by the same spec through a
modifier-and-code mirror in `KeySpec::matches`, so the arrow still says which
direction the user meant while there is exactly one line in the menu. The
`OpenFolderThere` action, its label and its rows are gone.

The `F9` chord is deleted outright (`leader` state, `LEADER_WINDOW`, the status
hint, the footer chip, the `F1` line and the help text). Its one useful effect —
opening the cursor folder in the panel on the arrow's side — is what
`Milestone::OpenInOther` already does.

**Consequences:** The settings list has one row per real behaviour, and the
footer/`F1` can no longer advertise a chord that is not in the menu. A welcome
side effect: `F9` is a free, ordinary key that can be bound like any other.

## ADR-050 — `S` explains itself outside the sidebar

**Status:** accepted (2026-11-13, `0.6.6`)

**Context:** `S` adds a separator, but only while the sidebar has the focus.
Pressed in a panel it did nothing, which is indistinguishable from a broken
feature — and it was reported as exactly that.

**Decision:** `App::handle_panel_key` intercepts `S` and writes
"Separators live in the sidebar: press § or Ctrl+D to focus Favorites, then S"
into the status line. The key still does its real job inside the sidebar.

**Consequences:** A key that would otherwise be a silent no-op now teaches the
navigation it needs. Nothing else changed: `S` is not a rebindable action, and
the status line is the place the app already explains itself.

## ADR-051 — One `y2d` command as the project entry point

**Status:** accepted (2026-11-13, tooling for `0.6.6`)

**Context:** The commands needed to build, run, install, update and test the
project are long, platform-specific and easy to forget — and the project is
worked on from at least two machines (macOS and Linux) plus the agent. Every
"how do I run it again?" question was answered by reading `AGENTS.md`.

**Decision:** A single shell script, `y2d`, at the repository root, with
subcommands: `run`, `app`, `update`, `test`, `note`, `notes`, `status`, `log`,
`clean`, `setup`. It:

* resolves its own location through symlinks, so it works when linked into
  `~/.local/bin`;
* loads `$HOME/.cargo/env` when `cargo` is not on `PATH`;
* sets `CARGO_TARGET_DIR` outside the repository when it detects a synced
  folder, and otherwise asks `cargo metadata` where the target directory is;
* dispatches `app` per platform — the macOS bundle or the Linux `.desktop`
  entry;
* guards `update` with a clean-working-tree check so a pull cannot overwrite
  local work.

It deliberately has **no dependencies** beyond bash, git and Rust: `y2d` must
keep working on a bare Linux box before anything is installed.

**Consequences:** One memorable command replaces a page of instructions, and
`README.md` / `AGENTS.md` lead with it instead of with `cargo`. The manual cargo
commands remain documented for CI and for anyone who prefers them.

## ADR-052 — `y2d note` writes to `docs/FEEDBACK.md`

**Status:** accepted (2026-11-13, tooling for `0.6.6`)

**Context:** The user dictates findings in natural language and wants them to
survive until the next session, on whichever machine they are using. A chat
scrollback does not survive a machine switch, and a tracker is too much ceremony
for "the double tap in favorites feels wrong".

**Decision:** `y2d note "text"` prepends a timestamped entry to a tracked
Markdown file, `docs/FEEDBACK.md`, under a stable marker comment. `y2d notes`
prints it. Entries are newest-first because the file is an inbox, and the agent
reads it at the start of a session.

**Consequences:** Feedback lives in the repository — it syncs with `git`, so it
arrives on the other machine with `y2d update`. The marker line keeps the
rewrite idempotent and the header stable, so the file merges cleanly.

## ADR-053 — The installed `y2d` is a wrapper, and scripts are run through bash

**Status:** accepted (2026-11-13, tooling for `0.6.6`)

**Context:** This checkout lives inside a Nextcloud folder. The sync client
rewrites file modes to `644`, so `./y2d`, `packaging/macos/build-app.sh` and
`scripts/e2e_pty.py` all failed with `permission denied`, and `git status`
reported a phantom mode change on `packaging/windows/build-windows.sh`. A
symlink in `~/.local/bin` pointing at a non-executable file cannot be started
either, and `chmod` is undone again on the next sync.

**Decision:**

1. `y2d setup` installs a **generated wrapper** — `exec bash "<repo>/y2d" "$@"` —
   instead of a symlink. It only needs to be readable, whatever the sync client
   does to modes, and it always runs the current copy in the repo.
2. `y2d` itself invokes the packaging scripts through `bash`, never relying on
   their executable bit.
3. `yazi2d` in `~/.local/bin` is a symlink when the binary lives outside the
   repo (the normal case with `CARGO_TARGET_DIR`) and a copy when it does not.
4. `git config core.filemode false` is set in such a checkout, which is what
   removes the phantom mode diffs.

**Consequences:** The tools keep working inside a synced folder without fighting
the sync client on every run. Running a script as `bash script.sh` is stated
explicitly in `README.md` and `AGENTS.md` so nobody re-diagnoses it later.

## ADR-054 — The environment lock is shared by every test

**Status:** accepted (2026-11-13, tooling for `0.6.6`)

**Context:** Three modules had their own `ENV_LOCK` (or none), while
`cargo test` runs all tests in one process, in parallel. `graphics` tests set
`COLORTERM` and `TERM_PROGRAM`; a UI test that renders an image consults those
same variables. The result was a test that passed alone and failed in the full
suite — and, on this machine, three permanently red tests.

**Decision:** `src/testutil.rs` owns one process-wide `env_lock()`, plus an
`EnvGuard` that sets variables, holds the lock, and restores the previous values
on drop (including when an assertion panics). Every test that writes *or reads*
such a variable takes it. `ui` renders images under an explicit
`YAZI2D_IMAGE_PROTOCOL=halfblocks` plus `COLORTERM=truecolor`, so the result no
longer depends on the terminal that started `cargo test`.

**Consequences:** The suite is deterministic on macOS Terminal, Ghostty, a bare
`xterm` and CI. Tests that used to depend on the developer's terminal now state
their assumptions. A test that forgets the guard can still race — the rule is
"touch `std::env` only through `EnvGuard`".

## ADR-055 — A helper that outlives the grace period must not be waited for

**Status:** accepted (2026-11-13, tooling for `0.6.6`)

**Context:** `clipboard::run_helper` waits up to 400 ms for a helper to exit,
because `wl-copy` and `xclip` stay alive on purpose to serve the selection. The
unit test wrote its output file in a shell script and read it immediately, so on
a loaded machine it read before the shell had written it.

**Decision:** The test waits for the output file with a deadline instead of
asserting immediately. The production code keeps its deliberate "don't wait for
long-lived helpers" behaviour, which is what makes copying work on Wayland.

**Consequences:** The test expresses the real contract — the text *eventually*
arrives — without changing behaviour that exists for a platform reason.

## ADR-056 — Key acceleration needs proof that the terminal reports releases

**Status:** accepted (2026-11-13, `0.6.6`)

**Context:** The app moves the cursor itself while an arrow is held, because
terminal key repeat is slow and capped (ADR on fast arrows). That is only safe
if the app learns when the key is let go, which happens through
`KeyEventKind::Release` — available only with keyboard enhancement, and even then
a release can be lost.

Found while making the PTY end-to-end test reliable: pressing `j` once and never
sending a release made the cursor walk for 45 seconds, wrap past the last entry
onto `..`, and then report "Nothing selected" for `c`. The same thing happens on
Terminal.app, which reports no releases at all: hold a key, let go, and the
cursor keeps moving.

**Decision:** Two guards in `tick_key_repeat`:

1. **`releases_seen`** — the app never repeats on its own until it has observed
   at least one release event. Terminals without them keep their native repeat,
   which is slower but always stops when the key goes up.
2. **`last_arrow_activity` / `LOST_KEY_RELEASE`** — any event for the arrow
   refreshes the timestamp, and 600 ms of silence ends the hold. A key that is
   really down sends events far more often than that.

**Consequences:** A lost release can no longer move the cursor behind the user's
back, and a 45-second runaway is impossible. Ghostty/kitty/WezTerm keep the fast
accelerating cursor; Terminal.app keeps a plain, predictable repeat. The trade is
one extra field and two guards in the hot path, plus the e2e test now sending a
release like a real terminal does.

The test was also made honest: its double-tap check clicked a hard-coded row that
happened to be empty, so "the folder is still there" passed without anything
being clicked. It now looks the row up on the reconstructed screen.

## ADR-057 — The PTY test asserts on a reconstructed screen, and polls

**Status:** accepted (2026-11-13, tooling for `0.6.6`)

**Context:** The end-to-end test asserted on the raw byte stream. ratatui only
repaints changed cells, so `hello from yazi2d` never arrives as one contiguous
string — the cursor moves between runs and unchanged spaces are skipped. On top
of that, fixed `sleep()` calls raced against worker threads (preview loading,
directory listing, image decoding), which produced failures in roughly one run
out of ten.

**Decision:** The script carries a small `Screen` model that applies absolute
cursor moves, erases and printable text, and every assertion reads the composed
screen. Every wait became `until(needle, timeout)` — poll for the expected state
instead of sleeping and hoping. Keys whose effect is only valid once a prompt
exists (`c` … then `Esc`, `d` … then `Enter`) wait for that prompt first, or the
second key arrives while nothing is pending and is ignored.

**Consequences:** The suite is green run after run and failures now mean a real
behaviour change. The checks are also stronger than before, which is how the
`..`-cursor bug in ADR-056 surfaced.
