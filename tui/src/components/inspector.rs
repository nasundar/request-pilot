use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use crossterm::event::{KeyCode, KeyEvent};
use crate::app::{App, TreeNode};
use crate::ui::theme;
use request_pilot_core::history::HistoryEntry;

/// Build detail lines for whatever tree node is currently selected.
fn build_inspect_lines(app: &App) -> (String, Vec<Line<'static>>) {
    if app.tree_nodes.is_empty() {
        return ("Inspector".to_string(), vec![Line::from("No items")]);
    }
    let node = &app.tree_nodes[app.tree_cursor.min(app.tree_nodes.len() - 1)];
    match node {
        TreeNode::File { file_idx } => build_file_detail(app, *file_idx),
        TreeNode::Group { file_idx, group_name } => build_group_detail(app, *file_idx, group_name),
        TreeNode::Block { file_idx, block_idx } => build_block_detail(app, *file_idx, *block_idx),
    }
}

fn build_file_detail(app: &App, fi: usize) -> (String, Vec<Line<'static>>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let file = match app.loaded_files.get(fi) {
        Some(f) => f,
        None => return ("File".to_string(), vec![Line::from("File not found")]),
    };

    let title = format!(" 📄 {} ", file.name);
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::TEXT_DIM());
    let val = Style::default().fg(theme::TEXT());

    // Path
    if let Some(ref p) = file.path {
        lines.push(Line::from(vec![
            Span::styled("  Path: ", dim),
            Span::styled(p.display().to_string(), val),
        ]));
    }

    // Description — extract from header comments
    let desc = extract_file_description(&file.content);
    if !desc.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Description", h)));
        for d in desc {
            lines.push(Line::from(Span::styled(format!("  {}", d), dim)));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Structure", h)));

    let blocks = &file.suite.blocks;
    let setups = blocks.iter().filter(|b| b.block_type == "setup").count();
    let tests = blocks.iter().filter(|b| b.block_type == "test" || b.block_type == "request").count();
    let teardowns = blocks.iter().filter(|b| b.block_type == "teardown").count();
    let groups: std::collections::HashSet<&str> = blocks.iter()
        .filter_map(|b| b.group.as_deref()).collect();
    let vars = file.suite.variables.len();

    lines.push(Line::from(vec![
        Span::styled("  ⚙ Setup: ", dim), Span::styled(setups.to_string(), val),
        Span::styled("  🧪 Tests: ", dim), Span::styled(tests.to_string(), val),
        Span::styled("  🧹 Teardown: ", dim), Span::styled(teardowns.to_string(), val),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  ⊞ Groups: ", dim), Span::styled(groups.len().to_string(), val),
        Span::styled("  📝 Variables: ", dim), Span::styled(vars.to_string(), val),
        Span::styled("  Total: ", dim), Span::styled(blocks.len().to_string(), val),
    ]));

    // Block list
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Blocks", h)));
    for (bi, block) in blocks.iter().enumerate() {
        let type_icon = match block.block_type.as_str() {
            "setup" => "⚙",
            "teardown" => "🧹",
            _ => "🧪",
        };
        let name = if block.name.is_empty() { "(unnamed)" } else { &block.name };
        let mut parts: Vec<Span<'static>> = vec![
            Span::styled(format!("  {} ", type_icon), dim),
            Span::styled(name.to_string(), val),
        ];
        if let Some(ref g) = block.group {
            parts.push(Span::styled(format!(" [{}]", g), Style::default().fg(theme::LAVENDER())));
        }
        if block.disabled {
            parts.push(Span::styled(" ⊘", Style::default().fg(theme::TEXT_FAINT())));
        }
        // Run result badge
        if let Some(ref results) = file.results {
            if let Some(br) = results.block_results.get(bi) {
                let (icon, color) = match br.status.as_str() {
                    "passed" => ("✓", theme::GREEN()),
                    "failed" | "error" => ("✗", theme::RED()),
                    "skipped" => ("⊘", theme::YELLOW()),
                    _ => ("·", theme::TEXT_FAINT()),
                };
                parts.push(Span::styled(format!(" {} {}ms", icon, br.time_ms), Style::default().fg(color)));
            }
        }
        lines.push(Line::from(parts));
    }

    // Run stats
    if let Some(ref r) = file.results {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Last Run", h)));
        lines.push(Line::from(vec![
            Span::styled("  ✓ ", Style::default().fg(theme::GREEN())),
            Span::styled(format!("{} passed  ", r.passed), val),
            Span::styled("✗ ", Style::default().fg(theme::RED())),
            Span::styled(format!("{} failed  ", r.failed), val),
            Span::styled("⊘ ", Style::default().fg(theme::YELLOW())),
            Span::styled(format!("{} skipped  ", r.skipped), val),
            Span::styled(format!("{}ms", r.total_time_ms), dim),
        ]));
    }

    // History
    let hist_entries: Vec<&HistoryEntry> = app.history.entries.iter()
        .filter(|e| e.file_name.as_deref() == Some(&file.name))
        .collect();
    lines.extend(build_history_lines(&hist_entries, 8));

    (title, lines)
}

