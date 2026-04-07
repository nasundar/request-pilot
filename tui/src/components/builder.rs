//! Builder panel — visual request builder with method/URL/headers/body editing.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, BuilderFocus, InputMode, InputPurpose, ConfirmPurpose};
use crate::ui::theme;
use crate::ui::truncate_to;

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

fn method_color(method: &str) -> ratatui::style::Color {
    match method {
        "GET" => theme::GREEN(),
        "POST" => theme::BLUE(),
        "PUT" => theme::YELLOW(),
        "PATCH" => theme::PINK(),
        "DELETE" => theme::RED(),
        "HEAD" => theme::SKY(),
        "OPTIONS" => theme::LAVENDER(),
        _ => theme::TEXT(),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn render_builder(frame: &mut Frame, app: &App, area: Rect) {
    let (fi, bi) = match (app.active_file_idx, app.active_block_idx) {
        (Some(fi), Some(bi)) => (fi, bi),
        _ => {
            let block = Block::default()
                .title(" Builder ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::TEXT_FAINT()));
            frame.render_widget(
                Paragraph::new("Select a block from the file tree")
                    .block(block)
                    .style(Style::default().fg(theme::TEXT_FAINT())),
                area,
            );
            return;
        }
    };

    let file = match app.loaded_files.get(fi) {
        Some(f) => f,
        None => return,
    };
    let blk = match file.suite.blocks.get(bi) {
        Some(b) => b,
        None => return,
    };

    if blk.compare && !blk.steps.is_empty() {
        render_compare_builder(frame, app, blk, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // method + URL
            Constraint::Length(blk.request.headers.len().max(1) as u16 + 2), // headers
            Constraint::Min(3),   // body
            Constraint::Length(blk.assertions.len().max(1) as u16 + 2), // assertions
            Constraint::Length(blk.extracts.len().max(1) as u16 + 2),   // extracts
        ])
        .split(area);

    render_method_url(frame, app, blk, chunks[0]);
    render_headers(frame, app, blk, chunks[1]);
    render_body(frame, app, blk, chunks[2]);
    render_assertions(frame, app, blk, chunks[3]);
    render_extracts(frame, app, blk, chunks[4]);

    // Render variable autocomplete overlay if open
    if app.var_ac_open && app.builder_focus == BuilderFocus::Url {
        render_var_autocomplete(frame, app, chunks[0]);
    }
}

fn render_method_url(
    frame: &mut Frame,
    app: &App,
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let is_focused = app.builder_focus == BuilderFocus::Method
        || app.builder_focus == BuilderFocus::Url;
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };

    let block = Block::default()
        .title(" Method / URL  (Tab cycle, ←/→ method, Ctrl+Enter send) ")
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(if is_focused {
            Style::default().bg(theme::BG_FOCUS())
        } else {
            Style::default()
        });

    let method_style = if app.builder_focus == BuilderFocus::Method {
        Style::default()
            .fg(theme::BG_DARK())
            .bg(method_color(&blk.request.method))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(method_color(&blk.request.method))
            .add_modifier(Modifier::BOLD)
    };

    let url_text = &blk.request.url;
    let url_spans = highlight_variables(url_text, app.builder_focus == BuilderFocus::Url);

    let mut spans = vec![
        Span::styled(format!(" {} ", blk.request.method), method_style),
        Span::raw(" "),
    ];
    spans.extend(url_spans);

    if app.builder_focus == BuilderFocus::Url {
        spans.push(Span::styled("█", Style::default().fg(theme::BLUE())));
    }

    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line).block(block), area);
}

