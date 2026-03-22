# Request Pilot ✈

A developer toolkit for HTTP traffic — a **Tauri v2 desktop app** for authoring and running E2E integration test suites from `.http` files, a **browser extension** for header injection, request blocking, and traffic analysis, a **terminal UI** for headless testing, and a **shared Rust core** that powers them all.

## Architecture

```
request-pilot/
├── core/              # Shared Rust core library (171 tests)
│   └── src/
│       ├── http_parser.rs     # .http file parser & generator
│       ├── http_client.rs     # reqwest-based HTTP client
│       ├── test_runner.rs     # Suite executor with parallel scheduling
│       ├── assertions.rs      # Response assertion engine
│       ├── variables.rs       # Variable interpolation & built-ins
│       ├── azure_auth.rs      # Azure CLI + device code auth
│       ├── history.rs         # Request history store & filters
│       ├── url_trie.rs        # Segment-aware URL autocomplete trie
│       └── env_file.rs        # .env file reader/writer
├── desktop/           # Desktop GUI app (Tauri v2)
│   ├── ui/            # HTML/CSS/JS frontend (vanilla, no framework)
│   │   ├── app.js             # ~5000 lines — full app logic
│   │   ├── index.html         # Main window layout
│   │   ├── popout.html        # Pop-out window template
│   │   └── styles.css         # Dark IDE theme
│   └── src-tauri/     # Rust backend (imports core)
│       └── src/lib.rs         # 20 Tauri IPC commands
├── extension/         # Browser extension (Edge/Chrome, Manifest V3)
│   ├── manifest.json
│   ├── background.js          # Rules engine, DNR, webRequest
│   ├── popup.html/css/js      # Extension popup UI
│   └── content-script*.js     # Response body interception
├── tui/               # Terminal UI (ratatui + crossterm)
│   └── src/
│       ├── main.rs, app.rs    # App state & event loop
│       ├── events.rs          # Keyboard/terminal events
│       └── ui.rs              # TUI rendering
├── docs/              # Documentation & sample .http files
├── tests/             # Jest unit tests for browser extension (94 tests)
├── .github/skills/    # AI skill for generating .http test files
├── Cargo.toml         # Workspace root (members: core, tui)
└── README.md
```

---

## Desktop App (Tauri v2)

A cross-platform HTTP client and **E2E integration test framework** — author requests, run structured test suites from enhanced `.http` files, and inspect responses. Built with Rust + Tauri v2 (HTML/CSS/JS frontend, Rust backend).

### Core Features

| Feature | Description |
|---|---|
| **Enhanced `.http` Format** | `@variables`, `@setup`/`@test`/`@teardown` blocks, `@assert`/`@extract`/`@description`/`@disabled`/`@group`/`@depends`/`@mode`/`@dev_auth` directives, `{{variable}}` interpolation |
| **Test Runner** | Setup → Test → Teardown lifecycle; parallel execution via `tokio::JoinSet` with `@group`/`@depends` dependency graph (topological wave scheduling), skip-on-failure |
| **Variables System** | Static definitions, `.env` file integration, dynamic `@extract` from responses, built-in generators (`$timestamp`, `$uuid`, `$randomInt`) |
| **Assertions** | `# @assert status == 200`, `# @assert $.field != null`, `# @assert $.items.length > 0` — 7 operators with JSON path support |
| **Run Modes** | Run All, Run Single Block, Run Group, Run File — all with live streaming progress |

### UI Modes

| Mode | Description |
|---|---|
| **🔧 Builder** | Form-based editor — method picker, URL bar, header rows, body type selector, response viewer with tabs |
| **📝 Code** | Full syntax-highlighted `.http` editor with line numbers; auto-scrolls to selected block with jump-back indicator |
| **📊 History** | Request history with domain→path grouping, method/status/source filters, URL autocomplete, request comparison, statistics |
| **📋 Logs** | Application log viewer with level filters (info/warn/error/debug), auto-scroll, live streaming |

### Response Viewer

| Feature | Description |
|---|---|
| **Smart Content Detection** | Auto-detects JSON, XML, HTML, YAML, CSV, protobuf from Content-Type + body heuristics |
| **JSON Tree Viewer** | Interactive expand/collapse at every level with syntax highlighting, Expand All / Collapse All |
| **XML/HTML Rendering** | Pretty-printed with tag, attribute, and comment highlighting |
| **Assertions Tab** | Pass/fail results per assertion with expected vs. actual values |

### Azure Authentication

Multi-scope Azure auth for developers — skip client-credentials setup blocks and authenticate with your own identity.

| Feature | Description |
|---|---|
| **Stateful Auth Button** | 🔒 Off → 🔓 Needs Auth (amber pulse) → 🔑 Authenticated (green) → ⚠ Expired (orange) |
| **Multi-Scope Support** | Auto-detects all `# @dev_auth <scope>` directives; caches tokens per scope |
| **3-Strategy Cascade** | 1) `az` CLI → 2) device code with file's `client_id` → 3) device code with Azure CLI public client |
| **`# @mode app\|dev`** | `app` blocks run in normal mode (client credentials), `dev` blocks run in dev mode (user auth) |
| **`# @dev_auth <scope>`** | Declares Azure scope per token-fetching setup block (e.g., `https://management.azure.com/.default`) |
| **Scope Tooltip** | Hover the auth button to see per-scope details and expiry times |

