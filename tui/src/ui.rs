use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use crate::app::{App, Mode, Focus, SidebarTab, InputMode};
use crate::components;
use crate::toolbar;
use request_pilot_core::history::HistoryFilter;

/// Truncate text to fit within max_width chars, adding ellipsis if needed.
pub(crate) fn truncate_to(text: &str, max_width: usize) -> String {
    if max_width == 0 { return String::new(); }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_width {
        text.to_string()
    } else {
        let mut s: String = chars[..max_width.saturating_sub(1)].iter().collect();
        s.push('…');
        s
    }
}

// Runtime-switchable color palettes
#[allow(non_snake_case)]
pub mod theme {
    use ratatui::style::Color;
    use std::sync::atomic::{AtomicU8, Ordering};

    static ACTIVE: AtomicU8 = AtomicU8::new(2);

    #[derive(Clone, Copy)]
    pub struct Palette {
        pub bg_dark: Color,
        pub bg_base: Color,
        pub bg_surface: Color,
        pub bg_overlay: Color,
        pub bg_highlight: Color,
        pub text: Color,
        pub text_dim: Color,
        pub text_faint: Color,
        pub blue: Color,
        pub lavender: Color,
        pub sapphire: Color,
        pub green: Color,
        pub yellow: Color,
        pub peach: Color,
        pub red: Color,
        pub pink: Color,
        pub mauve: Color,
        pub sky: Color,
    }

    pub const PALETTE_COUNT: usize = 7;

