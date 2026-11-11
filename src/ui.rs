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
use crate::panel::{Entry, Panel, PanelMode};
use crate::preview::{is_image_path, ImageCache, PreviewCache, PreviewContent};

const PATH_BG_ACTIVE: Color = Color::Rgb(0, 95, 135);
const PATH_BG: Color = Color::Rgb(52, 58, 70);
const SELECT_BG_ACTIVE: Color = Color::Rgb(0, 80, 110);
const SELECT_BG: Color = Color::Rgb(55, 55, 65);
const STATUS_BG: Color = Color::Rgb(30, 32, 40);
const FOOTER_BG: Color = Color::Rgb(20, 20, 26);

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
        );
    }

    draw_status(frame, app, chunks[1]);
    draw_footer(frame, app, chunks[2]);

    if app.help {
        draw_help(frame, area);
    }
    if let Some(prompt) = &app.prompt {
        draw_prompt(frame, area, prompt);
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
            draw_list(frame, panel, rows[2], active);
        }
        PanelMode::Thumbnails => {
            if rows[1].height > 0 {
                let caption = if panel.loading {
                    loading_caption(panel)
                } else {
                    format!(
                        " {} items · arrows move · Ctrl+T list ",
                        panel.entries.len().saturating_sub(1)
                    )
                };
                draw_caption(frame, rows[1], &caption);
            }
            draw_thumbnails(frame, panel, rows[2], active, image_cache);
        }
        PanelMode::Preview => {
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
            );
        }
    }
}

fn loading_caption(panel: &Panel) -> String {
    let subject = panel.loading_label().unwrap_or_else(|| "…".to_string());
    format!(" Loading {subject}… · Esc cancels ")
}

fn draw_list(frame: &mut Frame, panel: &mut Panel, list_area: Rect, active: bool) {
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
            list_item(entry, selected, width)
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
    let size = path.metadata().map(|meta| meta.len()).unwrap_or(0);

    if path.is_dir() {
        draw_caption(frame, header, &format!(" {name}  ·  folder "));
        match preview_cache.get(path, show_hidden) {
            PreviewContent::Directory { children, total } => {
                let max_scroll = children.len().saturating_sub(area.height as usize);
                clamp_preview(panel, max_scroll, 0);

                if children.is_empty() {
                    render_message(frame, area, "(empty folder)");
                    return;
                }

                let lines: Vec<Line> = children
                    .iter()
                    .map(|child| {
                        let marker = if child.is_dir {
                            Span::styled("▸ ", Style::default().fg(Color::LightBlue))
                        } else if child.is_image {
                            Span::styled("▣ ", Style::default().fg(Color::LightMagenta))
                        } else {
                            Span::styled("  ", Style::default())
                        };
                        let style = if child.is_dir {
                            Style::default().fg(Color::LightBlue)
                        } else {
                            Style::default().fg(Color::Gray)
                        };
                        Line::from(vec![
                            marker,
                            Span::styled(pad_right(&child.name, 32), style),
                            Span::styled(
                                human_size(child.size),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ])
                    })
                    .collect();

                let text =
                    Paragraph::new(lines).scroll((panel.preview_scroll, panel.preview_scroll_x));
                frame.render_widget(text, area);

                if *total > children.len() {
                    let more = format!(" … {} more item(s)", total.saturating_sub(children.len()));
                    let y = area.y + area.height.saturating_sub(1);
                    if area.height > 0 {
                        frame.render_widget(
                            Paragraph::new(more).style(Style::default().fg(Color::DarkGray)),
                            Rect {
                                x: area.x,
                                y,
                                width: area.width,
                                height: 1,
                            },
                        );
                    }
                }
            }
            PreviewContent::Error(error) => render_message(frame, area, error),
            _ => render_message(frame, area, "Cannot preview this folder"),
        }
        return;
    }

    if is_image_path(path) {
        match image_cache.render(path, area.width, area.height) {
            Some(image) => {
                let dimensions = format!(
                    " {name}  ·  image {}×{}  ·  {} ",
                    image.width(),
                    image.height().div_ceil(2),
                    human_size(size)
                );
                draw_caption(frame, header, &dimensions);
                clamp_preview(panel, 0, 0);
                render_halfblocks(frame, &image, area);
            }
            None => {
                draw_caption(frame, header, &format!(" {name}  ·  image "));
                render_message(frame, area, "Cannot decode this image");
            }
        }
        return;
    }

    match preview_cache.get(path, show_hidden) {
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

fn draw_thumbnails(
    frame: &mut Frame,
    panel: &mut Panel,
    area: Rect,
    active: bool,
    image_cache: &mut ImageCache,
) {
    const TILE_MIN_WIDTH: u16 = 22;
    const TILE_MAX_WIDTH: u16 = 32;

    if area.height == 0 || area.width == 0 {
        return;
    }

    // Adaptive tiles: 3 columns on wide panels, 2 on narrow ones. The height is
    // chosen so the image area is roughly square in pixels (2 px per cell row).
    let tile_width = (area.width / 3).clamp(TILE_MIN_WIDTH, TILE_MAX_WIDTH);
    let tile_height = ((tile_width.saturating_sub(2)) / 2 + 3).clamp(11, 22);

    let cols = (area.width / tile_width).max(1) as usize;
    let rows = (area.height / tile_height).max(1) as usize;
    panel.grid_cols = cols;
    panel.grid_rows = rows;
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
        );
    }
}

fn draw_tile(
    frame: &mut Frame,
    entry: &Entry,
    tile: Rect,
    focused: bool,
    selected: bool,
    active: bool,
    image_cache: &mut ImageCache,
) {
    let background = if focused {
        if active {
            SELECT_BG_ACTIVE
        } else {
            SELECT_BG
        }
    } else {
        Color::Reset
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(background)),
        tile,
    );

    let image_area = Rect {
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

    if image_area.width > 0 && image_area.height > 0 {
        if !entry.is_dir && is_image_path(&entry.path) {
            match image_cache.render(&entry.path, image_area.width, image_area.height) {
                Some(image) => render_halfblocks(frame, &image, image_area),
                None => render_message(frame, image_area, "no preview"),
            }
        } else if entry.is_dir {
            frame.render_widget(
                Paragraph::new("▸")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::LightBlue).bg(background)),
                center_vertically(image_area, 1),
            );
        } else {
            let label = entry
                .path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| format!(".{}", ext.to_lowercase()))
                .unwrap_or_else(|| "file".to_string());
            frame.render_widget(
                Paragraph::new(label)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray).bg(background)),
                center_vertically(image_area, 1),
            );
        }
    }

    let marker = if selected {
        "▣ "
    } else if entry.is_dir {
        "▸ "
    } else {
        ""
    };
    let name = format!("{marker}{}", entry.name);
    let name = truncate_right(&name, name_area.width as usize);
    let style = if selected || focused {
        Style::default()
            .fg(Color::LightYellow)
            .add_modifier(Modifier::BOLD)
    } else if entry.is_dir {
        Style::default().fg(Color::LightBlue)
    } else {
        Style::default().fg(Color::Gray)
    };

    frame.render_widget(
        Paragraph::new(name)
            .alignment(Alignment::Center)
            .style(style.bg(background)),
        name_area,
    );
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

