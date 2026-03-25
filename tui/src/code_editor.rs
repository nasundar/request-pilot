//! Code editor mode — full-screen editing of .http files with syntax highlighting.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, Mode, Focus};
use crate::ui::theme;

const DIRECTIVES: &[&str] = &[
    "# @name", "# @description", "# @assert", "# @extract",
    "# @setup", "# @test", "# @teardown", "# @group",
    "# @depends", "# @mode", "# @dev_auth", "# @disabled",
    "# @type",
];

const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

fn method_color(method: &str) -> ratatui::style::Color {
    match method {
        "GET" => theme::GREEN,
        "POST" => theme::BLUE,
        "PUT" => theme::YELLOW,
        "PATCH" => theme::PINK,
        "DELETE" => theme::RED,
        "HEAD" => theme::SKY,
        "OPTIONS" => theme::LAVENDER,
        _ => theme::TEXT,
    }
}

fn highlight_variables(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        if let Some(end) = rest[start..].find("}}") {
            if start > 0 {
                spans.push(Span::styled(rest[..start].to_string(), base_style));
            }
            spans.push(Span::styled(
                rest[start..start + end + 2].to_string(),
                Style::default().fg(theme::PEACH),
            ));
            rest = &rest[start + end + 2..];
        } else {
            break;
        }
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), base_style));
    }
    spans
}

fn highlight_line(line: &str, in_body: bool) -> Vec<Span<'static>> {
    let trimmed = line.trim();

    if trimmed.starts_with("###") {
        return vec![Span::styled(
            line.to_string(),
            Style::default().fg(theme::YELLOW).add_modifier(Modifier::BOLD),
        )];
    }

    for d in DIRECTIVES {
        if trimmed.starts_with(d) {
            return vec![Span::styled(line.to_string(), Style::default().fg(theme::MAUVE))];
        }
    }

    if trimmed.starts_with('@') {
        return highlight_variables(line, Style::default().fg(theme::MAUVE));
    }

    if trimmed.starts_with('#') {
        return vec![Span::styled(line.to_string(), Style::default().fg(theme::GREEN))];
    }

    for m in HTTP_METHODS {
        if trimmed.starts_with(m) {
            let after = &trimmed[m.len()..];
            if after.is_empty() || after.starts_with(' ') {
                let leading_len = line.len() - line.trim_start().len();
                let mut spans = vec![];
                if leading_len > 0 {
                    spans.push(Span::styled(line[..leading_len].to_string(), Style::default()));
                }
                spans.push(Span::styled(
                    m.to_string(),
                    Style::default().fg(method_color(m)).add_modifier(Modifier::BOLD),
                ));
                let url_part = &line[leading_len + m.len()..];
                if !url_part.is_empty() {
                    spans.extend(highlight_variables(url_part, Style::default().fg(theme::TEXT)));
                }
                return spans;
            }
        }
    }

    if !in_body {
        if let Some(colon_pos) = trimmed.find(": ") {
            let leading_len = line.len() - line.trim_start().len();
            let key = &trimmed[..colon_pos];
            let value = &trimmed[colon_pos + 2..];
            let mut spans = vec![];
            if leading_len > 0 {
                spans.push(Span::styled(line[..leading_len].to_string(), Style::default()));
            }
            spans.push(Span::styled(key.to_string(), Style::default().fg(theme::BLUE)));
            spans.push(Span::styled(": ".to_string(), Style::default().fg(theme::TEXT_DIM)));
            spans.extend(highlight_variables(value, Style::default().fg(theme::TEXT)));
            return spans;
        }
    }

    if in_body && !trimmed.is_empty() {
        return highlight_json_line(line);
    }

    highlight_variables(line, Style::default().fg(theme::TEXT))
}