    pub const PALETTES: [Palette; PALETTE_COUNT] = [
        // 0: Tokyo Night
        Palette {
            bg_dark:      Color::Rgb(26, 27, 38),
            bg_base:      Color::Rgb(30, 31, 43),
            bg_surface:   Color::Rgb(42, 44, 60),
            bg_overlay:   Color::Rgb(55, 58, 77),
            bg_highlight: Color::Rgb(73, 77, 100),
            text:         Color::Rgb(169, 177, 214),
            text_dim:     Color::Rgb(120, 130, 165),
            text_faint:   Color::Rgb(86, 95, 137),
            blue:         Color::Rgb(122, 162, 247),
            lavender:     Color::Rgb(187, 154, 247),
            sapphire:     Color::Rgb(125, 207, 255),
            green:        Color::Rgb(158, 206, 106),
            yellow:       Color::Rgb(224, 175, 104),
            peach:        Color::Rgb(255, 158, 100),
            red:          Color::Rgb(247, 118, 142),
            pink:         Color::Rgb(255, 117, 127),
            mauve:        Color::Rgb(187, 154, 247),
            sky:          Color::Rgb(125, 207, 255),
        },
        // 1: Dracula
        Palette {
            bg_dark:      Color::Rgb(40, 42, 54),
            bg_base:      Color::Rgb(44, 47, 60),
            bg_surface:   Color::Rgb(55, 58, 74),
            bg_overlay:   Color::Rgb(68, 71, 90),
            bg_highlight: Color::Rgb(98, 114, 164),
            text:         Color::Rgb(248, 248, 242),
            text_dim:     Color::Rgb(189, 147, 249),
            text_faint:   Color::Rgb(98, 114, 164),
            blue:         Color::Rgb(139, 233, 253),
            lavender:     Color::Rgb(189, 147, 249),
            sapphire:     Color::Rgb(139, 233, 253),
            green:        Color::Rgb(80, 250, 123),
            yellow:       Color::Rgb(241, 250, 140),
            peach:        Color::Rgb(255, 184, 108),
            red:          Color::Rgb(255, 85, 85),
            pink:         Color::Rgb(255, 121, 198),
            mauve:        Color::Rgb(189, 147, 249),
            sky:          Color::Rgb(139, 233, 253),
        },
        // 2: Gruvbox Dark
        Palette {
            bg_dark:      Color::Rgb(40, 40, 40),
            bg_base:      Color::Rgb(50, 48, 47),
            bg_surface:   Color::Rgb(60, 56, 54),
            bg_overlay:   Color::Rgb(80, 73, 69),
            bg_highlight: Color::Rgb(102, 92, 84),
            text:         Color::Rgb(235, 219, 178),
            text_dim:     Color::Rgb(189, 174, 147),
            text_faint:   Color::Rgb(146, 131, 116),
            blue:         Color::Rgb(131, 165, 152),
            lavender:     Color::Rgb(211, 134, 155),
            sapphire:     Color::Rgb(131, 165, 152),
            green:        Color::Rgb(184, 187, 38),
            yellow:       Color::Rgb(250, 189, 47),
            peach:        Color::Rgb(254, 128, 25),
            red:          Color::Rgb(251, 73, 52),
            pink:         Color::Rgb(211, 134, 155),
            mauve:        Color::Rgb(211, 134, 155),
            sky:          Color::Rgb(131, 165, 152),
        },
        // 3: Nord
        Palette {
            bg_dark:      Color::Rgb(46, 52, 64),
            bg_base:      Color::Rgb(52, 59, 73),
            bg_surface:   Color::Rgb(59, 66, 82),
            bg_overlay:   Color::Rgb(67, 76, 94),
            bg_highlight: Color::Rgb(76, 86, 106),
            text:         Color::Rgb(216, 222, 233),
            text_dim:     Color::Rgb(171, 178, 191),
            text_faint:   Color::Rgb(127, 140, 160),
            blue:         Color::Rgb(136, 192, 208),
            lavender:     Color::Rgb(180, 142, 173),
            sapphire:     Color::Rgb(129, 161, 193),
            green:        Color::Rgb(163, 190, 140),
            yellow:       Color::Rgb(235, 203, 139),
            peach:        Color::Rgb(208, 135, 112),
            red:          Color::Rgb(191, 97, 106),
            pink:         Color::Rgb(180, 142, 173),
            mauve:        Color::Rgb(180, 142, 173),
            sky:          Color::Rgb(136, 192, 208),
        },
        // 4: One Dark
        Palette {
            bg_dark:      Color::Rgb(33, 37, 43),
            bg_base:      Color::Rgb(40, 44, 52),
            bg_surface:   Color::Rgb(50, 55, 65),
            bg_overlay:   Color::Rgb(62, 68, 81),
            bg_highlight: Color::Rgb(78, 86, 102),
            text:         Color::Rgb(171, 178, 191),
            text_dim:     Color::Rgb(130, 137, 151),
            text_faint:   Color::Rgb(92, 99, 112),
            blue:         Color::Rgb(97, 175, 239),
            lavender:     Color::Rgb(198, 120, 221),
            sapphire:     Color::Rgb(86, 182, 194),
            green:        Color::Rgb(152, 195, 121),
            yellow:       Color::Rgb(229, 192, 123),
            peach:        Color::Rgb(209, 154, 102),
            red:          Color::Rgb(224, 108, 117),
            pink:         Color::Rgb(198, 120, 221),
            mauve:        Color::Rgb(198, 120, 221),
            sky:          Color::Rgb(86, 182, 194),
        },
        // 5: Everforest
        Palette {
            bg_dark:      Color::Rgb(39, 46, 34),
            bg_base:      Color::Rgb(45, 53, 39),
            bg_surface:   Color::Rgb(52, 61, 46),
            bg_overlay:   Color::Rgb(68, 78, 60),
            bg_highlight: Color::Rgb(90, 101, 80),
            text:         Color::Rgb(211, 198, 170),
            text_dim:     Color::Rgb(168, 159, 138),
            text_faint:   Color::Rgb(127, 120, 105),
            blue:         Color::Rgb(127, 187, 179),
            lavender:     Color::Rgb(214, 153, 182),
            sapphire:     Color::Rgb(131, 192, 146),
            green:        Color::Rgb(167, 192, 128),
            yellow:       Color::Rgb(219, 188, 127),
            peach:        Color::Rgb(230, 152, 117),
            red:          Color::Rgb(230, 126, 128),
            pink:         Color::Rgb(214, 153, 182),
            mauve:        Color::Rgb(214, 153, 182),
            sky:          Color::Rgb(131, 192, 146),
        },
        // 6: Kanagawa
        Palette {
            bg_dark:      Color::Rgb(22, 22, 29),
            bg_base:      Color::Rgb(30, 30, 41),
            bg_surface:   Color::Rgb(42, 42, 55),
            bg_overlay:   Color::Rgb(54, 54, 70),
            bg_highlight: Color::Rgb(73, 73, 95),
            text:         Color::Rgb(220, 215, 186),
            text_dim:     Color::Rgb(168, 162, 138),
            text_faint:   Color::Rgb(114, 108, 92),
            blue:         Color::Rgb(127, 180, 202),
            lavender:     Color::Rgb(149, 127, 184),
            sapphire:     Color::Rgb(106, 149, 137),
            green:        Color::Rgb(152, 187, 108),
            yellow:       Color::Rgb(226, 194, 110),
            peach:        Color::Rgb(255, 160, 102),
            red:          Color::Rgb(195, 64, 67),
            pink:         Color::Rgb(214, 126, 162),
            mauve:        Color::Rgb(149, 127, 184),
            sky:          Color::Rgb(106, 149, 137),
        },
    ];

