# Request Pilot — Architecture Overview

This document is for **developers and AI agents** working on the codebase. It maps the repository structure, explains how the pieces connect, and provides pointers for common modifications.

## Repository Layout

```
request-pilot/
├── core/                  # Shared Rust core library (223 tests)
│   └── src/
│       ├── http_parser.rs # .http file parser (TestSuite, TestBlock)
│       ├── http_client.rs # reqwest-based HTTP execution
│       ├── test_runner.rs # 3-phase test engine (setup→parallel tests→teardown)
│       ├── assertions.rs  # Assertion evaluation (==, contains, exists, etc.)
│       ├── variables.rs   # VariableStore with {{interpolation}}
│       ├── history.rs     # Request history storage & search
│       ├── url_trie.rs    # Segment-aware URL autocomplete trie
│       ├── env_file.rs    # .env file parser/writer
│       ├── azure_auth.rs  # Azure CLI + device code auth
│       ├── telemetry.rs   # OTEL telemetry (OTLP export)
│       ├── env_file.rs    # .env file parser/writer
│       └── otlp_proto.rs  # OTLP protobuf encoding (internal)
│
├── extension/             # Browser extension (Edge / Chrome)
│   ├── ARCHITECTURE.md    # Extension internals
│   ├── README.md          # Extension features & usage
│   ├── manifest.json      # Manifest V3
│   ├── background.js      # Service worker (rules, DNR, logging)
│   ├── popup.html/css/js  # Popup UI (rules, log, stats)
│   ├── content-script.js  # ISOLATED world — message relay
│   └── content-script-main.js  # MAIN world — response body capture
│
├── desktop/               # Tauri v2 desktop app
│   ├── ARCHITECTURE.md    # Detailed module-by-module guide
│   ├── README.md          # Desktop features & usage
│   ├── src-tauri/src/     # Rust backend (thin wrapper over core)
│   │   ├── lib.rs         # Tauri command registry + AppState
│   │   ├── live_capture.rs# WebSocket server for browser extension live capture
│   │   └── main.rs        # Entry point
│   └── ui/                # Frontend (vanilla HTML/CSS/JS)
│       ├── index.html     # Single page — sidebar + main area
│       ├── app.js         # All application logic (~5200+ lines)
│       └── styles.css     # All styles (~3600+ lines, dark IDE theme)
│
├── tui/                   # Terminal UI (ratatui + crossterm, 113 tests)
│   └── src/
│       ├── main.rs            # CLI argument parsing (clap), terminal init
│       ├── app.rs             # App struct (~1600 lines), state, run loop, runner messages
│       ├── events.rs          # Keyboard event handling (~500 lines)
│       ├── ui.rs              # Main draw function, top bar, splash screen, themes, progress
│       ├── code_editor.rs     # Full-screen code editor with syntax highlighting
│       ├── live_capture.rs    # WebSocket server for extension connector (port 9718)
│       ├── toolbar.rs         # Popup overlays: extra headers, Azure, OTEL, live capture
│       ├── tests.rs           # 113 unit tests
│       └── components/
│           ├── mod.rs
│           ├── sidebar.rs     # File tree with groups, compare icons, auto-scroll
│           ├── builder.rs     # Request builder + compare-aware step view
│           ├── response.rs    # Response viewer with Body/Headers/Assertions + compare support
│           ├── history.rs     # History mode with grouping, filters, detail overlay
│           ├── diff_viewer.rs # Side-by-side diff with LCS algorithm
│           ├── logs.rs        # Log viewer with level filters
│           └── overlays.rs    # Help overlay content
│
├── docs/                  # Documentation & samples
│   ├── http-file-format.md    # .http format reference
│   └── samples/               # Example .http files (Azure, Prometheus)
│
├── tests/                 # Extension unit tests (Jest, 97 tests)
│   ├── __tests__/
│   └── jest.config.js
│
├── .github/skills/        # Copilot skills
│   └── e2e-http-test-generator/SKILL.md
│
├── Cargo.toml             # Workspace root (members: core, desktop, tui)
├── README.md              # Project overview & features
└── ARCHITECTURE.md        # ← You are here
```

## Three Products, One Core

| Component | Tech | Purpose |
|-----------|------|---------|
| **Shared Core** | Rust (`request-pilot-core`) | HTTP parsing, execution, test runner, assertions, variables, history, telemetry |
| **Desktop App** | Tauri v2 (core + HTML/JS) | GUI HTTP client + E2E test framework for `.http` files |
| **Terminal UI** | ratatui (core + crossterm) | Headless terminal interface for `.http` testing |
| **Browser Extension** | Manifest V3, vanilla JS | Intercept/modify HTTP traffic in the browser — header injection, blocking, redirects, traffic analysis |

