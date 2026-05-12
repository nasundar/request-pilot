# `.http` File Format Reference

Request Pilot uses an enhanced `.http` file format that is backwards-compatible with standard `.http` parsers (VS Code REST Client, IntelliJ, etc.) while adding directives for E2E integration testing.

---

## File Structure

A `.http` file is divided into **blocks** separated by `###`. Each block represents either a variables definition or a request.

```
@variables               ← variables block (define ALL variables here)
key = value

### @setup Setup Step     ← block separator + type + name
# @description Brief explanation of what this step does
GET https://example.com   ← request line
Header: value             ← headers

body                      ← request body (blank line separates headers from body)

# @extract var = $.path   ← extract directive
# @assert status == 200   ← assert directive

### @test Test Step       ← next block
...

### @teardown Cleanup
# @disabled               ← block is skipped when disabled
...
```

### Test Plan Header

Start every `.http` file with a structured comment block listing all test scenarios:

```http
# ============================================================
# Feature Name — E2E Tests
#
# Brief description of what this file tests.
#
# Test Scenarios:
#   Setup:
#     1. Auth step — description
#   Tests:
#     [group-name]
#       - Test name — what it validates
#   Teardown:
#     1. Cleanup — what it removes
#
# Prerequisites:
#   - Required access / infrastructure
# ============================================================
```

This header is displayed in the desktop app when hovering over the file in the sidebar, providing a quick overview of all test scenarios without opening the file.

---

## Variables Block

The `@variables` block defines static key-value pairs that can be referenced throughout the file with `{{variableName}}` syntax. **All variables used in the file should be defined here** so they are visible and manageable in one place.

```http
@variables
base_url = https://api.example.com
auth_token = Bearer my-secret-token
timeout = 5000
```

**Rules:**
- Must appear before the first `###` separator
- One variable per line: `key = value`
- Lines starting with `#` are comments (ignored)
- Empty lines are ignored
- Variable names are case-sensitive

### Telemetry Variables

Telemetry is configured by setting well-known variables in the `@variables` block. If any endpoint variable is set, telemetry auto-enables.

```http
@variables
telemetry_traces_endpoint = https://otel-collector.example.com/v1/traces
telemetry_metrics_endpoint = https://otel-collector.example.com/v1/metrics
telemetry_logs_endpoint = https://otel-collector.example.com/v1/logs
telemetry_token = my-bearer-token
telemetry_service = my-api-e2e
base_url = https://api.example.com
```

| Variable | Purpose | Required |
|----------|---------|----------|
| `telemetry_traces_endpoint` | Full OTLP traces URL | At least one endpoint needed |
| `telemetry_metrics_endpoint` | Full OTLP metrics URL | At least one endpoint needed |
| `telemetry_logs_endpoint` | Full OTLP logs URL | At least one endpoint needed |
| `telemetry_token` | Bearer auth token | Optional |
| `telemetry_api_key` | API key (sent as `x-ms-ikey`) | Optional (alternative to token) |
| `telemetry_service` | OTEL `service.name` | Optional (defaults to filename) |

Endpoints can also be extracted dynamically via `@setup` blocks (e.g., fetching OTLP endpoints from an ARM API response and extracting them into the well-known variables).

---

## Block Types

Each block after `###` can have a type annotation on its first non-empty line:

| Type | Syntax | Execution | Purpose |
|------|--------|-----------|---------|
| **setup** | `@setup [Name]` | **Sequential** (first, top → bottom) | Initialize state, create test data, extract variables for later steps |
| **test** | `@test [Name]` | **Parallel-safe** (after all setups) | Core test assertions — each test should be independent |
| **teardown** | `@teardown [Name]` | **Sequential** (last, always runs) | Clean up, delete test data |
| **request** | *(no annotation)* | Sequential (in order) | Plain request (no test semantics) |

**Execution rules:**
- **Setup blocks** run first, **sequentially in file order** — each setup can use variables extracted by the previous one (e.g., authenticate → create resource → create child resource)
- If any setup block fails, all test blocks are **skipped**
- **Test blocks** run after all setups — they are **parallel-safe**, meaning each test must be self-contained and should not depend on another test's result. Tests should only rely on variables set during the setup phase.
- **Teardown blocks** always run (**sequentially in file order**), even when setups/tests fail — use reverse dependency order (delete child before parent)

