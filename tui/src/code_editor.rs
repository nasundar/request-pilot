//! Code editor mode — full-screen editing of .http files with syntax highlighting.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, Mode, Focus};
use crate::ui::theme;

const DIRECTIVES: &[&str] = &[
    "# @name", "# @description", "# @assert", "# @extract",
    "# @setup", "# @test", "# @teardown", "# @group",
    "# @depends", "# @mode", "# @dev_auth", "# @disabled",
    "# @type", "# @compare", "# @step", "# @diff",
];

const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

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
                Style::default().fg(theme::PEACH()),
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
            Style::default().fg(theme::YELLOW()).add_modifier(Modifier::BOLD),
        )];
    }

    for d in DIRECTIVES {
        if trimmed.starts_with(d) {
            return vec![Span::styled(line.to_string(), Style::default().fg(theme::MAUVE()))];
        }
    }

    if trimmed.starts_with('@') {
        return highlight_variables(line, Style::default().fg(theme::MAUVE()));
    }

    if trimmed.starts_with('#') {
        return vec![Span::styled(line.to_string(), Style::default().fg(theme::GREEN()))];
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
                    spans.extend(highlight_variables(url_part, Style::default().fg(theme::TEXT())));
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
            spans.push(Span::styled(key.to_string(), Style::default().fg(theme::BLUE())));
            spans.push(Span::styled(": ".to_string(), Style::default().fg(theme::TEXT_DIM())));
            spans.extend(highlight_variables(value, Style::default().fg(theme::TEXT())));
            return spans;
        }
    }

    if in_body && !trimmed.is_empty() {
        return highlight_json_line(line);
    }

    highlight_variables(line, Style::default().fg(theme::TEXT()))
}