fn build_group_detail(app: &App, fi: usize, group_name: &str) -> (String, Vec<Line<'static>>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let file = match app.loaded_files.get(fi) {
        Some(f) => f,
        None => return ("Group".to_string(), vec![Line::from("File not found")]),
    };

    let title = format!(" ⊞ {} ", group_name);
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::TEXT_DIM());
    let val = Style::default().fg(theme::TEXT());

    lines.push(Line::from(vec![
        Span::styled("  File: ", dim),
        Span::styled(file.name.clone(), val),
    ]));

    let group_blocks: Vec<(usize, &request_pilot_core::http_parser::TestBlock)> = file.suite.blocks.iter()
        .enumerate()
        .filter(|(_, b)| b.group.as_deref() == Some(group_name))
        .collect();

    let assertions: usize = group_blocks.iter().map(|(_, b)| b.assertions.len()).sum();
    let extracts: usize = group_blocks.iter().map(|(_, b)| b.extracts.len()).sum();

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Summary", h)));
    lines.push(Line::from(vec![
        Span::styled("  🧪 Tests: ", dim), Span::styled(group_blocks.len().to_string(), val),
        Span::styled("  ✓ Assertions: ", dim), Span::styled(assertions.to_string(), val),
        Span::styled("  ↤ Extracts: ", dim), Span::styled(extracts.to_string(), val),
    ]));

    // Dependencies
    let deps: std::collections::HashSet<String> = group_blocks.iter()
        .flat_map(|(_, b)| b.depends.iter().cloned())
        .collect();
    if !deps.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  Depends: ", dim),
            Span::styled(deps.into_iter().collect::<Vec<_>>().join(", "), Style::default().fg(theme::PEACH())),
        ]));
    }

    // Block list with run results
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Tests", h)));
    for (bi, block) in &group_blocks {
        let name = if block.name.is_empty() { "(unnamed)" } else { &block.name };
        let method = &block.request.method;
        let mut parts: Vec<Span<'static>> = vec![
            Span::styled(format!("  {:<7} ", method), Style::default().fg(method_color(method))),
            Span::styled(name.to_string(), val),
        ];
        if let Some(ref results) = file.results {
            if let Some(br) = results.block_results.get(*bi) {
                let (icon, color) = match br.status.as_str() {
                    "passed" => ("✓", theme::GREEN()),
                    "failed" | "error" => ("✗", theme::RED()),
                    "skipped" => ("⊘", theme::YELLOW()),
                    _ => ("·", theme::TEXT_FAINT()),
                };
                parts.push(Span::styled(format!(" {} {}ms", icon, br.time_ms), Style::default().fg(color)));
            }
        }
        lines.push(Line::from(parts));
        // Show description if present
        if !block.description.is_empty() {
            lines.push(Line::from(Span::styled(format!("          {}", block.description), dim)));
        }
    }

    // Run stats for group
    if let Some(ref r) = file.results {
        let group_results: Vec<_> = group_blocks.iter()
            .filter_map(|(bi, _)| r.block_results.get(*bi))
            .collect();
        if !group_results.is_empty() {
            let passed = group_results.iter().filter(|b| b.status == "passed").count();
            let failed = group_results.iter().filter(|b| b.status == "failed" || b.status == "error").count();
            let total_ms: u64 = group_results.iter().map(|b| b.time_ms).sum();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("  Run Results", h)));
            lines.push(Line::from(vec![
                Span::styled("  ✓ ", Style::default().fg(theme::GREEN())),
                Span::styled(format!("{} passed  ", passed), val),
                Span::styled("✗ ", Style::default().fg(theme::RED())),
                Span::styled(format!("{} failed  ", failed), val),
                Span::styled(format!("{}ms total", total_ms), dim),
            ]));
        }
    }

    // History for this group
    let hist_entries: Vec<&HistoryEntry> = app.history.entries.iter()
        .filter(|e| {
            e.file_name.as_deref() == Some(&file.name)
                && e.group.as_deref() == Some(group_name)
        })
        .collect();
    if !hist_entries.is_empty() {
        lines.extend(build_history_lines(&hist_entries, 6));
    }

    (title, lines)
}

