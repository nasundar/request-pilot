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
├── tui/                   # Terminal UI (ratatui + crossterm)
│   └── src/
│       ├── main.rs, app.rs    # App state & event loop
│       ├── events.rs          # Keyboard/terminal events
│       └── ui.rs              # TUI rendering
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
    Phase 2: Test blocks (parallel via JoinSet, topological wave scheduling)
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
- `core/src/test_runner.rs::run_tests_with_groups()` — parallel wave scheduling
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
| Perf (Rust unit) | `cargo test` | 62 | `cargo test -p request-pilot-desktop` (unit tests in perf.rs) |
| Perf (Rust E2E) | `cargo test` | 22 | `cargo test -p request-pilot-desktop --test perf_e2e` |
| Extension (JS) | Jest | 97 | `cd tests && npm test` |
| Perf Features (JS) | Jest | 42 | `cd tests && npm test -- perf-features` |

## Platform Notes

- **Windows file locking**: `$env:CARGO_BUILD_JOBS="1"` to avoid OS error 32
- **WebView2**: Required on Windows (pre-installed on Win 10/11)
- **macOS**: Needs Xcode CLI Tools
- **Linux**: Needs webkit2gtk, appindicator3, librsvg2, patchelf