fn highlight_json_line(line: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = line.char_indices().peekable();
    let mut buf = String::new();

    while let Some(&(i, ch)) = chars.peek() {
        match ch {
            '"' => {
                if !buf.is_empty() {
                    spans.push(Span::styled(buf.clone(), Style::default().fg(theme::TEXT())));
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
                    spans.push(Span::styled(s, Style::default().fg(theme::BLUE())));
                } else if s.contains("{{") {
                    spans.extend(highlight_variables(&s, Style::default().fg(theme::GREEN())));
                } else {
                    spans.push(Span::styled(s, Style::default().fg(theme::GREEN())));
                }
            }
            '0'..='9' | '-'
                if buf.trim().is_empty()
                    || buf.ends_with(|c: char| c == ':' || c == ' ' || c == ',' || c == '[') =>
            {
                if !buf.is_empty() {
                    spans.push(Span::styled(buf.clone(), Style::default().fg(theme::TEXT())));
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
                spans.push(Span::styled(num, Style::default().fg(theme::PEACH())));
                continue;
            }
            '{' | '}' | '[' | ']' | ',' | ':' => {
                if !buf.is_empty() {
                    spans.push(Span::styled(buf.clone(), Style::default().fg(theme::TEXT())));
                    buf.clear();
                }
                spans.push(Span::styled(ch.to_string(), Style::default().fg(theme::TEXT_DIM())));
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
                                Style::default().fg(theme::TEXT()),
                            ));
                        }
                        spans.push(Span::styled(kw.to_string(), Style::default().fg(theme::MAUVE())));
                        buf.clear();
                        break;
                    }
                }
            }
        }
    }
    if !buf.is_empty() {
        if buf.contains("{{") {
            spans.extend(highlight_variables(&buf, Style::default().fg(theme::TEXT())));
        } else {
            spans.push(Span::styled(buf, Style::default().fg(theme::TEXT())));
        }
    }
    if spans.is_empty() {
        spans.push(Span::styled(line.to_string(), Style::default().fg(theme::TEXT())));
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
    // Strip \r for consistent rendering on Windows (\r\n → \n)
    let clean: String = content.replace('\r', "");
    let raw_lines: Vec<&str> = clean.split('\n').collect();
    let total_lines = raw_lines.len();
    let gutter_width: u16 = (total_lines.to_string().len() as u16).max(3) + 1;
    let has_search_bar = app.editor_search_active
        || !app.editor_search_matches.is_empty()
        || app.editor_goto_active;
    let chrome_rows: u16 = if has_search_bar { 3 } else { 2 };
    let visible_height = area.height.saturating_sub(chrome_rows) as usize;
    let scroll = app.code_editor_scroll as usize;
    let area_width = area.width as usize;

    let active_range = find_active_block_range(&raw_lines, app.code_editor_cursor_line);

    // Build a set of search match positions for fast lookup
    let search_positions: std::collections::HashMap<(usize, usize), bool> = {
        let mut map = std::collections::HashMap::new();
        let query_len = app.editor_search_query.len();
        if query_len > 0 {
            for (idx, &(line, col)) in app.editor_search_matches.iter().enumerate() {
                let is_current = idx == app.editor_search_idx;
                map.insert((line, col), is_current);
            }
        }
        map
    };

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
            } else if trimmed.starts_with("# @step ") || trimmed == "# @compare" || trimmed.starts_with("# @diff ") {
                // Compare directives reset body state (act like sub-block separators)
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

    let query_char_len = app.editor_search_query.chars().count();

    let mut display_lines: Vec<Line<'static>> = Vec::with_capacity(visible_height);
    for i in scroll..total_lines.min(scroll + visible_height) {
        let raw = raw_lines[i];
        let is_cursor_line = i == app.code_editor_cursor_line;
        let is_active_block = active_range.map_or(false, |(s, e)| i >= s && i < e);
        let bg = if is_cursor_line {
            theme::BG_OVERLAY()
        } else if is_active_block {
            theme::BG_SURFACE()
        } else {
            theme::BG_DARK()
        };
        let num_style = if is_cursor_line {
            Style::default().fg(theme::YELLOW()).bg(bg)
        } else {
            Style::default().fg(theme::TEXT_FAINT()).bg(bg)
        };
        let gutter = format!("{:>w$} ", i + 1, w = gutter_width as usize - 1);
        let mut spans = vec![Span::styled(gutter, num_style)];

        // Collect matches on this line for highlight overlay
        let line_matches: Vec<(usize, bool)> = if !search_positions.is_empty() {
            app.editor_search_matches
                .iter()
                .enumerate()
                .filter(|(_, &(line, _))| line == i)
                .map(|(idx, &(_, col))| (col, idx == app.editor_search_idx))
                .collect()
        } else {
            Vec::new()
        };

        let has_selection = app.editor_selection_anchor.is_some();

        if line_matches.is_empty() && !has_selection {
            for s in highlight_line(raw, in_body_flags[i]) {
                spans.push(Span::styled(s.content.to_string(), s.style.bg(bg)));
            }
        } else {
            // Render with per-char styles for search highlights and/or selection
            let base_spans = highlight_line(raw, in_body_flags[i]);
            let chars: Vec<char> = raw.chars().collect();
            let mut char_styles: Vec<Style> = vec![Style::default().fg(theme::TEXT()).bg(bg); chars.len()];
            {
                let mut char_idx = 0;
                for s in &base_spans {
                    let span_chars: Vec<char> = s.content.chars().collect();
                    for _ in &span_chars {
                        if char_idx < char_styles.len() {
                            char_styles[char_idx] = s.style.bg(bg);
                        }
                        char_idx += 1;
                    }
                }
            }
            // Apply selection highlight
            if has_selection {
                for ci in 0..chars.len() {
                    if is_in_selection(app, i, ci) {
                        let base = char_styles[ci];
                        char_styles[ci] = base.bg(theme::BG_HIGHLIGHT());
                    }
                }
            }
            // Apply search highlight (on top of selection)
            for &(col, is_current) in &line_matches {
                let hl_bg = if is_current { theme::PEACH() } else { theme::YELLOW() };
                let hl_fg = theme::BG_DARK();
                for offset in 0..query_char_len {
                    let ci = col + offset;
                    if ci < char_styles.len() {
                        char_styles[ci] = Style::default().fg(hl_fg).bg(hl_bg);
                    }
                }
            }
            // Build spans from per-char styles
            if !chars.is_empty() {
                let mut run_start = 0;
                let mut current_style = char_styles[0];
                for ci in 1..chars.len() {
                    if char_styles[ci] != current_style {
                        let text: String = chars[run_start..ci].iter().collect();
                        spans.push(Span::styled(text, current_style));
                        run_start = ci;
                        current_style = char_styles[ci];
                    }
                }
                let text: String = chars[run_start..].iter().collect();
                spans.push(Span::styled(text, current_style));
            }
        }

        // Pad every line to full area width to clear ghost content from previous frame
        let w: usize = spans.iter().map(|s| s.width()).sum();
        if w < area_width {
            spans.push(Span::styled(" ".repeat(area_width - w), Style::default().bg(bg)));
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
            Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(title, Style::default().fg(theme::TEXT())),
        Span::styled(" Esc=exit Ctrl+S=save /=search Ctrl+G=goto ", Style::default().fg(theme::TEXT_FAINT())),
    ]);
    let hints_line = Line::from(vec![Span::styled(
        " \u{2191}\u{2193}=move  Ctrl+U/D=half-page  n/N=next/prev match  Type to edit ",
        Style::default().fg(theme::TEXT_FAINT()),
    )]);

    let title_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    frame.render_widget(
        Paragraph::new(title_line).style(Style::default().bg(theme::BG_BASE())),
        title_area,
    );
    let content_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(chrome_rows),
    };
    // Clear the content area to remove any ghost content from previous frames
    frame.render_widget(Clear, content_area);
    frame.render_widget(
        Paragraph::new(display_lines).style(Style::default().bg(theme::BG_DARK())),
        content_area,
    );

    // Show native blinking cursor at the editor cursor position
    let cursor_visible_line = app.code_editor_cursor_line as i32 - scroll as i32;
    if cursor_visible_line >= 0 && (cursor_visible_line as usize) < visible_height {
        let cursor_x = content_area.x + gutter_width + app.code_editor_cursor_col as u16;
        let cursor_y = content_area.y + cursor_visible_line as u16;
        if cursor_x < content_area.x + content_area.width {
            frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
        }
    }

    // Search/goto bar or hints bar at the bottom
    let bottom_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };

    if app.editor_search_active {
        let match_info = if app.editor_search_matches.is_empty() {
            if app.editor_search_query.is_empty() {
                String::new()
            } else {
                " (no matches)".to_string()
            }
        } else {
            format!(" ({} matches)", app.editor_search_matches.len())
        };
        let search_line = Line::from(vec![
            Span::styled(" /", Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD)),
            Span::styled(
                app.editor_search_query.clone(),
                Style::default().fg(theme::TEXT()),
            ),
            Span::styled("\u{2588}", Style::default().fg(theme::BLUE())),
            Span::styled(match_info, Style::default().fg(theme::TEXT_DIM())),
        ]);
        frame.render_widget(
            Paragraph::new(search_line).style(Style::default().bg(theme::BG_SURFACE())),
            bottom_area,
        );
    } else if app.editor_goto_active {
        let goto_line = Line::from(vec![
            Span::styled(
                " Go to line: ",
                Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.editor_goto_buffer.clone(),
                Style::default().fg(theme::TEXT()),
            ),
            Span::styled("\u{2588}", Style::default().fg(theme::BLUE())),
        ]);
        frame.render_widget(
            Paragraph::new(goto_line).style(Style::default().bg(theme::BG_SURFACE())),
            bottom_area,
        );
    } else if !app.editor_search_matches.is_empty() {
        let info_line = Line::from(vec![
            Span::styled(
                format!(
                    " /{} \u{2014} {}/{} matches  n=next N=prev Esc=clear ",
                    app.editor_search_query,
                    app.editor_search_idx + 1,
                    app.editor_search_matches.len()
                ),
                Style::default().fg(theme::TEXT_DIM()),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(info_line).style(Style::default().bg(theme::BG_BASE())),
            bottom_area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(hints_line).style(Style::default().bg(theme::BG_BASE())),
            bottom_area,
        );
    }
}

fn compute_search_matches(content: &str, query: &str) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    if query.is_empty() {
        return matches;
    }
    let query_lower = query.to_lowercase();
    for (line_idx, line) in content.split('\n').enumerate() {
        let line_lower = line.to_lowercase();
        let mut start = 0;
        while let Some(pos) = line_lower[start..].find(&query_lower) {
            matches.push((line_idx, start + pos));
            start += pos + 1;
        }
    }
    matches
}

