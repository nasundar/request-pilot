---
name: e2e-http-test-generator
description: >
  Generate comprehensive E2E integration test .http files from code changes.
  Analyzes diffs, PRs, commits, or files to identify API endpoints, maps
  dependencies, and produces structured Request Pilot .http test files with
  variables, setup/test/teardown blocks, assertions, and extracts.
  Use when asked to create .http tests, E2E tests, integration tests, or
  API tests from code changes.
---

# Generate E2E Integration Test (.http file) from Code Changes

You are an expert at writing E2E integration tests using the Request Pilot `.http` file format. Given a code change (diff, PR, commit, or file), you analyze the APIs involved and produce a comprehensive, well-structured `.http` test file.

## Your Task

1. **Analyze the code change** — identify every API endpoint (REST, GraphQL, etc.) that is created, modified, or consumed
2. **Map the data flow** — understand which APIs depend on each other, what resources are created/read/updated/deleted, and what authentication is required
3. **Generate a complete `.http` test file** following the patterns below

## .http File Format

Request Pilot `.http` files **extend the VS Code REST Client format** — they remain valid REST Client files. Pilot-specific features sit in comments using a `# @@` prefix that REST Client safely ignores. You write top-level `@name = value` variables (REST Client native), `### @@setup`/`@@test`/`@@teardown` block markers, `# @@assert`/`# @@extract` directives, and reference variables with `{{name}}`.

### Variables (top of file)

Always start with all configurable values defined as top-level `@name = value` lines. Group them logically with `#` comment headers:

```http
# --- Authentication ---
@tenant_id = your-tenant-id
@client_id = your-client-id
@client_secret = your-client-secret

# --- Environment ---
@base_url = https://api.example.com
@api_version = 2024-01-01

# --- Test Data ---
@test_user_email = testuser@example.com
@test_resource_name = e2e-test-resource
```

**Rules for variables:**
- **Define ALL variables at the very beginning of the file** as top-level `@name = value` lines (REST Client native syntax) — this is mandatory
- The `=` sign is required: `@x = value` is a variable, `@x value` (no `=`) is a bare directive (e.g. `@name`, `@description`)
- Every URL hostname, API version, credential, and test data value MUST be a variable
- Use `your-` prefix for values the user must fill in (e.g. `your-subscription-id`)
- Use realistic defaults for non-sensitive values (e.g. `@api_version = 2024-01-01`)
- Group variables by category with `#` comment headers
- Use `snake_case` for variable names
- Keep variable names descriptive but concise
- Reference variables anywhere with `{{name}}` (no `@` inside braces)
- The legacy top-level `@name = value` definitions (without per-line `@`) is still accepted but should not be used in new files

### Block Types and Execution Order

Blocks execute in three phases: **setup (sequential) → test (parallel-safe) → teardown (sequential)**

| Block | Execution | Purpose | When to use |
|-------|-----------|---------|-------------|
| `@@setup` | **Sequential** (top → bottom) | Initialize state | Auth token fetch, create prerequisite resources — each step can depend on variables extracted by the previous one |
| `@@test` | **Parallel-safe** (independent) | Core assertions | Test each API endpoint — tests MUST NOT depend on each other's results, only on setup variables |
| `@@teardown` | **Sequential** (top → bottom, always runs) | Clean up | Delete created resources — use reverse dependency order (child before parent) |

> **Key design principle:** Setup blocks form a dependency chain (authenticate → create parent → create child). Test blocks are independent validations that only read the state created during setup. This means tests can safely run in any order or in parallel.

### Directives

```http
# @name Human-readable name          — names the block in the UI
# @@description Brief explanation      — subtitle shown in sidebar
# @@disabled                           — skip this block during execution
# @@group group-name                   — assign to a named group (parallel within group)
# @@depends group-or-block-name        — wait for named group/block to complete first
# @@assert status == 200               — validate response values
# @@assert $.field != null              — JSON path assertions
# @@assert $.items.length > 0          — array length checks
# @@assert $.name contains partial     — substring match
# @@extract var_name = $.json.path     — save response value for later blocks
# @@mode app|dev                    — restrict to app mode or dev mode (mutually exclusive auth)
# @@dev_auth <scope>               — Azure scope for user auth (used with @mode app)
# @@auto_run <interval>            — file-level: auto-re-run tests at interval (e.g. 15m, 1h, 1d)
# @@request-id [header]            — file-level: auto-inject fresh UUIDv4 per request into `header` (default `X-Request-Id`). Block-level override: # @@request-id X-Custom-Id. Opt out: # @@request-id off
# @@compare                            — enable multi-step comparison mode
# @@step <name>                        — define a named request step
# @@diff <step_a> <step_b>             — compare responses of two steps
# @@assert $diff.match == true          — diff assertion example
# @@for <iter_var> in <source_var>     — iterate block once per element of `source_var` (a JSON-array variable). Bare var name, NOT `{{source_var}}`. Auto-binds `{{$index}}` (0-based) and `{{$iteration}}` (1-based). For object elements, use dotted paths: `{{iter_var.field}}`. Cannot combine with `@@compare`.
# @@redact body $.json.path             — scrub a JSON field from recorded bodies (JSONPath: $.foo, $.foo.bar, $.foo[0], $.foo[*])
# @@redact body /regex/                 — scrub bytes matching a regex from recorded bodies (file- or block-level)
```

**Telemetry variables** (set as top-level `@name = value` definitions or extracted via `@@setup`):
- `telemetry_traces_endpoint` — full OTLP traces URL (at least one endpoint needed)
- `telemetry_metrics_endpoint` — full OTLP metrics URL
- `telemetry_logs_endpoint` — full OTLP logs URL
- `telemetry_token` — Bearer auth token (optional)
- `telemetry_api_key` — API key sent as `x-ms-ikey` (optional, alternative to token)
- `telemetry_service` — OTEL `service.name` (optional, defaults to filename)

**Assertion operators:** `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`
**Special values:** `null`, `!null`
**Paths:** `status`, `$.field`, `$.nested.field`, `$[0].field`, `$.field.length`, `$.headers.header-name`

### Built-in Variables

REST Client–compatible set:

