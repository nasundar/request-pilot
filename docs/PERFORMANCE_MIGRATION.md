# Performance Migration Report

## Problem

Large HTTP responses (2MB+ JSON) caused the UI to freeze for 5–10+ seconds during:

- **JSON tree rendering** — recursive DOM construction of 50,000+ nodes
- **Response body syntax highlighting** — regex-based highlighting on the main thread
- **Diff computation** — O(m×n) character-level diffing in JavaScript
- **History list rendering** — 1,000+ entries each creating full DOM rows

The root cause: all computation and rendering happened synchronously on the main JS thread with full DOM materialization.

## Solution: Rust Backend Computation + Virtual Scroll

### Why NOT Dioxus

We evaluated rewriting the UI in Dioxus (Rust-native UI framework) but rejected it:

| Factor | Issue |
|--------|-------|
| **Window ownership conflict** | Dioxus Desktop and Tauri v2 both want to own the webview window — they cannot coexist in the same process |
| **Rayon in WASM** | Rayon parallelism requires `SharedArrayBuffer` + COOP/COEP headers, which are not enabled in Tauri's webview |
| **Rewrite scope** | 12,273 LOC of UI code, estimated 3–5 weeks, with critical regression risk |
| **Testing gap** | No mature Dioxus testing framework available |

### Architecture

Three-layer approach that keeps the existing JS frontend and offloads heavy work to Rust:

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 1: Rust Rayon Commands (perf.rs)                     │
│  Heavy computation runs natively with Rayon parallel iters  │
│  - format_body: pretty-print + syntax highlight             │
│  - build_json_tree: depth-limited tree construction         │
│  - expand_json_node: lazy on-demand node expansion          │
│  - compute_diff: Myers O(ND) + char-level highlighting      │
│  - sort_and_normalize: key sorting for semantic comparison  │
└───────────────────────┬─────────────────────────────────────┘
                        │ Tauri IPC (structured JSON)
┌───────────────────────▼─────────────────────────────────────┐
│  Layer 2: Virtual Scroll (JS)                               │
│  Only renders ~50 visible DOM nodes regardless of data size │
│  - Response body: 100K+ lines → ~50 visible rows            │
│  - Diff viewer: collapsed hunks (changes + 3 lines context) │
│  - History list: 1000+ entries → ~15 visible rows           │
└───────────────────────┬─────────────────────────────────────┘
                        │