fn highlight_json_line(line: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = line.char_indices().peekable();
    let mut buf = String::new();

    while let Some(&(i, ch)) = chars.peek() {
        match ch {
            '"' => {
                if !buf.is_empty() {
                    spans.push(Span::styled(buf.clone(), Style::default().fg(theme::TEXT)));
                    buf.clear();
                }
                let mut s = String::new();
                s.push(ch);
                chars.next();
                let mut escaped = false;
                loop {
                    match chars.peek() {
                        Some(&(_, '\\')) if !escaped => {
                            escaped = true;
                            s.push('\\');
                            chars.next();
                        }
                        Some(&(_, '"')) if !escaped => {
                            s.push('"');
                            chars.next();
                            break;
                        }
                        Some(&(_, c)) => {
                            escaped = false;
                            s.push(c);
                            chars.next();
                        }
                        None => break,
                    }
                }
                let rest = &line[i + s.len()..];
                if rest.trim_start().starts_with(':') {
                    spans.push(Span::styled(s, Style::default().fg(theme::BLUE)));
                } else if s.contains("{{") {
                    spans.extend(highlight_variables(&s, Style::default().fg(theme::GREEN)));
                } else {
                    spans.push(Span::styled(s, Style::default().fg(theme::GREEN)));
                }
            }
            '0'..='9' | '-'
                if buf.trim().is_empty()
                    || buf.ends_with(|c: char| c == ':' || c == ' ' || c == ',' || c == '[') =>
            {
                if !buf.is_empty() {
                    spans.push(Span::styled(buf.clone(), Style::default().fg(theme::TEXT)));
                    buf.clear();
                }
                let mut num = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' || c == '+' {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                spans.push(Span::styled(num, Style::default().fg(theme::PEACH)));
                continue;
            }
            '{' | '}' | '[' | ']' | ',' | ':' => {
                if !buf.is_empty() {
                    spans.push(Span::styled(buf.clone(), Style::default().fg(theme::TEXT)));
                    buf.clear();
                }
                spans.push(Span::styled(ch.to_string(), Style::default().fg(theme::TEXT_DIM)));
                chars.next();
            }
            _ => {
                buf.push(ch);
                chars.next();
                for kw in &["true", "false", "null"] {
                    if buf.ends_with(kw) {
                        let prefix_len = buf.len() - kw.len();
                        if prefix_len > 0 {
                            spans.push(Span::styled(
                                buf[..prefix_len].to_string(),
                                Style::default().fg(theme::TEXT),
                            ));
                        }
                        spans.push(Span::styled(kw.to_string(), Style::default().fg(theme::MAUVE)));
                        buf.clear();
                        break;
                    }
                }
            }
        }
    }
    if !buf.is_empty() {
        if buf.contains("{{") {
            spans.extend(highlight_variables(&buf, Style::default().fg(theme::TEXT)));
        } else {
            spans.push(Span::styled(buf, Style::default().fg(theme::TEXT)));
        }
    }
    if spans.is_empty() {
        spans.push(Span::styled(line.to_string(), Style::default().fg(theme::TEXT)));
    }
    spans
}

fn find_active_block_range(lines: &[&str], cursor_line: usize) -> Option<(usize, usize)> {
    if lines.is_empty() || cursor_line >= lines.len() {
        return None;
    }
    let separators: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim().starts_with("###"))
        .map(|(i, _)| i)
        .collect();
    if separators.is_empty() {
        return Some((0, lines.len()));
    }
    let mut block_start = 0;
    let mut block_end = lines.len();
    for (i, &sep) in separators.iter().enumerate() {
        if cursor_line < sep {
            block_end = sep;
            break;
        }
        block_start = sep;
        block_end = separators.get(i + 1).copied().unwrap_or(lines.len());
    }
    Some((block_start, block_end))
}

