use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use crate::app::{App, Focus, Mode, SidebarTab, TreeNode, InputMode, ResponseTab};
use request_pilot_core::history::HistoryFilter;

// Catppuccin Mocha-inspired palette
mod theme {
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
        draw_help_popup(frame, frame.area());
    }

    if let Some(idx) = app.history_detail_idx {
        draw_history_detail(frame, app, frame.area(), idx);
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

    let line = Line::from(vec![
        Span::styled(" \u{2708} Request Pilot ", Style::default().fg(theme::BLUE).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(" F1 Files ", mode_style(Mode::Files, app.mode)),
        Span::raw(" "),
        Span::styled(" F3 History ", mode_style(Mode::History, app.mode)),
        Span::raw("  "),
        Span::styled("F5 Run All", Style::default().fg(theme::GREEN)),
        Span::raw("  "),
        Span::styled("? Help", Style::default().fg(theme::TEXT_FAINT)),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(theme::BG_BASE)), area);
}

fn draw_main(frame: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Files => draw_files_mode(frame, app, area),
        Mode::History => draw_history_mode(frame, app, area),
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

    draw_code_view(frame, app, v_chunks[0]);
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

fn draw_code_view(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::CodeView {
        Style::default().fg(theme::BLUE)
    } else {
        Style::default().fg(theme::TEXT_FAINT)
    };

    let block = Block::default()
        .title(" Code ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(blk) = file.suite.blocks.get(bi) {
                let mut lines = Vec::new();

                let type_color = match blk.block_type.as_str() {
                    "setup" => theme::BLUE,
                    "test" => theme::GREEN,
                    "teardown" => theme::RED,
                    _ => theme::TEXT,
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("@{}", blk.block_type), Style::default().fg(type_color).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(&blk.name, Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD)),
                    if blk.disabled {
                        Span::styled(" [disabled]", Style::default().fg(theme::TEXT_FAINT))
                    } else {
                        Span::raw("")
                    },
                ]));

                if !blk.description.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("# @description {}", blk.description),
                        Style::default().fg(theme::TEXT_FAINT),
                    )));
                }

                lines.push(Line::from(""));

                let method_color = match blk.request.method.as_str() {
                    "GET" => theme::GREEN,
                    "POST" => theme::BLUE,
                    "PUT" => theme::YELLOW,
                    "PATCH" => theme::PINK,
                    "DELETE" => theme::RED,
                    _ => theme::TEXT,
                };
                lines.push(Line::from(vec![
                    Span::styled(&blk.request.method, Style::default().fg(method_color).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(&blk.request.url, Style::default().fg(theme::TEXT)),
                ]));

                for (k, v) in &blk.request.headers {
                    lines.push(Line::from(vec![
                        Span::styled(k, Style::default().fg(theme::LAVENDER)),
                        Span::raw(": "),
                        Span::styled(v, Style::default().fg(theme::TEXT)),
                    ]));
                }

                if let Some(ref body) = blk.request.body {
                    lines.push(Line::from(""));
                    for line in body.lines() {
                        lines.push(Line::from(Span::styled(line, Style::default().fg(theme::TEXT))));
                    }
                }

                if !blk.assertions.is_empty() {
                    lines.push(Line::from(""));
                    for a in &blk.assertions {
                        lines.push(Line::from(Span::styled(
                            format!("# @assert {} {} {}", a.left, a.operator, a.right),
                            Style::default().fg(theme::YELLOW),
                        )));
                    }
                }

                if !blk.extracts.is_empty() {
                    for e in &blk.extracts {
                        lines.push(Line::from(Span::styled(
                            format!("# @extract {} = {}", e.variable_name, e.source_path),
                            Style::default().fg(theme::MAUVE),
                        )));
                    }
                }

                let paragraph = Paragraph::new(lines)
                    .block(block)
                    .wrap(Wrap { trim: false })
                    .scroll((app.code_scroll, 0));
                frame.render_widget(paragraph, area);
                return;
            }
        }
    }

    let msg = Paragraph::new("Select a block from the file tree")
        .block(block)
        .style(Style::default().fg(theme::TEXT_FAINT));
    frame.render_widget(msg, area);
}