**Setup:**
1. Install [Azure CLI](https://learn.microsoft.com/en-us/cli/azure/install-azure-cli) and run `az login`
2. Annotate token-fetching setup blocks:
   ```http
   ### @setup Fetch ARM Token
   # @mode app
   # @dev_auth https://management.azure.com/.default
   POST https://login.microsoftonline.com/{{tenant_id}}/oauth2/v2.0/token
   Content-Type: application/x-www-form-urlencoded

   grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope=https://management.azure.com/.default

   # @extract arm_token = $.access_token
   ```
3. Click **🔒 Azure** in the toolbar — the app detects scopes, fetches tokens via `az account get-access-token`, and injects them as the block's `@extract` variables

### Additional Features

| Feature | Description |
|---|---|
| **Pop-out Panels** | Detach History or Logs into separate native OS windows (Tauri WebviewWindow) for dual-monitor workflows |
| **Zoom Controls** | `Ctrl+Scroll` / `Ctrl++` / `Ctrl+-` / `Ctrl+0` with persistent level (50%–200%) |
| **URL Autocomplete** | Rust-backed trie with segment-aware fuzzy matching, Google-style ghost text, Tab completion |
| **Rich Hover Tooltips** | Block-level (request details, assertions, trend bar), group-level (test listing, stats), file-level (test plan, run stats) |
| **JWT/Token Decode** | Hover any variable to see decoded value; JWT tokens show header, claims, issuer, audience, expiry status |
| **Hierarchical Sidebar** | Files → Groups → Blocks with collapse/expand, status dots, count badges, ▶ Run buttons |
| **Block Progress Streaming** | Live pass/fail/skip updates during test execution |
| **Step Toggle** | Enable/disable blocks via sidebar checkbox or `# @disabled` directive |
| **Guided Tour** | Two-part onboarding tour covering all features; re-open via ❓ button |
| **Cross-Platform** | Windows, macOS, Linux via Tauri v2 |

### Building & Running

**Prerequisites:** [Rust](https://rustup.rs/) (1.70+) and [Tauri CLI v2](https://v2.tauri.app/start/prerequisites/)

```bash
# Install Tauri CLI (first time only)
cargo install tauri-cli --version "^2"

# Run in development mode (compiles Rust + opens app with hot-reload)
cd request-pilot/desktop
cargo tauri dev

# Build production binary
cargo tauri build
```

> **Windows:** If you hit a file locking error (`os error 32`), run `$env:CARGO_BUILD_JOBS="1"` first.

**Platform dependencies:**
- **Windows**: Edge WebView2 (pre-installed on Win 10/11)
- **macOS**: Xcode CLI Tools (`xcode-select --install`)
- **Linux**: `sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`

---

## Browser Extension (Edge/Chrome)

A Manifest V3 extension for intercepting, modifying, and analyzing HTTP traffic.

### Rules Engine
| Feature | Description |
|---|---|
| **Header Injection** | Add or overwrite request headers on matching URLs |
| **Request Blocking** | Block requests to specific endpoints entirely |
| **URL Redirect** | Redirect matching requests to a different URL |
| **Method Filtering** | Restrict rules to specific HTTP methods (GET, POST, etc.) |
| **Import / Export** | Share rule configurations as JSON files |
| **Toggle Rules** | Enable or disable individual rules without deleting |

### Network Monitoring
| Feature | Description |
|---|---|
| **Rule-Filtered Log** | Only shows requests matching your configured rules |
| **Similarity Scoring** | Select a request to see match % on others based on headers + payload |
| **Request Comparison** | Select 2 requests → side-by-side diff of headers, body (JSON key-by-key) |
| **Response Body Capture** | Intercepted via content scripts (MAIN + ISOLATED world) with re-fetch fallback |
| **Method & Status Filters** | Filter by HTTP method and status code range |
| **Group By** | Group entries by URL or any request/response header |
| **Log Export / Import** | Export as JSON, import JSON or HAR files |

### Statistics
| Feature | Description |
|---|---|
| **Summary Cards** | Total requests, success, client errors, server errors, rule coverage |
| **Response Time Percentiles** | Avg, P50, P90, P95, P99, Max |
| **Domain & Path Tree** | Collapsible accordion with rule impact badges and one-click Add Rule |

### Installation
1. Open Edge → `edge://extensions/` (or Chrome → `chrome://extensions/`)
2. Enable **Developer mode**
3. Click **Load unpacked** → select the **`request-pilot/extension`** folder
4. The Request Pilot icon appears in your toolbar

> **Important:** Select the `extension` subfolder, not the root directory.

---

## Terminal UI (TUI)

A full-featured terminal interface for HTTP testing — same core engine as the desktop app, runs entirely in your terminal.

### Running

```bash
# Build
cd request-pilot
cargo build -p request-pilot-tui

# Run with .http files
cargo run -p request-pilot-tui -- api.http

# Multiple files + env
cargo run -p request-pilot-tui -- api.http tests.http --env .env

# Auto-run tests on startup
cargo run -p request-pilot-tui -- api.http --run
```

### Keybindings

| Key | Action |
|-----|--------|
| `j`/`k` or `↑`/`↓` | Navigate / scroll |
| `g` / `G` | Jump to top / bottom |
| `Tab` | Cycle focus between panels |
| `Enter` | Select / expand / view detail |
| `r` | Run selected block / group / file |
| `R` / `F5` | Run all tests |
| `t` | Toggle block enabled / disabled |
| `o` / `Ctrl+O` | Open .http file |
| `Ctrl+E` | Load .env file |
| `?` | Toggle help overlay |
| `q` | Quit |

### Features
| Feature | Description |
|---|---|
| **File Tree** | Hierarchical sidebar — files → groups → blocks with status icons |
| **Code View** | Syntax-colored display of method, URL, headers, body, assertions, extracts |
| **Response Viewer** | Body / Headers / Assertions sub-tabs with status badge and timing |
| **Test Runner** | Async execution with live spinner + progress bar |
| **Variables Panel** | Sorted list, extracted vars highlighted in purple |
| **History Mode** | Stats panel, filter bar, scrollable entry list |
| **Catppuccin Theme** | Warm Mocha-inspired color palette |

---

## Shared Core (`request-pilot-core`)

The Rust library powering all three frontends — **171 unit tests**, zero warnings.

| Module | Description |
|---|---|
| **`http_parser`** | Parse and generate `.http` files — `@variables`, block types, all directives (`@assert`, `@extract`, `@group`, `@depends`, `@mode`, `@dev_auth`, `@disabled`), HTTP request syntax |
| **`test_runner`** | Execute test suites — setup → parallel tests (topological wave scheduling via `@group`/`@depends`) → teardown, with streaming progress events |
| **`http_client`** | reqwest-based async HTTP with variable interpolation, smart URL encoding |
| **`assertions`** | Evaluate `@assert` directives — 7 operators (`==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`), JSON path selectors, status/header/body targets |
| **`variables`** | Variable store with `{{interpolation}}`, built-ins (`$timestamp`, `$uuid`, `$randomInt`), merge with .env, unresolved detection |
| **`azure_auth`** | Azure CLI token fetch (`az`/`az.cmd`), device code flow (start + poll), availability detection |
| **`history`** | In-memory history store (max 1000), filtering by method/status/URL/source/run_id, trie-based URL autocomplete |
| **`url_trie`** | Segment-aware trie — split URLs by `/`, `.`, `:`, `?`, `&`, `=` for frequency-ranked autocomplete and domain→path grouping |
| **`env_file`** | Parse/write `.env` files with quotes, comments, and sorted output |

### Running Tests

```bash
# Core library tests (171 tests — parser, assertions, variables, runner, history, url_trie, http_client)
cd request-pilot
cargo test -p request-pilot-core

# Desktop adapter check
cd request-pilot/desktop/src-tauri
cargo check

# Browser extension tests (94 tests)
cd request-pilot/tests
npm install && npm test
```

---

## Documentation & Samples

The [`docs/`](docs/) folder contains:
- **[`.http` File Format Reference](docs/http-file-format.md)** — complete guide to variables, block types, directives, assertions, extracts, and interpolation
- **[Sample `.http` files](docs/samples/):**
  - [`azure-managed-prometheus.http`](docs/samples/azure-managed-prometheus.http) — PromQL queries, rule groups, and alert rules
  - [`azure-aks-cluster.http`](docs/samples/azure-aks-cluster.http) — AKS cluster operations: node pools, upgrade profiles, credentials
  - [`azure-aks-prometheus-monitoring.http`](docs/samples/azure-aks-prometheus-monitoring.http) — AKS + Prometheus E2E monitoring

### Architecture Guides

For developers and AI agents working on the codebase:
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — High-level overview, repo layout, data flows, common modification patterns
- **[desktop/ARCHITECTURE.md](desktop/ARCHITECTURE.md)** — Rust module-by-module guide, frontend structure, IPC commands
- **[extension/ARCHITECTURE.md](extension/ARCHITECTURE.md)** — Extension internals, message passing, rule system, content scripts

### AI Skill

The `.github/skills/e2e-http-test-generator/` directory contains a Copilot skill for generating `.http` test files from code changes. It analyzes diffs, PRs, or commits to produce structured test files following all Request Pilot conventions.

---

## Tech Stack

| Layer | Technologies |
|---|---|
| **Shared Core** | Rust, reqwest, tokio, serde, chrono |
| **Desktop App** | Tauri v2, HTML/CSS/JS (vanilla), Rust backend |
| **Browser Extension** | Manifest V3, declarativeNetRequest, webRequest, content scripts, chrome.storage |
| **Terminal UI** | ratatui 0.29, crossterm 0.28, clap 4, tokio |
| **Tests** | Rust `#[test]` (171 core), Jest (94 extension) |