- `{{$timestamp}}` — Unix timestamp (seconds)
- `{{$datetime}}` — ISO 8601 timestamp (RFC 3339)
- `{{$uuid}}` — Random UUID v4
- `{{$guid}}` — Alias of `$uuid`
- `{{$randomInt}}` — Random integer 0–9999
- `{{$randomInt min max}}` — Random integer in `[min, max)` (inclusive low, exclusive high)
- `{{$processEnv VAR}}` — Value of OS environment variable `VAR`
- `{{$localHostname}}` — The local machine hostname

### Comments and separators

- Comments: `#` or `//` (both supported; pick one and stay consistent in a file)
- Request separator: `###` or `---` at the start of a line, optionally followed by a title or `@@` block-type marker
- Bare REST Client directives (no `=`): `@name foo`, `@description ...`, `@note ...`, `@prompt VAR description`

### Sessions and body redaction

When the user has enabled the **Sessions** feature (Request Pilot's
local recorder of every test run), request and response bodies are
written to disk. To keep secrets out of those recordings, add
`# @@redact body` directives near the top of the file (or inside a
specific block). If the file POSTs credentials, OAuth client secrets,
PII, or anything you would not paste into a bug report, add a
redaction rule for it.

```http
# File-level — applies to every block
# @@redact body $.password,$.user.token
# @@redact body /Bearer\s+[A-Za-z0-9._-]+/

### @@test Login
# Block-level — appended to file-level rules for this block
# @@redact body $.session.refresh_token
POST {{base_url}}/login
```

- `$.path` uses a small JSONPath subset: `$.foo`, `$.foo.bar`,
  `$.foo[0]`, `$.foo[*]`. Recursive descent (`$..foo`) is **not**
  supported.
- `/regex/` is a slash-delimited regex applied to the raw body string —
  use it for tokens in `application/x-www-form-urlencoded` bodies or
  any non-JSON payload.
- Matched values are replaced with the literal string `[REDACTED]`.
- Global patterns (apply to every recorded session) live in
  `SessionsConfig.body_redaction_paths` — they use the same syntax.

## Patterns to Follow

### Pattern 1: Authentication Setup

If the API requires authentication, ALWAYS add a setup block to fetch the token dynamically:

```http
### @@setup Authenticate
# @name Fetch Access Token
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @@extract access_token = $.access_token
# @@assert status == 200
# @@assert $.access_token != null
# @@assert $.token_type == Bearer
```

Then use `Authorization: Bearer {{access_token}}` in all subsequent blocks.

### Pattern 2: CRUD Lifecycle

For any resource that is created, test the full lifecycle:

```http
### @@setup Create Resource
POST {{base_url}}/resources
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "name": "{{test_resource_name}}-{{$uuid}}",
  "property": "value"
}

# @@extract resource_id = $.id
# @@assert status == 201
# @@assert $.id != null

### @@test Read Resource
GET {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.id == {{resource_id}}
# @@assert $.name contains {{test_resource_name}}

### @@test Update Resource
PUT {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "name": "updated-{{test_resource_name}}",
  "property": "new-value"
}

# @@assert status == 200
# @@assert $.name contains updated

### @@test List Resources
GET {{base_url}}/resources
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.value.length > 0

### @@teardown Delete Resource
DELETE {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}

# @@assert status >= 200
# @@assert status <= 204
```

### Pattern 3: Dependency Chain (Sequential Setup → Parallel Tests → Sequential Teardown)

When APIs depend on each other, use `@@setup` blocks with ``# @@extract` to chain values. Tests read setup state but never depend on each other. Teardown reverses the creation order:

```http
### @@setup Create Parent
# @@description Creates parent resource (sequential — runs first)
POST {{base_url}}/parents
# ...
# @@extract parent_id = $.id

### @@setup Create Child (depends on Parent)
# @@description Creates child under parent (sequential — uses parent_id from above)
POST {{base_url}}/parents/{{parent_id}}/children
# ...
# @@extract child_id = $.id

### @@test Verify Relationship
# @@description Parallel-safe — only reads setup variables
GET {{base_url}}/parents/{{parent_id}}/children/{{child_id}}
# ...

### @@test Verify Parent Has Children
# @@description Parallel-safe — independent of "Verify Relationship" test above
GET {{base_url}}/parents/{{parent_id}}/children
# @@assert status == 200
# @@assert $.length > 0

### @@teardown Delete Child
# @@description Sequential cleanup — delete child first (reverse dependency order)
DELETE {{base_url}}/parents/{{parent_id}}/children/{{child_id}}

### @@teardown Delete Parent
# @@description Sequential cleanup — delete parent after child is removed
DELETE {{base_url}}/parents/{{parent_id}}
```

### Pattern 4: Error Case Testing

Test error responses with separate blocks:

```http
### @@test Not Found Returns 404
GET {{base_url}}/resources/nonexistent-id-12345
Authorization: Bearer {{access_token}}

# @@assert status == 404

### @@test Unauthorized Without Token
GET {{base_url}}/resources
# (no Authorization header)

# @@assert status == 401

### @@test Bad Request With Invalid Body
POST {{base_url}}/resources
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "invalid_field": true }

# @@assert status == 400
```

### Pattern 5: Idempotency / Retry Safety

For PUT/PATCH endpoints, verify idempotency:

```http
### @@test Idempotent Update (first call)
PUT {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "status": "active" }

# @@extract etag = $.headers.etag
# @@assert status == 200

### @@test Idempotent Update (second call — same payload)
PUT {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "status": "active" }

# @@assert status == 200
```

### Pattern 6: Grouped Parallel Tests with Dependencies

When multiple independent tests can run in parallel but depend on shared setup, use ``# @@group` and ``# @@depends` to express the execution graph:

