use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, Focus, SidebarTab, TreeNode, InputMode, InputPurpose, ConfirmPurpose, LoadedFile};
use crate::ui::theme;
use crate::ui::truncate_to;

// ΓöÇΓöÇ Rendering ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

pub fn render_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = matches!(app.focus, Focus::FileTree | Focus::Variables);
    let border_style = if is_focused {
        Style::default().fg(theme::BLUE())
    } else {
        Style::default().fg(theme::TEXT_FAINT())
    };

    let title = match app.sidebar_tab {
        SidebarTab::Files => " \u{1f4c1} Files ",
        SidebarTab::Variables => " \u{1f510} Variables ",
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(if is_focused {
            Style::default().bg(theme::BG_FOCUS())
        } else {
            Style::default()
        });

    match app.sidebar_tab {
        SidebarTab::Files => {
            if app.tree_nodes.is_empty() {
                let msg = Paragraph::new("No files loaded.\nPress 'o' to open a file.")
                    .block(block)
                    .style(Style::default().fg(theme::TEXT_FAINT()));
                frame.render_widget(msg, area);
                return;
            }

            let items: Vec<ListItem> = app.tree_nodes.iter().enumerate().map(|(i, node)| {
                let (prefix, label, style) = match node {
                    TreeNode::File { file_idx } => {
                        let f = &app.loaded_files[*file_idx];
                        let chevron = if f.expanded { "\u{25bc}" } else { "\u{25b6}" };
                        (format!("{} \u{1f4c4} ", chevron), f.name.clone(), Style::default().fg(theme::TEXT()).add_modifier(Modifier::BOLD))
                    }
                    TreeNode::Group { file_idx, group_name } => {
                        let f = &app.loaded_files[*file_idx];
                        let expanded = f.group_expanded.get(group_name).copied().unwrap_or(true);
                        let chevron = if expanded { "\u{25bc}" } else { "\u{25b6}" };
                        (format!("  {} ", chevron), group_name.clone(), Style::default().fg(theme::SAPPHIRE()))
                    }
                    TreeNode::Block { file_idx, block_idx } => {
                        let f = &app.loaded_files[*file_idx];
                        let blk = &f.suite.blocks[*block_idx];
                        let icon = if blk.compare {
                            "\u{21C4}" // ⇄ for compare blocks
                        } else {
                            match blk.block_type.as_str() {
                                "setup" => "\u{2699}",
                                "test" => "\u{1f9ea}",
                                "teardown" => "\u{1f5d1}",
                                _ => "\u{1f4e4}",
                            }
                        };
                        let status = get_block_status(f, *block_idx);
                        // Show @mode app blocks as dimmed/skipped when dev mode is on
                        let dev_skip = app.dev_mode && blk.mode.as_deref() == Some("app");
                        let disabled_mark = if blk.disabled { " \u{d8}" } else if dev_skip { " \u{d8}" } else { "" };
                        let indent = if blk.group.is_some() { "      " } else { "    " };
                        let style = if dev_skip || blk.disabled {
                            Style::default().fg(theme::TEXT_FAINT())
                        } else {
                            Style::default().fg(theme::TEXT())
                        };
                        (format!("{}{} {} ", indent, status, icon), format!("{}{}", blk.name, disabled_mark), style)
                    }
                };

                let is_selected = i == app.tree_cursor;
                let final_style = if is_selected {
                    style.bg(theme::BG_HIGHLIGHT())
                } else {
                    style
                };
                let prefix_chars: usize = prefix.chars().count();
                let max_label = (area.width as usize).saturating_sub(prefix_chars + 3);
                let display_label = truncate_to(&label, max_label);
                ListItem::new(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(display_label, final_style),
                ]))
            }).collect();

            // Compute scroll offset to keep tree_cursor visible
            let visible_h = area.height.saturating_sub(2) as usize; // minus borders
            let scroll = if visible_h > 0 && app.tree_cursor >= visible_h {
                app.tree_cursor - visible_h + 1
            } else {
                0
            };
            let visible_items: Vec<ListItem> = items.into_iter().skip(scroll).collect();

            let list = List::new(visible_items).block(block);
            frame.render_widget(list, area);
        }
        SidebarTab::Variables => {
            let mut extract_vars: std::collections::HashSet<String> = std::collections::HashSet::new();
            for file in &app.loaded_files {
                for blk in &file.suite.blocks {
                    for e in &blk.extracts {
                        extract_vars.insert(e.variable_name.clone());
                    }
                }
            }

            let sorted_vars = app.get_sorted_var_names();

            let items: Vec<ListItem> = sorted_vars.iter().enumerate().map(|(i, k)| {
                let v = app.env_vars.get(k).map(|s| s.as_str()).unwrap_or("");
                let name_color = if extract_vars.contains(k.as_str()) {
                    theme::MAUVE()
                } else {
                    theme::SAPPHIRE()
                };
                let prefix = if extract_vars.contains(k.as_str()) { "\u{21d0} " } else { "  " };
                let is_selected = i == app.vars_cursor;
                let bg = if is_selected { theme::BG_HIGHLIGHT() } else { ratatui::style::Color::Reset };
                ListItem::new(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(name_color).bg(bg)),
                    Span::styled(k, Style::default().fg(name_color).bg(bg)),
                    Span::styled(" = ", Style::default().fg(theme::TEXT_FAINT()).bg(bg)),
                    Span::styled(v, Style::default().fg(theme::TEXT()).bg(bg)),
                ]))
            }).collect();

            let scroll = app.vars_scroll as usize;
            let visible_items: Vec<ListItem> = items.into_iter().skip(scroll).collect();

            let list = List::new(visible_items).block(block);
            frame.render_widget(list, area);
        }
    }
}

