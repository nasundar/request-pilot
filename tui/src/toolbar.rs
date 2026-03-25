//! Toolbar features: extra headers, clear all, Azure auth, OTEL status.
//!
//! Each feature is exposed as a popup overlay opened by a Ctrl-key shortcut.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, AzureAuthState, HeaderEditField, HeaderEditMode, RunnerMessage};
use crate::ui::theme;
use crate::ui::truncate_to;

fn centered_popup(area: Rect, w: u16, h: u16) -> Rect {
    let pw = w.min(area.width.saturating_sub(4));
    let ph = h.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(pw)) / 2;
    let y = area.y + (area.height.saturating_sub(ph)) / 2;
    Rect::new(x, y, pw, ph)
}

/// Build the toolbar badge spans shown in the top-right of the header bar.
pub fn toolbar_badges(app: &App) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let enabled = app.extra_headers.iter().filter(|(_, _, e)| *e).count();
    if !app.extra_headers.is_empty() {
        spans.push(Span::styled(
            format!(" H:{} ", enabled),
            Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD),
        ));
    }
    match app.azure_state {
        AzureAuthState::Off => {
            spans.push(Span::styled(" \u{1f512} ", Style::default().fg(theme::TEXT_FAINT())));
        }
        AzureAuthState::Authenticated => {
            spans.push(Span::styled(" \u{2713}Az ", Style::default().fg(theme::GREEN()).add_modifier(Modifier::BOLD)));
        }
        AzureAuthState::Expired => {
            spans.push(Span::styled(" \u{26a0}Az ", Style::default().fg(theme::YELLOW()).add_modifier(Modifier::BOLD)));
        }
    }
    if app.otel_enabled {
        spans.push(Span::styled(" \u{1f4e1} ", Style::default().fg(theme::SKY()).add_modifier(Modifier::BOLD)));
    }
    spans
}

