use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::assertions::{self, AssertionResult};
use crate::http_client;
use crate::http_parser::{TestBlock, TestSuite};
use crate::variables::VariableStore;

/// Lightweight progress event emitted per-block during suite execution.
#[derive(Debug, Serialize, Clone)]
pub struct BlockProgress {
    pub seq: Option<u64>,
    pub name: String,
    pub block_type: String,
    pub status: String, // "running", "passed", "failed", "skipped", "error"
    pub time_ms: u64,
    pub assertion_passed: usize,
    pub assertion_total: usize,
    pub extract_ok: usize,
    pub extract_total: usize,
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

/// Trait for receiving real-time block execution progress.
/// Desktop GUI implements this with Tauri events, TUI with mpsc channels.
pub trait ProgressHandler: Send + Sync {
    fn on_block_start(&self, progress: &BlockProgress);
    fn on_block_complete(&self, progress: &BlockProgress);
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TestRunResults {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total_time_ms: u64,
    pub block_results: Vec<BlockResult>,
    pub final_variables: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BlockResult {
    pub seq: Option<u64>,
    pub name: String,
    pub block_type: String,
    pub request_method: String,
    pub request_url: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<String>,
    pub status: String, // "passed", "failed", "skipped", "error"
    pub response: Option<http_client::HttpResponse>,
    pub assertion_results: Vec<AssertionResult>,
    pub extract_results: Vec<ExtractResult>,
    pub error: Option<String>,
    pub time_ms: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExtractResult {
    pub variable: String,
    pub value: Option<String>,
    pub success: bool,
}

// ── Helper functions ──

fn make_progress(
    name: &str,
    block_type: &str,
    status: &str,
    time_ms: u64,
    assertion_passed: usize,
    assertion_total: usize,
    extract_ok: usize,
    extract_total: usize,
    http_status: Option<u16>,
    error: Option<&str>,
) -> BlockProgress {
    BlockProgress {
        seq: None,
        name: name.to_string(),
        block_type: block_type.to_string(),
        status: status.to_string(),
        time_ms,
        assertion_passed,
        assertion_total,
        extract_ok,
        extract_total,
        http_status,
        error: error.map(|e| e.to_string()),
    }
}

fn emit_start(handler: &Option<Arc<dyn ProgressHandler>>, name: &str, block_type: &str) {
    if let Some(ref h) = handler {
        let progress = make_progress(name, block_type, "running", 0, 0, 0, 0, 0, None, None);
        h.on_block_start(&progress);
    }
}

fn emit_completed(handler: &Option<Arc<dyn ProgressHandler>>, result: &BlockResult) {
    if let Some(ref h) = handler {
        let assertion_passed = result.assertion_results.iter().filter(|r| r.passed).count();
        let extract_ok = result.extract_results.iter().filter(|e| e.success).count();
        let http_status = result.response.as_ref().map(|r| r.status);
        let progress = make_progress(
            &result.name,
            &result.block_type,
            &result.status,
            result.time_ms,
            assertion_passed,
            result.assertion_results.len(),
            extract_ok,
            result.extract_results.len(),
            http_status,
            result.error.as_deref(),
        );
        h.on_block_complete(&progress);
    }
}

fn emit_skipped(
    handler: &Option<Arc<dyn ProgressHandler>>,
    name: &str,
    block_type: &str,
    reason: &str,
) {
    if let Some(ref h) = handler {
        let progress = make_progress(name, block_type, "skipped", 0, 0, 0, 0, 0, None, Some(reason));
        h.on_block_complete(&progress);
    }
}

fn make_skipped_result(block: &TestBlock, reason: &str) -> BlockResult {
    BlockResult {
        seq: None,
        name: block.name.clone(),
        block_type: block.block_type.clone(),
        request_method: block.request.method.clone(),
        request_url: block.request.url.clone(),
        request_headers: Vec::new(),
        request_body: None,
        status: "skipped".to_string(),
        response: None,
        assertion_results: Vec::new(),
        extract_results: Vec::new(),
        error: Some(reason.to_string()),
        time_ms: 0,
    }
}

/// Execute a single block with owned values (suitable for tokio::spawn).
async fn execute_block(block: TestBlock, var_store: VariableStore) -> BlockResult {
    let url = var_store.interpolate(&block.request.url);
    let headers: Vec<(String, String)> = block
        .request
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), var_store.interpolate(v)))
        .collect();
    let body = block
        .request
        .body
        .as_ref()
        .map(|b| var_store.interpolate(b));

