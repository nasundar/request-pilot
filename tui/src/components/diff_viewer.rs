//! Diff viewer overlay — side-by-side comparison of responses.
//!
//! Supports LCS-based line diff, hunk collapsing, char-level highlighting,
//! side-by-side and changes-only view modes, scrolling, and keyboard navigation.
#![allow(dead_code)]

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::App;
use crate::ui::theme;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewMode {
    SideBySide,
    ChangesOnly,
}

#[derive(Debug, Clone)]
pub enum DiffLine {
    Same(String),
    Added(String),
    Removed(String),
    Modified(String, String),
}

#[derive(Debug, Clone)]
pub enum DisplayLine {
    Same(String),
    Added(String),
    Removed(String),
    Modified(String, String),
    HunkSep(usize),
}

#[derive(Debug, Clone)]
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
    pub unchanged: usize,
}

#[derive(Debug, Clone)]
pub struct DiffViewerData {
    pub label_a: String,
    pub label_b: String,
    pub diff_lines: Vec<DiffLine>,
    pub display_lines: Vec<DisplayLine>,
    pub scroll: usize,
    pub view_mode: DiffViewMode,
    pub stats: DiffStats,
}

// ---------------------------------------------------------------------------
// LCS-based line diff
// ---------------------------------------------------------------------------

pub(crate) fn compute_line_diff(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let n = a.len();
    let m = b.len();

    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    let mut result = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            result.push(DiffLine::Same(a[i - 1].to_string()));
            i -= 1;
            j -= 1;
        } else if i > 0 && j > 0
            && dp[i - 1][j - 1] >= dp[i - 1][j]
            && dp[i - 1][j - 1] >= dp[i][j - 1]
        {
            result.push(DiffLine::Modified(a[i - 1].to_string(), b[j - 1].to_string()));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            result.push(DiffLine::Added(b[j - 1].to_string()));
            j -= 1;
        } else {
            result.push(DiffLine::Removed(a[i - 1].to_string()));
            i -= 1;
        }
    }
    result.reverse();
    result
}

// ---------------------------------------------------------------------------
// Hunk collapsing
// ---------------------------------------------------------------------------

const CONTEXT_LINES: usize = 3;

fn build_display_lines(diff: &[DiffLine], changes_only: bool) -> Vec<DisplayLine> {
    if changes_only {
        return diff
            .iter()
            .filter_map(|dl| match dl {
                DiffLine::Same(_) => None,
                DiffLine::Added(s) => Some(DisplayLine::Added(s.clone())),
                DiffLine::Removed(s) => Some(DisplayLine::Removed(s.clone())),
                DiffLine::Modified(a, b) => Some(DisplayLine::Modified(a.clone(), b.clone())),
            })
            .collect();
    }

    let len = diff.len();
    let mut show = vec![false; len];
    for (i, dl) in diff.iter().enumerate() {
        if !matches!(dl, DiffLine::Same(_)) {
            let start = i.saturating_sub(CONTEXT_LINES);
            let end = (i + CONTEXT_LINES + 1).min(len);
            for slot in &mut show[start..end] {
                *slot = true;
            }
        }
    }

    let mut out = Vec::new();
    let mut hidden = 0usize;
    for (i, dl) in diff.iter().enumerate() {
        if show[i] {
            if hidden > 0 {
                out.push(DisplayLine::HunkSep(hidden));
                hidden = 0;
            }
            match dl {
                DiffLine::Same(s) => out.push(DisplayLine::Same(s.clone())),
                DiffLine::Added(s) => out.push(DisplayLine::Added(s.clone())),
                DiffLine::Removed(s) => out.push(DisplayLine::Removed(s.clone())),
                DiffLine::Modified(a, b) => out.push(DisplayLine::Modified(a.clone(), b.clone())),
            }
        } else {
            hidden += 1;
        }
    }
    if hidden > 0 {
        out.push(DisplayLine::HunkSep(hidden));
    }
    out
}

// ---------------------------------------------------------------------------
// Char-level diff spans
// ---------------------------------------------------------------------------

