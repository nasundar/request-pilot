use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;
use crate::app::{App, Focus, ResponseTab};
use crate::ui::theme;
use crate::ui::truncate_to;
use crate::components::sidebar::find_block_result;

const LARGE_BODY_THRESHOLD: usize = 262_144;
const PREVIEW_SIZE: usize = 8_192;
pub const DEFAULT_EXPAND_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonNodeType {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

#[derive(Debug, Clone)]
pub struct JsonTreeNode {
    pub path: std::string::String,
    pub key: std::string::String,
    pub value_preview: std::string::String,
    pub depth: usize,
    pub is_expandable: bool,
    pub is_expanded: bool,
    pub node_type: JsonNodeType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentKind {
    Json,
    Xml,
    Html,
    Yaml,
    Text,
}

pub(crate) fn detect_content_kind(headers: &[(String, String)], body: &str) -> ContentKind {
    let ct = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.to_lowercase())
        .unwrap_or_default();
    if ct.contains("json") {
        return ContentKind::Json;
    }
    if ct.contains("xml") {
        return ContentKind::Xml;
    }
    if ct.contains("html") {
        return ContentKind::Html;
    }
    if ct.contains("yaml") || ct.contains("yml") {
        return ContentKind::Yaml;
    }
    let trimmed = body.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(body).is_ok()
    {
        return ContentKind::Json;
    }
    if trimmed.starts_with("<?xml") || (trimmed.starts_with('<') && trimmed.contains("xmlns")) {
        return ContentKind::Xml;
    }
    if trimmed.starts_with("<!DOCTYPE html") || trimmed.starts_with("<html") {
        return ContentKind::Html;
    }
    ContentKind::Text
}

fn content_kind_badge(kind: ContentKind) -> (&'static str, ratatui::style::Color) {
    match kind {
        ContentKind::Json => ("JSON", theme::PEACH()),
        ContentKind::Xml => ("XML", theme::SAPPHIRE()),
        ContentKind::Html => ("HTML", theme::LAVENDER()),
        ContentKind::Yaml => ("YAML", theme::MAUVE()),
        ContentKind::Text => ("TEXT", theme::TEXT_DIM()),
    }
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1_048_576 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}

pub fn build_json_tree(
    value: &serde_json::Value,
    path: &str,
    key: &str,
    depth: usize,
    expanded: &HashSet<String>,
    nodes: &mut Vec<JsonTreeNode>,
) {
    let auto_expand = depth < DEFAULT_EXPAND_DEPTH;
    let is_expanded =
        expanded.contains(path) || (auto_expand && !expanded.contains(&format!("!{}", path)));
    match value {
        serde_json::Value::Object(map) => {
            nodes.push(JsonTreeNode {
                path: path.to_string(),
                key: key.to_string(),
                value_preview: format!("{{ {} keys }}", map.len()),
                depth,
                is_expandable: true,
                is_expanded,
                node_type: JsonNodeType::Object,
            });
            if is_expanded {
                for (k, v) in map {
                    let cp = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{}.{}", path, k)
                    };
                    build_json_tree(v, &cp, k, depth + 1, expanded, nodes);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            nodes.push(JsonTreeNode {
                path: path.to_string(),
                key: key.to_string(),
                value_preview: format!("[ {} items ]", arr.len()),
                depth,
                is_expandable: true,
                is_expanded,
                node_type: JsonNodeType::Array,
            });
            if is_expanded {
                for (i, v) in arr.iter().enumerate() {
                    let cp = format!("{}[{}]", path, i);
                    let ck = format!("[{}]", i);
                    build_json_tree(v, &cp, &ck, depth + 1, expanded, nodes);
                }
            }
        }
        serde_json::Value::String(s) => {
            let preview = if s.len() > 80 {
                format!("\"{}...\"", &s[..77])
            } else {
                format!("\"{}\"", s)
            };
            nodes.push(JsonTreeNode {
                path: path.to_string(),
                key: key.to_string(),
                value_preview: preview,
                depth,
                is_expandable: false,
                is_expanded: false,
                node_type: JsonNodeType::String,
            });
        }
        serde_json::Value::Number(n) => {
            nodes.push(JsonTreeNode {
                path: path.to_string(),
                key: key.to_string(),
                value_preview: n.to_string(),
                depth,
                is_expandable: false,
                is_expanded: false,
                node_type: JsonNodeType::Number,
            });
        }
        serde_json::Value::Bool(b) => {
            nodes.push(JsonTreeNode {
                path: path.to_string(),
                key: key.to_string(),
                value_preview: b.to_string(),
                depth,
                is_expandable: false,
                is_expanded: false,
                node_type: JsonNodeType::Bool,
            });
        }
        serde_json::Value::Null => {
            nodes.push(JsonTreeNode {
                path: path.to_string(),
                key: key.to_string(),
                value_preview: "null".to_string(),
                depth,
                is_expandable: false,
                is_expanded: false,
                node_type: JsonNodeType::Null,
            });
        }
    }
}

pub fn rebuild_json_tree(app: &mut App, body: &str) {
    app.json_tree_nodes.clear();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        build_json_tree(
            &value,
            "root",
            "root",
            0,
            &app.json_expanded,
            &mut app.json_tree_nodes,
        );
    }
}

pub fn collect_all_paths(value: &serde_json::Value, path: &str, paths: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            paths.insert(path.to_string());
            for (k, v) in map {
                let c = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", path, k)
                };
                collect_all_paths(v, &c, paths);
            }
        }
        serde_json::Value::Array(arr) => {
            paths.insert(path.to_string());
            for (i, v) in arr.iter().enumerate() {
                collect_all_paths(v, &format!("{}[{}]", path, i), paths);
            }
        }
        _ => {}
    }
}