pub fn get_block_status(file: &LoadedFile, block_idx: usize) -> &'static str {
    if let Some(ref results) = file.results {
        let block_name = &file.suite.blocks[block_idx].name;
        if let Some(br) = find_block_result(&results.block_results, block_name) {
            return match br.status.as_str() {
                "passed" => "\u{2713}",
                "failed" | "error" => "\u{2717}",
                "skipped" => "\u{2298}",
                "running" => "\u{2800}",
                _ => "\u{00b7}",
            };
        }
    }
    "\u{00b7}"
}

/// Look up a block result by name (avoids index mismatch when mode-filtered blocks are skipped).
pub fn find_block_result<'a>(
    block_results: &'a [request_pilot_core::test_runner::BlockResult],
    name: &str,
) -> Option<&'a request_pilot_core::test_runner::BlockResult> {
    block_results.iter().find(|br| br.name == name)
}

// ΓöÇΓöÇ Event handling ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

pub fn handle_file_tree_keys(app: &mut App, key: KeyEvent) {
    if app.tree_nodes.is_empty() {
        match key.code {
            KeyCode::Char('o') => {
                app.input_mode = InputMode::Input {
                    prompt: "Open file: ".to_string(),
                    purpose: InputPurpose::OpenFile,
                    buffer: String::new(),
                };
            }
            KeyCode::Char('n') => {
                app.new_file();
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.tree_cursor > 0 {
                app.tree_cursor -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.tree_cursor < app.tree_nodes.len().saturating_sub(1) {
                app.tree_cursor += 1;
            }
        }
        KeyCode::Char('g') => {
            app.tree_cursor = 0;
        }
        KeyCode::Char('G') => {
            app.tree_cursor = app.tree_nodes.len().saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.tree_cursor = (app.tree_cursor + 10).min(app.tree_nodes.len().saturating_sub(1));
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.tree_cursor = app.tree_cursor.saturating_sub(10);
        }
        KeyCode::Char('r') => {
            let node = app.tree_nodes[app.tree_cursor].clone();
            match node {
                TreeNode::Block { file_idx, block_idx } => {
                    app.select_block(file_idx, block_idx);
                    app.queue_run_single(file_idx, block_idx);
                }
                TreeNode::Group { file_idx, group_name } => {
                    let indices: Vec<usize> = app.loaded_files.get(file_idx)
                        .map(|f| f.suite.blocks.iter().enumerate()
                            .filter(|(_, b)| b.group.as_deref() == Some(&group_name))
                            .map(|(i, _)| i)
                            .collect())
                        .unwrap_or_default();
                    app.active_file_idx = Some(file_idx);
                    app.queue_run_group(file_idx, indices);
                }
                TreeNode::File { file_idx } => {
                    app.active_file_idx = Some(file_idx);
                    app.queue_run_all();
                }
            }
        }
        KeyCode::Char('t') => {
            let node = app.tree_nodes[app.tree_cursor].clone();
            if let TreeNode::Block { file_idx, block_idx } = node {
                app.select_block(file_idx, block_idx);
                app.toggle_block_disabled();
            }
        }
        KeyCode::Char('o') => {
            app.input_mode = InputMode::Input {
                prompt: "Open file: ".to_string(),
                purpose: InputPurpose::OpenFile,
                buffer: String::new(),
            };
        }
        KeyCode::Char('n') => {
            app.new_file();
        }
        KeyCode::Char('s') => {
            app.save_current_file();
        }
        KeyCode::Char('x') => {
            let node = app.tree_nodes[app.tree_cursor].clone();
            let file_idx = match node {
                TreeNode::File { file_idx } => file_idx,
                TreeNode::Group { file_idx, .. } => file_idx,
                TreeNode::Block { file_idx, .. } => file_idx,
            };
            app.input_mode = InputMode::Confirm {
                prompt: format!("Close file '{}'? (y/n)", app.loaded_files[file_idx].name),
                purpose: ConfirmPurpose::CloseFile { file_idx },
            };
        }
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let idx = (c as u8 - b'1') as usize;
            if idx < app.loaded_files.len() {
                for (ti, node) in app.tree_nodes.iter().enumerate() {
                    if let TreeNode::File { file_idx } = node {
                        if *file_idx == idx {
                            app.tree_cursor = ti;
                            app.active_file_idx = Some(idx);
                            if app.loaded_files[idx].suite.blocks.first().is_some() {
                                app.active_block_idx = Some(0);
                            }
                            break;
                        }
                    }
                }
            }
        }
        KeyCode::Enter => {
            let node = app.tree_nodes[app.tree_cursor].clone();
            match node {
                TreeNode::File { file_idx } => {
                    app.loaded_files[file_idx].expanded = !app.loaded_files[file_idx].expanded;
                    app.rebuild_tree();
                }
                TreeNode::Group { file_idx, group_name } => {
                    let expanded = app.loaded_files[file_idx]
                        .group_expanded
                        .entry(group_name)
                        .or_insert(true);
                    *expanded = !*expanded;
                    app.rebuild_tree();
                }
                TreeNode::Block { file_idx, block_idx } => {
                    app.select_block(file_idx, block_idx);
                    app.focus = Focus::CodeView;
                }
            }
        }
        KeyCode::Char(' ') => {
            let node = app.tree_nodes[app.tree_cursor].clone();
            match node {
                TreeNode::File { file_idx } => {
                    app.loaded_files[file_idx].expanded = !app.loaded_files[file_idx].expanded;
                    app.rebuild_tree();
                }
                TreeNode::Group { file_idx, group_name } => {
                    let expanded = app.loaded_files[file_idx]
                        .group_expanded
                        .entry(group_name)
                        .or_insert(true);
                    *expanded = !*expanded;
                    app.rebuild_tree();
                }
                _ => {}
            }
        }
        KeyCode::Char('e') => {
            // Jump to code editor at the selected block
            let node = app.tree_nodes[app.tree_cursor].clone();
            match node {
                TreeNode::Block { file_idx, block_idx } => {
                    app.select_block(file_idx, block_idx);
                    app.enter_code_editor_at_block(block_idx);
                }
                TreeNode::File { file_idx } => {
                    app.active_file_idx = Some(file_idx);
                    app.enter_code_editor();
                }
                TreeNode::Group { file_idx, .. } => {
                    app.active_file_idx = Some(file_idx);
                    app.enter_code_editor();
                }
            }
        }
        _ => {}
    }
}

pub fn handle_variables_keys(app: &mut App, key: KeyEvent) {
    let var_count = app.env_vars.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if var_count > 0 && app.vars_cursor < var_count.saturating_sub(1) {
                app.vars_cursor += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.vars_cursor = app.vars_cursor.saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.vars_cursor = (app.vars_cursor + 10).min(var_count.saturating_sub(1));
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.vars_cursor = app.vars_cursor.saturating_sub(10);
        }
        KeyCode::Char('a') => {
            app.input_mode = InputMode::Input {
                prompt: "Variable name: ".to_string(),
                purpose: InputPurpose::AddVarName,
                buffer: String::new(),
            };
        }
        KeyCode::Char('e') => {
            let names = app.get_sorted_var_names();
            if let Some(name) = names.get(app.vars_cursor) {
                let current_val = app.env_vars.get(name).cloned().unwrap_or_default();
                app.input_mode = InputMode::Input {
                    prompt: format!("Edit '{}': ", name),
                    purpose: InputPurpose::EditVarValue { name: name.clone() },
                    buffer: current_val,
                };
            }
        }
        KeyCode::Char('d') | KeyCode::Char('x') => {
            let names = app.get_sorted_var_names();
            if let Some(name) = names.get(app.vars_cursor) {
                app.input_mode = InputMode::Confirm {
                    prompt: format!("Delete variable '{}'? (y/n)", name),
                    purpose: ConfirmPurpose::DeleteVar { name: name.clone() },
                };
            }
        }
        _ => {}
    }
}
