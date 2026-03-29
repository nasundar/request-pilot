use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::App;
use crate::ui::theme;
use request_pilot_core::jwt::{self, ValueType};

/// Build detail lines for the currently selected variable.
fn build_var_detail_lines(app: &App) -> (String, Vec<Line<'static>>) {
    let names = app.get_sorted_var_names();
    let name = match names.get(app.vars_cursor) {
        Some(n) => n.clone(),
        None => return ("Variable".to_string(), vec![Line::from("No variable selected")]),
    };
    let value = app.env_vars.get(&name).cloned().unwrap_or_default();
    let title = name.clone();

    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let label = Style::default().fg(theme::TEXT_DIM());
    let val_style = Style::default().fg(theme::TEXT());
    let dim = Style::default().fg(theme::TEXT_FAINT());

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Check if variable is from extract
    let mut is_extract = false;
    for file in &app.loaded_files {
        for blk in &file.suite.blocks {
            for e in &blk.extracts {
                if e.variable_name == name {
                    is_extract = true;
                }
            }
        }
    }

    // Header: name + source badge
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(name.clone(), Style::default().fg(theme::SAPPHIRE()).add_modifier(Modifier::BOLD)),
        Span::styled(
            if is_extract { "  ↤ extracted" } else { "  ◆ defined" },
            Style::default().fg(if is_extract { theme::MAUVE() } else { theme::GREEN() }),
        ),
    ]));

    // Detect value type
    let detected = jwt::detect_value_type(&value);
    let type_label = match &detected {
        ValueType::Empty => "Empty",
        ValueType::Jwt(_) => "JWT Token",
        ValueType::BearerOpaque(_) => "Bearer Token (Opaque)",
        ValueType::Url => "URL",
        ValueType::Uuid => "UUID",
        ValueType::Json => "JSON",
        ValueType::Plain => "String",
    };

    lines.push(Line::from(vec![
        Span::styled("  Type: ", label),
        Span::styled(type_label.to_string(), Style::default().fg(theme::PEACH())),
    ]));
    lines.push(Line::from(""));

    match detected {
        ValueType::Empty => {
            lines.push(Line::from(Span::styled("  (empty value)", dim)));
        }
        ValueType::Jwt(decoded) => {
            build_jwt_detail(&mut lines, &decoded, &value);
        }
        ValueType::BearerOpaque(inner) => {
            lines.push(Line::from(Span::styled("  Value", h)));
            let preview = if inner.len() > 80 {
                format!("  {}…", &inner[..80])
            } else {
                format!("  {}", inner)
            };
            lines.push(Line::from(Span::styled(preview, val_style)));
            lines.push(Line::from(Span::styled(
                format!("  Length: {} chars", inner.len()),
                dim,
            )));
        }
        ValueType::Url => {
            build_url_detail(&mut lines, &value);
        }
        ValueType::Uuid => {
            lines.push(Line::from(Span::styled("  Value", h)));
            lines.push(Line::from(Span::styled(
                format!("  {}", value),
                Style::default().fg(theme::LAVENDER()),
            )));
        }
        ValueType::Json => {
            build_json_detail(&mut lines, &value);
        }
        ValueType::Plain => {
            lines.push(Line::from(Span::styled("  Value", h)));
            // Show value, wrapping long values across multiple lines
            let max_line = 70;
            if value.len() <= max_line {
                lines.push(Line::from(Span::styled(format!("  {}", value), val_style)));
            } else {
                for chunk in value.as_bytes().chunks(max_line) {
                    let s = String::from_utf8_lossy(chunk);
                    lines.push(Line::from(Span::styled(format!("  {}", s), val_style)));
                }
            }
            lines.push(Line::from(Span::styled(
                format!("  Length: {} chars", value.len()),
                dim,
            )));
        }
    }

    // Show which files/blocks use this variable
    lines.push(Line::from(""));
    build_usage_section(&mut lines, app, &name);

    (title, lines)
}

