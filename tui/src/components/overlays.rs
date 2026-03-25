use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use crate::ui::theme;

/// Render the help overlay popup (triggered by `?`).
pub fn render_help_popup(frame: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(4);
    let h = area.height.saturating_sub(4);
    if w < 10 || h < 3 { return; }
    let popup_width = 60.min(w);
    let popup_height = 38.min(h);
    let x = (area.width - popup_width) / 2;
    let y = (area.height - popup_height) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let kb = Style::default().fg(theme::PEACH());
    let dim = Style::default().fg(theme::TEXT_FAINT());

    let help_text = vec![
        Line::from(Span::styled("Keybindings", Style::default().fg(theme::BLUE()).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Navigation \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("j/k \u{2191}\u{2193}       ", kb), Span::styled("Navigate / scroll", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("g / G         ", kb), Span::styled("Jump to top / bottom", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+u/d      ", kb), Span::styled("Half-page up / down", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("1-9           ", kb), Span::styled("Quick jump to file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Tab           ", kb), Span::styled("Cycle focus", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Enter         ", kb), Span::styled("Select / expand / detail", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Space         ", kb), Span::styled("Toggle expand", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} File Operations \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("o / Ctrl+O    ", kb), Span::styled("Open file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("n / Ctrl+N    ", kb), Span::styled("New file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("s / Ctrl+S    ", kb), Span::styled("Save file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("x             ", kb), Span::styled("Close file", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Ctrl+E        ", kb), Span::styled("Load .env file", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Execution \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("r             ", kb), Span::styled("Run selected (block/group/file)", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("R             ", kb), Span::styled("Run all tests", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("t             ", kb), Span::styled("Toggle block enabled/disabled", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Variables \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("Ctrl+V        ", kb), Span::styled("Toggle sidebar tab", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("a             ", kb), Span::styled("Add variable", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("e             ", kb), Span::styled("Edit variable", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("d             ", kb), Span::styled("Delete variable", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Response \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("b / h / a     ", kb), Span::styled("Body / Headers / Assertions tab", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("y             ", kb), Span::styled("Copy response body", Style::default().fg(theme::TEXT()))]),
        Line::from(""),
        Line::from(Span::styled("\u{2500}\u{2500} Modes & History \u{2500}\u{2500}", dim)),
        Line::from(vec![Span::styled("f             ", kb), Span::styled("Files mode", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("h             ", kb), Span::styled("History mode", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("/             ", kb), Span::styled("Filter history", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("C             ", kb), Span::styled("Clear history", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("E             ", kb), Span::styled("Export history", Style::default().fg(theme::TEXT()))]),
        Line::from(vec![Span::styled("Enter         ", kb), Span::styled("View details", Style::default().fg(theme::TEXT()))]),
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

    let paragraph = Paragraph::new(help_text).block(block);
    frame.render_widget(paragraph, popup_area);
}
