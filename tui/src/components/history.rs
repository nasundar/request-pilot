use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use crate::app::{App, Focus, InputMode, InputPurpose, ConfirmPurpose, HistoryGroupBy, HistoryPopup, Mode};
use crate::ui::theme;
use request_pilot_core::history::{HistoryEntry, HistoryFilter};

const METHOD_OPTIONS: &[&str] = &["All", "GET", "POST", "PUT", "PATCH", "DELETE"];
const STATUS_OPTIONS: &[&str] = &["All", "2xx", "3xx", "4xx", "5xx"];

fn build_filter(app: &App) -> HistoryFilter {
    let mut filter = HistoryFilter::default();
    if !app.filter_text.is_empty() {
        filter.url_contains = Some(app.filter_text.clone());
    }
    if let Some(ref m) = app.history_method_filter {
        filter.method = Some(m.clone());
    }
    if let Some(ref s) = app.history_status_filter {
        match s.as_str() {
            "2xx" => { filter.status_min = Some(200); filter.status_max = Some(299); }
            "3xx" => { filter.status_min = Some(300); filter.status_max = Some(399); }
            "4xx" => { filter.status_min = Some(400); filter.status_max = Some(499); }
            "5xx" => { filter.status_min = Some(500); filter.status_max = Some(599); }
            _ => {}
        }
    }
    filter
}

fn get_filtered_entries<'a>(app: &'a App) -> Vec<&'a HistoryEntry> {
    let filter = build_filter(app);
    let mut entries = app.history.filter(&filter);
    entries.reverse();
    entries
}

fn extract_domain(url: &str) -> String {
    let without_scheme = url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    without_scheme.split('/').next()
        .unwrap_or("unknown")
        .split(':').next()
        .unwrap_or("unknown")
        .to_string()
}

fn status_bucket(status: u16) -> String {
    match status {
        200..=299 => "2xx Success".to_string(),
        300..=399 => "3xx Redirect".to_string(),
        400..=499 => "4xx Client Error".to_string(),
        500..=599 => "5xx Server Error".to_string(),
        _ => format!("{} Other", status),
    }
}

fn source_label(source: &str) -> &str {
    match source {
        "tui" | "test-run" => "Test",
        "extension-live" | "manual" => "Manual",
        "live" => "Live",
        _ => source,
    }
}

fn source_badge_color(source: &str) -> Color {
    match source {
        "tui" | "test-run" => theme::SAPPHIRE(),
        "extension-live" | "manual" => theme::PEACH(),
        "live" => theme::GREEN(),
        _ => theme::TEXT_FAINT(),
    }
}

fn time_color(ms: u64) -> Color {
    if ms < 200 { theme::GREEN() }
    else if ms < 500 { theme::YELLOW() }
    else { theme::RED() }
}

fn method_color(method: &str) -> Color {
    match method {
        "GET" => theme::GREEN(),
        "POST" => theme::BLUE(),
        "PUT" => theme::YELLOW(),
        "PATCH" => theme::PINK(),
        "DELETE" => theme::RED(),
        _ => theme::TEXT(),
    }
}

