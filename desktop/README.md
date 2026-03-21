# Request Pilot — Desktop

A cross-platform HTTP client and **E2E integration test framework** built with **Rust** (Tauri v2) and an HTML/CSS/JS frontend. Author requests, run structured test suites from `.http` files, and inspect responses — all from a single desktop app.

## Features

- **Enhanced `.http` file format** — full E2E integration test framework
  - `@variables` block — define all variables at the top of the file
  - `@setup` / `@test` / `@teardown` block types
  - `# @assert` directives (`==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`, null checks)
  - `# @extract` directives for dynamic variable extraction from responses
  - `# @group` — assign blocks to named groups for parallel execution
  - `# @depends` — declare dependencies between blocks/groups for ordered execution
  - `# @description` — human-readable description per block
  - `# @disabled` — skip blocks without removing them
  - `{{variable}}` interpolation in URL, headers, and body
  - Built-in variables: `{{$timestamp}}`, `{{$uuid}}`, `{{$randomInt}}`
- **Builder / Code mode** — toggle between visual form editor and full syntax-highlighted `.http` code editor
- **Left sidebar** — collapsible file tree with hierarchical group nodes (▶ Run Group), enable/disable toggles, environment variables panel
- **Test runner** — setup → test → teardown execution with **parallel test execution via tokio::JoinSet** and `@group`/`@depends` dependency graph (topological wave scheduling), automatically skips remaining tests on setup failure
- **Block progress streaming** — real-time DOM updates during execution via Tauri event emission
- **Variables system** — static variables, `.env` file integration, dynamic extraction from responses, variable persistence across files
- **Environment panel** — load/save `.env` files, add/edit/delete/clear variables, red/green status indicators
- **Request client** — manual request sending with full control over method, URL, headers, and body
- **Response viewer** — status, headers, body with JSON formatting, assertions tab, and **interactive JSON tree viewer** with syntax highlighting and expand/collapse at every level (state preserved across re-renders)
- **Rich hover tooltips** — contextual popovers at block, group, file, and workspace levels with run history trends
- **"Viewed" indicator** — marks result rows and history entries as viewed
- **Failure reason badges** — ⚠ Error, HTTP status, ✗ assertion, and combined badges on blocks
- **Run history tracking** — per-block/file run history with trend bar visualization
- **Google-style inline autocomplete** — ghost text suggestions for URLs with domain+path frequency ranking
- **URL parser hardened** — handles special characters in query strings (PromQL queries with spaces, braces, etc.)
- **Body trailing comment stripping** — removes section decoration comments from request bodies while preserving mid-body `#` (YAML)
- **Cross-platform** — Windows, macOS, Linux via Tauri v2

## Enhanced `.http` Format

```http
@variables
base_url = https://api.example.com
auth_token = my-secret-token
user_name = testuser

### @setup Create User
# @description Creates a test user and extracts the user ID for subsequent tests
POST {{base_url}}/users
Content-Type: application/json
Authorization: Bearer {{auth_token}}

{
  "name": "{{user_name}}",
  "email": "{{user_name}}@example.com",
  "created_at": "{{$timestamp}}"
}

# @extract user_id = $.id
# @extract user_email = $.email
# @assert status == 201
# @assert $.name == {{user_name}}

### @test Get Created User
GET {{base_url}}/users/{{user_id}}
Authorization: Bearer {{auth_token}}

# @assert status == 200
# @assert $.id == {{user_id}}
# @assert $.email == {{user_email}}
# @assert $.name contains test

### @test Update User
PUT {{base_url}}/users/{{user_id}}
Content-Type: application/json
Authorization: Bearer {{auth_token}}

{
  "name": "updated-user",
  "request_id": "{{$uuid}}"
}

# @assert status == 200
# @assert $.name != {{user_name}}

### @teardown Delete User
DELETE {{base_url}}/users/{{user_id}}
Authorization: Bearer {{auth_token}}

# @assert status >= 200
# @assert status <= 299
```

**Block types:** `@setup` runs first (sequentially — each step can depend on variables extracted by the previous one). `@test` blocks run after all setups complete and are **parallel-safe** — they should be independent of each other with no ordering dependency. `@teardown` runs last (sequentially, always, even if tests fail). If a `@setup` block fails, all `@test` blocks are skipped but `@teardown` still executes.

**Execution order:**
| Phase | Execution | Why |
|---|---|---|
| `@setup` | **Sequential** (top → bottom) | Each step may extract variables needed by the next |
| `@test` | **Parallel-safe** (independent) | Tests should not depend on each other's results |
| `@test` + `# @group` | **Parallel within group** | Blocks in the same group run together |
| `@test` + `# @depends` | **Waits for dependency** | Block/group won't start until the named group/block finishes |
| `@teardown` | **Sequential** (top → bottom) | Cleanup order may matter (delete child before parent) |

**Directives:**
| Directive | Purpose | Example |
|---|---|---|
| `# @assert` | Validate response values | `# @assert status == 200` |
| `# @extract` | Save response values to variables | `# @extract token = $.access_token` |
| `# @description` | Human-readable block description | `# @description Fetches an auth token` |
| `# @disabled` | Skip this block during execution | `# @disabled` |
| `# @group` | Assign block to a named group | `# @group validation` |
| `# @depends` | Declare execution dependency | `# @depends setup-data` |