---

## Request Format

Each block contains an HTTP request in standard format:

```http
METHOD URL
Header-Name: Header-Value
Another-Header: Another-Value

{
  "optional": "request body"
}
```

**Supported methods:** `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`

**Examples:**

```http
### Simple GET
GET https://api.example.com/users

### POST with JSON body
POST https://api.example.com/users
Content-Type: application/json
Authorization: Bearer {{auth_token}}

{
  "name": "John Doe",
  "email": "john@example.com"
}

### PUT with form data
PUT https://api.example.com/settings
Content-Type: application/x-www-form-urlencoded

theme=dark&language=en
```

### URL Encoding

Request Pilot automatically percent-encodes special characters in URL query strings before sending (spaces → `%20`, `{` → `%7B`, `}` → `%7D`, etc.). You can write URLs with literal special characters and they will be encoded at send time.

However, for complex query values containing spaces, braces, or parentheses (common in PromQL, LogQL, etc.), prefer using **POST with form-urlencoded body** instead of GET with query parameters:

```http
### GET with simple query (works fine)
GET {{endpoint}}/api/v1/query?query=up

### POST for complex queries (recommended)
POST {{endpoint}}/api/v1/query_range
Content-Type: application/x-www-form-urlencoded

query=rate({"system.cpu.time"}[5m])&start={{start}}&end={{end}}&step={{step}}
```

---

## Directives

Directives are special comments that control test behavior. They use the `# @` prefix so standard `.http` parsers treat them as comments.

### `# @name`

Gives the request block a human-readable name (used in UI and results).

```http
# @name Create new user
POST https://api.example.com/users
```

### `# @assert`

Validates response values. If any assertion fails, the block is marked as failed.

```http
# @assert status == 200
# @assert $.name == John
# @assert $.items.length > 0
# @assert $.email contains @example.com
```

**Syntax:** `# @assert <path> <operator> <value>`

#### Assertion Paths

| Path | Description | Example |
|------|-------------|---------|
| `status` | HTTP response status code | `status == 200` |
| `$.field` | Top-level JSON body field | `$.name == John` |
| `$.parent.child` | Nested JSON field | `$.user.profile.name == John` |
| `$[index].field` | Array element access | `$[0].id == 1` |
| `$.field.length` | Array/string length | `$.items.length > 0` |
| `$.headers.name` | Response header (case-insensitive) | `$.headers.content-type contains json` |

> **Note:** In assert paths, `status` is shorthand for `response.status`, and `$.field` is shorthand for `response.body.field`. The full `response.` prefix also works.

#### Assertion Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `==` | Equal (numeric or string) | `status == 200` |
| `!=` | Not equal | `status != 404` |
| `>` | Greater than (numeric) | `status > 199` |
| `<` | Less than (numeric) | `status < 300` |
| `>=` | Greater than or equal | `status >= 200` |
| `<=` | Less than or equal | `status <= 299` |
| `contains` | String contains | `$.name contains test` |

**Special values:**
- `null` — check if a field is null: `# @assert $.token == null`
- `!null` — check if a field is not null: `# @assert $.token != null`

### `# @extract`

Saves a response value into a variable for use in subsequent blocks.

```http
# @extract user_id = $.id
# @extract auth_token = $.access_token
# @extract item_count = $.items.length
```

**Syntax:** `# @extract <variable_name> = <json_path>`

Extracted variables are immediately available via `{{variable_name}}` in all following blocks.

### `# @description`

Provides a human-readable description for a block. Displayed as a subtitle in the sidebar and preserved in code mode.

```http
### @setup Authenticate
# @description Fetches an OAuth2 token using client credentials flow
POST https://login.microsoftonline.com/{{tenant_id}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{scope}}
```

### `# @disabled`

Marks a block as disabled. Disabled blocks are skipped during execution but remain in the file for reference. Toggle in the UI sidebar or add/remove the directive in code mode.

```http
### @test Experimental Endpoint
# @disabled
GET {{base_url}}/experimental/feature-flag
Authorization: Bearer {{access_token}}

# @assert status == 200
```

