#[cfg(test)]
mod tests {
    use crate::{
        app::{
            App, BuilderFocus, ConfirmPurpose, Focus, HistoryGroupBy, HistoryPopup, InputMode,
            InputPurpose, LoadedFile, LogEntry, LogFilter, LogLevel, Mode, ResponseTab,
            SidebarTab, TreeNode,
        },
        code_editor,
        components::{
            builder,
            diff_viewer::{self, compute_line_diff, DiffLine, DiffViewMode, DisplayLine, DiffViewerData},
            history,
            logs::matches_filter,
            response::{self, detect_content_kind, ContentKind, JsonNodeType},
            sidebar,
        },
        events::handle_key,
        ui::theme,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use request_pilot_core::{
        assertions::AssertionResult,
        history::HistoryEntry,
        http_client::HttpResponse,
        http_parser::parse_test_suite,
        test_runner::{BlockResult, ExtractResult, StepResult, TestRunResults},
    };
    use std::{
        collections::HashMap,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_ARTIFACT_ID: AtomicU64 = AtomicU64::new(0);

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_char(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn key_ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    struct TestArtifact {
        path: PathBuf,
    }

    impl TestArtifact {
        fn new(name: &str, extension: &str, content: &str) -> Self {
            let mut path = PathBuf::from(r"C:\Booshi\Repos\request-pilot\tui\target\test-artifacts");
            fs::create_dir_all(&path).unwrap();
            let id = NEXT_ARTIFACT_ID.fetch_add(1, Ordering::Relaxed);
            path.push(format!("{}-{}-{}.{}", name, std::process::id(), id, extension));
            fs::write(&path, content).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestArtifact {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    fn sample_suite_content() -> &'static str {
        r#"### 
@test First
GET https://example.com/first
Accept: application/json

###
@test Second
# @group api
POST https://example.com/second
Content-Type: application/json

{"ok":true}

###
@test Third
# @group api
GET https://example.com/third
"#
    }

    fn push_file(app: &mut App, name: &str, content: &str) {
        let suite = parse_test_suite(content);
        app.loaded_files.push(LoadedFile {
            id: app.loaded_files.len() as u64,
            path: None,
            name: name.to_string(),
            content: content.to_string(),
            suite,
            results: None,
            expanded: true,
            group_expanded: HashMap::new(),
        });
        app.rebuild_tree();
        if app.active_file_idx.is_none() {
            app.active_file_idx = Some(0);
        }
        if app.active_block_idx.is_none()
            && !app.loaded_files[0].suite.blocks.is_empty()
        {
            app.active_block_idx = Some(0);
        }
    }

    fn app_with_file() -> App {
        let mut app = App::new();
        push_file(&mut app, "sample.http", sample_suite_content());
        app
    }

    fn app_with_history_entries() -> App {
        let mut app = App::new();
        app.focus = Focus::HistoryList;
        app.mode = Mode::History;
        app.history.add(history_entry(1, "GET", "https://alpha.example.com/one", 200, "manual", Some("suite.http"), Some("group-a")));
        app.history.add(history_entry(2, "POST", "https://alpha.example.com/two", 404, "tui", Some("suite.http"), Some("group-a")));
        app.history.add(history_entry(3, "DELETE", "https://beta.example.com/three", 500, "live", Some("other.http"), Some("group-b")));
        app
    }

    fn history_entry(
        seq: u64,
        method: &str,
        url: &str,
        status: u16,
        source: &str,
        file_name: Option<&str>,
        group: Option<&str>,
    ) -> HistoryEntry {
        HistoryEntry {
            seq,
            id: format!("id-{seq}"),
            run_id: Some("run-1".to_string()),
            source: source.to_string(),
            file_name: file_name.map(str::to_string),
            group: group.map(str::to_string),
            block_name: Some(format!("block-{seq}")),
            compare_step: None,
            method: method.to_string(),
            url: url.to_string(),
            request_headers: vec![],
            request_body: None,
            status,
            response_headers: vec![],
            response_body: Some(format!("body-{seq}")),
            response_time_ms: seq * 100,
            response_size_bytes: 100 + seq as usize,
            timestamp: format!("2024-01-0{}T12:00:0{}Z", seq, seq),
        }
    }

    fn make_response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse {
            status,
            status_text: format!("Status {status}"),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            body: body.to_string(),
            time_ms: 42,
            size_bytes: body.len(),
        }
    }

    fn make_block_result(name: &str, method: &str, url: &str, status: &str, response: Option<HttpResponse>) -> BlockResult {
        BlockResult {
            seq: None,
            name: name.to_string(),
            block_type: "test".to_string(),
            group: Some("api".to_string()),
            request_method: method.to_string(),
            request_url: url.to_string(),
            request_headers: vec![("Accept".to_string(), "application/json".to_string())],
            request_body: None,
            status: status.to_string(),
            response,
            assertion_results: vec![AssertionResult {
                assertion: "status == 200".to_string(),
                passed: status == "passed",
                actual: Some("200".to_string()),
                expected: Some("200".to_string()),
            }],
            extract_results: vec![ExtractResult {
                variable: "token".to_string(),
                value: Some("abc123".to_string()),
                success: true,
            }],
            error: None,
            time_ms: 42,
            step_results: vec![StepResult {
                name: "baseline".to_string(),
                request_method: method.to_string(),
                request_url: url.to_string(),
                request_headers: vec![],
                request_body: None,
                response: None,
                assertion_results: vec![],
                extract_results: vec![],
                time_ms: 1,
                error: None,
            }],
            diff_result: None,
        }
    }

    fn attach_result(app: &mut App, body: &str, headers: &[(&str, &str)]) {
        let result = make_block_result(
            "First",
            "GET",
            "https://example.com/first",
            "passed",
            Some(make_response(200, headers, body)),
        );
        app.loaded_files[0].results = Some(TestRunResults {
            passed: 1,
            failed: 0,
            skipped: 0,
            total_time_ms: 42,
            block_results: vec![result],
            final_variables: HashMap::new(),
            telemetry: None,
        });
        app.active_file_idx = Some(0);
        app.active_block_idx = Some(0);
    }

    fn visible_log_count(app: &App) -> usize {
        app.log_entries
            .iter()
            .filter(|entry| matches_filter(entry, &app.log_filter))
            .count()
    }

    #[test]
    fn app_new_initializes_defaults() {
        let app = App::new();

        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.focus, Focus::FileTree);
        assert_eq!(app.sidebar_tab, SidebarTab::Files);
        assert!(app.loaded_files.is_empty());
        assert!(app.env_vars.is_empty());
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.response_tab, ResponseTab::Body);
        assert_eq!(app.history_group_by, HistoryGroupBy::Flat);
        assert_eq!(app.builder_focus, BuilderFocus::Method);
        assert_eq!(app.log_filter, LogFilter::All);
        assert!(app.log_auto_scroll);
        assert!(!app.diff_viewer_open);
    }

    #[test]
    fn spinner_char_cycles_every_eight_ticks() {
        let mut app = App::new();
        let first = app.spinner_char();
        app.spinner_tick = 8;
        assert_eq!(app.spinner_char(), first);
    }

    #[test]
    fn select_block_resets_response_state_and_rebuilds_json_tree() {
        let mut app = app_with_file();
        attach_result(&mut app, r#"{"outer":{"inner":1},"ok":true}"#, &[("content-type", "application/json")]);
        app.code_scroll = 7;
        app.response_scroll = 9;
        app.json_cursor = 3;
        app.body_fully_loaded = true;
        app.response_body_full = Some("cached".to_string());
        app.json_expanded.insert("root.outer".to_string());

        app.select_block(0, 0);

        assert_eq!(app.active_file_idx, Some(0));
        assert_eq!(app.active_block_idx, Some(0));
        assert_eq!(app.code_scroll, 0);
        assert_eq!(app.response_scroll, 0);
        assert_eq!(app.json_cursor, 0);
        assert!(!app.body_fully_loaded);
        assert!(app.response_body_full.is_none());
        assert!(app.json_expanded.is_empty());
        assert!(!app.json_tree_nodes.is_empty());
    }

    #[test]
    fn submit_input_add_var_name_enters_value_prompt() {
        let mut app = App::new();

        app.submit_input(InputPurpose::AddVarName, "API_KEY".to_string());

        assert_eq!(
            app.input_mode,
            InputMode::Input {
                prompt: "Value for 'API_KEY': ".to_string(),
                purpose: InputPurpose::AddVarValue {
                    name: "API_KEY".to_string(),
                },
                buffer: String::new(),
            }
        );
    }

    #[test]
    fn submit_input_add_var_name_ignores_empty_name() {
        let mut app = App::new();

        app.submit_input(InputPurpose::AddVarName, String::new());

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.env_vars.is_empty());
    }

    #[test]
    fn submit_input_add_var_value_inserts_variable() {
        let mut app = App::new();

        app.submit_input(
            InputPurpose::AddVarValue {
                name: "BASE_URL".to_string(),
            },
            "https://example.com".to_string(),
        );

        assert_eq!(
            app.env_vars.get("BASE_URL").map(String::as_str),
            Some("https://example.com")
        );
    }

    #[test]
    fn submit_input_edit_var_value_updates_existing_variable() {
        let mut app = App::new();
        app.env_vars.insert("TOKEN".to_string(), "old".to_string());

        app.submit_input(
            InputPurpose::EditVarValue {
                name: "TOKEN".to_string(),
            },
            "new".to_string(),
        );

        assert_eq!(app.env_vars.get("TOKEN").map(String::as_str), Some("new"));
    }

    #[test]
    fn submit_input_edit_var_value_ignores_missing_variable() {
        let mut app = App::new();

        app.submit_input(
            InputPurpose::EditVarValue {
                name: "TOKEN".to_string(),
            },
            "new".to_string(),
        );

        assert!(!app.env_vars.contains_key("TOKEN"));
    }

    #[test]
    fn submit_input_open_file_loads_suite() {
        let artifact = TestArtifact::new("open-file", "http", sample_suite_content());
        let mut app = App::new();

        app.submit_input(
            InputPurpose::OpenFile,
            artifact.path().display().to_string(),
        );

        assert_eq!(app.loaded_files.len(), 1);
        assert_eq!(app.active_file_idx, Some(0));
        assert_eq!(app.active_block_idx, Some(0));
        assert!(matches!(app.tree_nodes.first(), Some(TreeNode::File { file_idx: 0 })));
    }

    #[test]
    fn submit_input_load_env_reads_variables() {
        let artifact = TestArtifact::new("load-env", "env", "TOKEN=abc\nHOST=example.com\n");
        let mut app = App::new();

        app.submit_input(
            InputPurpose::LoadEnv,
            artifact.path().display().to_string(),
        );

        assert_eq!(app.env_vars.get("TOKEN").map(String::as_str), Some("abc"));
        assert_eq!(app.env_vars.get("HOST").map(String::as_str), Some("example.com"));
        assert_eq!(app.env_path.as_deref(), Some(artifact.path()));
    }

    #[test]
    fn clear_all_clears_results_history_and_progress() {
        let mut app = app_with_file();
        attach_result(&mut app, r#"{"ok":true}"#, &[("content-type", "application/json")]);
        app.history.add(history_entry(1, "GET", "https://example.com", 200, "tui", Some("sample.http"), None));
        app.history_cursor = 4;
        app.progress_current = 2;
        app.progress_total = 3;

        app.clear_all();

        assert!(app.history.entries.is_empty());
        assert_eq!(app.history_cursor, 0);
        assert!(app.loaded_files.iter().all(|file| file.results.is_none()));
        assert_eq!(app.progress_current, 0);
        assert_eq!(app.progress_total, 0);
    }

    #[test]
    fn confirm_action_delete_var_removes_variable() {
        let mut app = App::new();
        app.env_vars.insert("TOKEN".to_string(), "abc".to_string());

        app.confirm_action(ConfirmPurpose::DeleteVar {
            name: "TOKEN".to_string(),
        });

        assert!(!app.env_vars.contains_key("TOKEN"));
    }

    #[test]
    fn confirm_action_clear_history_resets_cursor() {
        let mut app = app_with_history_entries();
        app.history_cursor = 2;

        app.confirm_action(ConfirmPurpose::ClearHistory);

        assert!(app.history.entries.is_empty());
        assert_eq!(app.history_cursor, 0);
    }

    #[test]
    fn confirm_action_close_file_updates_selection() {
        let mut app = app_with_file();
        push_file(&mut app, "other.http", sample_suite_content());
        app.active_file_idx = Some(1);
        app.tree_cursor = app.tree_nodes.len().saturating_sub(1);

        app.confirm_action(ConfirmPurpose::CloseFile { file_idx: 1 });

        assert_eq!(app.loaded_files.len(), 1);
        assert_eq!(app.active_file_idx, Some(0));
        assert!(app.tree_cursor < app.tree_nodes.len());
    }

    #[test]
    fn builder_tab_cycles_focus_forward() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Method;

        builder::handle_builder_keys(&mut app, key(KeyCode::Tab));
        assert_eq!(app.builder_focus, BuilderFocus::Url);
        builder::handle_builder_keys(&mut app, key(KeyCode::Tab));
        assert_eq!(app.builder_focus, BuilderFocus::Headers);
        builder::handle_builder_keys(&mut app, key(KeyCode::Tab));
        assert_eq!(app.builder_focus, BuilderFocus::Body);
        builder::handle_builder_keys(&mut app, key(KeyCode::Tab));
        assert_eq!(app.builder_focus, BuilderFocus::Method);
    }

    #[test]
    fn builder_backtab_cycles_focus_backward() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Method;

        builder::handle_builder_keys(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.builder_focus, BuilderFocus::Body);
        builder::handle_builder_keys(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.builder_focus, BuilderFocus::Headers);
        builder::handle_builder_keys(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.builder_focus, BuilderFocus::Url);
    }

    #[test]
    fn builder_method_right_advances() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Method;

        builder::handle_builder_keys(&mut app, key(KeyCode::Right));

        assert_eq!(app.loaded_files[0].suite.blocks[0].request.method, "POST");
    }