    let start = std::time::Instant::now();
    let result =
        http_client::execute_request(&block.request.method, &url, &headers, body.as_deref()).await;
    let time_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(response) => {
            let extract_results: Vec<ExtractResult> = block
                .extracts
                .iter()
                .map(|extract| {
                    let value = assertions::resolve_response_value(
                        &extract.source_path,
                        response.status,
                        &response.headers,
                        &response.body,
                    );
                    ExtractResult {
                        variable: extract.variable_name.clone(),
                        success: value.is_some(),
                        value,
                    }
                })
                .collect();

            let assertion_results: Vec<AssertionResult> = block
                .assertions
                .iter()
                .map(|a| {
                    assertions::evaluate(a, response.status, &response.headers, &response.body)
                })
                .collect();
            let all_passed = assertion_results.iter().all(|r| r.passed);

            BlockResult {
                seq: None,
                name: block.name.clone(),
                block_type: block.block_type.clone(),
                request_method: block.request.method.clone(),
                request_url: url,
                request_headers: headers,
                request_body: body,
                status: if all_passed { "passed" } else { "failed" }.to_string(),
                response: Some(response),
                assertion_results,
                extract_results,
                error: None,
                time_ms,
            }
        }
        Err(err) => BlockResult {
            seq: None,
            name: block.name.clone(),
            block_type: block.block_type.clone(),
            request_method: block.request.method.clone(),
            request_url: url,
            request_headers: headers,
            request_body: body,
            status: "error".to_string(),
            response: None,
            assertion_results: Vec::new(),
            extract_results: Vec::new(),
            error: Some(err),
            time_ms,
        },
    }
}

/// Run test blocks with group-based parallelism using topological waves.
async fn run_tests_with_groups(
    tests: &[&TestBlock],
    var_store: &mut VariableStore,
    handler: &Option<Arc<dyn ProgressHandler>>,
) -> Vec<BlockResult> {
    // Assign each block to a group (blocks without @group get unique pseudo-groups)
    let mut group_blocks: HashMap<String, Vec<(usize, TestBlock)>> = HashMap::new();
    let mut block_group: HashMap<usize, String> = HashMap::new();

    for (i, block) in tests.iter().enumerate() {
        let group_name = block
            .group
            .clone()
            .unwrap_or_else(|| format!("__standalone_{}", i));
        group_blocks
            .entry(group_name.clone())
            .or_default()
            .push((i, (*block).clone()));
        block_group.insert(i, group_name);
    }

    // Build dependency graph: group → set of group names it depends on
    let mut group_deps: HashMap<String, HashSet<String>> = HashMap::new();
    for (i, block) in tests.iter().enumerate() {
        let group_name = block_group.get(&i).unwrap().clone();
        for dep in &block.depends {
            group_deps
                .entry(group_name.clone())
                .or_default()
                .insert(dep.clone());
        }
    }

    let all_group_names: HashSet<String> = group_blocks.keys().cloned().collect();
    let mut completed_groups: HashSet<String> = HashSet::new();
    let mut all_results: Vec<(usize, BlockResult)> = Vec::new();

    // Execute in topological waves
    loop {
        // Find groups that are ready: not completed AND all deps satisfied
        let ready: Vec<String> = all_group_names
            .iter()
            .filter(|g| !completed_groups.contains(*g))
            .filter(|g| {
                group_deps
                    .get(*g)
                    .map_or(true, |deps| deps.iter().all(|d| completed_groups.contains(d)))
            })
            .cloned()
            .collect();

        if ready.is_empty() {
            // Either all done, or circular dependency — skip remaining
            let remaining: Vec<String> = all_group_names
                .difference(&completed_groups)
                .cloned()
                .collect();
            for group_name in &remaining {
                if let Some(blocks) = group_blocks.remove(group_name) {
                    for (idx, block) in blocks {
                        emit_skipped(
                            handler,
                            &block.name,
                            &block.block_type,
                            "Skipped due to circular dependency",
                        );
                        all_results.push((
                            idx,
                            make_skipped_result(&block, "Skipped due to circular dependency"),
                        ));
                    }
                }
            }
            break;
        }

        // Collect all blocks from ready groups and spawn them concurrently
        let var_snapshot = var_store.clone();
        let mut join_set: JoinSet<(usize, BlockResult)> = JoinSet::new();

        for group_name in &ready {
            if let Some(blocks) = group_blocks.remove(group_name) {
                for (idx, block) in blocks {
                    let vs = var_snapshot.clone();
                    let h = handler.clone();
                    join_set.spawn(async move {
                        emit_start(&h, &block.name, &block.block_type);
                        let result = execute_block(block, vs).await;
                        emit_completed(&h, &result);
                        (idx, result)
                    });
                }
            }
        }

        // Await all parallel tasks in this wave
        while let Some(join_result) = join_set.join_next().await {
            if let Ok((idx, result)) = join_result {
                // Merge extracts back into var_store for next wave
                for er in &result.extract_results {
                    if er.success {
                        if let Some(ref v) = er.value {
                            var_store.set(&er.variable, v);
                        }
                    }
                }
                all_results.push((idx, result));
            }
        }

        // Mark wave groups as completed
        for group_name in ready {
            completed_groups.insert(group_name);
        }
    }

    // Sort by original index to maintain file order in results
    all_results.sort_by_key(|(idx, _)| *idx);
    all_results.into_iter().map(|(_, r)| r).collect()
}

