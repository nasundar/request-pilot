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

Request Pilot `.http` files use an enhanced format with `@variables`, block types (`@setup`, `@test`, `@teardown`), assertions (`# @assert`), and variable extraction (`# @extract`). Blocks are separated by `###`.

### Variables Block

Always start with a `@variables` block containing ALL configurable values. Group them logically with comments:

```http
@variables
# --- Authentication ---
tenant_id = your-tenant-id
client_id = your-client-id
client_secret = your-client-secret

# --- Environment ---
base_url = https://api.example.com
api_version = 2024-01-01

# --- Test Data ---
test_user_email = testuser@example.com
test_resource_name = e2e-test-resource
```

**Rules for variables:**
- **Define ALL variables at the very beginning of the file** in the `@variables` block — this is mandatory
- Every URL hostname, API version, credential, and test data value MUST be a variable
- Use `your-` prefix for values the user must fill in (e.g. `your-subscription-id`)
- Use realistic defaults for non-sensitive values (e.g. `api_version = 2024-01-01`)
- Group variables by category with `#` comment headers
- Use `snake_case` for variable names
- Keep variable names descriptive but concise

### Block Types and Execution Order

Blocks execute in three phases: **setup (sequential) → test (parallel-safe) → teardown (sequential)**

| Block | Execution | Purpose | When to use |
|-------|-----------|---------|-------------|
| `@setup` | **Sequential** (top → bottom) | Initialize state | Auth token fetch, create prerequisite resources — each step can depend on variables extracted by the previous one |
| `@test` | **Parallel-safe** (independent) | Core assertions | Test each API endpoint — tests MUST NOT depend on each other's results, only on setup variables |
| `@teardown` | **Sequential** (top → bottom, always runs) | Clean up | Delete created resources — use reverse dependency order (child before parent) |

> **Key design principle:** Setup blocks form a dependency chain (authenticate → create parent → create child). Test blocks are independent validations that only read the state created during setup. This means tests can safely run in any order or in parallel.

### Directives

```http
# @name Human-readable name          — names the block in the UI
# @description Brief explanation      — subtitle shown in sidebar
# @disabled                           — skip this block during execution
# @group group-name                   — assign to a named group (parallel within group)
# @depends group-or-block-name        — wait for named group/block to complete first
# @assert status == 200               — validate response values
# @assert $.field != null              — JSON path assertions
# @assert $.items.length > 0          — array length checks
# @assert $.name contains partial     — substring match
# @extract var_name = $.json.path     — save response value for later blocks
# @mode app|dev                    — restrict to app mode or dev mode (mutually exclusive auth)
# @dev_auth <scope>               — Azure scope for user auth (used with @mode app)
# @telemetry <connection_var>      — (file-level) enable OTEL telemetry export using this variable's connection string
# @telemetry_service <name>        — (file-level) override the service.name resource attribute (defaults to filename)
```

**Assertion operators:** `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`
**Special values:** `null`, `!null`
**Paths:** `status`, `$.field`, `$.nested.field`, `$[0].field`, `$.field.length`, `$.headers.header-name`

### Built-in Variables

- `{{$timestamp}}` — Unix timestamp
- `{{$uuid}}` — Random UUID v4
- `{{$randomInt}}` — Random integer 0–9999

## Patterns to Follow

### Pattern 1: Authentication Setup

If the API requires authentication, ALWAYS add a setup block to fetch the token dynamically:

```http
### @setup Authenticate
# @name Fetch Access Token
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @extract access_token = $.access_token
# @assert status == 200
# @assert $.access_token != null
# @assert $.token_type == Bearer
```

Then use `Authorization: Bearer {{access_token}}` in all subsequent blocks.

### Pattern 2: CRUD Lifecycle

For any resource that is created, test the full lifecycle:

```http
### @setup Create Resource
POST {{base_url}}/resources
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "name": "{{test_resource_name}}-{{$uuid}}",
  "property": "value"
}

# @extract resource_id = $.id
# @assert status == 201
# @assert $.id != null

### @test Read Resource
GET {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}

# @assert status == 200
# @assert $.id == {{resource_id}}
# @assert $.name contains {{test_resource_name}}

### @test Update Resource
PUT {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "name": "updated-{{test_resource_name}}",
  "property": "new-value"
}

# @assert status == 200
# @assert $.name contains updated

### @test List Resources
GET {{base_url}}/resources
Authorization: Bearer {{access_token}}

# @assert status == 200
# @assert $.value.length > 0

### @teardown Delete Resource
DELETE {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}

# @assert status >= 200
# @assert status <= 204
```