    #[test]
    fn builder_method_left_wraps_from_get_to_options() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Method;

        builder::handle_builder_keys(&mut app, key(KeyCode::Left));

        assert_eq!(app.loaded_files[0].suite.blocks[0].request.method, "OPTIONS");
    }

    #[test]
    fn builder_method_right_wraps_from_options_to_get() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Method;
        app.loaded_files[0].suite.blocks[0].request.method = "OPTIONS".to_string();

        builder::handle_builder_keys(&mut app, key(KeyCode::Right));

        assert_eq!(app.loaded_files[0].suite.blocks[0].request.method, "GET");
    }

    #[test]
    fn builder_url_left_clamps_at_zero() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Url;
        app.builder_url_cursor = 0;

        builder::handle_builder_keys(&mut app, key(KeyCode::Left));

        assert_eq!(app.builder_url_cursor, 0);
    }

    #[test]
    fn builder_url_right_clamps_to_char_count() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Url;
        let char_count = app.loaded_files[0].suite.blocks[0].request.url.chars().count();
        app.builder_url_cursor = char_count;

        builder::handle_builder_keys(&mut app, key(KeyCode::Right));

        assert_eq!(app.builder_url_cursor, char_count);
    }

    #[test]
    fn builder_url_home_and_end_move_cursor() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Url;
        app.builder_url_cursor = 4;

        builder::handle_builder_keys(&mut app, key(KeyCode::Home));
        assert_eq!(app.builder_url_cursor, 0);

        builder::handle_builder_keys(&mut app, key(KeyCode::End));
        assert_eq!(
            app.builder_url_cursor,
            app.loaded_files[0].suite.blocks[0].request.url.chars().count()
        );
    }

    #[test]
    fn builder_url_insert_is_unicode_safe() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Url;
        app.loaded_files[0].suite.blocks[0].request.url = "éx".to_string();
        app.builder_url_cursor = 1;

        builder::handle_builder_keys(&mut app, key_char('!'));

        assert_eq!(app.loaded_files[0].suite.blocks[0].request.url, "é!x");
        assert_eq!(app.builder_url_cursor, 2);
    }

    #[test]
    fn builder_url_backspace_removes_previous_char() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Url;
        app.loaded_files[0].suite.blocks[0].request.url = "éx".to_string();
        app.builder_url_cursor = 1;

        builder::handle_builder_keys(&mut app, key(KeyCode::Backspace));

        assert_eq!(app.loaded_files[0].suite.blocks[0].request.url, "x");
        assert_eq!(app.builder_url_cursor, 0);
    }

    #[test]
    fn builder_headers_cursor_is_bounded() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Headers;

        builder::handle_builder_keys(&mut app, key(KeyCode::Down));
        assert_eq!(app.builder_header_cursor, 0);

        app.loaded_files[0].suite.blocks[0]
            .request
            .headers
            .push(("X-Test".to_string(), "1".to_string()));
        builder::handle_builder_keys(&mut app, key(KeyCode::Down));
        assert_eq!(app.builder_header_cursor, 1);
        builder::handle_builder_keys(&mut app, key(KeyCode::Down));
        assert_eq!(app.builder_header_cursor, 1);
    }

    #[test]
    fn builder_body_scroll_uses_saturating_math() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Body;

        builder::handle_builder_keys(&mut app, key_ctrl('u'));
        assert_eq!(app.builder_body_scroll, 0);

        builder::handle_builder_keys(&mut app, key_ctrl('d'));
        assert_eq!(app.builder_body_scroll, 10);
    }

    #[test]
    fn builder_ctrl_enter_queues_selected_block() {
        let mut app = app_with_file();
        app.builder_focus = BuilderFocus::Body;
        app.active_file_idx = Some(0);
        app.active_block_idx = Some(1);

        builder::handle_builder_keys(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );

        assert_eq!(app.run_single_queued, Some((0, 1)));
    }

    macro_rules! history_group_by_tests {
        ($($name:ident: $from:expr => $to:expr,)+) => {
            $(
                #[test]
                fn $name() {
                    assert_eq!($from.next(), $to);
                }
            )+
        };
    }

    history_group_by_tests! {
        history_group_by_flat_next_domain: HistoryGroupBy::Flat => HistoryGroupBy::Domain,
        history_group_by_domain_next_status: HistoryGroupBy::Domain => HistoryGroupBy::Status,
        history_group_by_status_next_source: HistoryGroupBy::Status => HistoryGroupBy::Source,
        history_group_by_source_next_file_group: HistoryGroupBy::Source => HistoryGroupBy::FileGroupTest,
        history_group_by_file_group_wraps_flat: HistoryGroupBy::FileGroupTest => HistoryGroupBy::Flat,
    }

    #[test]
    fn history_space_toggles_compare_selection() {
        let mut app = app_with_history_entries();

        history::handle_history_keys(&mut app, key_char(' '));
        assert_eq!(app.history_selected_seqs, vec![1]);

        history::handle_history_keys(&mut app, key_char(' '));
        assert!(app.history_selected_seqs.is_empty());
    }

    #[test]
    fn history_compare_selection_is_capped_at_two() {
        let mut app = app_with_history_entries();

        history::handle_history_keys(&mut app, key_char(' '));
        history::handle_history_keys(&mut app, key_char('j'));
        history::handle_history_keys(&mut app, key_char(' '));
        history::handle_history_keys(&mut app, key_char('j'));
        history::handle_history_keys(&mut app, key_char(' '));

        assert_eq!(app.history_selected_seqs, vec![1, 2]);
        assert!(
            app.status_message
                .as_ref()
                .map(|(msg, _)| msg.contains("Max 2 entries"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn history_method_popup_applies_filter() {
        let mut app = app_with_history_entries();
        app.history_popup = Some(HistoryPopup::MethodFilter { cursor: 2 });

        history::handle_history_popup_keys(&mut app, key(KeyCode::Enter));

        assert_eq!(app.history_method_filter.as_deref(), Some("POST"));
        assert!(app.history_popup.is_none());
        assert_eq!(app.history_cursor, 0);
    }

    #[test]
    fn history_status_popup_applies_filter() {
        let mut app = app_with_history_entries();
        app.history_popup = Some(HistoryPopup::StatusFilter { cursor: 3 });

        history::handle_history_popup_keys(&mut app, key(KeyCode::Enter));

        assert_eq!(app.history_status_filter.as_deref(), Some("4xx"));
        assert!(app.history_popup.is_none());
    }

    #[test]
    fn history_enter_on_group_header_toggles_collapse() {
        let mut app = app_with_history_entries();
        app.history_group_by = HistoryGroupBy::Domain;
        app.history_cursor = 0;

        history::handle_history_keys(&mut app, key(KeyCode::Enter));

        assert!(app.history_groups_collapsed.contains("alpha.example.com"));
    }

    #[test]
    fn history_filter_input_edits_text() {
        let mut app = app_with_history_entries();
        app.focus = Focus::FilterInput;

        history::handle_history_keys(&mut app, key_char('a'));
        history::handle_history_keys(&mut app, key_char('p'));
        history::handle_history_keys(&mut app, key(KeyCode::Backspace));

        assert_eq!(app.filter_text, "a");
        assert_eq!(app.history_cursor, 0);
    }

    #[test]
    fn history_slash_moves_focus_to_filter() {
        let mut app = app_with_history_entries();

        history::handle_history_keys(&mut app, key_char('/'));

        assert_eq!(app.focus, Focus::FilterInput);
    }

    #[test]
    fn history_g_cycles_grouping_and_clears_collapsed() {
        let mut app = app_with_history_entries();
        app.history_groups_collapsed.insert("alpha.example.com".to_string());
        app.history_cursor = 2;

        history::handle_history_keys(&mut app, key_char('g'));

        assert_eq!(app.history_group_by, HistoryGroupBy::Domain);
        assert_eq!(app.history_cursor, 0);
        assert!(app.history_groups_collapsed.is_empty());
    }

    #[test]
    fn history_enter_on_entry_opens_details_and_resets_scroll() {
        let mut app = app_with_history_entries();
        app.history_detail_scroll = 9;

        history::handle_history_keys(&mut app, key(KeyCode::Enter));

        assert_eq!(app.history_detail_idx, Some(0));
        assert_eq!(app.history_detail_scroll, 0);
    }

    #[test]
    fn compute_line_diff_identical_produces_same_lines() {
        let diff = compute_line_diff(&["a", "b"], &["a", "b"]);

        assert_eq!(diff.len(), 2);
        assert!(diff.iter().all(|line| matches!(line, DiffLine::Same(_))));
    }

    #[test]
    fn compute_line_diff_addition_produces_added_line() {
        let diff = compute_line_diff(&["a"], &["a", "b"]);

        assert!(matches!(diff.last(), Some(DiffLine::Added(line)) if line == "b"));
    }

    #[test]
    fn compute_line_diff_removal_produces_removed_line() {
        let diff = compute_line_diff(&["a", "b"], &["a"]);

        assert!(matches!(diff.last(), Some(DiffLine::Removed(line)) if line == "b"));
    }

    #[test]
    fn compute_line_diff_mixed_produces_modified_line() {
        let diff = compute_line_diff(&["same", "old"], &["same", "new"]);

        assert!(matches!(diff[0], DiffLine::Same(ref line) if line == "same"));
        assert!(matches!(diff[1], DiffLine::Modified(ref left, ref right) if left == "old" && right == "new"));
    }

    #[test]
    fn diff_viewer_new_computes_stats() {
        let data = DiffViewerData::new("a\nold\nc", "a\nnew\nc\nplus", "left".to_string(), "right".to_string());

        assert_eq!(data.stats.unchanged, 2);
        assert_eq!(data.stats.modified, 1);
        assert_eq!(data.stats.added, 1);
        assert_eq!(data.stats.removed, 0);
    }

    #[test]
    fn diff_viewer_builds_hunk_separator_for_distant_changes() {
        let left = (0..20)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let right = (0..20)
            .map(|i| match i {
                2 => "line-two-changed".to_string(),
                18 => "line-eighteen-changed".to_string(),
                _ => format!("line-{i}"),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let data = DiffViewerData::new(&left, &right, "a".to_string(), "b".to_string());

        assert!(data
            .display_lines
            .iter()
            .any(|line| matches!(line, DisplayLine::HunkSep(hidden) if *hidden > 0)));
    }

    #[test]
    fn diff_viewer_toggle_mode_shows_changes_only() {
        let mut app = App::new();
        app.open_diff_viewer("same\nold", "same\nnew\nplus", "a".to_string(), "b".to_string());

        diff_viewer::handle_diff_keys(&mut app, key_char('m'));

        let data = app.diff_viewer_data.as_ref().unwrap();
        assert_eq!(data.view_mode, DiffViewMode::ChangesOnly);
        assert!(data
            .display_lines
            .iter()
            .all(|line| !matches!(line, DisplayLine::Same(_))));
    }

    #[test]
    fn diff_viewer_handle_keys_scroll_and_close() {
        let mut app = App::new();
        app.open_diff_viewer("a\nb\nc", "a\nx\nc\nd", "a".to_string(), "b".to_string());

        diff_viewer::handle_diff_keys(&mut app, key_char('j'));
        assert_eq!(app.diff_viewer_data.as_ref().unwrap().scroll, 1);

        diff_viewer::handle_diff_keys(&mut app, key_char('g'));
        assert_eq!(app.diff_viewer_data.as_ref().unwrap().scroll, 0);

        diff_viewer::handle_diff_keys(&mut app, key_char('G'));
        assert_eq!(
            app.diff_viewer_data.as_ref().unwrap().scroll,
            app.diff_viewer_data.as_ref().unwrap().display_lines.len().saturating_sub(1)
        );

        diff_viewer::handle_diff_keys(&mut app, key(KeyCode::Esc));
        assert!(!app.diff_viewer_open);
        assert!(app.diff_viewer_data.is_none());
    }

    #[test]
    fn app_open_and_close_diff_viewer_manage_state() {
        let mut app = App::new();

        app.open_diff_viewer("left", "right", "old".to_string(), "new".to_string());
        assert!(app.diff_viewer_open);
        assert!(app.diff_viewer_data.is_some());

        app.close_diff_viewer();
        assert!(!app.diff_viewer_open);
        assert!(app.diff_viewer_data.is_none());
    }

    macro_rules! content_kind_tests {
        ($($name:ident: $headers:expr, $body:expr => $kind:expr,)+) => {
            $(
                #[test]
                fn $name() {
                    let headers: Vec<(String, String)> = $headers
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    assert_eq!(detect_content_kind(&headers, $body), $kind);
                }
            )+
        };
    }

    content_kind_tests! {
        response_detects_json_from_content_type: vec![("Content-Type", "application/json")], r#"{"ok":true}"# => ContentKind::Json,
        response_detects_xml_from_content_type: vec![("Content-Type", "application/xml")], r#"<root xmlns="x"></root>"# => ContentKind::Xml,
        response_detects_html_from_content_type: vec![("Content-Type", "text/html")], "<html></html>" => ContentKind::Html,
        response_detects_yaml_from_content_type: vec![("Content-Type", "application/yaml")], "name: value" => ContentKind::Yaml,
        response_detects_json_heuristically: Vec::<(&str, &str)>::new(), r#"{"ok":true}"# => ContentKind::Json,
        response_detects_text_fallback: Vec::<(&str, &str)>::new(), "plain text body" => ContentKind::Text,
    }

    #[test]
    fn rebuild_json_tree_creates_object_nodes() {
        let mut app = App::new();

        response::rebuild_json_tree(&mut app, r#"{"name":"alice","nested":{"count":1}}"#);

        assert!(!app.json_tree_nodes.is_empty());
        assert_eq!(app.json_tree_nodes[0].node_type, JsonNodeType::Object);
        assert!(app
            .json_tree_nodes
            .iter()
            .any(|node| node.key == "nested" && node.is_expandable));
    }

    #[test]
    fn rebuild_json_tree_creates_array_nodes() {
        let mut app = App::new();

        response::rebuild_json_tree(&mut app, r#"[1,true,null]"#);

        assert_eq!(app.json_tree_nodes[0].node_type, JsonNodeType::Array);
        assert!(app.json_tree_nodes.len() > 1);
    }

    #[test]
    fn rebuild_json_tree_creates_string_leaf_nodes() {
        let mut app = App::new();

        response::rebuild_json_tree(&mut app, r#""hello""#);

        assert_eq!(app.json_tree_nodes[0].node_type, JsonNodeType::String);
        assert_eq!(app.json_tree_nodes[0].value_preview, "\"hello\"");
    }

    #[test]
    fn rebuild_json_tree_creates_null_leaf_nodes() {
        let mut app = App::new();

        response::rebuild_json_tree(&mut app, "null");

        assert_eq!(app.json_tree_nodes[0].node_type, JsonNodeType::Null);
        assert_eq!(app.json_tree_nodes[0].value_preview, "null");
    }

    #[test]
    fn response_toggle_current_node_collapses_and_expands() {
        let mut app = app_with_file();
        attach_result(&mut app, r#"{"a":{"b":1},"c":2}"#, &[("content-type", "application/json")]);
        app.focus = Focus::Response;
        app.response_tab = ResponseTab::Body;
        app.select_block(0, 0);

        response::handle_response_keys(&mut app, key_char(' '));
        assert_eq!(app.json_tree_nodes.len(), 1);
        assert!(app.json_expanded.contains("!root"));

        response::handle_response_keys(&mut app, key_char(' '));
        assert!(app.json_tree_nodes.len() > 1);
        assert!(!app.json_expanded.contains("!root"));
    }

    #[test]
    fn response_expand_all_and_collapse_all_update_state() {
        let mut app = app_with_file();
        attach_result(&mut app, r#"{"a":{"b":1},"c":[1,2]}"#, &[("content-type", "application/json")]);
        app.focus = Focus::Response;
        app.response_tab = ResponseTab::Body;
        app.select_block(0, 0);

        response::handle_response_keys(&mut app, key_char('C'));
        assert_eq!(app.json_cursor, 0);
        assert_eq!(app.json_tree_nodes.len(), 1);
        assert!(app.json_expanded.iter().all(|path| path.starts_with('!')));

        response::handle_response_keys(&mut app, key_char('E'));
        assert!(app.json_tree_nodes.len() > 1);
        assert!(app.json_expanded.iter().any(|path| !path.starts_with('!')));
    }

    #[test]
    fn response_large_body_requires_explicit_load() {
        let large_json = format!(r#"{{"payload":"{}"}}"#, "a".repeat(262_200));
        let mut app = app_with_file();
        attach_result(&mut app, &large_json, &[("content-type", "application/json")]);
        app.focus = Focus::Response;
        app.response_tab = ResponseTab::Body;
        app.select_block(0, 0);

        assert!(!app.body_fully_loaded);
        assert!(app.response_body_full.is_none());
        assert!(app.json_tree_nodes.is_empty());

        response::handle_response_keys(&mut app, key_char('l'));

        assert!(app.body_fully_loaded);
        assert_eq!(app.response_body_full.as_deref(), Some(large_json.as_str()));
        assert!(!app.json_tree_nodes.is_empty());
    }

    #[test]
    fn response_tab_shortcuts_switch_tabs_and_reset_scroll() {
        let mut app = app_with_file();
        attach_result(&mut app, r#"{"ok":true}"#, &[("content-type", "application/json")]);
        app.response_scroll = 8;

        response::handle_response_keys(&mut app, key_char('h'));
        assert_eq!(app.response_tab, ResponseTab::Headers);
        assert_eq!(app.response_scroll, 0);

        app.response_scroll = 5;
        response::handle_response_keys(&mut app, key_char('a'));
        assert_eq!(app.response_tab, ResponseTab::Assertions);
        assert_eq!(app.response_scroll, 0);

        app.response_scroll = 5;
        response::handle_response_keys(&mut app, key_char('b'));
        assert_eq!(app.response_tab, ResponseTab::Body);
        assert_eq!(app.response_scroll, 0);
    }

    #[test]
    fn response_json_cursor_moves_within_bounds() {
        let mut app = app_with_file();
        attach_result(&mut app, r#"{"a":1,"b":{"c":2},"d":[1,2]}"#, &[("content-type", "application/json")]);
        app.focus = Focus::Response;
        app.response_tab = ResponseTab::Body;
        app.select_block(0, 0);

        response::handle_response_keys(&mut app, key_char('G'));
        let last = app.json_tree_nodes.len().saturating_sub(1);
        assert_eq!(app.json_cursor, last);

        response::handle_response_keys(&mut app, key_char('j'));
        assert_eq!(app.json_cursor, last);

        response::handle_response_keys(&mut app, key_char('g'));
        assert_eq!(app.json_cursor, 0);
    }

    #[test]
    fn log_appends_and_autoscrolls() {
        let mut app = App::new();

        app.log(LogLevel::Info, "hello".to_string());

        assert_eq!(app.log_entries.len(), 1);
        assert_eq!(app.log_scroll, 0);
    }

    #[test]
    fn log_does_not_autoscroll_when_disabled() {
        let mut app = App::new();
        app.log_auto_scroll = false;
        app.log_scroll = 7;

        app.log(LogLevel::Info, "hello".to_string());

        assert_eq!(app.log_scroll, 7);
    }

    #[test]
    fn log_fifo_keeps_2000_entries() {
        let mut app = App::new();

        for i in 0..2005 {
            app.log(LogLevel::Info, format!("entry-{i}"));
        }

        assert_eq!(app.log_entries.len(), 2000);
        assert_eq!(app.log_entries.first().map(|entry| entry.message.as_str()), Some("entry-5"));
        assert_eq!(app.log_entries.last().map(|entry| entry.message.as_str()), Some("entry-2004"));
    }

    #[test]
    fn matches_filter_allows_all_for_all() {
        let entry = LogEntry {
            timestamp: "00:00:00".to_string(),
            level: LogLevel::Debug,
            message: "debug".to_string(),
        };

        assert!(matches_filter(&entry, &LogFilter::All));
    }

    #[test]
    fn matches_filter_info_excludes_debug() {
        let debug = LogEntry {
            timestamp: "00:00:00".to_string(),
            level: LogLevel::Debug,
            message: "debug".to_string(),
        };
        let info = LogEntry {
            timestamp: "00:00:00".to_string(),
            level: LogLevel::Info,
            message: "info".to_string(),
        };

        assert!(!matches_filter(&debug, &LogFilter::Info));
        assert!(matches_filter(&info, &LogFilter::Info));
    }

    #[test]
    fn matches_filter_warn_shows_warn_and_error() {
        let warn = LogEntry {
            timestamp: "00:00:00".to_string(),
            level: LogLevel::Warn,
            message: "warn".to_string(),
        };
        let error = LogEntry {
            timestamp: "00:00:00".to_string(),
            level: LogLevel::Error,
            message: "error".to_string(),
        };

        assert!(matches_filter(&warn, &LogFilter::Warn));
        assert!(matches_filter(&error, &LogFilter::Warn));
    }

    #[test]
    fn matches_filter_error_shows_only_error() {
        let warn = LogEntry {
            timestamp: "00:00:00".to_string(),
            level: LogLevel::Warn,
            message: "warn".to_string(),
        };
        let error = LogEntry {
            timestamp: "00:00:00".to_string(),
            level: LogLevel::Error,
            message: "error".to_string(),
        };

        assert!(!matches_filter(&warn, &LogFilter::Error));
        assert!(matches_filter(&error, &LogFilter::Error));
    }

    #[test]
    fn logs_key_handler_cycles_filter_and_auto_scroll() {
        let mut app = App::new();
        app.log(LogLevel::Debug, "debug".to_string());
        app.log(LogLevel::Info, "info".to_string());

        crate::components::logs::handle_logs_keys(&mut app, key_char('f'));
        assert_eq!(app.log_filter, LogFilter::Debug);
        assert_eq!(visible_log_count(&app), 2);

        crate::components::logs::handle_logs_keys(&mut app, key_char('f'));
        assert_eq!(app.log_filter, LogFilter::Info);
        assert_eq!(visible_log_count(&app), 1);

        crate::components::logs::handle_logs_keys(&mut app, key_char('a'));
        assert!(!app.log_auto_scroll);

        crate::components::logs::handle_logs_keys(&mut app, key_char('a'));
        assert!(app.log_auto_scroll);
    }

    #[test]
    fn logs_key_handler_clear_and_scroll_commands() {
        let mut app = App::new();
        for i in 0..30 {
            app.log(LogLevel::Info, format!("entry-{i}"));
        }

        crate::components::logs::handle_logs_keys(&mut app, key_char('g'));
        assert_eq!(app.log_scroll, 0);
        assert!(!app.log_auto_scroll);

        crate::components::logs::handle_logs_keys(&mut app, key_char('G'));
        assert_eq!(app.log_scroll, app.log_entries.len().saturating_sub(1));
        assert!(app.log_auto_scroll);

        crate::components::logs::handle_logs_keys(&mut app, key_char('c'));
        assert!(app.log_entries.is_empty());
        assert_eq!(app.log_scroll, 0);
    }

    #[test]
    fn code_editor_escape_returns_to_files_mode() {
        let mut app = App::new();
        app.mode = Mode::Code;
        app.focus = Focus::CodeView;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Esc));

        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.focus, Focus::FileTree);
    }

    #[test]
    fn code_editor_left_at_line_start_moves_to_previous_line_end() {
        let mut app = App::new();
        app.code_editor_content = "abc\ndef".to_string();
        app.code_editor_cursor_line = 1;
        app.code_editor_cursor_col = 0;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Left));

        assert_eq!(app.code_editor_cursor_line, 0);
        assert_eq!(app.code_editor_cursor_col, 3);
    }

    #[test]
    fn code_editor_right_at_line_end_moves_to_next_line() {
        let mut app = App::new();
        app.code_editor_content = "abc\ndef".to_string();
        app.code_editor_cursor_line = 0;
        app.code_editor_cursor_col = 3;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Right));

        assert_eq!(app.code_editor_cursor_line, 1);
        assert_eq!(app.code_editor_cursor_col, 0);
    }

    #[test]
    fn code_editor_up_and_down_clamp_cursor() {
        let mut app = App::new();
        app.code_editor_content = "abcd\nx".to_string();
        app.code_editor_cursor_line = 0;
        app.code_editor_cursor_col = 4;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Down));
        assert_eq!(app.code_editor_cursor_line, 1);
        assert_eq!(app.code_editor_cursor_col, 1);

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Up));
        assert_eq!(app.code_editor_cursor_line, 0);
        assert_eq!(app.code_editor_cursor_col, 1);
    }

    #[test]
    fn code_editor_home_and_end_move_to_bounds() {
        let mut app = App::new();
        app.code_editor_content = "abc".to_string();
        app.code_editor_cursor_col = 2;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Home));
        assert_eq!(app.code_editor_cursor_col, 0);

        code_editor::handle_editor_keys(&mut app, key(KeyCode::End));
        assert_eq!(app.code_editor_cursor_col, 3);
    }

    #[test]
    fn code_editor_enter_inserts_newline() {
        let mut app = App::new();
        app.code_editor_content = "abc".to_string();
        app.code_editor_cursor_col = 1;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Enter));

        assert_eq!(app.code_editor_content, "a\nbc");
        assert_eq!(app.code_editor_cursor_line, 1);
        assert_eq!(app.code_editor_cursor_col, 0);
        assert!(app.code_editor_modified);
    }

    #[test]
    fn code_editor_backspace_deletes_previous_char() {
        let mut app = App::new();
        app.code_editor_content = "abc".to_string();
        app.code_editor_cursor_col = 2;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Backspace));

        assert_eq!(app.code_editor_content, "ac");
        assert_eq!(app.code_editor_cursor_col, 1);
    }

    #[test]
    fn code_editor_backspace_merges_lines() {
        let mut app = App::new();
        app.code_editor_content = "abc\ndef".to_string();
        app.code_editor_cursor_line = 1;
        app.code_editor_cursor_col = 0;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Backspace));

        assert_eq!(app.code_editor_content, "abcdef");
        assert_eq!(app.code_editor_cursor_line, 0);
        assert_eq!(app.code_editor_cursor_col, 3);
    }

    #[test]
    fn code_editor_delete_removes_current_char() {
        let mut app = App::new();
        app.code_editor_content = "abc".to_string();
        app.code_editor_cursor_col = 1;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Delete));

        assert_eq!(app.code_editor_content, "ac");
    }

    #[test]
    fn code_editor_tab_inserts_two_spaces() {
        let mut app = App::new();
        app.code_editor_content = "abc".to_string();
        app.code_editor_cursor_col = 1;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Tab));

        assert_eq!(app.code_editor_content, "a  bc");
        assert_eq!(app.code_editor_cursor_col, 3);
    }

    #[test]
    fn code_editor_unicode_insert_is_char_safe() {
        let mut app = App::new();
        app.code_editor_content = "éx".to_string();
        app.code_editor_cursor_col = 1;

        code_editor::handle_editor_keys(&mut app, key_char('!'));

        assert_eq!(app.code_editor_content, "é!x");
        assert_eq!(app.code_editor_cursor_col, 2);
    }

    #[test]
    fn code_editor_unicode_backspace_removes_full_char() {
        let mut app = App::new();
        app.code_editor_content = "éx".to_string();
        app.code_editor_cursor_col = 1;

        code_editor::handle_editor_keys(&mut app, key(KeyCode::Backspace));

        assert_eq!(app.code_editor_content, "x");
        assert_eq!(app.code_editor_cursor_col, 0);
    }

    #[test]
    fn code_editor_ctrl_page_keys_adjust_scroll() {
        let mut app = App::new();
        app.code_editor_content = (0..40)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.code_editor_cursor_line = 20;

        code_editor::handle_editor_keys(&mut app, key_ctrl('u'));
        assert_eq!(app.code_editor_cursor_line, 5);

        code_editor::handle_editor_keys(&mut app, key_ctrl('d'));
        assert_eq!(app.code_editor_cursor_line, 20);
    }

    #[test]
    fn handle_key_q_sets_should_quit() {
        let mut app = App::new();

        handle_key(&mut app, key_char('q'));

        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_question_toggles_help() {
        let mut app = App::new();

        handle_key(&mut app, key_char('?'));
        assert!(app.show_help);

        handle_key(&mut app, key_char('?'));
        assert!(!app.show_help);
    }

    #[test]
    fn help_overlay_consumes_other_keys() {
        let mut app = app_with_file();
        app.show_help = true;
        app.tree_cursor = 1;

        handle_key(&mut app, key_char('q'));

        assert!(!app.should_quit);
        assert_eq!(app.tree_cursor, 1);
        assert!(app.show_help);
    }

    #[test]
    fn handle_key_mode_switches_files_history_logs() {
        let mut app = App::new();

        handle_key(&mut app, key_char('h'));
        assert_eq!(app.mode, Mode::History);
        assert_eq!(app.focus, Focus::HistoryList);

        handle_key(&mut app, key_char('l'));
        assert_eq!(app.mode, Mode::Logs);

        handle_key(&mut app, key_char('f'));
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.focus, Focus::FileTree);
    }

    #[test]
    fn handle_key_c_enters_code_mode() {
        let mut app = app_with_file();

        handle_key(&mut app, key_char('c'));

        assert_eq!(app.mode, Mode::Code);
        assert_eq!(app.code_editor_content, app.loaded_files[0].content);
    }

    #[test]
    fn handle_key_tab_cycles_focus_without_variables_tab() {
        let mut app = app_with_file();
        app.mode = Mode::Files;
        app.focus = Focus::FileTree;
        app.sidebar_tab = SidebarTab::Files;

        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Builder);
        assert_eq!(app.builder_focus, BuilderFocus::Method);

        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Builder);
        assert_eq!(app.builder_focus, BuilderFocus::Url);

        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Builder);
        assert_eq!(app.builder_focus, BuilderFocus::Headers);
    }

    #[test]
    fn handle_key_ctrl_v_toggles_sidebar_tab_and_focus() {
        let mut app = app_with_file();
        app.mode = Mode::Files;
        app.focus = Focus::FileTree;

        handle_key(&mut app, key_ctrl('v'));
        assert_eq!(app.sidebar_tab, SidebarTab::Variables);
        assert_eq!(app.focus, Focus::Variables);

        handle_key(&mut app, key_ctrl('v'));
        assert_eq!(app.sidebar_tab, SidebarTab::Files);
        assert_eq!(app.focus, Focus::FileTree);
    }

    #[test]
    fn handle_key_in_filter_input_does_not_quit() {
        let mut app = App::new();
        app.mode = Mode::History;
        app.focus = Focus::FilterInput;

        handle_key(&mut app, key_char('q'));

        assert!(!app.should_quit);
        assert_eq!(app.filter_text, "q");
    }

    #[test]
    fn code_mode_priority_prevents_global_q_and_edits_text() {
        let mut app = App::new();
        app.mode = Mode::Code;
        app.focus = Focus::CodeView;
        app.code_editor_content = String::new();

        handle_key(&mut app, key_char('q'));

        assert!(!app.should_quit);
        assert_eq!(app.code_editor_content, "q");
    }

    #[test]
    fn handle_key_t_cycles_theme() {
        let mut app = App::new();
        let initial = theme::active_name();

        handle_key(&mut app, key_char('T'));

        let cycled = theme::active_name();
        assert_ne!(cycled, initial);

        for _ in 0..theme::PALETTE_NAMES.len() - 1 {
            theme::cycle_next();
            if theme::active_name() == initial {
                break;
            }
        }
        assert_eq!(theme::active_name(), initial);
    }

    #[test]
    fn history_mode_keys_do_not_change_tree_cursor() {
        let mut app = app_with_history_entries();
        app.tree_cursor = 2;

        handle_key(&mut app, key_char('j'));

        assert_eq!(app.history_cursor, 1);
        assert_eq!(app.tree_cursor, 2);
    }

    #[test]
    fn history_tab_cycles_between_list_and_filter() {
        let mut app = App::new();
        app.mode = Mode::History;
        app.focus = Focus::HistoryList;

        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::FilterInput);

        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::HistoryList);
    }

    #[test]
    fn sidebar_enter_on_block_selects_code_view() {
        let mut app = app_with_file();
        let block_idx = app
            .tree_nodes
            .iter()
            .position(|node| matches!(node, TreeNode::Block { block_idx: 1, .. }))
            .unwrap();
        app.tree_cursor = block_idx;

        sidebar::handle_file_tree_keys(&mut app, key(KeyCode::Enter));

        assert_eq!(app.active_block_idx, Some(1));
        assert_eq!(app.focus, Focus::CodeView);
    }

    #[test]
    fn sidebar_space_toggles_file_expand_without_changing_focus() {
        let mut app = app_with_file();
        app.tree_cursor = 0;
        let starting_len = app.tree_nodes.len();

        sidebar::handle_file_tree_keys(&mut app, key_char(' '));

        assert!(!app.loaded_files[0].expanded);
        assert!(app.tree_nodes.len() < starting_len);
    }

    #[test]
    fn variables_keys_open_add_edit_and_delete_flows() {
        let mut app = App::new();
        app.focus = Focus::Variables;
        app.sidebar_tab = SidebarTab::Variables;
        app.env_vars.insert("Alpha".to_string(), "1".to_string());
        app.env_vars.insert("beta".to_string(), "2".to_string());

        sidebar::handle_variables_keys(&mut app, key_char('a'));
        assert!(matches!(app.input_mode, InputMode::Input { purpose: InputPurpose::AddVarName, .. }));

        app.input_mode = InputMode::Normal;
        sidebar::handle_variables_keys(&mut app, key_char('e'));
        assert!(matches!(app.input_mode, InputMode::Input { purpose: InputPurpose::EditVarValue { .. }, .. }));

        app.input_mode = InputMode::Normal;
        sidebar::handle_variables_keys(&mut app, key_char('d'));
        assert!(matches!(app.input_mode, InputMode::Confirm { purpose: ConfirmPurpose::DeleteVar { .. }, .. }));
    }
}
