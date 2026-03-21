# RequestPilot — Browser Extension

A Manifest V3 browser extension for Chrome and Edge that lets developers inject headers, block endpoints, redirect requests, and monitor network traffic — all from the toolbar.

## Features

### Rules Engine
- **Header injection** — add or override request/response headers on matching URLs
- **Request blocking** — block requests by URL pattern
- **URL redirect** — rewrite request URLs on the fly
- **Method filtering** — scope rules to specific HTTP methods (GET, POST, etc.)
- **Import / Export** — share rule sets as JSON files
- **Toggle & Duplicate** — enable/disable individual rules; duplicate for quick variations

### Network Monitoring
- **Rule-filtered log** — view only requests that match your active rules
- **Method & status filters** — narrow the log by HTTP method or status code
- **Header picker** — choose which request/response headers to display in the log
- **Group by** — group logged requests by domain, path, method, or status
- **Similarity scoring** — score how similar two requests are (headers + body)
- **Match key picker** — select which headers/body fields factor into similarity
- **Min match filter** — hide request pairs below a similarity threshold
- **Pause / Resume** — temporarily stop logging without clearing the log
- **Request comparison** — side-by-side diff of two selected requests
- **Request detail view** — expand any entry to inspect full headers and body
- **Viewed tracking** — dim already-inspected entries
- **Response body capture** — intercepts `fetch` and `XMLHttpRequest` responses via content scripts
- **Log export / import** — save and reload logs as JSON or HAR

### Statistics Tab
- **Summary cards** — total requests, unique domains, average response time
- **Response time percentiles** — p50, p90, p95, p99 latency breakdown
- **HTTP methods breakdown** — distribution of GET, POST, PUT, DELETE, etc.
- **Domain & path tree** — expandable tree view of all captured URLs
- **Rule impact badges** — see which rules matched which paths
- **One-click add rule** — create a new rule directly from a URL in the tree
- **Expand / Collapse all** — quickly open or close every tree node

### UI
- **Dark & light themes** — toggle between themes via the header button
- **Pop-out to tab** — open the popup in a full browser tab for more room
- **Responsive layout** — works in both the compact popup and a full-width tab

## Installation

1. Clone or download this repository.
2. Open **chrome://extensions** (Chrome) or **edge://extensions** (Edge).
3. Enable **Developer mode**.
4. Click **Load unpacked** and select the `extension/` folder.
5. Pin the RequestPilot icon in the toolbar for quick access.

> No build step is required — all source files are vanilla JS loaded directly.

## Usage

### Quick Start

1. Click the RequestPilot toolbar icon to open the popup.
2. Press **+ Add Rule** to create your first rule:
   - Choose a type: **Headers**, **Block**, or **Redirect**.
   - Enter a URL pattern (supports `*` wildcards, e.g. `*api.example.com/v1/*`).
   - Optionally restrict to a specific HTTP method.
   - For header rules, add one or more header name/value pairs.
3. Toggle rules on or off with the switch next to each rule.
4. Switch to the **Log** tab to see matching requests appear in real time.

### Comparing Requests

1. In the Log tab, select two requests by clicking their checkboxes.
2. Click **Compare** to open a side-by-side diff view.
3. Use the **Match Key Picker** to choose which fields (headers, body keys) to compare.
4. The similarity score updates in real time as you adjust the match keys and minimum threshold.

### HAR Analysis

- **Export**: Click the export button in the Log tab and choose the HAR format to save a standard HAR file compatible with browser DevTools and other analysis tools.
- **Import**: Click the import button and select a `.har` or `.json` file to load previously captured traffic back into the log.

### URL Patterns

Rules use Chrome's `declarativeNetRequest` URL filter syntax:

| Pattern | Matches |
|---|---|
| `*example.com*` | Any URL containing `example.com` |
| `*api.example.com/v1/*` | Paths under `/v1/` on `api.example.com` |
| `*example.com/users*` | Any path starting with `/users` |

## Tech Stack

- **Manifest V3** — modern Chrome/Edge extension platform
- **Vanilla JavaScript** — no frameworks, no build step
- **chrome.declarativeNetRequest** — performant, rule-based request modification
- **chrome.webRequest** — request/response metadata capture for the log
- **chrome.storage.local** — persistent storage for rules and settings
- **Content Scripts** (MAIN + ISOLATED worlds) — response body interception via `fetch`/`XHR` patching