The desktop app and TUI share the `request-pilot-core` Rust crate, which contains all HTTP parsing, execution, and test logic. The browser extension is an independent codebase with no Rust dependency.

## Desktop App — How It Works

```
┌─────────────────────────────────────────────────────┐
│                    Frontend (ui/)                     │
│  app.js ←→ Tauri IPC (invoke/listen) ←→ Rust backend │
└─────────────────────────────────────────────────────┘

User loads .http file
  → JS: fileInput.change → file.text()
  → invoke('parse_test_file', { content })
  → Rust: http_parser::parse() → TestSuite { variables, blocks }
  → JS: stores in loadedFiles[], renders sidebar tree

User clicks Run All
  → JS: runAllTests() → for each file: invoke('run_test_suite', { suite, extraVariables, extraHeaders, runMode, fileName })
  → Rust: test_runner::run_suite_with_headers()
    Phase 1: Setup blocks (sequential)
    Phase 2: Test blocks (parallel via JoinSet, topological wave scheduling + bounded by file-level `# @@parallel <N>` so in-flight task count <= cap)
    Phase 3: Teardown blocks (sequential, always runs)
  → During execution: Rust emits 'block-progress' events
  → JS: startBlockProgressListener() updates DOM in-place
  → After: Results returned, JS remaps to original indices, updates sidebar dots

Response display
  → JS: renderJsonTree(parsed) builds interactive DOM tree
  → Each object/array is collapsible with ▾/▸ toggle
  → Syntax highlighting via CSS classes (json-key, json-string, json-number, etc.)
```

### IPC Commands (lib.rs → invoke from JS)

| Command | Module | Purpose |
|---------|--------|---------|
| `send_request` | http_client | Single HTTP request + history recording |
| `parse_http_file` | http_parser | Parse .http content → ParsedRequest list |
| `parse_test_file` | http_parser | Parse .http content → TestSuite |
| `generate_http` | http_parser | Generate .http content from TestSuite |
| `run_test_suite` | test_runner | Execute suite, stream progress, record history |
| `resolve_variables` | test_runner | Resolve all variables without running requests |
| `check_variables` | variables | Find unresolved {{vars}} in a suite |
| `get_history` | history | Get all history entries |
| `get_filtered_history` | history | Query history with filters |
| `clear_history` | history | Clear all history |
| `get_history_distinct_values` | history | Distinct file names, groups, and block names |
| `suggest_urls` | url_trie | URL prefix autocomplete |
| `suggest_domain_paths` | url_trie | Domain→path top-N suggestions |
| `read_env_file` | env_file | Read .env file into key-value map |
| `write_env_file` | env_file | Write key-value map to .env file |
| `fetch_azure_token` | azure_auth | Fetch Azure token via CLI |
| `check_azure_cli` | azure_auth | Check if `az` CLI is available |
| `start_device_code` | azure_auth | Start OAuth device code flow |
| `poll_device_code` | azure_auth | Poll for device code token completion |
| `start_live_capture` | live_capture | Start WebSocket server on port 9718 for browser extension |
| `stop_live_capture` | live_capture | Stop the live capture WebSocket server |
| `set_live_capture_mode` | live_capture | Set capture mode: `off`, `all`, or `filtered` |
| `get_live_capture_status` | live_capture | Query current mode, running, and connected status |

### IPC Events (Rust → JS)

| Event | Payload | Purpose |
|-------|---------|---------|
| `block-progress` | BlockProgress | Per-block status during test run |
| `live-request` | CapturedRequest | Forwarded HTTP request from browser extension |
| `live-connection-status` | String (`listening`, `connected`, `disconnected`, `stopped`) | WebSocket connection state changes |

## Terminal UI — How It Works

```
┌────────────────────────────────────────────────────────┐
│              TUI (ratatui + crossterm)                   │
│  main.rs → App::new() → App::run() event loop           │
│    ↕ 100ms poll tick    ↕ RunnerMessage mpsc channel     │
│  events.rs (keys)     app.rs (state + async results)     │
│    ↕ Mode/Focus       ↕ tokio::spawn                     │
│  ui.rs → draw() dispatches by Mode to components/        │
└────────────────────────────────────────────────────────┘
```

### App State

The `App` struct (~1600 lines in `app.rs`) is the single source of truth for all TUI state. Key enums:

| Enum | Purpose |
|------|---------|
| `Mode` | Top-level screen: `Normal`, `History`, `Logs`, `Editor`, `Diff` |
| `Focus` | Active panel in Normal mode: `Sidebar`, `Builder`, `Response` |
| `BuilderFocus` | Active field in the request builder: `Url`, `Method`, `Headers`, `Body`, etc. |

State includes loaded files, selected block indices, response data, history entries, filter states, popup visibility flags, and all component-specific state.

### Event Loop

```
main.rs: terminal init → App::run()
  loop {
    1. terminal.draw(|f| draw(f, &mut app))     // immediate-mode render
    2. poll(Duration::from_millis(100))           // crossterm event poll
    3. if key event → handle_key_event(&mut app)  // events.rs dispatch
    4. while let Ok(msg) = runner_rx.try_recv()   // drain RunnerMessage channel
         → app.handle_runner_message(msg)
  }