**Assertion operators:** `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`, `null`, `!null`

## Getting Started

### Prerequisites

- **[Rust](https://www.rust-lang.org/tools/install)** (1.70+ with `cargo`)
- **[Tauri CLI v2](https://v2.tauri.app/start/prerequisites/)** — install with:
  ```bash
  cargo install tauri-cli --version "^2"
  ```
- **Platform-specific dependencies:**
  - **Windows**: Microsoft Edge WebView2 (pre-installed on Windows 10/11)
  - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
  - **Linux**: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`
    ```bash
    # Ubuntu/Debian
    sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
    ```

### Run in Development Mode

```bash
cd request-pilot/desktop
cargo tauri dev
```

This compiles the Rust backend and opens the app with hot-reload for the HTML/CSS/JS frontend. On first run, dependency compilation may take several minutes.

> **Windows note:** If you see a file locking error (`os error 32`) during compilation, set `CARGO_BUILD_JOBS=1` to serialize builds:
> ```powershell
> $env:CARGO_BUILD_JOBS="1"
> cargo tauri dev
> ```

### Build for Production

```bash
cd request-pilot/desktop
cargo tauri build
```

The installer/binary is output to `src-tauri/target/release/bundle/`.

### Usage

1. **Load a `.http` file** — Click the 📂 button in the sidebar or press `Ctrl+O`
2. **Browse requests** — Expand the file tree in the left sidebar to see all blocks
3. **Run a single request** — Click a block, then press `Ctrl+Enter` or click Send
4. **Run all tests** — Click **Run All** in the toolbar or press `Ctrl+Shift+Enter`
5. **Switch to Code mode** — Use the Builder/Code toggle to edit raw `.http` source
6. **Create a new file** — Click **+ New** to start from a template in Code mode
7. **Manage variables** — Expand the Variables section in the sidebar to view/override values

See the [`docs/`](../docs/) folder for the full `.http` file format reference and sample files.

## Running Unit Tests

```bash
cd request-pilot/desktop/src-tauri
cargo test
```

> **Windows note:** If you hit file locking errors, use single-job mode:
> ```powershell
> $env:CARGO_BUILD_JOBS="1"
> cargo test
> ```

**152+ unit tests** across all core modules:

| Module | Tests | Coverage |
|--------|-------|----------|
| `http_parser` | 48 | Parsing, generation, round-trip, special chars, body comments |
| `assertions` | 27 | All operators, JSON paths, null handling |
| `history` | 20 | Add, retrieve, search, capacity limits |
| `variables` | 17 | Interpolation, built-ins, merge |
| `url_trie` | 15 | Prefix search, domain paths, frequency ranking |
| `env_file` | 9 | KEY=VALUE parsing, comments, edge cases |
| `test_runner` | 9 | Execution order, skip-on-failure, extracts, groups |
| `http_client` | 7 | URL encoding, request execution |

## Architecture

The backend is split into **core modules** (no Tauri dependency) and a **GUI layer**:

**Core modules** — pure Rust, reusable by any frontend or CLI:
| Module | Responsibility |
|---|---|
| `http_parser` | Parses `.http` files into structured blocks with directives |
| `variables` | Variable store, interpolation, built-in variable expansion |
| `assertions` | Evaluates `@assert` directives against response data |
| `test_runner` | Orchestrates setup → test → teardown with parallel group execution |
| `http_client` | Sends HTTP requests via reqwest with rustls |
| `history` | Stores and retrieves past requests with search |
| `url_trie` | Segment-aware trie for URL autocomplete suggestions |
| `env_file` | Parses `.env` files (KEY=VALUE with comments) |

**GUI layer:**
| Module | Responsibility |
|---|---|
| `lib.rs` | Tauri command wrappers — bridges core modules to the frontend |
| `ui/` | HTML/CSS/JS frontend — sidebar, editors, response viewer |

A CLI frontend is planned for the future and will reuse the same core modules.

## Project Structure

```
desktop/
├── ui/                     # Frontend (HTML/CSS/JS)
│   ├── index.html          # Layout with sidebar + split panels
│   ├── styles.css          # Dark IDE theme (~2500 lines)
│   └── app.js              # UI logic + state management (~3300 lines)
├── src-tauri/              # Rust backend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs         # Entry point
│       ├── lib.rs          # Tauri command wrappers (14 commands)
│       ├── http_parser.rs  # .http file parser (48 tests)
│       ├── http_client.rs  # HTTP execution via reqwest (7 tests)
│       ├── variables.rs    # Variable store + interpolation (17 tests)
│       ├── assertions.rs   # Assertion evaluator (27 tests)
│       ├── test_runner.rs  # Test orchestrator with parallel groups (9 tests)
│       ├── history.rs      # Request history + search (20 tests)
│       ├── url_trie.rs     # URL autocomplete trie (15 tests)
│       └── env_file.rs     # .env file parser (9 tests)
├── ARCHITECTURE.md
└── README.md
```

## Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+Enter` | Send request |
| `Ctrl+Shift+Enter` | Run all tests |
| `Ctrl+L` | Focus URL bar |
| `Ctrl+O` | Open `.http` file |

## License

MIT
