use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, TimeZone};
use image::RgbImage;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus, Prompt};
use crate::colors::FileGroup;
use crate::panel::{Entry, Panel, PanelMode};
use crate::preview::{is_image_path, ImageCache, PreviewCache, PreviewContent};
use crate::settings::{Action, KeySpec};
use crossterm::event::KeyCode;

const PATH_BG_ACTIVE: Color = Color::Rgb(0, 95, 135);
const PATH_BG: Color = Color::Rgb(52, 58, 70);
const SELECT_BG_ACTIVE: Color = Color::Rgb(0, 80, 110);
const SELECT_BG: Color = Color::Rgb(55, 55, 65);
const STATUS_BG: Color = Color::Rgb(30, 32, 40);
const FOOTER_BG: Color = Color::Rgb(20, 20, 26);
/// Palette plate behind every key hint in the footer.
const CHIP_BG: Color = Color::Rgb(214, 168, 110);
/// The key itself: dark, bold, on the plate.
const CHIP_KEY: Color = Color::Rgb(32, 24, 12);
/// The description: dark but softer than the key.
const CHIP_TEXT: Color = Color::Rgb(58, 44, 24);
/// Slightly lighter than the panel so thumbnail tiles read as cards.
const TILE_BG: Color = Color::Rgb(24, 26, 33);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let body = chunks[0];
    let show_favorites = app.show_favorites && body.width >= 72;

    // A preview panel mirrors the cursor of the opposite panel.
    let left_target = app.panels[1]
        .current_entry()
        .map(|entry| entry.path.clone());
    let right_target = app.panels[0]
        .current_entry()
        .map(|entry| entry.path.clone());
    let left_loading = app.panels[1].loading;
    let right_loading = app.panels[0].loading;
    let hidden = app.hidden;
    let colors = app.colors;
    let show_icons = app.settings.icons;
    let folder_sizes = app.folder_sizes.clone();

    if show_favorites {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(26),
                Constraint::Min(20),
                Constraint::Min(20),
            ])
            .split(body);
        draw_favorites(frame, app, cols[0]);
        draw_panel(
            frame,
            &mut app.panels[0],
            left_target,
            left_loading,
            cols[1],
            app.active == 0,
            "Left",
            &mut app.preview_cache,
            &mut app.image_cache,
            hidden,
            &colors,
            show_icons,
            &folder_sizes,
        );
        draw_panel(
            frame,
            &mut app.panels[1],
            right_target,
            right_loading,
            cols[2],
            app.active == 1,
            "Right",
            &mut app.preview_cache,
            &mut app.image_cache,
            hidden,
            &colors,
            show_icons,
            &folder_sizes,
        );
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Min(20)])
            .split(body);
        draw_panel(
            frame,
            &mut app.panels[0],
            left_target,
            left_loading,
            cols[0],
            app.active == 0,
            "Left",
            &mut app.preview_cache,
            &mut app.image_cache,
            hidden,
            &colors,
            show_icons,
            &folder_sizes,
        );
        draw_panel(
            frame,
            &mut app.panels[1],
            right_target,
            right_loading,
            cols[1],
            app.active == 1,
            "Right",
            &mut app.preview_cache,
            &mut app.image_cache,
            hidden,
            &colors,
            show_icons,
            &folder_sizes,
        );
    }

    draw_status(frame, app, chunks[1]);
    draw_footer(frame, app, chunks[2]);

    if app.help {
        draw_help(frame, area, app);
    }
    if app.sort_menu {
        draw_sort_menu(frame, area, &app.panels[app.active]);
    }
    if app.theme_menu {
        draw_theme_menu(frame, area, &app.colors);
    }
    if app.settings_menu {
        draw_settings_menu(frame, area, app);
    }
    if let Some(prompt) = &app.prompt {
        draw_prompt(frame, area, prompt);
    }
    if let Some(menu) = &mut app.context_menu {
        draw_context_menu(frame, area, menu);
    }
    if let Some(search) = &mut app.search {
        draw_search(frame, area, search);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(
    frame: &mut Frame,
    panel: &mut Panel,
    preview_target: Option<PathBuf>,
    preview_loading: bool,
    area: Rect,
    active: bool,
    label: &str,
    preview_cache: &mut PreviewCache,
    image_cache: &mut ImageCache,
    show_hidden: bool,
    colors: &crate::colors::Colors,
    show_icons: bool,
    folder_sizes: &std::collections::HashMap<PathBuf, u64>,
) {
    let mode = panel.mode;
    let border_color = if active {
        Color::Cyan
    } else {
        Color::Rgb(90, 90, 100)
    };

    let title = match mode {
        PanelMode::List => format!(" {label} "),
        PanelMode::Thumbnails => format!(" {label} · thumbnails "),
        PanelMode::Preview => format!(" {label} · preview "),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    panel.screen_area = area;
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    // Remember the rows so a mouse click can be mapped back to the path bar or
    // to a specific entry.
    panel.path_area = rows[0];
    panel.body_area = rows[2];

    // The path bar is the first row of every panel.
    let path = panel.cwd.display().to_string();
    let path = truncate_left(&path, rows[0].width.saturating_sub(1) as usize);
    let path_bar = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            path,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .style(Style::default().bg(if active { PATH_BG_ACTIVE } else { PATH_BG }));
    frame.render_widget(path_bar, rows[0]);

    match mode {
        PanelMode::List => {
            if rows[1].height > 0 {
                if panel.loading {
                    draw_caption(frame, rows[1], &loading_caption(panel));
                } else {
                    frame.render_widget(
                        Paragraph::new(header_line(rows[1].width as usize))
                            .style(Style::default().fg(Color::DarkGray)),
                        rows[1],
                    );
                }
            }
            draw_list(
                frame,
                panel,
                rows[2],
                active,
                colors,
                show_icons,
                folder_sizes,
            );
        }
        PanelMode::Thumbnails => {
            panel.body_area = rows[2];
            if rows[1].height > 0 {
                let caption = if panel.loading {
                    loading_caption(panel)
                } else {
                    format!(
                        " {} items · {} · arrows move · Ctrl+T list ",
                        panel.entries.len().saturating_sub(1),
                        sort_label(panel)
                    )
                };
                draw_caption(frame, rows[1], &caption);
            }
            draw_thumbnails(frame, panel, rows[2], active, image_cache, colors);
        }
        PanelMode::Preview => {
            panel.body_area = rows[1];
            draw_preview(
                frame,
                panel,
                preview_target.as_deref(),
                preview_loading,
                rows[1],
                rows[2],
                preview_cache,
                image_cache,
                show_hidden,
                colors,
            );
        }
    }
}

fn loading_caption(panel: &Panel) -> String {
    let subject = panel.loading_label().unwrap_or_else(|| "…".to_string());
    format!(" Loading {subject}… · Esc cancels ")
}

fn sort_label(panel: &Panel) -> String {
    format!(
        "sort: {}{}",
        panel.sort_key.label(),
        if panel.sort_reverse { " ↓" } else { "" }
    )
}

fn draw_list(
    frame: &mut Frame,
    panel: &mut Panel,
    list_area: Rect,
    active: bool,
    colors: &crate::colors::Colors,
    show_icons: bool,
    folder_sizes: &std::collections::HashMap<PathBuf, u64>,
) {
    if list_area.height == 0 {
        return;
    }
    panel.page_size = list_area.height as usize;
    panel.ensure_visible();

    if panel.entries.is_empty() {
        let message = if panel.loading {
            format!("Loading {}…", panel.loading_label().unwrap_or_default())
        } else {
            panel.error.clone().unwrap_or_else(|| "(empty)".to_string())
        };
        render_message(frame, list_area, &message);
        return;
    }

    let width = list_area.width as usize;
    let items: Vec<ListItem> = panel
        .entries
        .iter()
        .map(|entry| {
            let selected = !entry.is_parent && panel.selected.contains(&entry.path);
            list_item(
                entry,
                selected,
                width,
                colors,
                show_icons,
                folder_sizes.get(&entry.path).copied(),
            )
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(if active { SELECT_BG_ACTIVE } else { SELECT_BG })
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default()
        .with_offset(panel.offset)
        .with_selected(Some(panel.cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
    panel.offset = state.offset();
}

#[allow(clippy::too_many_arguments)]
fn draw_preview(
    frame: &mut Frame,
    panel: &mut Panel,
    target: Option<&Path>,
    loading: bool,
    header: Rect,
    area: Rect,
    preview_cache: &mut PreviewCache,
    image_cache: &mut ImageCache,
    show_hidden: bool,
    colors: &crate::colors::Colors,
) {
    panel.page_size = area.height.max(1) as usize;

    if loading {
        draw_caption(frame, header, " preview · waiting for the other panel ");
        render_message(frame, area, "Loading…");
        return;
    }
    let Some(path) = target else {
        draw_caption(frame, header, " preview · move the other panel's cursor ");
        render_message(frame, area, "Nothing to preview");
        return;
    };

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let size = fs_metadata_size(path);

    if path.is_dir() {
        draw_caption(frame, header, &format!(" {name}  ·  folder "));
        let Some(content) = preview_cache.get(path, show_hidden) else {
            render_message(frame, area, "Loading folder…");
            return;
        };
        match content {
            PreviewContent::Directory { children, total } => {
                if children.is_empty() {
                    render_message(frame, area, "(empty folder)");
                    return;
                }

                let caption = if *total > children.len() {
                    format!(
                        " {name}  ·  folder  ·  {} of {total} items ",
                        children.len()
                    )
                } else {
                    format!(" {name}  ·  folder  ·  {total} items ")
                };
                draw_caption(frame, header, &caption);
                // Queue the decodes first: the renderer below draws whatever is
                // already cached this frame and shows `…` for the rest.
                for child in children.iter().filter(|child| child.is_image) {
                    image_cache.prefetch(&child.path);
                }
                draw_folder_grid(frame, panel, children, area, image_cache, colors);
            }
            PreviewContent::Error(error) => render_message(frame, area, error),
            _ => render_message(frame, area, "Cannot preview this folder"),
        }
        return;
    }

    if is_image_path(path) {
        match image_cache.decoded(path) {
            Some(image) => {
                // Without truecolor half blocks are just a dithered mess, which
                // is the "huge pixels" complaint. macOS Terminal gets a warning
                // that points at a terminal which draws the preview properly.
                if !crate::graphics::native()
                    && crate::graphics::terminal() == crate::graphics::Terminal::Limited
                {
                    draw_caption(
                        frame,
                        header,
                        &format!(" {name}  ·  image  ·  {} ", human_size(size)),
                    );
                    render_message(
                        frame,
                        area,
                        "This terminal cannot draw truecolor images.\n\nUse Ghostty, kitty or WezTerm for sharp previews,\nor press Ctrl+T for the thumbnail grid.",
                    );
                    return;
                }
                let native = crate::graphics::native();
                // With a native protocol the terminal draws real pixels, so the
                // result is roughly `area.width * 8` pixels across; half blocks
                // give one pixel per cell column. Report what is really shown.
                let pixels_wide = if native {
                    image.width().saturating_mul(8)
                } else {
                    image.width()
                };
                let dimensions = format!(
                    " {name}  ·  image {}×{}  ·  {} ",
                    pixels_wide,
                    image.height().div_ceil(2),
                    human_size(size)
                );
                draw_caption(frame, header, &dimensions);
                clamp_preview(panel, 0, 0);
                if !draw_native(frame, area, image_cache.protocol(path, area)) {
                    let halfblocks = image_cache.render(path, area.width, area.height);
                    if let Some(halfblocks) = halfblocks {
                        render_halfblocks(frame, &halfblocks, area);
                    }
                }
            }
            None => {
                // Start the background decode through the regular cache API.
                let _ = image_cache.render(path, area.width, area.height);
                draw_caption(frame, header, &format!(" {name}  ·  image "));
                let message = if image_cache.is_loading() {
                    "Decoding image…"
                } else if size > crate::preview::MAX_IMAGE_BYTES {
                    "Image is too large to decode"
                } else {
                    "Cannot decode this image"
                };
                render_message(frame, area, message);
            }
        }
        return;
    }

    let Some(content) = preview_cache.get(path, show_hidden) else {
        draw_caption(frame, header, &format!(" {name}  ·  loading "));
        render_message(frame, area, "Loading…");
        return;
    };
    match content {
        PreviewContent::Text {
            lines,
            truncated,
            size,
        } => {
            let width = lines.iter().map(|line| line.width()).max().unwrap_or(0);
            let max_scroll = lines.len().saturating_sub(area.height as usize);
            let max_scroll_x = width.saturating_sub(area.width as usize);
            clamp_preview(panel, max_scroll, max_scroll_x);

            let suffix = if *truncated { "  ·  truncated" } else { "" };
            draw_caption(
                frame,
                header,
                &format!(" {name}  ·  text  ·  {}{suffix} ", human_size(*size)),
            );

            let rendered: Vec<Line> = lines
                .iter()
                .map(|line| {
                    Line::from(Span::styled(line.clone(), Style::default().fg(Color::Gray)))
                })
                .collect();

            let text =
                Paragraph::new(rendered).scroll((panel.preview_scroll, panel.preview_scroll_x));
            frame.render_widget(text, area);
        }
        PreviewContent::Binary { size, sample } => {
            draw_caption(
                frame,
                header,
                &format!(" {name}  ·  binary  ·  {} ", human_size(*size)),
            );
            render_message(
                frame,
                area,
                &format!("Binary file ({} bytes)\n\n{sample}", size),
            );
        }
        PreviewContent::Error(error) => {
            draw_caption(frame, header, &format!(" {name}  ·  error "));
            render_message(frame, area, error);
        }
        PreviewContent::Directory { .. } => render_message(frame, area, "(folder)"),
    }
}

/// Draw a folder preview as a thumbnail grid.
///
/// The preview pane shows thumbnails when the subject is a folder; files keep
/// the normal text/image/binary preview. Decoding is asynchronous, so a tile
/// shows `…` for a frame or two before its image appears.
fn draw_folder_grid(
    frame: &mut Frame,
    panel: &mut Panel,
    children: &[crate::preview::DirChild],
    area: Rect,
    image_cache: &mut ImageCache,
    colors: &crate::colors::Colors,
) {
    const TILE_MIN_WIDTH: u16 = 18;
    const TILE_MAX_WIDTH: u16 = 28;

    if area.height == 0 || area.width == 0 {
        return;
    }

    let tile_width = (area.width / 3).clamp(TILE_MIN_WIDTH, TILE_MAX_WIDTH);
    let tile_height = ((tile_width.saturating_sub(2)) / 2 + 3).clamp(8, 18);
    let cols = (area.width / tile_width).max(1) as usize;
    let rows = (area.height / tile_height).max(1) as usize;

    let page = cols * rows;
    let max_scroll = children.len().saturating_sub(page);
    clamp_preview(panel, max_scroll, 0);

    let start = (panel.preview_scroll as usize) * cols;
    let end = (start + page).min(children.len());

    for (relative, child) in children[start..end].iter().enumerate() {
        let tile = Rect {
            x: area.x + (relative % cols) as u16 * tile_width,
            y: area.y + (relative / cols) as u16 * tile_height,
            width: tile_width,
            height: tile_height,
        };
        let color = colors.color(FileGroup::classify(&child.path, child.is_dir));

        // Same layout as the Ctrl+T grid: image thumbnail, or a block icon for
        // the file group, then the name.
        let body = Rect {
            x: tile.x + 1,
            y: tile.y,
            width: tile.width.saturating_sub(2),
            height: tile.height.saturating_sub(2),
        };
        let name_area = Rect {
            x: tile.x,
            y: tile.y + tile.height.saturating_sub(1),
            width: tile.width,
            height: 1,
        };

        if body.width > 0 && body.height > 0 {
            if child.is_dir {
                draw_group_icon(frame, body, FileGroup::Directory, color, TILE_BG);
            } else if child.is_image && body.width > 2 {
                if !draw_tile_image(frame, image_cache, &child.path, body) {
                    render_message(frame, body, "…");
                }
            } else {
                let group = FileGroup::classify(&child.path, false);
                draw_group_icon(frame, body, group, color, TILE_BG);
            }
        }

        frame.render_widget(
            Paragraph::new(truncate_right(&child.name, name_area.width as usize))
                .alignment(Alignment::Center)
                .style(Style::default().fg(color).bg(TILE_BG)),
            name_area,
        );
    }
}

fn draw_thumbnails(
    frame: &mut Frame,
    panel: &mut Panel,
    area: Rect,
    active: bool,
    image_cache: &mut ImageCache,
    colors: &crate::colors::Colors,
) {
    // Queue decodes for the tiles that are about to be drawn.
    for entry in &panel.entries {
        if !entry.is_dir && is_image_path(&entry.path) {
            image_cache.prefetch(&entry.path);
        }
    }

    const TILE_MIN_WIDTH: u16 = 16;
    const TILE_MAX_WIDTH: u16 = 32;

    if area.height == 0 || area.width == 0 {
        return;
    }

    // Adaptive tiles: 3 columns on wide panels, 2 on narrow ones. The height is
    // chosen so the image area is roughly square in pixels (2 px per cell row).
    let tile_width = (area.width / 3).clamp(TILE_MIN_WIDTH, TILE_MAX_WIDTH);
    let tile_height = ((tile_width.saturating_sub(2)) / 2 + 3).clamp(8, 22);

    let cols = (area.width / tile_width).max(1) as usize;
    let rows = (area.height / tile_height).max(1) as usize;
    panel.grid_cols = cols;
    panel.grid_rows = rows;
    panel.tile_width = tile_width;
    panel.tile_height = tile_height;
    panel.page_size = cols * rows;
    panel.ensure_grid_visible();

    if panel.entries.is_empty() {
        let message = if panel.loading {
            format!("Loading {}…", panel.loading_label().unwrap_or_default())
        } else {
            panel.error.clone().unwrap_or_else(|| "(empty)".to_string())
        };
        render_message(frame, area, &message);
        return;
    }

    let start = panel.offset * cols;
    let end = (start + cols * rows).min(panel.entries.len());

    for index in start..end {
        let relative = index - start;
        let tile = Rect {
            x: area.x + (relative % cols) as u16 * tile_width,
            y: area.y + (relative / cols) as u16 * tile_height,
            width: tile_width,
            height: tile_height,
        };
        let entry = &panel.entries[index];
        let selected = !entry.is_parent && panel.selected.contains(&entry.path);
        draw_tile(
            frame,
            entry,
            tile,
            index == panel.cursor,
            selected,
            active,
            image_cache,
            colors,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_tile(
    frame: &mut Frame,
    entry: &Entry,
    tile: Rect,
    focused: bool,
    selected: bool,
    active: bool,
    image_cache: &mut ImageCache,
    colors: &crate::colors::Colors,
) {
    let background = if focused {
        if active {
            SELECT_BG_ACTIVE
        } else {
            SELECT_BG
        }
    } else {
        TILE_BG
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(background)),
        tile,
    );

    let body = Rect {
        x: tile.x + 1,
        y: tile.y,
        width: tile.width.saturating_sub(2),
        height: tile.height.saturating_sub(2),
    };
    let name_area = Rect {
        x: tile.x,
        y: tile.y + tile.height.saturating_sub(2),
        width: tile.width,
        height: 1,
    };
    let ext_area = Rect {
        x: tile.x,
        y: tile.y + tile.height.saturating_sub(1),
        width: tile.width,
        height: 1,
    };

    if body.width > 0 && body.height > 0 {
        if entry.is_dir {
            draw_group_icon(
                frame,
                body,
                FileGroup::Directory,
                colors.color(FileGroup::Directory),
                background,
            );
        } else if is_image_path(&entry.path) {
            if !draw_tile_image(frame, image_cache, &entry.path, body) {
                render_message(frame, body, "…");
            }
        } else {
            draw_file_icon(frame, entry, body, colors, background);
        }
    }

    let marker = if selected {
        "▣ "
    } else if entry.is_dir {
        "▸ "
    } else {
        ""
    };
    let name = format!("{marker}{}", entry_stem(entry));
    let name = truncate_right(&name, name_area.width as usize);
    let style = if selected || focused {
        Style::default()
            .fg(Color::LightYellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(colors.color(FileGroup::classify(&entry.path, entry.is_dir)))
    };

    frame.render_widget(
        Paragraph::new(name)
            .alignment(Alignment::Center)
            .style(style.bg(background)),
        name_area,
    );

    // The extension sits on its own line so it never glues itself to the name.
    let extension = entry_extension(entry);
    if !extension.is_empty() {
        let style = if selected || focused {
            Style::default().fg(Color::LightYellow).bg(background)
        } else {
            Style::default().fg(Color::DarkGray).bg(background)
        };
        frame.render_widget(
            Paragraph::new(truncate_right(&extension, ext_area.width as usize))
                .alignment(Alignment::Center)
                .style(style),
            ext_area,
        );
    }
}

/// Draw the block icon for a file group, centred in `area`.
///
/// Falls back to a single coloured glyph when the tile is too small for the
/// artwork, so a tile is never empty.
fn draw_group_icon(
    frame: &mut Frame,
    area: Rect,
    group: FileGroup,
    color: Color,
    background: Color,
) {
    let art = crate::icons::art(group);
    let art_height = art.len() as u16;
    let art_width = crate::icons::ICON_WIDTH as u16;

    if area.width < art_width || area.height < art_height {
        frame.render_widget(
            Paragraph::new(crate::icons::art_glyph(group).to_string())
                .alignment(Alignment::Center)
                .style(
                    Style::default()
                        .fg(color)
                        .bg(background)
                        .add_modifier(Modifier::BOLD),
                ),
            center_vertically(area, 1),
        );
        return;
    }

    let y = area.y + (area.height - art_height) / 2;
    for (offset, line) in art.iter().enumerate() {
        let line_area = Rect {
            x: area.x,
            y: y + offset as u16,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(*line)
                .alignment(Alignment::Center)
                .style(Style::default().fg(color).bg(background)),
            line_area,
        );
    }
}

/// Non-previewable file: a large icon for its group plus the extension.
///
/// Previously this was two faint rules and a short label, which left most of the
/// tile black. The icon fills that space and makes the type recognisable.
fn draw_file_icon(
    frame: &mut Frame,
    entry: &Entry,
    area: Rect,
    colors: &crate::colors::Colors,
    background: Color,
) {
    let group = FileGroup::classify(&entry.path, false);
    let color = colors.color(group);
    let icon_height = crate::icons::ICON_HEIGHT as u16;

    let (icon_area, label_area) = if area.height > icon_height {
        (
            Rect {
                height: area.height - 1,
                ..area
            },
            Some(Rect {
                x: area.x,
                y: area.y + area.height - 1,
                width: area.width,
                height: 1,
            }),
        )
    } else {
        (area, None)
    };

    draw_group_icon(frame, icon_area, group, color, background);

    if let Some(label_area) = label_area {
        let extension = entry_extension(entry);
        let label = if extension.is_empty() {
            "file".to_string()
        } else {
            extension.trim_start_matches('.').to_uppercase()
        };
        frame.render_widget(
            Paragraph::new(truncate_right(&label, label_area.width as usize))
                .alignment(Alignment::Center)
                .style(
                    Style::default()
                        .fg(color)
                        .bg(background)
                        .add_modifier(Modifier::BOLD),
                ),
            label_area,
        );
    }
}

/// Draw an image into a tile, preferring the terminal's native protocol.
///
/// Falls back to half blocks, then to a "loading" ellipsis. Keeps the three
/// call sites (folder preview grid, thumbnail grid, single preview) identical.
fn draw_tile_image(frame: &mut Frame, cache: &mut ImageCache, path: &Path, body: Rect) -> bool {
    if draw_native(frame, body, cache.protocol(path, body)) {
        return true;
    }
    match cache.render(path, body.width, body.height) {
        Some(image) => {
            render_halfblocks(frame, &image, body);
            true
        }
        None => false,
    }
}

/// Draw an image with the terminal's native graphics protocol.
///
/// Returns `false` when the terminal has no native protocol (or the thumbnail
/// is not ready yet), and the caller then falls back to half blocks. The
/// protocol object is cached per size in `ImageCache`, because encoding a
/// 1200×800 image on every frame would dominate the redraw time.
fn draw_native(
    frame: &mut Frame,
    area: Rect,
    protocol: Option<std::rc::Rc<ratatui_image::protocol::Protocol>>,
) -> bool {
    if !crate::graphics::native() || area.width == 0 || area.height == 0 {
        return false;
    }
    let Some(protocol) = protocol else {
        return false;
    };
    if protocol.area().width == 0 || protocol.area().height == 0 {
        return false;
    }

    let rendered = Rect {
        x: area.x + (area.width - protocol.area().width) / 2,
        y: area.y + (area.height - protocol.area().height) / 2,
        ..protocol.area()
    };
    frame.render_widget(ratatui_image::Image::new(&protocol), rendered);
    true
}

fn render_halfblocks(frame: &mut Frame, image: &RgbImage, area: Rect) {
    if image.width() == 0 || image.height() == 0 || area.width == 0 || area.height == 0 {
        return;
    }

    let width = image.width().min(area.width as u32) as u16;
    let rows = image.height().div_ceil(2).min(area.height as u32) as u16;
    if width == 0 || rows == 0 {
        return;
    }

    let x0 = area.x + (area.width - width) / 2;
    let y0 = area.y + (area.height - rows) / 2;
    let buffer = frame.buffer_mut();

    for row in 0..rows {
        for col in 0..width {
            let top = image.get_pixel(col as u32, (row * 2) as u32);
            let bottom_row = ((row * 2 + 1) as u32).min(image.height() - 1);
            let bottom = image.get_pixel(col as u32, bottom_row);

            if let Some(cell) = buffer.cell_mut((x0 + col, y0 + row)) {
                cell.set_symbol("▀");
                cell.set_fg(Color::Rgb(top[0], top[1], top[2]));
                cell.set_bg(Color::Rgb(bottom[0], bottom[1], bottom[2]));
            }
        }
    }
}

fn clamp_preview(panel: &mut Panel, max_scroll: usize, max_scroll_x: usize) {
    panel.preview_max_scroll = max_scroll.min(u16::MAX as usize) as u16;
    panel.preview_max_scroll_x = max_scroll_x.min(u16::MAX as usize) as u16;
    panel.preview_scroll = panel.preview_scroll.min(panel.preview_max_scroll);
    panel.preview_scroll_x = panel.preview_scroll_x.min(panel.preview_max_scroll_x);
}

fn draw_caption(frame: &mut Frame, area: Rect, text: &str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let text = truncate_right(text, area.width as usize);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_message(frame: &mut Frame, area: Rect, message: &str) {
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn center_vertically(area: Rect, height: u16) -> Rect {
    let height = height.min(area.height);
    Rect {
        x: area.x,
        y: area.y + (area.height - height) / 2,
        width: area.width,
        height,
    }
}

fn draw_favorites(frame: &mut Frame, app: &mut App, area: Rect) {
    let active = app.focus == Focus::Favorites;
    let border_color = if active {
        Color::Yellow
    } else {
        Color::Rgb(90, 90, 100)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " Favorites ",
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(" Ctrl+B add ")
                .right_aligned()
                .style(Style::default().fg(Color::DarkGray)),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if app.favorites.paths.is_empty() {
        frame.render_widget(
            Paragraph::new("No favorites yet.\n\nPress Ctrl+B to add\nthe current folder.")
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let width = inner.width as usize;
    let items: Vec<ListItem> = app
        .favorites
        .paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            let is_current = path == &app.panels[app.active].cwd;
            let marker = if is_current { "● " } else { "  " };
            let name = truncate_right(&name, width.saturating_sub(2));
            let style = if is_current {
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(Color::LightYellow)),
                Span::styled(pad_right(&name, width.saturating_sub(2)), style),
            ]))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(if active { SELECT_BG_ACTIVE } else { SELECT_BG })
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let cursor = app
        .fav_cursor
        .min(app.favorites.paths.len().saturating_sub(1));
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(list, inner, &mut state);
    // Remember where the rows are and which row is first, so a click in the
    // sidebar selects the bookmark the user actually pointed at.
    app.favorites_area = inner;
    app.fav_offset = 0;
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (left, right) = if let Some(job) = &app.job {
        let percent = job
            .done
            .saturating_mul(100)
            .checked_div(job.total)
            .unwrap_or(0)
            .min(100);
        (
            format!(
                " {} {}% ({}/{})",
                job.kind.verb(),
                percent,
                job.done,
                job.total
            ),
            job.current.clone(),
        )
    } else {
        let panel = app.active();
        let selected = panel.selected.len();
        let left = if app.status.is_empty() {
            if selected > 0 {
                format!(" {} selected", selected)
            } else {
                format!(" {} entries", panel.entries.len().saturating_sub(1))
            }
        } else {
            format!(" {}", app.status)
        };

        let right = panel
            .current_entry()
            .map(|entry| {
                if entry.is_parent {
                    "..".to_string()
                } else {
                    // The status line repeats the row: a folder shows its
                    // measured size once Space asked for it.
                    let kind = if entry.is_dir {
                        match app.folder_sizes.get(&entry.path) {
                            Some(bytes) => format!("dir · {}", human_size(*bytes)),
                            None => "dir".to_string(),
                        }
                    } else {
                        human_size(entry.size)
                    };
                    let date = format_mtime(entry.modified);
                    if date.is_empty() {
                        format!("{}  {}", entry.name, kind)
                    } else {
                        format!("{}  {}  {}", entry.name, kind, date)
                    }
                }
            })
            .unwrap_or_default();

        (left, right)
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let left = truncate_right(&left, chunks[0].width as usize);
    let right = truncate_right(&right, chunks[1].width as usize);

    frame.render_widget(
        Paragraph::new(left).style(Style::default().bg(STATUS_BG).fg(Color::Gray)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::default().bg(STATUS_BG).fg(Color::Gray)),
        chunks[1],
    );
}

/// One footer chip: a key and what it does.
struct Hint {
    key: String,
    label: &'static str,
}

/// Short, human labels for the footer.
///
/// The settings menu uses long labels ("Copy to other panel"); a one-line footer
/// needs short ones, so each action carries both.
fn short_label(action: crate::settings::Action) -> &'static str {
    use crate::settings::Action as A;
    match action {
        A::Quit => "quit",
        A::Help => "help",
        A::Copy => "copy",
        A::Move => "move",
        A::Mkdir => "new folder",
        A::Rename => "rename",
        A::Delete => "delete",
        A::SelectAll => "select all",
        A::BulkRename => "bulk rename",
        A::ToggleHidden => "hidden",
        A::Refresh => "refresh",
        A::SortMenu => "sort",
        A::ColourMenu => "colours",
        A::Settings => "settings",
        A::TogglePreview => "preview",
        A::ToggleThumbnails => "thumbnails",
        A::OpenInOther => "open there",
        A::AddFavorite => "add fav",
        A::FocusFavorites => "favorites",
        A::ToggleSidebar => "sidebar",
        A::CopyPath => "copy path",
        A::CopyName => "copy name",
        A::CopySelection => "copy paths",
        A::SwitchPanel => "switch",
        A::OpenHere => "open",
        A::OpenFolderThere => "open there",
        A::PreviewThere => "preview there",
        A::Confirm => "confirm",
        A::Cancel => "cancel",
    }
}

/// Display priority of an action in the footer (lower comes first).
fn footer_rank(action: crate::settings::Action) -> u8 {
    use crate::settings::Action as A;
    match action {
        A::Copy => 0,
        A::Move => 1,
        A::Mkdir => 2,
        A::Rename => 3,
        A::Delete => 4,
        A::OpenHere => 5,
        A::TogglePreview => 6,
        A::ToggleThumbnails => 7,
        A::SortMenu => 8,
        A::ToggleHidden => 9,
        A::SelectAll => 10,
        A::BulkRename => 11,
        A::Refresh => 12,
        A::OpenInOther | A::OpenFolderThere => 13,
        A::PreviewThere => 14,
        A::AddFavorite => 15,
        A::FocusFavorites => 16,
        A::ToggleSidebar => 17,
        A::CopyPath => 21,
        A::CopyName => 22,
        A::CopySelection => 23,
        A::SwitchPanel => 24,
        A::ColourMenu => 18,
        A::Settings => 19,
        A::Help => 20,
        // `Ctrl+C` is the terminal's interrupt and `Enter`/`Esc` only confirm a
        // prompt; `hints_for` drops all three before ranking.
        A::Confirm | A::Cancel | A::Quit => 250,
    }
}

/// Chips for the actions bound to exactly `modifiers`.
///
/// Reads [`crate::settings::Settings::shortcuts`] — the same resolved list the
/// dispatcher uses — and shows **one chip per action**, listing all of its keys
/// (`F5/C copy`, not `F5 copy` and `C copy`). That is what keeps the footer free
/// of the "three copies of copy" effect and leaves room for other chords.
///
/// Modifiers are compared exactly: the default screen shows unmodified keys, and
/// holding Ctrl replaces it with the Ctrl screen.
fn hints_for(app: &App, modifiers: crossterm::event::KeyModifiers) -> Vec<Hint> {
    use crossterm::event::KeyModifiers as M;

    let resolved = app.settings.shortcuts();
    let mut order: Vec<(u8, crate::settings::Action)> = Vec::new();
    let mut keys: std::collections::HashMap<crate::settings::Action, Vec<String>> =
        std::collections::HashMap::new();

    for shortcut in &resolved {
        let wanted = shortcut.spec.modifiers;
        let matches_screen = match modifiers {
            M::NONE => wanted.is_empty(),
            _ => wanted == modifiers,
        };
        if !matches_screen {
            continue;
        }
        // `Ctrl+C` is the terminal's own interrupt and `Enter`/`Esc` only
        // confirm a prompt: none of them is a panel command worth a chip.
        if matches!(
            shortcut.action,
            crate::settings::Action::Quit
                | crate::settings::Action::Confirm
                | crate::settings::Action::Cancel
        ) {
            continue;
        }

        let entry = keys.entry(shortcut.action).or_default();
        if !entry.iter().any(|key| *key == chip_key(&shortcut.spec)) {
            entry.push(chip_key(&shortcut.spec));
            if entry.len() == 1 {
                order.push((footer_rank(shortcut.action), shortcut.action));
            }
        }
    }

    // `Action::Settings` is deliberately absent from `Action::ALL`, because the
    // settings menu must stay reachable, so its chips are added by hand.
    if modifiers == M::CONTROL {
        let specs: Vec<KeySpec> = (0..2)
            .filter_map(|slot| app.settings.stored_slots(crate::settings::Action::Settings)[slot])
            .collect();
        if !specs.is_empty() {
            keys.insert(
                crate::settings::Action::Settings,
                specs.iter().map(chip_key).collect(),
            );
            order.push((
                footer_rank(crate::settings::Action::Settings),
                crate::settings::Action::Settings,
            ));
        }
    }

    // A narrow footer must show the workhorses first, not `F6 move`.
    order.sort_by_key(|(rank, action)| (*rank, action.id()));

    let mut hints: Vec<Hint> = order
        .into_iter()
        .filter_map(|(_, action)| {
            let keys = keys.remove(&action)?;
            Some(Hint {
                key: keys.join("/"),
                label: short_label(action),
            })
        })
        .collect();

    // The `F9` chord has no `Action` of its own: it is documented as
    // `F9` + ←/→ only, so a single chip is enough.
    if modifiers == M::NONE {
        if let Some((index, _)) = hints
            .iter()
            .enumerate()
            .find(|(_, hint)| hint.label == "open")
        {
            hints.insert(
                index + 1,
                Hint {
                    key: "F9+←→".to_string(),
                    label: "open there",
                },
            );
        }
    }

    hints
}

/// Compact key name for a chip.
fn chip_key(spec: &KeySpec) -> String {
    let name = match spec.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_uppercase().to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Backspace => "Bksp".to_string(),
        KeyCode::Delete => "Del".to_string(),
        KeyCode::Insert => "Ins".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };

    // Modifiers are already the topic of the screen, so they are not repeated
    // on every chip; only Shift on an arrow (a different gesture) stays.
    if spec
        .modifiers
        .contains(crossterm::event::KeyModifiers::SHIFT)
        && matches!(spec.code, KeyCode::Left | KeyCode::Right | KeyCode::Tab)
    {
        return format!("Shift+{name}");
    }
    name
}

/// Width a chip occupies: key, separator, label, padding.
fn chip_width(hint: &Hint) -> usize {
    hint.key.width() + hint.label.width() + 3
}

/// Draw the footer as a row of chips.
///
/// While a modifier is held the row shows only the chords of that modifier, so
/// pressing Ctrl turns the footer into the Ctrl screen, releasing it restores
/// the default screen. A chip is a light-orange plate with dark text; the key
/// itself is bold and the label regular.
fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(FOOTER_BG)),
        area,
    );

    let hints = if app.focus == Focus::Favorites {
        vec![
            Hint {
                key: "↑↓".to_string(),
                label: "move",
            },
            Hint {
                key: "Enter".to_string(),
                label: "go",
            },
            Hint {
                key: "Del".to_string(),
                label: "remove",
            },
            Hint {
                key: "Ctrl+B".to_string(),
                label: "add",
            },
            Hint {
                key: "Esc".to_string(),
                label: "back",
            },
        ]
    } else {
        hints_for(app, app.held_modifiers)
    };

    let width = area.width as usize;
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;

    // A one-column gutter keeps the chips from touching the screen edge.
    spans.push(Span::styled(" ", Style::default().bg(FOOTER_BG)));
    used += 1;

    for hint in &hints {
        let needed = chip_width(hint) + 1;
        if used + needed > width {
            // Never draw a half chip: stop at the last complete one.
            break;
        }
        spans.push(Span::styled(
            format!(" {} ", hint.key),
            Style::default()
                .bg(CHIP_BG)
                .fg(CHIP_KEY)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{} ", hint.label),
            Style::default().bg(CHIP_BG).fg(CHIP_TEXT),
        ));
        spans.push(Span::styled(" ", Style::default().bg(FOOTER_BG)));
        used += needed;
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(4).min(80);
    let height = area.height.saturating_sub(2);
    if width == 0 || height == 0 {
        return;
    }
    let rect = centered(width, height, area);
    frame.render_widget(Clear, rect);

    let header = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::LightYellow);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Keyboard shortcuts ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Rgb(18, 18, 24)));

    // Every rebindable entry is generated from the live bindings, so F1 always
    // agrees with the settings menu and with the footer. Only the fixed keys
    // (navigation, Tab, prompt keys) are written out by hand.
    let live = |action: crate::settings::Action| {
        let label = app.settings.keys_label(action);
        if label.is_empty() {
            "—".to_string()
        } else {
            label.replace("backtab", "Shift+Tab")
        }
    };

    let mut lines = vec![
        Line::from(Span::styled("NAVIGATION", header)),
        help_line("  ↑↓ · PgUp/PgDn · Home/End", "move / page / jump", key),
        help_line("  Tab", "switch active panel", key),
        help_line("  Backspace", "parent directory", key),
        help_line("  Esc", "clear selection / close preview", key),
        Line::from(Span::styled("FILE OPERATIONS", header)),
        help_line(
            &format!("  {}", live(Action::Copy)),
            "copy to the other panel",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::Move)),
            "move to the other panel",
            key,
        ),
        help_line(&format!("  {}", live(Action::Mkdir)), "create folder", key),
        help_line(&format!("  {}", live(Action::Rename)), "rename", key),
        help_line(
            &format!("  {}", live(Action::Delete)),
            "delete (Enter or y confirms)",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::OpenHere)),
            "open folder, or file in its app",
            key,
        ),
        help_line(&format!("  {}", live(Action::SelectAll)), "select all", key),
        help_line(
            &format!("  {}", live(Action::BulkRename)),
            "bulk rename",
            key,
        ),
        Line::from(Span::styled("VIEW & PANELS", header)),
        help_line(
            &format!("  {}", live(Action::TogglePreview)),
            "quick preview in the OTHER panel",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::ToggleThumbnails)),
            "thumbnail grid",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::OpenInOther)),
            "open cursor folder in the other panel",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::OpenFolderThere)),
            "open cursor folder in that panel",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::PreviewThere)),
            "quick preview in that panel",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::SortMenu)),
            "sort menu (name/size/type/date)",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::ToggleHidden)),
            "hidden files",
            key,
        ),
        Line::from(Span::styled("FAVORITES & MISC", header)),
        help_line(
            &format!("  {}", live(Action::FocusFavorites)),
            "focus favorites / back to panels",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::AddFavorite)),
            "add current dir to favorites",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::ToggleSidebar)),
            "show / hide the sidebar",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::Refresh)),
            "refresh panels",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::ColourMenu)),
            "colour menu for file types",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::CopyPath)),
            "copy the path",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::CopyName)),
            "copy the name",
            key,
        ),
        help_line(
            &format!("  {}", live(Action::Settings)),
            "settings: slots, icons, trackpad",
            key,
        ),
        help_line(
            "  Alt+F7",
            "find files (Shift: search the other panel)",
            key,
        ),
        Line::from(Span::styled("MOUSE", header)),
        help_line("  click a row", "focus the panel and move the cursor", key),
        help_line("  double click", "open a folder, or a file in its app", key),
        help_line("  click the path bar", "copy the folder path", key),
        help_line("  right click", "menu: open, copy, rename, delete…", key),
        help_line("  wheel", "scroll the list", key),
        Line::from(Span::styled(
            format!("  {} close help", live(Action::Help)),
            Style::default().fg(Color::Gray),
        )),
    ];

    // Only whole sections fit; dropping a trailing half-line is worse than
    // dropping a line, so the list is simply cut.
    let visible: Vec<Line> = lines
        .drain(..)
        .take(rect.height.saturating_sub(2) as usize)
        .collect();
    frame.render_widget(Paragraph::new(visible).block(block), rect);
}

