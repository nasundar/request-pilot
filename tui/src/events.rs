use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, BuilderFocus, Focus, Mode, SidebarTab, InputMode, InputPurpose};
use crate::toolbar;
use crate::components;

pub fn handle_key(app: &mut App, key: KeyEvent) {
    // Code editor mode priority — in view mode, let mode-switch keys fall through
    if app.mode == Mode::Code {
        if !app.code_editor_editing {
            match key.code {
                KeyCode::Char('f') | KeyCode::Char('h') | KeyCode::Char('l')
                | KeyCode::Char('q') | KeyCode::Char('?')
                | KeyCode::Char('V') | KeyCode::Char('T') | KeyCode::Char('R')
                | KeyCode::Char('D') => {
                    // fall through to global key handling below
                }
                _ => {
                    crate::code_editor::handle_editor_keys(app, key);
                    return;
                }
            }
        } else {
            crate::code_editor::handle_editor_keys(app, key);
            return;
        }
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

    // 1b. Env picker overlay
    if app.env_picker_open {
        let n = app.env_config.entries.len();
        // cursor 0 = (none), 1..=n = entries
        match key.code {
            KeyCode::Esc => { app.env_picker_open = false; }
            KeyCode::Down | KeyCode::Char('j') => {
                app.env_picker_cursor = (app.env_picker_cursor + 1).min(n);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.env_picker_cursor = app.env_picker_cursor.saturating_sub(1);
            }
            KeyCode::Enter => {
                let idx = if app.env_picker_cursor == 0 { None } else { Some(app.env_picker_cursor - 1) };
                app.activate_env_index(idx);
                app.env_picker_open = false;
            }
            KeyCode::Char('d') => {
                if app.env_picker_cursor > 0 {
                    let idx = app.env_picker_cursor - 1;
                    app.remove_env_index(idx);
                    // Clamp cursor to new length
                    let new_n = app.env_config.entries.len();
                    app.env_picker_cursor = app.env_picker_cursor.min(new_n);
                }
            }
            KeyCode::Char('a') => {
                // Open "Load .env" input prompt and close picker.
                app.env_picker_open = false;
                app.input_mode = InputMode::Input {
                    prompt: "Load .env file: ".to_string(),
                    purpose: InputPurpose::LoadEnv,
                    buffer: String::new(),
                };
            }
            _ => {}
        }
        return;
    }

    // 2. Help overlay with scroll
    if app.show_help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                app.show_help = false;
                app.help_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.help_scroll = app.help_scroll.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.help_scroll = app.help_scroll.saturating_add(10);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.help_scroll = app.help_scroll.saturating_sub(10);
            }
            KeyCode::Char('g') => { app.help_scroll = 0; }
            KeyCode::Char('G') => { app.help_scroll = 200; }
            _ => {}
        }
        return;
    }

    // 3. Diff viewer overlay
    if app.diff_viewer_open {
        components::diff_viewer::handle_diff_keys(app, key);
        return;
    }

    // 3a. Snapshot replay confirmation modal — must come before any other
    // overlay/mode handler so y/n/Esc/Enter are routed to the modal even if
    // the underlying file is locked.
    if app.pending_replay.is_some() {
        handle_replay_confirm_keys(app, key);
        return;
    }

    // 3a'. Snapshot detach confirmation modal — same routing rule as the
    // replay modal: it must intercept keys before any lock check or other
    // overlay handler runs, otherwise the locked-snapshot read-only guard
    // in the code editor would swallow `y`/`n`.
    if app.pending_detach.is_some() {
        handle_detach_confirm_keys(app, key);
        return;
    }

    // 3b. Inspector overlay
    if components::inspector::handle_inspector_keys(app, key) { return; }

    // 3c. Variable detail overlay
    if components::var_detail::handle_var_detail_keys(app, key) { return; }

    // 4. Toolbar popup key consumption
    if toolbar::handle_auto_run_keys(app, key) { return; }
    if toolbar::handle_extra_headers_keys(app, key) { return; }
    if toolbar::handle_azure_keys(app, key) { return; }
    if toolbar::handle_otel_keys(app, key) { return; }
    if toolbar::handle_live_capture_keys(app, key) { return; }

    // 5. History popup
    if app.history_popup.is_some() {
        components::history::handle_history_popup_keys(app, key);
        return;
    }

    // 6. History detail overlay
    if app.history_detail_idx.is_some() {
        let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => {
                app.history_detail_idx = None;
                app.history_json_nodes.clear();
                app.history_json_cursor = 0;
                app.history_json_built = false;
                app.history_json_expanded.clear();
            }
            KeyCode::Tab | KeyCode::BackTab => {
                app.history_detail_tab = if app.history_detail_tab == 0 { 1 } else { 0 };
                app.history_detail_scroll = 0;
                app.history_json_built = false;
                app.history_json_cursor = 0;
            }
            // Ctrl+Up/Down for page scroll
            KeyCode::Up if is_ctrl => {
                app.history_detail_scroll = app.history_detail_scroll.saturating_sub(5);
            }
            KeyCode::Down if is_ctrl => {
                app.history_detail_scroll = app.history_detail_scroll.saturating_add(5);
            }
            // Ctrl+U/D for half-page scroll
            KeyCode::Char('u') if is_ctrl => {
                app.history_detail_scroll = app.history_detail_scroll.saturating_sub(10);
            }
            KeyCode::Char('d') if is_ctrl => {
                app.history_detail_scroll = app.history_detail_scroll.saturating_add(10);
            }
            // Up/Down for JSON tree cursor navigation (response tab with JSON) or scroll
            KeyCode::Up | KeyCode::Char('k') => {
                if app.history_detail_tab == 1 && !app.history_json_nodes.is_empty() {
                    app.history_json_cursor = app.history_json_cursor.saturating_sub(1);
                    history_auto_scroll(app);
                } else {
                    app.history_detail_scroll = app.history_detail_scroll.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if app.history_detail_tab == 1 && !app.history_json_nodes.is_empty() {
                    let max = app.history_json_nodes.len().saturating_sub(1);
                    if app.history_json_cursor < max {
                        app.history_json_cursor += 1;
                    }
                    history_auto_scroll(app);
                } else {
                    app.history_detail_scroll = app.history_detail_scroll.saturating_add(1);
                }
            }
            // Enter to toggle expand/collapse individual node
            KeyCode::Enter => {
                if app.history_detail_tab == 1 && !app.history_json_nodes.is_empty() {
                    let cursor = app.history_json_cursor;
                    if let Some(node) = app.history_json_nodes.get(cursor) {
                        if node.is_expandable {
                            let path = node.path.clone();
                            if node.is_expanded {
                                app.history_json_expanded.remove(&path);
                                app.history_json_expanded.insert(format!("!{}", path));
                            } else {
                                app.history_json_expanded.remove(&format!("!{}", path));
                                app.history_json_expanded.insert(path);
                            }
                            app.history_json_built = false; // trigger rebuild
                        }
                    }
                }
            }
            // E to expand all
            KeyCode::Char('E') => {
                if app.history_detail_tab == 1 {
                    history_expand_all(app);
                }
            }
            // C to collapse all
            KeyCode::Char('C') => {
                if app.history_detail_tab == 1 {
                    history_collapse_all(app);
                }
            }
            // 1-9 to expand to specific depth
            KeyCode::Char(c @ '1'..='9') => {
                if app.history_detail_tab == 1 {
                    let depth = (c as usize) - ('0' as usize);
                    history_expand_to_depth(app, depth);
                }
            }
            _ => {}
        }
        return;
    }

    // 6b. Progress panel expanded overlay
    if app.progress_expanded {
        match key.code {
            KeyCode::Esc | KeyCode::Char('E') => { app.progress_expanded = false; }
            KeyCode::Up => { app.progress_scroll = app.progress_scroll.saturating_sub(1); }
            KeyCode::Down => {
                let max = app.run_progress_lines.len().saturating_sub(1);
                if app.progress_scroll < max { app.progress_scroll += 1; }
            }
            KeyCode::Enter => {
                // Select the block at current scroll position and jump to it
                if let Some((_, name)) = app.run_progress_lines.get(app.progress_scroll) {
                    // Find the block by name and select it
                    if let Some(fi) = app.active_file_idx {
                        if let Some(file) = app.loaded_files.get(fi) {
                            if let Some(bi) = file.suite.blocks.iter().position(|b| b.name == *name) {
                                app.select_block(fi, bi);
                                app.progress_expanded = false;
                            }
                        }
                    }
                }
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                app.run_progress_lines.clear();
                app.progress_expanded = false;
                app.progress_scroll = 0;
            }
            _ => {}
        }
        return;
    }

    // 7. Global keybinds
    let in_text_input = matches!(app.focus, Focus::FilterInput);

    // 7a. Sessions mode owns most keys (including 'f', 'h', 'l', 'g', 'r', '/'),
    // so route them before the global mode-switch handlers below grab them.
    if app.mode == Mode::Sessions {
        // Always pass through quit and help.
        let pass_through = matches!(
            key.code,
            KeyCode::Char('q') | KeyCode::Char('?')
        ) || matches!(key.code, KeyCode::F(1) | KeyCode::F(3) | KeyCode::F(4) | KeyCode::F(5));
        if !pass_through {
            handle_sessions_mode(app, key);
            return;
        }
    }

    match key.code {
        KeyCode::Char('q') if !in_text_input => {
            app.should_quit = true;
            return;
        }
        KeyCode::Char('?') if !in_text_input => {
            app.show_help = !app.show_help;
            return;
        }
        // Mode switching
        KeyCode::Char('f') if !in_text_input && app.mode != Mode::Files => {
            app.mode = Mode::Files;
            app.focus = Focus::FileTree;
            return;
        }
        KeyCode::Char('h') if !in_text_input && app.mode != Mode::History
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && app.focus != Focus::Response => {
            app.mode = Mode::History;
            app.focus = Focus::HistoryList;
            return;
        }
        KeyCode::Char('l') if !in_text_input && app.mode != Mode::Logs
            && !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.mode = Mode::Logs;
            return;
        }
        KeyCode::Char('S') if !in_text_input && app.mode != Mode::Sessions => {
            app.mode = Mode::Sessions;
            app.sessions_tab.refresh();
            return;
        }
        KeyCode::F(4) => {
            app.mode = Mode::Sessions;
            app.sessions_tab.refresh();
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
            // If the active file was loaded from a snapshot, Shift-R opens the
            // replay-all confirmation modal instead of kicking off a regular
            // run. The modal records the new run under the snapshot's
            // `file_id` so it groups beside the original in the Sessions tab.
            if let Some(fi) = app.active_file_idx {
                if app
                    .loaded_files
                    .get(fi)
                    .and_then(|f| f.snapshot.as_ref())
                    .is_some()
                {
                    if let Some(replay) = app.build_all_replay_confirm(fi) {
                        app.pending_replay = Some(replay);
                        app.set_status("Replay snapshot? [Y]es / [N]o".to_string());
                        return;
                    }
                }
            }
            app.queue_run_all();
            return;
        }
        KeyCode::Char('D') if !in_text_input => {
            // Shift-D detaches the active snapshot: the recorded session
            // stays on disk in the Sessions tab, but this loaded entry
            // becomes a regular unsaved buffer the user can edit. We
            // open a confirm modal first; the modal-key handler above
            // applies the detach on `y`/`Y`/Enter.
            if let Some(fi) = app.active_file_idx {
                if let Some(detach) = app.confirm_detach(fi) {
                    app.pending_detach = Some(detach);
                    app.set_status("Detach snapshot? [Y]es / [N]o".to_string());
                    return;
                }
            }
            // Not on a locked snapshot: nothing to do.
            return;
        }
        KeyCode::Char('c') if !in_text_input => {
            app.enter_code_editor();
            return;
        }
        KeyCode::Char('T') if !in_text_input => {
            crate::ui::theme::cycle_next();
            return;
        }
        KeyCode::Char('V') if !in_text_input => {
            app.sidebar_tab = match app.sidebar_tab {
                SidebarTab::Files => {
                    app.focus = Focus::Variables;
                    app.mode = Mode::Files;
                    SidebarTab::Variables
                }
                SidebarTab::Variables => {
                    app.focus = Focus::FileTree;
                    app.mode = Mode::Files;
                    SidebarTab::Files
                }
            };
            return;
        }
        KeyCode::Char('E') if !in_text_input && !app.run_progress_lines.is_empty() => {
            app.progress_expanded = !app.progress_expanded;
            return;
        }
        KeyCode::Char('X') if !in_text_input && !app.run_progress_lines.is_empty() && !app.is_running => {
            app.run_progress_lines.clear();
            app.progress_scroll = 0;
            return;
        }
        KeyCode::Tab if app.mode != Mode::Logs && app.focus != Focus::Builder => {
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
        KeyCode::Char('N') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.new_test_block();
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
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Env picker: list loaded .env files, activate / remove / add.
            // Position cursor on the currently-active entry (or "(none)" at 0).
            app.env_picker_cursor = match app.env_config.active_index {
                Some(i) => i + 1,
                None => 0,
            };
            app.env_picker_open = true;
            return;
        }
        // Toolbar shortcuts
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // In Code mode, Ctrl+X = Cut (not clear all)
            if app.mode != Mode::Code {
                toolbar::handle_toolbar_shortcuts(app, key);
                return;
            }
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // In Code mode, Ctrl+A = Select All (not Azure popup)
            if app.mode != Mode::Code {
                toolbar::handle_toolbar_shortcuts(app, key);
                return;
            }
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toolbar::handle_toolbar_shortcuts(app, key);
            return;
        }
        _ => {}
    }

    // 8. Mode-specific keybinds
    match app.mode {
        Mode::Files => handle_files_mode(app, key),
        Mode::History => components::history::handle_history_keys(app, key),
        Mode::Code => crate::code_editor::handle_editor_keys(app, key),
        Mode::Logs => components::logs::handle_logs_keys(app, key),
        Mode::Sessions => handle_sessions_mode(app, key),
    }
}