fn build_block_detail(app: &App, fi: usize, bi: usize) -> (String, Vec<Line<'static>>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let file = match app.loaded_files.get(fi) {
        Some(f) => f,
        None => return ("Block".to_string(), vec![Line::from("File not found")]),
    };
    let block = match file.suite.blocks.get(bi) {
        Some(b) => b,
        None => return ("Block".to_string(), vec![Line::from("Block not found")]),
    };

    let type_icon = match block.block_type.as_str() {
        "setup" => "⚙",
        "teardown" => "🧹",
        _ => "🧪",
    };
    let name = if block.name.is_empty() { "(unnamed)" } else { &block.name };
    let title = format!(" {} {} ", type_icon, name);
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::TEXT_DIM());
    let val = Style::default().fg(theme::TEXT());

    // Metadata
    lines.push(Line::from(Span::styled("  Metadata", h)));
    lines.push(Line::from(vec![
        Span::styled("  Type: ", dim),
        Span::styled(block.block_type.clone(), val),
    ]));
    if !block.description.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  Description: ", dim),
            Span::styled(block.description.clone(), val),
        ]));
    }
    if let Some(ref g) = block.group {
        lines.push(Line::from(vec![
            Span::styled("  Group: ", dim),
            Span::styled(g.clone(), Style::default().fg(theme::LAVENDER())),
        ]));
    }
    if !block.depends.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  Depends: ", dim),
            Span::styled(block.depends.join(", "), Style::default().fg(theme::PEACH())),
        ]));
    }
    if let Some(ref m) = block.mode {
        lines.push(Line::from(vec![
            Span::styled("  Mode: ", dim),
            Span::styled(m.clone(), Style::default().fg(theme::MAUVE())),
        ]));
    }
    if let Some(ref da) = block.dev_auth {
        lines.push(Line::from(vec![
            Span::styled("  Dev Auth: ", dim),
            Span::styled(da.clone(), Style::default().fg(theme::MAUVE())),
        ]));
    }
    if block.disabled {
        lines.push(Line::from(Span::styled("  ⊘ Disabled", Style::default().fg(theme::TEXT_FAINT()))));
    }
    if block.compare {
        lines.push(Line::from(Span::styled("  ⇔ Compare block", Style::default().fg(theme::SKY()))));
    }

    // Request
    if !block.compare {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Request", h)));
        lines.push(Line::from(vec![
            Span::styled(format!("  {}", block.request.method), Style::default().fg(method_color(&block.request.method)).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {}", block.request.url), val),
        ]));
        if !block.request.headers.is_empty() {
            lines.push(Line::from(Span::styled("  Headers:", dim)));
            for (k, v) in &block.request.headers {
                lines.push(Line::from(vec![
                    Span::styled(format!("    {}: ", k), Style::default().fg(theme::PEACH())),
                    Span::styled(truncate(v, 60), dim),
                ]));
            }
        }
        if let Some(ref body) = block.request.body {
            if !body.is_empty() {
                lines.push(Line::from(Span::styled("  Body:", dim)));
                for bline in body.lines().take(10) {
                    lines.push(Line::from(Span::styled(format!("    {}", bline), dim)));
                }
                if body.lines().count() > 10 {
                    lines.push(Line::from(Span::styled("    ... (truncated)", Style::default().fg(theme::TEXT_FAINT()))));
                }
            }
        }
    } else {
        // Compare block steps
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Steps", h)));
        for (si, step) in block.steps.iter().enumerate() {
            let sname = if step.name.is_empty() { format!("Step {}", si + 1) } else { step.name.clone() };
            lines.push(Line::from(vec![
                Span::styled(format!("  #{} ", si + 1), Style::default().fg(theme::PEACH())),
                Span::styled(sname, val),
            ]));
            lines.push(Line::from(vec![
                Span::styled(format!("    {} ", step.request.method), Style::default().fg(method_color(&step.request.method))),
                Span::styled(truncate(&step.request.url, 50), dim),
            ]));
        }
    }

    // Assertions
    if !block.assertions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Assertions", h)));
        let result = file.results.as_ref().and_then(|r| r.block_results.get(bi));
        for (ai, assertion) in block.assertions.iter().enumerate() {
            let expr = format!("{} {} {}", assertion.left, assertion.operator, assertion.right);
            if let Some(br) = result {
                if let Some(ar) = br.assertion_results.get(ai) {
                    let (icon, color) = if ar.passed { ("✓", theme::GREEN()) } else { ("✗", theme::RED()) };
                    let mut parts = vec![
                        Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                        Span::styled(expr, Style::default().fg(color)),
                    ];
                    if !ar.passed {
                        if let Some(ref actual) = ar.actual {
                            parts.push(Span::styled(format!("  (got: {})", truncate(actual, 40)), Style::default().fg(theme::RED())));
                        }
                    }
                    lines.push(Line::from(parts));
                } else {
                    lines.push(Line::from(Span::styled(format!("  · {}", expr), dim)));
                }
            } else {
                lines.push(Line::from(Span::styled(format!("  · {}", expr), dim)));
            }
        }
    }

    // Extracts
    if !block.extracts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Extracts", h)));
        let result = file.results.as_ref().and_then(|r| r.block_results.get(bi));
        for (ei, ext) in block.extracts.iter().enumerate() {
            if let Some(br) = result {
                if let Some(er) = br.extract_results.get(ei) {
                    let (icon, color) = if er.success { ("✓", theme::GREEN()) } else { ("✗", theme::RED()) };
                    let mut parts = vec![
                        Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                        Span::styled(format!("{} = {}", ext.variable_name, ext.source_path), val),
                    ];
                    if let Some(ref v) = er.value {
                        parts.push(Span::styled(format!("  → {}", truncate(v, 40)), Style::default().fg(theme::GREEN())));
                    }
                    lines.push(Line::from(parts));
                } else {
                    lines.push(Line::from(Span::styled(format!("  ↤ {} = {}", ext.variable_name, ext.source_path), dim)));
                }
            } else {
                lines.push(Line::from(Span::styled(format!("  ↤ {} = {}", ext.variable_name, ext.source_path), dim)));
            }
        }
    }

    // Run result summary
    if let Some(ref results) = file.results {
        if let Some(br) = results.block_results.get(bi) {
            if !br.status.is_empty() && br.status != "pending" {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("  Run Result", h)));
                let (icon, color) = match br.status.as_str() {
                    "passed" => ("✓ Passed", theme::GREEN()),
                    "failed" => ("✗ Failed", theme::RED()),
                    "error" => ("✗ Error", theme::RED()),
                    "skipped" => ("⊘ Skipped", theme::YELLOW()),
                    _ => ("· Unknown", theme::TEXT_FAINT()),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("in {}ms", br.time_ms), dim),
                ]));
                if let Some(ref url) = Some(&br.request_url) {
                    if !url.is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled("  URL: ", dim),
                            Span::styled(truncate(url, 60), val),
                        ]));
                    }
                }
                if let Some(ref resp) = br.response {
                    lines.push(Line::from(vec![
                        Span::styled("  Status: ", dim),
                        Span::styled(resp.status.to_string(), Style::default().fg(
                            if resp.status < 300 { theme::GREEN() }
                            else if resp.status < 400 { theme::YELLOW() }
                            else { theme::RED() }
                        )),
                        Span::styled(format!("  Size: {} bytes", resp.body.len()), dim),
                    ]));
                }
                if let Some(ref e) = br.error {
                    lines.push(Line::from(Span::styled(format!("  Error: {}", e), Style::default().fg(theme::RED()))));
                }
            }
        }
    }

    // History for this block
    let block_name = if block.name.is_empty() { None } else { Some(block.name.as_str()) };
    let hist_entries: Vec<&HistoryEntry> = app.history.entries.iter()
        .filter(|e| {
            e.file_name.as_deref() == Some(&file.name)
                && e.block_name.as_deref() == block_name
        })
        .collect();
    lines.extend(build_history_lines(&hist_entries, 6));

    (title, lines)
}

