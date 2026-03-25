use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, Focus, Mode, SidebarTab, TreeNode, InputMode, InputPurpose, ConfirmPurpose};
use crate::toolbar;

pub fn handle_key(app: &mut App, key: KeyEvent) {
    // Code editor mode priority
    if app.mode == Mode::Code {
        crate::code_editor::handle_editor_keys(app, key);
        return;
    }

    // 1. Input mode handling takes priority
    match &app.input_mode {
        InputMode::Input { .. } => {
            handle_input_mode(app, key);
            return;
        }
        InputMode::Confirm { .. } => {
            handle_confirm_mode(app, key);
            return;
        }
        InputMode::Normal => {}
    }

    // 2. Help overlay consumes all keys
    if app.show_help {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
            app.show_help = false;
        }
        return;
    }

    // 3. Toolbar popup key consumption
    if toolbar::handle_extra_headers_keys(app, key) { return; }
    if toolbar::handle_azure_keys(app, key) { return; }
    if toolbar::handle_otel_keys(app, key) { return; }

    // 4. History detail overlay
    if app.history_detail_idx.is_some() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Backspace) {
            app.history_detail_idx = None;
        }
        return;
    }

    // 4. Global keybinds
    let in_text_input = matches!(app.focus, Focus::FilterInput);

    match key.code {
        KeyCode::Char('q') if !in_text_input => {
            app.should_quit = true;
            return;
        }
        KeyCode::Char('?') if !in_text_input => {
            app.show_help = !app.show_help;
            return;
        }
        KeyCode::F(1) => {
            app.mode = Mode::Files;
            app.focus = Focus::FileTree;
            return;
        }
        KeyCode::F(3) => {
            app.mode = Mode::History;
            app.focus = Focus::HistoryList;
            return;
        }
        KeyCode::F(5) => {
            app.queue_run_all();
            return;
        }
        KeyCode::Char('R') if !in_text_input => {
            app.queue_run_all();
            return;
        }
        KeyCode::Char('c') if !in_text_input => {
            app.enter_code_editor();
            return;
        }
        KeyCode::Tab => {
            cycle_focus(app);
            return;
        }
        // File operations (Ctrl variants work everywhere)
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_mode = InputMode::Input {
                prompt: "Open file: ".to_string(),
                purpose: InputPurpose::OpenFile,
                buffer: String::new(),
            };
            return;
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.new_file();
            return;
        }
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.save_current_file();
            return;
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_mode = InputMode::Input {
                prompt: "Load .env file: ".to_string(),
                purpose: InputPurpose::LoadEnv,
                buffer: String::new(),
            };
            return;
        }
        // Toolbar shortcuts
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        _ => {}
    }

    // 6. Mode-specific keybinds
    match app.mode {
        Mode::Files => handle_files_mode(app, key),
        Mode::History => handle_history_mode(app, key),
        Mode::Code => crate::code_editor::handle_editor_keys(app, key),
    }
}

fn handle_input_mode(app: &mut App, key: KeyEvent) {
    let (purpose, buffer) = match &mut app.input_mode {
        InputMode::Input { purpose, buffer, .. } => (purpose.clone(), buffer),
        _ => return,
    };

    match key.code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
        }
        KeyCode::Enter => {
            let value = buffer.clone();
            let p = purpose.clone();
            app.input_mode = InputMode::Normal;
            app.submit_input(p, value);
        }
        KeyCode::Backspace => {
            buffer.pop();
        }
        KeyCode::Char(c) => {
            buffer.push(c);
        }
        _ => {}
    }
}

fn handle_confirm_mode(app: &mut App, key: KeyEvent) {
    let purpose = match &app.input_mode {
        InputMode::Confirm { purpose, .. } => purpose.clone(),
        _ => return,
    };

    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            app.input_mode = InputMode::Normal;
            app.confirm_action(purpose);
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

fn cycle_focus(app: &mut App) {
    match app.focus {
        Focus::FileTree => {
            if app.sidebar_tab == SidebarTab::Variables {
                app.focus = Focus::Variables;
            } else {
                app.focus = Focus::CodeView;
            }
        }
        Focus::Variables => app.focus = Focus::CodeView,
        Focus::CodeView => app.focus = Focus::Response,
        Focus::Response => app.focus = Focus::FileTree,
        Focus::HistoryList => app.focus = Focus::FilterInput,
        Focus::FilterInput => app.focus = Focus::HistoryList,
    }
}

fn handle_files_mode(app: &mut App, key: KeyEvent) {
    // Sidebar tab switching (Ctrl+V)
    if key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.sidebar_tab = match app.sidebar_tab {
            SidebarTab::Files => {
                app.focus = Focus::Variables;
                SidebarTab::Variables
            }
            SidebarTab::Variables => {
                app.focus = Focus::FileTree;
                SidebarTab::Files
            }
        };
        return;
    }

    match app.focus {
        Focus::FileTree => handle_file_tree(app, key),
        Focus::CodeView => handle_code_scroll(app, key),
        Focus::Response => handle_response(app, key),
        Focus::Variables => handle_variables(app, key),
        _ => {}
    }
}