fn build_jwt_detail(lines: &mut Vec<Line<'static>>, decoded: &jwt::DecodedJwt, _raw: &str) {
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let label = Style::default().fg(theme::TEXT_DIM());
    let val_style = Style::default().fg(theme::TEXT());
    let dim = Style::default().fg(theme::TEXT_FAINT());

    // Token info bar
    let mut info_parts = vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            format!(" {} ", decoded.token_type),
            Style::default().fg(theme::BG_SURFACE()).bg(theme::PEACH()),
        ),
    ];
    if let Some(ref exp) = decoded.expiry {
        let (exp_text, exp_color) = if exp.is_expired {
            (format!(" ⚠ {} ", exp.display), theme::RED())
        } else {
            (format!(" ✓ {} remaining ", exp.display), theme::GREEN())
        };
        info_parts.push(Span::styled("  ", Style::default()));
        info_parts.push(Span::styled(exp_text, Style::default().fg(exp_color)));
    }
    lines.push(Line::from(info_parts));
    lines.push(Line::from(""));

    // Key Claims summary
    let claims = jwt::key_claims(&decoded.payload);
    if !claims.is_empty() {
        lines.push(Line::from(Span::styled("  Key Claims", h)));
        for (key, value) in &claims {
            let display_val = if value.len() > 60 {
                format!("{}…", &value[..60])
            } else {
                value.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("    {}: ", key), label),
                Span::styled(display_val, val_style),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Header JSON
    lines.push(Line::from(vec![
        Span::styled("  Header", h),
        Span::styled(
            format!(
                " ({} fields)",
                decoded.header.as_object().map(|o| o.len()).unwrap_or(0)
            ),
            dim,
        ),
    ]));
    if let Ok(pretty) = serde_json::to_string_pretty(&decoded.header) {
        for line in pretty.lines() {
            lines.push(Line::from(Span::styled(
                format!("    {}", line),
                Style::default().fg(theme::SKY()),
            )));
        }
    }
    lines.push(Line::from(""));

    // Payload JSON
    lines.push(Line::from(vec![
        Span::styled("  Payload", h),
        Span::styled(
            format!(
                " ({} claims)",
                decoded.payload.as_object().map(|o| o.len()).unwrap_or(0)
            ),
            dim,
        ),
    ]));
    if let Ok(pretty) = serde_json::to_string_pretty(&decoded.payload) {
        for line in pretty.lines() {
            lines.push(Line::from(Span::styled(
                format!("    {}", line),
                Style::default().fg(theme::SKY()),
            )));
        }
    }
    lines.push(Line::from(""));

    // Signature preview
    lines.push(Line::from(Span::styled("  Signature", h)));
    lines.push(Line::from(Span::styled(
        format!("    {}", decoded.signature_preview),
        dim,
    )));
}

fn build_url_detail(lines: &mut Vec<Line<'static>>, value: &str) {
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let label = Style::default().fg(theme::TEXT_DIM());
    let val_style = Style::default().fg(theme::TEXT());

    lines.push(Line::from(Span::styled("  URL", h)));
    lines.push(Line::from(Span::styled(
        format!("  {}", value),
        Style::default().fg(theme::LAVENDER()),
    )));

    // Parse components
    if let Some(after_scheme) = value.find("://") {
        let scheme = &value[..after_scheme];
        let rest = &value[after_scheme + 3..];

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Components", h)));
        lines.push(Line::from(vec![
            Span::styled("    Scheme: ", label),
            Span::styled(scheme.to_string(), val_style),
        ]));

        let (host_port, path_query) = if let Some(slash) = rest.find('/') {
            (&rest[..slash], &rest[slash..])
        } else {
            (rest, "")
        };

        let (host, port) = if let Some(colon) = host_port.rfind(':') {
            let port_str = &host_port[colon + 1..];
            if port_str.chars().all(|c| c.is_ascii_digit()) {
                (&host_port[..colon], Some(port_str))
            } else {
                (host_port, None)
            }
        } else {
            (host_port, None)
        };

        lines.push(Line::from(vec![
            Span::styled("    Host:   ", label),
            Span::styled(host.to_string(), val_style),
        ]));
        if let Some(p) = port {
            lines.push(Line::from(vec![
                Span::styled("    Port:   ", label),
                Span::styled(p.to_string(), val_style),
            ]));
        }

        if !path_query.is_empty() {
            let (path, query) = if let Some(q) = path_query.find('?') {
                (&path_query[..q], Some(&path_query[q + 1..]))
            } else {
                (path_query, None)
            };
            lines.push(Line::from(vec![
                Span::styled("    Path:   ", label),
                Span::styled(path.to_string(), val_style),
            ]));
            if let Some(q) = query {
                lines.push(Line::from(vec![
                    Span::styled("    Query:  ", label),
                    Span::styled(q.to_string(), val_style),
                ]));
                // Parse query params
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("  Query Parameters", h)));
                for param in q.split('&') {
                    if let Some(eq) = param.find('=') {
                        lines.push(Line::from(vec![
                            Span::styled(format!("    {}", &param[..eq]), label),
                            Span::styled(" = ", Style::default().fg(theme::TEXT_FAINT())),
                            Span::styled(param[eq + 1..].to_string(), val_style),
                        ]));
                    } else {
                        lines.push(Line::from(Span::styled(format!("    {}", param), val_style)));
                    }
                }
            }
        }
    }
}