/// Execute a full test suite: setup → test/request → teardown.
///
/// Pass an `Arc<dyn ProgressHandler>` to receive real-time progress events.
/// The `Arc` is required because the runner spawns parallel tasks that need
/// shared ownership of the handler.
pub async fn run_suite(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    progress: Option<Arc<dyn ProgressHandler>>,
    run_mode: Option<&str>,
) -> TestRunResults {
    run_suite_inner(suite, extra_variables, progress, run_mode).await
}

/// Inner implementation — emits block-progress events when handler is available.
async fn run_suite_inner(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    handler: Option<Arc<dyn ProgressHandler>>,
    run_mode: Option<&str>,
) -> TestRunResults {
    let mut var_store = VariableStore::from_pairs(&suite.variables);
    var_store.merge(extra_variables);

    // Partition blocks by type (preserving file order)
    let mut setups: Vec<&TestBlock> = Vec::new();
    let mut tests: Vec<&TestBlock> = Vec::new();
    let mut teardowns: Vec<&TestBlock> = Vec::new();
    for block in &suite.blocks {
        match block.block_type.as_str() {
            "setup" => setups.push(block),
            "teardown" => teardowns.push(block),
            _ => tests.push(block),
        }
    }

    // Filter blocks by run mode
    let mode_filter = |block: &&TestBlock| -> bool {
        match (&block.mode, &run_mode) {
            (None, _) => true,
            (Some(m), None) => m != "dev",
            (Some(m), Some(rm)) => m.as_str() == *rm,
        }
    };
    let setups: Vec<&TestBlock> = setups.into_iter().filter(mode_filter).collect();
    let tests: Vec<&TestBlock> = tests.into_iter().filter(mode_filter).collect();
    let teardowns: Vec<&TestBlock> = teardowns.into_iter().filter(mode_filter).collect();

    let mut block_results: Vec<BlockResult> = Vec::new();
    let mut setup_failed = false;
    let total_start = std::time::Instant::now();

    // ── Phase 1: Setup — sequential ──
    for block in &setups {
        emit_start(&handler, &block.name, &block.block_type);
        let result = execute_block((*block).clone(), var_store.clone()).await;
        // Merge extracts into var_store
        for er in &result.extract_results {
            if er.success {
                if let Some(ref v) = er.value {
                    var_store.set(&er.variable, v);
                }
            }
        }
        if result.status != "passed" {
            setup_failed = true;
        }
        emit_completed(&handler, &result);
        block_results.push(result);
        if setup_failed {
            break;
        }
    }

    // ── Phase 2: Tests — parallel with dependency graph ──
    if !setup_failed {
        let test_results = run_tests_with_groups(&tests, &mut var_store, &handler).await;
        block_results.extend(test_results);
    } else {
        // Skip all tests
        for block in &tests {
            emit_skipped(
                &handler,
                &block.name,
                &block.block_type,
                "Skipped due to setup failure",
            );
            block_results.push(make_skipped_result(block, "Skipped due to setup failure"));
        }
    }

    // ── Phase 3: Teardown — sequential (always runs) ──
    for block in &teardowns {
        emit_start(&handler, &block.name, &block.block_type);
        let result = execute_block((*block).clone(), var_store.clone()).await;
        for er in &result.extract_results {
            if er.success {
                if let Some(ref v) = er.value {
                    var_store.set(&er.variable, v);
                }
            }
        }
        emit_completed(&handler, &result);
        block_results.push(result);
    }

    let total_time_ms = total_start.elapsed().as_millis() as u64;
    let passed = block_results
        .iter()
        .filter(|r| r.status == "passed")
        .count();
    let failed = block_results
        .iter()
        .filter(|r| r.status == "failed" || r.status == "error")
        .count();
    let skipped = block_results
        .iter()
        .filter(|r| r.status == "skipped")
        .count();

    TestRunResults {
        passed,
        failed,
        skipped,
        total_time_ms,
        block_results,
        final_variables: var_store.to_map(),
    }
}