fn handle_code_scroll(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.code_scroll = app.code_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.code_scroll = app.code_scroll.saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.code_scroll = app.code_scroll.saturating_add(10);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.code_scroll = app.code_scroll.saturating_sub(10);
        }
        KeyCode::Char('g') => {
            app.code_scroll = 0;
        }
        KeyCode::Char('G') => {
            app.code_scroll = u16::MAX;
        }
        _ => {}
    }
}

fn handle_response(app: &mut App, key: KeyEvent) {
    crate::components::response::handle_response_keys(app, key);
}

fn handle_variables(app: &mut App, key: KeyEvent) {
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
        // Add variable
        KeyCode::Char('a') => {
            app.input_mode = InputMode::Input {
                prompt: "Variable name: ".to_string(),
                purpose: InputPurpose::AddVarName,
                buffer: String::new(),
            };
        }
        // Edit variable
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
        // Delete variable
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

fn handle_file_tree(app: &mut App, key: KeyEvent) {
    if app.tree_nodes.is_empty() {
        // Allow file operations even with empty tree
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
        // Run selected (block, group, or file)
        KeyCode::Char('r') => {
            let node = app.tree_nodes[app.tree_cursor].clone();
            match node {
                TreeNode::Block { file_idx, block_idx } => {
                    app.select_block(file_idx, block_idx);
                    app.queue_run_single(file_idx, block_idx);
                }
                TreeNode::Group { file_idx, group_name } => {
                    // Collect all block indices in this group
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
        // Toggle block enabled/disabled
        KeyCode::Char('t') => {
            let node = app.tree_nodes[app.tree_cursor].clone();
            if let TreeNode::Block { file_idx, block_idx } = node {
                app.select_block(file_idx, block_idx);
                app.toggle_block_disabled();
            }
        }
        // Open file
        KeyCode::Char('o') => {
            app.input_mode = InputMode::Input {
                prompt: "Open file: ".to_string(),
                purpose: InputPurpose::OpenFile,
                buffer: String::new(),
            };
        }
        // New file
        KeyCode::Char('n') => {
            app.new_file();
        }
        // Save file
        KeyCode::Char('s') => {
            app.save_current_file();
        }
        // Close file
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
        // Quick jump to file by index (1-9)
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
        _ => {}
    }
}

fn handle_history_mode(app: &mut App, key: KeyEvent) {
    match app.focus {
        Focus::HistoryList => {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    app.history_cursor = app.history_cursor.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if app.history_cursor < app.filtered_history_len.saturating_sub(1) {
                        app.history_cursor += 1;
                    }
                }
                KeyCode::Char('g') => {
                    app.history_cursor = 0;
                }
                KeyCode::Char('G') => {
                    app.history_cursor = app.filtered_history_len.saturating_sub(1);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.history_cursor = (app.history_cursor + 10)
                        .min(app.filtered_history_len.saturating_sub(1));
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.history_cursor = app.history_cursor.saturating_sub(10);
                }
                KeyCode::Char('/') => {
                    app.focus = Focus::FilterInput;
                }
                // Clear history
                KeyCode::Char('C') => {
                    app.input_mode = InputMode::Confirm {
                        prompt: "Clear all history? (y/n)".to_string(),
                        purpose: ConfirmPurpose::ClearHistory,
                    };
                }
                // View entry details
                KeyCode::Enter => {
                    if app.filtered_history_len > 0 {
                        app.history_detail_idx = Some(app.history_cursor);
                    }
                }
                // Export history
                KeyCode::Char('E') => {
                    app.input_mode = InputMode::Input {
                        prompt: "Export history to: ".to_string(),
                        purpose: InputPurpose::ExportHistory,
                        buffer: "history.json".to_string(),
                    };
                }
                _ => {}
            }
        }
        Focus::FilterInput => {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    app.focus = Focus::HistoryList;
                }
                KeyCode::Char(c) => {
                    app.filter_text.push(c);
                    app.history_cursor = 0;
                }
                KeyCode::Backspace => {
                    app.filter_text.pop();
                    app.history_cursor = 0;
                }
                _ => {}
            }
        }
        _ => {}
    }
}