```

The event loop runs on the tokio async runtime. Test execution and Azure auth run on background `tokio::spawn` tasks, sending results back via an `mpsc::UnboundedSender<RunnerMessage>` channel.

### Rendering

All rendering uses ratatui's immediate-mode approach — the entire screen is redrawn every frame. The top-level `draw()` function in `ui.rs`:

1. Draws the **top bar** (file name, mode indicator, keybind hints)
2. Dispatches to the active mode's renderer:
   - `Normal` → sidebar + builder + response (3-panel layout)
   - `History` → history list + detail overlay
   - `Logs` → log viewer with level filters
   - `Editor` → full-screen code editor
   - `Diff` → side-by-side diff viewer
3. Draws any active **popup overlays** (toolbar.rs) on top

### Components

Each file in `components/` owns rendering and key handling for its area:

| Component | File | Responsibility |
|-----------|------|---------------|
| Sidebar | `sidebar.rs` | File tree with expandable groups, compare icons, status dots, auto-scroll to active block |
| Builder | `builder.rs` | Request builder showing URL, method, headers, body; compare-aware step view during test runs |
| Response | `response.rs` | Response viewer with Body/Headers/Assertions tabs; compare support for side-by-side results |
| History | `history.rs` | History mode with date grouping, method/status/source filters, detail overlay |
| Diff Viewer | `diff_viewer.rs` | Side-by-side diff with LCS algorithm, line-level change highlighting |
| Logs | `logs.rs` | Log viewer with level filters (info, warn, error) |
| Overlays | `overlays.rs` | Help overlay content and keybind reference |

### IPC / Messages

The `RunnerMessage` enum carries async results from background tasks to the event loop:

| Variant | Purpose |
|---------|---------|
| `BlockStart { seq, name }` | A test block has started executing |
| `BlockComplete { seq, name, status, time_ms, assertions, http_status, error }` | A test block finished (pass/fail/error) |
| `SuiteComplete { file_idx, file_id, results }` | All blocks in a file finished; final results |
| `AzureAuthResult { ... }` | Azure token fetch completed |
| `AzureCliCheck { available }` | Azure CLI availability check result |
| `LiveCaptureRequest(CapturedRequest)` | HTTP request received from browser extension |
| `LiveCaptureStatus { connected }` | WebSocket connection state changed |

### Extension Connector (Live Capture)

`live_capture.rs` runs a WebSocket server on `127.0.0.1:9718` using tokio-tungstenite. A `LiveCaptureState` (wrapped in `Arc<Mutex<>>`) tracks the connection and capture mode. Incoming requests from the browser extension are deserialized and forwarded to the event loop as `RunnerMessage::LiveCaptureRequest`. The same protocol and port as the desktop app's live capture module.

### Themes

Three runtime-switchable color palettes (Dark, Light, Solarized) controlled by an `AtomicU8`. All colors are accessed through `theme::` functions (e.g., `theme::fg()`, `theme::accent()`, `theme::border()`), so components never hardcode colors. Users cycle themes with a keybind; the change takes effect on the next frame.

### Code Editor

`code_editor.rs` provides a full-screen text editor with:
- Syntax highlighting for `.http` files (keywords, headers, JSON bodies)
- Search (`Ctrl+F`) with match highlighting and navigation
- Go-to-line (`Ctrl+G`)
- Text selection, copy, cut, paste
- Line numbers and scroll tracking

### Test Runner Integration

The TUI reuses `core::test_runner` for execution. When the user triggers a run:

1. `app.rs` clones the parsed `TestSuite` and spawns a `tokio::spawn` task
2. The task calls `run_suite_with_headers()`, streaming `BlockProgress` events
3. Each progress event is mapped to a `RunnerMessage` and sent via the mpsc channel
4. The event loop drains the channel, updating sidebar status dots and response data in real time

## Browser Extension — How It Works

```
┌──────────────────────────────────────────┐
│  popup.js (UI) ↔ background.js (worker)  │
│           ↕ chrome.runtime.sendMessage    │
│  content-script-main.js (response capture)│
│           ↕ window.postMessage            │
│  content-script.js (message relay)        │
└──────────────────────────────────────────┘
```

- **Rules** stored in `chrome.storage.local`, compiled to `declarativeNetRequest` rules
- **Network log** captured via `webRequest` API listeners
- **Response bodies** intercepted by patching `fetch`/`XHR` in MAIN world content script
- **No backend server** — everything runs in the browser

## Live Capture — Extension ↔ Desktop Bridge

The desktop app runs a WebSocket server on `127.0.0.1:9718` (module: `live_capture.rs`). The browser extension connects as a client and forwards intercepted HTTP requests as JSON messages (`{ "action": "request", "data": CapturedRequest }`). The desktop app can push mode changes (`SetMode`) back to the extension. Three capture modes are supported: `off`, `all` (every request), and `filtered` (only requests matching extension rules). Captured requests are emitted to the frontend via Tauri events, auto-appended to a virtual `.http` file, and recorded in history with the `extension-live` source tag.

## For AI Agents: Common Tasks

### Adding a Tauri Command
1. Write function in the relevant Rust module
2. Add `#[tauri::command]` wrapper in `lib.rs`
3. Register in `generate_handler![]` macro in `lib.rs`
4. Call from JS: `await invoke('command_name', { param1, param2 })`