fn draw_response_view(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::Response {
        Style::default().fg(theme::BLUE)
    } else {
        Style::default().fg(theme::TEXT_FAINT)
    };

    // Build tab header
    let tab_style = |tab: ResponseTab| {
        if tab == app.response_tab {
            Style::default().fg(theme::BG_DARK).bg(theme::BLUE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT_FAINT)
        }
    };
    let title_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(" Body(b) ", tab_style(ResponseTab::Body)),
        Span::raw(" "),
        Span::styled(" Headers(h) ", tab_style(ResponseTab::Headers)),
        Span::raw(" "),
        Span::styled(" Assertions(a) ", tab_style(ResponseTab::Assertions)),
        Span::raw(" "),
    ]);

    let block_widget = Block::default()
        .title(title_line)
        .borders(Borders::ALL)
        .border_style(border_style);

    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(ref results) = file.results {
                if let Some(br) = results.block_results.get(bi) {
                    if br.status == "pending" {
                        let msg = Paragraph::new("Run tests to see responses here")
                            .block(block_widget)
                            .style(Style::default().fg(theme::TEXT_FAINT));
                        frame.render_widget(msg, area);
                        return;
                    }

                    let mut lines = Vec::new();

                    match app.response_tab {
                        ResponseTab::Body => {
                            // Status line
                            let status_color = match br.status.as_str() {
                                "passed" => theme::GREEN,
                                "failed" | "error" => theme::RED,
                                "skipped" => theme::YELLOW,
                                _ => theme::TEXT,
                            };
                            lines.push(Line::from(vec![
                                Span::styled(
                                    format!(" {} ", br.status.to_uppercase()),
                                    Style::default().fg(theme::BG_DARK).bg(status_color).add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(format!(" \u{00b7} {}ms", br.time_ms), Style::default().fg(theme::TEXT_DIM)),
                            ]));

                            if let Some(ref resp) = br.response {
                                lines.push(Line::from(""));
                                lines.push(Line::from(vec![
                                    Span::styled("HTTP ", Style::default().fg(theme::TEXT_FAINT)),
                                    Span::styled(
                                        format!("{} {}", resp.status, resp.status_text),
                                        Style::default().fg(if resp.status < 400 { theme::GREEN } else { theme::RED }),
                                    ),
                                ]));

                                if !resp.body.is_empty() {
                                    lines.push(Line::from(""));
                                    let body_preview = if resp.body.len() > 2000 {
                                        &resp.body[..2000]
                                    } else {
                                        &resp.body
                                    };
                                    for line in body_preview.lines().take(50) {
                                        lines.push(Line::from(Span::styled(line, Style::default().fg(theme::TEXT))));
                                    }
                                }
                            }
                        }
                        ResponseTab::Headers => {
                            if let Some(ref resp) = br.response {
                                lines.push(Line::from(vec![
                                    Span::styled("HTTP ", Style::default().fg(theme::TEXT_FAINT)),
                                    Span::styled(
                                        format!("{} {}", resp.status, resp.status_text),
                                        Style::default().fg(if resp.status < 400 { theme::GREEN } else { theme::RED }),
                                    ),
                                ]));
                                lines.push(Line::from(""));
                                for (k, v) in &resp.headers {
                                    lines.push(Line::from(vec![
                                        Span::styled(k, Style::default().fg(theme::LAVENDER)),
                                        Span::raw(": "),
                                        Span::styled(v, Style::default().fg(theme::TEXT_DIM)),
                                    ]));
                                }
                            } else {
                                lines.push(Line::from(Span::styled("No response headers", Style::default().fg(theme::TEXT_FAINT))));
                            }
                        }
                        ResponseTab::Assertions => {
                            if !br.assertion_results.is_empty() {
                                for ar in &br.assertion_results {
                                    let (icon, color) = if ar.passed {
                                        ("\u{2713}", theme::GREEN)
                                    } else {
                                        ("\u{2717}", theme::RED)
                                    };
                                    lines.push(Line::from(Span::styled(
                                        format!(" {} {}", icon, ar.assertion),
                                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                                    )));
                                }
                            } else {
                                lines.push(Line::from(Span::styled("No assertions defined", Style::default().fg(theme::TEXT_FAINT))));
                            }

                            if !br.extract_results.is_empty() {
                                lines.push(Line::from(""));
                                lines.push(Line::from(Span::styled(
                                    "\u{2500}\u{2500}\u{2500} Extracts \u{2500}\u{2500}\u{2500}",
                                    Style::default().fg(theme::TEXT_FAINT),
                                )));
                                for er in &br.extract_results {
                                    let val_str = er.value.as_deref().unwrap_or("(none)");
                                    lines.push(Line::from(vec![
                                        Span::styled(&er.variable, Style::default().fg(theme::MAUVE)),
                                        Span::styled(" = ", Style::default().fg(theme::TEXT_FAINT)),
                                        Span::styled(val_str, Style::default().fg(theme::TEXT)),
                                    ]));
                                }
                            }
                        }
                    }

                    let paragraph = Paragraph::new(lines)
                        .block(block_widget)
                        .wrap(Wrap { trim: false })
                        .scroll((app.response_scroll, 0));
                    frame.render_widget(paragraph, area);
                    return;
                }
            }
        }
    }

    let msg = Paragraph::new("Run tests to see responses here")
        .block(block_widget)
        .style(Style::default().fg(theme::TEXT_FAINT));
    frame.render_widget(msg, area);
}