```http
### @@setup Authenticate
# @@description Sequential setup — fetches token
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{scope}}

# @@extract access_token = $.access_token
# @@assert status == 200

### @@setup Create Order
# @@description Sequential setup — creates test order
POST {{base_url}}/orders
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "name": "e2e-order-{{$uuid}}" }

# @@extract order_id = $.id
# @@assert status == 201

### @@test Validate Order Details
# @@group validate-order
# @@description Parallel-safe — runs alongside other tests in this group
GET {{base_url}}/orders/{{order_id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.id == {{order_id}}

### @@test Validate Order Items
# @@group validate-order
# @@description Parallel-safe — same group as "Validate Order Details"
GET {{base_url}}/orders/{{order_id}}/items
Authorization: Bearer {{access_token}}

# @@assert status == 200

### @@test Update Order Status
# @@group update-order
# @@depends validate-order
# @@description Waits for validate-order group to complete
PATCH {{base_url}}/orders/{{order_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "status": "confirmed" }

# @@assert status == 200

### @@test Generate Invoice
# @@depends update-order
# @@description Waits for update-order group to complete
POST {{base_url}}/orders/{{order_id}}/invoice
Authorization: Bearer {{access_token}}

# @@assert status == 201

### @@teardown Delete Order
DELETE {{base_url}}/orders/{{order_id}}
Authorization: Bearer {{access_token}}

# @@assert status >= 200
# @@assert status <= 204
```

**Execution flow:**
1. `@@setup` blocks run sequentially (Authenticate → Create Order)
2. `validate-order` group: both Validate tests run in parallel
3. `update-order` group: runs after `validate-order` completes
4. `Generate Invoice`: runs after `update-order` completes
5. `@@teardown` runs last (always)

Groups can depend on other groups, forming nested execution chains. Blocks without ``# @@group` are standalone and follow the default execution rules for their block type.

### Pattern 7: Test Plan Header

Every `.http` file MUST start with a structured comment block that lists all test scenarios. This serves as a table of contents and is displayed in the desktop app's file tooltip on hover.

```http
# ============================================================
# [Project/Feature Name] — E2E Tests
#
# [Brief description of what this file tests and why]
#
# Test Scenarios:
#   Setup:
#     1. Authenticate — fetch OAuth2 token
#     2. Create resource — provision test data
#
#   Tests:
#     [group-name]
#       - Test A — description of what it validates
#       - Test B — description of what it validates
#     [another-group]
#       - Test C — description
#
#   Teardown:
#     1. Delete resource — cleanup test data
#
# Prerequisites:
#   - [Required access, credentials, infrastructure]
#   - [Environment variables or config needed]
#
# Metrics / Assertions:
#   - Total: N tests across M groups
#   - Key assertions: [summary of what's being validated]
# ============================================================
```