    pub const PALETTE_NAMES: [&str; PALETTE_COUNT] = [
        "Tokyo Night", "Dracula", "Gruvbox Dark", "Nord", "One Dark", "Everforest", "Kanagawa",
    ];

    fn p() -> &'static Palette {
        &PALETTES[ACTIVE.load(Ordering::Relaxed) as usize]
    }

    pub fn active_name() -> &'static str {
        PALETTE_NAMES[ACTIVE.load(Ordering::Relaxed) as usize]
    }

    pub fn cycle_next() {
        ACTIVE.store(
            ((ACTIVE.load(Ordering::Relaxed) as usize + 1) % PALETTE_COUNT) as u8,
            Ordering::Relaxed,
        );
    }

    pub fn BG_DARK() -> Color { p().bg_dark }
    pub fn BG_BASE() -> Color { p().bg_base }
    pub fn BG_SURFACE() -> Color { p().bg_surface }
    pub fn BG_OVERLAY() -> Color { p().bg_overlay }
    pub fn BG_HIGHLIGHT() -> Color { p().bg_highlight }
    pub fn TEXT() -> Color { p().text }
    pub fn TEXT_DIM() -> Color { p().text_dim }
    pub fn TEXT_FAINT() -> Color { p().text_faint }
    pub fn BLUE() -> Color { p().blue }
    pub fn LAVENDER() -> Color { p().lavender }
    pub fn SAPPHIRE() -> Color { p().sapphire }
    pub fn GREEN() -> Color { p().green }
    pub fn YELLOW() -> Color { p().yellow }
    pub fn PEACH() -> Color { p().peach }
    pub fn RED() -> Color { p().red }
    pub fn PINK() -> Color { p().pink }
    pub fn MAUVE() -> Color { p().mauve }
    pub fn SKY() -> Color { p().sky }
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
    let show_progress = !app.run_progress_lines.is_empty() || app.is_running;

    let mut constraints = vec![
        Constraint::Length(1),    // top bar
    ];
    constraints.push(Constraint::Min(10)); // main area
    if show_progress {
        constraints.push(Constraint::Length(4)); // progress strip
    }
    if has_input {
        constraints.push(Constraint::Length(1)); // input bar
    }
    constraints.push(Constraint::Length(1)); // status bar

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    let mut ci = 0;
    draw_top_bar(frame, app, chunks[ci]); ci += 1;
    draw_main(frame, app, chunks[ci]); ci += 1;
    if show_progress {
        render_progress_strip(frame, app, chunks[ci]); ci += 1;
    }
    if has_input {
        draw_input_bar(frame, app, chunks[ci]); ci += 1;
    }
    draw_status_bar(frame, app, chunks[ci]);

    // Expanded progress overlay
    if app.progress_expanded && show_progress {
        render_progress_expanded(frame, app, frame.area());
    }

    if app.show_help {
        components::overlays::render_help_popup(frame, frame.area());
    }

    if let Some(idx) = app.history_detail_idx {
        draw_history_detail(frame, app, frame.area(), idx);
    }

    // Diff viewer overlay
    if app.diff_viewer_open {
        components::diff_viewer::render_diff_overlay(frame, app, frame.area());
    }

    // Toolbar popup overlays
    if app.extra_headers_open {
        toolbar::render_extra_headers_popup(frame, app, frame.area());
    }
    if app.azure_popup_open {
        toolbar::render_azure_popup(frame, app, frame.area());
    }
    if app.otel_popup_open {
        toolbar::render_otel_popup(frame, app, frame.area());
    }
}