/// Build run history lines from matching history entries.
fn build_history_lines(entries: &[&HistoryEntry], max_rows: usize) -> Vec<Line<'static>> {
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::TEXT_DIM());
    let mut lines = Vec::new();

    if entries.is_empty() {
        return lines;
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Run History", h)));

    // Aggregate by run_id to show per-run summaries
    let mut runs: Vec<RunSummary> = Vec::new();
    let mut current_run: Option<RunSummary> = None;

    for entry in entries.iter().rev() {
        let rid = entry.run_id.clone().unwrap_or_default();
        match &mut current_run {
            Some(run) if run.run_id == rid => {
                run.total += 1;
                if entry.status >= 200 && entry.status < 400 { run.passed += 1; }
                else { run.failed += 1; }
                run.total_ms += entry.response_time_ms;
            }
            _ => {
                if let Some(run) = current_run.take() {
                    runs.push(run);
                }
                let passed = if entry.status >= 200 && entry.status < 400 { 1 } else { 0 };
                current_run = Some(RunSummary {
                    run_id: rid,
                    timestamp: entry.timestamp.clone(),
                    total: 1,
                    passed,
                    failed: 1 - passed,
                    total_ms: entry.response_time_ms,
                });
            }
        }
    }
    if let Some(run) = current_run {
        runs.push(run);
    }

    // Show most recent runs first
    runs.reverse();

    let shown = runs.len().min(max_rows);
    for run in &runs[..shown] {
        let ts = if run.timestamp.len() >= 19 { &run.timestamp[..19] } else { &run.timestamp };
        let pass_rate = if run.total > 0 { (run.passed as f64 / run.total as f64 * 100.0) as u32 } else { 0 };
        let rate_color = if pass_rate == 100 { theme::GREEN() }
            else if pass_rate >= 50 { theme::YELLOW() }
            else { theme::RED() };

        // Build a mini bar: ▓ for passed, ░ for failed
        let bar_width = 10usize;
        let filled = if run.total > 0 { (run.passed * bar_width / run.total).max(if run.passed > 0 { 1 } else { 0 }) } else { 0 };
        let empty = bar_width - filled;
        let bar = format!("{}{}", "▓".repeat(filled), "░".repeat(empty));

        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", ts), dim),
            Span::styled(bar, Style::default().fg(rate_color)),
            Span::styled(format!(" {}%", pass_rate), Style::default().fg(rate_color)),
            Span::styled(format!("  ✓{} ✗{}", run.passed, run.failed), dim),
            Span::styled(format!("  {}ms", run.total_ms), Style::default().fg(theme::TEXT_FAINT())),
        ]));
    }
    if runs.len() > shown {
        lines.push(Line::from(Span::styled(
            format!("  … and {} more runs", runs.len() - shown),
            Style::default().fg(theme::TEXT_FAINT()),
        )));
    }

    lines
}