/// Renders a compare-block-specific builder view showing steps and diff assertions.
fn render_compare_builder(
    frame: &mut Frame,
    app: &App,
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let is_focused = matches!(app.builder_focus, BuilderFocus::Method | BuilderFocus::Url);
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };
    let block_widget = Block::default()
        .title(" ⇄ Compare Block ")
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(if is_focused {
            Style::default().bg(theme::BG_FOCUS())
        } else {
            Style::default()
        });

    let inner = block_widget.inner(area);
    frame.render_widget(block_widget, area);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Block name
    if !blk.name.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                blk.name.clone(),
                Style::default().fg(theme::TEXT()).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if !blk.description.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(blk.description.clone(), Style::default().fg(theme::TEXT_DIM())),
        ]));
    }
    lines.push(Line::from(""));

    // Steps
    for (i, step) in blk.steps.iter().enumerate() {
        let step_marker = if i == 0 { "▸ " } else { "▸ " };
        let m = &step.request.method;
        let mc = method_color(m);
        let url_max = (inner.width as usize).saturating_sub(m.len() + step.name.len() + 12);
        let url_display = truncate_to(&step.request.url, url_max);

        lines.push(Line::from(vec![
            Span::styled(format!("  {}", step_marker), Style::default().fg(theme::YELLOW())),
            Span::styled(
                step.name.clone(),
                Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                m.clone(),
                Style::default().fg(mc).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
            Span::styled(url_display, Style::default().fg(theme::TEXT_DIM())),
        ]));

        // Step headers summary
        let hdr_count = step.request.headers.len();
        let assert_count = step.assertions.len();
        let extract_count = step.extracts.len();
        let mut meta_spans: Vec<Span<'static>> = vec![
            Span::styled("      ", Style::default()),
        ];
        meta_spans.push(Span::styled(
            format!("{} headers", hdr_count),
            Style::default().fg(theme::TEXT_FAINT()),
        ));
        if assert_count > 0 {
            meta_spans.push(Span::styled(
                format!("  {} assertions", assert_count),
                Style::default().fg(theme::TEXT_FAINT()),
            ));
        }
        if extract_count > 0 {
            meta_spans.push(Span::styled(
                format!("  {} extracts", extract_count),
                Style::default().fg(theme::TEXT_FAINT()),
            ));
        }
        if let Some(ref body) = step.request.body {
            let body_len = body.len();
            meta_spans.push(Span::styled(
                format!("  body: {} bytes", body_len),
                Style::default().fg(theme::TEXT_FAINT()),
            ));
        }
        lines.push(Line::from(meta_spans));

        // Show header details for each step
        for (k, v) in &step.request.headers {
            let val_max = (inner.width as usize).saturating_sub(k.len() + 12);
            let val_display = truncate_to(v, val_max);
            lines.push(Line::from(vec![
                Span::styled("        ", Style::default()),
                Span::styled(k.clone(), Style::default().fg(theme::LAVENDER())),
                Span::styled(": ", Style::default().fg(theme::TEXT_FAINT())),
                Span::styled(val_display, Style::default().fg(theme::TEXT_DIM())),
            ]));
        }

        lines.push(Line::from(""));
    }

    // Diff directive
    if let Some(ref diff) = blk.diff {
        lines.push(Line::from(vec![
            Span::styled("  ⇄ diff ", Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD)),
            Span::styled(diff.step_a.clone(), Style::default().fg(theme::PEACH())),
            Span::styled(" ↔ ", Style::default().fg(theme::TEXT_FAINT())),
            Span::styled(diff.step_b.clone(), Style::default().fg(theme::PEACH())),
        ]));
    }

    // Diff assertions
    if !blk.assertions.is_empty() {
        for a in &blk.assertions {
            lines.push(Line::from(vec![
                Span::styled("  ● ", Style::default().fg(theme::YELLOW())),
                Span::styled(
                    format!("{} {} {}", a.left, a.operator, a.right),
                    Style::default().fg(theme::TEXT()),
                ),
            ]));
        }
    }

    let scroll = app.builder_body_scroll;
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0)),
        inner,
    );
}

fn highlight_variables(text: &str, focused: bool) -> Vec<Span<'_>> {
    let base_style = if focused {
        Style::default().fg(theme::TEXT())
    } else {
        Style::default().fg(theme::TEXT_DIM())
    };
    let var_style = Style::default()
        .fg(theme::MAUVE())
        .add_modifier(Modifier::BOLD);

    let mut spans = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find("{{") {
        if start > 0 {
            spans.push(Span::styled(&rest[..start], base_style));
        }
        if let Some(end) = rest[start..].find("}}") {
            let var_end = start + end + 2;
            spans.push(Span::styled(&rest[start..var_end], var_style));
            rest = &rest[var_end..];
        } else {
            spans.push(Span::styled(&rest[start..], base_style));
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest, base_style));
    }
    spans
}