fn char_diff_spans<'a>(
    old: &'a str,
    new: &'a str,
    base_old: Style,
    base_new: Style,
    hl_style: Style,
) -> (Vec<Span<'a>>, Vec<Span<'a>>) {
    let oc: Vec<char> = old.chars().collect();
    let nc: Vec<char> = new.chars().collect();

    let mut prefix = 0;
    while prefix < oc.len() && prefix < nc.len() && oc[prefix] == nc[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < oc.len().saturating_sub(prefix)
        && suffix < nc.len().saturating_sub(prefix)
        && oc[oc.len() - 1 - suffix] == nc[nc.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let old_spans = build_hl_spans(old, &oc, prefix, oc.len().saturating_sub(suffix), base_old, hl_style);
    let new_spans = build_hl_spans(new, &nc, prefix, nc.len().saturating_sub(suffix), base_new, hl_style);
    (old_spans, new_spans)
}

fn build_hl_spans<'a>(
    text: &'a str,
    chars: &[char],
    hl_start: usize,
    hl_end: usize,
    base: Style,
    hl: Style,
) -> Vec<Span<'a>> {
    if hl_start >= hl_end || chars.is_empty() {
        return vec![Span::styled(text, base)];
    }
    let mut spans = Vec::new();
    let byte_start: usize = chars[..hl_start].iter().map(|c| c.len_utf8()).sum();
    let byte_end: usize = chars[..hl_end].iter().map(|c| c.len_utf8()).sum();
    if byte_start > 0 {
        spans.push(Span::styled(&text[..byte_start], base));
    }
    spans.push(Span::styled(&text[byte_start..byte_end], hl));
    if byte_end < text.len() {
        spans.push(Span::styled(&text[byte_end..], base));
    }
    spans
}

// ---------------------------------------------------------------------------
// DiffViewerData construction
// ---------------------------------------------------------------------------

impl DiffViewerData {
    pub fn new(text_a: &str, text_b: &str, label_a: String, label_b: String) -> Self {
        let lines_a: Vec<&str> = text_a.lines().collect();
        let lines_b: Vec<&str> = text_b.lines().collect();
        let diff_lines = compute_line_diff(&lines_a, &lines_b);

        let mut stats = DiffStats { added: 0, removed: 0, modified: 0, unchanged: 0 };
        for dl in &diff_lines {
            match dl {
                DiffLine::Same(_) => stats.unchanged += 1,
                DiffLine::Added(_) => stats.added += 1,
                DiffLine::Removed(_) => stats.removed += 1,
                DiffLine::Modified(_, _) => stats.modified += 1,
            }
        }

        let display_lines = build_display_lines(&diff_lines, false);

        Self {
            label_a,
            label_b,
            diff_lines,
            display_lines,
            scroll: 0,
            view_mode: DiffViewMode::SideBySide,
            stats,
        }
    }

    fn rebuild_display(&mut self) {
        let changes_only = self.view_mode == DiffViewMode::ChangesOnly;
        self.display_lines = build_display_lines(&self.diff_lines, changes_only);
        if self.scroll >= self.display_lines.len() {
            self.scroll = self.display_lines.len().saturating_sub(1);
        }
    }

    fn toggle_mode(&mut self) {
        self.view_mode = match self.view_mode {
            DiffViewMode::SideBySide => DiffViewMode::ChangesOnly,
            DiffViewMode::ChangesOnly => DiffViewMode::SideBySide,
        };
        self.rebuild_display();
    }
}

// ---------------------------------------------------------------------------
// App integration helpers
// ---------------------------------------------------------------------------

impl App {
    pub fn open_diff_viewer(
        &mut self,
        text_a: &str,
        text_b: &str,
        label_a: String,
        label_b: String,
    ) {
        self.diff_viewer_data = Some(DiffViewerData::new(text_a, text_b, label_a, label_b));
        self.diff_viewer_open = true;
    }

    pub fn close_diff_viewer(&mut self) {
        self.diff_viewer_open = false;
        self.diff_viewer_data = None;
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn render_diff_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let data = match &app.diff_viewer_data {
        Some(d) => d,
        None => return,
    };

    let margin = 2u16;
    if area.width < margin * 2 + 20 || area.height < margin * 2 + 6 {
        return;
    }
    let inner = Rect {
        x: area.x + margin,
        y: area.y + margin,
        width: area.width - margin * 2,
        height: area.height - margin * 2,
    };

    frame.render_widget(Clear, inner);

    let mode_label = match data.view_mode {
        DiffViewMode::SideBySide => "Side-by-Side",
        DiffViewMode::ChangesOnly => "Changes Only",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " Diff: {} vs {} | +{} -{} ~{} | {} | Esc close, m toggle ",
            data.label_a,
            data.label_b,
            data.stats.added,
            data.stats.removed,
            data.stats.modified,
            mode_label,
        ))
        .border_style(Style::default().fg(theme::LAVENDER()))
        .style(Style::default().bg(theme::BG_DARK()));

    let content_area = block.inner(inner);
    frame.render_widget(block, inner);

    if content_area.height < 2 || content_area.width < 10 {
        return;
    }