pub fn render_extra_headers_popup(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_popup(area, 64, 22);
    if popup.width < 10 || popup.height < 5 { return; }
    frame.render_widget(Clear, popup);
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled("Extra Headers", Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));
    if app.extra_headers.is_empty() {
        lines.push(Line::from(Span::styled("  (no extra headers)", Style::default().fg(theme::TEXT_FAINT()))));
    } else {
        for (i, (key, val, en)) in app.extra_headers.iter().enumerate() {
            let marker = if i == app.extra_headers_cursor { "\u{25b8} " } else { "  " };
            let check = if *en { "\u{2611}" } else { "\u{2610}" };
            let style = if i == app.extra_headers_cursor {
                Style::default().fg(theme::TEXT()).bg(theme::BG_OVERLAY())
            } else {
                Style::default().fg(theme::TEXT())
            };
            let prefix_len = marker.len() + 3 + key.len() + 2;
            let max_val = (popup.width as usize).saturating_sub(prefix_len + 4);
            let display_val = truncate_to(val, max_val);
            lines.push(Line::from(vec![
                Span::styled(format!("{}{} ", marker, check), style),
                Span::styled(format!("{}: ", key), Style::default().fg(theme::PEACH())),
                Span::styled(display_val, Style::default().fg(theme::TEXT_DIM())),
            ]));
        }
    }
    match &app.extra_headers_edit_mode {
        HeaderEditMode::Browse => {}
        HeaderEditMode::Adding { field, key_buf, val_buf } => {
            lines.push(Line::from(""));
            let (ks, vs) = match field {
                HeaderEditField::Key => (
                    Style::default().fg(theme::GREEN()).add_modifier(Modifier::UNDERLINED),
                    Style::default().fg(theme::TEXT_FAINT()),
                ),
                HeaderEditField::Value => (
                    Style::default().fg(theme::GREEN()),
                    Style::default().fg(theme::GREEN()).add_modifier(Modifier::UNDERLINED),
                ),
            };
            lines.push(Line::from(vec![
                Span::styled("  Key: ", Style::default().fg(theme::TEXT_DIM())),
                Span::styled(format!("{}\u{2588}", key_buf), ks),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Val: ", Style::default().fg(theme::TEXT_DIM())),
                Span::styled(format!("{}\u{2588}", val_buf), vs),
            ]));
        }
        HeaderEditMode::Editing { field, key_buf, val_buf, .. } => {
            lines.push(Line::from(""));
            let (ks, vs) = match field {
                HeaderEditField::Key => (
                    Style::default().fg(theme::YELLOW()).add_modifier(Modifier::UNDERLINED),
                    Style::default().fg(theme::TEXT_FAINT()),
                ),
                HeaderEditField::Value => (
                    Style::default().fg(theme::YELLOW()),
                    Style::default().fg(theme::YELLOW()).add_modifier(Modifier::UNDERLINED),
                ),
            };
            lines.push(Line::from(vec![
                Span::styled("  Key: ", Style::default().fg(theme::TEXT_DIM())),
                Span::styled(format!("{}\u{2588}", key_buf), ks),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Val: ", Style::default().fg(theme::TEXT_DIM())),
                Span::styled(format!("{}\u{2588}", val_buf), vs),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" a", Style::default().fg(theme::PEACH())),
        Span::styled("=add ", Style::default().fg(theme::TEXT_FAINT())),
        Span::styled("d", Style::default().fg(theme::PEACH())),
        Span::styled("=del ", Style::default().fg(theme::TEXT_FAINT())),
        Span::styled("e", Style::default().fg(theme::PEACH())),
        Span::styled("=edit ", Style::default().fg(theme::TEXT_FAINT())),
        Span::styled("Space", Style::default().fg(theme::PEACH())),
        Span::styled("=toggle ", Style::default().fg(theme::TEXT_FAINT())),
        Span::styled("Esc", Style::default().fg(theme::PEACH())),
        Span::styled("=close", Style::default().fg(theme::TEXT_FAINT())),
    ]));
    let block = Block::default()
        .title(" \u{2708} Extra Headers ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_SURFACE()));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

pub fn render_azure_popup(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_popup(area, 56, 16);
    if popup.width < 10 || popup.height < 5 { return; }
    frame.render_widget(Clear, popup);
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled("Azure Authentication", Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));
    let (state_label, state_style) = match app.azure_state {
        AzureAuthState::Off => ("Off", Style::default().fg(theme::TEXT_FAINT())),
        AzureAuthState::Authenticated => ("Authenticated", Style::default().fg(theme::GREEN()).add_modifier(Modifier::BOLD)),
        AzureAuthState::Expired => ("Expired", Style::default().fg(theme::YELLOW()).add_modifier(Modifier::BOLD)),
    };
    lines.push(Line::from(vec![
        Span::styled("  Status: ", Style::default().fg(theme::TEXT_DIM())),
        Span::styled(state_label, state_style),
    ]));
    let cli_ok = app.az_cli_available.unwrap_or(false);
    lines.push(Line::from(vec![
        Span::styled("  Az CLI: ", Style::default().fg(theme::TEXT_DIM())),
        if cli_ok { Span::styled("Available", Style::default().fg(theme::GREEN())) }
        else { Span::styled("Not found (checking...)", Style::default().fg(theme::YELLOW())) },
    ]));
    if let Some(ref token) = app.azure_token {
        lines.push(Line::from(""));
        if let Some(ref tenant) = token.tenant {
            lines.push(Line::from(vec![
                Span::styled("  Tenant: ", Style::default().fg(theme::TEXT_DIM())),
                Span::styled(tenant.clone(), Style::default().fg(theme::TEXT())),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("  Expires: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(token.expires_on.clone(), Style::default().fg(theme::TEXT())),
        ]));
    }
    if app.azure_loading {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Authenticating...", Style::default().fg(theme::SKY()).add_modifier(Modifier::BOLD))));
    }
    lines.push(Line::from(""));
    if cli_ok && !app.azure_loading {
        lines.push(Line::from(Span::styled("  Press Enter to authenticate", Style::default().fg(theme::GREEN()))));
    }
    lines.push(Line::from(Span::styled("  Esc=close", Style::default().fg(theme::TEXT_FAINT()))));
    let block = Block::default()
        .title(" \u{1f512} Azure Auth ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_SURFACE()));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

pub fn render_otel_popup(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_popup(area, 52, 16);
    if popup.width < 10 || popup.height < 5 { return; }
    frame.render_widget(Clear, popup);
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled("OpenTelemetry Status", Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));
    let (el, es) = if app.otel_enabled {
        ("Enabled", Style::default().fg(theme::GREEN()).add_modifier(Modifier::BOLD))
    } else {
        ("Disabled", Style::default().fg(theme::TEXT_FAINT()))
    };
    lines.push(Line::from(vec![
        Span::styled("  Status: ", Style::default().fg(theme::TEXT_DIM())),
        Span::styled(el, es),
    ]));
    if let Some(ref stats) = app.otel_stats {
        let max_ep = (popup.width as usize).saturating_sub(16);
        let display_ep = truncate_to(&stats.endpoint, max_ep);
        lines.push(Line::from(vec![
            Span::styled("  Endpoint: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(display_ep, Style::default().fg(theme::TEXT())),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Traces sent:  ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(stats.traces_sent.to_string(), Style::default().fg(theme::PEACH())),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Metrics sent: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(stats.metrics_sent.to_string(), Style::default().fg(theme::PEACH())),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Logs sent:    ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(stats.logs_sent.to_string(), Style::default().fg(theme::PEACH())),
        ]));
        if !stats.errors.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(format!("  Errors: {}", stats.errors.len()), Style::default().fg(theme::RED()))));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Endpoint: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled("(not configured)", Style::default().fg(theme::TEXT_FAINT())),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" Space", Style::default().fg(theme::PEACH())),
        Span::styled("=toggle ", Style::default().fg(theme::TEXT_FAINT())),
        Span::styled("Esc", Style::default().fg(theme::PEACH())),
        Span::styled("=close", Style::default().fg(theme::TEXT_FAINT())),
    ]));
    let block = Block::default()
        .title(" \u{1f4e1} Telemetry ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_SURFACE()));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

pub fn handle_extra_headers_keys(app: &mut App, key: KeyEvent) -> bool {
    if !app.extra_headers_open { return false; }
    match &mut app.extra_headers_edit_mode {
        HeaderEditMode::Adding { field, key_buf, val_buf } => {
            match key.code {
                KeyCode::Esc => { app.extra_headers_edit_mode = HeaderEditMode::Browse; }
                KeyCode::Tab => {
                    *field = match field {
                        HeaderEditField::Key => HeaderEditField::Value,
                        HeaderEditField::Value => HeaderEditField::Key,
                    };
                }
                KeyCode::Enter => {
                    if *field == HeaderEditField::Key && !key_buf.is_empty() {
                        *field = HeaderEditField::Value;
                    } else if *field == HeaderEditField::Value {
                        let k = key_buf.clone();
                        let v = val_buf.clone();
                        if !k.is_empty() {
                            app.extra_headers.push((k, v, true));
                            app.extra_headers_cursor = app.extra_headers.len().saturating_sub(1);
                        }
                        app.extra_headers_edit_mode = HeaderEditMode::Browse;
                    }
                }
                KeyCode::Backspace => {
                    match field {
                        HeaderEditField::Key => { key_buf.pop(); }
                        HeaderEditField::Value => { val_buf.pop(); }
                    }
                }
                KeyCode::Char(c) => {
                    match field {
                        HeaderEditField::Key => key_buf.push(c),
                        HeaderEditField::Value => val_buf.push(c),
                    }
                }
                _ => {}
            }
            return true;
        }
        HeaderEditMode::Editing { index, field, key_buf, val_buf } => {
            match key.code {
                KeyCode::Esc => { app.extra_headers_edit_mode = HeaderEditMode::Browse; }
                KeyCode::Tab => {
                    *field = match field {
                        HeaderEditField::Key => HeaderEditField::Value,
                        HeaderEditField::Value => HeaderEditField::Key,
                    };
                }
                KeyCode::Enter => {
                    if *field == HeaderEditField::Key && !key_buf.is_empty() {
                        *field = HeaderEditField::Value;
                    } else if *field == HeaderEditField::Value {
                        let idx = *index;
                        let k = key_buf.clone();
                        let v = val_buf.clone();
                        if idx < app.extra_headers.len() && !k.is_empty() {
                            app.extra_headers[idx].0 = k;
                            app.extra_headers[idx].1 = v;
                        }
                        app.extra_headers_edit_mode = HeaderEditMode::Browse;
                    }
                }
                KeyCode::Backspace => {
                    match field {
                        HeaderEditField::Key => { key_buf.pop(); }
                        HeaderEditField::Value => { val_buf.pop(); }
                    }
                }
                KeyCode::Char(c) => {
                    match field {
                        HeaderEditField::Key => key_buf.push(c),
                        HeaderEditField::Value => val_buf.push(c),
                    }
                }
                _ => {}
            }
            return true;
        }
        HeaderEditMode::Browse => {}
    }
    match key.code {
        KeyCode::Esc => {
            app.extra_headers_open = false;
            app.extra_headers_edit_mode = HeaderEditMode::Browse;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.extra_headers_cursor > 0 { app.extra_headers_cursor -= 1; }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !app.extra_headers.is_empty() && app.extra_headers_cursor < app.extra_headers.len() - 1 {
                app.extra_headers_cursor += 1;
            }
        }
        KeyCode::Char(' ') => {
            if let Some(h) = app.extra_headers.get_mut(app.extra_headers_cursor) { h.2 = !h.2; }
        }
        KeyCode::Char('a') => {
            app.extra_headers_edit_mode = HeaderEditMode::Adding {
                field: HeaderEditField::Key,
                key_buf: String::new(),
                val_buf: String::new(),
            };
        }
        KeyCode::Char('d') => {
            if !app.extra_headers.is_empty() {
                app.extra_headers.remove(app.extra_headers_cursor);
                if app.extra_headers_cursor >= app.extra_headers.len() && !app.extra_headers.is_empty() {
                    app.extra_headers_cursor = app.extra_headers.len() - 1;
                }
            }
        }
        KeyCode::Char('e') => {
            if let Some((k, v, _)) = app.extra_headers.get(app.extra_headers_cursor) {
                app.extra_headers_edit_mode = HeaderEditMode::Editing {
                    index: app.extra_headers_cursor,
                    field: HeaderEditField::Key,
                    key_buf: k.clone(),
                    val_buf: v.clone(),
                };
            }
        }
        _ => {}
    }
    true
}

pub fn handle_azure_keys(app: &mut App, key: KeyEvent) -> bool {
    if !app.azure_popup_open { return false; }
    match key.code {
        KeyCode::Esc => { app.azure_popup_open = false; }
        KeyCode::Enter => {
            let cli_ok = *app.az_cli_available.get_or_insert_with(|| {
                request_pilot_core::azure_auth::is_az_cli_available()
            });
            if !app.azure_loading && cli_ok {
                app.azure_loading = true;
                app.set_status("⏳ Fetching Azure token...".to_string());
                let tx = app.runner_tx();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(|| {
                        request_pilot_core::azure_auth::fetch_token("https://management.azure.com")
                    }).await;
                    let msg = match result {
                        Ok(Ok(token)) => RunnerMessage::AzureAuthResult(Ok(token)),
                        Ok(Err(e)) => RunnerMessage::AzureAuthResult(Err(e)),
                        Err(e) => RunnerMessage::AzureAuthResult(Err(format!("Task panicked: {}", e))),
                    };
                    let _ = tx.send(msg);
                });
            }
        }
        _ => {}
    }
    true
}