### Pattern 3: Dependency Chain (Sequential Setup → Parallel Tests → Sequential Teardown)

When APIs depend on each other, use `@setup` blocks with `# @extract` to chain values. Tests read setup state but never depend on each other. Teardown reverses the creation order:

```http
### @setup Create Parent
# @description Creates parent resource (sequential — runs first)
POST {{base_url}}/parents
# ...
# @extract parent_id = $.id

### @setup Create Child (depends on Parent)
# @description Creates child under parent (sequential — uses parent_id from above)
POST {{base_url}}/parents/{{parent_id}}/children
# ...
# @extract child_id = $.id

### @test Verify Relationship
# @description Parallel-safe — only reads setup variables
GET {{base_url}}/parents/{{parent_id}}/children/{{child_id}}
# ...

### @test Verify Parent Has Children
# @description Parallel-safe — independent of "Verify Relationship" test above
GET {{base_url}}/parents/{{parent_id}}/children
# @assert status == 200
# @assert $.length > 0

### @teardown Delete Child
# @description Sequential cleanup — delete child first (reverse dependency order)
DELETE {{base_url}}/parents/{{parent_id}}/children/{{child_id}}

### @teardown Delete Parent
# @description Sequential cleanup — delete parent after child is removed
DELETE {{base_url}}/parents/{{parent_id}}
```

### Pattern 4: Error Case Testing

Test error responses with separate blocks:

```http
### @test Not Found Returns 404
GET {{base_url}}/resources/nonexistent-id-12345
Authorization: Bearer {{access_token}}

# @assert status == 404

### @test Unauthorized Without Token
GET {{base_url}}/resources
# (no Authorization header)

# @assert status == 401

### @test Bad Request With Invalid Body
POST {{base_url}}/resources
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "invalid_field": true }

# @assert status == 400
```

### Pattern 5: Idempotency / Retry Safety

For PUT/PATCH endpoints, verify idempotency:

```http
### @test Idempotent Update (first call)
PUT {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "status": "active" }

# @extract etag = $.headers.etag
# @assert status == 200

### @test Idempotent Update (second call — same payload)
PUT {{base_url}}/resources/{{resource_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "status": "active" }

# @assert status == 200
```

### Pattern 6: Grouped Parallel Tests with Dependencies

When multiple independent tests can run in parallel but depend on shared setup, use `# @group` and `# @depends` to express the execution graph:

```http
### @setup Authenticate
# @description Sequential setup — fetches token
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{scope}}

# @extract access_token = $.access_token
# @assert status == 200

### @setup Create Order
# @description Sequential setup — creates test order
POST {{base_url}}/orders
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "name": "e2e-order-{{$uuid}}" }

# @extract order_id = $.id
# @assert status == 201

### @test Validate Order Details
# @group validate-order
# @description Parallel-safe — runs alongside other tests in this group
GET {{base_url}}/orders/{{order_id}}
Authorization: Bearer {{access_token}}

# @assert status == 200
# @assert $.id == {{order_id}}

### @test Validate Order Items
# @group validate-order
# @description Parallel-safe — same group as "Validate Order Details"
GET {{base_url}}/orders/{{order_id}}/items
Authorization: Bearer {{access_token}}

# @assert status == 200

### @test Update Order Status
# @group update-order
# @depends validate-order
# @description Waits for validate-order group to complete
PATCH {{base_url}}/orders/{{order_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "status": "confirmed" }

# @assert status == 200

### @test Generate Invoice
# @depends update-order
# @description Waits for update-order group to complete
POST {{base_url}}/orders/{{order_id}}/invoice
Authorization: Bearer {{access_token}}

# @assert status == 201

### @teardown Delete Order
DELETE {{base_url}}/orders/{{order_id}}
Authorization: Bearer {{access_token}}

# @assert status >= 200
# @assert status <= 204
```

**Execution flow:**
1. `@setup` blocks run sequentially (Authenticate → Create Order)
2. `validate-order` group: both Validate tests run in parallel
3. `update-order` group: runs after `validate-order` completes
4. `Generate Invoice`: runs after `update-order` completes
5. `@teardown` runs last (always)

Groups can depend on other groups, forming nested execution chains. Blocks without `# @group` are standalone and follow the default execution rules for their block type.

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
- Group tests by their `@group` names
- Include a brief description for each test (matches `# @description`)
- List `Prerequisites:` so users know what's needed before running
- Include `Metrics:` with total test count and key assertion summary
- Keep descriptions concise — one line per test