fn draw_top_bar(frame: &mut Frame, app: &App, area: Rect) {
    if area.width < 10 {
        frame.render_widget(Paragraph::new("").style(Style::default().bg(theme::BG_BASE())), area);
        return;
    }
    let mode_style = |m: Mode, current: Mode| {
        if m == current {
            Style::default().fg(theme::BG_DARK()).bg(theme::BLUE()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT_FAINT())
        }
    };

    let mut spans = vec![
        Span::styled(" ✈ Request Pilot ", Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(" f Files ", mode_style(Mode::Files, app.mode)),
        Span::raw(" "),
        Span::styled(" h History ", mode_style(Mode::History, app.mode)),
        Span::raw(" "),
        Span::styled(" c Code ", mode_style(Mode::Code, app.mode)),
        Span::raw(" "),
        Span::styled(" l Logs ", mode_style(Mode::Logs, app.mode)),
        Span::raw("  "),
        Span::styled("R Run All", Style::default().fg(theme::GREEN())),
        Span::raw("  "),
        Span::styled("? Help", Style::default().fg(theme::TEXT_FAINT())),
        Span::raw("  "),
        Span::styled(format!("\u{1f3a8} {} ", theme::active_name()), Style::default().fg(theme::TEXT_DIM())),
        Span::raw("  "),
    ];
    spans.extend(toolbar::toolbar_badges(app));
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(theme::BG_BASE())), area);
}

fn draw_main(frame: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Files => draw_files_mode(frame, app, area),
        Mode::History => components::history::render_history_mode(frame, app, area),
        Mode::Code => crate::code_editor::render_editor(frame, app, area),
        Mode::Logs => components::logs::render_logs_mode(frame, app, area),
    }
}

fn draw_input_bar(frame: &mut Frame, app: &App, area: Rect) {
    let (prompt, buffer) = match &app.input_mode {
        InputMode::Input { prompt, buffer, .. } => (prompt.as_str(), buffer.as_str()),
        InputMode::Confirm { prompt, .. } => (prompt.as_str(), ""),
        InputMode::Normal => return,
    };

    let line = Line::from(vec![
        Span::styled(prompt, Style::default().fg(theme::PEACH()).add_modifier(Modifier::BOLD)),
        Span::styled(buffer, Style::default().fg(theme::TEXT())),
        Span::styled("\u{2588}", Style::default().fg(theme::BLUE())),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::BG_SURFACE())),
        area,
    );

    // Autocomplete dropdown
    if !app.autocomplete_suggestions.is_empty() && app.autocomplete_idx.is_some() {
        let max_show = 8usize;
        let count = app.autocomplete_suggestions.len().min(max_show);
        let popup_height = count as u16 + 2; // +2 for borders
        let popup_width = area.width.min(60);
        let popup_y = area.y.saturating_sub(popup_height);
        let popup_area = Rect::new(area.x, popup_y, popup_width, popup_height);

        frame.render_widget(ratatui::widgets::Clear, popup_area);

        let selected = app.autocomplete_idx.unwrap_or(0);
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, suggestion) in app.autocomplete_suggestions.iter().take(max_show).enumerate() {
            let is_dir = suggestion.ends_with('/');
            let icon = if is_dir { "\u{1f4c1} " } else { "\u{1f4c4} " };
            let style = if i == selected {
                Style::default().fg(theme::BG_DARK()).bg(theme::BLUE()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT()).bg(theme::BG_OVERLAY())
            };
            lines.push(Line::from(Span::styled(format!("{}{}", icon, suggestion), style)));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BLUE()))
            .style(Style::default().bg(theme::BG_OVERLAY()));
        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, popup_area);
    }
}

fn draw_files_mode(frame: &mut Frame, app: &App, area: Rect) {
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(75),
        ])
        .split(area);

    components::sidebar::render_sidebar(frame, app, h_chunks[0]);

    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(h_chunks[1]);

    // Show builder panel (top) and response (bottom)
    components::builder::render_builder(frame, app, v_chunks[0]);
    draw_response_view(frame, app, v_chunks[1]);
}