pub fn handle_editor_keys(app: &mut App, key: KeyEvent) {
    let total_lines = app.code_editor_content.split('\n').count();

    // Goto-line mini-input mode
    if app.editor_goto_active {
        match key.code {
            KeyCode::Esc => {
                app.editor_goto_active = false;
                app.editor_goto_buffer.clear();
            }
            KeyCode::Enter => {
                if let Ok(line_num) = app.editor_goto_buffer.parse::<usize>() {
                    let target = line_num.saturating_sub(1).min(total_lines.saturating_sub(1));
                    app.code_editor_cursor_line = target;
                    app.code_editor_cursor_col = 0;
                    ensure_cursor_visible(app);
                }
                app.editor_goto_active = false;
                app.editor_goto_buffer.clear();
            }
            KeyCode::Backspace => {
                app.editor_goto_buffer.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                app.editor_goto_buffer.push(c);
            }
            _ => {}
        }
        return;
    }

    // Search input mode
    if app.editor_search_active {
        match key.code {
            KeyCode::Esc => {
                app.editor_search_active = false;
                app.editor_search_query.clear();
                app.editor_search_matches.clear();
                app.editor_search_idx = 0;
            }
            KeyCode::Enter => {
                app.editor_search_active = false;
                if !app.editor_search_matches.is_empty() {
                    // Find the first match at or after the cursor
                    let start_idx = app
                        .editor_search_matches
                        .iter()
                        .position(|&(line, _)| line >= app.code_editor_cursor_line)
                        .unwrap_or(0);
                    app.editor_search_idx = start_idx;
                    let (line, _col) = app.editor_search_matches[start_idx];
                    app.code_editor_cursor_line = line;
                    clamp_cursor_col(app);
                    ensure_cursor_visible(app);
                }
            }
            KeyCode::Backspace => {
                app.editor_search_query.pop();
                app.editor_search_matches =
                    compute_search_matches(&app.code_editor_content, &app.editor_search_query);
                app.editor_search_idx = 0;
            }
            KeyCode::Char(c) => {
                app.editor_search_query.push(c);
                app.editor_search_matches =
                    compute_search_matches(&app.code_editor_content, &app.editor_search_query);
                app.editor_search_idx = 0;
            }
            _ => {}
        }
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('s') => { app.save_code_editor(); }
            KeyCode::Char('u') => {
                app.code_editor_cursor_line = app.code_editor_cursor_line.saturating_sub(15);
                clamp_cursor_col(app);
                ensure_cursor_visible(app);
                app.editor_selection_anchor = None;
            }
            KeyCode::Char('d') => {
                app.code_editor_cursor_line =
                    (app.code_editor_cursor_line + 15).min(total_lines.saturating_sub(1));
                clamp_cursor_col(app);
                ensure_cursor_visible(app);
                app.editor_selection_anchor = None;
            }
            KeyCode::Char('g') => {
                app.editor_goto_active = true;
                app.editor_goto_buffer.clear();
            }
            KeyCode::Char('c') => {
                if let Some(text) = get_selected_text(app) {
                    if clipboard_set(&text) {
                        app.set_status(format!("Copied {} chars", text.len()));
                    } else {
                        app.set_status("Copy failed".into());
                    }
                }
            }
            KeyCode::Char('v') => {
                if let Some(text) = clipboard_get() {
                    // If there's a selection, delete it first
                    if app.editor_selection_anchor.is_some() {
                        delete_selection(app);
                    }
                    let off = line_col_to_offset(
                        &app.code_editor_content,
                        app.code_editor_cursor_line,
                        app.code_editor_cursor_col,
                    );
                    app.code_editor_content.insert_str(off, &text);
                    // Move cursor to end of pasted text
                    let newlines = text.matches('\n').count();
                    if newlines > 0 {
                        app.code_editor_cursor_line += newlines;
                        let last_line = text.rsplit('\n').next().unwrap_or("");
                        app.code_editor_cursor_col = last_line.chars().count();
                    } else {
                        app.code_editor_cursor_col += text.chars().count();
                    }
                    app.code_editor_modified = true;
                    app.editor_selection_anchor = None;
                    ensure_cursor_visible(app);
                    app.set_status(format!("Pasted {} chars", text.len()));
                }
            }
            KeyCode::Char('a') => {
                // Select all
                app.editor_selection_anchor = Some((0, 0));
                let last_line = total_lines.saturating_sub(1);
                app.code_editor_cursor_line = last_line;
                app.code_editor_cursor_col = get_line_len(&app.code_editor_content, last_line);
                ensure_cursor_visible(app);
            }
            KeyCode::Char('x') => {
                // Cut
                if let Some(text) = get_selected_text(app) {
                    if clipboard_set(&text) {
                        delete_selection(app);
                        app.code_editor_modified = true;
                        app.set_status(format!("Cut {} chars", text.len()));
                    }
                }
            }
            _ => {}
        }
        return;
    }

    // Shift+Arrow for selection
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
            | KeyCode::Home | KeyCode::End => {
                // Set anchor if starting a new selection
                if app.editor_selection_anchor.is_none() {
                    app.editor_selection_anchor = Some((
                        app.code_editor_cursor_line,
                        app.code_editor_cursor_col,
                    ));
                }
                // Move cursor (same logic as normal movement)
                match key.code {
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
                    _ => {}
                }
                return;
            }
            _ => {}
        }
    }

    // Clear selection on any non-shift, non-ctrl key
    if !key.modifiers.contains(KeyModifiers::SHIFT) {
        app.editor_selection_anchor = None;
    }

    match key.code {
        KeyCode::Esc => {
            if !app.editor_search_matches.is_empty() {
                app.editor_search_query.clear();
                app.editor_search_matches.clear();
                app.editor_search_idx = 0;
            } else {
                app.mode = Mode::Files;
                app.focus = Focus::FileTree;
            }
        }
        KeyCode::Char('/') => {
            app.editor_search_active = true;
            app.editor_search_query.clear();
            app.editor_search_matches.clear();
            app.editor_search_idx = 0;
        }
        KeyCode::Char('n')
            if !app.editor_search_matches.is_empty()
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.editor_search_idx =
                (app.editor_search_idx + 1) % app.editor_search_matches.len();
            let (line, _col) = app.editor_search_matches[app.editor_search_idx];
            app.code_editor_cursor_line = line;
            clamp_cursor_col(app);
            ensure_cursor_visible(app);
        }
        KeyCode::Char('N') if !app.editor_search_matches.is_empty() => {
            let len = app.editor_search_matches.len();
            app.editor_search_idx = (app.editor_search_idx + len - 1) % len;
            let (line, _col) = app.editor_search_matches[app.editor_search_idx];
            app.code_editor_cursor_line = line;
            clamp_cursor_col(app);
            ensure_cursor_visible(app);
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
                if let Some((prev_off, _)) = app.code_editor_content[..off].char_indices().last() {
                    app.code_editor_content.remove(prev_off);
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
    content.split('\n').nth(line_idx).map_or(0, |l| l.chars().count())
}

fn line_col_to_offset(content: &str, line: usize, col: usize) -> usize {
    let mut offset = 0;
    for (i, l) in content.split('\n').enumerate() {
        if i == line {
            let char_offset = l.char_indices().nth(col.min(l.chars().count())).map(|(idx, _)| idx).unwrap_or(l.len());
            return offset + char_offset;
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

/// Get the ordered selection range as (start_offset, end_offset) into the content string.
fn selection_offsets(app: &App) -> Option<(usize, usize)> {
    let anchor = app.editor_selection_anchor?;
    let cursor = (app.code_editor_cursor_line, app.code_editor_cursor_col);
    let (start, end) = if anchor <= cursor { (anchor, cursor) } else { (cursor, anchor) };
    let s = line_col_to_offset(&app.code_editor_content, start.0, start.1);
    let e = line_col_to_offset(&app.code_editor_content, end.0, end.1);
    if s == e { None } else { Some((s, e)) }
}

fn get_selected_text(app: &App) -> Option<String> {
    let (s, e) = selection_offsets(app)?;
    Some(app.code_editor_content[s..e].to_string())
}

fn delete_selection(app: &mut App) {
    if let Some((s, e)) = selection_offsets(app) {
        let anchor = app.editor_selection_anchor.unwrap();
        let cursor = (app.code_editor_cursor_line, app.code_editor_cursor_col);
        let start = if anchor <= cursor { anchor } else { cursor };
        app.code_editor_content.replace_range(s..e, "");
        app.code_editor_cursor_line = start.0;
        app.code_editor_cursor_col = start.1;
        app.editor_selection_anchor = None;
        clamp_cursor_col(app);
    }
}

/// Check if a position (line, col) is within the current selection range.
pub fn is_in_selection(app: &App, line: usize, col: usize) -> bool {
    let anchor = match app.editor_selection_anchor {
        Some(a) => a,
        None => return false,
    };
    let cursor = (app.code_editor_cursor_line, app.code_editor_cursor_col);
    let (start, end) = if anchor <= cursor { (anchor, cursor) } else { (cursor, anchor) };
    if line < start.0 || line > end.0 { return false; }
    if line == start.0 && line == end.0 { return col >= start.1 && col < end.1; }
    if line == start.0 { return col >= start.1; }
    if line == end.0 { return col < end.1; }
    true
}

fn clipboard_set(text: &str) -> bool {
    use std::io::Write;
    std::process::Command::new("clip")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(text.as_bytes())?;
            child.wait()
        })
        .is_ok()
}

fn clipboard_get() -> Option<String> {
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "Get-Clipboard"])
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        let trimmed = text.trim_end_matches("\r\n").trim_end_matches('\n');
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    } else {
        None
    }
}

