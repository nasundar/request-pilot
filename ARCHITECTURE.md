# Request Pilot — Architecture Overview

This document is for **developers and AI agents** working on the codebase. It maps the repository structure, explains how the pieces connect, and provides pointers for common modifications.

## Repository Layout

```
request-pilot/
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
│   ├── src-tauri/src/     # Rust backend
│   │   ├── lib.rs         # Tauri command registry + AppState
│   │   ├── http_parser.rs # .http file parser (TestSuite, TestBlock)
│   │   ├── http_client.rs # reqwest-based HTTP execution
│   │   ├── test_runner.rs # 3-phase test engine (setup→parallel tests→teardown)
│   │   ├── assertions.rs  # Assertion evaluation (==, contains, exists, etc.)
│   │   ├── variables.rs   # VariableStore with {{interpolation}}
│   │   ├── history.rs     # Request history storage & search
│   │   ├── url_trie.rs    # Segment-aware URL autocomplete trie
│   │   └── env_file.rs    # .env file parser
│   └── ui/                # Frontend (vanilla HTML/CSS/JS)
│       ├── index.html     # Single page — sidebar + main area
│       ├── app.js         # All application logic (~3500+ lines)
│       └── styles.css     # All styles (~2500+ lines, dark IDE theme)
│
├── docs/                  # Documentation & samples
│   ├── http-file-format.md    # .http format reference
│   └── samples/               # Example .http files (Azure, Prometheus)
│
├── tests/                 # Extension unit tests (Jest, 94 tests)
│   ├── __tests__/
│   └── jest.config.js
│
├── .github/skills/        # Copilot skills
│   └── e2e-http-test-generator/SKILL.md
│
├── README.md              # Project overview & features
└── ARCHITECTURE.md        # ← You are here
```

## Two Products, One Repo

| Component | Tech | Purpose |
|-----------|------|---------|
| **Browser Extension** | Manifest V3, vanilla JS | Intercept/modify HTTP traffic in the browser — header injection, blocking, redirects, traffic analysis |
| **Desktop App** | Rust (Tauri v2) + HTML/JS | Standalone HTTP client + E2E test framework for `.http` files |

They share the `.http` file format concept and the Request Pilot brand, but are **independent codebases** with no shared code.

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
  → JS: runAllTests() → for each file: invoke('run_test_suite', { suite, extraVariables })
  → Rust: test_runner::run_suite_inner()
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
| `parse_test_file` | http_parser | Parse .http content → TestSuite |
| `run_test_suite` | test_runner | Execute suite, stream progress |
| `send_request` | http_client | Single HTTP request |
| `check_variables` | variables | Find unresolved {{vars}} |
| `search_history` | history | Query request history |
| `suggest_completions` | url_trie | URL prefix autocomplete |
| `suggest_domain_paths` | url_trie | Domain→path top-N suggestions |

### IPC Events (Rust → JS)

| Event | Payload | Purpose |
|-------|---------|---------|
| `block-progress` | BlockProgress | Per-block status during test run |

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

## For AI Agents: Common Tasks

### Adding a Tauri Command
1. Write function in the relevant Rust module
2. Add `#[tauri::command]` wrapper in `lib.rs`
3. Register in `generate_handler![]` macro in `lib.rs`
4. Call from JS: `await invoke('command_name', { param1, param2 })`

### Adding a New .http Directive
1. `http_parser.rs` — parse the `# @directive` line, add field to `TestBlock`
2. `test_runner.rs` — handle the directive during execution
3. `app.js` — display in sidebar tooltip (`showBlockTooltip`)
4. `docs/http-file-format.md` — document the directive
5. `SKILL.md` — update the generation skill if applicable

### Modifying Test Execution
- `test_runner.rs::run_suite_inner()` — 3-phase orchestration
- `test_runner.rs::run_tests_with_groups()` — parallel wave scheduling
- `test_runner.rs::execute_block()` — single block execution

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
| Desktop (Rust) | `cargo test` | 156+ | `cd desktop/src-tauri && cargo test` |
| Extension (JS) | Jest | 94 | `cd tests && npm test` |

## Platform Notes

- **Windows file locking**: `$env:CARGO_BUILD_JOBS="1"` to avoid OS error 32
- **WebView2**: Required on Windows (pre-installed on Win 10/11)
- **macOS**: Needs Xcode CLI Tools
- **Linux**: Needs webkit2gtk, appindicator3, librsvg2, patchelf
