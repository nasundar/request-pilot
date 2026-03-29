use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use crate::ui::theme;

/// Render the help overlay popup (triggered by `?`).
pub fn render_help_popup(frame: &mut Frame, area: Rect, scroll: u16) {
    let w = area.width.saturating_sub(4);
    let h = area.height.saturating_sub(4);
    if w < 10 || h < 3 { return; }
    let popup_width = 64.min(w);
    let popup_height = 70.min(h);
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let kb = Style::default().fg(theme::PEACH());
    let dim = Style::default().fg(theme::TEXT_FAINT());

    let help_text = vec![
        Line::from(Span::styled("Keybindings", Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("Pattern: lowercase=context action  Ctrl+=system/toolbar  Shift/Upper=mode switch", dim)),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Navigation (everywhere) \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("j/k \u{2191}\u{2193}       ", kb), Span::styled("Navigate / scroll", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("g / G         ", kb), Span::styled("Jump to top / bottom", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+U/D      ", kb), Span::styled("Half-page up / down", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Tab           ", kb), Span::styled("Cycle focus", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Enter         ", kb), Span::styled("Select / expand / detail", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Esc           ", kb), Span::styled("Back / close popup", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} File Tree \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("r             ", kb), Span::styled("Run selected (block/group/file)", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("R             ", kb), Span::styled("Run all tests", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("t             ", kb), Span::styled("Toggle block enabled/disabled", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("i             ", kb), Span::styled("Inspect details (file/group/block)", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("e             ", kb), Span::styled("Jump to code editor", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("1-9           ", kb), Span::styled("Quick jump to file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Space         ", kb), Span::styled("Toggle expand", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} File Operations (Ctrl+) \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("o / Ctrl+O    ", kb), Span::styled("Open file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("n / Ctrl+N    ", kb), Span::styled("New file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("s / Ctrl+S    ", kb), Span::styled("Save file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("x             ", kb), Span::styled("Close file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+E        ", kb), Span::styled("Load .env file", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Variables (V to switch tab) \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("V             ", kb), Span::styled("Toggle sidebar tab (files/vars)", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("a             ", kb), Span::styled("Add variable", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("e             ", kb), Span::styled("Edit variable value", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("d             ", kb), Span::styled("Delete variable", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Enter / i     ", kb), Span::styled("Inspect variable (JWT, URL, JSON decode)", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Response Panel \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("b / h / a     ", kb), Span::styled("Body / Headers / Assertions tab", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("y             ", kb), Span::styled("Copy response body", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("D             ", kb), Span::styled("View compare diff", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("E / C         ", kb), Span::styled("Expand / Collapse all JSON nodes", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Code Editor \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("c             ", kb), Span::styled("Open code editor (read-only view)", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("i / e         ", kb), Span::styled("Enter edit mode (in code editor)", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Esc           ", kb), Span::styled("Exit edit mode → view mode → files", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("/             ", kb), Span::styled("Search in file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("n / N         ", kb), Span::styled("Next / prev search match", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+G        ", kb), Span::styled("Go to line number", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+C/X/V/A  ", kb), Span::styled("Copy / Cut / Paste / Select all", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Mode Switches \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("f             ", kb), Span::styled("Files mode", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("c             ", kb), Span::styled("Code editor", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("h             ", kb), Span::styled("History mode", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("l             ", kb), Span::styled("Logs viewer", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} History Mode \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("g             ", kb), Span::styled("Change grouping", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("m / s         ", kb), Span::styled("Filter by method / status", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("/             ", kb), Span::styled("Filter history", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Space         ", kb), Span::styled("Select entries for compare", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("d             ", kb), Span::styled("Diff selected (needs 2)", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("e             ", kb), Span::styled("Export history", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Toolbar (Ctrl+) \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("Ctrl+R        ", kb), Span::styled("Auto-run interval", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+T        ", kb), Span::styled("OTEL telemetry popup", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+A        ", kb), Span::styled("Azure auth popup", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+L        ", kb), Span::styled("Extension connector", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+H        ", kb), Span::styled("Extra headers popup", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} General \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("T             ", kb), Span::styled("Cycle theme", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("?             ", kb), Span::styled("Toggle help", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("q             ", kb), Span::styled("Quit", Style::default().fg(theme::TEXT()))]),
    ];

    let block = Block::default()
        .title(" \u{2708} Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BLUE()))
        .style(Style::default().bg(theme::BG_OVERLAY()));

    let paragraph = Paragraph::new(help_text)
        .block(block)
        .scroll((scroll, 0));
    frame.render_widget(paragraph, popup_area);

    // Footer key hints
    let hint_y = popup_area.y + popup_area.height - 1;
    if hint_y < area.height {
        let hint_area = Rect::new(popup_area.x + 2, hint_y, popup_area.width.saturating_sub(4), 1);
        let hint = Paragraph::new(Line::from(vec![
            Span::styled("↑↓", Style::default().fg(theme::PEACH())),
            Span::styled("=scroll ", Style::default().fg(theme::TEXT_FAINT())),
            Span::styled("?/Esc", Style::default().fg(theme::PEACH())),
            Span::styled("=close", Style::default().fg(theme::TEXT_FAINT())),
        ]));
        frame.render_widget(hint, hint_area);
    }
}