fn draw_favorites(frame: &mut Frame, app: &App, area: Rect) {
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
                    let kind = if entry.is_dir {
                        "dir".to_string()
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

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let text = match app.focus {
        Focus::Favorites => {
            " Yazi2D · Favorites: ↑↓/jk move · Enter open · Del remove · Ctrl+B add · Shift+Tab/Ctrl+D panels · ? help · q quit"
        }
        Focus::Panels if app.job.is_some() => {
            " Yazi2D · operation in progress — navigation stays available"
        }
        Focus::Panels => {
            " Yazi2D · Tab switch · Shift+Tab favs · Ctrl+Q/Ctrl+P preview other · Ctrl+T thumbs · Ctrl+O open there · F5/c copy · F6/m move · F8/d delete · ? help · q quit"
        }
    };

    let text = truncate_right(text, area.width as usize);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().bg(FOOTER_BG).fg(Color::DarkGray)),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(4).min(76);
    let height = area.height.saturating_sub(2).min(34);
    if width == 0 || height == 0 {
        return;
    }
    let rect = centered(width, height, area);
    frame.render_widget(Clear, rect);

    let header = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::LightYellow);

    let lines = vec![
        Line::from(Span::styled("NAVIGATION", header)),
        help_line("  ↑/k, ↓/j", "move cursor / scroll preview", key),
        help_line("  PgUp / PgDn", "page up / down", key),
        help_line("  Home/g, End/G", "top / bottom", key),
        help_line("  Enter / l", "open directory", key),
        help_line("  Backspace / h", "parent directory", key),
        help_line("  Space / Insert", "select and move down", key),
        help_line("  Esc", "clear selection / close preview", key),
        Line::from(""),
        Line::from(Span::styled("FILE OPERATIONS", header)),
        help_line("  F5 / c", "copy to the other panel", key),
        help_line("  F6 / m", "move to the other panel", key),
        help_line("  F7 / n", "create directory", key),
        help_line("  F2 / r", "rename", key),
        help_line("  F8 / d / Del", "delete (Enter or y confirms)", key),
        Line::from(""),
        Line::from(Span::styled("VIEW & CROSS-PANEL", header)),
        help_line("  Ctrl+Q / Ctrl+P", "quick preview in the OTHER panel", key),
        help_line("  Ctrl+T", "thumbnail grid in the active panel", key),
        help_line("  Ctrl+O", "open cursor folder in the other panel", key),
        help_line(
            "  Ctrl+← / Ctrl+→",
            "preview in that panel (may be OS-bound)",
            key,
        ),
        help_line("  Alt+← / Alt+→", "open cursor folder in that panel", key),
        Line::from(""),
        Line::from(Span::styled("PANELS & FAVORITES", header)),
        help_line("  Tab", "switch active panel", key),
        help_line(
            "  Shift+Tab / Ctrl+D",
            "focus favorites / back to panels",
            key,
        ),
        help_line("  Ctrl+B / a", "add current dir to favorites", key),
        help_line("  Ctrl+F / f", "toggle favorites sidebar", key),
        help_line("  Ctrl+R", "refresh both panels", key),
        help_line("  Ctrl+H / .", "toggle hidden files", key),
        help_line("  Esc while loading", "cancel the directory load", key),
        Line::from(""),
        Line::from(Span::styled(
            "  ? / F1 close help      q / Ctrl+C quit",
            Style::default().fg(Color::Gray),
        )),
    ];

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

    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