fn handle_sessions_mode(app: &mut App, key: KeyEvent) {
    // Search input mode: capture chars/backspace, Esc clears, Enter exits.
    if app.sessions_tab.searching {
        let s = &mut app.sessions_tab;
        match key.code {
            KeyCode::Esc => {
                s.search.clear();
                s.searching = false;
                s.clamp_selection();
            }
            KeyCode::Enter => {
                s.searching = false;
            }
            KeyCode::Backspace => {
                s.search.pop();
                s.clamp_selection();
            }
            KeyCode::Char(c) => {
                s.search.push(c);
                s.clamp_selection();
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
        KeyCode::Enter => {
            // Load the selected session as a read-only snapshot into the
            // Files tab, mirroring the desktop app's behavior.
            let selection = app.sessions_tab.selected_row_ref().map(|r| {
                (
                    r.file.file_id.clone(),
                    r.version.sha256.clone(),
                    r.session.run_id.clone(),
                    r.file.display_name.clone(),
                )
            });
            if let Some((file_id, sha, run_id, name)) = selection {
                match app.load_snapshot_from_session(&file_id, &sha, &run_id, &name) {
                    Ok(()) => {
                        let short = &sha[..sha.len().min(8)];
                        app.set_status(format!("Loaded snapshot {} ({})", name, short));
                    }
                    Err(e) => {
                        app.set_status(format!("Failed to load snapshot: {}", e));
                    }
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => app.sessions_tab.move_down(),
        KeyCode::Up | KeyCode::Char('k') => app.sessions_tab.move_up(),
        KeyCode::Right | KeyCode::Char('l') => app.sessions_tab.next_group(),
        KeyCode::Left | KeyCode::Char('h') => app.sessions_tab.prev_group(),
        KeyCode::Tab | KeyCode::Char('g') => app.sessions_tab.toggle_group_by(),
        KeyCode::Char('f') => app.sessions_tab.cycle_status_filter(),
        KeyCode::Char('/') => {
            app.sessions_tab.searching = true;
            app.sessions_tab.search.clear();
        }
        KeyCode::Char('r') => app.sessions_tab.refresh(),
        _ => {}
    }
}

fn handle_input_mode(app: &mut App, key: KeyEvent) {
    // Extract purpose and buffer to avoid borrow conflicts
    let (purpose, mut buffer) = match &app.input_mode {
        InputMode::Input { purpose, buffer, .. } => (purpose.clone(), buffer.clone()),
        _ => return,
    };

    match key.code {
        KeyCode::Esc => {
            app.autocomplete_suggestions.clear();
            app.autocomplete_idx = None;
            app.input_mode = InputMode::Normal;
            return;
        }
        KeyCode::Enter => {
            app.autocomplete_suggestions.clear();
            app.autocomplete_idx = None;
            let value = buffer;
            app.input_mode = InputMode::Normal;
            app.submit_input(purpose, value);
            return;
        }
        KeyCode::Tab => {
            if matches!(purpose, InputPurpose::OpenFile | InputPurpose::LoadEnv | InputPurpose::SaveFile) {
                if app.autocomplete_suggestions.is_empty() || app.autocomplete_idx.is_none() {
                    app.compute_file_completions(&buffer);
                }
                if !app.autocomplete_suggestions.is_empty() {
                    let idx = app
                        .autocomplete_idx
                        .map(|i| (i + 1) % app.autocomplete_suggestions.len())
                        .unwrap_or(0);
                    app.autocomplete_idx = Some(idx);
                    buffer = app.autocomplete_suggestions[idx].clone();
                }
            }
        }
        KeyCode::BackTab => {
            if matches!(purpose, InputPurpose::OpenFile | InputPurpose::LoadEnv | InputPurpose::SaveFile)
                && !app.autocomplete_suggestions.is_empty()
            {
                let len = app.autocomplete_suggestions.len();
                let idx = app
                    .autocomplete_idx
                    .map(|i| (i + len - 1) % len)
                    .unwrap_or(len - 1);
                app.autocomplete_idx = Some(idx);
                buffer = app.autocomplete_suggestions[idx].clone();
            }
        }
        KeyCode::Backspace => {
            buffer.pop();
            app.autocomplete_suggestions.clear();
            app.autocomplete_idx = None;
        }
        KeyCode::Char(c) => {
            buffer.push(c);
            app.autocomplete_suggestions.clear();
            app.autocomplete_idx = None;
        }
        _ => {}
    }

    // Write buffer back
    if let InputMode::Input { buffer: ref mut buf, .. } = app.input_mode {
        *buf = buffer;
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
                app.focus = Focus::Builder;
            }
        }
        Focus::Variables => app.focus = Focus::Builder,
        Focus::Builder => app.focus = Focus::Response,
        Focus::CodeView => app.focus = Focus::Response,
        Focus::Response => app.focus = Focus::FileTree,
        Focus::HistoryList => app.focus = Focus::FilterInput,
        Focus::FilterInput => app.focus = Focus::HistoryList,
    }
}

fn handle_files_mode(app: &mut App, key: KeyEvent) {
    // Esc navigates back one level
    if key.code == KeyCode::Esc {
        match app.focus {
            Focus::Builder => { app.focus = Focus::FileTree; }
            Focus::Response => { app.focus = Focus::Builder; }
            Focus::Variables => {
                app.focus = Focus::FileTree;
                app.sidebar_tab = SidebarTab::Files;
            }
            Focus::CodeView => { app.focus = Focus::FileTree; }
            Focus::FileTree => {}
            _ => {}
        }
        return;
    }

    // Arrow key pane navigation (skip when editing URL in builder)
    let in_url_edit = app.focus == Focus::Builder && app.builder_focus == BuilderFocus::Url;
    if !in_url_edit {
        match key.code {
            KeyCode::Left if matches!(app.focus, Focus::Builder | Focus::Response | Focus::CodeView) => {
                app.focus = Focus::FileTree;
                return;
            }
            KeyCode::Right if app.focus == Focus::FileTree => {
                app.focus = Focus::Builder;
                return;
            }
            KeyCode::Up if app.focus == Focus::Response => {
                app.focus = Focus::Builder;
                return;
            }
            KeyCode::Down if app.focus == Focus::Builder => {
                app.focus = Focus::Response;
                return;
            }
            _ => {}
        }
    }

    match app.focus {
        Focus::FileTree => components::sidebar::handle_file_tree_keys(app, key),
        Focus::CodeView => handle_code_scroll(app, key),
        Focus::Response => handle_response(app, key),
        Focus::Variables => components::sidebar::handle_variables_keys(app, key),
        Focus::Builder => components::builder::handle_builder_keys(app, key),
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

/// Auto-scroll history detail so the JSON cursor stays visible.
/// The JSON tree starts after ~6 header lines (status, blank, headers section title,
/// headers, blank, body title, blank). We estimate the offset and scroll accordingly.
fn history_auto_scroll(app: &mut App) {
    // Estimate: response headers count + ~7 fixed lines before JSON tree
    let header_lines = if app.history_detail_idx.is_some() {
        // Get a rough count of response headers
        let hdr_count = {
            use request_pilot_core::history::HistoryFilter;
            let filter = if app.filter_text.is_empty() {
                HistoryFilter::default()
            } else {
                HistoryFilter { url_contains: Some(app.filter_text.clone()), ..Default::default() }
            };
            let mut filtered = app.history.filter(&filter);
            filtered.reverse();
            app.history_detail_idx
                .and_then(|idx| filtered.get(idx).map(|e| e.response_headers.len()))
                .unwrap_or(0)
        };
        // status + blank + "--- Headers (N) ---" + blank + N headers + blank + "--- Body ---" + blank
        2 + 2 + hdr_count + 2 + 2
    } else {
        8
    };
    let cursor_line = header_lines + app.history_json_cursor;
    let visible_h = app.history_detail_content_height as usize;
    let scroll = app.history_detail_scroll as usize;

    if visible_h == 0 { return; }

    // If cursor is below visible area, scroll down
    if cursor_line >= scroll + visible_h {
        app.history_detail_scroll = (cursor_line.saturating_sub(visible_h) + 1) as u16;
    }
    // If cursor is above visible area, scroll up
    if cursor_line < scroll {
        app.history_detail_scroll = cursor_line as u16;
    }
}

fn get_history_response_body(app: &App) -> Option<String> {
    let idx = app.history_detail_idx?;
    let filter = if app.filter_text.is_empty() {
        request_pilot_core::history::HistoryFilter::default()
    } else {
        request_pilot_core::history::HistoryFilter {
            url_contains: Some(app.filter_text.clone()),
            ..Default::default()
        }
    };
    let mut filtered = app.history.filter(&filter);
    filtered.reverse();
    let entry = filtered.get(idx)?;
    entry.response_body.clone()
}

fn history_expand_all(app: &mut App) {
    if let Some(body) = get_history_response_body(app) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            let mut paths = std::collections::HashSet::new();
            components::response::collect_all_paths(&value, "root", &mut paths);
            app.history_json_expanded.retain(|p| !p.starts_with('!'));
            for p in paths {
                app.history_json_expanded.insert(p);
            }
            app.history_json_built = false;
        }
    }
}

fn history_collapse_all(app: &mut App) {
    if let Some(body) = get_history_response_body(app) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            let mut paths = std::collections::HashSet::new();
            components::response::collect_all_paths(&value, "root", &mut paths);
            app.history_json_expanded.clear();
            for p in paths {
                app.history_json_expanded.insert(format!("!{}", p));
            }
            app.history_json_built = false;
            app.history_json_cursor = 0;
        }
    }
}

fn history_expand_to_depth(app: &mut App, max_depth: usize) {
    if let Some(body) = get_history_response_body(app) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            app.history_json_expanded.clear();
            expand_paths_to_depth(&value, "root", 0, max_depth, &mut app.history_json_expanded);
            app.history_json_built = false;
        }
    }
}

