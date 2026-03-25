use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use crate::app::{App, Focus, Mode, SidebarTab, TreeNode, InputMode};
use crate::components;
use crate::components::sidebar::get_block_status;
use crate::toolbar;
use request_pilot_core::history::HistoryFilter;

// Catppuccin Mocha-inspired palette
pub mod theme {
    use ratatui::style::Color;

    pub const BG_DARK: Color = Color::Rgb(30, 30, 46);
    pub const BG_BASE: Color = Color::Rgb(36, 39, 58);
    pub const BG_SURFACE: Color = Color::Rgb(49, 50, 68);
    pub const BG_OVERLAY: Color = Color::Rgb(69, 71, 90);
    pub const BG_HIGHLIGHT: Color = Color::Rgb(88, 91, 112);

    pub const TEXT: Color = Color::Rgb(205, 214, 244);
    pub const TEXT_DIM: Color = Color::Rgb(147, 153, 178);
    pub const TEXT_FAINT: Color = Color::Rgb(108, 112, 134);

    pub const BLUE: Color = Color::Rgb(137, 180, 250);
    pub const LAVENDER: Color = Color::Rgb(180, 190, 254);
    pub const SAPPHIRE: Color = Color::Rgb(116, 199, 236);
    pub const GREEN: Color = Color::Rgb(166, 227, 161);
    pub const YELLOW: Color = Color::Rgb(249, 226, 175);
    pub const PEACH: Color = Color::Rgb(250, 179, 135);
    pub const RED: Color = Color::Rgb(243, 139, 168);
    pub const PINK: Color = Color::Rgb(245, 194, 231);
    pub const MAUVE: Color = Color::Rgb(203, 166, 247);
    pub const SKY: Color = Color::Rgb(137, 220, 235);
}

pub fn init_terminal() -> color_eyre::Result<ratatui::DefaultTerminal> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    Ok(terminal)
}

pub fn restore_terminal() -> color_eyre::Result<()> {
    ratatui::restore();
    Ok(())
}

pub fn draw(frame: &mut Frame, app: &App) {
    let has_input = !matches!(app.input_mode, InputMode::Normal);

    let constraints = if has_input {
        vec![
            Constraint::Length(1),    // top bar
            Constraint::Min(10),      // main area
            Constraint::Length(1),    // input bar
            Constraint::Length(1),    // status bar
        ]
    } else {
        vec![
            Constraint::Length(1),    // top bar
            Constraint::Min(10),      // main area
            Constraint::Length(1),    // status bar
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    draw_top_bar(frame, app, chunks[0]);

    if has_input {
        draw_main(frame, app, chunks[1]);
        draw_input_bar(frame, app, chunks[2]);
        draw_status_bar(frame, app, chunks[3]);
    } else {
        draw_main(frame, app, chunks[1]);
        draw_status_bar(frame, app, chunks[2]);
    }

    if app.show_help {
        components::overlays::render_help_popup(frame, frame.area());
    }

    if let Some(idx) = app.history_detail_idx {
        draw_history_detail(frame, app, frame.area(), idx);
    }

    // Diff viewer overlay
    if app.diff_viewer_open {
        components::diff_viewer::render_diff_overlay(frame, app, frame.area());
    }

    // Toolbar popup overlays
    if app.extra_headers_open {
        toolbar::render_extra_headers_popup(frame, app, frame.area());
    }
    if app.azure_popup_open {
        toolbar::render_azure_popup(frame, app, frame.area());
    }
    if app.otel_popup_open {
        toolbar::render_otel_popup(frame, app, frame.area());
    }
}

fn draw_top_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mode_style = |m: Mode, current: Mode| {
        if m == current {
            Style::default().fg(theme::BG_DARK).bg(theme::BLUE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT_FAINT)
        }
    };

    let mut spans = vec![
        Span::styled(" ✈ Request Pilot ", Style::default().fg(theme::BLUE).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(" f Files ", mode_style(Mode::Files, app.mode)),
        Span::raw(" "),
        Span::styled(" h History ", mode_style(Mode::History, app.mode)),
        Span::raw(" "),
        Span::styled(" c Code ", mode_style(Mode::Code, app.mode)),
        Span::raw(" "),
        Span::styled(" l Logs ", mode_style(Mode::Logs, app.mode)),
        Span::raw("  "),
        Span::styled("R Run All", Style::default().fg(theme::GREEN)),
        Span::raw("  "),
        Span::styled("? Help", Style::default().fg(theme::TEXT_FAINT)),
        Span::raw("  "),
    ];
    spans.extend(toolbar::toolbar_badges(app));
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(theme::BG_BASE)), area);
}

fn draw_main(frame: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Files => draw_files_mode(frame, app, area),
        Mode::History => components::history::render_history_mode(frame, app, area),
        Mode::Code => crate::code_editor::render_editor(frame, app, area),
        Mode::Logs => components::logs::render_logs_mode(frame, app, area),
    }
}