### Adding a New .http Directive
1. `core/src/http_parser.rs` — parse the `# @directive` line, add field to `TestBlock`
2. `core/src/test_runner.rs` — handle the directive during execution
3. `desktop/ui/app.js` — display in sidebar tooltip (`showBlockTooltip`)
4. `docs/http-file-format.md` — document the directive
5. `SKILL.md` — update the generation skill if applicable

### Modifying Test Execution
- `core/src/test_runner.rs::run_suite_inner()` — 3-phase orchestration
- `core/src/test_runner.rs::run_tests_with_groups()` — parallel wave scheduling with bounded **spawn-on-completion** dispatcher: at most `file_parallel` tasks are live at a time (cap comes from `TestSuite.parallel` / file-level `# @@parallel <N>`, default 16). Each completion drains a result and spawns the next queued block, so wave size has no effect on peak concurrency. The cap also propagates to per-loop workers via a `tokio::task_local!` (`FILE_PARALLEL_CAP`) read inside `execute_loop_block`.
- `core/src/test_runner.rs::execute_block()` — single block execution

### UI Changes (Desktop)
- All frontend code in `desktop/ui/app.js` (no framework, vanilla JS)
- Styles in `desktop/ui/styles.css` with CSS variables for theming
- Search by function name — key functions documented in `desktop/ARCHITECTURE.md`

### Extension Changes
- `background.js` for rule logic, network capture, storage
- `popup.js` for UI rendering, user interactions
- `content-script-main.js` for response body interception
- No build step — edit and reload extension

### TUI Changes

**Adding a new mode:**
1. Add variant to `Mode` enum in `app.rs`
2. Add key dispatch branch in `events.rs` (`handle_key_event` match on `app.mode`)
3. Add render branch in `ui.rs` (`draw_main` match arm)
4. Create component file in `components/` if needed

**Adding a new popup/toolbar overlay:**
1. Add visibility flag and state fields to `App` struct in `app.rs`
2. Add render function in `toolbar.rs` (draws over main content)
3. Add key handler function in `toolbar.rs`
4. Wire `Ctrl+<key>` toggle in `events.rs`

**Adding a new component:**
1. Create file in `tui/src/components/`
2. Add `pub mod` in `components/mod.rs`
3. Add render function (`draw_<component>(f, area, app)`)
4. Wire into `ui.rs` layout and `events.rs` key handling

## Performance Architecture

### Why Rust Backend Computation + Virtual Scroll (Not Dioxus/WASM)

Large HTTP responses (2MB+ JSON) froze the UI for 5–10+ seconds during JSON tree rendering, syntax highlighting, and diff computation. We evaluated a Dioxus Desktop rewrite but rejected it:

- Dioxus Desktop and Tauri v2 both want window ownership — they conflict in the same process
- Rayon in WASM requires `SharedArrayBuffer` + COOP/COEP headers, which Tauri's webview doesn't enable
- Full UI rewrite: 12,273 LOC, 3–5 weeks, critical regression risk
- No mature Dioxus testing framework

Instead, we added a **three-layer performance architecture** that keeps the existing JS frontend and offloads heavy work:

### Three-Layer Approach

```
Layer 1: Rust Rayon Commands (perf.rs)
  Heavy computation runs natively with Rayon parallel iterators
  ↓ returns structured data via Tauri IPC

Layer 2: Virtual Scroll (JS)
  Only renders ~50 visible rows regardless of data size (100K+ rows)
  Collapsed diff hunks show only change regions + 3 lines context

Layer 3: Web Workers (JS)
  Pool of 2 workers handles escapeHtml, JSON operations, text search off-thread
```

### Data Flow

```
JS requests computation
  → Tauri IPC invoke()
  → Rust: tokio::spawn_blocking → Rayon parallel processing
  → Returns structured data (HighlightedLine[], JsonTreeNode, DiffResult)
  → JS renders minimal DOM via virtual scroll (only visible rows)
```

### New Tauri Commands (`desktop/src-tauri/src/perf.rs`)

| Command | Parameters | Description |
|---------|-----------|-------------|
| `format_body` | `body`, `content_type` | Pretty-print + syntax highlight (JSON/XML/YAML/CSV). Rayon parallel highlighting for 1000+ lines |
| `build_json_tree` | `json`, `max_depth?`, `max_children?` | Build depth-limited tree (default depth=3, max_children=100). Rayon at depth ≤ 1 for wide objects |
| `expand_json_node` | `json`, `path`, `max_depth?`, `max_children?` | On-demand expansion of a single node by JSON path. Enables lazy loading |
| `compute_diff` | `text_a`, `text_b`, `content_type` | Myers O(ND) diff with JSON/XML normalization. Rayon parallel char-level highlighting. Capped at 10,000 edit distance |
| `sort_and_normalize` | `body`, `content_type` | Sort JSON keys lexicographically at all levels; sort XML attributes alphabetically. Used before diff for semantic comparison |

All commands use `tokio::spawn_blocking` for Tauri async compatibility.

### Virtual Scrolling

The JS frontend renders only the visible viewport (~50 rows) regardless of total data size. Scroll events dynamically swap content, keeping DOM node count constant. This applies to:

- **Response body viewer** — 100K+ highlighted lines rendered as ~50 DOM nodes
- **Diff viewer** — 10,000+ line diffs rendered as ~50 visible rows
- **History list** — 1,000+ entries rendered as ~15 visible rows

### Collapsed Diff Hunks

Diffs display only change regions with 3 lines of surrounding context (configurable). Unchanged sections appear as expandable "N lines hidden" separators. For a 10,000-line file with 10 changes, this reduces rendered lines from 10,000 to ~70.

### Lazy JSON Tree

The JSON tree viewer is depth-limited (default: 3 levels). Nodes beyond the limit show child counts but no children. Clicking expands a node via `expand_json_node`, which navigates to the node's path in the original JSON and builds a subtree. Arrays with >100 items show a "... N more items" truncation sentinel.

### Web Worker Pool

A pool of 2 Web Workers handles off-thread text processing:
- `escapeHtml` — HTML entity escaping for safe DOM insertion
- JSON operations — parse/stringify for large payloads
- Text search — regex matching across large response bodies

This keeps the main thread responsive during rendering.

## Testing

| Component | Framework | Count | Command |
|-----------|-----------|-------|---------|
| Core (Rust) | `cargo test` | 223 | `cargo test -p request-pilot-core` (from workspace root) |
| TUI (Rust) | `cargo test` | 113 | `cargo test -p request-pilot-tui` (from workspace root) |
| Perf (Rust unit) | `cargo test` | 62 | `cargo test -p request-pilot-desktop` (unit tests in perf.rs) |
| Perf (Rust E2E) | `cargo test` | 22 | `cargo test -p request-pilot-desktop --test perf_e2e` |
| Extension (JS) | Jest | 97 | `cd tests && npm test` |
| Perf Features (JS) | Jest | 42 | `cd tests && npm test -- perf-features` |

## Platform Notes

- **Windows file locking**: `$env:CARGO_BUILD_JOBS="1"` to avoid OS error 32
- **WebView2**: Required on Windows (pre-installed on Win 10/11)
- **macOS**: Needs Xcode CLI Tools
- **Linux**: Needs webkit2gtk, appindicator3, librsvg2, patchelf