/// Run only `@setup` blocks to resolve variables, returning the final variable map.
pub async fn resolve_variables_only(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    run_mode: Option<&str>,
) -> Result<HashMap<String, String>, String> {
    let mut var_store = VariableStore::from_pairs(&suite.variables);
    var_store.merge(extra_variables);

    let mode_filter = |b: &&TestBlock| -> bool {
        match (&b.mode, &run_mode) {
            (None, _) => true,
            (Some(m), None) => m != "dev",
            (Some(m), Some(rm)) => m.as_str() == *rm,
        }
    };

    let mut setup_blocks: Vec<&TestBlock> = suite
        .blocks
        .iter()
        .filter(|b| b.block_type == "setup")
        .filter(mode_filter)
        .collect();
    setup_blocks.sort_by_key(|b| suite.blocks.iter().position(|sb| std::ptr::eq(sb, *b)));

    for block in &setup_blocks {
        let url = var_store.interpolate(&block.request.url);
        let headers: Vec<(String, String)> = block
            .request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), var_store.interpolate(v)))
            .collect();
        let body = block
            .request
            .body
            .as_ref()
            .map(|b| var_store.interpolate(b));

        let result =
            http_client::execute_request(&block.request.method, &url, &headers, body.as_deref())
                .await;

        match result {
            Ok(response) => {
                for extract in &block.extracts {
                    let value = assertions::resolve_response_value(
                        &extract.source_path,
                        response.status,
                        &response.headers,
                        &response.body,
                    );
                    if let Some(v) = value {
                        var_store.set(&extract.variable_name, &v);
                    }
                }
            }
            Err(err) => {
                var_store.set(
                    &format!("__error_{}", block.name),
                    &format!("Setup block '{}' failed: {}", block.name, err),
                );
            }
        }
    }

    Ok(var_store.to_map())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_type_order(block_type: &str) -> u8 {
        match block_type {
            "setup" => 0,
            "test" | "request" => 1,
            "teardown" => 2,
            _ => 1,
        }
    }

    #[test]
    fn block_type_order_setup() {
        assert_eq!(block_type_order("setup"), 0);
    }

    #[test]
    fn block_type_order_test() {
        assert_eq!(block_type_order("test"), 1);
    }

    #[test]
    fn block_type_order_request() {
        assert_eq!(block_type_order("request"), 1);
    }

    #[test]
    fn block_type_order_teardown() {
        assert_eq!(block_type_order("teardown"), 2);
    }

    #[test]
    fn block_type_order_unknown_defaults_to_1() {
        assert_eq!(block_type_order("unknown"), 1);
        assert_eq!(block_type_order(""), 1);
    }

    #[test]
    fn block_type_order_sorting_correctness() {
        let mut types = vec!["teardown", "test", "setup", "request"];
        types.sort_by_key(|t| block_type_order(t));
        assert_eq!(types, vec!["setup", "test", "request", "teardown"]);
    }

    #[test]
    fn test_run_results_struct_defaults() {
        let results = TestRunResults {
            passed: 0,
            failed: 0,
            skipped: 0,
            total_time_ms: 0,
            block_results: Vec::new(),
            final_variables: HashMap::new(),
        };
        assert_eq!(results.passed, 0);
        assert!(results.block_results.is_empty());
    }

    #[test]
    fn block_result_status_values() {
        let result = BlockResult {
            seq: None,
            name: "test".to_string(),
            block_type: "test".to_string(),
            request_method: "GET".to_string(),
            request_url: "http://example.com".to_string(),
            request_headers: Vec::new(),
            request_body: None,
            status: "passed".to_string(),
            response: None,
            assertion_results: Vec::new(),
            extract_results: Vec::new(),
            error: None,
            time_ms: 0,
        };
        assert_eq!(result.status, "passed");
        assert!(result.error.is_none());
        assert!(result.seq.is_none());
    }

    #[test]
    fn extract_result_fields() {
        let er = ExtractResult {
            variable: "token".to_string(),
            value: Some("abc123".to_string()),
            success: true,
        };
        assert!(er.success);
        assert_eq!(er.value.as_deref(), Some("abc123"));
    }

    use crate::http_parser::{Extract, ParsedRequest};

    fn make_block(
        block_type: &str,
        name: &str,
        extracts: Vec<Extract>,
        depends: Vec<String>,
        group: Option<&str>,
    ) -> TestBlock {
        TestBlock {
            block_type: block_type.to_string(),
            name: name.to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: group.map(|s| s.to_string()),
            depends,
            request: ParsedRequest {
                name: Some(name.to_string()),
                method: "GET".to_string(),
                url: "http://127.0.0.1:0/nonexistent".to_string(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts,
        }
    }

    #[tokio::test]
    async fn test_resolve_variables_only_includes_static_vars() {
        let suite = TestSuite {
            variables: vec![
                ("host".to_string(), "example.com".to_string()),
                ("token".to_string(), "abc123".to_string()),
            ],
            blocks: Vec::new(),
        };
        let result = resolve_variables_only(&suite, &[], None).await.unwrap();
        assert_eq!(result.get("host").unwrap(), "example.com");
        assert_eq!(result.get("token").unwrap(), "abc123");
    }

    #[tokio::test]
    async fn test_resolve_variables_only_merges_extra_variables() {
        let suite = TestSuite {
            variables: vec![("host".to_string(), "example.com".to_string())],
            blocks: Vec::new(),
        };
        let extras = vec![("env".to_string(), "staging".to_string())];
        let result = resolve_variables_only(&suite, &extras, None).await.unwrap();
        assert_eq!(result.get("host").unwrap(), "example.com");
        assert_eq!(result.get("env").unwrap(), "staging");
    }

    #[tokio::test]
    async fn test_resolve_variables_only_skips_test_blocks() {
        // Test and teardown blocks should NOT be executed.
        // We use an unreachable address; if the function tried to call it,
        // it would store an error key. No error key ⇒ block was skipped.
        let suite = TestSuite {
            variables: vec![("base".to_string(), "val".to_string())],
            blocks: vec![
                make_block("test", "my_test", Vec::new(), vec![], None),
                make_block("teardown", "my_teardown", Vec::new(), vec![], None),
            ],
        };
        let result = resolve_variables_only(&suite, &[], None).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("base").unwrap(), "val");
        // No error keys from test/teardown blocks
        assert!(!result.keys().any(|k| k.starts_with("__error_")));
    }

    #[tokio::test]
    async fn test_resolve_variables_only_runs_setup_blocks() {
        // A setup block pointing to an unreachable address will fail,
        // but the function should continue and store an error variable.
        let suite = TestSuite {
            variables: vec![("static_var".to_string(), "hello".to_string())],
            blocks: vec![make_block(
                "setup",
                "auth_setup",
                vec![Extract {
                    variable_name: "token".to_string(),
                    source_path: "$.token".to_string(),
                }],
                vec![],
                None,
            )],
        };
        let result = resolve_variables_only(&suite, &[], None).await.unwrap();
        assert_eq!(result.get("static_var").unwrap(), "hello");
        // Setup block should have been attempted; connection refusal ⇒ error key
        assert!(
            result.contains_key("__error_auth_setup"),
            "Expected error key for failed setup block"
        );
    }

    #[tokio::test]
    async fn test_groups_run_in_parallel_order() {
        // Two groups with no deps — both should run (order doesn't matter for correctness)
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "A", vec![], vec![], Some("group1")),
                make_block("test", "B", vec![], vec![], Some("group1")),
                make_block("test", "C", vec![], vec![], Some("group2")),
            ],
        };
        let results = run_suite_inner(&suite, &[], None, None).await;
        assert_eq!(results.block_results.len(), 3);
        for r in &results.block_results {
            assert_ne!(r.status, "skipped");
        }
    }

    #[tokio::test]
    async fn test_depends_runs_after_dependency() {
        // group-b depends on group-a
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "A1", vec![], vec![], Some("group-a")),
                make_block(
                    "test",
                    "B1",
                    vec![],
                    vec!["group-a".to_string()],
                    Some("group-b"),
                ),
            ],
        };
        let results = run_suite_inner(&suite, &[], None, None).await;
        assert_eq!(results.block_results.len(), 2);
        for r in &results.block_results {
            assert_ne!(r.status, "skipped");
        }
    }

    #[tokio::test]
    async fn test_standalone_blocks_run_parallel() {
        // Blocks without @group get unique pseudo-groups and run in parallel
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "Solo1", vec![], vec![], None),
                make_block("test", "Solo2", vec![], vec![], None),
                make_block("test", "Solo3", vec![], vec![], None),
            ],
        };
        let results = run_suite_inner(&suite, &[], None, None).await;
        assert_eq!(results.block_results.len(), 3);
    }

    #[tokio::test]
    async fn test_setup_sequential_then_tests_parallel() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("setup", "Setup1", vec![], vec![], None),
                make_block("test", "T1", vec![], vec![], Some("tests")),
                make_block("test", "T2", vec![], vec![], Some("tests")),
                make_block("teardown", "Cleanup", vec![], vec![], None),
            ],
        };
        let results = run_suite_inner(&suite, &[], None, None).await;
        // Setup fails (unreachable addr) → tests skipped, but teardown always runs
        assert_eq!(results.block_results.len(), 4);
    }

    #[tokio::test]
    async fn test_circular_dependency_skips_blocks() {
        // group-a depends on group-b and vice versa → circular
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block(
                    "test",
                    "A",
                    vec![],
                    vec!["group-b".to_string()],
                    Some("group-a"),
                ),
                make_block(
                    "test",
                    "B",
                    vec![],
                    vec!["group-a".to_string()],
                    Some("group-b"),
                ),
            ],
        };
        let results = run_suite_inner(&suite, &[], None, None).await;
        assert_eq!(results.block_results.len(), 2);
        for r in &results.block_results {
            assert_eq!(r.status, "skipped");
        }
    }

    #[tokio::test]
    async fn test_teardown_always_runs_after_setup_failure() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("setup", "FailSetup", vec![], vec![], None),
                make_block("test", "T1", vec![], vec![], None),
                make_block("teardown", "Cleanup", vec![], vec![], None),
            ],
        };
        let results = run_suite_inner(&suite, &[], None, None).await;
        assert_eq!(results.block_results.len(), 3);
        // Setup errors (connection refused)
        assert_eq!(results.block_results[0].status, "error");
        // Test skipped
        assert_eq!(results.block_results[1].status, "skipped");
        // Teardown still runs (will error too, but it runs)
        assert_ne!(results.block_results[2].status, "skipped");
    }

    #[tokio::test]
    async fn test_results_preserve_file_order() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "First", vec![], vec![], Some("g1")),
                make_block("test", "Second", vec![], vec![], Some("g2")),
                make_block("test", "Third", vec![], vec![], Some("g3")),
            ],
        };
        let results = run_suite_inner(&suite, &[], None, None).await;
        assert_eq!(results.block_results[0].name, "First");
        assert_eq!(results.block_results[1].name, "Second");
        assert_eq!(results.block_results[2].name, "Third");
    }
}