### `# @group`

Assigns a block to a named group. Blocks in the same group are **parallel-safe** — they can execute concurrently. Use with `# @depends` to create execution chains between groups.

```http
### @test Validate Order
# @group validation
GET {{base_url}}/orders/{{order_id}}

# @assert status == 200

### @test Validate Invoice
# @group validation
GET {{base_url}}/invoices/{{invoice_id}}

# @assert status == 200
```

Both blocks above belong to group `validation` and can run in parallel.

### `# @depends`

Declares that this block (or its group) must wait for the named group or block to complete before executing. Use to create sequential chains between otherwise parallel groups.

```http
### @test Create Resources
# @group create
POST {{base_url}}/resources
# @extract resource_id = $.id
# @assert status == 201

### @test Validate Resources
# @group validate
# @depends create
GET {{base_url}}/resources/{{resource_id}}
# @assert status == 200

### @test Generate Report
# @depends validate
GET {{base_url}}/report
# @assert status == 200
```

**Execution flow:** `create` group runs first → `validate` group runs after `create` completes → `Generate Report` runs after `validate` completes. Groups can depend on other groups, forming a nested execution chain.

### `# @mode`

Restricts a block to a specific execution mode. Blocks with `# @mode app` only run in normal (app) mode and are skipped in dev mode. Blocks with `# @mode dev` only run in dev mode and are skipped in normal mode.

```http
### @setup Fetch Token (App Mode)
# @mode app
POST https://login.microsoftonline.com/{{tenant_id}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{scope}}

# @extract access_token = $.access_token

### @setup Dev-Only Mock
# @mode dev
GET {{base_url}}/dev/mock-setup

# @assert status == 200
```

### `# @dev_auth`

Declares an Azure scope for user authentication on a `# @mode app` token-fetch block. When dev mode is active, the app-mode block is skipped and Request Pilot authenticates the user for the declared scope, injecting the token into the extracted variable automatically.

```http
### @setup Authenticate
# @mode app
# @dev_auth https://prometheus.monitor.azure.com/.default
POST https://login.microsoftonline.com/{{tenant_id}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{scope}}

# @extract access_token = $.access_token
```

In dev mode, this block is skipped entirely — instead, Request Pilot performs interactive user authentication for the `https://prometheus.monitor.azure.com/.default` scope and stores the resulting token in `{{access_token}}`.

### `# @@request-id`

File-level directive that auto-injects a unique request id (UUIDv4) into every request's headers. Useful for correlating test calls in downstream server logs. Place it in the file header (before `@variables` or any `###` block).

**Syntax:** `# @@request-id [header-name]`

- No argument → header name defaults to `X-Request-Id`
- Custom name → any valid HTTP header name (e.g. `X-Correlation-ID`, `traceparent`)

```http
# @@request-id X-Correlation-ID

@base_url = https://api.example.com

### @@test Health Check
GET {{base_url}}/health
# @@assert status == 200
```

Every request dispatched by Pilot receives a fresh `X-Correlation-ID: <uuid>` header.

**Block-level override / opt-out:**

```http
# @@request-id X-Global

### @@test Inherits global
GET http://x

### @@test Override header name
# @@request-id X-Local
GET http://x

### @@test Skip auto-injection
# @@request-id off
GET http://x
```

**Behavior:**
- A fresh UUIDv4 is generated per request (not per suite run) — retries and auto-runs each get unique ids
- If the user explicitly sets the same header (case-insensitive) in the block, the directive is a no-op for that block
- Coexists with the built-in variable `{{$uuid}}` — you can still reference uuids in URLs or bodies
- Accepts legacy `# @request_id` and `# @@request_id` (underscore) aliases

### `# @auto_run`

File-level directive that configures automatic test re-execution at a fixed interval. Place it in the file header (before `@variables` or any `###` block).

**Syntax:** `# @auto_run <interval>`

**Valid intervals:** `30s`, `1m`, `5m`, `15m`, `1h`, `2h`, `4h`, `1d`, or any `<number><s|m|h|d>` combination.

```http
# @auto_run 15m
@variables
base_url = https://api.example.com

### @test Health Check
GET {{base_url}}/health
# @assert status == 200
```