fn draw_response_view(frame: &mut Frame, app: &App, area: Rect) {
    crate::components::response::render_response(frame, app, area);
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

    let w = area.width.saturating_sub(4);
    let h = area.height.saturating_sub(4);
    if w < 10 || h < 3 { return; }
    let popup_width = w.min(100);
    let popup_height = h.min(40);
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let mut lines = Vec::new();

    // Request info
    let method_color = match entry.method.as_str() {
        "GET" => theme::GREEN(),
        "POST" => theme::BLUE(),
        "PUT" => theme::YELLOW(),
        "PATCH" => theme::PINK(),
        "DELETE" => theme::RED(),
        _ => theme::TEXT(),
    };

    lines.push(Line::from(vec![
        Span::styled(&entry.method, Style::default().fg(method_color).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(&entry.url, Style::default().fg(theme::TEXT())),
    ]));

    let status_color = if entry.status < 300 { theme::GREEN() } else if entry.status < 400 { theme::YELLOW() } else { theme::RED() };
    lines.push(Line::from(vec![
        Span::styled(format!("Status: {}", entry.status), Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  \u{00b7} {}ms  \u{00b7} {} bytes", entry.response_time_ms, entry.response_size_bytes), Style::default().fg(theme::TEXT_DIM())),
    ]));
    lines.push(Line::from(Span::styled(format!("Time: {}", entry.timestamp), Style::default().fg(theme::TEXT_FAINT()))));

    // Request headers
    if !entry.request_headers.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Request Headers \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT()))));
        for (k, v) in &entry.request_headers {
            lines.push(Line::from(vec![
                Span::styled(k, Style::default().fg(theme::LAVENDER())),
                Span::raw(": "),
                Span::styled(v, Style::default().fg(theme::TEXT_DIM())),
            ]));
        }
    }

    // Request body
    if let Some(ref body) = entry.request_body {
        if !body.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Request Body \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT()))));
            for line in body.lines().take(10) {
                lines.push(Line::from(Span::styled(line, Style::default().fg(theme::TEXT()))));
            }
        }
    }

    // Response headers
    if !entry.response_headers.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Response Headers \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT()))));
        for (k, v) in &entry.response_headers {
            lines.push(Line::from(vec![
                Span::styled(k, Style::default().fg(theme::LAVENDER())),
                Span::raw(": "),
                Span::styled(v, Style::default().fg(theme::TEXT_DIM())),
            ]));
        }
    }

    // Response body
    if let Some(ref body) = entry.response_body {
        if !body.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("\u{2500}\u{2500}\u{2500} Response Body \u{2500}\u{2500}\u{2500}", Style::default().fg(theme::TEXT_FAINT()))));
            let preview = if body.len() > 1500 { &body[..1500] } else { body.as_str() };
            for line in preview.lines().take(20) {
                lines.push(Line::from(Span::styled(line, Style::default().fg(theme::TEXT()))));
            }
        }
    }

    let block = Block::default()
        .title(" \u{1f4cb} Request Detail (Esc to close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_OVERLAY()));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup_area);
}

