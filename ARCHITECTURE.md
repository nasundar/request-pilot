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

## Testing

| Component | Framework | Count | Command |
|-----------|-----------|-------|---------|
| Core (Rust) | `cargo test` | 223 | `cargo test -p request-pilot-core` (from workspace root) |
| Extension (JS) | Jest | 97 | `cd tests && npm test` |

## Platform Notes

- **Windows file locking**: `$env:CARGO_BUILD_JOBS="1"` to avoid OS error 32
- **WebView2**: Required on Windows (pre-installed on Win 10/11)
- **macOS**: Needs Xcode CLI Tools
- **Linux**: Needs webkit2gtk, appindicator3, librsvg2, patchelf