fn render_headers(
    frame: &mut Frame,
    app: &App,
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let is_focused = app.builder_focus == BuilderFocus::Headers;
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };

    let block = Block::default()
        .title(format!(" Headers ({}) ", blk.request.headers.len()))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(if is_focused {
            Style::default().bg(theme::BG_FOCUS())
        } else {
            Style::default()
        });

    if blk.request.headers.is_empty() {
        frame.render_widget(
            Paragraph::new("  (no headers)")
                .block(block)
                .style(Style::default().fg(theme::TEXT_FAINT())),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = blk
        .request
        .headers
        .iter()
        .enumerate()
        .map(|(i, (k, v))| {
            let is_sel = is_focused && i == app.builder_header_cursor;
            let bg = if is_sel {
                theme::BG_HIGHLIGHT()
            } else {
                ratatui::style::Color::Reset
            };
            let max_val = (area.width as usize).saturating_sub(k.len() + 6);
            let display_val = truncate_to(v, max_val);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {}", k),
                    Style::default().fg(theme::LAVENDER()).bg(bg),
                ),
                Span::styled(": ", Style::default().fg(theme::TEXT_FAINT()).bg(bg)),
                Span::styled(display_val, Style::default().fg(theme::TEXT()).bg(bg)),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

fn render_body(
    frame: &mut Frame,
    app: &App,
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let is_focused = app.builder_focus == BuilderFocus::Body;
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };

    let block = Block::default()
        .title(" Body ")
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(if is_focused {
            Style::default().bg(theme::BG_FOCUS())
        } else {
            Style::default()
        });

    match &blk.request.body {
        Some(body) if !body.is_empty() => {
            let lines: Vec<Line> = body
                .lines()
                .enumerate()
                .map(|(i, line)| {
                    let ln = Span::styled(
                        format!("{:>3} ", i + 1),
                        Style::default().fg(theme::TEXT_FAINT()),
                    );
                    let content = Span::styled(line, Style::default().fg(theme::TEXT()));
                    Line::from(vec![ln, content])
                })
                .collect();
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .wrap(Wrap { trim: false })
                    .scroll((app.builder_body_scroll, 0)),
                area,
            );
        }
        _ => {
            frame.render_widget(
                Paragraph::new("  (no body)")
                    .block(block)
                    .style(Style::default().fg(theme::TEXT_FAINT())),
                area,
            );
        }
    }
}

fn render_assertions(
    frame: &mut Frame,
    app: &App,
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let is_focused = app.builder_focus == BuilderFocus::Assertions;
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };
    let title = if is_focused {
        format!(" Assertions ({}) [a]dd [d]el ", blk.assertions.len())
    } else {
        format!(" Assertions ({}) ", blk.assertions.len())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    if blk.assertions.is_empty() {
        let hint = if is_focused { "  (none) — press [a] to add" } else { "  (none)" };
        frame.render_widget(
            Paragraph::new(hint)
                .block(block)
                .style(Style::default().fg(theme::TEXT_FAINT())),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = blk
        .assertions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let full = format!("{} {} {}", a.left, a.operator, a.right);
            let max_a = (area.width as usize).saturating_sub(6);
            let display_a = truncate_to(&full, max_a);
            let is_active = is_focused && i == app.builder_assert_cursor;
            let style = if is_active {
                Style::default().fg(theme::BG_DARK()).bg(theme::YELLOW()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT())
            };
            let bullet_style = if is_active {
                Style::default().fg(theme::BG_DARK()).bg(theme::YELLOW())
            } else {
                Style::default().fg(theme::YELLOW())
            };
            ListItem::new(Line::from(vec![
                Span::styled("  ● ", bullet_style),
                Span::styled(display_a, style),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

fn render_extracts(
    frame: &mut Frame,
    app: &App,
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let is_focused = app.builder_focus == BuilderFocus::Extracts;
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };
    let title = if is_focused {
        format!(" Extracts ({}) [a]dd [d]el ", blk.extracts.len())
    } else {
        format!(" Extracts ({}) ", blk.extracts.len())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    if blk.extracts.is_empty() {
        let hint = if is_focused { "  (none) — press [a] to add" } else { "  (none)" };
        frame.render_widget(
            Paragraph::new(hint)
                .block(block)
                .style(Style::default().fg(theme::TEXT_FAINT())),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = blk
        .extracts
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let is_active = is_focused && i == app.builder_extract_cursor;
            let var_style = if is_active {
                Style::default().fg(theme::BG_DARK()).bg(theme::MAUVE()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::MAUVE())
            };
            let path_style = if is_active {
                Style::default().fg(theme::BG_DARK()).bg(theme::MAUVE())
            } else {
                Style::default().fg(theme::TEXT())
            };
            let sep_style = if is_active {
                Style::default().fg(theme::BG_DARK()).bg(theme::MAUVE())
            } else {
                Style::default().fg(theme::TEXT_FAINT())
            };
            ListItem::new(Line::from(vec![
                Span::styled("  ⇐ ", var_style),
                Span::styled(e.variable_name.clone(), var_style),
                Span::styled(" = ", sep_style),
                Span::styled(e.source_path.clone(), path_style),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

/// Render variable autocomplete popup below the URL bar.
pub fn render_var_autocomplete(frame: &mut Frame, app: &App, url_area: Rect) {
    use ratatui::widgets::Clear;

    if app.var_ac_filtered.is_empty() { return; }

    let max_items = 8u16;
    let item_count = app.var_ac_filtered.len().min(max_items as usize) as u16;
    let popup_height = item_count + 2; // borders
    let popup_width = url_area.width.min(50).max(30);

    let popup_area = Rect {
        x: url_area.x + 1,
        y: url_area.y + url_area.height,
        width: popup_width,
        height: popup_height.min(frame.area().height.saturating_sub(url_area.y + url_area.height)),
    };

    if popup_area.height < 3 { return; }

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Variables (↑↓ Enter) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_SURFACE()));

    // Scroll so cursor is always visible
    let scroll_offset = if app.var_ac_cursor >= max_items as usize {
        app.var_ac_cursor - (max_items as usize) + 1
    } else {
        0
    };

    let items: Vec<ListItem> = app.var_ac_filtered.iter().enumerate()
        .skip(scroll_offset)
        .take(max_items as usize)
        .map(|(i, (name, value))| {
            let is_active = i == app.var_ac_cursor;
            let style = if is_active {
                Style::default().fg(theme::BG_DARK()).bg(theme::BLUE()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT())
            };
            let val_style = if is_active {
                Style::default().fg(theme::BG_DARK()).bg(theme::BLUE())
            } else {
                Style::default().fg(theme::TEXT_DIM())
            };
            let max_name = (popup_width as usize).saturating_sub(8);
            let display_name = truncate_to(name, max_name);
            let max_val = (popup_width as usize).saturating_sub(display_name.len() + 6);
            let display_val = truncate_to(value, max_val);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {}", display_name), style),
                Span::styled(format!(" {}", display_val), val_style),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), popup_area);
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

pub fn handle_builder_keys(app: &mut App, key: KeyEvent) {
    // If autocomplete is open, handle its keys first
    if app.var_ac_open {
        match key.code {
            KeyCode::Down => {
                app.var_ac_cursor = (app.var_ac_cursor + 1).min(app.var_ac_filtered.len().saturating_sub(1));
                return;
            }
            KeyCode::Up => {
                app.var_ac_cursor = app.var_ac_cursor.saturating_sub(1);
                return;
            }
            KeyCode::Enter | KeyCode::Tab => {
                insert_var_autocomplete_builder(app);
                return;
            }
            KeyCode::Esc => {
                app.close_var_autocomplete();
                return;
            }
            KeyCode::Char(_c) => {
                // Let the char go through to the URL handler, then update filter
                if app.builder_focus == BuilderFocus::Url {
                    handle_url_keys(app, key);
                    app.flush_builder_to_file();
                    update_ac_from_url(app);
                    return;
                }
            }
            KeyCode::Backspace => {
                if app.builder_focus == BuilderFocus::Url {
                    handle_url_keys(app, key);
                    app.flush_builder_to_file();
                    update_ac_from_url(app);
                    return;
                }
            }
            _ => {
                app.close_var_autocomplete();
                // Fall through to normal key handling below
            }
        }
    }

    match key.code {
        // Ctrl+Space opens autocomplete
        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.builder_focus == BuilderFocus::Url {
                let prefix = extract_var_prefix_from_url(app);
                app.open_var_autocomplete(&prefix);
            }
            return;
        }
        // Tab cycles builder sub-focus
        KeyCode::Tab => {
            app.builder_focus = match app.builder_focus {
                BuilderFocus::Method => BuilderFocus::Url,
                BuilderFocus::Url => BuilderFocus::Headers,
                BuilderFocus::Headers => BuilderFocus::Body,
                BuilderFocus::Body => BuilderFocus::Assertions,
                BuilderFocus::Assertions => BuilderFocus::Extracts,
                BuilderFocus::Extracts => BuilderFocus::Method,
            };
        }
        KeyCode::BackTab => {
            app.builder_focus = match app.builder_focus {
                BuilderFocus::Method => BuilderFocus::Extracts,
                BuilderFocus::Url => BuilderFocus::Method,
                BuilderFocus::Headers => BuilderFocus::Url,
                BuilderFocus::Body => BuilderFocus::Headers,
                BuilderFocus::Assertions => BuilderFocus::Body,
                BuilderFocus::Extracts => BuilderFocus::Assertions,
            };
        }
        // Ctrl+Enter sends request
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
                app.select_block(fi, bi);
                app.queue_run_single(fi, bi);
            }
        }
        _ => {
            match app.builder_focus {
                BuilderFocus::Method => {
                    handle_method_keys(app, key);
                    // Method change always mutates — flush to keep code in sync
                    if matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l')) {
                        app.flush_builder_to_file();
                    }
                }
                BuilderFocus::Url => {
                    handle_url_keys(app, key);
                    // Flush on content-changing keys
                    if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace) {
                        app.flush_builder_to_file();
                    }
                    // Auto-trigger on {{ typed
                    if matches!(key.code, KeyCode::Char('{')) {
                        let prefix = extract_var_prefix_from_url(app);
                        if prefix.is_empty() || !prefix.contains('\n') {
                            // Check if we just typed the second {
                            if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
                                if let Some(file) = app.loaded_files.get(fi) {
                                    if let Some(blk) = file.suite.blocks.get(bi) {
                                        let text_before: String = blk.request.url.chars().take(app.builder_url_cursor).collect();
                                        if text_before.ends_with("{{") {
                                            app.open_var_autocomplete("");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                BuilderFocus::Headers => handle_headers_keys(app, key),
                BuilderFocus::Body => handle_body_keys(app, key),
                BuilderFocus::Assertions => handle_assertions_keys(app, key),
                BuilderFocus::Extracts => handle_extracts_keys(app, key),
            }
        }
    }
}

/// Extract the variable prefix from URL text before cursor (text after last `{{`).
fn extract_var_prefix_from_url(app: &App) -> String {
    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(blk) = file.suite.blocks.get(bi) {
                let text_before: String = blk.request.url.chars().take(app.builder_url_cursor).collect();
                if let Some(pos) = text_before.rfind("{{") {
                    let after_close = text_before.rfind("}}").unwrap_or(0);
                    if pos > after_close || after_close == 0 {
                        return text_before[pos + 2..].to_string();
                    }
                }
            }
        }
    }
    String::new()
}

/// Update autocomplete filter based on current URL text.
fn update_ac_from_url(app: &mut App) {
    let prefix = extract_var_prefix_from_url(app);
    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(blk) = file.suite.blocks.get(bi) {
                let text_before: String = blk.request.url.chars().take(app.builder_url_cursor).collect();
                let has_open = text_before.rfind("{{").map(|p| {
                    let close = text_before.rfind("}}").unwrap_or(0);
                    p > close || close == 0
                }).unwrap_or(false);
                if has_open {
                    app.update_var_autocomplete(&prefix);
                    return;
                }
            }
        }
    }
    app.close_var_autocomplete();
}

/// Insert the selected autocomplete variable into the builder URL.
fn insert_var_autocomplete_builder(app: &mut App) {
    if app.var_ac_filtered.is_empty() {
        app.close_var_autocomplete();
        return;
    }
    let selected = app.var_ac_filtered[app.var_ac_cursor].0.clone();
    app.close_var_autocomplete();

    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get_mut(fi) {
            if let Some(blk) = file.suite.blocks.get_mut(bi) {
                let text_before: String = blk.request.url.chars().take(app.builder_url_cursor).collect();
                // Find the {{ that started this autocomplete
                if let Some(open_pos) = text_before.rfind("{{") {
                    let insertion = format!("{{{{{}}}}}", selected);
                    let before: String = blk.request.url.chars().take(open_pos).collect();
                    // Consume trailing `}}` if cursor is inside an existing placeholder
                    let after_text: String = blk.request.url.chars().skip(app.builder_url_cursor).collect();
                    let after: String = if after_text.starts_with("}}") {
                        after_text.chars().skip(2).collect()
                    } else if let Some(close_idx) = after_text.find("}}") {
                        let between = &after_text[..close_idx];
                        if !between.contains('{') {
                            after_text[close_idx + 2..].to_string()
                        } else {
                            after_text
                        }
                    } else {
                        after_text
                    };
                    blk.request.url = format!("{}{}{}", before, insertion, after);
                    app.builder_url_cursor = open_pos + insertion.chars().count();
                } else {
                    // No {{ found — insert full {{var}} at cursor
                    let insertion = format!("{{{{{}}}}}", selected);
                    let byte_pos = blk.request.url.char_indices().nth(app.builder_url_cursor).map(|(i, _)| i).unwrap_or(blk.request.url.len());
                    blk.request.url.insert_str(byte_pos, &insertion);
                    app.builder_url_cursor += insertion.chars().count();
                }
            }
        }
    }
    app.flush_builder_to_file();
}

fn handle_method_keys(app: &mut App, key: KeyEvent) {
    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get_mut(fi) {
            if let Some(blk) = file.suite.blocks.get_mut(bi) {
                let current_idx = METHODS
                    .iter()
                    .position(|&m| m == blk.request.method)
                    .unwrap_or(0);
                match key.code {
                    KeyCode::Left | KeyCode::Char('h') => {
                        let new_idx = if current_idx == 0 {
                            METHODS.len() - 1
                        } else {
                            current_idx - 1
                        };
                        blk.request.method = METHODS[new_idx].to_string();
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        let new_idx = (current_idx + 1) % METHODS.len();
                        blk.request.method = METHODS[new_idx].to_string();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn handle_url_keys(app: &mut App, key: KeyEvent) {
    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get_mut(fi) {
            if let Some(blk) = file.suite.blocks.get_mut(bi) {
                match key.code {
                    KeyCode::Char(c) => {
                        let byte_pos = blk.request.url.char_indices().nth(app.builder_url_cursor).map(|(i, _)| i).unwrap_or(blk.request.url.len());
                        blk.request.url.insert(byte_pos, c);
                        app.builder_url_cursor += 1;
                    }
                    KeyCode::Backspace => {
                        if app.builder_url_cursor > 0 {
                            let byte_pos = blk.request.url.char_indices().nth(app.builder_url_cursor - 1).map(|(i, _)| i).unwrap_or(blk.request.url.len());
                            if byte_pos < blk.request.url.len() {
                                blk.request.url.remove(byte_pos);
                                app.builder_url_cursor -= 1;
                            }
                        }
                    }
                    KeyCode::Left => {
                        app.builder_url_cursor = app.builder_url_cursor.saturating_sub(1);
                    }
                    KeyCode::Right => {
                        app.builder_url_cursor =
                            (app.builder_url_cursor + 1).min(blk.request.url.chars().count());
                    }
                    KeyCode::Home => {
                        app.builder_url_cursor = 0;
                    }
                    KeyCode::End => {
                        app.builder_url_cursor = blk.request.url.chars().count();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn handle_headers_keys(app: &mut App, key: KeyEvent) {
    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        let header_count = app
            .loaded_files
            .get(fi)
            .and_then(|f| f.suite.blocks.get(bi))
            .map(|b| b.request.headers.len())
            .unwrap_or(0);

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                app.builder_header_cursor = app.builder_header_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if header_count > 0 {
                    app.builder_header_cursor =
                        (app.builder_header_cursor + 1).min(header_count.saturating_sub(1));
                }
            }
            _ => {}
        }
    }
}

fn handle_body_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            app.builder_body_scroll = app.builder_body_scroll.saturating_add(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.builder_body_scroll = app.builder_body_scroll.saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.builder_body_scroll = app.builder_body_scroll.saturating_add(10);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.builder_body_scroll = app.builder_body_scroll.saturating_sub(10);
        }
        _ => {}
    }
}

fn handle_assertions_keys(app: &mut App, key: KeyEvent) {
    let count = if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        app.loaded_files.get(fi)
            .and_then(|f| f.suite.blocks.get(bi))
            .map(|b| b.assertions.len())
            .unwrap_or(0)
    } else {
        0
    };

    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 {
                app.builder_assert_cursor = (app.builder_assert_cursor + 1).min(count - 1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.builder_assert_cursor = app.builder_assert_cursor.saturating_sub(1);
        }
        // Add assertion
        KeyCode::Char('a') => {
            app.input_mode = InputMode::Input {
                prompt: "Left operand (e.g. status, $.field): ".to_string(),
                purpose: InputPurpose::AssertLeft,
                buffer: String::new(),
            };
        }
        // Delete assertion
        KeyCode::Char('d') => {
            if count > 0 && app.builder_assert_cursor < count {
                let idx = app.builder_assert_cursor;
                if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
                    if let Some(a) = app.loaded_files[fi].suite.blocks[bi].assertions.get(idx) {
                        let desc = format!("{} {} {}", a.left, a.operator, a.right);
                        app.input_mode = InputMode::Confirm {
                            prompt: format!("Delete assertion '{}'? (y/n)", desc),
                            purpose: ConfirmPurpose::DeleteAssertion { index: idx },
                        };
                    }
                }
            }
        }
        // Edit assertion
        KeyCode::Char('e') | KeyCode::Enter => {
            if count > 0 && app.builder_assert_cursor < count {
                let idx = app.builder_assert_cursor;
                if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
                    if let Some(a) = app.loaded_files[fi].suite.blocks[bi].assertions.get(idx) {
                        app.input_mode = InputMode::Input {
                            prompt: "Left operand: ".to_string(),
                            purpose: InputPurpose::EditAssertLeft { index: idx },
                            buffer: a.left.clone(),
                        };
                    }
                }
            }
        }
        _ => {}
    }
}

fn handle_extracts_keys(app: &mut App, key: KeyEvent) {
    let count = if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        app.loaded_files.get(fi)
            .and_then(|f| f.suite.blocks.get(bi))
            .map(|b| b.extracts.len())
            .unwrap_or(0)
    } else {
        0
    };

    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 {
                app.builder_extract_cursor = (app.builder_extract_cursor + 1).min(count - 1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.builder_extract_cursor = app.builder_extract_cursor.saturating_sub(1);
        }
        // Add extract
        KeyCode::Char('a') => {
            app.input_mode = InputMode::Input {
                prompt: "Variable name: ".to_string(),
                purpose: InputPurpose::ExtractVarName,
                buffer: String::new(),
            };
        }
        // Delete extract
        KeyCode::Char('d') => {
            if count > 0 && app.builder_extract_cursor < count {
                let idx = app.builder_extract_cursor;
                if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
                    if let Some(e) = app.loaded_files[fi].suite.blocks[bi].extracts.get(idx) {
                        app.input_mode = InputMode::Confirm {
                            prompt: format!("Delete extract '{}'? (y/n)", e.variable_name),
                            purpose: ConfirmPurpose::DeleteExtract { index: idx },
                        };
                    }
                }
            }
        }
        // Edit extract
        KeyCode::Char('e') | KeyCode::Enter => {
            if count > 0 && app.builder_extract_cursor < count {
                let idx = app.builder_extract_cursor;
                if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
                    if let Some(e) = app.loaded_files[fi].suite.blocks[bi].extracts.get(idx) {
                        app.input_mode = InputMode::Input {
                            prompt: "Variable name: ".to_string(),
                            purpose: InputPurpose::EditExtractVarName { index: idx },
                            buffer: e.variable_name.clone(),
                        };
                    }
                }
            }
        }
        _ => {}
    }
}
