# RequestPilot 🛫

A developer toolkit for HTTP traffic — a browser extension for header injection, request blocking, and traffic analysis, plus a standalone Rust desktop HTTP client.

## Repository Structure

```
request-pilot/
├── extension/         # Browser extension (load in Edge/Chrome)
│   ├── manifest.json
│   ├── background.js
│   ├── popup.html, popup.css, popup.js
│   ├── content-script.js, content-script-main.js
│   └── icons/
├── desktop/           # Rust desktop app + E2E test framework (Tauri v2)
│   ├── ui/            # HTML/CSS/JS frontend
│   ├── src-tauri/     # Rust backend
│   │   └── src/
│   │       ├── main.rs, lib.rs
│   │       ├── http_parser.rs, http_client.rs
│   │       ├── variables.rs, assertions.rs
│   │       ├── test_runner.rs, history.rs
│   │       ├── url_trie.rs
│   │       └── env_file.rs
│   └── README.md
├── docs/              # Documentation & samples
│   ├── http-file-format.md      # .http format reference
│   └── samples/
│       ├── azure-managed-prometheus.http      # Prometheus queries & rules
│       ├── azure-aks-cluster.http             # AKS cluster operations
│       └── azure-aks-prometheus-monitoring.http # AKS + Prometheus E2E
├── tests/             # Jest unit tests for the extension (94 tests)
│   ├── __tests__/, __mocks__/
│   ├── jest.config.js, package.json
└── README.md
```

## Features

### Rules Engine
| Feature | Description |
|---|---|
| **Header Injection** | Add or overwrite request headers on matching URLs |
| **Request Blocking** | Block requests to specific endpoints entirely |
| **URL Redirect** | Redirect matching requests to a different URL |
| **Method Filtering** | Restrict rules to specific HTTP methods (GET, POST, etc.) |
| **Import / Export** | Share rule configurations as JSON files |
| **Duplicate Rules** | One-click duplicate with edit-before-save workflow |
| **Toggle Rules** | Enable or disable individual rules without deleting |

### Network Monitoring (Filtered Log)
| Feature | Description |
|---|---|
| **Rule-Filtered Log** | Only shows requests matching your configured rules |
| **Rule Multi-Select** | Choose which rules filter the log view |
| **Method & Status Filters** | Filter by HTTP method and status code (2xx/3xx/4xx/5xx) |
| **Show Headers** | Multi-select picker to display request/response header values inline |
| **Group By** | Group entries by URL or any request/response header |
| **Similarity Scoring** | Select a request to see match % on others based on headers + payload |
| **Match Key Picker** | Choose which headers and payload keys drive the similarity score |
| **Min Match Filter** | Filter to only show entries above a similarity threshold |
| **Pause / Resume** | Pause network capture without losing existing data |
| **Request Comparison** | Select 2 requests → side-by-side diff of headers, body (JSON key-by-key) |
| **Request Detail View** | Click 👁 to inspect any request's full headers + body |
| **Viewed Tracking** | View buttons show ✓ Viewed state so you know what you've inspected |
| **Response Body Capture** | Captured via content scripts + re-fetch fallback with original headers/cookies |
| **Log Export / Import** | Export as JSON, import JSON or HAR files |

### Statistics Tab
| Feature | Description |
|---|---|
| **Summary Cards** | Total requests, success, client errors, server errors, rule coverage |
| **Response Time Percentiles** | Visual bars for Avg, P50, P90, P95, P99, Max |
| **HTTP Methods Breakdown** | Chip display of request counts per method |
| **Domain & Path Tree** | Collapsible accordion grouped by domain → path → individual requests |
| **Rule Impact Badges** | Shows which rule types match each path |
| **One-Click Add Rule** | Create rules directly from domain/path entries |
| **Expand All / Collapse All** | Toggle for the full tree |

### UI
- **Dark & Light themes** with persistent toggle
- **Pop-out to tab** for full-screen resizable view
- **Responsive layout** — filters wrap, standalone mode uses full viewport

