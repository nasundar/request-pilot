# Sessions — persisted test-run history

Request Pilot's **Sessions** feature lets you persist every test run to a local
directory so that:

- A continuous-testing setup (CI, scheduled runner) can dump results that you
  later browse, drill into, and replay locally.
- You can audit trends across versions of the same `.http` file.
- A teammate can hand you a sessions directory on a share and you can open
  any run, see exactly what was sent, what came back, and what asserted.

> Sessions is **off by default**. Nothing is written until you explicitly
> configure a sessions root path. Captured data follows a privacy-by-default
> policy (see *Capture & redaction* below).

---

## Quick start (Desktop)

1. Open the **🗂 Sessions** tab in the top toolbar.
2. Click **⚙ Settings** and enter an absolute path for the sessions root
   (e.g. `C:\Users\me\rp-sessions` or `~/rp-sessions`). Click **Save**.
3. The **Auto-record on test runs** checkbox is on by default — every
   subsequent suite run will be recorded.
4. Run any `.http` file. The Sessions tab now lists the file, its current
   content version (a `sha256` of the LF-normalized source), and one
   session per run.

---

## On-disk layout

```
<root>/
  files/<file-id>/
    file.json                       — identity manifest (display name, observed paths)
    versions/<sha256>/
      source.http                   — LF-normalized source bytes (immutable)
      meta.json                     — version metadata
      sessions/<run-id>/            — run-id is a UUIDv7 (sortable + unique)
        run.json                    — full SessionRecord (immutable)
        summary.md                  — human-readable run summary
        blobs/<sha256>.bin          — large captured payloads (when policy retains them)
      .cache/                       — rebuildable: stats, HTML viewer (Phase 2)
```

Everything is content-addressed by the `sha256` of the LF-normalized source.
Two CRLF-vs-LF variants of the same file collapse to one version. A real
edit to the file produces a new version directory; old versions stay intact.

`run.json` and `source.http` are the only sources of truth. Everything under
`.cache/` can be deleted and rebuilt at any time.

All writes go through a temp file + `rename` so a crashed run never leaves
the store in a half-written state.

---

## File identity

The on-disk `file-id` is a hash of one of:

| Identity                                  | When                                        |
|-------------------------------------------|---------------------------------------------|
| **Alias** (e.g. `e2e-checkout`)           | User-supplied — most stable; survives moves |
| **Repo-relative** (`<repo>/<rel-path>`)   | File is inside a Git repo                   |
| **Absolute path** (lower-cased, slash-norm) | Last resort — marked `degraded_identity`  |
| **Unsaved buffer**                        | New buffer with no path yet                 |

Different identity *kinds* never collide (each has a `alias-` / `repo-` /
`abs-` / `unsaved-` prefix in the `file-id`).

The desktop currently auto-detects repo-relative identity by walking up
looking for a `.git` directory. Aliases (most stable) will be exposed in the
UI in a follow-up.

---

## Capture & redaction

Each run records the *exact* `CapturePolicy` it used and a `RedactionReport`
(how many headers / queries / variables / bodies were dropped or
truncated). Three built-in presets are exposed in **⚙ Settings → Capture**:

### 📸 Snapshot (default)

Designed for the "view / replay what actually ran" use case — request and
response bodies are kept (capped at 10 MB each) so loading a session
reconstitutes the full test state.

| Field             | Snapshot default                       |
|-------------------|----------------------------------------|
| Variables         | `NamesOnly` — values dropped           |
| Request bodies    | `Truncated(10 MB)`                     |
| Response bodies   | `Truncated(10 MB)`                     |
| Header denylist   | `authorization`, `cookie`, `set-cookie`, `x-api-key`, `proxy-authorization`, `www-authenticate` |
| Query denylist    | `token`, `key`, `secret`, `signature`, `code` |

### 🔒 Privacy-first

For shared / production use cases — request bodies are dropped entirely
and response bodies capped at 1 MB.

### 🐛 Full debug

For local-only debugging on non-sensitive endpoints — captures everything
with no redaction, no truncation. **Do not enable** when running against
real auth tokens or PII.

Values matching the denylists are replaced with the literal string
`<redacted>`. The denylists are case-insensitive. You can audit what each
run dropped via the `redaction_report` field of `run.json` or the
**Redaction** line in `summary.md`.

---

## Loading a snapshot

In the Sessions tab (desktop **🗂 Sessions** or TUI **Sessions** tab),
select any session row to load it as a snapshot. This restores the full
test state at the moment the run executed:

- The original `.http` source as it was when the run executed
- Block statuses (✓ / ✗ / ⊘) and assertion outcomes
- Captured request and response payloads in each block's response panel
- Redaction summary in the snapshot banner

A snapshot is **read-only by default** — there's a persistent left-edge
warning stripe on the desktop editor (and a yellow snapshot banner with
run-id, recorded date, and short sha in the TUI). Edit keys are blocked
in the TUI; inputs are disabled in the desktop. Use **🔓 Detach to edit**
(desktop) or **D** (TUI) to unlock the file. A confirm dialog warns you
that the original snapshot stays intact on disk; on Yes the file becomes
an unlocked unsaved buffer ready for editing.

### Replaying a snapshot

Both clients can re-run a loaded snapshot from its original source:

| Action          | Desktop                       | TUI                       |
|-----------------|-------------------------------|---------------------------|
| Replay all      | **▶ Replay all** in banner    | **Shift-R**               |
| Replay one block| (per-block ▶ in detail)       | **r** on the focused block|
| Detach to edit  | **🔓 Detach to edit** in banner | **D**                   |

A confirm modal shows the mode, env, and target host before any network
traffic is sent. Replays:

- Use the **current** environment (the snapshot's env file path is shown
  for reference but variables are looked up at run time)
- Always create a **new session** under the same `file_id`, so trends
  accumulate across runs and group with the original
- Never modify the snapshot you're viewing

---

## Stats

Every recorded run is folded into two `StatsRecord` rollups:

- `<root>/files/<file-id>/versions/<sha>/stats.json` — per-version
- `<root>/files/<file-id>/stats.json` — file-level rollup across all versions

Updates are **incremental** — each run's outcome is folded into the
existing `stats.json` with a temp-file + atomic rename, so partial
crashes never corrupt the rollup. If a `stats.json` is ever lost or
suspect, `SessionStore::rebuild_stats(file_id)` rebuilds both files
from scratch by replaying every `run.json` on disk.

### What gets computed

For each rollup:

- **Totals** — passed / failed / mixed run counts, skipped blocks,
  session count, first-seen and last-seen timestamps.
- **Hourly buckets** — last **24h**, one bucket per hour. Each holds
  `runs`, `passed`, `failed`, and bucket-local `p50_ms` / `p95_ms`.
- **Daily buckets** — last **30d**, one bucket per day, same shape.
- **Per-block aggregates** — keyed by block name: runs / passed /
  failed / skipped plus `p50_ms` / `p95_ms` over a capped sample of
  recent latencies.
- **Latency** — overall `p50` / `p95` / `p99` / `max` across all
  retained samples.

Older buckets fall off the rolling windows automatically as new runs
arrive.

### Markdown view

`render_stats_md(&stats)` renders a `StatsRecord` as human-friendly
markdown — totals, a 24h pass-rate sparkline, the top 5 flakiest blocks
(by fail rate), and a 24h `p50` latency sparkline. Sample shape:

```markdown
# Stats for tests/auth.http @ abcdef01

## Totals
- pass: 7, fail: 2, mixed: 1, skipped blocks: 0
- pass-rate: 70.0%
- session_count: 10
- window: 2025-01-01T00:00:00Z → 2025-01-02T00:00:00Z

## Pass rate (last 24h)
`▆█`

## Top 5 flakiest blocks
| block | pass | fail | flakiness |
|---|---|---|---|
| login | 7 | 3 | 30.0% |
| stable | 10 | 0 | 0.0% |

## Latency trend (p50)
`█▂`
```

### Desktop UI

- Each version group in the Sessions tree shows an inline **pass-rate
  strip** so you can spot regressions at a glance.
- Open a session and click the **📊 Stats** tab in the detail panel to
  see the rendered rollup. The frontend calls the
  `sessions_get_stats(file_id, sha?)` Tauri command and gets back the
  full `StatsRecord` JSON — pass `sha` for the per-version stats, omit
  it for the file-level rollup.

---

## TUI Sessions tab

Press **S** or **F4** from any other tab to switch to the Sessions tab.
It reads the same `<config_dir>/request-pilot/sessions_config.json` as
the desktop (sessions root, auto-record, capture policy/preset) — there
is no separate TUI config.

### Layout

