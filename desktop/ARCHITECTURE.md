# Desktop App Architecture

## Overview

Tauri v2 app: Rust backend (`src-tauri/src/`) + HTML/CSS/JS frontend (`ui/`).
IPC via Tauri commands (`invoke`) and events (`emit`/`listen`).

## Rust Backend Modules

### lib.rs — Tauri Command Registry

- All `#[tauri::command]` functions registered in `generate_handler![]`
- Commands: `parse_test_file`, `run_test_suite`, `send_request`, `check_variables`, `suggest_urls`, `suggest_domain_paths`, `get_history`, `get_filtered_history`, `clear_history`, `resolve_variables`, `parse_http_file`, `generate_http`, `read_env_file`, `write_env_file`
- State: `AppState` with `Mutex<History>` and `Mutex<UrlTrie>`

### http_parser.rs — .http File Parser

- Parses enhanced `.http` format into `TestSuite { variables, blocks }`
- `TestBlock`: name, block_type (setup/test/teardown), request (method/url/headers/body), assertions, extracts, group, depends, description, disabled
- Smart URL parsing: first word = method, checks last token for `HTTP/x.x` version, rest = URL (preserves spaces in query params)
- Body parsing strips trailing comment lines (section decorations) but preserves mid-body `#` (YAML)
- Key types: `TestSuite`, `TestBlock`, `RequestData`, `Assertion`, `Extract`

### http_client.rs — HTTP Request Execution

- `execute_request()` via reqwest with rustls
- `encode_url()` percent-encodes illegal URI chars in query strings only
- Returns `HttpResponse { status, status_text, headers, body, time_ms, size_bytes }`

### test_runner.rs — Test Execution Engine

- 3-phase execution: Setup (sequential) → Tests (parallel) → Teardown (sequential, always runs)
- Parallel tests via `tokio::task::JoinSet` with topological wave scheduling
- `@group`/`@depends` dependency graph: find groups with satisfied deps → spawn wave → await → merge extracts → repeat
- `BlockProgress` events streamed via Tauri `emit` during execution
- Key types: `BlockResult`, `TestRunResults`, `BlockProgress`, `AssertionResult`, `ExtractResult`

### assertions.rs — Assertion Evaluation

- `evaluate()` resolves left/right values against response (status, headers, body via JSONPath)
- Operators: `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`, `!contains`, `exists`, `!exists`
- `resolve_response_value()` handles: `response.status`, `response.headers.X`, `response.body`, `response.body.path.to.field`

### variables.rs — Variable Store

- `VariableStore`: `HashMap<String, String>` with `interpolate()`, `set()`, `merge()`
- Resolves `{{varName}}` in strings, supports built-in vars: `$timestamp`, `$uuid`, `$randomInt`
- Clone-able for parallel execution (each task gets snapshot, extracts merged after wave)

### history.rs — Request History

- Stores all requests (manual + test runs) with auto-incrementing seq numbers
- `search()`, `get_by_seq()`, `clear()`
- `HistoryEntry`: seq, method, url, status, headers, body, time, source, block_name, run_id

### url_trie.rs — URL Autocomplete Trie

- Segment-aware trie (splits by `/` and `.`)
- `suggest()` for prefix-based URL completions
- `suggest_domain_paths()` strips query strings, aggregates by domain+path, returns top N
- `DomainPathSuggestion { domain_path, frequency, url_count }`

### env_file.rs — .env File Parser

- Parses `KEY=VALUE` format with comment and empty line handling

## Frontend (ui/)

### app.js — Main Application Logic (~3300 lines)

Key sections (search by function name):

- **State**: `loadedFiles`, `activeFileIndex`, `activeBlockIndex`, `disabledBlocks`, `envVars`, `runHistory`, `viewedResults`, `treeExpandState`
- **File tree**: `renderFileTree()` — hierarchical with group nodes, `createBlockItem()`, `updateBlockStatuses()`
- **Run group**: `runGroup(fileIdx, groupName)` — runs setup + group blocks + teardown
- **JSON tree viewer**: `renderJsonTree()`, `renderJsonNode()` — recursive DOM with syntax highlighting
- **Tooltips**: `showBlockTooltip()`, `showGroupTooltip()`, `showFileTooltip()`, `showAllFilesTooltip()`, `positionTooltip()`
- **Autocomplete**: `fetchAutocompleteSuggestions()`, `renderAutocomplete()`, `updateGhostText()`
- **Test execution**: `runAllTests()`, `runSingleFile()`, `runSingleBlock()`
- **Block progress streaming**: `startBlockProgressListener()` — updates DOM in-place during execution
- **History**: `renderHistoryStats()`, `renderHistoryLog()`, `createHistoryEntryRow()`, `showHistoryDetail()`
- **Run history**: `pushRunHistory()`, `getBlockRunHistory()`, `getFileRunStats()`, `buildTrendBar()`

### styles.css — All Styles (~2500 lines)

- CSS variables at top for theming (dark IDE theme)
- Key sections: sidebar, file tree, group nodes, block items, tooltips, response body, JSON tree, history, autocomplete

### index.html — Single Page

- Sidebar (files + env) | Main area (toolbar, request editor, response viewer, history)
- `#blockTooltip` div for hover popovers

## Data Flow

1. **User loads .http file** → JS reads file → `invoke('parse_test_file')` → Rust parser → `TestSuite` returned to JS
2. **User clicks Run** → JS builds enabled suite → `invoke('run_test_suite')` → Rust runner executes 3-phase
3. **During execution**: Rust emits `'block-progress'` events → JS listener updates DOM in-place
4. **After completion**: Results returned → JS remaps to original indices → updates sidebar status dots
5. **History**: Each request stored in Rust `History` → JS queries via `invoke('search_history')`
6. **Autocomplete**: Each URL added to Rust `UrlTrie` → JS queries `suggest`/`suggest_domain_paths`

## Key Patterns for Agents

- **Adding a new Tauri command**: Define fn in relevant module → add `#[tauri::command]` wrapper in `lib.rs` → register in `generate_handler![]` → call from JS via `invoke('command_name', { args })`
- **Adding a new directive**: Update `http_parser.rs` parse logic → add field to `TestBlock` → handle in `test_runner.rs` → expose in JS tooltip/UI
- **Modifying test execution**: `test_runner.rs` `run_suite_inner()` for phases, `run_tests_with_groups()` for parallel logic
- **UI changes**: All in `app.js` (vanilla JS, no framework). DOM manipulation + Tauri IPC. Styles in `styles.css`.
- **File locking on Windows**: Use `$env:CARGO_BUILD_JOBS="1"` or clean `target/debug/deps` before building