fn draw_input_bar(frame: &mut Frame, app: &App, area: Rect) {
    let (prompt, buffer) = match &app.input_mode {
        InputMode::Input { prompt, buffer, .. } => (prompt.as_str(), buffer.as_str()),
        InputMode::Confirm { prompt, .. } => (prompt.as_str(), ""),
        InputMode::Normal => return,
    };

    let line = Line::from(vec![
        Span::styled(prompt, Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD)),
        Span::styled(buffer, Style::default().fg(theme::TEXT)),
        Span::styled("\u{2588}", Style::default().fg(theme::BLUE)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::BG_SURFACE)),
        area,
    );
}

fn draw_files_mode(frame: &mut Frame, app: &App, area: Rect) {
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(75),
        ])
        .split(area);

    draw_sidebar(frame, app, h_chunks[0]);

    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(h_chunks[1]);

    // Show builder panel (top) and response (bottom)
    components::builder::render_builder(frame, app, v_chunks[0]);
    draw_response_view(frame, app, v_chunks[1]);
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = matches!(app.focus, Focus::FileTree | Focus::Variables);
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE)
    } else {
        Style::default().fg(theme::TEXT_FAINT)
    };

    let title = match app.sidebar_tab {
        SidebarTab::Files => " \u{1f4c1} Files ",
        SidebarTab::Variables => " \u{1f510} Variables ",
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    match app.sidebar_tab {
        SidebarTab::Files => {
            if app.tree_nodes.is_empty() {
                let msg = Paragraph::new("No files loaded.\nPress 'o' to open a file.")
                    .block(block)
                    .style(Style::default().fg(theme::TEXT_FAINT));
                frame.render_widget(msg, area);
                return;
            }

            let items: Vec<ListItem> = app.tree_nodes.iter().enumerate().map(|(i, node)| {
                let (prefix, label, style) = match node {
                    TreeNode::File { file_idx } => {
                        let f = &app.loaded_files[*file_idx];
                        let chevron = if f.expanded { "\u{25bc}" } else { "\u{25b6}" };
                        (format!("{} \u{1f4c4} ", chevron), f.name.clone(), Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD))
                    }
                    TreeNode::Group { file_idx, group_name } => {
                        let f = &app.loaded_files[*file_idx];
                        let expanded = f.group_expanded.get(group_name).copied().unwrap_or(true);
                        let chevron = if expanded { "\u{25bc}" } else { "\u{25b6}" };
                        (format!("  {} ", chevron), group_name.clone(), Style::default().fg(theme::SAPPHIRE))
                    }
                    TreeNode::Block { file_idx, block_idx } => {
                        let f = &app.loaded_files[*file_idx];
                        let blk = &f.suite.blocks[*block_idx];
                        let icon = match blk.block_type.as_str() {
                            "setup" => "\u{2699}",
                            "test" => "\u{1f9ea}",
                            "teardown" => "\u{1f5d1}",
                            _ => "\u{1f4e4}",
                        };
                        let status = get_block_status(f, *block_idx);
                        let disabled_mark = if blk.disabled { " \u{d8}" } else { "" };
                        let indent = if blk.group.is_some() { "      " } else { "    " };
                        (format!("{}{} {} ", indent, status, icon), format!("{}{}", blk.name, disabled_mark), Style::default().fg(theme::TEXT))
                    }
                };

                let is_selected = i == app.tree_cursor;
                let final_style = if is_selected {
                    style.bg(theme::BG_HIGHLIGHT)
                } else {
                    style
                };
                ListItem::new(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(label, final_style),
                ]))
            }).collect();

            let list = List::new(items).block(block);
            frame.render_widget(list, area);
        }
        SidebarTab::Variables => {
            let mut extract_vars: std::collections::HashSet<String> = std::collections::HashSet::new();
            for file in &app.loaded_files {
                for blk in &file.suite.blocks {
                    for e in &blk.extracts {
                        extract_vars.insert(e.variable_name.clone());
                    }
                }
            }

            let sorted_vars = app.get_sorted_var_names();

            let items: Vec<ListItem> = sorted_vars.iter().enumerate().map(|(i, k)| {
                let v = app.env_vars.get(k).map(|s| s.as_str()).unwrap_or("");
                let name_color = if extract_vars.contains(k.as_str()) {
                    theme::MAUVE
                } else {
                    theme::SAPPHIRE
                };
                let prefix = if extract_vars.contains(k.as_str()) { "\u{21d0} " } else { "  " };
                let is_selected = i == app.vars_cursor;
                let bg = if is_selected { theme::BG_HIGHLIGHT } else { Color::Reset };
                ListItem::new(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(name_color).bg(bg)),
                    Span::styled(k, Style::default().fg(name_color).bg(bg)),
                    Span::styled(" = ", Style::default().fg(theme::TEXT_FAINT).bg(bg)),
                    Span::styled(v, Style::default().fg(theme::TEXT).bg(bg)),
                ]))
            }).collect();

            let scroll = app.vars_scroll as usize;
            let visible_items: Vec<ListItem> = items.into_iter().skip(scroll).collect();

            let list = List::new(visible_items).block(block);
            frame.render_widget(list, area);
        }
    }
}

fn draw_response_view(frame: &mut Frame, app: &App, area: Rect) {
    crate::components::response::render_response(frame, app, area);
}