fn help_line(keys: &str, description: &str, key_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(keys.to_string(), key_style),
        Span::styled(format!("  {description}"), Style::default().fg(Color::Gray)),
    ])
}

/// Centered overlay listing the sort options and the active one.
fn draw_sort_menu(frame: &mut Frame, area: Rect, panel: &Panel) {
    let header = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::LightYellow);
    let active = Style::default()
        .fg(Color::LightGreen)
        .add_modifier(Modifier::BOLD);

    let rows = [
        ("n", "Name", crate::panel::SortKey::Name),
        ("s", "Size", crate::panel::SortKey::Size),
        ("t", "Type / extension", crate::panel::SortKey::Extension),
        ("d", "Date modified", crate::panel::SortKey::Modified),
    ];

    let mut lines = vec![Line::from(Span::styled("SORT ORDER", header))];
    for (binding, label, kind) in rows {
        let style = if panel.sort_key == kind { active } else { key };
        let marker = if panel.sort_key == kind { "●" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(format!("  {marker} {binding}  "), style),
            Span::styled(
                label,
                if panel.sort_key == kind {
                    active
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(help_line(
        "  r",
        if panel.sort_reverse {
            "reverse: on"
        } else {
            "reverse: off"
        },
        key,
    ));
    lines.push(help_line("  Esc", "close", key));

    draw_menu_overlay(frame, area, " Sort ", 34, lines);
}

/// Centered overlay with a preview of every file group colour.
fn draw_theme_menu(frame: &mut Frame, area: Rect, colors: &crate::colors::Colors) {
    let header = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(Span::styled(
            format!("THEME: {}", colors.preset().label().to_uppercase()),
            header,
        )),
        Line::from(""),
    ];

    for group in FileGroup::ALL {
        lines.push(Line::from(vec![
            Span::styled("  ██  ", Style::default().fg(colors.color(group))),
            Span::styled(
                format!("{:<12}", group.label()),
                Style::default().fg(colors.color(group)),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(help_line(
        "  1 / 2 / 3",
        "default / vivid / mono",
        Style::default().fg(Color::LightYellow),
    ));
    lines.push(help_line(
        "  Esc",
        "close",
        Style::default().fg(Color::LightYellow),
    ));

    draw_menu_overlay(frame, area, " Colours ", 40, lines);
}

fn draw_menu_overlay(frame: &mut Frame, area: Rect, title: &str, width: u16, lines: Vec<Line>) {
    let height = (lines.len() as u16 + 2).min(area.height);
    let width = width.min(area.width.saturating_sub(2));
    if width == 0 || height == 0 {
        return;
    }

    let rect = centered(width, height, area);
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Rgb(18, 18, 24)));

    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// Settings overlay: trackpad direction plus every rebindable action.
fn draw_settings_menu(frame: &mut Frame, area: Rect, app: &crate::app::App) {
    let header = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::LightYellow);
    let custom = Style::default().fg(Color::LightGreen);
    let muted = Style::default().fg(Color::DarkGray);
    let focused_slot = Style::default()
        .fg(Color::Black)
        .bg(Color::LightYellow)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(Span::styled("TRACKPAD & DISPLAY", header)),
        Line::from(vec![
            Span::styled("  i  ", key),
            Span::styled(
                if app.settings.invert_scroll {
                    "inverted (drag the list)  ●"
                } else {
                    "normal (scroll moves the cursor)"
                },
                Style::default().fg(Color::Gray),
            ),
        ]),
        Line::from(vec![
            Span::styled("  g  ", key),
            Span::styled(
                if app.settings.icons {
                    "Nerd Font icons  ●"
                } else {
                    "ASCII markers (no Nerd Font)"
                },
                Style::default().fg(Color::Gray),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "KEY BINDINGS · ←→ picks the slot being edited",
            header,
        )),
        Line::from(Span::styled(
            format!("    {:<24}{:<15}{}", "action", "slot 1", "slot 2"),
            muted,
        )),
    ];

    let actions = crate::settings::Action::menu_actions();
    for (index, action) in actions.iter().enumerate() {
        let selected = index == app.settings_cursor;
        let marker = if selected { "▶" } else { " " };

        let slots = app.settings.stored_slots(*action);
        let mut spans = vec![
            Span::styled(format!(" {marker} "), Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{:<24}", action.label()),
                if selected {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ];

        // Both slots are shown and both are editable; the one under the cursor
        // slot marker is highlighted so it is obvious which Enter will change.
        for (slot, entry) in slots.iter().enumerate() {
            let focused = selected && slot == app.settings_slot;
            let label = entry
                .map(|spec| spec.label())
                .unwrap_or_else(|| "—".to_string());
            let style = if focused {
                focused_slot
            } else if app.settings.is_custom(*action) {
                custom
            } else {
                key
            };
            spans.push(Span::styled(format!("{:<15}", label), style));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    if app.rebinding {
        lines.push(Line::from(Span::styled(
            format!(
                "  Press the key for slot {}…  Esc cancels",
                app.settings_slot + 1
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(help_line(
            "  ↑↓ · ←→ · Enter",
            "action · slot · assign that slot",
            muted,
        ));
        lines.push(help_line(
            "  Del · r · i/g · Esc",
            "clear slot · reset action · trackpad/icons · close",
            muted,
        ));
    }

    draw_menu_overlay(frame, area, " Settings ", 72, lines);
}

/// Right-click menu anchored at the pointer.
///
/// The rectangle is computed from the number of items and clamped to the
/// screen, so a menu opened near the bottom or right edge stays visible. The
/// rectangle is stored back into the menu so the click handler can map a row
/// back to an item.
fn draw_context_menu(frame: &mut Frame, area: Rect, menu: &mut crate::app::ContextMenu) {
    let width = menu
        .items
        .iter()
        .map(|item| item.label().len())
        .max()
        .unwrap_or(10) as u16
        + 6;
    let width = width.min(area.width.saturating_sub(2)).max(16);
    let height = (menu.items.len() as u16 + 2).min(area.height);
    if width == 0 || height == 0 {
        return;
    }

    // Open below-right of the pointer, flipping when there is no room.
    let x = if menu.x + width + 1 > area.right() {
        area.right().saturating_sub(width + 1)
    } else {
        menu.x
    };
    let y = if menu.y + height + 1 > area.bottom() {
        menu.y.saturating_sub(height)
    } else {
        menu.y
    };
    let rect = Rect {
        x: x.max(area.x),
        y: y.max(area.y),
        width,
        height,
    };
    menu.rect = rect;

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Actions ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Rgb(18, 18, 24)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let lines: Vec<Line> = menu
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let selected = index == menu.cursor;
            let marker = if selected { "▶ " } else { "  " };
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(vec![
                Span::styled(marker, Style::default().fg(Color::Cyan)),
                Span::styled(
                    truncate_right(item.label(), inner.width as usize - 2),
                    style,
                ),
            ])
        })
        .take(inner.height as usize)
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// `Alt+F7` search window: query on top, hits below.
fn draw_search(frame: &mut Frame, area: Rect, search: &mut crate::app::SearchWindow) {
    let width = area.width.saturating_sub(8).clamp(40, 96);
    let height = area.height.saturating_sub(2).max(8);
    if width == 0 || height == 0 {
        return;
    }
    let rect = centered(width, height, area);
    frame.render_widget(Clear, rect);

    let state = if search.running {
        format!("searching… {} entries", search.scanned)
    } else if search.hits.is_empty() {
        "no results yet · Enter starts the search".to_string()
    } else {
        format!(
            "{} result(s) in {} entries{}",
            search.hits.len(),
            search.scanned,
            if search.truncated {
                " (limit reached)"
            } else {
                ""
            }
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" Find in {} ", search.root.display()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(format!(
                " Enter open · Tab open there · F5 search · Esc close · {state} "
            ))
            .style(Style::default().fg(Color::DarkGray)),
        )
        .style(Style::default().bg(Color::Rgb(18, 18, 24)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let query_area = Rect { height: 1, ..inner };
    let list_area = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    search.list_area = list_area;
    search.ensure_visible();

    let query = Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Yellow)),
        Span::styled(search.query.clone(), Style::default().fg(Color::White)),
        Span::styled("▌", Style::default().fg(Color::Yellow)),
    ]);
    frame.render_widget(Paragraph::new(query), query_area);

    if list_area.height == 0 {
        return;
    }

    let page = list_area.height as usize;
    let start = search.offset.min(search.hits.len());
    let end = (start + page).min(search.hits.len());
    let width = list_area.width as usize;

    let lines: Vec<Line> = search.hits[start..end]
        .iter()
        .enumerate()
        .map(|(relative, hit)| {
            let index = start + relative;
            let selected = index == search.cursor;
            let marker = if selected { "▶ " } else { "  " };
            let name = hit
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| hit.path.display().to_string());

            // Show the folder relative to the search root, so hits from
            // different subfolders stay distinguishable.
            let parent = hit.path.parent().unwrap_or(search.root.as_path());
            let folder = parent
                .strip_prefix(&search.root)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| parent.display().to_string());
            let folder = if folder.is_empty() {
                ".".to_string()
            } else {
                folder
            };

            let icon = if hit.is_dir { "▸ " } else { "  " };
            let name_style = if selected {
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD)
            } else if hit.is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };

            let used = marker.len() + icon.len() + name.width();
            let room = width.saturating_sub(used + 2);
            Line::from(vec![
                Span::styled(marker, Style::default().fg(Color::Cyan)),
                Span::styled(icon, name_style),
                Span::styled(truncate_right(&name, width.saturating_sub(6)), name_style),
                Span::styled(
                    format!("   {}", truncate_right(&folder, room)),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), list_area);
}

fn draw_prompt(frame: &mut Frame, area: Rect, prompt: &Prompt) {
    let (title, preferred_width, preferred_height) = match prompt {
        Prompt::Mkdir { .. } => (" New folder ", 62, 3),
        Prompt::Rename { .. } => (" Rename ", 62, 3),
        Prompt::BulkRename { .. } => (" Bulk rename ", 72, 5),
        Prompt::Delete { .. } => (" Confirm delete ", 64, 5),
        Prompt::Transfer { kind, .. } => match kind {
            crate::transfer::JobKind::Copy => (" Confirm copy ", 68, 6),
            _ => (" Confirm move ", 68, 6),
        },
    };

    let width = preferred_width.min(area.width.saturating_sub(2));
    let height = preferred_height.min(area.height);
    if width == 0 || height == 0 {
        return;
    }

    let rect = centered(width, height, area);
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    match prompt {
        Prompt::Mkdir { buffer } | Prompt::Rename { buffer, .. } => {
            let line = Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::Yellow)),
                Span::styled(buffer.clone(), Style::default().fg(Color::White)),
                Span::styled("▌", Style::default().fg(Color::Yellow)),
            ]);
            frame.render_widget(Paragraph::new(line), inner);

            let x = inner.x + 2 + buffer.width() as u16;
            let y = inner.y;
            frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
        }
        Prompt::BulkRename { buffer, sources } => {
            let hint = format!(
                "{} item(s) · {{name}} base · {{ext}} ext · {{n}} counter · %e=ext",
                sources.len()
            );
            let lines = vec![
                Line::from(vec![
                    Span::styled("> ", Style::default().fg(Color::Yellow)),
                    Span::styled(buffer.clone(), Style::default().fg(Color::White)),
                    Span::styled("▌", Style::default().fg(Color::Yellow)),
                ]),
                Line::from(""),
                Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
            ];
            frame.render_widget(Paragraph::new(lines), inner);

            let x = inner.x + 2 + buffer.width() as u16;
            frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y));
        }
        Prompt::Transfer {
            kind,
            sources,
            dest,
        } => {
            let verb = match kind {
                crate::transfer::JobKind::Copy => "Copy",
                _ => "Move",
            };
            let text = format!(
                "{verb} {} item(s) to:\n{}\n\nPress Enter or y to confirm, n/Esc to cancel.",
                sources.len(),
                dest.display()
            );
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::Gray))
                    .wrap(Wrap { trim: true }),
                inner,
            );
        }
        Prompt::Delete { sources } => {
            let text = format!(
                "Delete {} item(s)?\n\nPress Enter or y to confirm, n/Esc to cancel.",
                sources.len()
            );
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::Gray))
                    .wrap(Wrap { trim: true }),
                inner,
            );
        }
    }
}