fn draw_history_mode(frame: &mut Frame, app: &App, area: Rect) {
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(5),
        ])
        .split(area);

    draw_history_stats(frame, app, v_chunks[0]);
    draw_history_filter(frame, app, v_chunks[1]);
    draw_history_list(frame, app, v_chunks[2]);
}

fn draw_history_stats(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" \u{1f4ca} Stats ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE));

    let entries = &app.history.entries;
    let total = entries.len();

    if total == 0 {
        let msg = Paragraph::new("No history entries yet. Run some tests!")
            .block(block)
            .style(Style::default().fg(theme::TEXT_FAINT));
        frame.render_widget(msg, area);
        return;
    }

    let success_count = entries.iter().filter(|e| e.status < 400).count();
    let pass_rate = if total > 0 {
        (success_count as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    let avg_time = if total > 0 {
        entries.iter().map(|e| e.response_time_ms).sum::<u64>() / total as u64
    } else {
        0
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("  Total runs: ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled(format!("{}", total), Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD)),
            Span::raw("    "),
            Span::styled("Pass rate: ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled(
                format!("{:.1}%", pass_rate),
                Style::default().fg(if pass_rate >= 80.0 { theme::GREEN } else if pass_rate >= 50.0 { theme::YELLOW } else { theme::RED }).add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled("Avg time: ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled(format!("{}ms", avg_time), Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  \u{2713} ", Style::default().fg(theme::GREEN)),
            Span::styled(format!("{} succeeded", success_count), Style::default().fg(theme::GREEN)),
            Span::raw("  "),
            Span::styled("\u{2717} ", Style::default().fg(theme::RED)),
            Span::styled(format!("{} failed", total - success_count), Style::default().fg(theme::RED)),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_history_filter(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::FilterInput {
        Style::default().fg(theme::BLUE)
    } else {
        Style::default().fg(theme::TEXT_FAINT)
    };

    let block = Block::default()
        .title(" \u{1f50d} Filter (/ to search) ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let text = if app.filter_text.is_empty() {
        if app.focus == Focus::FilterInput {
            "Type to filter..."
        } else {
            "Press / to filter"
        }
    } else {
        &app.filter_text
    };

    let style = if app.filter_text.is_empty() {
        Style::default().fg(theme::TEXT_FAINT)
    } else {
        Style::default().fg(theme::TEXT)
    };

    let paragraph = Paragraph::new(Span::styled(text, style)).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_history_list(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::HistoryList {
        Style::default().fg(theme::BLUE)
    } else {
        Style::default().fg(theme::TEXT_FAINT)
    };

    let block = Block::default()
        .title(" History Entries ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let filter = if app.filter_text.is_empty() {
        HistoryFilter::default()
    } else {
        HistoryFilter {
            url_contains: Some(app.filter_text.clone()),
            ..Default::default()
        }
    };

    let filtered: Vec<_> = app.history.filter(&filter);

    if filtered.is_empty() {
        let msg = if app.filter_text.is_empty() {
            "No history entries. Run tests to populate."
        } else {
            "No matching entries."
        };
        let paragraph = Paragraph::new(msg)
            .block(block)
            .style(Style::default().fg(theme::TEXT_FAINT));
        frame.render_widget(paragraph, area);
        return;
    }

    let mut sorted_entries = filtered;
    sorted_entries.reverse();

    let items: Vec<ListItem> = sorted_entries.iter().enumerate().map(|(i, entry)| {
        let is_selected = i == app.history_cursor;

        let status_color = if entry.status < 300 {
            theme::GREEN
        } else if entry.status < 400 {
            theme::YELLOW
        } else {
            theme::RED
        };

        let method_color = match entry.method.as_str() {
            "GET" => theme::GREEN,
            "POST" => theme::BLUE,
            "PUT" => theme::YELLOW,
            "PATCH" => theme::PINK,
            "DELETE" => theme::RED,
            _ => theme::TEXT,
        };

        let bg = if is_selected {
            theme::BG_HIGHLIGHT
        } else {
            Color::Reset
        };

        let time_str = if entry.timestamp.len() >= 19 {
            &entry.timestamp[11..19]
        } else {
            &entry.timestamp
        };

        ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {} ", entry.status),
                Style::default().fg(theme::BG_DARK).bg(status_color),
            ),
            Span::styled(" ", Style::default().bg(bg)),
            Span::styled(
                format!("{:<7}", entry.method),
                Style::default().fg(method_color).bg(bg),
            ),
            Span::styled(&entry.url, Style::default().fg(theme::TEXT).bg(bg)),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(
                format!("{}ms", entry.response_time_ms),
                Style::default().fg(theme::TEXT_FAINT).bg(bg),
            ),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(
                time_str,
                Style::default().fg(theme::TEXT_FAINT).bg(bg),
            ),
        ]))
    }).collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
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

fn draw_help_popup(frame: &mut Frame, area: Rect) {
    let popup_width = 60.min(area.width - 4);
    let popup_height = 38.min(area.height - 4);
    let x = (area.width - popup_width) / 2;
    let y = (area.height - popup_height) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let kb = Style::default().fg(theme::PEACH);
    let dim = Style::default().fg(theme::TEXT_FAINT);

    let help_text = vec![
        Line::from(Span::styled("Keybindings", Style::default().fg(theme::BLUE).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Navigation \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("j/k \u{2191}\u{2193}       ", kb), Span::styled("Navigate / scroll", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("g / G         ", kb), Span::styled("Jump to top / bottom", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("Ctrl+u/d      ", kb), Span::styled("Half-page up / down", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("1-9           ", kb), Span::styled("Quick jump to file", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("Tab           ", kb), Span::styled("Cycle focus", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("Enter         ", kb), Span::styled("Select / expand / detail", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("Space         ", kb), Span::styled("Toggle expand", Style::default().fg(theme::TEXT))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} File Operations \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("o / Ctrl+O    ", kb), Span::styled("Open file", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("n / Ctrl+N    ", kb), Span::styled("New file", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("s / Ctrl+S    ", kb), Span::styled("Save file", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("x             ", kb), Span::styled("Close file", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("Ctrl+E        ", kb), Span::styled("Load .env file", Style::default().fg(theme::TEXT))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Execution \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("r             ", kb), Span::styled("Run selected (block/group/file)", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("R / F5        ", kb), Span::styled("Run all tests", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("t             ", kb), Span::styled("Toggle block enabled/disabled", Style::default().fg(theme::TEXT))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Variables \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("Ctrl+V        ", kb), Span::styled("Toggle sidebar tab", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("a             ", kb), Span::styled("Add variable", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("e             ", kb), Span::styled("Edit variable", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("d             ", kb), Span::styled("Delete variable", Style::default().fg(theme::TEXT))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Response \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("b / h / a     ", kb), Span::styled("Body / Headers / Assertions tab", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("y             ", kb), Span::styled("Copy response body", Style::default().fg(theme::TEXT))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} History \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("/             ", kb), Span::styled("Filter", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("C             ", kb), Span::styled("Clear history", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("E             ", kb), Span::styled("Export history", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("Enter         ", kb), Span::styled("View details", Style::default().fg(theme::TEXT))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} General \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("F1            ", kb), Span::styled("Files mode", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("F3            ", kb), Span::styled("History mode", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("?             ", kb), Span::styled("Toggle help", Style::default().fg(theme::TEXT))]),
        Line::from(vec![Span::styled("q             ", kb), Span::styled("Quit", Style::default().fg(theme::TEXT))]),
    ];

    let block = Block::default()
        .title(" \u{2708} Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE))
        .style(Style::default().bg(theme::BG_OVERLAY));

    let paragraph = Paragraph::new(help_text).block(block);
    frame.render_widget(paragraph, popup_area);
}

fn get_block_status(file: &crate::app::LoadedFile, block_idx: usize) -> &'static str {
    if let Some(ref results) = file.results {
        if let Some(br) = results.block_results.get(block_idx) {
            return match br.status.as_str() {
                "passed" => "\u{2713}",
                "failed" | "error" => "\u{2717}",
                "skipped" => "\u{2298}",
                "running" => "\u{2800}",
                _ => "\u{00b7}",
            };
        }
    }
    "\u{00b7}"
}