fn status_color(status: u16) -> Color {
    if status < 300 { theme::GREEN() }
    else if status < 400 { theme::YELLOW() }
    else { theme::RED() }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() { return 0; }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

enum DisplayRow<'a> {
    GroupHeader { label: String, count: usize },
    Entry { entry: &'a HistoryEntry, flat_idx: usize },
}

fn build_display_rows<'a>(app: &'a App, entries: &[&'a HistoryEntry]) -> Vec<DisplayRow<'a>> {
    match app.history_group_by {
        HistoryGroupBy::Flat => {
            entries.iter().enumerate()
                .map(|(i, e)| DisplayRow::Entry { entry: e, flat_idx: i })
                .collect()
        }
        HistoryGroupBy::Domain => build_grouped_rows(entries, |e| extract_domain(&e.url), &app.history_groups_collapsed),
        HistoryGroupBy::Status => build_grouped_rows(entries, |e| status_bucket(e.status), &app.history_groups_collapsed),
        HistoryGroupBy::Source => build_grouped_rows(entries, |e| source_label(&e.source).to_string(), &app.history_groups_collapsed),
        HistoryGroupBy::FileGroupTest => build_file_group_test_rows(entries, &app.history_groups_collapsed),
    }
}

fn build_grouped_rows<'a, F>(
    entries: &[&'a HistoryEntry], key_fn: F, collapsed: &std::collections::HashSet<String>,
) -> Vec<DisplayRow<'a>>
where F: Fn(&HistoryEntry) -> String,
{
    let mut groups: Vec<(String, Vec<(usize, &'a HistoryEntry)>)> = Vec::new();
    let mut group_map: BTreeMap<String, usize> = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate() {
        let key = key_fn(entry);
        if let Some(&idx) = group_map.get(&key) {
            groups[idx].1.push((i, entry));
        } else {
            group_map.insert(key.clone(), groups.len());
            groups.push((key, vec![(i, entry)]));
        }
    }
    let mut rows = Vec::new();
    for (label, members) in &groups {
        rows.push(DisplayRow::GroupHeader { label: label.clone(), count: members.len() });
        if !collapsed.contains(label) {
            for &(flat_idx, entry) in members {
                rows.push(DisplayRow::Entry { entry, flat_idx });
            }
        }
    }
    rows
}

fn build_file_group_test_rows<'a>(
    entries: &[&'a HistoryEntry], collapsed: &std::collections::HashSet<String>,
) -> Vec<DisplayRow<'a>> {
    let mut files: Vec<(String, Vec<(String, Vec<(usize, &'a HistoryEntry)>)>)> = Vec::new();
    let mut file_map: BTreeMap<String, usize> = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate() {
        let file_key = entry.file_name.as_deref().unwrap_or("(no file)").to_string();
        let group_key = entry.group.as_deref().unwrap_or("(default)").to_string();
        let file_idx = if let Some(&idx) = file_map.get(&file_key) { idx } else {
            file_map.insert(file_key.clone(), files.len());
            files.push((file_key.clone(), Vec::new()));
            files.len() - 1
        };
        let groups = &mut files[file_idx].1;
        let gpos = groups.iter().position(|(g, _)| g == &group_key);
        if let Some(gidx) = gpos { groups[gidx].1.push((i, entry)); }
        else { groups.push((group_key, vec![(i, entry)])); }
    }
    let mut rows = Vec::new();
    for (file_label, groups) in &files {
        let file_count: usize = groups.iter().map(|(_, v)| v.len()).sum();
        let fk = format!("file:{}", file_label);
        rows.push(DisplayRow::GroupHeader { label: fk.clone(), count: file_count });
        if collapsed.contains(&fk) { continue; }
        for (group_label, members) in groups {
            let gk = format!("group:{}:{}", file_label, group_label);
            rows.push(DisplayRow::GroupHeader { label: gk.clone(), count: members.len() });
            if collapsed.contains(&gk) { continue; }
            for &(flat_idx, entry) in members { rows.push(DisplayRow::Entry { entry, flat_idx }); }
        }
    }
    rows
}

fn group_display_label(label: &str) -> String {
    if let Some(rest) = label.strip_prefix("file:") { format!("\u{1f4c4} {}", rest) }
    else if let Some(rest) = label.strip_prefix("group:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        format!("  \u{1f4c1} {}", parts.get(1).unwrap_or(&rest))
    } else { label.to_string() }
}
// -- Rendering ---------------------------------------------------------------

pub fn render_history_mode(frame: &mut Frame, app: &App, area: Rect) {
    let entries = get_filtered_entries(app);
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Length(3), Constraint::Min(5)])
        .split(area);
    render_history_stats(frame, app, v_chunks[0], &entries);
    render_history_filter(frame, app, v_chunks[1]);
    render_history_list(frame, app, v_chunks[2], &entries);
    if let Some(ref popup) = app.history_popup { render_filter_popup(frame, area, popup); }
}

fn render_history_stats(frame: &mut Frame, app: &App, area: Rect, entries: &[&HistoryEntry]) {
    let title = format!(" \u{1f4ca} Stats  [g:{}]  [m:{}]  [s:{}] ",
        app.history_group_by.label(),
        app.history_method_filter.as_deref().unwrap_or("All"),
        app.history_status_filter.as_deref().unwrap_or("All"));
    let block = Block::default().title(title).borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()));
    let total = entries.len();
    if total == 0 {
        frame.render_widget(Paragraph::new("No history entries yet. Run some tests!")
            .block(block).style(Style::default().fg(theme::TEXT_FAINT())), area);
        return;
    }
    let success_count = entries.iter().filter(|e| e.status < 400).count();
    let error_count = total - success_count;
    let pass_rate = (success_count as f64 / total as f64) * 100.0;
    let mut method_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in entries { *method_counts.entry(e.method.as_str()).or_insert(0) += 1; }
    let method_parts: Vec<String> = method_counts.iter().map(|(m, c)| format!("{}: {}", m, c)).collect();
    let mut times: Vec<u64> = entries.iter().map(|e| e.response_time_ms).collect();
    times.sort_unstable();
    let avg = times.iter().sum::<u64>() / total as u64;
    let p50 = percentile(&times, 50.0);
    let p90 = percentile(&times, 90.0);
    let p99 = percentile(&times, 99.0);
    let max_t = times.last().copied().unwrap_or(0);
    let lines = vec![
        Line::from(vec![
            Span::styled("  Total: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("{}", total), Style::default().fg(theme::TEXT()).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled("\u{2713} ", Style::default().fg(theme::GREEN())),
            Span::styled(format!("{}", success_count), Style::default().fg(theme::GREEN())),
            Span::raw("  "),
            Span::styled("\u{2717} ", Style::default().fg(theme::RED())),
            Span::styled(format!("{}", error_count), Style::default().fg(theme::RED())),
            Span::raw("    "),
            Span::styled("Pass rate: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("{:.1}%", pass_rate),
                Style::default().fg(if pass_rate >= 80.0 { theme::GREEN() } else if pass_rate >= 50.0 { theme::YELLOW() } else { theme::RED() }).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  Methods: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(method_parts.join(", "), Style::default().fg(theme::TEXT())),
        ]),
        Line::from(vec![
            Span::styled("  Timing: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("avg {}ms", avg), Style::default().fg(time_color(avg))),
            Span::styled("  p50 ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("{}ms", p50), Style::default().fg(time_color(p50))),
            Span::styled("  p90 ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("{}ms", p90), Style::default().fg(time_color(p90))),
            Span::styled("  p99 ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("{}ms", p99), Style::default().fg(time_color(p99))),
            Span::styled("  max ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("{}ms", max_t), Style::default().fg(time_color(max_t))),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_history_filter(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::FilterInput { Style::default().fg(theme::BLUE()) } else { Style::default().fg(theme::TEXT_FAINT()) };
    let mut af: Vec<String> = Vec::new();
    if let Some(ref m) = app.history_method_filter { af.push(format!("method={}", m)); }
    if let Some(ref s) = app.history_status_filter { af.push(format!("status={}", s)); }
    let fi = if af.is_empty() { String::new() } else { format!(" [{}]", af.join(" ")) };
    let title = format!(" \u{1f50d} Filter (/ search, m method, s status){} ", fi);
    let block = Block::default().title(title).borders(Borders::ALL).border_style(border_style);
    let text = if app.filter_text.is_empty() {
        if app.focus == Focus::FilterInput { "Type to filter..." } else { "Press / to filter" }
    } else { &app.filter_text };
    let style = if app.filter_text.is_empty() { Style::default().fg(theme::TEXT_FAINT()) } else { Style::default().fg(theme::TEXT()) };
    frame.render_widget(Paragraph::new(Span::styled(text, style)).block(block), area);
}
fn render_history_list(frame: &mut Frame, app: &App, area: Rect, entries: &[&HistoryEntry]) {
    let border_style = if app.focus == Focus::HistoryList { Style::default().fg(theme::BLUE()) } else { Style::default().fg(theme::TEXT_FAINT()) };
    let ci = if !app.history_selected_seqs.is_empty() { format!(" ({}/2 selected)", app.history_selected_seqs.len()) } else { String::new() };
    let title = format!(" History \u{2014} {} [g cycle, Space sel, d diff]{} ", app.history_group_by.label(), ci);
    let block = Block::default().title(title).borders(Borders::ALL).border_style(border_style);
    if entries.is_empty() {
        let msg = if app.filter_text.is_empty() && app.history_method_filter.is_none() && app.history_status_filter.is_none() { "No history entries. Run tests to populate." } else { "No matching entries." };
        frame.render_widget(Paragraph::new(msg).block(block).style(Style::default().fg(theme::TEXT_FAINT())), area);
        return;
    }
    let rows = build_display_rows(app, entries);
    let items: Vec<ListItem> = rows.iter().enumerate().map(|(i, row)| {
        match row {
            DisplayRow::GroupHeader { label, count } => {
                let is_sel = i == app.history_cursor;
                let bg = if is_sel { theme::BG_HIGHLIGHT() } else { Color::Reset };
                let is_col = app.history_groups_collapsed.contains(label);
                let marker = if is_col { "\u{25b6}" } else { "\u{25bc}" };
                let display = group_display_label(label);
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} {} ", marker, display), Style::default().fg(theme::LAVENDER()).bg(bg).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("({})", count), Style::default().fg(theme::TEXT_DIM()).bg(bg)),
                ]))
            }
            DisplayRow::Entry { entry, .. } => {
                let is_sel = i == app.history_cursor;
                let is_cmp = app.history_selected_seqs.contains(&entry.seq);
                let bg = if is_sel { theme::BG_HIGHLIGHT() } else if is_cmp { theme::BG_OVERLAY() } else { Color::Reset };
                let cm = if is_cmp { "\u{25c6} " } else { "  " };
                let ts = if entry.timestamp.len() >= 19 { &entry.timestamp[11..19] } else { &entry.timestamp };
                let badge = source_label(&entry.source);
                let bc = source_badge_color(&entry.source);
                ListItem::new(Line::from(vec![
                    Span::styled(cm, Style::default().fg(theme::MAUVE()).bg(bg)),
                    Span::styled(format!(" {} ", entry.status), Style::default().fg(theme::BG_DARK()).bg(status_color(entry.status))),
                    Span::styled(" ", Style::default().bg(bg)),
                    Span::styled(format!("{:<7}", entry.method), Style::default().fg(method_color(&entry.method)).bg(bg)),
                    Span::styled(&entry.url, Style::default().fg(theme::TEXT()).bg(bg)),
                    Span::styled("  ", Style::default().bg(bg)),
                    Span::styled(format!("{}ms", entry.response_time_ms), Style::default().fg(time_color(entry.response_time_ms)).bg(bg)),
                    Span::styled("  ", Style::default().bg(bg)),
                    Span::styled(format!("[{}]", badge), Style::default().fg(bc).bg(bg)),
                    Span::styled("  ", Style::default().bg(bg)),
                    Span::styled(ts, Style::default().fg(theme::TEXT_FAINT()).bg(bg)),
                ]))
            }
        }
    }).collect();
    frame.render_widget(List::new(items).block(block), area);
}

fn render_filter_popup(frame: &mut Frame, area: Rect, popup: &HistoryPopup) {
    let (title, options, cursor) = match popup {
        HistoryPopup::MethodFilter { cursor } => ("Method Filter", METHOD_OPTIONS, *cursor),
        HistoryPopup::StatusFilter { cursor } => ("Status Filter", STATUS_OPTIONS, *cursor),
    };
    let pw = 30u16.min(area.width.saturating_sub(4));
    let ph = (options.len() as u16 + 2).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(pw)) / 2;
    let y = (area.height.saturating_sub(ph)) / 2;
    let pa = Rect::new(x, y, pw, ph);
    frame.render_widget(ratatui::widgets::Clear, pa);
    let block = Block::default().title(format!(" {} ", title)).borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE())).style(Style::default().bg(theme::BG_OVERLAY()));
    let items: Vec<ListItem> = options.iter().enumerate().map(|(i, opt)| {
        let is_sel = i == cursor;
        let style = if is_sel { Style::default().fg(theme::BG_DARK()).bg(theme::BLUE()).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme::TEXT()) };
        let m = if is_sel { "\u{25b8} " } else { "  " };
        ListItem::new(Line::from(Span::styled(format!("{}{}", m, opt), style)))
    }).collect();
    frame.render_widget(List::new(items).block(block), pa);
}
// -- Event handling ----------------------------------------------------------

pub fn handle_history_popup_keys(app: &mut App, key: KeyEvent) {
    let popup = match app.history_popup.as_mut() { Some(p) => p, None => return };
    match popup {
        HistoryPopup::MethodFilter { cursor } => match key.code {
            KeyCode::Esc => { app.history_popup = None; }
            KeyCode::Up | KeyCode::Char('k') => { *cursor = cursor.saturating_sub(1); }
            KeyCode::Down | KeyCode::Char('j') => { if *cursor < METHOD_OPTIONS.len() - 1 { *cursor += 1; } }
            KeyCode::Enter => {
                let sel = METHOD_OPTIONS[*cursor];
                app.history_method_filter = if sel == "All" { None } else { Some(sel.to_string()) };
                app.history_cursor = 0;
                app.history_popup = None;
            }
            _ => {}
        },
        HistoryPopup::StatusFilter { cursor } => match key.code {
            KeyCode::Esc => { app.history_popup = None; }
            KeyCode::Up | KeyCode::Char('k') => { *cursor = cursor.saturating_sub(1); }
            KeyCode::Down | KeyCode::Char('j') => { if *cursor < STATUS_OPTIONS.len() - 1 { *cursor += 1; } }
            KeyCode::Enter => {
                let sel = STATUS_OPTIONS[*cursor];
                app.history_status_filter = if sel == "All" { None } else { Some(sel.to_string()) };
                app.history_cursor = 0;
                app.history_popup = None;
            }
            _ => {}
        },
    }
}

pub fn handle_history_keys(app: &mut App, key: KeyEvent) {
    match app.focus {
        Focus::HistoryList => match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Files;
                app.focus = Focus::FileTree;
            }
            KeyCode::Up | KeyCode::Char('k') => { app.history_cursor = app.history_cursor.saturating_sub(1); }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = display_row_count(app).saturating_sub(1);
                if app.history_cursor < max { app.history_cursor += 1; }
            }
            KeyCode::Char('g') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.history_group_by = app.history_group_by.next();
                app.history_cursor = 0;
                app.history_groups_collapsed.clear();
                app.set_status(format!("Group by: {}", app.history_group_by.label()));
            }
            KeyCode::Char('G') => { app.history_cursor = display_row_count(app).saturating_sub(1); }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let max = display_row_count(app).saturating_sub(1);
                app.history_cursor = (app.history_cursor + 10).min(max);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.history_cursor = app.history_cursor.saturating_sub(10);
            }
            KeyCode::Char('/') => { app.focus = Focus::FilterInput; }
            KeyCode::Char('m') => {
                let cur = match &app.history_method_filter {
                    None => 0, Some(m) => METHOD_OPTIONS.iter().position(|o| o == m).unwrap_or(0),
                };
                app.history_popup = Some(HistoryPopup::MethodFilter { cursor: cur });
            }
            KeyCode::Char('s') => {
                let cur = match &app.history_status_filter {
                    None => 0, Some(s) => STATUS_OPTIONS.iter().position(|o| o == s).unwrap_or(0),
                };
                app.history_popup = Some(HistoryPopup::StatusFilter { cursor: cur });
            }
            KeyCode::Char(' ') => {
                let entries = get_filtered_entries(app);
                let rows = build_display_rows(app, &entries);
                if let Some(DisplayRow::Entry { entry, .. }) = rows.get(app.history_cursor) {
                    let seq = entry.seq;
                    if let Some(pos) = app.history_selected_seqs.iter().position(|&s| s == seq) {
                        app.history_selected_seqs.remove(pos);
                    } else if app.history_selected_seqs.len() < 2 {
                        app.history_selected_seqs.push(seq);
                    } else {
                        app.set_status("Max 2 entries for compare. Deselect first.".to_string());
                    }
                }
            }
            KeyCode::Char('d') => {
                if app.history_selected_seqs.len() == 2 {
                    app.set_status("Compare: placeholder \u{2014} will be implemented by tui2-comparison".to_string());
                } else {
                    app.set_status("Select 2 entries with Space to compare".to_string());
                }
            }
            KeyCode::Char('C') => {
                app.input_mode = InputMode::Confirm {
                    prompt: "Clear all history? (y/n)".to_string(),
                    purpose: ConfirmPurpose::ClearHistory,
                };
            }
            KeyCode::Enter => {
                let entries = get_filtered_entries(app);
                let rows = build_display_rows(app, &entries);
                if let Some(row) = rows.get(app.history_cursor) {
                    match row {
                        DisplayRow::Entry { flat_idx, .. } => {
                            app.history_detail_idx = Some(*flat_idx);
                            app.history_detail_scroll = 0;
                            app.history_detail_tab = 0;
                        }
                        DisplayRow::GroupHeader { label, .. } => {
                            let label = label.clone();
                            drop(rows);
                            drop(entries);
                            toggle_group_collapse(app, &label);
                        }
                    }
                }
            }
            KeyCode::Char('E') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.history_groups_collapsed.clear();
                app.set_status("All groups expanded".to_string());
            }
            KeyCode::Char('W') => {
                let entries = get_filtered_entries(app);
                let rows = build_display_rows(app, &entries);
                let labels: Vec<String> = rows.iter().filter_map(|row| {
                    if let DisplayRow::GroupHeader { label, .. } = row {
                        Some(label.clone())
                    } else {
                        None
                    }
                }).collect();
                drop(rows);
                drop(entries);
                for label in labels {
                    app.history_groups_collapsed.insert(label);
                }
                app.history_cursor = 0;
                app.set_status("All groups collapsed".to_string());
            }
            KeyCode::Char('e') => {
                app.input_mode = InputMode::Input {
                    prompt: "Export history to: ".to_string(),
                    purpose: InputPurpose::ExportHistory,
                    buffer: "history.json".to_string(),
                };
            }
            _ => {}
        },
        Focus::FilterInput => match key.code {
            KeyCode::Esc | KeyCode::Enter => { app.focus = Focus::HistoryList; }
            KeyCode::Char(c) => { app.filter_text.push(c); app.history_cursor = 0; }
            KeyCode::Backspace => { app.filter_text.pop(); app.history_cursor = 0; }
            _ => {}
        },
        _ => {}
    }
}

fn toggle_group_collapse(app: &mut App, label: &str) {
    if app.history_groups_collapsed.contains(label) {
        app.history_groups_collapsed.remove(label);
    } else {
        app.history_groups_collapsed.insert(label.to_string());
    }
}

fn display_row_count(app: &App) -> usize {
    let entries = get_filtered_entries(app);
    build_display_rows(app, &entries).len()
}