fn help_line<'a>(keys: &'a str, description: &'a str, key_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(keys, key_style),
        Span::styled(format!("  {description}"), Style::default().fg(Color::Gray)),
    ])
}

fn draw_prompt(frame: &mut Frame, area: Rect, prompt: &Prompt) {
    let (title, preferred_width, preferred_height) = match prompt {
        Prompt::Mkdir { .. } => (" New folder ", 62, 3),
        Prompt::Rename { .. } => (" Rename ", 62, 3),
        Prompt::Delete { .. } => (" Confirm delete ", 64, 5),
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

fn list_item(entry: &Entry, selected: bool, width: usize) -> ListItem<'static> {
    let (prefix, prefix_style) = if entry.is_parent {
        ("↑ ", Style::default().fg(Color::DarkGray))
    } else if selected {
        ("▣ ", Style::default().fg(Color::LightYellow))
    } else if entry.is_dir {
        ("▸ ", Style::default().fg(Color::LightBlue))
    } else {
        ("  ", Style::default())
    };

    let name = if entry.is_parent {
        "..".to_string()
    } else if entry.is_dir {
        format!("{}/", entry.name)
    } else {
        entry.name.clone()
    };
    let name = if entry.is_symlink {
        if entry.is_dir {
            format!("{}@/", entry.name)
        } else {
            format!("{name}@")
        }
    } else {
        name
    };

    let size = if entry.is_parent {
        String::new()
    } else if entry.is_dir {
        "<DIR>".to_string()
    } else {
        human_size(entry.size)
    };

    let date = format_mtime(entry.modified);

    let name_style = if selected {
        Style::default().fg(Color::LightYellow)
    } else if entry.is_parent {
        Style::default().fg(Color::DarkGray)
    } else if entry.is_dir {
        Style::default().fg(Color::LightBlue)
    } else {
        Style::default().fg(Color::Gray)
    };

    let styles = RowStyles {
        prefix: prefix_style,
        name: name_style,
        detail: Style::default().fg(Color::DarkGray),
    };
    ListItem::new(build_row(width, prefix, &name, &size, &date, styles))
}

fn header_line(width: usize) -> Line<'static> {
    build_row(
        width,
        "  ",
        "Name",
        "Size",
        "Modified",
        RowStyles {
            prefix: Style::default(),
            name: Style::default().add_modifier(Modifier::BOLD),
            detail: Style::default(),
        },
    )
}

#[derive(Clone, Copy)]
struct RowStyles {
    prefix: Style,
    name: Style,
    detail: Style,
}

fn build_row(
    width: usize,
    prefix: &str,
    name: &str,
    size: &str,
    date: &str,
    styles: RowStyles,
) -> Line<'static> {
    let show_date = width >= 52;
    let show_size = width >= 34;
    let detail_width = (if show_size { 8 } else { 0 })
        + (if show_size && show_date { 1 } else { 0 })
        + (if show_date { 16 } else { 0 });
    let name_width = width.saturating_sub(2 + detail_width);

    let name = truncate_right(name, name_width);
    let mut spans = vec![
        Span::styled(prefix.to_string(), styles.prefix),
        Span::styled(pad_right(&name, name_width), styles.name),
    ];

    if show_size {
        spans.push(Span::styled(format!("{size:>8}"), styles.detail));
    }
    if show_date {
        spans.push(Span::styled(format!(" {date:<16}"), styles.detail));
    }

    Line::from(spans)
}

fn human_size(bytes: u64) -> String {
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
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
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
        assert!(text.contains("Ctrl+B"));
        assert!(text.contains("Ctrl+Q"));
        assert!(text.contains("Alt+"));
    }

    #[test]
    fn renders_a_text_preview_of_the_other_panel_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "hello preview\nsecond line\n").unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].select_name("notes.txt");
        app.panels[0].set_mode(PanelMode::Preview);
        app.panels[1].select_name("notes.txt");

        let text = render(&mut app, 120, 30);
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
    fn renders_a_thumbnail_grid_with_file_names() {
        let tmp = tempfile::tempdir().unwrap();
        let image = RgbImage::from_pixel(4, 4, image::Rgb([200, 40, 40]));
        image.save(tmp.path().join("red.png")).unwrap();

        let mut app = App::new(vec![tmp.path().to_path_buf(), tmp.path().to_path_buf()]);
        app.panels[0].set_mode(PanelMode::Thumbnails);

        let text = render(&mut app, 120, 40);
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

        let text = render(&mut app, 120, 30);
        assert!(text.contains("image"));
        assert!(text.contains("blue.png"));
        assert!(
            text.contains("▀"),
            "image preview was not drawn with half blocks"
        );
    }
}