fn get_keyhints(app: &App) -> String {
    if app.show_help {
        return " Esc=close ".to_string();
    }
    if !matches!(app.input_mode, InputMode::Normal) {
        return " Type...  Tab=complete  Enter=submit  Esc=cancel ".to_string();
    }
    match app.mode {
        Mode::Files => match app.focus {
            Focus::FileTree | Focus::CodeView => {
                if app.sidebar_tab == SidebarTab::Variables {
                    " \u{2191}\u{2193}=nav  a=add  e=edit  d=delete  V=files  Esc=quit ".to_string()
                } else {
                    " \u{2191}\u{2193}=nav  Enter=select  e=code  o=open  r=run  R=RunAll  t=toggle  V=vars  Esc=quit ".to_string()
                }
            }
            Focus::Variables => " \u{2191}\u{2193}=nav  a=add  e=edit  d=delete  V=files  Esc=quit ".to_string(),
            Focus::Builder => " Tab=cycle  \u{2190}\u{2192}=method  Enter=edit  Ctrl+Enter=send  Esc=back ".to_string(),
            Focus::Response => " \u{2191}\u{2193}=nav  Enter=expand  E=all  C=collapse  b=body  h=hdrs  y=copy  Esc=back ".to_string(),
            _ => " q=quit  Tab=focus  r=run  ?=help ".to_string(),
        },
        Mode::History => " \u{2191}\u{2193}=nav  g=group  m=method  s=status  Enter=detail  Space=compare  Esc=files ".to_string(),
        Mode::Code => " Type to edit  Ctrl+S=save  Ctrl+C/X/V=copy/cut/paste  Ctrl+A=select all  Shift+Arrow=select  /=search  Esc=exit ".to_string(),
        Mode::Logs => " \u{2191}\u{2193}=scroll  f=filter  c=clear  Esc=files ".to_string(),
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![];

    if app.is_running {
        let spinner = app.spinner_char();
        if app.progress_total > 0 {
            spans.push(Span::styled(
                format!(" {} Running {}/{}... ", spinner, app.progress_current, app.progress_total),
                Style::default().fg(theme::SKY()).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} Running... ", spinner),
                Style::default().fg(theme::SKY()).add_modifier(Modifier::BOLD),
            ));
        }
    } else if let Some(fi) = app.active_file_idx {
        if let Some(file) = app.loaded_files.get(fi) {
            if let Some(ref r) = file.results {
                if r.passed > 0 {
                    spans.push(Span::styled(
                        format!(" \u{2713} {} passed ", r.passed),
                        Style::default().fg(theme::GREEN()).add_modifier(Modifier::BOLD),
                    ));
                }
                if r.failed > 0 {
                    spans.push(Span::styled(
                        format!(" \u{2717} {} failed ", r.failed),
                        Style::default().fg(theme::RED()).add_modifier(Modifier::BOLD),
                    ));
                }
                if r.skipped > 0 {
                    spans.push(Span::styled(
                        format!(" \u{2298} {} skipped ", r.skipped),
                        Style::default().fg(theme::YELLOW()),
                    ));
                }
                spans.push(Span::styled(
                    format!(" \u{00b7} {}ms ", r.total_time_ms),
                    Style::default().fg(theme::TEXT_FAINT()),
                ));
            }
        }
    }

    if let Some((ref msg, _)) = app.status_message {
        if spans.is_empty() {
            let hints_len = get_keyhints(app).len();
            let max_msg = (area.width as usize).saturating_sub(hints_len + 4);
            let display = truncate_to(msg, max_msg);
            spans.push(Span::styled(format!(" {} ", display), Style::default().fg(theme::TEXT_DIM())));
        }
    }

    let hints = get_keyhints(app);
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let available = area.width as usize;
    if available > used + hints.len() {
        let pad = available - used - hints.len();
        spans.push(Span::raw(" ".repeat(pad)));
    }
    spans.push(Span::styled(hints.as_str(), Style::default().fg(theme::TEXT_FAINT())));

    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::BG_BASE())),
        area,
    );
}