**Rules for the test plan header:**
- Always include `Test Scenarios:` with categorized setup/test/teardown listings
- Group tests by their `@@group` names
- Include a brief description for each test (matches ``# @@description`)
- List `Prerequisites:` so users know what's needed before running
- Include `Metrics:` with total test count and key assertion summary
- Keep descriptions concise — one line per test

### Pattern 8: Dev Mode — Azure CLI User Auth

When testing APIs that require Azure AD tokens, `.http` files typically have `@@setup` blocks that fetch tokens via OAuth2 client credentials (requiring `client_id` and `client_secret`). In **dev mode**, developers can skip those blocks and use their own Azure CLI login (`az account get-access-token`) instead.

Use ``# @@mode app` on client-credentials setup blocks and ``# @@mode dev` on dev-mode-only blocks. They are mutually exclusive — only one runs depending on the active mode. Add ``# @@dev_auth <scope>` to specify the Azure scope for user auth on each `@@mode app` token-fetch block.

#### Example: Dual-Mode Auth Setup

```http
### @@setup Authenticate
# @@mode app
# @@dev_auth https://management.azure.com/.default
# @@description Fetches token using client credentials (skipped in dev mode, replaced with user token)
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @@extract access_token = $.access_token
# @@assert status == 200

### @@test List Users
# @@description Works in both modes — uses access_token from either setup
GET https://graph.microsoft.com/v1.0/users?$top=5
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.value.length > 0
```

**How it works:**
- **App mode** (default): The ``# @@mode app` setup block runs, fetching a token via client credentials. Tests use the extracted `access_token`.
- **Dev mode** (toggle ON): The ``# @@mode app` block is skipped. The desktop app reads ``# @@dev_auth` from the block, authenticates the user for that scope via device code flow, and injects the token into the block's `@@extract` variable. Tests run with the user's own token.

**Rules:**
- ``# @@mode app` blocks only run when dev mode is OFF
- ``# @@mode dev` blocks only run when dev mode is ON
- Blocks without ``# @@mode` always run (backward compatible)
- ``# @@dev_auth <scope>` on a `@@mode app` block tells the app which Azure scope to request when authenticating the user
- The injected token uses the same variable name (`access_token`) so downstream tests work unchanged

### Pattern 9: E2E Observability — OTEL Telemetry

When test files need to export traces, metrics, and logs to an OTLP-compatible backend (such as Azure Monitor Application Insights), set the well-known telemetry variables. If any endpoint variable is set (non-empty), telemetry auto-enables — no directives needed.

#### Telemetry Variables

Set these in the top-level `@name = value` definitions or extract them via `@@setup` steps:

| Variable | Purpose | Required |
|----------|---------|----------|
| `telemetry_traces_endpoint` | Full OTLP traces URL | At least one endpoint needed |
| `telemetry_metrics_endpoint` | Full OTLP metrics URL | At least one endpoint needed |
| `telemetry_logs_endpoint` | Full OTLP logs URL | At least one endpoint needed |
| `telemetry_token` | Bearer auth token | Optional |
| `telemetry_api_key` | API key (sent as `x-ms-ikey`) | Optional (alternative to token) |
| `telemetry_service` | OTEL `service.name` | Optional (defaults to filename) |

**Direct OTLP endpoints:**
```http
# --- Telemetry ---
@telemetry_traces_endpoint = https://otel-collector.example.com/v1/traces
@telemetry_metrics_endpoint = https://otel-collector.example.com/v1/metrics
@telemetry_logs_endpoint = https://otel-collector.example.com/v1/logs
@telemetry_service = my-api-e2e-tests

# --- Environment ---
@base_url = https://api.example.com
```

**ARM resolution via setup step (App Insights):**
```http
# --- Telemetry (endpoints extracted from ARM response during setup) ---
@appinsights_resource_id = /subscriptions/xxx/resourceGroups/xxx/providers/microsoft.insights/components/xxx
@telemetry_traces_endpoint =
@telemetry_metrics_endpoint =
@telemetry_logs_endpoint =
@telemetry_token =
@telemetry_service = my-api-e2e-tests

### @@setup Fetch ARM Token
# @@mode app
# @@dev_auth https://management.azure.com/.default
POST https://login.microsoftonline.com/{{tenant_id}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope=https://management.azure.com/.default

# @@extract telemetry_token = $.access_token
# @@assert status == 200

### @@setup Fetch OTLP Endpoints
GET https://management.azure.com{{appinsights_resource_id}}?api-version=2025-01-23-preview
Authorization: Bearer {{telemetry_token}}

# @@extract telemetry_traces_endpoint = $.properties.OTLPTracesEndpoint
# @@extract telemetry_metrics_endpoint = $.properties.OTLPMetricsEndpoint
# @@extract telemetry_logs_endpoint = $.properties.OTLPLogsEndpoint
# @@assert status == 200
```

#### What Gets Exported

| Signal | Content | Purpose |
|--------|---------|---------|
| **Traces** | Root span per suite, child spans per block, grandchild per HTTP request. Assertions/extracts as span events. Exported to the dedicated `OTLPTracesEndpoint`. | Investigation of failures |
| **Metrics** | `rp.suite.runs`, `rp.suite.duration`, `rp.block.runs`, `rp.block.duration`, `rp.assertion.total` — low-cardinality dimensions (file, block_type, outcome). Exported to the dedicated `OTLPMetricsEndpoint`. | Dashboards and alerts |
| **Logs** | Structured log records for suite start, block completion, assertion failures, HTTP errors. Exported to the dedicated `OTLPLogsEndpoint`. | Debugging and auditing |

#### Example: Full Test File with Telemetry

```http
# ============================================================
# User API — E2E Tests with OTEL Telemetry
#
# Exports traces, metrics, and logs to Application Insights
# on every run for dashboarding and alerting.
#
# Test Scenarios:
#   Setup: Authenticate, Fetch OTLP Endpoints, Create User
#   Tests: Get User, Update User
#   Teardown: Delete User
#
# Prerequisites:
#   - App Insights resource ID (in .env or variables)
#   - ARM access token (from setup block or dev mode)
# ============================================================

# --- Telemetry (endpoints extracted from ARM during setup) ---
@appinsights_resource_id = /subscriptions/your-subscription-id/resourceGroups/your-resource-group/providers/microsoft.insights/components/your-app-insights
@telemetry_traces_endpoint =
@telemetry_metrics_endpoint =
@telemetry_logs_endpoint =
@telemetry_token =
@telemetry_service = user-api-e2e

# --- Auth ---
@auth_url = https://login.microsoftonline.com/your-tenant-id
@client_id = your-client-id
@client_secret = your-client-secret
@auth_scope = https://api.example.com/.default
@arm_scope = https://management.azure.com/.default

# --- Environment ---
@base_url = https://api.example.com/v1

### @@setup Fetch ARM Token
# @@mode app
# @@dev_auth https://management.azure.com/.default
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{arm_scope}}

# @@extract telemetry_token = $.access_token
# @@assert status == 200

### @@setup Fetch OTLP Endpoints
GET https://management.azure.com{{appinsights_resource_id}}?api-version=2025-01-23-preview
Authorization: Bearer {{telemetry_token}}

# @@extract telemetry_traces_endpoint = $.properties.OTLPTracesEndpoint
# @@extract telemetry_metrics_endpoint = $.properties.OTLPMetricsEndpoint
# @@extract telemetry_logs_endpoint = $.properties.OTLPLogsEndpoint
# @@assert status == 200

### @@setup Authenticate API
# @@mode app
# @@dev_auth https://api.example.com/.default
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @@extract access_token = $.access_token
# @@assert status == 200

### @@setup Create User
POST {{base_url}}/users
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "name": "e2e-user-{{$uuid}}", "email": "e2e-{{$uuid}}@test.com" }

# @@extract user_id = $.id
# @@assert status == 201

### @@test Get User
GET {{base_url}}/users/{{user_id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.id == {{user_id}}

### @@test Update User
PATCH {{base_url}}/users/{{user_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "name": "updated-e2e-user" }

# @@assert status == 200

### @@teardown Delete User
DELETE {{base_url}}/users/{{user_id}}
Authorization: Bearer {{access_token}}

# @@assert status >= 200
# @@assert status <= 204
```

**Rules:**
- Set `telemetry_traces_endpoint`, `telemetry_metrics_endpoint`, and/or `telemetry_logs_endpoint` variables — if any is non-empty, telemetry auto-enables
- Endpoints can be set directly as top-level `@name = value` definitions or extracted via `@@setup` blocks (e.g., ARM API calls to fetch App Insights OTLP endpoints)
- Use `telemetry_token` for Bearer auth or `telemetry_api_key` for `x-ms-ikey` header
- **RBAC requirement:** The authenticated identity (user or service principal) must have the **Monitoring Metrics Publisher** role assigned on the Data Collection Rule (DCR) associated with the App Insights resource. This role grants `Microsoft.Insights/Metrics/Write` and `Microsoft.Insights/Telemetry/Write` dataActions needed for OTLP ingestion.
- `telemetry_service` is optional — defaults to the filename
- Telemetry export never fails the test run — errors are captured in stats
- The desktop app shows a 📡 indicator when telemetry is configured

### Pattern 10: Response Comparison (`@@compare`)

When an API is being migrated, versioned, or A/B tested, use `@@compare` to execute two requests within a single test block and diff their responses. This is useful for:
- **API migration parity** — verify v1 and v2 return equivalent data
- **A/B testing** — compare responses from two backends or feature flag states
- **Version comparison** — ensure a refactored endpoint matches the original

```http
### @@test API v1 vs v2 Parity — User Endpoint
# @@compare
# @@description Compares v1 and v2 user responses to ensure migration parity

# @@step baseline
GET {{base_url}}/api/v1/users/{{user_id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@extract v1_user_name = $.name

# @@step candidate
GET {{base_url}}/api/v2/users/{{user_id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@extract v2_user_name = $.name

# @@diff baseline candidate
# @@assert $diff.match == true
# @@assert $diff.similarity >= 0.95
# @@assert $diff.changed_count == 0
```

**`$diff` variable reference:**

| Path | Type | Description |
|------|------|-------------|
| `$diff.match` | bool | `true` if responses are identical |
| `$diff.similarity` | float | 0.0–1.0 similarity score |
| `$diff.is_json` | bool | `true` if both responses are valid JSON |
| `$diff.added_count` | int | Number of paths only in step B |
| `$diff.removed_count` | int | Number of paths only in step A |
| `$diff.changed_count` | int | Number of paths with different values |
| `$diff.added_paths.length` | int | Length of added paths array |
| `$diff.removed_paths.length` | int | Length of removed paths array |
| `$diff.changed_paths.length` | int | Length of changed paths array |
| `$diff.changed_paths[N].path` | string | Path of Nth changed field |
| `$diff.changed_paths[N].left` | string | Step A value |
| `$diff.changed_paths[N].right` | string | Step B value |

**Rules:**
- ``# @@compare` is a modifier on a `@@test` (or `@@setup`/`@@teardown`) block — place it on the line after the block header
- ``# @@step <name>` defines a named step within the compare block; each step has its own request line, headers, body, assertions, and extracts
- ``# @@diff <step_a> <step_b>` triggers comparison between two named steps — place it after all steps
- Steps execute sequentially; each step's `@@assert` and `@@extract` directives evaluate immediately after that step's HTTP request completes
- Comparison assertions using `$diff.*` paths evaluate after ALL steps complete
- JSON responses get deep comparison with path-level diffs (e.g., `user.address.city`); non-JSON responses use character-level similarity scoring
- Use `$diff.similarity >= 0.95` to allow small acceptable drift during incremental migration rollouts
- Use `$diff.changed_count == 0` for strict parity checks where no field differences are allowed

### Pattern 11: Repeater (`# @@for` over a JSON-array variable)

When a setup step produces a list of values (IDs, names, objects) and the test must run once **per value**, use `# @@for <iter_var> in <source_var>` on the test block. The runner iterates sequentially, binding `iter_var` to each element. The test block — including its assertions and extracts — runs once per iteration; per-iteration extracts stay scoped to that iteration and do NOT leak into the global variable store.

#### Scalar element loop (V1 happy path)

```http
### @@setup Get user list
# @@description Fetches the array of user IDs to drive the loop below.
GET {{base_url}}/users
Authorization: Bearer {{access_token}}

# @@extract user_ids = $.user_ids
# @@assert status == 200
# @@assert $.user_ids.length > 0

### @@test Validate each user
# @@for user_id in user_ids
# @@description Runs once per element in user_ids — N HTTP requests, N assertion sets.
GET {{base_url}}/users/{{user_id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.id == {{user_id}}
```

#### Object element loop (`{{item.field}}` traversal)

When the source variable holds an array of objects, bind to each object and reach fields with dotted paths. The element variable behaves like any other JSON-valued variable:

```http
### @@setup Get user objects
GET {{base_url}}/users
# @@extract users = $.users           # users is a JSON array like [{"id":"u1","email":"a@x"},…]
# @@assert status == 200

### @@test Each user matches expected email
# @@for user in users
# @@description Per-iteration: user.id and user.email available via dotted paths.
GET {{base_url}}/users/{{user.id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.id == {{user.id}}
# @@assert $.email == {{user.email}}
```

#### Built-in iteration counters

Inside a `# @@for` block, two extra variables are auto-bound on every iteration:

| Variable | Type | Value |
|----------|------|-------|
| `{{$index}}` | integer | 0-based iteration index |
| `{{$iteration}}` | integer | 1-based iteration index (for human-readable names) |

```http
### @@test Tag each created resource with its iteration
# @@for tag in tag_list
POST {{base_url}}/resources/{{resource_id}}/tags
Content-Type: application/json

{ "tag": "{{tag}}", "order": {{$iteration}} }

# @@assert status == 201
# @@assert $.order == {{$iteration}}
```

#### Source-variable contract

- `<source_var>` MUST be the **bare** variable name — NOT `{{source_var}}`. The directive parser reads it directly, identical to `# @@group <name>` and `# @@depends <name>`.
- The variable's value MUST be a **JSON array string** at run time. Use `# @@extract foo = $.path.to.array` to populate it.
- An empty array runs zero iterations and the block status is `passed` (not `skipped`).
- A missing or non-JSON-array source is a block-level error (`Loop source 'X' is undefined` / `is not a JSON array`).

#### Constraints (V1)

| Rule | Why |
|------|-----|
| Sequential execution only | `# @@parallel` deferred to V1.5 |
| Cannot combine `# @@compare` and `# @@for` on the same block | Parser rejects with a clear error — they'd produce ambiguous result shapes |
| Per-iteration extracts stay local | The final variable store excludes per-iteration extracts. Use a downstream block to aggregate, or wait for V1.5's `# @@collect` directive. |
| Storage trims successful iteration bodies | Failed iterations + first/last success retain full bodies; successes 2..N-1 store summary only. The desktop UI annotates `(body omitted)`. |

#### Authoring tips

- Prefer scalar loops when the request only needs an ID or name. They're easier to read and the source variable is just `$.array_path`.
- For object loops, extract the array of objects in setup (`# @@extract users = $.users`) so the iter binding can dot-traverse without a second HTTP round-trip.
- Combine `# @@for` with `# @@group` to fan out a per-element validation group, then run a downstream block via `# @@depends` after all iterations pass.

## Common Pitfalls

### Never write into the Sessions store
Request Pilot persists test runs under a user-configured **sessions root**
(see `docs/sessions.md`). The directory layout is
`<root>/files/<file-id>/versions/<sha256>/sessions/<run-id>/...`. Files
under that tree are produced **exclusively** by the Request Pilot runtime
and are content-addressed by `sha256` of the LF-normalized source. **Never
generate `.http` files into a sessions root.** If a user asks you to "add a
test to my sessions directory", clarify and write the new `.http` file
elsewhere (typically `tests/`, `e2e/`, or `docs/`). Writing into the
sessions tree corrupts version identity and can shadow real run records.

### Comments leaking into request body
**Problem:** Section decoration comments (`# ====`, `# ----`, `# Group: xxx`) placed between blocks (before the next `###`) become part of the previous block's HTTP body, causing 400 errors.

**Solution:** Always use an empty `###` separator before section comments:
```http
### @@test Previous test
GET {{base_url}}/api
# @@assert status == 200

###
# ============================================================
#  SECTION HEADER
# ============================================================

### @@test Next test
GET {{base_url}}/other
```

### Special characters in GET query strings
**Problem:** PromQL and similar query languages use `{`, `}`, `*`, spaces, and parentheses that can break URL parsing.

**Solution:** Use POST with `Content-Type: application/x-www-form-urlencoded` and put the query in the body. Most query APIs (Prometheus, Grafana, etc.) support both GET and POST.

> 💡 **Authoring `application/x-www-form-urlencoded` bodies in Request Pilot:** prefer **human-readable form** — write spaces, parentheses, brackets, braces, quotes, commas, and newlines as literal characters. Pilot's runtime auto-encodes the body for the wire on send. You only need to encode four characters in your source (`%` `&` `=` `+`) — see the **Form-urlencoded body encoding** section below for the full rules. **Portability note:** decoded bodies require Pilot's runtime auto-encoder; if you also need the file to work in REST Client (VS Code), IntelliJ HTTP, or `curl --data`, you'll need to fully wire-encode the body instead.

### Form-urlencoded body encoding
**Recommended (Pilot-native):** Author form bodies in **human-readable form** — leave structurally-irrelevant characters (parens, braces, quotes, spaces, newlines, etc.) as literal characters. Pilot's runtime auto-encodes the body before sending: it preserves any `%XX` escape sequences you wrote and encodes everything else per `application/x-www-form-urlencoded` rules. The most common gotcha is **literal `+` characters** (e.g. PromQL operators between vector expressions): the server URL-decodes `+` → space, so the operator silently vanishes. Always write literal `+` as `%2B`.

**Encoding rules for the source `.http` file:**

| Source character | Write as | Why |
|---|---|---|
| space, newline, tab | literal | runtime encodes to `+` / `%0A` / `%09` |
| `(` `)` `[` `]` `{` `}` | literal | runtime encodes to `%XX` |
| `,` `:` `"` `'` `!` `*` `/` `<` `>` etc. | literal | runtime encodes to `%XX` (or passes through if unreserved) |
| non-ASCII (e.g. `é`, `中`) | literal | runtime UTF-8 encodes to `%XX%XX...` |
| `+` (literal `+` operator) | `%2B` | otherwise server decodes to space (silent bug) |
| `%` (literal `%` character) | `%25` | otherwise looks like the start of an escape |
| `&` (inside a value) | `%26` | otherwise splits the body into two pairs |
| `=` (inside a value) | `%3D` | optional — only the FIRST `=` per pair is structural; the runtime preserves later `=` chars in values, but encoding them avoids ambiguity |

Pair delimiters (`&` between pairs) and the FIRST `=` per pair stay unencoded — they're structural.

**Wrong (PromQL bug):**
```http
POST {{endpoint}}/api/v1/query
Content-Type: application/x-www-form-urlencoded

query=sum by (pod) (
    ({"up"} * 1)
    +
    ({"down"} * 2)
)&time={{time}}
```
The literal `+` operator decodes to space server-side → "parse error".

**Correct (human-readable, Pilot auto-encodes on send):**
```http
POST {{endpoint}}/api/v1/query
Content-Type: application/x-www-form-urlencoded

query=sum by (pod) (
    ({"up"} * 1)
    %2B
    ({"down"} * 2)
)&time={{time}}
```

**Also correct (fully wire-encoded — portable to other `.http` clients):**
```http
POST {{endpoint}}/api/v1/query
Content-Type: application/x-www-form-urlencoded

query=sum+by+%28pod%29+%28%0A++++%28%7B%22up%22%7D+*+1%29%0A++++%2B%0A++++%28%7B%22down%22%7D+*+2%29%0A%29&time={{time}}
```
Use this form if the `.http` file must run in REST Client, IntelliJ HTTP, `curl`, etc. — those tools don't auto-encode and will send the raw bytes.

**Authoring tips:**
- When capturing requests via the browser extension or HAR import, the body is captured already wire-encoded by the browser. **Decode it for readability before writing to the `.http` file** — you can paste it into any URL-decoding helper, then re-encode the four ambiguous chars (`%` `&` `=` `+` if they appear in values) per the table above. Pilot will re-encode on send.
- When hand-authoring, just write the body the way it reads naturally. Only worry about encoding `+` (when used as a literal operator), `%`, `&`-inside-a-value, and `=`-inside-a-value.
- `{{var}}` placeholders are interpolated BEFORE the body is encoded, so leave them literal.

### Variables with empty values
**Problem:** Variables like `arm_token =` (empty value) are valid — they serve as placeholders that get populated by ``# @@extract` during setup. Don't remove them or add placeholder values that might accidentally be sent.

**Solution:** Keep empty variables for extracted values and document them with a comment:
```http
# --- Extracted at runtime (populated by @setup blocks) ---
@arm_token =
@query_endpoint =
@access_token =
```

### `# @@for` source variable wrapped in `{{...}}`
**Problem:** Writing `# @@for user_id in {{user_ids}}` looks intuitive but the parser reads `<source_var>` as a bare variable name, identical to `# @@group <name>` and `# @@depends <name>`. Wrapping in `{{}}` makes the directive lookup fail.

**Solution:** Use the bare name:
```http
# ✗ WRONG — interpolation form, the parser cannot resolve this
# @@for user_id in {{user_ids}}

# ✓ RIGHT — bare variable name
# @@for user_id in user_ids
```
`{{user_id}}` interpolation inside the request body / URL / headers IS still correct — the directive line is the only place that uses the bare name.

### `# @@for` combined with `# @@compare` on the same block
**Problem:** A loop iterates a single request shape; a compare block fans out N parallel-shaped requests. Combining them produces an ambiguous result tree (do you compare iteration 1 against iteration 2? Or run each compare-step N times?). V1 rejects the combination at parse time.

**Solution:** Pick one. Most "compare across N iterations" intents are better served by a `# @@for` over a list of values + post-loop assertion comparing extracts, not by `@@compare`.

### `# @@for` source is not a JSON array
**Problem:** `# @@extract user_ids = $.user_ids[*].id` does NOT work — extract paths don't support `[*]` wildcards. The variable ends up as a plain string and the loop fails with `Loop source 'user_ids' is not a JSON array`.

**Solution:** Have the upstream API return the IDs as a top-level array, then extract the whole array with a non-wildcard path:
```http
# Upstream returns: { "user_ids": ["u1", "u2", "u3"] }
# @@extract user_ids = $.user_ids

# ✗ Does NOT work — wildcards aren't supported in extract paths
# @@extract user_ids = $.users[*].id
```
If you only have a list of objects (`{ "users": [{...}, {...}] }`), extract the object array (`$.users`) and use object-element loops with `{{user.id}}`.

## How to Analyze a Code Change

1. **Find API endpoints** — look for route definitions, controller methods, API handlers, SDK calls, HTTP client usage
2. **Identify the HTTP method and path** — `GET /api/v1/users`, `POST /api/v1/orders`, etc.
3. **Extract request/response schemas** — what headers, query params, body fields, and response shapes are expected
4. **Map dependencies** — which endpoint must be called before another (e.g., create before read)
5. **Identify auth requirements** — bearer tokens, API keys, OAuth flows
6. **Note error cases** — what validation exists, what error codes are returned
7. **Check for side effects** — resources created that need cleanup (teardown)

## Output Rules

- **Define ALL variables at the top** in the top-level `@name = value` definitions — every hostname, version, credential, test data value
- **Environment-overridable values:** committed defaults live in the `.http` file; per-environment overrides (e.g. `BASE_URL`, `AUTH_SCOPE`, `CLIENT_ID`) live in standard `.env` files that users load at runtime via Pilot's env picker. Keep your `.http` variable placeholders named the same as the `.env` keys you expect (e.g. `base_url`) — Pilot resolves them automatically when an env is active.
- **One `.http` file per logical API group** (e.g., one for user management, one for billing)
- **File header comment** explaining what is tested and prerequisites
- **All URLs must use variables** — never hardcode hostnames or API versions
- **All auth tokens must be fetched dynamically** via `@@setup` blocks
- **Setup blocks form the dependency chain** — authenticate, then create resources in dependency order; each `@@extract` feeds the next step
- **Test blocks must be independent** — each `@@test` should only depend on variables from setup, never on another test's output. This keeps tests parallel-safe.
- **Use ``# @@group`** to group related parallel tests (e.g., ``# @@group validation` for all read-only validations)
- **Use ``# @@depends`** when a test or group needs another group to finish first (e.g., ``# @@depends create` before running validation)
- **Groups can depend on groups** — chain them to express nested execution: `create → validate → report`
- **Teardown in reverse dependency order** — delete children before parents (e.g., delete member → delete project)
- **Add ``# @@description`** to each block explaining its purpose
- **Test names must be descriptive** — `### @@test Get User Profile` not `### @@test Test 1`
- **Assertions must be comprehensive** — check status codes, key response fields, data types, relationships
- **Teardown must clean up everything** created during setup/test
- **Use `{{$uuid}}` in resource names** to avoid collisions between test runs
- **Use ``# @@mode app` on client-credentials auth blocks** — marks them as app-mode-only so dev mode can skip them and use Azure CLI user auth instead. Add ``# @@dev_auth <scope>` to specify the Azure scope for user auth.
- **Use ``# @@dev_auth <scope>` on `@@mode app` token-fetch blocks** — specifies the Azure scope for user auth. When the user authenticates via device code flow, the app fetches a user token with this scope and injects it into the block's `@@extract` variable.
- **Add telemetry variables for E2E observability** — when the test file should export OTEL telemetry (traces, metrics, logs), set `telemetry_traces_endpoint`, `telemetry_metrics_endpoint`, and/or `telemetry_logs_endpoint` variables. These can be set directly as top-level `@name = value` definitions or extracted via `@@setup` steps (e.g., fetching OTLP endpoints from ARM). Use `telemetry_token` for Bearer auth or `telemetry_api_key` for `x-ms-ikey`. Optionally set `telemetry_service` to customize the `service.name` resource attribute (defaults to filename).
- **Use ``# @@compare` for API migration and comparison scenarios** — when the code change involves versioned endpoints, A/B testing, or endpoint migration, generate a `@@compare` test block with ``# @@step` for each endpoint and ``# @@diff` to assert response parity. Use `$diff.match`, `$diff.similarity`, and `$diff.changed_count` assertions to validate equivalence (see Pattern 10).
- **Use ``# @@auto_run <interval>` for continuous monitoring** — add as a file-level directive (before any `@name = value` definitions) when tests should auto-repeat. Valid intervals: `30s`, `1m`, `5m`, `15m`, `1h`, `2h`, `4h`, `1d`. Both TUI and desktop show a toolbar control; the directive sets the default. Example: ``# @@auto_run 15m`
- **Use ``# @@request-id [header]` to tag every request with a unique id** — add as a file-level directive when downstream services log a correlation/request-id header. A fresh UUIDv4 is injected into every request under the named header (default `X-Request-Id`) unless the block already sets that header. Examples: ``# @@request-id` (uses `X-Request-Id`), ``# @@request-id X-Correlation-ID`. Per-block override: ``# @@request-id X-Other-Id`. Opt out for a specific block: ``# @@request-id off`. Useful for tracing test calls in server logs.
- **Comments explain non-obvious logic** — especially complex assertions or why a specific test exists
- **Always start with a Test Plan header comment** — a structured comment block listing all test scenarios, grouped by type and group name, with prerequisites and assertion summary (see Pattern 7)
- **Section decoration comments MUST be placed AFTER `###`, not before**— the parser splits the file at `###` boundaries, so any comments between blocks that appear before the next `###` become part of the previous block's request body. Put section headers, group labels, and decorative separators immediately after a `###` line:
  ```http
  ### @@test Last test in previous group
  GET {{base_url}}/api
  # @@assert status == 200

  ### 
  # --------------------------------------------------
  # Group: next-group
  # Description of what this group tests
  # --------------------------------------------------

  ### @@test First test in next group
  GET {{base_url}}/other
  ```
  Note the empty `###` separator before the section comment block — this ensures comments don't leak into the previous test's body.
- **Use POST with form-urlencoded body for complex query parameters** — if a query value contains spaces, braces, parentheses, or other special characters (e.g., PromQL expressions), use a POST request with the parameters in the body instead of cramming them into the URL query string. **Author the body in human-readable form** — leave structural characters (parens, braces, quotes, spaces, newlines) literal; encode only `%`, `&`-in-values, `=`-in-values, and `+`-as-literal-operator (see the "Form-urlencoded body encoding" section above for full rules). Pilot auto-encodes the body for the wire on send.
  ```http
  # ✗ BAD — spaces and braces in GET query cause issues
  GET {{endpoint}}/api/v1/query_range?query={"system.cpu.time"} * on() group_left up&start={{start}}

  # ✗ BAD — POST with literal `+` operator decoded to space by the server. Parse error.
  POST {{endpoint}}/api/v1/query_range
  Content-Type: application/x-www-form-urlencoded

  query={"system.cpu.time"} * on() group_left up + 1&start={{start}}&end={{end}}&step={{step}}

  # ✓ GOOD — POST with human-readable body. Pilot auto-encodes on send.
  POST {{endpoint}}/api/v1/query_range
  Content-Type: application/x-www-form-urlencoded

  query={"system.cpu.time"} * on() group_left up %2B 1&start={{start}}&end={{end}}&step={{step}}
  ```
  Note: `{{var}}` placeholders are resolved BEFORE encoding — leave them unencoded in the source. The literal `+` operator must be written `%2B` because `+` means space in form-urlencoded; everything else (spaces, parens, quotes, braces) stays literal.
- **Body is terminated by trailing comments** — the parser strips trailing comment lines and empty lines from the request body. Do not rely on `#` comments being part of the body. If you need literal `#` content in a body, ensure it is not the last line (add actual body content after it).

## Example: Analyzing a Code Change

If the diff shows a new REST controller with these endpoints:
- `POST /api/v1/projects` (create project)
- `GET /api/v1/projects/{id}` (get project)
- `PATCH /api/v1/projects/{id}` (update project)
- `DELETE /api/v1/projects/{id}` (delete project)
- `POST /api/v1/projects/{id}/members` (add member)
- `GET /api/v1/projects/{id}/members` (list members)

You would produce:

```http
# ============================================================
# Project Management API — E2E Tests
#
# Tests the full project lifecycle including member management:
# CRUD operations, member addition, listing, and error handling.
#
# Test Scenarios:
#   Setup:
#     1. Authenticate — OAuth2 client credentials flow
#     2. Create Project — provision test project with unique name
#
#   Tests:
#     (ungrouped — parallel)
#       - Get Project — verify created project is retrievable
#       - Update Project — update description with timestamp
#       - Add Member to Project — add contributor, extract member ID
#       - List Project Members — verify member exists
#       - Get Non-Existent Project Returns 404 — error handling
#
#   Teardown:
#     1. Delete Project — cascading cleanup of project + members
#
# Prerequisites:
#   - API server running at base_url
#   - Valid auth credentials with project admin role
#
# Metrics:
#   - Total: 5 tests, 2 setup, 1 teardown
#   - Key assertions: CRUD status codes, field values, 404 handling
# ============================================================

# --- Authentication ---
@auth_url = https://auth.example.com
@client_id = your-client-id
@client_secret = your-client-secret
@auth_scope = https://api.example.com/.default

# --- Environment ---
@base_url = https://api.example.com/api/v1

# --- Test Data ---
@project_name = e2e-test-project
@member_email = testmember@example.com

### @@setup Authenticate
# @@description Fetches OAuth2 token using client credentials flow
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @@extract access_token = $.access_token
# @@assert status == 200

### @@setup Create Project
# @@description Creates a test project with a unique name for isolation
POST {{base_url}}/projects
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "name": "{{project_name}}-{{$uuid}}",
  "description": "E2E test project created at {{$timestamp}}"
}

# @@extract project_id = $.id
# @@extract project_name_actual = $.name
# @@assert status == 201
# @@assert $.id != null
# @@assert $.name contains {{project_name}}

### @@test Get Project
# @@description Verifies the created project can be retrieved by ID
GET {{base_url}}/projects/{{project_id}}
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.id == {{project_id}}
# @@assert $.name == {{project_name_actual}}

### @@test Update Project
# @@description Updates the project description with a timestamp
PATCH {{base_url}}/projects/{{project_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "description": "Updated by E2E test at {{$timestamp}}"
}

# @@assert status == 200
# @@assert $.description contains Updated by E2E test

### @@test Add Member to Project
# @@description Adds a contributor member and extracts the member ID
POST {{base_url}}/projects/{{project_id}}/members
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "email": "{{member_email}}",
  "role": "contributor"
}

# @@extract member_id = $.id
# @@assert status == 201
# @@assert $.email == {{member_email}}
# @@assert $.role == contributor

### @@test List Project Members
# @@description Verifies at least one member exists in the project
GET {{base_url}}/projects/{{project_id}}/members
Authorization: Bearer {{access_token}}

# @@assert status == 200
# @@assert $.length > 0
# @@assert $[0].email == {{member_email}}

### @@test Get Non-Existent Project Returns 404
# @@description Validates proper 404 handling for missing resources
GET {{base_url}}/projects/00000000-0000-0000-0000-000000000000
Authorization: Bearer {{access_token}}

# @@assert status == 404

### @@teardown Delete Project (cascades members)
# @@description Cleans up the test project and all associated members
DELETE {{base_url}}/projects/{{project_id}}
Authorization: Bearer {{access_token}}

# @@assert status >= 200
# @@assert status <= 204
```