pub fn render_editor(frame: &mut Frame, app: &App, area: Rect) {
    let content = &app.code_editor_content;
    let raw_lines: Vec<&str> = content.split('\n').collect();
    let total_lines = raw_lines.len();
    let gutter_width: u16 = (total_lines.to_string().len() as u16).max(3) + 1;
    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll = app.code_editor_scroll as usize;
    let area_width = area.width as usize;

    let active_range = find_active_block_range(&raw_lines, app.code_editor_cursor_line);

    let mut in_body_flags = vec![false; total_lines];
    {
        let mut had_method = false;
        let mut had_empty_after_headers = false;
        for (i, line) in raw_lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("###") {
                had_method = false;
                had_empty_after_headers = false;
                in_body_flags[i] = false;
            } else if HTTP_METHODS.iter().any(|m| {
                trimmed.starts_with(m)
                    && (trimmed.len() == m.len() || trimmed[m.len()..].starts_with(' '))
            }) {
                had_method = true;
                had_empty_after_headers = false;
                in_body_flags[i] = false;
            } else if trimmed.is_empty() && had_method && !had_empty_after_headers {
                had_empty_after_headers = true;
                in_body_flags[i] = false;
            } else if had_empty_after_headers {
                in_body_flags[i] = true;
            } else {
                in_body_flags[i] = false;
            }
        }
    }

    let mut display_lines: Vec<Line<'static>> = Vec::with_capacity(visible_height);
    for i in scroll..total_lines.min(scroll + visible_height) {
        let raw = raw_lines[i];
        let is_cursor_line = i == app.code_editor_cursor_line;
        let is_active_block = active_range.map_or(false, |(s, e)| i >= s && i < e);
        let bg = if is_cursor_line {
            theme::BG_OVERLAY
        } else if is_active_block {
            theme::BG_SURFACE
        } else {
            theme::BG_DARK
        };
        let num_style = if is_cursor_line {
            Style::default().fg(theme::YELLOW).bg(bg)
        } else {
            Style::default().fg(theme::TEXT_FAINT).bg(bg)
        };
        let gutter = format!("{:>w$} ", i + 1, w = gutter_width as usize - 1);
        let mut spans = vec![Span::styled(gutter, num_style)];
        for s in highlight_line(raw, in_body_flags[i]) {
            spans.push(Span::styled(s.content.to_string(), s.style.bg(bg)));
        }
        if is_cursor_line {
            let w: usize = spans.iter().map(|s| s.width()).sum();
            if w < area_width {
                spans.push(Span::styled(" ".repeat(area_width - w), Style::default().bg(bg)));
            }
        }
        display_lines.push(Line::from(spans));
    }

    let file_name = app
        .active_file_idx
        .and_then(|fi| app.loaded_files.get(fi))
        .map(|f| f.name.as_str())
        .unwrap_or("untitled");
    let modified = if app.code_editor_modified { " [modified]" } else { "" };
    let title = format!(
        " {}{} \u{2014} Ln {}, Col {} ",
        file_name, modified,
        app.code_editor_cursor_line + 1,
        app.code_editor_cursor_col + 1,
    );
    let title_line = Line::from(vec![
        Span::styled(
            " \u{270e} Code Editor",
            Style::default().fg(theme::BLUE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(title, Style::default().fg(theme::TEXT)),
        Span::styled(" Esc=exit Ctrl+S=save ", Style::default().fg(theme::TEXT_FAINT)),
    ]);
    let hints_line = Line::from(vec![Span::styled(
        " \u{2191}\u{2193}=move  Ctrl+U/D=half-page  Type to edit ",
        Style::default().fg(theme::TEXT_FAINT),
    )]);

    let title_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    frame.render_widget(
        Paragraph::new(title_line).style(Style::default().bg(theme::BG_BASE)),
        title_area,
    );
    let content_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(2),
    };
    frame.render_widget(
        Paragraph::new(display_lines).style(Style::default().bg(theme::BG_DARK)),
        content_area,
    );
    let bottom_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(hints_line).style(Style::default().bg(theme::BG_BASE)),
        bottom_area,
    );
}