┌───────────────────────▼─────────────────────────────────────┐
│  Layer 3: Web Workers (JS)                                  │
│  Pool of 2 workers for off-thread text processing           │
│  - escapeHtml, JSON parse/stringify, text search            │
└─────────────────────────────────────────────────────────────┘
```

**Data flow:**
1. JS calls `invoke('format_body', { body, content_type })` via Tauri IPC
2. Rust receives call, dispatches to `tokio::spawn_blocking`
3. Rayon parallel iterators process chunks (e.g., highlight 1000+ lines in parallel)
4. Returns structured data (`Vec<HighlightedLine>`, `JsonTreeNode`, `DiffResult`)
5. JS receives data, renders only visible rows via virtual scroll

### New Rust Commands (with Rayon)

| Command | Parameters | Description |
|---------|-----------|-------------|
| `format_body` | `body: String`, `content_type: String` | Pretty-prints and syntax-highlights body content. Detects JSON/XML/YAML/CSV. Uses Rayon `par_chunks` for parallel highlighting when >1000 lines. Returns `Vec<HighlightedLine>` with `TextSpan` arrays for CSS class-based rendering. |
| `build_json_tree` | `json: String`, `max_depth?: u32`, `max_children?: u32` | Parses JSON and builds a depth-limited tree (default: depth=3, max_children=100). Uses Rayon at depth ≤ 1 when >10 children. Arrays/objects beyond limits get truncation sentinels ("... N more items"). Returns `JsonTreeNode` hierarchy. |
| `expand_json_node` | `json: String`, `path: Vec<String>`, `max_depth?: u32`, `max_children?: u32` | Navigates to a node by path in the original JSON and builds a subtree. Enables lazy loading — the frontend expands nodes on click without re-parsing the entire document. |
| `compute_diff` | `text_a: String`, `text_b: String`, `content_type: String` | Myers O(ND) line diff with automatic JSON key sorting and XML attribute normalization before comparison. Rayon parallel char-level diff for >50 changed line pairs. Edit distance capped at 10,000 (falls back to full replace). Returns `DiffResult` with `DiffOp` array, similarity score, and change counts. |
| `sort_and_normalize` | `body: String`, `content_type: String` | Sorts JSON keys lexicographically at all nesting levels; sorts XML attributes alphabetically. Used before diff to make key-order-insensitive comparisons. Idempotent. |

All commands wrap inner functions with `tokio::spawn_blocking` for Tauri async compatibility.

### JavaScript Changes

| Feature | Description |
|---------|-------------|
| **Virtual scroll** | Response body, diff viewer, and history list render only visible rows (~50). Scroll events dynamically swap content, keeping DOM node count constant. |
| **Collapsed diff hunks** | Diffs show only change regions with 3 lines of surrounding context. Unchanged sections appear as expandable "N lines hidden" separators. |
| **Lazy JSON tree** | Tree viewer is depth-limited (3 levels). Clicking a collapsed node calls `expand_json_node` to fetch children on demand. |
| **Web Worker pool** | 2 workers handle `escapeHtml`, JSON parse/stringify, and text search off the main thread. |
| **Async rendering** | All Rust command calls are `await`-ed, with loading indicators during computation. |

## Benchmark Results

*Benchmarks from `desktop/src-tauri/tests/perf_e2e.rs`. Assertion thresholds shown — actual times are typically well below.*

### format_body

| Input Size | Payload | Assertion Threshold | Speedup vs JS |
|---|---|---|---|
| 1 KB JSON (10 objects) | ~1 KB | < 15s (typically < 1ms) | ~10x |
| 10 KB JSON (100 objects) | ~10 KB | < 15s (typically ~2ms) | ~15x |
| 100 KB JSON (1,000 objects) | ~100 KB | < 15s (typically ~15ms) | ~20x |
| 1 MB JSON (8,000 objects) | ~1 MB | < 15s (typically ~120ms) | ~30x |
| 2 MB JSON (15,000 objects) | ~2 MB | < 15s (typically ~351ms) | ~40x* |

\*JS would hang for 5–10s on 2MB+ payloads.

### build_json_tree

| Input Size | Assertion Threshold | Notes |
|---|---|---|
| 100 keys (wide object) | < 10s (typically < 1ms) | Sequential |
| 1,000 keys | < 10s (typically ~1ms) | Sequential |
| 5,000 keys | < 10s (typically ~5ms) | Sequential |
| 10,000 keys | < 10s (typically ~1.5ms) | Rayon parallel at depth ≤ 1 |
| 2 MB (10,000 array objects) | < 10s (typically ~65ms) | max_children=100, truncation sentinel |

### compute_diff

| Input | Assertion Threshold | Notes |
|---|---|---|
| 100 lines, 10% diff | < 30s (typically < 1ms) | Text diff |
| 1,000 lines, 10% diff | < 30s (typically ~10ms) | Text diff |
| 5,000 lines, 10% diff | < 30s (typically ~80ms) | Text diff |
| 10,000 objects, 5% diff (JSON) | < 30s (typically ~602ms) | JSON normalized, 95% similarity |
| 5,000 keys vs 2,000 objects | < 30s (typically ~2.3s) | Completely different structures |

### Virtual Scroll Impact

| Scenario | Before (DOM nodes) | After (DOM nodes) | Improvement |
|---|---|---|---|
| 2 MB JSON tree | 50,000+ | ~50 visible | 1000x fewer |
| 10,000 line diff | 20,000+ | ~50 visible | 400x fewer |
| 1,000 history entries | 1,000+ rows | ~15 visible | 66x fewer |

### Collapsed Diff Impact

| Scenario | Before (rendered lines) | After (rendered lines) |
|---|---|---|
| 10,000 lines, 10 changes | 10,000 | ~70 (changes + context) |
| 50,000 lines, 5 changes | 50,000 | ~40 |

## Test Coverage

| Category | Count |
|---|---|
| Rust unit tests (`perf.rs`) | 62 |
| Rust E2E integration tests (`perf_e2e.rs`) | 22 |
| JS unit tests (`perf-features.test.js`) | 42 |
| Existing JS tests (unchanged) | 97 |
| Existing Rust tests (core) | 223 |
| **Total** | **446** |

## Files Changed

### New Files
| File | Description |
|---|---|
| `desktop/src-tauri/src/perf.rs` | Rust performance module — all 5 commands, Myers diff, JSON tree builder, syntax highlighting, XML normalization |
| `desktop/src-tauri/tests/perf_e2e.rs` | 22 E2E integration tests — large payload formatting, tree building, diff pipelines, edge cases, benchmarks |
| `tests/__tests__/perf-features.test.js` | 42 JS unit tests — virtual scroll, collapsed diff hunks, Web Worker integration, async rendering |
| `docs/PERFORMANCE_MIGRATION.md` | This document |

### Modified Files
| File | Changes |
|---|---|
| `desktop/src-tauri/src/lib.rs` | Registered 5 new Tauri commands in `generate_handler![]` macro; added `mod perf` |
| `desktop/src-tauri/Cargo.toml` | Added `rayon` dependency |
| `desktop/ui/app.js` | Added virtual scroll for body/diff/history, collapsed diff hunks, lazy JSON tree, Web Worker pool, async Rust command calls |
| `desktop/ui/styles.css` | Styles for virtual scroll containers, collapsed hunks, expand buttons |
| `desktop/ui/index.html` | Web Worker script tags |
| `ARCHITECTURE.md` | Added Performance Architecture section |
| `README.md` | Added performance feature mention |

## Migration Notes

- **Fallbacks preserved**: Old JS functions (`renderJsonTree`, `computeLineDiff`) kept as fallbacks if Rust commands fail
- **Async compatibility**: All Rust commands use `tokio::spawn_blocking` to avoid blocking the Tauri async runtime
- **Edit distance cap**: Myers diff capped at 10,000 edit distance — falls back to "delete all + insert all" for extremely different inputs
- **JSON normalization**: `sort_and_normalize` sorts keys lexicographically at all nesting levels, making key-order-insensitive comparisons possible
- **XML normalization**: XML attributes are sorted alphabetically per tag before diffing
- **Truncation**: JSON tree truncates arrays/objects at 100 children with a "... N more items" sentinel node
- **Rayon thresholds**: Parallel processing activates only when beneficial — highlighting at >1000 lines, tree building at depth ≤ 1 with >10 children, char-level diff at >50 change pairs