/// Compact 4-line progress strip at the bottom — always visible when tests are running or have results.
fn render_progress_strip(frame: &mut Frame, app: &App, area: Rect) {
    if area.height < 2 || area.width < 10 { return; }

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme::BLUE()))
        .title(" ⟳ Test Progress ")
        .title_style(Style::default().fg(theme::SKY()).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(theme::BG_SURFACE()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 { return; }
    let max_lines = inner.height as usize;

    let mut lines: Vec<Line> = Vec::new();

    // Line 1: Progress bar or summary — combined with hint
    let hint = if app.progress_expanded { "E=collapse" } else { "E=expand ↑↓=scroll X=dismiss" };
    if app.is_running && app.progress_total > 0 {
        let pct = app.progress_current as f64 / app.progress_total as f64;
        let bar_w = inner.width.saturating_sub(30) as usize;
        let filled = (pct * bar_w as f64) as usize;
        let empty = bar_w.saturating_sub(filled);
        let elapsed = app.run_start_time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(empty)),
                Style::default().fg(theme::GREEN()),
            ),
            Span::styled(
                format!(" {}/{} · {}s ", app.progress_current, app.progress_total, elapsed),
                Style::default().fg(theme::TEXT_DIM()),
            ),
            Span::styled(hint, Style::default().fg(theme::TEXT_FAINT())),
        ]));
    } else if app.is_running {
        let spinner = app.spinner_char();
        let elapsed = app.run_start_time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} Preparing... · {}s  ", spinner, elapsed),
                Style::default().fg(theme::SKY()),
            ),
            Span::styled(hint, Style::default().fg(theme::TEXT_FAINT())),
        ]));
    } else {
        let passed = app.run_progress_lines.iter().filter(|(i, _)| i == "\u{2713}").count();
        let failed = app.run_progress_lines.iter().filter(|(i, _)| i == "\u{2717}").count();
        let total = app.run_progress_lines.len();
        lines.push(Line::from(vec![
            Span::styled("Results: ", Style::default().fg(theme::TEXT_DIM())),
            Span::styled(format!("✓{} ", passed), Style::default().fg(theme::GREEN())),
            Span::styled(format!("✗{} ", failed), Style::default().fg(if failed > 0 { theme::RED() } else { theme::TEXT_DIM() })),
            Span::styled(format!("({} total)  ", total), Style::default().fg(theme::TEXT_FAINT())),
            Span::styled(hint, Style::default().fg(theme::TEXT_FAINT())),
        ]));
    }

    // Remaining rows for progress entries
    let entry_rows = max_lines.saturating_sub(lines.len());
    if entry_rows > 0 && !app.run_progress_lines.is_empty() {
        let total_entries = app.run_progress_lines.len();
        let max_scroll = total_entries.saturating_sub(entry_rows);
        let scroll = app.progress_scroll.min(max_scroll);
        for (icon, name) in app.run_progress_lines.iter().skip(scroll).take(entry_rows) {
            let color = match icon.as_str() {
                "\u{2713}" => theme::GREEN(),
                "\u{2717}" => theme::RED(),
                _ => theme::YELLOW(),
            };
            let max_name = inner.width.saturating_sub(5) as usize;
            lines.push(Line::from(vec![
                Span::styled(format!(" {} ", icon), Style::default().fg(color)),
                Span::styled(truncate_to(name, max_name), Style::default().fg(theme::TEXT())),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::BG_SURFACE())),
        inner,
    );
}

/// Expanded progress overlay — larger window with full scrollable list.
fn render_progress_expanded(frame: &mut Frame, app: &App, area: Rect) {
    if area.width < 30 || area.height < 10 { return; }
    let width = (area.width * 3 / 4).max(50).min(area.width);
    let height = (area.height * 3 / 4).max(15).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::SKY()))
        .title(" ⟳ Test Progress (Expanded) ")
        .title_style(Style::default().fg(theme::SKY()).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(theme::BG_OVERLAY()));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height == 0 || inner.width == 0 { return; }

    let mut lines: Vec<Line> = Vec::new();

    // Progress bar
    if app.is_running && app.progress_total > 0 {
        let pct = app.progress_current as f64 / app.progress_total as f64;
        let bar_w = inner.width.saturating_sub(20) as usize;
        let filled = (pct * bar_w as f64) as usize;
        let empty = bar_w.saturating_sub(filled);
        let elapsed = app.run_start_time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}{} ", "\u{2588}".repeat(filled), "\u{2591}".repeat(empty)),
                Style::default().fg(theme::GREEN()),
            ),
            Span::styled(
                format!("{}/{} · {}s", app.progress_current, app.progress_total, elapsed),
                Style::default().fg(theme::TEXT_DIM()),
            ),
        ]));
        lines.push(Line::from(""));
    }

    // All entries with scroll
    let list_height = inner.height.saturating_sub(lines.len() as u16 + 2) as usize;
    let total = app.run_progress_lines.len();
    let max_scroll = total.saturating_sub(list_height);
    let scroll = app.progress_scroll.min(max_scroll);
    let visible_entries: Vec<_> = app.run_progress_lines.iter().enumerate().skip(scroll).take(list_height).collect();
    for (idx, (icon, name)) in visible_entries {
        let color = match icon.as_str() {
            "\u{2713}" => theme::GREEN(),
            "\u{2717}" => theme::RED(),
            _ => theme::YELLOW(),
        };
        let selected = idx == scroll + app.progress_scroll.min(total.saturating_sub(1));
        let prefix = if selected && app.progress_expanded { "▸" } else { " " };
        let max_name = inner.width.saturating_sub(8) as usize;
        lines.push(Line::from(vec![
            Span::styled(format!("{} {} ", prefix, icon), Style::default().fg(color)),
            Span::styled(truncate_to(name, max_name), Style::default().fg(theme::TEXT())),
        ]));
    }

    // Scroll indicator
    if total > list_height {
        lines.push(Line::from(Span::styled(
            format!(" [{}-{} of {}]", scroll + 1, (scroll + list_height).min(total), total),
            Style::default().fg(theme::TEXT_FAINT()),
        )));
    }

    lines.push(Line::from(Span::styled(
        " ↑↓=scroll  Enter=select  E=collapse  Esc=close ",
        Style::default().fg(theme::TEXT_FAINT()),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::BG_OVERLAY())),
        inner,
    );
}