pub fn handle_editor_keys(app: &mut App, key: KeyEvent) {
    let total_lines = app.code_editor_content.split('\n').count();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('s') => { app.save_code_editor(); }
            KeyCode::Char('u') => {
                app.code_editor_cursor_line = app.code_editor_cursor_line.saturating_sub(15);
                clamp_cursor_col(app);
                ensure_cursor_visible(app);
            }
            KeyCode::Char('d') => {
                app.code_editor_cursor_line =
                    (app.code_editor_cursor_line + 15).min(total_lines.saturating_sub(1));
                clamp_cursor_col(app);
                ensure_cursor_visible(app);
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Files;
            app.focus = Focus::FileTree;
        }
        KeyCode::Up => {
            if app.code_editor_cursor_line > 0 {
                app.code_editor_cursor_line -= 1;
                clamp_cursor_col(app);
                ensure_cursor_visible(app);
            }
        }
        KeyCode::Down => {
            if app.code_editor_cursor_line + 1 < total_lines {
                app.code_editor_cursor_line += 1;
                clamp_cursor_col(app);
                ensure_cursor_visible(app);
            }
        }
        KeyCode::Left => {
            if app.code_editor_cursor_col > 0 {
                app.code_editor_cursor_col -= 1;
            } else if app.code_editor_cursor_line > 0 {
                app.code_editor_cursor_line -= 1;
                app.code_editor_cursor_col =
                    get_line_len(&app.code_editor_content, app.code_editor_cursor_line);
                ensure_cursor_visible(app);
            }
        }
        KeyCode::Right => {
            let ll = get_line_len(&app.code_editor_content, app.code_editor_cursor_line);
            if app.code_editor_cursor_col < ll {
                app.code_editor_cursor_col += 1;
            } else if app.code_editor_cursor_line + 1 < total_lines {
                app.code_editor_cursor_line += 1;
                app.code_editor_cursor_col = 0;
                ensure_cursor_visible(app);
            }
        }
        KeyCode::Home => { app.code_editor_cursor_col = 0; }
        KeyCode::End => {
            app.code_editor_cursor_col =
                get_line_len(&app.code_editor_content, app.code_editor_cursor_line);
        }
        KeyCode::Enter => {
            let off = line_col_to_offset(
                &app.code_editor_content, app.code_editor_cursor_line, app.code_editor_cursor_col,
            );
            app.code_editor_content.insert(off, '\n');
            app.code_editor_cursor_line += 1;
            app.code_editor_cursor_col = 0;
            app.code_editor_modified = true;
            ensure_cursor_visible(app);
        }
        KeyCode::Backspace => {
            if app.code_editor_cursor_col > 0 {
                let off = line_col_to_offset(
                    &app.code_editor_content, app.code_editor_cursor_line, app.code_editor_cursor_col,
                );
                if off > 0 {
                    app.code_editor_content.remove(off - 1);
                    app.code_editor_cursor_col -= 1;
                    app.code_editor_modified = true;
                }
            } else if app.code_editor_cursor_line > 0 {
                let prev_len =
                    get_line_len(&app.code_editor_content, app.code_editor_cursor_line - 1);
                let off = line_col_to_offset(
                    &app.code_editor_content, app.code_editor_cursor_line, 0,
                );
                if off > 0 {
                    app.code_editor_content.remove(off - 1);
                    app.code_editor_cursor_line -= 1;
                    app.code_editor_cursor_col = prev_len;
                    app.code_editor_modified = true;
                    ensure_cursor_visible(app);
                }
            }
        }
        KeyCode::Delete => {
            let off = line_col_to_offset(
                &app.code_editor_content, app.code_editor_cursor_line, app.code_editor_cursor_col,
            );
            if off < app.code_editor_content.len() {
                app.code_editor_content.remove(off);
                app.code_editor_modified = true;
            }
        }
        KeyCode::Tab => {
            let off = line_col_to_offset(
                &app.code_editor_content, app.code_editor_cursor_line, app.code_editor_cursor_col,
            );
            app.code_editor_content.insert_str(off, "  ");
            app.code_editor_cursor_col += 2;
            app.code_editor_modified = true;
        }
        KeyCode::Char(c) => {
            let off = line_col_to_offset(
                &app.code_editor_content, app.code_editor_cursor_line, app.code_editor_cursor_col,
            );
            app.code_editor_content.insert(off, c);
            app.code_editor_cursor_col += 1;
            app.code_editor_modified = true;
        }
        _ => {}
    }
}

fn get_line_len(content: &str, line_idx: usize) -> usize {
    content.split('\n').nth(line_idx).map_or(0, |l| l.len())
}

fn line_col_to_offset(content: &str, line: usize, col: usize) -> usize {
    let mut offset = 0;
    for (i, l) in content.split('\n').enumerate() {
        if i == line {
            return offset + col.min(l.len());
        }
        offset += l.len() + 1;
    }
    content.len()
}

fn clamp_cursor_col(app: &mut App) {
    let ll = get_line_len(&app.code_editor_content, app.code_editor_cursor_line);
    if app.code_editor_cursor_col > ll {
        app.code_editor_cursor_col = ll;
    }
}

fn ensure_cursor_visible(app: &mut App) {
    let cursor = app.code_editor_cursor_line as u16;
    let scroll = app.code_editor_scroll;
    let visible = 30u16;
    if cursor < scroll {
        app.code_editor_scroll = cursor;
    } else if cursor >= scroll + visible {
        app.code_editor_scroll = cursor.saturating_sub(visible - 1);
    }
}

