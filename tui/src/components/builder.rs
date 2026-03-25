//! Builder panel — visual request builder with method/URL/headers/body editing.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, BuilderFocus};
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
    render_assertions(frame, blk, chunks[3]);
    render_extracts(frame, blk, chunks[4]);
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
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let block = Block::default()
        .title(format!(" Assertions ({}) ", blk.assertions.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::TEXT_FAINT()));

    if blk.assertions.is_empty() {
        frame.render_widget(
            Paragraph::new("  (none)")
                .block(block)
                .style(Style::default().fg(theme::TEXT_FAINT())),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = blk
        .assertions
        .iter()
        .map(|a| {
            let full = format!("{} {} {}", a.left, a.operator, a.right);
            let max_a = (area.width as usize).saturating_sub(6);
            let display_a = truncate_to(&full, max_a);
            ListItem::new(Line::from(vec![
                Span::styled("  ● ", Style::default().fg(theme::YELLOW())),
                Span::styled(
                    display_a,
                    Style::default().fg(theme::TEXT()),
                ),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

fn render_extracts(
    frame: &mut Frame,
    blk: &request_pilot_core::http_parser::TestBlock,
    area: Rect,
) {
    let block = Block::default()
        .title(format!(" Extracts ({}) ", blk.extracts.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::TEXT_FAINT()));

    if blk.extracts.is_empty() {
        frame.render_widget(
            Paragraph::new("  (none)")
                .block(block)
                .style(Style::default().fg(theme::TEXT_FAINT())),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = blk
        .extracts
        .iter()
        .map(|e| {
            ListItem::new(Line::from(vec![
                Span::styled("  ⇐ ", Style::default().fg(theme::MAUVE())),
                Span::styled(&e.variable_name, Style::default().fg(theme::MAUVE())),
                Span::styled(" = ", Style::default().fg(theme::TEXT_FAINT())),
                Span::styled(&e.source_path, Style::default().fg(theme::TEXT())),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

pub fn handle_builder_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        // Tab cycles builder sub-focus
        KeyCode::Tab => {
            app.builder_focus = match app.builder_focus {
                BuilderFocus::Method => BuilderFocus::Url,
                BuilderFocus::Url => BuilderFocus::Headers,
                BuilderFocus::Headers => BuilderFocus::Body,
                BuilderFocus::Body => BuilderFocus::Method,
            };
        }
        KeyCode::BackTab => {
            app.builder_focus = match app.builder_focus {
                BuilderFocus::Method => BuilderFocus::Body,
                BuilderFocus::Url => BuilderFocus::Method,
                BuilderFocus::Headers => BuilderFocus::Url,
                BuilderFocus::Body => BuilderFocus::Headers,
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
                BuilderFocus::Method => handle_method_keys(app, key),
                BuilderFocus::Url => handle_url_keys(app, key),
                BuilderFocus::Headers => handle_headers_keys(app, key),
                BuilderFocus::Body => handle_body_keys(app, key),
            }
        }
    }
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