fn expand_paths_to_depth(
    value: &serde_json::Value,
    path: &str,
    depth: usize,
    max_depth: usize,
    expanded: &mut std::collections::HashSet<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if depth < max_depth {
                expanded.insert(path.to_string());
                for (k, v) in map {
                    let cp = if path.is_empty() { k.clone() } else { format!("{}.{}", path, k) };
                    expand_paths_to_depth(v, &cp, depth + 1, max_depth, expanded);
                }
            } else {
                expanded.insert(format!("!{}", path));
            }
        }
        serde_json::Value::Array(arr) => {
            if depth < max_depth {
                expanded.insert(path.to_string());
                for (i, v) in arr.iter().enumerate() {
                    expand_paths_to_depth(v, &format!("{}[{}]", path, i), depth + 1, max_depth, expanded);
                }
            } else {
                expanded.insert(format!("!{}", path));
            }
        }
        _ => {}
    }
}

fn handle_response(app: &mut App, key: KeyEvent) {
    crate::components::response::handle_response_keys(app, key);
}

/// Handle keys while the snapshot replay confirmation modal is open. Accepts
/// `y`/`Y`/Enter to confirm, `n`/`N`/Esc to cancel. Any other key is ignored
/// (the modal stays open).
fn handle_replay_confirm_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            if let Some(replay) = app.pending_replay.take() {
                match &replay.kind {
                    crate::app::ReplayKind::Block(_) => {
                        app.spawn_single_block_run_for_replay(replay);
                    }
                    crate::app::ReplayKind::All => {
                        app.spawn_suite_run_for_replay(replay);
                    }
                }
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.pending_replay = None;
            app.set_status("Replay cancelled".to_string());
        }
        _ => {}
    }
}

/// Handle keys while the snapshot detach confirmation modal is open. Accepts
/// `y`/`Y`/Enter to confirm (clears the snapshot marker and unlocks the
/// file), `n`/`N`/Esc to cancel. Any other key is ignored (the modal
/// stays open).
fn handle_detach_confirm_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            app.apply_detach();
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.pending_detach = None;
            app.set_status("Detach cancelled".to_string());
        }
        _ => {}
    }
}

// Sidebar event handling delegated to components::sidebar