/// Name without its extension, plus `@` for symlinks and `/` for folders.
fn entry_stem(entry: &Entry) -> String {
    if entry.is_parent {
        return "..".to_string();
    }

    let stem = entry
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&entry.name)
        .to_string();

    if entry.is_symlink {
        if entry.is_dir {
            format!("{stem}@/")
        } else {
            format!("{stem}@")
        }
    } else if entry.is_dir {
        format!("{stem}/")
    } else {
        stem
    }
}

/// Extension including the dot, or an empty string when there is none.
fn entry_extension(entry: &Entry) -> String {
    if entry.is_parent || entry.is_dir {
        return String::new();
    }
    entry
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_lowercase()))
        .unwrap_or_default()
}

/// Marker shown in front of a selected row, whatever icon mode is active.
const SELECTED_MARKER: &str = "▣ ";

fn list_item(
    entry: &Entry,
    selected: bool,
    width: usize,
    colors: &crate::colors::Colors,
    show_icons: bool,
    folder_size: Option<u64>,
) -> ListItem<'static> {
    let group = FileGroup::classify(&entry.path, entry.is_dir);
    let group_color = colors.color(group);

    // The icon column mirrors Yazi: a per-type Nerd Font glyph in the type's
    // own colour. A selected row keeps its marker so multi-select stays
    // readable, and `--no-icons` falls back to the ASCII arrows.
    let (prefix, prefix_style) = if selected {
        (
            SELECTED_MARKER.to_string(),
            Style::default().fg(Color::LightYellow),
        )
    } else if show_icons {
        let icon = crate::icons::resolve(&entry.name, &entry.path, entry.is_dir, entry.is_parent);
        (
            format!("{} ", icon.glyph),
            Style::default().fg(crate::icons::color(icon.color)),
        )
    } else if entry.is_parent {
        ("↑ ".to_string(), Style::default().fg(Color::DarkGray))
    } else if entry.is_dir {
        ("▸ ".to_string(), Style::default().fg(group_color))
    } else {
        ("  ".to_string(), Style::default())
    };

    let name = if entry.is_parent {
        "..".to_string()
    } else {
        entry_stem(entry)
    };
    // A folder has no extension, so that column carries how many folders it
    // contains — the one number Yazi users ask for when scanning a tree.
    let extension = if entry.is_dir && !entry.is_parent {
        match entry.subdirs {
            Some(count) => format!("{count}"),
            None => "…".to_string(),
        }
    } else {
        entry_extension(entry)
    };

    let size = if entry.is_parent {
        String::new()
    } else if entry.is_dir {
        // `Dir` shows the measured size once Space asked for it, `<DIR>`
        // otherwise, so the column keeps its Total Commander meaning.
        match folder_size {
            Some(bytes) => human_size(bytes),
            None => "<DIR>".to_string(),
        }
    } else if entry.stats_loaded {
        human_size(entry.size)
    } else {
        "…".to_string()
    };

    let date = format_mtime(entry.modified);

    let name_style = if selected {
        Style::default().fg(Color::LightYellow)
    } else if entry.is_parent {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(group_color)
    };

    let styles = RowStyles {
        prefix: prefix_style,
        name: name_style,
        extension: Style::default().fg(Color::DarkGray),
        detail: Style::default().fg(Color::DarkGray),
    };
    ListItem::new(build_row(
        width, &prefix, &name, &extension, &size, &date, styles,
    ))
}

