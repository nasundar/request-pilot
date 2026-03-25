//! Logs panel — application log viewer with level filtering and auto-scroll.
#![allow(dead_code)]

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, LogEntry, LogFilter, LogLevel};
use crate::ui::theme;

const MAX_LOG_ENTRIES: usize = 2000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn level_style(level: &LogLevel) -> Style {
    match level {
        LogLevel::Debug => Style::default().fg(theme::TEXT_FAINT()),
        LogLevel::Info => Style::default().fg(theme::BLUE()),
        LogLevel::Warn => Style::default().fg(theme::YELLOW()),
        LogLevel::Error => Style::default().fg(theme::RED()),
    }
}

fn level_badge(level: &LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "DBG",
        LogLevel::Info => "INF",
        LogLevel::Warn => "WRN",
        LogLevel::Error => "ERR",
    }
}

fn matches_filter(entry: &LogEntry, filter: &LogFilter) -> bool {
    match filter {
        LogFilter::All => true,
        LogFilter::Debug => true,
        LogFilter::Info => !matches!(entry.level, LogLevel::Debug),
        LogFilter::Warn => matches!(entry.level, LogLevel::Warn | LogLevel::Error),
        LogFilter::Error => matches!(entry.level, LogLevel::Error),
    }
}

// ---------------------------------------------------------------------------
// App integration
// ---------------------------------------------------------------------------

impl App {
    pub fn log(&mut self, level: LogLevel, message: String) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| {
                let secs = d.as_secs();
                let hrs = (secs / 3600) % 24;
                let mins = (secs / 60) % 60;
                let s = secs % 60;
                format!("{:02}:{:02}:{:02}", hrs, mins, s)
            })
            .unwrap_or_else(|_| "??:??:??".to_string());

        self.log_entries.push(LogEntry {
            timestamp,
            level,
            message,
        });

        // FIFO: trim oldest entries
        if self.log_entries.len() > MAX_LOG_ENTRIES {
            let excess = self.log_entries.len() - MAX_LOG_ENTRIES;
            self.log_entries.drain(..excess);
        }

        // Auto-scroll to bottom when new entries arrive
        if self.log_auto_scroll {
            self.log_scroll = self.log_entries.len().saturating_sub(1);
        }
    }

    pub fn log_info(&mut self, message: String) {
        self.log(LogLevel::Info, message);
    }

    pub fn log_error(&mut self, message: String) {
        self.log(LogLevel::Error, message);
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn render_logs_mode(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);

    render_log_toolbar(frame, app, chunks[0]);
    render_log_entries(frame, app, chunks[1]);
}

fn render_log_toolbar(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" 📋 Logs ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()));

    let filter_label = match app.log_filter {
        LogFilter::All => "All",
        LogFilter::Debug => "≥Debug",
        LogFilter::Info => "≥Info",
        LogFilter::Warn => "≥Warn",
        LogFilter::Error => "Error",
    };

    let total = app.log_entries.len();
    let visible: usize = app
        .log_entries
        .iter()
        .filter(|e| matches_filter(e, &app.log_filter))
        .count();

    let auto_label = if app.log_auto_scroll {
        "auto-scroll ON"
    } else {
        "auto-scroll OFF"
    };

    let line = Line::from(vec![
        Span::styled(
            format!("  Filter: {} ", filter_label),
            Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}/{} entries  ", visible, total),
            Style::default().fg(theme::TEXT_DIM()),
        ),
        Span::styled(
            format!("  {}  ", auto_label),
            Style::default().fg(if app.log_auto_scroll {
                theme::GREEN()
            } else {
                theme::TEXT_FAINT()
            }),
        ),
        Span::styled(
            "  f=filter  c=clear  a=auto-scroll  ",
            Style::default().fg(theme::TEXT_FAINT()),
        ),
    ]);

    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn render_log_entries(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::TEXT_FAINT()));

    let filtered: Vec<&LogEntry> = app
        .log_entries
        .iter()
        .filter(|e| matches_filter(e, &app.log_filter))
        .collect();

    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("  No log entries. Logs will appear as you use the app.")
                .block(block)
                .style(Style::default().fg(theme::TEXT_FAINT())),
            area,
        );
        return;
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let total = filtered.len();
    let scroll = app.log_scroll.min(total.saturating_sub(inner_height));

    let items: Vec<ListItem> = filtered
        .iter()
        .skip(scroll)
        .take(inner_height)
        .map(|entry| {
            let badge = level_badge(&entry.level);
            let badge_style = level_style(&entry.level).add_modifier(Modifier::BOLD);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", &entry.timestamp),
                    Style::default().fg(theme::TEXT_FAINT()),
                ),
                Span::styled(format!("[{}]", badge), badge_style),
                Span::styled(
                    format!(" {}", &entry.message),
                    Style::default().fg(theme::TEXT()),
                ),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

pub fn handle_logs_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            app.log_auto_scroll = false;
            app.log_scroll = app.log_scroll.saturating_add(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.log_auto_scroll = false;
            app.log_scroll = app.log_scroll.saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.log_auto_scroll = false;
            app.log_scroll = app.log_scroll.saturating_add(20);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.log_auto_scroll = false;
            app.log_scroll = app.log_scroll.saturating_sub(20);
        }
        KeyCode::Char('g') => {
            app.log_auto_scroll = false;
            app.log_scroll = 0;
        }
        KeyCode::Char('G') => {
            app.log_auto_scroll = true;
            app.log_scroll = app.log_entries.len().saturating_sub(1);
        }
        KeyCode::Char('f') => {
            app.log_filter = match app.log_filter {
                LogFilter::All => LogFilter::Debug,
                LogFilter::Debug => LogFilter::Info,
                LogFilter::Info => LogFilter::Warn,
                LogFilter::Warn => LogFilter::Error,
                LogFilter::Error => LogFilter::All,
            };
        }
        KeyCode::Char('a') => {
            app.log_auto_scroll = !app.log_auto_scroll;
            if app.log_auto_scroll {
                app.log_scroll = app.log_entries.len().saturating_sub(1);
            }
        }
        KeyCode::Char('c') => {
            app.log_entries.clear();
            app.log_scroll = 0;
        }
        _ => {}
    }
}