### Pattern 8: Dev Mode — Azure CLI User Auth

When testing APIs that require Azure AD tokens, `.http` files typically have `@setup` blocks that fetch tokens via OAuth2 client credentials (requiring `client_id` and `client_secret`). In **dev mode**, developers can skip those blocks and use their own Azure CLI login (`az account get-access-token`) instead.

Use `# @mode app` on client-credentials setup blocks and `# @mode dev` on dev-mode-only blocks. They are mutually exclusive — only one runs depending on the active mode. Add `# @dev_auth <scope>` to specify the Azure scope for user auth on each `@mode app` token-fetch block.

#### Example: Dual-Mode Auth Setup

```http
### @setup Authenticate
# @mode app
# @dev_auth https://management.azure.com/.default
# @description Fetches token using client credentials (skipped in dev mode, replaced with user token)
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @extract access_token = $.access_token
# @assert status == 200

### @test List Users
# @description Works in both modes — uses access_token from either setup
GET https://graph.microsoft.com/v1.0/users?$top=5
Authorization: Bearer {{access_token}}

# @assert status == 200
# @assert $.value.length > 0
```

**How it works:**
- **App mode** (default): The `# @mode app` setup block runs, fetching a token via client credentials. Tests use the extracted `access_token`.
- **Dev mode** (toggle ON): The `# @mode app` block is skipped. The desktop app reads `# @dev_auth` from the block, authenticates the user for that scope via device code flow, and injects the token into the block's `@extract` variable. Tests run with the user's own token.

**Rules:**
- `# @mode app` blocks only run when dev mode is OFF
- `# @mode dev` blocks only run when dev mode is ON
- Blocks without `# @mode` always run (backward compatible)
- `# @dev_auth <scope>` on a `@mode app` block tells the app which Azure scope to request when authenticating the user
- The injected token uses the same variable name (`access_token`) so downstream tests work unchanged

### Pattern 9: E2E Observability — OTEL Telemetry

When test files need to export traces, metrics, and logs to an OTLP-compatible backend (e.g., Azure Monitor Application Insights), add the `# @telemetry` file-level directive. This instruments the entire test run with OTEL telemetry — no code changes needed in test blocks.

#### Telemetry Directives (file-level, before @variables)

```http
# @telemetry appinsights_connection_string
# @telemetry_service my-api-e2e-tests

@variables
# --- Telemetry (from .env) ---
appinsights_connection_string =

# --- Environment ---
base_url = https://api.example.com
```

**Connection string format (Azure Application Insights):**
```
InstrumentationKey=abc-123;IngestionEndpoint=https://eastus-1.in.applicationinsights.azure.com
```

Also supports plain OTLP endpoints: `https://my-otel-collector:4318`

#### What Gets Exported

| Signal | Content | Purpose |
|--------|---------|---------|
| **Traces** | Root span per suite, child spans per block, grandchild per HTTP request. Assertions/extracts as span events. | Investigation of failures |
| **Metrics** | `rp.suite.runs`, `rp.suite.duration`, `rp.block.runs`, `rp.block.duration`, `rp.assertion.total` — low-cardinality dimensions (file, block_type, outcome) | Dashboards and alerts |
| **Logs** | Structured log records for suite start, block completion, assertion failures, HTTP errors | Debugging and auditing |

#### Example: Full Test File with Telemetry

```http
# ============================================================
# User API — E2E Tests with OTEL Telemetry
#
# Exports traces, metrics, and logs to Application Insights
# on every run for dashboarding and alerting.
#
# Test Scenarios:
#   Setup: Authenticate, Create User
#   Tests: Get User, Update User
#   Teardown: Delete User
#
# Prerequisites:
#   - appinsights_connection_string in .env
# ============================================================

# @telemetry appinsights_connection_string
# @telemetry_service user-api-e2e

@variables
# --- Telemetry ---
appinsights_connection_string =

# --- Auth ---
auth_url = https://login.microsoftonline.com/your-tenant-id
client_id = your-client-id
client_secret = your-client-secret
auth_scope = https://api.example.com/.default

# --- Environment ---
base_url = https://api.example.com/v1

### @setup Authenticate
# @mode app
# @dev_auth https://api.example.com/.default
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @extract access_token = $.access_token
# @assert status == 200

### @setup Create User
POST {{base_url}}/users
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "name": "e2e-user-{{$uuid}}", "email": "e2e-{{$uuid}}@test.com" }

# @extract user_id = $.id
# @assert status == 201

### @test Get User
GET {{base_url}}/users/{{user_id}}
Authorization: Bearer {{access_token}}

# @assert status == 200
# @assert $.id == {{user_id}}

### @test Update User
PATCH {{base_url}}/users/{{user_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{ "name": "updated-e2e-user" }

# @assert status == 200

### @teardown Delete User
DELETE {{base_url}}/users/{{user_id}}
Authorization: Bearer {{access_token}}

# @assert status >= 200
# @assert status <= 204
```