## Installation

1. Open Edge and navigate to `edge://extensions/` (or `chrome://extensions/` for Chrome)
2. Enable **Developer mode**
3. Click **Load unpacked**
4. Select the **`request-pilot/extension`** folder (NOT the root `request-pilot` folder)
5. The RequestPilot icon appears in your toolbar

> **Important:** You must select the `extension` subfolder, not the root directory. The root contains the `tests` folder which will cause Edge to reject the extension.

## Running Tests

The test suite uses [Jest](https://jestjs.io/) with Chrome API mocks. Tests are located in `request-pilot/tests/`.

```bash
# Navigate to the tests directory
cd request-pilot/tests

# Install dependencies (first time only)
npm install

# Run all tests
npm test

# Run tests in watch mode (re-runs on file changes)
npx jest --watch

# Run a specific test file
npx jest background.test.js
npx jest popup.test.js
```

**Current coverage: 94 tests across 2 test suites:**
- `background.test.js` — Rule CRUD, DNR rule building, import/export, logging, pause/resume, response body capture, webRequest listeners
- `popup.test.js` — HTML/attribute escaping, time formatting, URL pattern matching, similarity scoring, HAR conversion, percentile calculation, status classification

## Usage

### Quick Start
1. Click the **RequestPilot** icon → **Add Rule**
2. Choose rule type (Modify Headers / Block / Redirect)
3. Enter a URL pattern (e.g. `https://api.example.com/*`)
4. Add headers or redirect URL, optionally filter by HTTP method
5. Save → rule is immediately active

### Comparing Requests
1. Switch to the **Filtered Log** tab
2. Enable a rule, make requests, disable it, make the same requests again
3. Select 1 request → similarity scores appear on all others
4. Use **Min Match** filter to find the best matches quickly
5. Select a 2nd request → click **Compare** for side-by-side diff
6. Request tab shows headers + body diff, Response tab shows status + headers + body diff

### Analyzing HAR Files
1. In the Filtered Log tab, click **↑ Import** and select a `.har` file
2. Switch to the **Stats** tab for full traffic breakdown
3. Expand domains → paths → individual requests
4. Click **+ Add Rule** on any path to create rules from the HAR data

### URL Pattern Syntax
| Pattern | Matches |
|---|---|
| `https://api.example.com/*` | Any URL starting with `https://api.example.com/` |
| `*://example.com/*` | HTTP and HTTPS requests to `example.com` |
| `*api*` | Any URL containing `api` |

## Desktop App (Rust + Tauri v2)

A cross-platform HTTP client and **E2E integration test framework** — author requests, run structured test suites from enhanced `.http` files, and inspect responses.

| Feature | Description |
|---|---|
| **Enhanced `.http` Format** | `@variables`, `@setup`/`@test`/`@teardown` blocks, `@assert`/`@extract`/`@description`/`@disabled`/`@group`/`@depends` directives, `{{variable}}` interpolation |
| **Builder / Code Mode** | Toggle between visual form editor and full syntax-highlighted `.http` code editor |
| **Test Runner** | Setup → Test → Teardown lifecycle; parallel execution via `tokio::JoinSet` with `@group`/`@depends` dependency graph (topological wave scheduling), skip-on-failure |
| **Variables System** | Static, `.env` file integration, dynamic extraction, built-in (`$timestamp`, `$uuid`, `$randomInt`) |
| **Environment Panel** | Load/save `.env` files, add/edit/delete/clear variables, status indicators |
| **Step Toggle** | Enable/disable blocks via sidebar checkbox or `# @disabled` directive |
| **Request Client** | Manual request sending with full control over method, URL, headers, body |
| **Response Viewer** | Status, headers, body with JSON formatting, assertions tab |
| **Request History** | Auto-incrementing seq numbers, run ID linking, domain→path grouping, status/source filters, request detail overlay |
| **Comparison & Scoring** | Side-by-side request diff (headers + JSON body), weighted similarity scoring, key picker |
| **URL Autocomplete** | Rust-backed trie with segment-aware fuzzy matching, Google-style inline autocomplete with ghost text, Tab completion, domain→path quick picks |
| **JSON Tree Viewer** | Interactive expand/collapse at every level with syntax highlighting (keys, strings, numbers, booleans, null), Expand All / Collapse All buttons |
| **Hierarchical Sidebar** | Groups shown as collapsible tree nodes with chevron, status dot, count badge, and ▶ Run Group button; setup/teardown at top/bottom, tests nested |
| **Rich Hover Tooltips** | Block-level (request details, assertions, extracts, trend bar), group-level (test listing, aggregate stats), file-level (test plan, run stats), workspace-level (all-files summary); scrollable with custom scrollbar |
| **Viewed Indicator** | 👁 badge on test result rows and history entries; dims already-viewed items |
| **Failure Reason Badges** | Inline badges on test results — ⚠ Error, HTTP 4xx/5xx, ✗ N assert, or combined |
| **Run History & Trends** | Per-file run history tracking with trend bar visualization (last 20 runs) |
| **Block Progress Streaming** | Live progress updates streamed to the UI during test execution |
| **Test Plan Header** | Structured comment block at the top of `.http` files parsed and displayed as test plan metadata |
| **Body Comment Stripping** | Trailing comments stripped from request bodies to prevent 400 errors from section decorations |
| **URL Parser Hardening** | Special characters in query strings (PromQL, etc.) handled correctly |
| **Auto-Format JSON** | Response bodies automatically formatted as pretty-printed JSON by default |
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

### Running Desktop Unit Tests

```bash
cd request-pilot/desktop/src-tauri
cargo test   # 156+ tests across parser, assertions, variables, runner, history, url_trie, http_client
```

See the `desktop/` directory for full documentation.

### Documentation & Samples

The [`docs/`](docs/) folder contains:
- **[`.http` File Format Reference](docs/http-file-format.md)** — complete guide to variables, block types, directives, assertions, extracts, and interpolation (parser backed by 156+ Rust unit tests)
- **[Sample `.http` files](docs/samples/):**
  - [`azure-managed-prometheus.http`](docs/samples/azure-managed-prometheus.http) — PromQL queries, rule groups, and alert rules against Azure Monitor workspace
  - [`azure-aks-cluster.http`](docs/samples/azure-aks-cluster.http) — AKS cluster operations: node pools, upgrade profiles, credentials
  - [`azure-aks-prometheus-monitoring.http`](docs/samples/azure-aks-prometheus-monitoring.http) — Integrated AKS + Prometheus monitoring: container CPU/memory, API server, CoreDNS, PVC usage, alert rules

### Architecture Guides

For developers and AI agents working on the codebase:
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — High-level overview, repo layout, data flows, common modification patterns
- **[desktop/ARCHITECTURE.md](desktop/ARCHITECTURE.md)** — Rust module-by-module guide, frontend structure, IPC commands
- **[extension/ARCHITECTURE.md](extension/ARCHITECTURE.md)** — Extension internals, message passing, rule system, content scripts

## Tech Stack

### Browser Extension
- **Manifest V3** — modern Chrome/Edge extension standard
- **declarativeNetRequest** — performant header modification, blocking, redirects
- **webRequest** — network traffic capture with request/response details
- **Content Scripts** — response body interception (MAIN + ISOLATED world)
- **chrome.storage.local** — persistent rule and preference storage
- **Jest** — 94 unit tests with Chrome API mocks
- Vanilla HTML / CSS / JS — no build step required

### Desktop App
- **[Tauri v2](https://v2.tauri.app/)** — Cross-platform desktop framework (Rust backend + WebView frontend)
- **[reqwest](https://github.com/seanmonstar/reqwest)** — HTTP client with rustls
- **[tokio](https://tokio.rs/)** — Async runtime
- **HTML / CSS / JS** — Frontend UI (dark IDE theme, sidebar, split panels)