fn build_json_detail(lines: &mut Vec<Line<'static>>, value: &str) {
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::TEXT_FAINT());

    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) {
        let field_count = match &parsed {
            serde_json::Value::Object(o) => format!("{} fields", o.len()),
            serde_json::Value::Array(a) => format!("{} items", a.len()),
            _ => String::new(),
        };
        lines.push(Line::from(vec![
            Span::styled("  Value", h),
            Span::styled(format!(" ({})", field_count), dim),
        ]));

        if let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
            let max_lines = 40;
            let json_lines: Vec<&str> = pretty.lines().collect();
            let shown = json_lines.len().min(max_lines);
            for line in &json_lines[..shown] {
                lines.push(Line::from(Span::styled(
                    format!("    {}", line),
                    Style::default().fg(theme::SKY()),
                )));
            }
            if json_lines.len() > max_lines {
                lines.push(Line::from(Span::styled(
                    format!("    … {} more lines", json_lines.len() - max_lines),
                    dim,
                )));
            }
        }
    }
}

/// Show which files and blocks reference this variable.
fn build_usage_section(lines: &mut Vec<Line<'static>>, app: &App, var_name: &str) {
    let h = Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::TEXT_FAINT());
    let val_style = Style::default().fg(theme::TEXT());

    let placeholder = format!("{{{{{}}}}}", var_name);
    let mut usages: Vec<(String, String)> = Vec::new(); // (file, block)

    for file in &app.loaded_files {
        for block in &file.suite.blocks {
            // Check URL, headers, body, assertions, extracts
            let mut used = false;
            if block.request.url.contains(&placeholder) {
                used = true;
            }
            for (_, hv) in &block.request.headers {
                if hv.contains(&placeholder) {
                    used = true;
                }
            }
            if block.request.body.as_deref().unwrap_or("").contains(&placeholder) {
                used = true;
            }
            for ext in &block.extracts {
                if ext.variable_name == var_name {
                    used = true;
                }
            }
            if used {
                usages.push((file.name.clone(), block.name.clone()));
            }
        }
    }

    if usages.is_empty() {
        return;
    }

    lines.push(Line::from(vec![
        Span::styled("  Used In", h),
        Span::styled(format!(" ({} blocks)", usages.len()), dim),
    ]));
    for (file, block) in &usages {
        let block_display = if block.is_empty() { "(unnamed)" } else { block.as_str() };
        lines.push(Line::from(vec![
            Span::styled(format!("    {} ", file), dim),
            Span::styled("→ ", Style::default().fg(theme::TEXT_FAINT())),
            Span::styled(block_display.to_string(), val_style),
        ]));
    }
}

/// Render the variable detail popup overlay.
pub fn render_var_detail(frame: &mut Frame, app: &App, area: Rect) {
    let w = area.width.saturating_sub(6);
    let h = area.height.saturating_sub(4);
    if w < 20 || h < 8 {
        return;
    }
    let popup_width = 85.min(w);
    let popup_height = h.min(45);
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let (title, content_lines) = build_var_detail_lines(app);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_SURFACE()));

    let para = Paragraph::new(content_lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.var_detail_scroll, 0));

    frame.render_widget(para, popup_area);

    // Footer key hints
    let hint_y = popup_area.y + popup_area.height - 1;
    if hint_y < area.height {
        let hint_area = Rect::new(
            popup_area.x + 2,
            hint_y,
            popup_area.width.saturating_sub(4),
            1,
        );
        let hint = Paragraph::new(Line::from(vec![
            Span::styled("↑↓", Style::default().fg(theme::PEACH())),
            Span::styled("=scroll ", Style::default().fg(theme::TEXT_FAINT())),
            Span::styled("Ctrl+U/D", Style::default().fg(theme::PEACH())),
            Span::styled("=half-page ", Style::default().fg(theme::TEXT_FAINT())),
            Span::styled("Enter/Esc", Style::default().fg(theme::PEACH())),
            Span::styled("=close", Style::default().fg(theme::TEXT_FAINT())),
        ]));
        frame.render_widget(hint, hint_area);
    }
}

/// Handle key events for the variable detail popup.
/// Returns true if the event was consumed.
pub fn handle_var_detail_keys(app: &mut App, key: KeyEvent) -> bool {
    if !app.var_detail_open {
        return false;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
            app.var_detail_open = false;
            app.var_detail_scroll = 0;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.var_detail_scroll = app.var_detail_scroll.saturating_add(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.var_detail_scroll = app.var_detail_scroll.saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.var_detail_scroll = app.var_detail_scroll.saturating_add(10);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.var_detail_scroll = app.var_detail_scroll.saturating_sub(10);
        }
        KeyCode::Char('g') => {
            app.var_detail_scroll = 0;
        }
        KeyCode::Char('G') => {
            app.var_detail_scroll = 500;
        }
        _ => {}
    }
    true
}