struct RunSummary {
    run_id: String,
    timestamp: String,
    total: usize,
    passed: usize,
    failed: usize,
    total_ms: u64,
}

/// Render the inspector popup overlay.
pub fn render_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let w = area.width.saturating_sub(6);
    let h = area.height.saturating_sub(4);
    if w < 20 || h < 8 { return; }
    let popup_width = 80.min(w);
    let popup_height = h.min(40);
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let (title, lines) = build_inspect_lines(app);

    let block = Block::default()
        .title(format!("{} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_SURFACE()));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.inspect_scroll, 0));

    frame.render_widget(para, popup_area);

    // Footer hint
    let hint_y = popup_area.y + popup_area.height - 1;
    if hint_y < area.height {
        let hint_area = Rect::new(popup_area.x + 2, hint_y, popup_area.width.saturating_sub(4), 1);
        let hint = Paragraph::new(Line::from(vec![
            Span::styled("↑↓", Style::default().fg(theme::PEACH())),
            Span::styled("=scroll ", Style::default().fg(theme::TEXT_FAINT())),
            Span::styled("i/Esc", Style::default().fg(theme::PEACH())),
            Span::styled("=close", Style::default().fg(theme::TEXT_FAINT())),
        ]));
        frame.render_widget(hint, hint_area);
    }
}

/// Handle keyboard input for the inspector popup.
pub fn handle_inspector_keys(app: &mut App, key: KeyEvent) -> bool {
    if !app.inspect_open { return false; }
    match key.code {
        KeyCode::Esc | KeyCode::Char('i') => {
            app.inspect_open = false;
            app.inspect_scroll = 0;
        }
        KeyCode::Down | KeyCode::Char('j') => { app.inspect_scroll = app.inspect_scroll.saturating_add(1); }
        KeyCode::Up | KeyCode::Char('k') => { app.inspect_scroll = app.inspect_scroll.saturating_sub(1); }
        KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            app.inspect_scroll = app.inspect_scroll.saturating_add(10);
        }
        KeyCode::Char('u') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            app.inspect_scroll = app.inspect_scroll.saturating_sub(10);
        }
        KeyCode::Char('g') => { app.inspect_scroll = 0; }
        KeyCode::Char('G') => { app.inspect_scroll = 200; } // large enough to hit bottom
        _ => {}
    }
    true
}

fn extract_file_description(content: &str) -> Vec<String> {
    let mut desc = Vec::new();
    let mut in_header = false;
    for line in content.lines().take(30) {
        let trimmed = line.trim();
        if trimmed.starts_with("# =") || trimmed.starts_with("# -") {
            in_header = true;
            continue;
        }
        if in_header && trimmed.starts_with('#') {
            let text = trimmed.trim_start_matches('#').trim();
            if !text.is_empty() && !text.starts_with('=') && !text.starts_with('-') && !text.starts_with("@") {
                desc.push(text.to_string());
            }
        }
        if trimmed.starts_with("@variables") || trimmed.starts_with("###") {
            break;
        }
    }
    desc
}

fn method_color(method: &str) -> ratatui::style::Color {
    match method {
        "GET" => theme::GREEN(),
        "POST" => theme::BLUE(),
        "PUT" => theme::YELLOW(),
        "PATCH" => theme::PEACH(),
        "DELETE" => theme::RED(),
        _ => theme::TEXT(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max { format!("{}…", &s[..max]) } else { s.to_string() }
}