```
┌─ Groups ──────┬─ Sessions ────────────────┬─ Detail ───────────────────┐
│ ▸ tests/auth  │ 2025-01-02 14:03  ✓ pass  │ run-id: 0193b1...           │
│   tests/cart  │ 2025-01-02 11:50  ✗ fail  │ sha:    abcdef01            │
│   tests/feed  │ 2025-01-02 09:12  ⊘ mixed │ blocks:                     │
│               │                           │   ✓ login    120 ms         │
│               │                           │   ✗ checkout 980 ms (assert)│
└───────────────┴───────────────────────────┴─────────────────────────────┘
```

### Keybinds

| Key            | Action                                                   |
|----------------|----------------------------------------------------------|
| **S** / **F4** | Enter the Sessions tab                                   |
| **Tab** / **g**| Cycle group-by: File → Date → Version                    |
| **f**          | Cycle status filter: all → pass → fail → mixed → ...     |
| **/**          | Search (filters group + session lists by substring)      |
| **r**          | Refresh the listing from disk                            |
| **Enter**      | Load the selected session as a snapshot (locks the file) |
| **Shift-R**    | Replay all blocks from the loaded snapshot (confirm)     |
| **r** *(on a focused block, snapshot loaded)* | Replay just that block (confirm) |
| **D**          | Detach the locked snapshot to an editable buffer         |

Loading a session pushes a locked `LoadedFile` with a yellow snapshot
banner showing run-id / recorded date / short sha. Edit keys are
blocked until you **D**etach. Replays are recorded under the same
`file_id`, so they group with the original session in every view.

---

## CLI

The `request-pilot` binary (in the `cli/` workspace member) ships a
`sessions` subcommand that operates on the same on-disk store as the
desktop and TUI clients. It reads the shared
`<config_dir>/request-pilot/sessions_config.json`, so any sessions root
configured from the desktop is picked up automatically.

```bash
$ request-pilot sessions list
$ request-pilot sessions list --file alias-9f4c2b81d3e7a012
$ request-pilot sessions list --since 24h
```

`list` prints a fixed-width, grep-friendly table of recorded runs.
Filter by `--file <file-id>` and/or `--since <duration>` (`humantime`
format, e.g. `15m`, `24h`, `7d`). The `<file-id>` is the exact value
shown in the `FILE` column of `sessions list` (e.g.
`alias-9f4c2b81d3e7a012`, `repo-…`, `abs-…`, or `unsaved-…`) — it is
**not** the human alias / display name.

```bash
$ request-pilot sessions show 0193b1a43d2c7e1faa9c4f0e2d1b3c40
$ request-pilot sessions show 0193b1a43d2c7e1faa9c4f0e2d1b3c40 --json
```

`show` pretty-prints a single run (totals, per-block table, redaction
report, capture policy). Pass `--json` to dump the raw `SessionRecord`
for scripting. Run IDs are unhyphenated 32-character UUIDv7 strings —
copy them verbatim from `sessions list`.

```bash
$ request-pilot sessions stats alias-9f4c2b81d3e7a012
$ request-pilot sessions stats alias-9f4c2b81d3e7a012 \
    --sha 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
$ request-pilot sessions stats alias-9f4c2b81d3e7a012 --json
```

`stats` renders the markdown rollup for a file (file-level by default,
per-version with `--sha <sha>`, raw `StatsRecord` JSON with `--json`).
`--sha` requires the **full** 64-character SHA-256 of the LF-normalized
source — copy it from a `versions/` directory name or the `sha256`
field of any `run.json`.

```bash
$ request-pilot sessions export 0193b1a43d2c7e1faa9c4f0e2d1b3c40 --format md -o run.md
$ request-pilot sessions export 0193b1a43d2c7e1faa9c4f0e2d1b3c40 --format json
```

`export` writes a run as JSON or Markdown. With no `-o/--output`, the
rendered document is printed to stdout. The Markdown form caps each
captured payload at 4 KB (longer payloads are truncated with a marker).

```bash
$ request-pilot sessions replay 0193b1a43d2c7e1faa9c4f0e2d1b3c40
$ request-pilot sessions replay 0193b1a43d2c7e1faa9c4f0e2d1b3c40 --block "List Users"
$ request-pilot sessions replay 0193b1a43d2c7e1faa9c4f0e2d1b3c40 --env tests/staging.env
```

`replay` re-executes a recorded run from its captured `source.http`. Use
`--block <name>` to run just one block, and `--env <path>` to merge a
KEY=VALUE env file into the suite-level variables. The new run is
recorded under the **original** `file_id`, so it groups with the
session you replayed.

```bash
$ request-pilot sessions prune --older-than 30d --keep-last 50 --dry-run
$ request-pilot sessions prune --max-size 5gb --force
```

`prune` applies a retention policy on demand (see *Retention* below).
At least one of `--older-than`, `--keep-last`, or `--max-size` is
required. `--dry-run` reports what would be removed without touching
disk; `--force` skips the interactive confirmation prompt (CI-friendly).

---

## Retention

Sessions accumulate forever by default. To bound disk usage, configure
an optional `retention` block in `sessions_config.json`:

```json
{
  "root": "/Users/me/rp-sessions",
  "auto_record": true,
  "retention": {
    "max_age_days": 30,
    "max_sessions_per_version": 50,
    "max_total_size_gb": 5.0
  }
}
```

Every field is optional; a missing field means "no cap for that
dimension". The desktop performs a **best-effort startup prune** when
any cap is set — failures are logged but never block startup. Run
`request-pilot sessions prune` for ad-hoc enforcement (CI, cron) or
when changing caps without restarting the desktop.

`--older-than` accepts `humantime` durations (`30d`, `12h`, `2weeks`)
and rounds to whole days. Sub-day inputs that round below 1 day are
rejected; values like `24h` or `36h` are accepted (rounded to 1 day).
`--max-size` accepts `gb`/`g` (default) or `mb`/`m` suffixes (e.g.
`5gb`, `512mb`). The oldest sessions are removed first; the immutable
`source.http` and `meta.json` for each version remain intact even after
every session under them is pruned.

---

## Body redaction

Headers and query parameters are redacted by the capture policy
denylists; **body redaction** lets you scrub specific JSON fields or
regex patterns out of request and response bodies before they're
written to disk. Replacements appear as the literal string `[REDACTED]`
and are counted in `RedactionReport.body_fields_redacted`.

### Per-file and per-block directives

```http
# File-level rule — applies to every block in this file.
# @@redact body $.password,$.user.token
# @@redact body /Bearer\s+[A-Za-z0-9._-]+/

### @@test Login
# Block-level rule — appended to file-level rules for this block only.
# @@redact body $.session.refresh_token
POST {{base_url}}/login
Content-Type: application/json

{ "email": "{{user_email}}", "password": "{{user_password}}" }
```

Two forms are recognised after `@@redact body`:

- **JSONPath** — `$.foo`, `$.foo.bar`, `$.foo[0]`, `$.foo[*]`. Multiple
  paths may be comma-separated on a single directive line. Applied to
  parsed JSON bodies; non-JSON bodies are left unchanged by JSONPath
  rules.
- **Regex** — `/pattern/` (slash-delimited). Applied to the raw body
  string regardless of content type, useful for stripping bearer tokens
  out of `application/x-www-form-urlencoded` or plaintext payloads.

> The supported JSONPath subset is intentionally small: `$.foo`,
> `$.foo.bar`, `$.foo[0]`, and `$.foo[*]`. Recursive descent (`$..foo`)
> and filter expressions are **not** supported. Paths that don't match
> are silently ignored.

### Global config

Patterns that should apply to every recorded session — regardless of
which `.http` file is run — go in `SessionsConfig.body_redaction_paths`:

```json
{
  "root": "/Users/me/rp-sessions",
  "body_redaction_paths": [
    "$.password",
    "$.user.token",
    "/Bearer\\s+[A-Za-z0-9._-]+/"
  ]
}
```

Each entry uses the same syntax as the `@@redact body` directive. Global
rules are merged with file- and block-level rules at recording time, so
configured-once secrets (e.g. an org-wide auth header pattern) never
need to be re-declared per file.

---

## End-to-end workflow

A complete example using the new Phase 3 features — configure once,
run, browse from the CLI, then archive.

**1. Configure the store and retention** in
`<config_dir>/request-pilot/sessions_config.json`:

```json
{
  "root": "/Users/me/rp-sessions",
  "auto_record": true,
  "capture_preset": "snapshot",
  "body_redaction_paths": [
    "$.access_token",
    "$.refresh_token",
    "/Bearer\\s+[A-Za-z0-9._-]+/"
  ],
  "retention": {
    "max_age_days": 30,
    "max_sessions_per_version": 50,
    "max_total_size_gb": 5.0
  }
}
```

**2. Add per-file scrubbing in your `.http` source** for anything the
global rules can't catch (e.g. form-encoded payloads):

```http
# @@redact body /client_secret=[^&\r\n]+/

### @@setup Fetch Token
POST {{auth_url}}/oauth2/v2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={{client_id}}&client_secret={{client_secret}}&scope={{scope}}

# @@extract access_token = $.access_token
# @@assert status == 200
```

**3. Run from the desktop, TUI, or CI.** Each run writes a redacted
`run.json` and folds into the per-file/per-version `stats.json`.

**4. Browse and triage from the CLI** — the `RUN_ID` and `FILE`
columns from `sessions list` feed directly into the other subcommands:

```bash
# What ran in the last day? Note RUN_ID and FILE values from the output.
$ request-pilot sessions list --since 24h

# Drill into the latest failure (RUN_ID is an unhyphenated 32-char UUIDv7)
$ request-pilot sessions show 0193b1a43d2c7e1faa9c4f0e2d1b3c40

# Markdown report for a teammate or a PR comment
$ request-pilot sessions export 0193b1a43d2c7e1faa9c4f0e2d1b3c40 --format md -o failure.md

# Re-run just the broken block against the current env
$ request-pilot sessions replay 0193b1a43d2c7e1faa9c4f0e2d1b3c40 --block "Checkout" --env tests/staging.env

# What's the trend look like? FILE is the exact `alias-…`/`repo-…`/`abs-…` id.
$ request-pilot sessions stats alias-9f4c2b81d3e7a012
```

**5. Trim disk usage on demand** (the desktop also runs this on
startup if `retention` is set):

```bash
$ request-pilot sessions prune --older-than 30d --keep-last 50 --max-size 5gb --dry-run
$ request-pilot sessions prune --older-than 30d --keep-last 50 --max-size 5gb --force
```

`--dry-run` shows what would be removed; `--force` skips the
confirmation prompt for unattended use.

---

## What's shipped

**Phase 1 — core store and desktop snapshot loading**

- ✅ `SessionStore` with content-addressed layout and atomic writes
- ✅ Privacy-by-default capture policy + redaction report
- ✅ UUIDv7 run-ids
- ✅ Markdown `summary.md` per run
- ✅ Desktop **🗂 Sessions** tab — tree view (files → versions →
  sessions), detail panel with per-block grid + redaction summary +
  source preview
- ✅ Desktop snapshot loading + **▶ Replay all** + **🔓 Detach to edit**
- ✅ Settings — sessions root, auto-record, capture preset, persisted at
  `<config_dir>/request-pilot/sessions_config.json`

**Phase 2 — stats and TUI parity**

- ✅ Per-version and file-level `stats.json` rollups (24h hourly + 30d
  daily buckets, per-block aggregates, latency p50/p95/p99/max),
  updated incrementally and rebuildable via `rebuild_stats`
- ✅ `render_stats_md` markdown view + desktop **📊 Stats** tab + inline
  pass-rate strip on each version group
- ✅ `sessions_get_stats(file_id, sha?)` Tauri command
- ✅ TUI **Sessions** tab — three-pane layout, group-by /
  filter / search / refresh, snapshot loading, **Shift-R** replay all,
  **r** replay one block, **D** detach
- ✅ Shared `sessions_config.json` between desktop and TUI

**Phase 3 — CLI, retention, and body redaction**

- ✅ `request-pilot sessions {list,show,stats,export,replay,prune}`
  CLI sharing the desktop/TUI store and config
- ✅ Retention policy (`max_age_days`, `max_sessions_per_version`,
  `max_total_size_gb`) with desktop best-effort startup prune and
  on-demand `sessions prune`
- ✅ Body redaction — `@@redact body $.path` (JSONPath) and
  `@@redact body /regex/` directives (file- and block-level), plus
  global `body_redaction_paths` in `sessions_config.json`

## Coming next

- "Re-run with captured variables" + warning that captured data may be stale
- "Re-run failed only"
- Compare-two-sessions

---

## FAQ

**Why not store mutable session ordinals?** Multiple runs can complete in
parallel (autorun + manual). UUIDv7 is sortable + unique with no
race-prone shared index.

**Why LF-normalize the source?** Otherwise CRLF-vs-LF flips fork the
version directory needlessly. The original byte size is preserved in
`meta.json`.

**Can I delete old sessions?** Yes — anything under `<root>/files/.../sessions/`
is safe to remove. `file.json` and `meta.json` will reflect the truth on next
run.

**Does the LLM (`/help`) generate files into the sessions directory?** No.
The skill file forbids it. Sessions are written only by the runner.