    // Header row
    let header_area = Rect { height: 1, ..content_area };
    let body_area = Rect {
        y: content_area.y + 1,
        height: content_area.height.saturating_sub(1),
        ..content_area
    };

    let half_w = content_area.width / 2;
    let header = Line::from(vec![
        Span::styled(
            format!(" {:<w$}", data.label_a, w = half_w as usize - 1),
            Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "\u{2502}",
            Style::default().fg(theme::BG_OVERLAY()),
        ),
        Span::styled(
            format!(" {}", data.label_b),
            Style::default().fg(theme::GREEN()).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header).style(Style::default().bg(theme::BG_SURFACE())),
        header_area,
    );

    // Body
    let visible = body_area.height as usize;
    let total = data.display_lines.len();
    let start = data.scroll.min(total.saturating_sub(visible));

    let style_same = Style::default().fg(theme::TEXT_DIM()).bg(theme::BG_DARK());
    let style_add = Style::default().fg(theme::GREEN()).bg(theme::BG_DARK());
    let style_rem = Style::default().fg(theme::RED()).bg(theme::BG_DARK());
    let style_mod_old = Style::default().fg(theme::PEACH()).bg(theme::BG_DARK());
    let style_mod_new = Style::default().fg(theme::YELLOW()).bg(theme::BG_DARK());
    let style_hl = Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let style_sep = Style::default().fg(theme::TEXT_FAINT()).bg(theme::BG_BASE());
    let style_divider = Style::default().fg(theme::BG_OVERLAY());

    let mut lines: Vec<Line> = Vec::with_capacity(visible);
    for idx in start..total.min(start + visible) {
        let dl = &data.display_lines[idx];
        let line = match dl {
            DisplayLine::Same(s) => {
                let left = truncate_pad(s, half_w as usize - 1);
                let right = truncate_pad(s, half_w as usize - 1);
                Line::from(vec![
                    Span::styled(format!(" {}", left), style_same),
                    Span::styled("\u{2502}", style_divider),
                    Span::styled(format!(" {}", right), style_same),
                ])
            }
            DisplayLine::Added(s) => {
                let left = " ".repeat(half_w as usize);
                let right = truncate_pad(s, half_w as usize - 1);
                Line::from(vec![
                    Span::styled(left, style_same),
                    Span::styled("\u{2502}", style_divider),
                    Span::styled(format!("+{}", right), style_add),
                ])
            }
            DisplayLine::Removed(s) => {
                let left = truncate_pad(s, half_w as usize - 1);
                let right = " ".repeat(half_w as usize);
                Line::from(vec![
                    Span::styled(format!("-{}", left), style_rem),
                    Span::styled("\u{2502}", style_divider),
                    Span::styled(right, style_same),
                ])
            }
            DisplayLine::Modified(old, new) => {
                let (old_spans, new_spans) =
                    char_diff_spans(old, new, style_mod_old, style_mod_new, style_hl);
                let mut spans = Vec::new();
                spans.push(Span::styled("~", style_mod_old));
                for sp in old_spans {
                    spans.push(sp);
                }
                spans.push(Span::styled("\u{2502}", style_divider));
                spans.push(Span::styled("~", style_mod_new));
                for sp in new_spans {
                    spans.push(sp);
                }
                Line::from(spans)
            }
            DisplayLine::HunkSep(n) => {
                let msg = format!("  ~~~ {} unchanged lines ~~~", n);
                Line::from(vec![Span::styled(msg, style_sep)])
            }
        };
        lines.push(line);
    }

    let para = Paragraph::new(lines).style(Style::default().bg(theme::BG_DARK()));
    frame.render_widget(para, body_area);
}

fn truncate_pad(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > width {
        chars[..width].iter().collect()
    } else {
        let mut out: String = chars.into_iter().collect();
        while out.len() < width {
            out.push(' ');
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

pub fn handle_diff_keys(app: &mut App, key: KeyEvent) {
    let data = match app.diff_viewer_data.as_mut() {
        Some(d) => d,
        None => {
            app.diff_viewer_open = false;
            return;
        }
    };

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.diff_viewer_open = false;
            app.diff_viewer_data = None;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            data.scroll = data.scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            data.scroll = data.scroll.saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            data.scroll = data.scroll.saturating_add(20);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            data.scroll = data.scroll.saturating_sub(20);
        }
        KeyCode::Char('g') => {
            data.scroll = 0;
        }
        KeyCode::Char('G') => {
            data.scroll = data.display_lines.len().saturating_sub(1);
        }
        KeyCode::Char('m') => {
            data.toggle_mode();
        }
        _ => {}
    }
}