**Rules:**
- `# @telemetry <var>` must appear before the `@variables` block (file-level directive)
- The variable value should be an Azure App Insights connection string or a plain OTLP endpoint URL
- Keep connection strings in `.env` files, not in the `.http` file
- `# @telemetry_service` is optional — defaults to the filename
- Telemetry export never fails the test run — errors are captured in stats
- The desktop app shows a 📡 indicator when telemetry is configured

## Common Pitfalls

### Comments leaking into request body
**Problem:** Section decoration comments (`# ====`, `# ----`, `# Group: xxx`) placed between blocks (before the next `###`) become part of the previous block's HTTP body, causing 400 errors.

**Solution:** Always use an empty `###` separator before section comments:
```http
### @test Previous test
GET {{base_url}}/api
# @assert status == 200

###
# ============================================================
#  SECTION HEADER
# ============================================================

### @test Next test
GET {{base_url}}/other
```

### Special characters in GET query strings
**Problem:** PromQL and similar query languages use `{`, `}`, `*`, spaces, and parentheses that can break URL parsing.

**Solution:** Use POST with `Content-Type: application/x-www-form-urlencoded` and put the query in the body. Most query APIs (Prometheus, Grafana, etc.) support both GET and POST. The body is sent as-is without URL encoding issues.

### Variables with empty values
**Problem:** Variables like `arm_token =` (empty value) are valid — they serve as placeholders that get populated by `# @extract` during setup. Don't remove them or add placeholder values that might accidentally be sent.

**Solution:** Keep empty variables for extracted values and document them with a comment:
```http
@variables
# --- Extracted at runtime (populated by @setup blocks) ---
arm_token =
query_endpoint =
access_token =
```

## How to Analyze a Code Change

1. **Find API endpoints** — look for route definitions, controller methods, API handlers, SDK calls, HTTP client usage
2. **Identify the HTTP method and path** — `GET /api/v1/users`, `POST /api/v1/orders`, etc.
3. **Extract request/response schemas** — what headers, query params, body fields, and response shapes are expected
4. **Map dependencies** — which endpoint must be called before another (e.g., create before read)
5. **Identify auth requirements** — bearer tokens, API keys, OAuth flows
6. **Note error cases** — what validation exists, what error codes are returned
7. **Check for side effects** — resources created that need cleanup (teardown)

## Output Rules

- **Define ALL variables at the top** in the `@variables` block — every hostname, version, credential, test data value
- **One `.http` file per logical API group** (e.g., one for user management, one for billing)
- **File header comment** explaining what is tested and prerequisites
- **All URLs must use variables** — never hardcode hostnames or API versions
- **All auth tokens must be fetched dynamically** via `@setup` blocks
- **Setup blocks form the dependency chain** — authenticate, then create resources in dependency order; each `@extract` feeds the next step
- **Test blocks must be independent** — each `@test` should only depend on variables from setup, never on another test's output. This keeps tests parallel-safe.
- **Use `# @group`** to group related parallel tests (e.g., `# @group validation` for all read-only validations)
- **Use `# @depends`** when a test or group needs another group to finish first (e.g., `# @depends create` before running validation)
- **Groups can depend on groups** — chain them to express nested execution: `create → validate → report`
- **Teardown in reverse dependency order** — delete children before parents (e.g., delete member → delete project)
- **Add `# @description`** to each block explaining its purpose
- **Test names must be descriptive** — `### @test Get User Profile` not `### @test Test 1`
- **Assertions must be comprehensive** — check status codes, key response fields, data types, relationships
- **Teardown must clean up everything** created during setup/test
- **Use `{{$uuid}}` in resource names** to avoid collisions between test runs
- **Use `# @mode app` on client-credentials auth blocks** — marks them as app-mode-only so dev mode can skip them and use Azure CLI user auth instead. Add `# @dev_auth <scope>` to specify the Azure scope for user auth.
- **Use `# @dev_auth <scope>` on `@mode app` token-fetch blocks** — specifies the Azure scope for user auth. When the user authenticates via device code flow, the app fetches a user token with this scope and injects it into the block's `@extract` variable.
- **Add `# @telemetry` for E2E observability** — when the test file should export OTEL telemetry (traces, metrics, logs), add `# @telemetry <connection_var>` as a file-level directive before `@variables`. The connection variable should hold an Azure App Insights connection string or a plain OTLP endpoint URL. Keep the actual connection string in a `.env` file. Optionally add `# @telemetry_service <name>` to set the `service.name` resource attribute (defaults to filename).
- **Comments explain non-obvious logic** — especially complex assertions or why a specific test exists
- **Always start with a Test Plan header comment** — a structured comment block listing all test scenarios, grouped by type and group name, with prerequisites and assertion summary (see Pattern 7)
- **Section decoration comments MUST be placed AFTER `###`, not before**— the parser splits the file at `###` boundaries, so any comments between blocks that appear before the next `###` become part of the previous block's request body. Put section headers, group labels, and decorative separators immediately after a `###` line:
  ```http
  ### @test Last test in previous group
  GET {{base_url}}/api
  # @assert status == 200

  ### 
  # --------------------------------------------------
  # Group: next-group
  # Description of what this group tests
  # --------------------------------------------------

  ### @test First test in next group
  GET {{base_url}}/other
  ```
  Note the empty `###` separator before the section comment block — this ensures comments don't leak into the previous test's body.