fn header_line(width: usize) -> Line<'static> {
    build_row(
        width,
        "  ",
        "Name",
        "Ext",
        "Size",
        "Modified",
        RowStyles {
            prefix: Style::default(),
            name: Style::default().add_modifier(Modifier::BOLD),
            extension: Style::default().add_modifier(Modifier::BOLD),
            detail: Style::default(),
        },
    )
}

#[derive(Clone, Copy)]
struct RowStyles {
    prefix: Style,
    name: Style,
    extension: Style,
    detail: Style,
}

fn build_row(
    width: usize,
    prefix: &str,
    name: &str,
    extension: &str,
    size: &str,
    date: &str,
    styles: RowStyles,
) -> Line<'static> {
    let show_date = width >= 52;
    let show_size = width >= 34;
    let show_ext = width >= 40;
    let detail_width = (if show_size { 8 } else { 0 })
        + (if show_size && show_date { 1 } else { 0 })
        + (if show_date { 16 } else { 0 })
        + (if show_ext { 7 } else { 0 });
    let name_width = width.saturating_sub(2 + detail_width);

    // Without a dedicated column the extension stays part of the name, so the
    // file type is never hidden.
    let full_name = if show_ext || extension.is_empty() {
        name.to_string()
    } else {
        format!("{name}{extension}")
    };
    let name = truncate_right(&full_name, name_width);
    let mut spans = vec![
        Span::styled(prefix.to_string(), styles.prefix),
        Span::styled(pad_right(&name, name_width), styles.name),
    ];

    if show_ext {
        let extension = truncate_right(extension, 6);
        spans.push(Span::styled(format!("{extension:<7}"), styles.extension));
    }
    if show_size {
        spans.push(Span::styled(format!("{size:>8}"), styles.detail));
    }
    if show_date {
        spans.push(Span::styled(format!(" {date:<16}"), styles.detail));
    }

    Line::from(spans)
}