fn highlight_xml_line(line: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '>' && chars[i] != ' ' {
                i += 1;
            }
            let tag: String = chars[start..i].iter().collect();
            spans.push(Span::styled(tag, Style::default().fg(theme::BLUE())));
            while i < chars.len() && chars[i] != '>' {
                if chars[i] == '"' {
                    let qs = i;
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        i += 1;
                    }
                    if i < chars.len() {
                        i += 1;
                    }
                    let av: String = chars[qs..i].iter().collect();
                    spans.push(Span::styled(av, Style::default().fg(theme::GREEN())));
                } else if chars[i] == ' ' {
                    spans.push(Span::styled(" ", Style::default().fg(theme::TEXT())));
                    i += 1;
                    let attr_start = i;
                    while i < chars.len()
                        && chars[i] != '='
                        && chars[i] != '>'
                        && chars[i] != ' '
                    {
                        i += 1;
                    }
                    if i > attr_start {
                        let an: String = chars[attr_start..i].iter().collect();
                        spans.push(Span::styled(an, Style::default().fg(theme::SAPPHIRE())));
                    }
                    if i < chars.len() && chars[i] == '=' {
                        spans.push(Span::styled("=", Style::default().fg(theme::TEXT_DIM())));
                        i += 1;
                    }
                } else {
                    spans.push(Span::styled(
                        chars[i].to_string(),
                        Style::default().fg(theme::TEXT()),
                    ));
                    i += 1;
                }
            }
            if i < chars.len() && chars[i] == '>' {
                spans.push(Span::styled(">", Style::default().fg(theme::BLUE())));
                i += 1;
            }
        } else {
            let start = i;
            while i < chars.len() && chars[i] != '<' {
                i += 1;
            }
            let t: String = chars[start..i].iter().collect();
            spans.push(Span::styled(t, Style::default().fg(theme::TEXT())));
        }
    }
    Line::from(spans)
}

fn highlight_yaml_line(line: &str) -> Line<'static> {
    if let Some(cp) = line.find(':') {
        Line::from(vec![
            Span::styled(line[..cp].to_string(), Style::default().fg(theme::BLUE())),
            Span::styled(line[cp..].to_string(), Style::default().fg(theme::TEXT())),
        ])
    } else if line.trim_start().starts_with('#') {
        Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme::TEXT_FAINT()),
        ))
    } else {
        Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme::TEXT()),
        ))
    }
}

fn http_status_color(status: u16) -> ratatui::style::Color {
    match status {
        200..=299 => theme::GREEN(),
        300..=399 => theme::BLUE(),
        400..=499 => theme::YELLOW(),
        500..=599 => theme::RED(),
        _ => theme::TEXT(),
    }
}