- **Use POST with form-urlencoded body for complex query parameters** — if a query value contains spaces, braces, parentheses, or other special characters (e.g., PromQL expressions), use a POST request with the parameters in the body instead of cramming them into the URL query string:
  ```http
  # ✗ BAD — spaces and braces in GET query cause issues
  GET {{endpoint}}/api/v1/query_range?query={"system.cpu.time"} * on() group_left up&start={{start}}

  # ✓ GOOD — use POST with form-urlencoded body
  POST {{endpoint}}/api/v1/query_range
  Content-Type: application/x-www-form-urlencoded

  query={"system.cpu.time"} * on() group_left up&start={{start}}&end={{end}}&step={{step}}
  ```
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

@variables
# --- Authentication ---
auth_url = https://auth.example.com
client_id = your-client-id
client_secret = your-client-secret
auth_scope = https://api.example.com/.default

# --- Environment ---
base_url = https://api.example.com/api/v1

# --- Test Data ---
project_name = e2e-test-project
member_email = testmember@example.com

### @setup Authenticate
# @description Fetches OAuth2 token using client credentials flow
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{auth_scope}}

# @extract access_token = $.access_token
# @assert status == 200

### @setup Create Project
# @description Creates a test project with a unique name for isolation
POST {{base_url}}/projects
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "name": "{{project_name}}-{{$uuid}}",
  "description": "E2E test project created at {{$timestamp}}"
}

# @extract project_id = $.id
# @extract project_name_actual = $.name
# @assert status == 201
# @assert $.id != null
# @assert $.name contains {{project_name}}

### @test Get Project
# @description Verifies the created project can be retrieved by ID
GET {{base_url}}/projects/{{project_id}}
Authorization: Bearer {{access_token}}

# @assert status == 200
# @assert $.id == {{project_id}}
# @assert $.name == {{project_name_actual}}

### @test Update Project
# @description Updates the project description with a timestamp
PATCH {{base_url}}/projects/{{project_id}}
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "description": "Updated by E2E test at {{$timestamp}}"
}

# @assert status == 200
# @assert $.description contains Updated by E2E test

### @test Add Member to Project
# @description Adds a contributor member and extracts the member ID
POST {{base_url}}/projects/{{project_id}}/members
Authorization: Bearer {{access_token}}
Content-Type: application/json

{
  "email": "{{member_email}}",
  "role": "contributor"
}

# @extract member_id = $.id
# @assert status == 201
# @assert $.email == {{member_email}}
# @assert $.role == contributor

### @test List Project Members
# @description Verifies at least one member exists in the project
GET {{base_url}}/projects/{{project_id}}/members
Authorization: Bearer {{access_token}}

# @assert status == 200
# @assert $.length > 0
# @assert $[0].email == {{member_email}}

### @test Get Non-Existent Project Returns 404
# @description Validates proper 404 handling for missing resources
GET {{base_url}}/projects/00000000-0000-0000-0000-000000000000
Authorization: Bearer {{access_token}}

# @assert status == 404

### @teardown Delete Project (cascades members)
# @description Cleans up the test project and all associated members
DELETE {{base_url}}/projects/{{project_id}}
Authorization: Bearer {{access_token}}

# @assert status >= 200
# @assert status <= 204
```