/// Size of a file without blocking the UI: directories and unknown paths report 0.
fn fs_metadata_size(path: &Path) -> u64 {
    if path.is_dir() {
        return 0;
    }
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else if size >= 10.0 {
        format!("{size:.0}{}", UNITS[unit])
    } else {
        format!("{size:.1}{}", UNITS[unit])
    }
}

fn format_mtime(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return String::new();
    };
    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let seconds = duration.as_secs() as i64;

    match Local.timestamp_opt(seconds, 0).single() {
        Some(datetime) => datetime.format("%Y-%m-%d %H:%M").to_string(),
        None => String::new(),
    }
}

fn truncate_right(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.width() <= max {
        return text.to_string();
    }

    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > max.saturating_sub(1) {
            break;
        }
        result.push(ch);
        used += width;
    }
    result.push('…');
    result
}

fn truncate_left(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.width() <= max {
        return text.to_string();
    }

    let mut reversed = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > max.saturating_sub(1) {
            break;
        }
        reversed.push(ch);
        used += width;
    }
    reversed.reverse();
    format!("…{}", reversed.into_iter().collect::<String>())
}

fn pad_right(text: &str, width: usize) -> String {
    let used = text.width();
    if used >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - used))
    }
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::fs;

    fn render(app: &mut App, width: u16, height: u16) -> String {
        render_until(app, width, height, |text| !text.is_empty())
    }

    /// Keep drawing until `done` accepts the frame.
    ///
    /// Previews and image decodes are asynchronous now, so the first frame is a
    /// "loading" placeholder. This mirrors what the real event loop does: the
    /// background worker finishes and the next frame shows the content.
    fn render_until(app: &mut App, width: u16, height: u16, done: impl Fn(&str) -> bool) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.tick();
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, app)).unwrap();
            let last: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if done(&last) || std::time::Instant::now() > deadline {
                return last;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn list_separates_the_extension_into_its_own_column() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "hello").unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].select_name("notes.txt");

        let width = 140usize;
        let text = render(&mut app, width as u16, 30);
        assert!(text.contains("Ext"), "extension column header missing");
        // `render` returns the raw cell stream, so rows are fixed-width chunks.
        let rows: Vec<String> = text
            .chars()
            .collect::<Vec<_>>()
            .chunks(width)
            .map(|chunk| chunk.iter().collect())
            .collect();
        let row = rows
            .iter()
            .find(|line| line.contains("notes"))
            .expect("notes row");
        // The left panel starts after the 26-column favorites sidebar; inspect
        // that panel's name cell only.
        let panel_row: String = row.chars().skip(28).take(30).collect();
        assert!(
            panel_row.contains("notes") && !panel_row.contains("notes.txt"),
            "extension still glued to the name: {panel_row:?}"
        );
        assert!(
            row.contains(".txt"),
            "extension column missing .txt: {row:?}"
        );
    }

    #[test]
    fn narrow_panels_keep_the_extension_with_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "hello").unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].select_name("notes.txt");

        // 80 columns with the favorites bar leaves each panel too narrow for
        // the Ext column; the extension must still be visible.
        let text = render(&mut app, 80, 30);
        assert!(
            text.contains("notes.tx"),
            "extension disappeared on a narrow panel"
        );
    }

    #[test]
    fn thumbnail_tiles_draw_a_folder_glyph_and_extension_badge() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("projects")).unwrap();
        fs::write(tmp.path().join("report.pdf"), "x").unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].set_mode(PanelMode::Thumbnails);

        let text = render(&mut app, 140, 40);
        assert!(text.contains("█"), "folder tile has no folder glyph");
        assert!(text.contains("PDF"), "non-image tile has no ext badge");
        assert!(text.contains("projects"), "folder name missing");
        assert!(text.contains("report"), "file name missing");
    }

    #[test]
    fn shows_two_panels_paths_files_and_favorites() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("leftdir");
        let right = tmp.path().join("rightdir");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("alpha.txt"), "hello").unwrap();

        let mut app = App::new(vec![left, right.clone()]);
        app.favorites.paths.clear();
        app.favorites.add(right);

        let text = render(&mut app, 120, 30);

        assert!(text.contains("Favorites"));
        assert!(text.contains("Left"));
        assert!(text.contains("Right"));
        assert!(text.contains("alpha.txt"));
        assert!(text.contains("leftdir"));
        assert!(text.contains("rightdir"));
    }

    #[test]
    fn shows_mkdir_prompt_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::new(vec![tmp.path().to_path_buf()]);
        app.prompt = Some(Prompt::Mkdir {
            buffer: "newdir".to_string(),
        });

        let text = render(&mut app, 100, 30);
        assert!(text.contains("New folder"));
        assert!(text.contains("newdir"));
    }

    #[test]
    fn shows_help_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::new(vec![tmp.path().to_path_buf()]);
        app.help = true;

        let text = render(&mut app, 100, 34);
        assert!(text.contains("Keyboard shortcuts"));
        // The help screen prints live labels, so chips read `ctrl+b`.
        assert!(text.contains("ctrl+b"), "favorite chord missing: {text}");
        assert!(text.contains("ctrl+q"), "preview chord missing: {text}");
        assert!(
            text.contains("alt+right"),
            "cross-panel chord missing: {text}"
        );
        // The removed undocumented chord must be gone.
        assert!(!text.contains("F9 + ↑"), "stale F9 chord is still listed");
    }

    #[test]
    fn renders_a_text_preview_of_the_other_panel_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "hello preview\nsecond line\n").unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].select_name("notes.txt");
        app.panels[0].set_mode(PanelMode::Preview);
        app.panels[1].select_name("notes.txt");

        let text = render_until(&mut app, 120, 30, |text| text.contains("hello preview"));
        assert!(text.contains("preview"));
        assert!(text.contains("notes.txt"));
        assert!(text.contains("hello preview"));
    }

    #[test]
    fn renders_a_loading_state_while_a_directory_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].loading = true;
        app.panels[0].loading_path = Some(tmp.path().join("slow-folder"));

        let text = render(&mut app, 120, 30);
        assert!(text.contains("Loading"), "loading caption missing");
        assert!(text.contains("slow-folder"));
    }

    #[test]
    fn preview_of_a_folder_shows_a_thumbnail_grid() {
        let tmp = tempfile::tempdir().unwrap();
        let inner = tmp.path().join("gallery");
        fs::create_dir(&inner).unwrap();
        let image = RgbImage::from_pixel(8, 8, image::Rgb([240, 120, 20]));
        image.save(inner.join("shot.png")).unwrap();
        fs::create_dir(inner.join("nested")).unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].select_name("gallery");
        app.panels[0].set_mode(PanelMode::Preview);
        app.panels[1].select_name("gallery");

        // The folder listing and the image decode are both asynchronous.
        let text = render_until(&mut app, 120, 40, |text| {
            text.contains("shot.png") && text.contains("▀")
        });
        assert!(text.contains("folder"), "folder caption missing");
        assert!(text.contains("shot.png"), "image name missing");
        assert!(text.contains("nested"), "subfolder name missing");
        assert!(
            text.contains("▀"),
            "folder preview did not draw image thumbnails"
        );
    }

    #[test]
    fn renders_a_thumbnail_grid_with_file_names() {
        let tmp = tempfile::tempdir().unwrap();
        let image = RgbImage::from_pixel(4, 4, image::Rgb([200, 40, 40]));
        image.save(tmp.path().join("red.png")).unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].set_mode(PanelMode::Thumbnails);

        let text = render_until(&mut app, 120, 40, |text| text.contains("▀"));
        assert!(text.contains("thumbnails"));
        assert!(text.contains("red.png"));
        assert!(
            text.contains("▀"),
            "image tile was not drawn with half blocks"
        );
    }

    #[test]
    fn renders_an_image_preview_with_half_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let image = RgbImage::from_pixel(8, 8, image::Rgb([30, 120, 220]));
        image.save(tmp.path().join("blue.png")).unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].select_name("blue.png");
        app.panels[0].set_mode(PanelMode::Preview);
        app.panels[1].select_name("blue.png");

        let text = render_until(&mut app, 120, 30, |text| text.contains("▀"));
        assert!(text.contains("image"));
        assert!(text.contains("blue.png"));
        assert!(
            text.contains("▀"),
            "image preview was not drawn with half blocks"
        );
    }

    /// Flatten a rendered buffer into lines, so a test can look at the footer.
    /// An app whose settings are the built-in defaults.
    ///
    /// `App::new` loads the user's real `settings.toml`; a footer test must not
    /// depend on whether the developer rebound a key on their machine.
    fn default_app(dir: &Path) -> App {
        let mut app = App::new(vec![dir.to_path_buf(), dir.to_path_buf()]);
        app.settings = crate::settings::Settings::defaults();
        app
    }

    fn footer_line(app: &mut App, width: u16, height: u16) -> String {
        let text = render(app, width, height);
        let row = (height as usize - 1) * width as usize;
        text.chars().skip(row).take(width as usize).collect()
    }

    #[test]
    fn the_footer_shows_chips_with_the_live_bindings() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = default_app(tmp.path());

        // Wide enough for every default chip; the real screen truncates instead.
        let line = footer_line(&mut app, 240, 30);
        // Default screen: unmodified keys only, in chips.
        assert!(line.contains("F5"), "copy chip missing: {line:?}");
        assert!(line.contains("copy"), "copy label missing: {line:?}");
        assert!(line.contains("F7"), "mkdir chip missing: {line:?}");
        assert!(line.contains("hidden"), "hidden chip missing: {line:?}");
        // Ctrl-only chords must not leak into the default screen.
        assert!(!line.contains("Ctrl"), "default screen shows Ctrl chords");
    }

    #[test]
    fn holding_ctrl_switches_to_the_ctrl_screen_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = default_app(tmp.path());

        app.held_modifiers = crossterm::event::KeyModifiers::CONTROL;
        let line = footer_line(&mut app, 240, 30);
        assert!(
            line.contains("fav"),
            "the Ctrl screen must still show the favorite chords: {line:?}"
        );
        assert!(line.contains("settings"), "Ctrl+E chip missing: {line:?}");
        assert!(line.contains("preview"), "Ctrl+Q chip missing: {line:?}");
        // Function keys without Ctrl belong to the default screen.
        assert!(!line.contains("F5"), "Ctrl screen shows F5: {line:?}");

        app.held_modifiers = crossterm::event::KeyModifiers::ALT;
        let line = footer_line(&mut app, 240, 30);
        assert!(line.contains("preview there"), "Alt screen: {line:?}");
        assert!(
            !line.contains("favorites"),
            "Alt screen shows Ctrl keys: {line:?}"
        );
    }

    #[test]
    fn a_rebound_key_changes_the_chip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = default_app(tmp.path());
        app.settings
            .assign(crate::settings::Action::Copy, 0, KeySpec::f(9))
            .unwrap();

        let line = footer_line(&mut app, 160, 30);
        assert!(line.contains("F9"), "rebound key missing: {line:?}");
        assert!(!line.contains("F5"), "old key still shown: {line:?}");
    }

    /// Extract the row of a rendered buffer that contains `needle`.
    fn row_with(text: &str, width: u16, needle: &str) -> String {
        text.chars()
            .collect::<Vec<_>>()
            .chunks(width as usize)
            .map(|chunk| chunk.iter().collect::<String>())
            .find(|line| line.contains(needle))
            .unwrap_or_default()
    }

    #[test]
    fn a_folder_row_shows_its_sub_folder_count_in_the_ext_column() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("Projects");
        fs::create_dir_all(folder.join("one")).unwrap();
        fs::create_dir_all(folder.join("two")).unwrap();
        fs::create_dir_all(folder.join("three")).unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.settings = crate::settings::Settings::defaults();
        app.show_favorites = false;
        app.panels[0].enrich_blocking();

        let text = render(&mut app, 140, 30);
        let row = row_with(&text, 140, "Projects");
        // The header calls the column `Ext`; a folder fills it with its count.
        assert!(row.contains('3'), "sub-folder count missing: {row:?}");
        assert!(row.contains("<DIR>"), "size column lost <DIR>: {row:?}");
    }

    #[test]
    fn a_measured_folder_shows_its_size_in_the_dir_column() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("Media");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("clip.bin"), vec![b'x'; 3 * 1024 * 1024]).unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.settings = crate::settings::Settings::defaults();
        app.show_favorites = false;
        app.panels[0].enrich_blocking();
        app.folder_sizes
            .insert(folder.clone(), crate::panel::folder_size(&folder));

        let text = render(&mut app, 140, 30);
        let row = row_with(&text, 140, "Media");
        assert!(row.contains("3.0M"), "measured size missing: {row:?}");
        assert!(!row.contains("<DIR>"), "placeholder survived: {row:?}");
    }

    #[test]
    fn the_footer_shows_one_chip_per_action_with_all_its_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = default_app(tmp.path());

        let line = footer_line(&mut app, 240, 30);
        // `F5` and `c` are the two slots of copy; they share one chip.
        assert!(
            line.contains("F5/C"),
            "copy must merge both slots into one chip: {line:?}"
        );
        assert_eq!(
            line.matches("copy").count(),
            1,
            "copy appears more than once: {line:?}"
        );
        assert_eq!(
            line.matches("delete").count(),
            1,
            "delete appears more than once: {line:?}"
        );
        assert!(line.contains("F8/Del"), "delete chip merged: {line:?}");
    }

    #[test]
    fn the_help_screen_has_no_undocumented_chords() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = default_app(tmp.path());
        app.help = true;

        let text = render(&mut app, 100, 40);
        assert!(text.contains("copy to the other panel"), "{text}");
        // The F9 Up/Down chord was never in the settings menu and is gone.
        assert!(!text.contains("F9 + ↑"), "stale chord still listed: {text}");
        assert!(!text.contains("Cmd+"), "alias that never worked is listed");
    }
}
