# Extension Architecture

## Overview

Manifest V3 browser extension. Background service worker + popup UI + content scripts.

## File Structure

| File | Role |
|---|---|
| `manifest.json` | Extension manifest — permissions, content scripts, service worker registration |
| `background.js` | Service worker: rule CRUD, DNR rule management, network logging, response body capture |
| `popup.html` | Popup markup — rules list, log table, stats tree, modals |
| `popup.css` | Popup styles — dark/light themes, responsive layout |
| `popup.js` | Popup logic — rule CRUD UI, log display, filters, comparison, statistics |
| `content-script.js` | ISOLATED world script — relays messages from MAIN world to the background service worker |
| `content-script-main.js` | MAIN world script — patches `fetch` and `XMLHttpRequest` to intercept response bodies |
| `icons/` | Extension icons (16 × 16, 48 × 48, 128 × 128) |

## Key Concepts

### Rule System

- Rules stored in `chrome.storage.local` under the key `requestPilotRules`.
- Each rule: `{ id, type (modify-headers | block | redirect), urlPattern, method, enabled, headers[], redirectUrl }`.
- Rules compiled to `declarativeNetRequest` (DNR) dynamic rules via `buildDnrRule()` for performant in-browser matching.
- DNR rules are fully rebuilt on every rule change (`syncAllRules()` removes all existing dynamic rules, then re-adds enabled ones).
- Auto-incrementing rule ID counter stored in `chrome.storage.local` (`requestPilotCounter`).

### Network Logging

- `chrome.webRequest.onBeforeRequest` + `onCompleted` listeners capture request metadata (URL, method, status, headers, timing).
- Response body captured via content script MAIN world (`content-script-main.js` patches `fetch`/`XHR`).
- Bodies are truncated at 100 KB (`MAX_BODY_SIZE`) to avoid memory pressure.
- In-memory log capped at 100 entries (`MAX_LOG_ENTRIES`), newest first.
- Logging can be paused/resumed without clearing the log.

### Message Passing

```
popup  ──chrome.runtime.sendMessage──▸  background (service worker)

content-script-main.js  ──window.postMessage──▸  content-script.js  ──chrome.runtime.sendMessage──▸  background
```

Key message actions:

| Action | Direction | Purpose |
|---|---|---|
| `getRules` | popup → background | Fetch all stored rules |
| `saveRule` | popup → background | Create or update a rule |
| `deleteRule` | popup → background | Remove a rule by ID |
| `duplicateRule` | popup → background | Clone a rule with a new ID |
| `toggleRule` | popup → background | Enable/disable a rule |
| `getLog` | popup → background | Retrieve the in-memory request log |
| `clearLog` | popup → background | Wipe the log |
| `togglePause` | popup → background | Pause or resume network logging |
| `exportRules` / `importRules` | popup → background | Bulk rule export/import |
| `captureResponseBody` | content-script → background | Deliver an intercepted response body |

### Statistics

- Computed entirely in `popup.js` from the log entries array.
- Domain → path tree built by splitting URLs and aggregating counts.
- Response-time percentiles (p50, p90, p95, p99) calculated via sorted-array indexing.
- Rule impact badges show which rules matched which paths, computed by testing each rule's URL pattern against logged URLs.

### Comparison Engine

- Similarity scoring: weighted comparison of request headers + JSON body keys/values.
- User-selectable match keys (choose which headers or body fields to compare).
- Minimum-match filter hides pairs below a threshold.
- Side-by-side diff view for any two selected requests.

## Key Patterns for Agents

- **All state lives in `chrome.storage.local`** — there is no backend server.
- **No build step** — vanilla JS loaded directly by the browser. No bundler, no transpiler.
- **Popup is torn down / recreated on open/close** — all state is reloaded from storage and the background log on every popup open.
- **DNR rule priority**: blocking > redirect > header modification.
- **Content scripts are injected into every web page** (`<all_urls>`) at `document_start` to capture responses before application code runs.
- **MAIN world script** runs in the page's JS context (can patch `fetch`/`XHR`); **ISOLATED world script** has access to `chrome.runtime` APIs. They communicate via `window.postMessage`.
- **Resource types** — every DNR rule is applied to all resource types (`ALL_RESOURCE_TYPES` constant) so rules match XHR, fetch, navigation, scripts, images, etc.