pub fn render_response(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::Response {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };
    let tab_style = |tab: ResponseTab| {
        if tab == app.response_tab {
            Style::default()
                .fg(theme::BG_DARK())
                .bg(theme::BLUE())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT_FAINT())
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
    let is_focused = app.focus == Focus::Response;
    let block_widget = Block::default()
        .title(title_line)
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(if is_focused {
            Style::default().bg(theme::BG_FOCUS())
        } else {
            Style::default()
        });

    if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(ref results) = file.results {
                let block_name = file.suite.blocks.get(bi).map(|b| b.name.as_str()).unwrap_or("");
                if let Some(br) = find_block_result(&results.block_results, block_name) {
                    if br.status == "pending" {
                        let msg = Paragraph::new("Run tests to see responses here")
                            .block(block_widget)
                            .style(Style::default().fg(theme::TEXT_FAINT()));
                        frame.render_widget(msg, area);
                        return;
                    }
                    let mut lines = Vec::new();
                    match app.response_tab {
                        ResponseTab::Body => {
                            // Check if this is a compare block
                            let active_block = file.suite.blocks.get(bi);
                            let is_compare = active_block.map(|b| b.compare).unwrap_or(false);

                            if is_compare && !br.step_results.is_empty() {
                                // === Compare block: show step results and diff summary ===
                                let sc = match br.status.as_str() {
                                    "passed" => theme::GREEN(),
                                    "failed" | "error" => theme::RED(),
                                    "skipped" => theme::YELLOW(),
                                    _ => theme::TEXT(),
                                };
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        format!(" {} ", br.status.to_uppercase()),
                                        Style::default().fg(theme::BG_DARK()).bg(sc).add_modifier(Modifier::BOLD),
                                    ),
                                    Span::styled(
                                        format!(" \u{21C4} Compare \u{00b7} {}ms", br.time_ms),
                                        Style::default().fg(theme::TEXT_DIM()),
                                    ),
                                ]));
                                lines.push(Line::from(""));

                                // Show each step's result
                                lines.push(Line::from(Span::styled(
                                    "\u{2500}\u{2500}\u{2500} Steps \u{2500}\u{2500}\u{2500}",
                                    Style::default().fg(theme::TEXT_FAINT()),
                                )));
                                for sr in &br.step_results {
                                    let (icon, color) = if sr.error.is_some() {
                                        ("\u{2717}", theme::RED())
                                    } else if sr.response.is_some() {
                                        ("\u{2713}", theme::GREEN())
                                    } else {
                                        ("\u{00b7}", theme::TEXT_FAINT())
                                    };
                                    let status_code = sr.response.as_ref().map(|r| r.status).unwrap_or(0);
                                    let mut step_spans = vec![
                                        Span::styled(format!(" {} ", icon), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                                        Span::styled(sr.name.clone(), Style::default().fg(theme::TEXT()).add_modifier(Modifier::BOLD)),
                                    ];
                                    if status_code > 0 {
                                        step_spans.push(Span::styled(
                                            format!("  {} \u{00b7} {}ms", status_code, sr.time_ms),
                                            Style::default().fg(theme::TEXT_DIM()),
                                        ));
                                    }
                                    if let Some(ref err) = sr.error {
                                        step_spans.push(Span::styled(
                                            format!("  {}", err),
                                            Style::default().fg(theme::RED()),
                                        ));
                                    }
                                    lines.push(Line::from(step_spans));

                                    // Show step assertions inline
                                    for ar in &sr.assertion_results {
                                        let (ai, ac) = if ar.passed { ("  \u{2713}", theme::GREEN()) } else { ("  \u{2717}", theme::RED()) };
                                        lines.push(Line::from(Span::styled(
                                            format!("   {} {}", ai, ar.assertion),
                                            Style::default().fg(ac),
                                        )));
                                    }
                                }

                                // Show diff summary
                                if let Some(ref diff) = br.diff_result {
                                    lines.push(Line::from(""));
                                    lines.push(Line::from(Span::styled(
                                        "\u{2500}\u{2500}\u{2500} Diff \u{2500}\u{2500}\u{2500}",
                                        Style::default().fg(theme::TEXT_FAINT()),
                                    )));
                                    let match_label = if diff.match_exact {
                                        Span::styled(" \u{2713} Exact Match ", Style::default().fg(theme::BG_DARK()).bg(theme::GREEN()).add_modifier(Modifier::BOLD))
                                    } else {
                                        let pct = (diff.similarity * 100.0) as u32;
                                        Span::styled(
                                            format!(" {}% similar ", pct),
                                            Style::default().fg(theme::BG_DARK()).bg(theme::YELLOW()).add_modifier(Modifier::BOLD),
                                        )
                                    };
                                    lines.push(Line::from(vec![
                                        Span::raw(" "),
                                        match_label,
                                        Span::styled(
                                            format!("  +{} -{} ~{}", diff.added_count, diff.removed_count, diff.changed_count),
                                            Style::default().fg(theme::TEXT_DIM()),
                                        ),
                                    ]));

                                    // Show changed paths
                                    if !diff.changed_paths.is_empty() {
                                        lines.push(Line::from(""));
                                        lines.push(Line::from(Span::styled(
                                            "Changed fields:",
                                            Style::default().fg(theme::TEXT_DIM()),
                                        )));
                                        for cp in diff.changed_paths.iter().take(20) {
                                            lines.push(Line::from(Span::styled(
                                                format!("  {} ", cp.path),
                                                Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD),
                                            )));
                                            lines.push(Line::from(vec![
                                                Span::styled("    - ", Style::default().fg(theme::RED())),
                                                Span::styled(truncate_to(&cp.left, (area.width as usize).saturating_sub(8)), Style::default().fg(theme::RED())),
                                            ]));
                                            lines.push(Line::from(vec![
                                                Span::styled("    + ", Style::default().fg(theme::GREEN())),
                                                Span::styled(truncate_to(&cp.right, (area.width as usize).saturating_sub(8)), Style::default().fg(theme::GREEN())),
                                            ]));
                                        }
                                        if diff.changed_paths.len() > 20 {
                                            lines.push(Line::from(Span::styled(
                                                format!("  ... and {} more", diff.changed_paths.len() - 20),
                                                Style::default().fg(theme::TEXT_FAINT()),
                                            )));
                                        }
                                    }

                                    // Show added/removed paths
                                    if !diff.added_paths.is_empty() {
                                        lines.push(Line::from(""));
                                        lines.push(Line::from(Span::styled(
                                            format!("Added paths ({}):", diff.added_paths.len()),
                                            Style::default().fg(theme::GREEN()),
                                        )));
                                        for p in diff.added_paths.iter().take(10) {
                                            lines.push(Line::from(Span::styled(
                                                format!("  + {}", p),
                                                Style::default().fg(theme::GREEN()),
                                            )));
                                        }
                                    }
                                    if !diff.removed_paths.is_empty() {
                                        lines.push(Line::from(""));
                                        lines.push(Line::from(Span::styled(
                                            format!("Removed paths ({}):", diff.removed_paths.len()),
                                            Style::default().fg(theme::RED()),
                                        )));
                                        for p in diff.removed_paths.iter().take(10) {
                                            lines.push(Line::from(Span::styled(
                                                format!("  - {}", p),
                                                Style::default().fg(theme::RED()),
                                            )));
                                        }
                                    }

                                    lines.push(Line::from(""));
                                    lines.push(Line::from(Span::styled(
                                        "Press D to view full diff",
                                        Style::default().fg(theme::TEXT_FAINT()),
                                    )));
                                }
                            } else {
                                // === Normal block (existing code) ===
                                let sc = match br.status.as_str() {
                                    "passed" => theme::GREEN(),
                                    "failed" | "error" => theme::RED(),
                                    "skipped" => theme::YELLOW(),
                                    _ => theme::TEXT(),
                                };
                                let mut ss = vec![Span::styled(
                                    format!(" {} ", br.status.to_uppercase()),
                                    Style::default()
                                        .fg(theme::BG_DARK())
                                        .bg(sc)
                                        .add_modifier(Modifier::BOLD),
                                )];
                                if let Some(ref resp) = br.response {
                                    let kind = detect_content_kind(&resp.headers, &resp.body);
                                    let (bt, bc) = content_kind_badge(kind);
                                    ss.push(Span::raw(" "));
                                    ss.push(Span::styled(
                                        format!(" {} ", bt),
                                        Style::default()
                                            .fg(theme::BG_DARK())
                                            .bg(bc)
                                            .add_modifier(Modifier::BOLD),
                                    ));
                                }
                                ss.push(Span::styled(
                                    format!(" \u{00b7} {}ms", br.time_ms),
                                    Style::default().fg(theme::TEXT_DIM()),
                                ));
                                if let Some(ref resp) = br.response {
                                    if resp.size_bytes > 0 {
                                        ss.push(Span::styled(
                                            format!(
                                                " \u{00b7} {}",
                                                format_size(resp.size_bytes)
                                            ),
                                            Style::default().fg(theme::TEXT_DIM()),
                                        ));
                                    }
                                }
                                lines.push(Line::from(ss));
                                if let Some(ref resp) = br.response {
                                    lines.push(Line::from(""));
                                    lines.push(Line::from(vec![
                                        Span::styled(
                                            "HTTP ",
                                            Style::default().fg(theme::TEXT_FAINT()),
                                        ),
                                        Span::styled(
                                            format!("{} {}", resp.status, resp.status_text),
                                            Style::default()
                                                .fg(http_status_color(resp.status)),
                                        ),
                                    ]));
                                    if !resp.body.is_empty() {
                                        let blen = resp.body.len();
                                        let is_large = blen > LARGE_BODY_THRESHOLD;
                                        let kind =
                                            detect_content_kind(&resp.headers, &resp.body);
                                        if is_large && !app.body_fully_loaded {
                                            lines.push(Line::from(""));
                                            lines.push(Line::from(Span::styled(
                                                format!(
                                                    "\u{26a0} Response is {} \u{2014} showing first 8 KiB preview (press l to load full)",
                                                    format_size(blen)
                                                ),
                                                Style::default()
                                                    .fg(theme::YELLOW())
                                                    .add_modifier(Modifier::BOLD),
                                            )));
                                            lines.push(Line::from(""));
                                            let preview = &resp.body
                                                [..PREVIEW_SIZE.min(resp.body.len())];
                                            render_body_content(
                                                preview, kind, app, &mut lines,
                                            );
                                        } else {
                                            let bt = if is_large {
                                                app.response_body_full
                                                    .as_deref()
                                                    .unwrap_or(&resp.body)
                                            } else {
                                                &resp.body
                                            };
                                            lines.push(Line::from(""));
                                            render_body_content(
                                                bt, kind, app, &mut lines,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        ResponseTab::Headers => {
                            if let Some(ref resp) = br.response {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        "HTTP ",
                                        Style::default().fg(theme::TEXT_FAINT()),
                                    ),
                                    Span::styled(
                                        format!("{} {}", resp.status, resp.status_text),
                                        Style::default()
                                            .fg(http_status_color(resp.status)),
                                    ),
                                ]));
                                lines.push(Line::from(""));
                                for (k, v) in &resp.headers {
                                    lines.push(Line::from(vec![
                                        Span::styled(
                                            k.to_string(),
                                            Style::default().fg(theme::LAVENDER()),
                                        ),
                                        Span::raw(": "),
                                        Span::styled(
                                            v.to_string(),
                                            Style::default().fg(theme::TEXT_DIM()),
                                        ),
                                    ]));
                                }
                            } else {
                                lines.push(Line::from(Span::styled(
                                    "No response headers",
                                    Style::default().fg(theme::TEXT_FAINT()),
                                )));
                            }
                        }
                        ResponseTab::Assertions => {
                            if !br.assertion_results.is_empty() {
                                for ar in &br.assertion_results {
                                    let (icon, color) = if ar.passed {
                                        ("\u{2713}", theme::GREEN())
                                    } else {
                                        ("\u{2717}", theme::RED())
                                    };
                                    let max_a = (area.width as usize).saturating_sub(6);
                                    let display_a = truncate_to(&ar.assertion, max_a);
                                    lines.push(Line::from(Span::styled(
                                        format!(" {} {}", icon, display_a),
                                        Style::default()
                                            .fg(color)
                                            .add_modifier(Modifier::BOLD),
                                    )));
                                }
                                // Show step-level assertions for compare blocks
                                if !br.step_results.is_empty() {
                                    for sr in &br.step_results {
                                        if !sr.assertion_results.is_empty() {
                                            lines.push(Line::from(""));
                                            lines.push(Line::from(Span::styled(
                                                format!("\u{2500}\u{2500}\u{2500} Step: {} \u{2500}\u{2500}\u{2500}", sr.name),
                                                Style::default().fg(theme::SAPPHIRE()),
                                            )));
                                            for ar in &sr.assertion_results {
                                                let (icon, color) = if ar.passed {
                                                    ("\u{2713}", theme::GREEN())
                                                } else {
                                                    ("\u{2717}", theme::RED())
                                                };
                                                let max_a = (area.width as usize).saturating_sub(6);
                                                let display_a = truncate_to(&ar.assertion, max_a);
                                                lines.push(Line::from(Span::styled(
                                                    format!(" {} {}", icon, display_a),
                                                    Style::default()
                                                        .fg(color)
                                                        .add_modifier(Modifier::BOLD),
                                                )));
                                            }
                                        }
                                    }
                                }
                            } else if !br.step_results.is_empty() {
                                // Compare block with no block-level assertions — show step assertions directly
                                for sr in &br.step_results {
                                    if !sr.assertion_results.is_empty() {
                                        lines.push(Line::from(Span::styled(
                                            format!("\u{2500}\u{2500}\u{2500} Step: {} \u{2500}\u{2500}\u{2500}", sr.name),
                                            Style::default().fg(theme::SAPPHIRE()),
                                        )));
                                        for ar in &sr.assertion_results {
                                            let (icon, color) = if ar.passed {
                                                ("\u{2713}", theme::GREEN())
                                            } else {
                                                ("\u{2717}", theme::RED())
                                            };
                                            let max_a = (area.width as usize).saturating_sub(6);
                                            let display_a = truncate_to(&ar.assertion, max_a);
                                            lines.push(Line::from(Span::styled(
                                                format!(" {} {}", icon, display_a),
                                                Style::default()
                                                    .fg(color)
                                                    .add_modifier(Modifier::BOLD),
                                            )));
                                        }
                                        lines.push(Line::from(""));
                                    }
                                }
                            } else {
                                lines.push(Line::from(Span::styled(
                                    "No assertions defined",
                                    Style::default().fg(theme::TEXT_FAINT()),
                                )));
                            }
                            if !br.extract_results.is_empty() {
                                lines.push(Line::from(""));
                                lines.push(Line::from(Span::styled(
                                    "\u{2500}\u{2500}\u{2500} Extracts \u{2500}\u{2500}\u{2500}",
                                    Style::default().fg(theme::TEXT_FAINT()),
                                )));
                                for er in &br.extract_results {
                                    let vs =
                                        er.value.as_deref().unwrap_or("(none)");
                                    lines.push(Line::from(vec![
                                        Span::styled(
                                            er.variable.to_string(),
                                            Style::default().fg(theme::MAUVE()),
                                        ),
                                        Span::styled(
                                            " = ",
                                            Style::default().fg(theme::TEXT_FAINT()),
                                        ),
                                        Span::styled(
                                            vs.to_string(),
                                            Style::default().fg(theme::TEXT()),
                                        ),
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
        .style(Style::default().fg(theme::TEXT_FAINT()));
    frame.render_widget(msg, area);
}

fn render_body_content(
    body: &str,
    kind: ContentKind,
    app: &App,
    lines: &mut Vec<Line<'static>>,
) {
    match kind {
        ContentKind::Json => render_json_tree(app, lines),
        ContentKind::Xml | ContentKind::Html => {
            for l in body.lines() {
                lines.push(highlight_xml_line(l));
            }
        }
        ContentKind::Yaml => {
            for l in body.lines() {
                lines.push(highlight_yaml_line(l));
            }
        }
        ContentKind::Text => {
            for l in body.lines() {
                lines.push(Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(theme::TEXT()),
                )));
            }
        }
    }
}

fn render_json_tree(app: &App, lines: &mut Vec<Line<'static>>) {
    if app.json_tree_nodes.is_empty() {
        lines.push(Line::from(Span::styled(
            "(empty JSON)",
            Style::default().fg(theme::TEXT_FAINT()),
        )));
        return;
    }
    for (idx, node) in app.json_tree_nodes.iter().enumerate() {
        let indent = "  ".repeat(node.depth);
        let toggle = if node.is_expandable {
            if node.is_expanded {
                "\u{25bc} "
            } else {
                "\u{25b6} "
            }
        } else {
            "  "
        };
        let is_cursor = app.focus == Focus::Response
            && app.response_tab == ResponseTab::Body
            && idx == app.json_cursor;
        let ks = Style::default().fg(theme::BLUE());
        let vs = match node.node_type {
            JsonNodeType::String => Style::default().fg(theme::GREEN()),
            JsonNodeType::Number => Style::default().fg(theme::PEACH()),
            JsonNodeType::Bool => Style::default().fg(theme::MAUVE()),
            JsonNodeType::Null => Style::default().fg(theme::TEXT_DIM()),
            JsonNodeType::Object | JsonNodeType::Array => {
                Style::default().fg(theme::TEXT_DIM())
            }
        };
        let mut spans = vec![
            Span::styled(indent, Style::default()),
            Span::styled(toggle.to_string(), Style::default().fg(theme::TEXT_FAINT())),
        ];
        if node.depth == 0 && node.key == "root" {
            spans.push(Span::styled(node.value_preview.clone(), vs));
        } else {
            spans.push(Span::styled(format!("{}: ", node.key), ks));
            spans.push(Span::styled(node.value_preview.clone(), vs));
        }
        let mut line = Line::from(spans);
        if is_cursor {
            line = line.patch_style(
                Style::default()
                    .bg(theme::BG_SURFACE())
                    .add_modifier(Modifier::BOLD),
            );
        }
        lines.push(line);
    }
}

pub fn handle_response_keys(app: &mut App, key: KeyEvent) {
    let jt =
        app.response_tab == ResponseTab::Body && !app.json_tree_nodes.is_empty();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if jt {
                if app.json_cursor + 1 < app.json_tree_nodes.len() {
                    app.json_cursor += 1;
                }
            } else {
                app.response_scroll = app.response_scroll.saturating_add(1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if jt {
                app.json_cursor = app.json_cursor.saturating_sub(1);
            } else {
                app.response_scroll = app.response_scroll.saturating_sub(1);
            }
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if jt {
                app.json_cursor = (app.json_cursor + 10)
                    .min(app.json_tree_nodes.len().saturating_sub(1));
            } else {
                app.response_scroll = app.response_scroll.saturating_add(10);
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if jt {
                app.json_cursor = app.json_cursor.saturating_sub(10);
            } else {
                app.response_scroll = app.response_scroll.saturating_sub(10);
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') if jt => {
            toggle_current_node(app);
        }
        KeyCode::Char('E') if jt => {
            expand_all(app);
        }
        KeyCode::Char('C') if jt => {
            collapse_all(app);
        }
        KeyCode::Char('l') => {
            load_full_body(app);
        }
        KeyCode::Char('g') => {
            if jt {
                app.json_cursor = 0;
            } else {
                app.response_scroll = 0;
            }
        }
        KeyCode::Char('G') => {
            if jt {
                app.json_cursor = app.json_tree_nodes.len().saturating_sub(1);
            } else {
                app.response_scroll = u16::MAX;
            }
        }
        KeyCode::Char('b') => {
            app.response_tab = ResponseTab::Body;
            app.response_scroll = 0;
            try_rebuild_json_tree(app);
        }
        KeyCode::Char('h') => {
            app.response_tab = ResponseTab::Headers;
            app.response_scroll = 0;
        }
        KeyCode::Char('a') => {
            app.response_tab = ResponseTab::Assertions;
            app.response_scroll = 0;
        }
        KeyCode::Char('y') => {
            copy_body(app);
        }
        KeyCode::Char('D') => {
            // Open diff viewer for compare blocks
            if let (Some(fi), Some(bi)) = (app.active_file_idx, app.active_block_idx) {
                let diff_data = app.loaded_files.get(fi).and_then(|file| {
                    let blk = file.suite.blocks.get(bi)?;
                    if !blk.compare { return None; }
                    let diff_dir = blk.diff.as_ref()?;
                    let results = file.results.as_ref()?;
                    let br = find_block_result(&results.block_results, &blk.name)?;
                    let body_a = br.step_results.iter()
                        .find(|s| s.name == diff_dir.step_a)
                        .and_then(|s| s.response.as_ref())
                        .map(|r| r.body.clone())
                        .unwrap_or_default();
                    let body_b = br.step_results.iter()
                        .find(|s| s.name == diff_dir.step_b)
                        .and_then(|s| s.response.as_ref())
                        .map(|r| r.body.clone())
                        .unwrap_or_default();
                    let label_a = format!("{} ({})", diff_dir.step_a, blk.name);
                    let label_b = format!("{} ({})", diff_dir.step_b, blk.name);
                    Some((body_a, body_b, label_a, label_b))
                });
                if let Some((body_a, body_b, label_a, label_b)) = diff_data {
                    let format_body = |body: &str| -> String {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
                            serde_json::to_string_pretty(&val).unwrap_or_else(|_| body.to_string())
                        } else {
                            body.to_string()
                        }
                    };
                    let text_a = format_body(&body_a);
                    let text_b = format_body(&body_b);
                    app.open_diff_viewer(&text_a, &text_b, label_a, label_b);
                }
            }
        }
        _ => {}
    }
}

fn toggle_current_node(app: &mut App) {
    if let Some(node) = app.json_tree_nodes.get(app.json_cursor) {
        if !node.is_expandable {
            return;
        }
        let path = node.path.clone();
        let expanded = node.is_expanded;
        let depth = node.depth;
        if expanded {
            if depth < DEFAULT_EXPAND_DEPTH {
                app.json_expanded.insert(format!("!{}", path));
                app.json_expanded.remove(&path);
            } else {
                app.json_expanded.remove(&path);
            }
        } else {
            app.json_expanded.insert(path.clone());
            app.json_expanded.remove(&format!("!{}", path));
        }
        rebuild_tree_from_current_body(app);
    }
}

fn expand_all(app: &mut App) {
    if let Some(body) = get_current_body(app) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            let mut paths = HashSet::new();
            collect_all_paths(&value, "root", &mut paths);
            app.json_expanded.retain(|p| !p.starts_with('!'));
            for p in paths {
                app.json_expanded.insert(p);
            }
            rebuild_json_tree(app, &body);
        }
    }
    app.set_status("Expanded all nodes".to_string());
}

fn collapse_all(app: &mut App) {
    if let Some(body) = get_current_body(app) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            let mut paths = HashSet::new();
            collect_all_paths(&value, "root", &mut paths);
            app.json_expanded.clear();
            for p in paths {
                app.json_expanded.insert(format!("!{}", p));
            }
            rebuild_json_tree(app, &body);
        }
    }
    app.json_cursor = 0;
    app.set_status("Collapsed all nodes".to_string());
}

fn load_full_body(app: &mut App) {
    if app.body_fully_loaded {
        return;
    }
    if let Some(body) = get_current_body(app) {
        if body.len() > LARGE_BODY_THRESHOLD {
            let ss = format_size(body.len());
            let hdrs = get_current_headers(app);
            let kind = detect_content_kind(&hdrs, &body);
            app.response_body_full = Some(body.clone());
            app.body_fully_loaded = true;
            app.set_status(format!("Loaded full response body ({})", ss));
            if kind == ContentKind::Json {
                rebuild_json_tree(app, &body);
            }
        }
    }
}

fn copy_body(app: &mut App) {
    if let Some(body) = get_current_body(app) {
        match std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.as_mut().unwrap().write_all(body.as_bytes())?;
                child.wait()
            }) {
            Ok(_) => app.set_status(format!(
                "Copied response body ({}) to clipboard",
                format_size(body.len())
            )),
            Err(e) => app.set_status(format!("Copy failed: {}", e)),
        }
    } else {
        app.set_status("No response body to copy".to_string());
    }
}

fn get_active_block_name(app: &App) -> Option<String> {
    let fi = app.active_file_idx?;
    let bi = app.active_block_idx?;
    let file = app.loaded_files.get(fi)?;
    Some(file.suite.blocks.get(bi)?.name.clone())
}

fn get_current_body(app: &App) -> Option<String> {
    let fi = app.active_file_idx?;
    let file = app.loaded_files.get(fi)?;
    let results = file.results.as_ref()?;
    let block_name = get_active_block_name(app)?;
    let br = find_block_result(&results.block_results, &block_name)?;
    let resp = br.response.as_ref()?;
    Some(resp.body.clone())
}

fn get_current_headers(app: &App) -> Vec<(String, String)> {
    if let (Some(fi), Some(_bi)) = (app.active_file_idx, app.active_block_idx) {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(ref results) = file.results {
                if let Some(block_name) = get_active_block_name(app) {
                    if let Some(br) = find_block_result(&results.block_results, &block_name) {
                        if let Some(ref resp) = br.response {
                            return resp.headers.clone();
                        }
                    }
                }
            }
        }
    }
    Vec::new()
}

pub fn try_rebuild_json_tree(app: &mut App) {
    if let Some(body) = get_current_body(app) {
        let hdrs = get_current_headers(app);
        let ebody = if body.len() > LARGE_BODY_THRESHOLD && !app.body_fully_loaded {
            &body[..PREVIEW_SIZE.min(body.len())]
        } else {
            &body
        };
        let kind = detect_content_kind(&hdrs, ebody);
        if kind == ContentKind::Json {
            rebuild_json_tree(app, ebody);
        } else {
            app.json_tree_nodes.clear();
        }
    } else {
        app.json_tree_nodes.clear();
    }
}

fn rebuild_tree_from_current_body(app: &mut App) {
    if let Some(body) = get_current_body(app) {
        let eb = if body.len() > LARGE_BODY_THRESHOLD && !app.body_fully_loaded {
            body[..PREVIEW_SIZE.min(body.len())].to_string()
        } else if app.body_fully_loaded {
            app.response_body_full.clone().unwrap_or(body)
        } else {
            body
        };
        rebuild_json_tree(app, &eb);
        if app.json_cursor >= app.json_tree_nodes.len() {
            app.json_cursor = app.json_tree_nodes.len().saturating_sub(1);
        }
    }
}