**Behavior:**
- Tests automatically re-run every 15 minutes after the previous run completes
- If a run is already in progress when the timer fires, the cycle is skipped and rescheduled
- Manual runs reset the timer to avoid immediate re-triggers
- Both TUI (`Ctrl+R` popup) and desktop (dropdown) allow overriding the interval at runtime
- Runtime overrides are ephemeral — they don't modify the file unless explicitly saved

### `# @@parallel` (file-level)

File-level directive that caps how many test blocks the wave scheduler runs concurrently. Place it at the top of the file before the first runnable `###` block (same placement rules as `# @@auto_run` / `# @@request-id`).

**Syntax:** `# @@parallel [<N>]`

**Rules:**
- Bare `# @@parallel` (no value) → uses the built-in default of **16**.
- Numeric value → clamped into `[1, 256]`. `0` / negative / non-numeric values are silently dropped (the default applies).
- Must appear before the first `###` block — directives after the first runnable block are ignored.
- Duplicates keep the first occurrence.
- Files without the directive use the default cap of **16**.

```http
# @@parallel 4

### @test Block 1
GET https://api.example.com/a

### @test Block 2
GET https://api.example.com/b
```

**Behavior:**
- The runner's wave scheduler keeps the live in-flight task count below the cap regardless of wave size. This protects suites with hundreds of blocks per wave from socket / connection-pool exhaustion.
- The cap also propagates into per-block `# @@for` loops: a loop with its own `# @@parallel <M>` is clamped to `min(M, file-level cap)` so a loop can never exceed the file's concurrency budget.
- File-level `# @@parallel` (this directive) caps wave-wide block execution. Loop-level `# @@parallel <N>` on a `# @@for` block (see below) is a separate, narrower control that bounds per-loop iteration concurrency.

### `### @@env` (in-file environment blocks)

Declare one or more named environments directly in the `.http` file. Each block holds a set of `key = value` definitions that the runner applies to the variable store at suite start.

**Syntax:**

```http
### @@env <name>
key = value
key2 = value2
```

`<name>` must match `[A-Za-z0-9_.-]+` (letters, digits, `_`, `-`, `.`). Names are unique within a file — duplicate `### @@env` blocks of the same name keep the first occurrence and drop subsequent ones. Empty / invalid names are silently dropped.

**Companion directive — `# @@active_env`:**

A file-level directive that selects which env block applies by default:

```http
# @@active_env prod

### @@env dev
host = dev.local

### @@env prod
host = prod.cloud
```

Like other file-level directives, `# @@active_env` must appear **before the first `###` block** (the parser ignores it once a runnable block has been seen). The named env must match one of the declared blocks — references to undeclared names are dropped at parse time to avoid silent mis-selection.

**Selection precedence at run time (low → high):**

1. Built-in variables (`{{$timestamp}}`, `{{$uuid}}`, …)
2. `@variables` block values
3. **Exactly one of**:
   - Sidecar `.env` file (Environments tab / Tools), OR
   - In-file env block (resolved as: UI/CLI override → `# @@active_env` → first declared block → none)
4. CLI / runtime extras (`--var KEY=VAL`, prompt answers)
5. `# @@extract` results captured during the run

A `# @@extract` that captures into the same variable name as an env value **overrides** the env value for the rest of the run. The desktop / CLI / TUI is responsible for enforcing the sidecar-vs-in-file XOR; the runner applies whichever overlay the caller selected.

**Example:**

```http
@variables
common = shared-value

# @@active_env prod

### @@env dev
host = dev.local
api_key = dev-key-xxxx

### @@env prod
host = prod.cloud
api_key = prod-key-xxxx

### @test Health
GET https://{{host}}/healthz
```

**Notes:**
- Env values are interpolated through the standard variable store, so cross-references like `key = {{common}}-x` work.
- Source order is preserved by the round-tripping generator (`### @@env dev` declared first stays first in output).
- Selecting "no env" via the UI suppresses both sidecar and in-file overlays; only `@variables` and CLI extras apply.

### `# @compare`

Marks a test block as a multi-step comparison block. Must be combined with `# @step` directives. The block contains multiple named steps that execute sequentially, and their responses can be compared using `# @diff`.