fn draw_history_detail(frame: &mut Frame, app: &App, area: Rect, idx: usize) {
    let filter = if app.filter_text.is_empty() {
        HistoryFilter::default()
    } else {
        HistoryFilter {
            url_contains: Some(app.filter_text.clone()),
            ..Default::default()
        }
    };
    let filtered: Vec<_> = app.history.filter(&filter);
    let mut sorted = filtered;
    sorted.reverse();

    let entry = match sorted.get(idx) {
        Some(e) => *e,
        None => return,
    };

    let popup_width = (area.width - 4).min(100);
    let popup_height = (area.height - 4).min(40);
    let x = (area.width - popup_width) / 2;
    let y = (area.height - popup_height) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let mut lines = Vec::new();

    // Request info
    let method_color = match entry.method.as_str() {
        "GET" => theme::GREEN,
        "POST" => theme::BLUE,
        "PUT" => theme::YELLOW,
        "PATCH" => theme::PINK,
        "DELETE" => theme::RED,
        _ => theme::TEXT,
    };

    lines.push(Line::from(vec![
        Span::styled(&entry.method, Style::default().fg(method_color).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(&entry.url, Style::default().fg(theme::TEXT)),
    ]));

    let status_color = if entry.status < 300 { theme::GREEN } else if entry.status < 400 { theme::YELLOW } else { theme::RED };
    lines.push(Line::from(vec![
        Span::styled(format!("Status: {}", entry.status), Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  \u{00b7} {}ms  \u{00b7} {} bytes", entry.response_time_ms, entry.response_size_bytes), Style::default().fg(theme::TEXT_DIM)),
    ]));
    lines.push(Line::from(Span::styled(format!("Time: {}", entry.timestamp), Style::default().fg(theme::TEXT_FAINT))));

    // Request headers
    if !entry.request_headers.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Request Headers \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT))));
        for (k, v) in &entry.request_headers {
            lines.push(Line::from(vec![
                Span::styled(k, Style::default().fg(theme::LAVENDER)),
                Span::raw(": "),
                Span::styled(v, Style::default().fg(theme::TEXT_DIM)),
            ]));
        }
    }

    // Request body
    if let Some(ref body) = entry.request_body {
        if !body.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Request Body \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT))));
            for line in body.lines().take(10) {
                lines.push(Line::from(Span::styled(line, Style::default().fg(theme::TEXT))));
            }
        }
    }

    // Response headers
    if !entry.response_headers.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Response Headers \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT))));
        for (k, v) in &entry.response_headers {
            lines.push(Line::from(vec![
                Span::styled(k, Style::default().fg(theme::LAVENDER)),
                Span::raw(": "),
                Span::styled(v, Style::default().fg(theme::TEXT_DIM)),
            ]));
        }
    }

    // Response body
    if let Some(ref body) = entry.response_body {
        if !body.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Response Body \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT))));
            let preview = if body.len() > 1500 { &body[..1500] } else { body.as_str() };
            for line in preview.lines().take(20) {
                lines.push(Line::from(Span::styled(line, Style::default().fg(theme::TEXT))));
            }
        }
    }

    let block = Block::default()
        .title(" \u{1f4cb} Request Detail (Esc to close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE))
        .style(Style::default().bg(theme::BG_OVERLAY));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup_area);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![];

    if app.is_running {
        let spinner = app.spinner_char();
        if app.progress_total > 0 {
            spans.push(Span::styled(
                format!(" {} Running {}/{}... ", spinner, app.progress_current, app.progress_total),
                Style::default().fg(theme::SKY).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} Running... ", spinner),
                Style::default().fg(theme::SKY).add_modifier(Modifier::BOLD),
            ));
        }
    } else if let Some(fi) = app.active_file_idx {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(ref r) = file.results {
                if r.passed > 0 {
                    spans.push(Span::styled(
                        format!(" \u{2713} {} passed ", r.passed),
                        Style::default().fg(theme::GREEN).add_modifier(Modifier::BOLD),
                    ));
                }
                if r.failed > 0 {
                    spans.push(Span::styled(
                        format!(" \u{2717} {} failed ", r.failed),
                        Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
                    ));
                }
                if r.skipped > 0 {
                    spans.push(Span::styled(
                        format!(" \u{2298} {} skipped ", r.skipped),
                        Style::default().fg(theme::YELLOW),
                    ));
                }
                spans.push(Span::styled(
                    format!(" \u{00b7} {}ms ", r.total_time_ms),
                    Style::default().fg(theme::TEXT_FAINT),
                ));
            }
        }
    }

    if let Some((ref msg, _)) = app.status_message {
        if spans.is_empty() {
            spans.push(Span::styled(format!(" {} ", msg), Style::default().fg(theme::TEXT_DIM)));
        }
    }

    let hints = " q=quit Tab=focus r=run ?=help ";
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let available = area.width as usize;
    if available > used + hints.len() {
        let pad = available - used - hints.len();
        spans.push(Span::raw(" ".repeat(pad)));
    }
    spans.push(Span::styled(hints, Style::default().fg(theme::TEXT_FAINT)));

    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::BG_BASE)),
        area,
    );
}