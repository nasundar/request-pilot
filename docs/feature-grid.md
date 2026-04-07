# Request Pilot — Feature Comparison Grid

Comprehensive feature mapping across **Desktop** (Tauri v2), **TUI** (ratatui terminal), and **Extension** (Edge/Chrome browser).

> Desktop and TUI share the same Rust `core` crate and `.http` file format.
> The extension serves a different purpose (live network interception, header injection, request blocking) so feature gaps vs desktop/TUI are expected and intentional.

**Legend:**  ✅ Full  · ⚡ Partial / Read-only  · ❌ Not available  · N/A Not applicable

---

## 1. File Management

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Open .http file | ✅ Native dialog | ✅ Ctrl+O with path autocomplete | N/A |
| Multi-file tabs | ✅ Tab bar, click to switch | ✅ Sidebar tree, navigate to switch | N/A |
| New / temp file | ✅ Auto-created on builder edit | ✅ New blank template | N/A |
| Save file | ✅ Ctrl+S, Save As with dialog | ✅ Ctrl+S, Save As with path prompt | N/A |
| Close file | ✅ Click × on tab | ✅ Close with confirmation | N/A |
| Load .env file | ✅ Via toolbar | ✅ Ctrl+L with autocomplete | N/A |
| Unsaved indicator | ✅ Dot on tab | ✅ `[modified]` badge in editor | N/A |
| File header tooltip | ✅ Hover file tab → parsed comment header | ❌ | N/A |
| Builder auto-flush to file | ✅ Debounced sync on every edit | ⚡ `flush_builder_to_file()` exists, not wired | N/A |

---

## 2. Builder (Visual Request Editor)

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| HTTP method selector | ✅ Dropdown | ✅ Arrow keys cycle | N/A |
| URL editing | ✅ Input field | ✅ Inline char-by-char edit | N/A |
| Headers editing | ✅ Dynamic key/value rows, add/remove/toggle | ⚡ Read-only display | N/A |
| Body editing | ✅ Textarea with syntax highlighting overlay | ⚡ Read-only display | N/A |
| Body language detection | ✅ JSON, XML, SQL, PromQL coloring | N/A (read-only) | N/A |
| Metadata accordion | ✅ Editable name, description, group, depends, mode, disabled | ⚡ View-only in inspector | N/A |
| Assertions & Extracts sub-tab | ✅ Add/edit/remove assertions & extracts | ⚡ Read-only list display | N/A |
| Compare multi-step tabs | ✅ Separate tab per step + comparison tab | ⚡ Special render, read-only | N/A |
| Add compare step | ✅ + button adds new step | ❌ | N/A |
| Comparison panel (diff rules) | ✅ Editable diff assertions | ⚡ Read-only display | N/A |
| Variable autocomplete | ✅ Ctrl+Space / `{{` in URL, headers, body | ✅ Ctrl+Space / `{{` in URL | N/A |
| Send from builder | ✅ Send button / Ctrl+Enter | ✅ Ctrl+Enter | N/A |

---