```http
### @test Compare API Versions
# @compare
# @step v1
# @step v2
# @diff v1 v2
```

See the [Response Comparison](#response-comparison) section for a full example.

### `# @step`

Defines a named step within a `# @compare` block. Each step has its own HTTP request, assertions, and extracts. Steps execute sequentially within the block, and their responses are captured for comparison.

**Syntax:** `# @step <name>`

### `# @diff`

Triggers a structured comparison between two named steps in a `# @compare` block. After the comparison runs, assertions using `$diff.*` paths evaluate the comparison result.

**Syntax:** `# @diff <step_a> <step_b> [allow_mismatch]`

A `@compare` block may declare multiple `# @diff` pairs. Each pair runs independently and produces its own outcome. Any `# @assert $diff.*` directive after a `# @diff` line routes to **that pair only** (V1.3+); assertions before the first `# @diff` line attach to the first pair declared in the block. Optional trailing `allow_mismatch` suppresses the implicit body-match check for that pair only — explicit `$diff.*` assertions still run.

```http
# @diff eastus westus
# @assert $diff.changed_count == 0

# @diff eastus canary allow_mismatch
# @assert $diff.similarity >= 0.9
```

---

## Variable Interpolation

Use `{{variableName}}` anywhere in the URL, headers, or body to insert a variable value.

```http
@variables
base_url = https://api.example.com
api_key = sk-12345

###
GET {{base_url}}/users?key={{api_key}}
Authorization: Bearer {{api_key}}
```

### Variable Resolution Order

1. **Extracted variables** (from `# @extract` in prior blocks)
2. **User-defined variables** (set in the UI's Variables panel)
3. **Static variables** (from the `@variables` block)
4. **Built-in variables** (see below)

If a variable is not found, `{{variableName}}` is left as-is (not replaced).

### Built-in Variables

| Variable | Description | Example Output |
|----------|-------------|---------------|
| `{{$timestamp}}` | Current Unix timestamp (seconds) | `1710694104` (see below for offsets) |
| `{{$timestamp <offset> <unit>}}` | Current Unix timestamp shifted by a signed offset | `{{$timestamp -1 h}}` → one hour ago |
| `{{$datetime}}` | Current UTC time in RFC 3339 / ISO 8601 | `2024-03-18T12:34:56+00:00` |
| `{{$uuid}}` / `{{$guid}}` | Random UUID v4 | `a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d` |
| `{{$randomInt}}` | Random integer in [0, 10000) | `4217` |
| `{{$randomInt min max}}` | Random integer in [min, max) | `{{$randomInt 1 100}}` → `42` |
| `{{$processEnv VARNAME}}` | Value of an OS environment variable (resolved during test-suite runs only) | `{{$processEnv API_KEY}}` |
| `{{$localHostname}}` | Local machine hostname (resolved during test-suite runs only) | `dev-host` |

#### `$timestamp` offsets

`{{$timestamp <offset> <unit>}}` returns the current Unix timestamp shifted by
the given offset. Both `<offset>` and `<unit>` are required; either alone is
left unresolved. Unknown units and non-integer offsets are also left unresolved.

| Unit | Meaning | Example |
|------|---------|---------|
| `y` | year | `{{$timestamp 1 y}}` — one year ahead |
| `M` | month (capital) | `{{$timestamp -3 M}}` — three months ago |
| `w` | week | `{{$timestamp 2 w}}` — two weeks ahead |
| `d` | day | `{{$timestamp -7 d}}` — a week ago |
| `h` | hour | `{{$timestamp -1 h}}` — one hour ago |
| `m` | minute (lowercase) | `{{$timestamp 30 m}}` — 30 minutes ahead |
| `s` | second | `{{$timestamp -10 s}}` — 10 seconds ago |
| `ms` | millisecond | `{{$timestamp -500 ms}}` — half a second ago |

> Note `M` (capital) means month and `m` (lowercase) means minute — same
> convention as VS Code REST Client.

Year and month offsets use calendar-correct arithmetic. Week/day/hour/minute/
second/millisecond offsets are exact fixed-duration math.

Note: `$processEnv` and `$localHostname` resolve only during full test-suite runs (which use the Rust core). When using the single-shot Send button in the desktop UI, those tokens are left unresolved as `{{$processEnv VAR}}`.

```http
POST https://api.example.com/events
Content-Type: application/json

{
  "event_id": "{{$uuid}}",
  "timestamp": {{$timestamp}},
  "priority": {{$randomInt}}
}
```

---

## Comments

Lines starting with `#` or `//` are treated as comments. Comments are stripped in different contexts:

| Context | Behavior |
|---------|----------|
| **Before request line** | Skipped (used for naming if no `# @name`) |
| **In header section** | Skipped entirely |
| **In body section** | **Trailing** comments and empty lines are stripped from the end of the body. Comments in the **middle** of a body are preserved (e.g., YAML with `#` comments). |
| **Between blocks** | Belong to the previous block — use an empty `###` separator to avoid them leaking into the body |

```http
# This is a comment (before request line)
GET https://api.example.com/health
# This comment is in the header section (ignored)
Authorization: Bearer token

body content here
# ← This trailing comment is stripped from the body

# @assert status == 200   ← This IS a directive (# @assert)
```

> **Important:** Section decoration comments between blocks (like `# ====` or `# Group: xxx`) must be placed **after** a `###` separator to avoid becoming part of the previous block's request body. Use an empty `###` line to start a comment-only separator block.

---

## Response Comparison

A `# @compare` block lets you send requests to multiple endpoints (or versions) and structurally compare their responses. This is useful for verifying API compatibility, migration correctness, or A/B testing.

### `$diff` Variable Paths

After a `# @diff` directive executes, the comparison result is available through `$diff.*` paths in assertions:

| Path | Type | Description |
|------|------|-------------|
| `$diff.match` | bool | `true` if both responses are identical |
| `$diff.similarity` | float | Similarity score from `0.0` (completely different) to `1.0` (identical) |
| `$diff.is_json` | bool | `true` if both responses are valid JSON |
| `$diff.added_count` | int | Number of fields present in step B but not in step A |
| `$diff.removed_count` | int | Number of fields present in step A but not in step B |
| `$diff.changed_count` | int | Number of fields present in both but with different values |
| `$diff.added_paths.length` | int | Length of the added paths array |
| `$diff.removed_paths.length` | int | Length of the removed paths array |
| `$diff.changed_paths.length` | int | Length of the changed paths array |
| `$diff.changed_paths[N].path` | string | JSON path of the Nth changed field |
| `$diff.changed_paths[N].left` | any | Value from step A for the Nth changed field |
| `$diff.changed_paths[N].right` | any | Value from step B for the Nth changed field |

### Full Comparison Example

```http
@variables
base_url_v1 = https://api.example.com/v1
base_url_v2 = https://api.example.com/v2
auth_token = Bearer my-secret-token

### @test Compare User API Versions
# @description Validates that the v2 users endpoint returns the same data as v1
# @compare

# @step v1
GET {{base_url_v1}}/users/42
Authorization: {{auth_token}}

# @assert status == 200
# @extract v1_user = $.name

# @step v2
GET {{base_url_v2}}/users/42
Authorization: {{auth_token}}

# @assert status == 200
# @extract v2_user = $.name

# @diff v1 v2
# @assert $diff.match == true
# @assert $diff.similarity >= 0.95
# @assert $diff.removed_count == 0
# @assert $diff.changed_paths.length == 0
```

In this example:
1. The `v1` step sends a request to the v1 API and asserts a `200` response.
2. The `v2` step sends the same request to the v2 API.
3. The `# @diff v1 v2` directive compares the two responses.
4. Assertions on `$diff.*` paths verify that v2 returns identical data — no removed fields and no changed values.

---

## Full Example

See the [`samples/`](samples/) folder for complete working examples:
- [`azure-managed-prometheus.http`](samples/azure-managed-prometheus.http) — PromQL queries, rule groups, alert rules
- [`azure-aks-cluster.http`](samples/azure-aks-cluster.http) — AKS cluster operations, node pools, upgrade profiles
- [`azure-aks-prometheus-monitoring.http`](samples/azure-aks-prometheus-monitoring.http) — Integrated AKS + Prometheus monitoring E2E tests