pub fn handle_otel_keys(app: &mut App, key: KeyEvent) -> bool {
    if !app.otel_popup_open { return false; }
    match key.code {
        KeyCode::Esc => { app.otel_popup_open = false; }
        KeyCode::Char(' ') => {
            app.otel_enabled = !app.otel_enabled;
            let state = if app.otel_enabled { "enabled" } else { "disabled" };
            app.set_status(format!("Telemetry {}", state));
        }
        _ => {}
    }
    true
}

pub fn handle_toolbar_shortcuts(app: &mut App, key: KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) { return false; }
    match key.code {
        KeyCode::Char('h') => {
            app.extra_headers_open = !app.extra_headers_open;
            if app.extra_headers_open {
                app.extra_headers_edit_mode = HeaderEditMode::Browse;
                app.azure_popup_open = false;
                app.otel_popup_open = false;
            }
            true
        }
        KeyCode::Char('x') => { app.clear_all(); true }
        KeyCode::Char('a') => {
            app.azure_popup_open = !app.azure_popup_open;
            if app.azure_popup_open {
                app.extra_headers_open = false;
                app.otel_popup_open = false;
                // Check CLI availability async on first open
                if app.az_cli_available.is_none() {
                    let tx = app.runner_tx();
                    tokio::spawn(async move {
                        let available = tokio::task::spawn_blocking(|| {
                            request_pilot_core::azure_auth::is_az_cli_available()
                        }).await.unwrap_or(false);
                        let _ = tx.send(RunnerMessage::AzureCliCheck(available));
                    });
                }
            }
            true
        }
        KeyCode::Char('t') => {
            app.otel_popup_open = !app.otel_popup_open;
            if app.otel_popup_open {
                app.extra_headers_open = false;
                app.azure_popup_open = false;
            }
            true
        }
        _ => false,
    }
}