## 3. Code Editor

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Full text editing | ✅ Textarea with transparent text + `<pre>` overlay | ✅ Custom char-by-char editor | N/A |
| Syntax highlighting | ✅ Directives, methods, headers, JSON, variables, comments | ✅ Same categories, Catppuccin palette | N/A |
| Block separators (###) | ✅ Bold yellow | ✅ Bold yellow | N/A |
| Variable `{{...}}` highlighting | ✅ Accent color | ✅ Peach/orange | N/A |
| Search (find) | ✅ Ctrl+F | ✅ `/` to search, n/N next/prev | N/A |
| Go to line | ❌ | ✅ Ctrl+G | N/A |
| Selection + copy/cut/paste | ✅ Native textarea selection | ✅ Shift+Arrow select, Ctrl+C/X/V | N/A |
| Select all | ✅ Ctrl+A | ✅ Ctrl+A | N/A |
| Line numbers | ✅ Gutter in overlay | ✅ Auto-width gutter | N/A |
| Active block highlight | ✅ Background shading | ✅ Background shading | N/A |
| View / edit mode toggle | N/A (always editable) | ✅ `i`/`e` to toggle | N/A |
| Variable autocomplete | ✅ Ctrl+Space / `{{` trigger | ✅ Ctrl+Space / `{{` trigger | N/A |

---

## 4. Execution

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Run single block | ✅ Click run on block | ✅ `r` on selected block | N/A |
| Run all blocks | ✅ Run All button | ✅ `R` key | N/A |
| Run group | ✅ From group context | ✅ Run group | N/A |
| Progress bar | ✅ Visual bar with block count | ✅ Filled/empty bar + counter | N/A |
| Progress strip (compact) | ✅ Inline per-block icons | ✅ 4-line compact strip | N/A |
| Progress expanded overlay | ❌ | ✅ `E` to expand full-screen | N/A |
| Block status icons | ✅ ✓/✗/⊘ in tree | ✅ ✓/✗/⊘/· in tree | N/A |
| Elapsed time display | ✅ Status bar | ✅ Status bar | N/A |
| Spinner animation | ❌ | ✅ Animated chars | N/A |
| Queue next run | ✅ | ✅ | N/A |
| Auto-run (timed interval) | ✅ Dropdown selector | ✅ Ctrl+A popup with interval input | N/A |
| Disabled blocks (@disabled) | ✅ Skipped during run, dimmed in tree | ✅ Skipped, ⊘ mark, dimmed | N/A |
| Dev mode (@mode app/dev) | ✅ Toggle, @mode app blocks skipped | ✅ Toggle, blocks shown as disabled | N/A |
| Azure AD auth (@dev_auth) | ✅ Device code flow, token injection | ✅ Ctrl+Shift+A popup, az CLI check | N/A |

---

## 5. Variables & Environment

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Suite variables (@variables) | ✅ Parsed from .http files | ✅ Parsed from .http files | N/A |
| Environment variables (.env) | ✅ Load .env file | ✅ Ctrl+L to load .env | N/A |
| Variable list panel | ✅ Sidebar section | ✅ `V` key, sidebar tab | N/A |
| Add variable | ✅ UI form | ✅ `a` key, two-step prompt | N/A |
| Edit variable | ✅ Inline edit | ✅ `e` key on selected | N/A |
| Delete variable | ✅ Click delete | ✅ `d` key with confirmation | N/A |
| Variable autocomplete | ✅ Ctrl+Space / `{{` everywhere | ✅ Ctrl+Space / `{{` in URL & code editor | N/A |
| Built-in variables | ✅ $timestamp, $uuid, $randomInt | ✅ Same three | N/A |
| Variable substitution at runtime | ✅ | ✅ | N/A |
| Extracted variable indicator | ✅ In results | ✅ ⇐ marker | N/A |
| Variable value tooltip | ✅ Hover shows full value + JWT decode | ✅ `i` to inspect, full value popup | N/A |

---

## 6. Results & Response Inspection

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Response body display | ✅ Formatted by content type | ✅ Raw text, auto-detect type | N/A |
| JSON tree viewer | ✅ Collapsible, expand/collapse all, depth 1-9 | ✅ Same — Enter toggle, E/C all, 1-9 depth | N/A |
| JSON syntax coloring | ✅ Keys, strings, numbers, booleans, null | ✅ Same categories | N/A |
| XML rendering | ✅ Pretty-printed with syntax highlighting | ❌ Raw text only | N/A |
| YAML rendering | ✅ Syntax highlighted | ❌ Raw text only | N/A |
| CSV rendering | ✅ Table display | ❌ Raw text only | N/A |
| Response headers | ✅ Tabbed panel | ✅ `h` key to switch to headers tab | N/A |
| HTTP status color coding | ✅ 2xx green, 3xx yellow, 4xx/5xx red | ✅ Same color coding | N/A |
| Response time display | ✅ ms with color coding | ✅ ms with color coding | N/A |
| Response size | ✅ Formatted B/KB/MB | ✅ Formatted | N/A |
| Assertion results | ✅ Pass/fail per assertion with details | ✅ Tab for assertions | N/A |
| Copy response to clipboard | ✅ Click to copy | ✅ `y` key | N/A |
| Large body handling | ✅ Lazy load full body | ✅ Preview first 8KB, load on demand | N/A |

---

## 7. Comparison & Diff

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Compare blocks (@compare) | ✅ Multi-step tabs in builder | ✅ Special render in builder | N/A |
| Diff viewer overlay | ✅ Side-by-side in modal | ✅ `D` key, side-by-side or stacked | N/A |
| Diff result caching | ✅ Hash-based cache, invalidation on change | ❌ | N/A |
| Step-by-step comparison | ✅ Tab per step + comparison tab | ⚡ Read-only display | N/A |

---

## 8. Run History

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Run history tracking | ✅ Per-file, per-block timestamped entries | ✅ Full history mode (`h` key) | N/A |
| Trend bar visualization | ✅ Mini sparkline in tooltips | ❌ | N/A |
| History grouping | ❌ | ✅ Flat / Domain / Status / Source / File+Group+Test | N/A |
| History filtering | ❌ | ✅ URL contains, method filter, status filter | N/A |
| History detail popup | ❌ | ✅ Request + Response tabs with JSON tree | N/A |
| History export | ❌ | ✅ Ctrl+E to JSON | N/A |
| Clear history | ❌ | ✅ Ctrl+X with confirmation | N/A |
| Multi-select for comparison | ❌ | ✅ Space to select, compare selected | N/A |

---

## 9. Network / Dev Tools Integration

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Live request capture | ✅ WebSocket connector to extension | ✅ Ctrl+Shift+L, same WS connector | ✅ Forward rules, WS to desktop |
| Filtered log | N/A | N/A | ✅ Full filtering: rule, method, status, resource type, similarity |
| Request stats | N/A | N/A | ✅ Percentiles, domain/path grouping, resource type breakdown |
| Stats comparison | N/A | N/A | ✅ Select 2 requests, side-by-side compare |
| Request blocking | N/A | N/A | ✅ DNR block rules with URL patterns |
| Header injection | N/A | N/A | ✅ DNR modify headers with method filters |
| Request redirection | N/A | N/A | ✅ DNR redirect rules |
| Extra headers (global) | ✅ Extra headers panel, toggle per header | ✅ Ctrl+Shift+H popup | N/A |
| Response body capture | N/A | N/A | ✅ Fetch/XHR interception, 100KB limit |
| HAR import | N/A | N/A | ✅ Import HAR files into log |
| Log export/import | N/A | N/A | ✅ JSON export/import |
| Rule export/import | N/A | N/A | ✅ JSON export/import |
| Request detail modal | N/A | N/A | ✅ Full request/response headers + body |
| Log grouping | N/A | N/A | ✅ By URL, domain, or path; collapsible groups |
| Resource type filter | N/A | N/A | ✅ Fetch/XHR, Doc, JS, CSS, Img, Media, Font, WS, Other |
| Similarity scoring | N/A | N/A | ✅ Header + payload key comparison with threshold filters |
| Add rule from log entry | N/A | N/A | ✅ Quick-create rule from captured request |
| Sniffing pause/resume | N/A | N/A | ✅ Toggle without clearing data |

---

## 10. UI / UX

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Theme: dark/light toggle | ✅ Toggle button | N/A (terminal colors) | ✅ Toggle button |
| Theme: multiple palettes | ❌ (dark/light only) | ✅ 7 palettes (Tokyo Night, Dracula, Gruvbox, Nord, One Dark, Everforest, Kanagawa) | ❌ |
| Zoom in/out | ✅ Ctrl+/Ctrl- | N/A (terminal zoom) | N/A |
| Splash screen | ❌ | ✅ Animated rocket, tagline, 4s duration | N/A |
| Help overlay | ❌ | ✅ `?` key, scrollable keybind reference | N/A |
| Keyboard shortcuts | ✅ Standard web shortcuts | ✅ Comprehensive vi-inspired keybindings | ⚡ Tab navigation |
| File tree / sidebar | ✅ Collapsible with groups, icons, status | ✅ Same tree structure with icons | N/A |
| Block tooltips | ✅ Rich hover tooltips with history, assertions, timing | ❌ | N/A |
| Group tooltips | ✅ Hover group → aggregate stats | ❌ | N/A |
| File tooltips | ✅ Hover file → parsed header, run stats, trend | ❌ | N/A |
| Status bar | ✅ Bottom bar with run results | ✅ Bottom bar with mode + notifications | ✅ Sniffing + desktop status |
| Pop-out to tab | N/A | N/A | ✅ Open popup in resizable browser tab |
| Input autocomplete prompts | N/A | ✅ File picker with Tab completion | N/A |
| Confirmation dialogs | ✅ Native OS dialogs | ✅ Inline confirmation prompts | ❌ |
| Application logs | ✅ Internal rpLog system | ✅ `l` key, level filtering, auto-scroll | N/A |
| OTEL integration | ❌ | ✅ Ctrl+Shift+O popup, toggle, stats display | N/A |

---

## 11. Rule Management (Extension-Only)

| Feature | Desktop | TUI | Extension |
|---------|---------|-----|-----------|
| Add/edit/delete rules | N/A | N/A | ✅ Modal form with validation |
| Toggle rule enable/disable | N/A | N/A | ✅ Checkbox toggle |
| Duplicate rule | N/A | N/A | ✅ Clone existing rule |
| Rule types | N/A | N/A | ✅ Modify Headers, Block, Redirect, Forward |
| URL pattern wildcards | N/A | N/A | ✅ `*` wildcard patterns |
| Method-specific filtering | N/A | N/A | ✅ Per-method checkboxes |
| Rule cards UI | N/A | N/A | ✅ Collapsible cards with badges |
| DNR compilation | N/A | N/A | ✅ Auto-compile to Chrome declarativeNetRequest |

---

## Summary: Parity Gaps (Desktop vs TUI)

These are features present in Desktop but missing or limited in TUI:

| Gap | Desktop | TUI | Priority |
|-----|---------|-----|----------|
| Builder headers editing | ✅ Full CRUD | ⚡ Read-only | Medium |
| Builder body editing | ✅ Full textarea | ⚡ Read-only | Medium |
| Builder metadata editing | ✅ Editable accordion | ⚡ View-only in inspector | Medium |
| Builder assertions/extracts editing | ✅ Add/edit/remove | ⚡ Read-only list | Medium |
| Builder auto-flush to file | ✅ Debounced on every edit | ⚡ Method exists, not wired | High |
| Compare step tabs (editable) | ✅ Full CRUD per step | ⚡ Read-only | Low |
| XML/YAML/CSV rendering | ✅ Formatted display | ❌ Raw text | Low |
| Block/file/group tooltips | ✅ Rich hover info | ❌ | Low |
| Diff result caching | ✅ Hash-based | ❌ | Low |
| Trend bar sparklines | ✅ In tooltips | ❌ | Low |

These are features present in TUI but missing in Desktop:

| Gap | TUI | Desktop | Priority |
|-----|-----|---------|----------|
| History mode (full) | ✅ Grouping, filtering, detail, export | ⚡ Run history in tooltips only | Medium |
| Go to line (Ctrl+G) | ✅ | ❌ | Low |
| 7 color themes | ✅ | ❌ (dark/light only) | Low |
| Splash screen | ✅ Animated | ❌ | Low |
| Help overlay | ✅ Scrollable keybinds | ❌ | Low |
| Progress expanded overlay | ✅ Full-screen | ❌ | Low |
| Application log viewer | ✅ Level filtering | ❌ (internal rpLog only) | Low |
| OTEL integration | ✅ | ❌ | Low |
