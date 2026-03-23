/* ============================================================
 * RequestPilot – Service Worker (Manifest V3)
 * ============================================================
 * Manages declarativeNetRequest rules, an in-memory request log,
 * and a message-based API consumed by the popup / options UI.
 * ========================================================== */

// ── In-memory request log (newest first, capped at 100) ─────
const matchLog = [];
const MAX_LOG_ENTRIES = 100;

// ── Pause / resume state ─────────────────────────────────────
let loggingPaused = false;

// ── Debug counter for response body captures ─────────────────
let captureDebugCount = 0;

// ── Live Capture (Desktop Connection) ────────────────────────
let liveSocket = null;
let liveMode = 'off'; // 'off' | 'all' | 'filtered'
let liveReconnectTimer = null;
const LIVE_WS_URL = 'ws://127.0.0.1:9718';
const LIVE_RECONNECT_DELAY = 3000;

// ── Full resource-type list used in every DNR condition ──────
const ALL_RESOURCE_TYPES = [
  "main_frame",
  "sub_frame",
  "stylesheet",
  "script",
  "image",
  "font",
  "object",
  "xmlhttprequest",
  "ping",
  "csp_report",
  "media",
  "websocket",
  "webtransport",
  "webbundle",
  "other",
];

// Mutex to serialize all storage mutations (prevents race conditions)
let _mutationQueue = Promise.resolve();
function withMutationLock(fn) {
  _mutationQueue = _mutationQueue.then(fn, fn);
  return _mutationQueue;
}

/* ============================================================
 * WebSocket connection to desktop app (live capture)
 * ========================================================== */

function connectToDesktop() {
  if (liveSocket && liveSocket.readyState <= WebSocket.OPEN) return;

  try {
    liveSocket = new WebSocket(LIVE_WS_URL);

    liveSocket.onopen = () => {
      console.log('[RequestPilot] Connected to desktop app');
      clearReconnectTimer();
      liveSocket.send(JSON.stringify({ action: 'connected' }));
    };

    liveSocket.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.action === 'set_mode') {
          liveMode = msg.mode || 'off';
          console.log('[RequestPilot] Live capture mode:', liveMode);
        } else if (msg.action === 'ping') {
          liveSocket.send(JSON.stringify({ action: 'pong' }));
        }
      } catch (e) {
        console.warn('[RequestPilot] WS message parse error:', e);
      }
    };

    liveSocket.onclose = () => {
      console.log('[RequestPilot] Disconnected from desktop app');
      liveSocket = null;
      liveMode = 'off';
      scheduleReconnect();
    };

    liveSocket.onerror = () => {
      // onclose will fire after onerror
    };
  } catch (e) {
    console.warn('[RequestPilot] WS connection failed:', e);
    scheduleReconnect();
  }
}

function disconnectFromDesktop() {
  clearReconnectTimer();
  liveMode = 'off';
  if (liveSocket) {
    liveSocket.close();
    liveSocket = null;
  }
}

function scheduleReconnect() {
  clearReconnectTimer();
  liveReconnectTimer = setTimeout(() => {
    connectToDesktop();
  }, LIVE_RECONNECT_DELAY);
}

function clearReconnectTimer() {
  if (liveReconnectTimer) {
    clearTimeout(liveReconnectTimer);
    liveReconnectTimer = null;
  }
}

function isLiveConnected() {
  return liveSocket && liveSocket.readyState === WebSocket.OPEN;
}

/* ============================================================
 * Live-capture forwarding helpers
 * ========================================================== */

function urlPatternToRegex(pattern) {
  const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*');
  return new RegExp(escaped, 'i');
}

/** Convert Chrome's formData object {key: [values]} to form-urlencoded string. */
function formDataToBody(data) {
  const parts = [];
  for (const [key, values] of Object.entries(data)) {
    for (const value of values) {
      parts.push(`${encodeURIComponent(key)}=${encodeURIComponent(value)}`);
    }
  }
  return parts.join('&');
}

function forwardToDesktop(entry) {
  if (!isLiveConnected() || liveMode === 'off') return;

  let bodyStr = null;
  if (entry.requestBody) {
    if (entry.requestBody.type === 'formData') {
      bodyStr = formDataToBody(entry.requestBody.data);
    } else if (entry.requestBody.type === 'raw') {
      bodyStr = entry.requestBody.data;
    }
  }

  const data = {
    url: entry.url,
    method: entry.method,
    request_headers: (entry.requestHeaders || []).map(h => ({ name: h.name, value: h.value || '' })),
    request_body: bodyStr,
    status_code: entry.statusCode || null,
    response_headers: (entry.responseHeaders || []).map(h => ({ name: h.name, value: h.value || '' })),
    response_body: entry.responseBody || null,
    duration: entry.duration || null,
    timestamp: entry.timestamp || null,
  };

  try {
    const msg = JSON.stringify({ action: 'request', data });
    liveSocket.send(msg);
  } catch (e) {
    console.warn('[RequestPilot] Failed to forward request:', e);
  }
}

/** Send a follow-up message with response body for a previously forwarded request. */
function forwardResponseBody(entry) {
  if (!isLiveConnected() || liveMode === 'off') return;
  try {
    const msg = JSON.stringify({
      action: 'response_body',
      url: entry.url,
      response_body: entry.responseBody || null,
      status_code: entry.statusCode || null,
    });
    liveSocket.send(msg);
  } catch (e) {
    console.warn('[RequestPilot] Failed to forward response body:', e);
  }
}

async function shouldForwardEntry(entry) {
  if (liveMode === 'all') return true;
  if (liveMode === 'filtered') {
    const rules = await getStoredRules();
    const activeRules = rules.filter(r => r.enabled);
    if (activeRules.length === 0) {
      console.log('[RequestPilot] Filtered: no enabled rules, skipping');
      return false;
    }
    const matched = activeRules.some(rule => {
      try {
        const regex = urlPatternToRegex(rule.urlPattern);
        return regex.test(entry.url);
      } catch {
        return false;
      }
    });
    if (!matched) {
      console.log('[RequestPilot] Filtered: no rule matched', entry.url,
        'patterns:', activeRules.map(r => r.urlPattern));
    }
    return matched;
  }
  console.log('[RequestPilot] shouldForward: unexpected mode', liveMode);
  return false;
}

function tryForwardEntry(entry) {
  if (!isLiveConnected() || liveMode === 'off') return;
  shouldForwardEntry(entry).then(should => {
    if (should) forwardToDesktop(entry);
  }).catch(err => {
    console.warn('[RequestPilot] Forward check failed:', err);
  });
}

/* ============================================================
 * Storage helpers
 * ========================================================== */

/** Return the saved rules array (defaults to []). */
async function getStoredRules() {
  const data = await chrome.storage.local.get("requestPilotRules");
  return data.requestPilotRules || [];
}

/** Persist the rules array to local storage. */
async function saveRules(rules) {
  await chrome.storage.local.set({ requestPilotRules: rules });
}

/** Get-and-increment the auto-ID counter (unsafe – caller must hold lock). */
async function _nextIdUnsafe() {
  const data = await chrome.storage.local.get("requestPilotCounter");
  const id = (data.requestPilotCounter || 0) + 1;
  await chrome.storage.local.set({ requestPilotCounter: id });
  return id;
}

/** Get-and-increment the auto-ID counter. */
async function nextId() {
  return withMutationLock(async () => {
    return _nextIdUnsafe();
  });
}

/* ============================================================
 * Sync to declarativeNetRequest
 * ============================================================
 * Removes every existing dynamic rule, then re-adds a DNR rule
 * for each *enabled* user rule.
 * ========================================================== */

async function syncAllRules() {
  const rules = await getStoredRules();

  // Remove all current dynamic rules
  const existing = await chrome.declarativeNetRequest.getDynamicRules();
  const removeIds = existing.map((r) => r.id);

  // Build DNR rules for enabled user rules only (skip forward-only rules)
  const addRules = rules
    .filter((r) => r.enabled && r.type !== 'forward')
    .map(buildDnrRule);

  await chrome.declarativeNetRequest.updateDynamicRules({
    removeRuleIds: removeIds,
    addRules,
  });
}

/**
 * Convert a user rule object into a chrome.declarativeNetRequest rule.
 */
function buildDnrRule(rule) {
  const dnr = {
    id: rule.id,
    priority: 1,
    condition: buildCondition(rule),
    action: buildAction(rule),
  };
  return dnr;
}

/** Build the DNR condition from a user rule. */
function buildCondition(rule) {
  const condition = {
    urlFilter: rule.urlPattern,
    resourceTypes: ALL_RESOURCE_TYPES,
  };

  // Only constrain methods when the user explicitly chose some
  if (Array.isArray(rule.methods) && rule.methods.length > 0) {
    condition.requestMethods = rule.methods.map((m) => m.toLowerCase());
  }

  return condition;
}

/** Build the DNR action from a user rule. */
function buildAction(rule) {
  switch (rule.type) {
    case "modify-headers":
      return {
        type: "modifyHeaders",
        requestHeaders: (rule.headers || []).map((h) => ({
          header: h.name,
          operation: "set",
          value: h.value,
        })),
      };

    case "block":
      return { type: "block" };

    case "redirect":
      return {
        type: "redirect",
        redirect: { url: rule.redirectUrl },
      };

    default:
      return { type: "block" };
  }
}

/* ============================================================
 * Message handler (popup / options → service worker)
 * ============================================================
 * Every action is async; we return true from the listener so
 * Chrome keeps the message channel open for sendResponse.
 * ========================================================== */

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  handleMessage(msg)
    .then(sendResponse)
    .catch((err) => sendResponse({ error: err.message }));
  return true; // keep the channel open for the async response
});

async function handleMessage(msg) {
  switch (msg.action) {
    // ── Read ────────────────────────────────────────────────
    case "getRules":
      return { rules: await getStoredRules() };

    case "exportRules":
      return { rules: await getStoredRules() };

    case "getLog":
      return { log: [...matchLog] };

    // ── Mutate ─────────────────────────────────────────────
    case "addRule":
      return handleAddRule(msg);

    case "updateRule":
      return handleUpdateRule(msg);

    case "deleteRule":
      return handleDeleteRule(msg);

    case "toggleRule":
      return handleToggleRule(msg);

    case "importRules":
      return handleImportRules(msg);

    // ── Log management ─────────────────────────────────────
    case "clearLog":
      matchLog.length = 0;
      return { success: true };

    case "getNetworkLog":
      return { log: [...networkLog] };

    case "clearNetworkLog":
      networkLog.length = 0;
      pendingRequests.clear();
      return { success: true };

    case "importNetworkLog": {
      const incoming = msg.log || [];
      incoming.forEach((entry) => networkLog.push(entry));
      return { success: true, count: incoming.length };
    }

    case "setLoggingPaused":
      loggingPaused = !!msg.paused;
      return { success: true, paused: loggingPaused };

    case "getLoggingPaused":
      return { paused: loggingPaused };

    case "captureResponseBody": {
      captureDebugCount++;
      const targetUrl = msg.url;

      // Strategy 1: exact URL match in networkLog
      let matched = networkLog.find(
        (e) => e.url === targetUrl && !e.responseBody
      );

      // Strategy 2: check pendingRequests (race with onCompleted)
      if (!matched) {
        for (const [, entry] of pendingRequests) {
          if (entry.url === targetUrl && !entry.responseBody) {
            matched = entry;
            break;
          }
        }
      }

      // Strategy 3: match URL without query string
      if (!matched) {
        const baseUrl = targetUrl.split("?")[0];
        matched = networkLog.find(
          (e) => e.url.split("?")[0] === baseUrl && !e.responseBody
        );
      }

      // Strategy 4: match most recent entry with same pathname
      if (!matched) {
        try {
          const targetPath = new URL(targetUrl).pathname;
          matched = networkLog.find(
            (e) => {
              try { return new URL(e.url).pathname === targetPath && !e.responseBody; }
              catch { return false; }
            }
          );
        } catch {}
      }

      // Strategy 5: most recent unmatched entry within 2 seconds
      if (!matched) {
        const now = Date.now();
        matched = networkLog.find(
          (e) => !e.responseBody && (now - e.timestamp) < 2000
        );
      }

      if (matched) {
        matched.responseBody = msg.body;
        // Forward response body to desktop if live capture is active
        if (isLiveConnected() && liveMode !== 'off') {
          forwardResponseBody(matched);
        }
      }
      return { success: true };
    }

    case "fetchResponseBody": {
      // Re-fetch the URL from the service worker to get the response body.
      // Replays the original request as faithfully as possible.
      try {
        const headers = {};
        if (msg.requestHeaders) {
          msg.requestHeaders.forEach((h) => {
            // Skip only headers that browsers forbid setting on fetch
            const skip = ["host", "connection", "content-length"];
            if (!skip.includes(h.name.toLowerCase())) {
              headers[h.name] = h.value;
            }
          });
        }

        const fetchOptions = {
          method: msg.method || "GET",
          headers,
          credentials: "include", // Forward cookies from the browser
        };

        // Include request body for non-GET/HEAD methods
        if (msg.method && !["GET", "HEAD"].includes(msg.method.toUpperCase()) && msg.requestBodyText) {
          fetchOptions.body = msg.requestBodyText;
        }

        const resp = await fetch(msg.url, fetchOptions);
        const text = await resp.text();
        const maxSize = 100000;
        const body = text.length > maxSize ? text.substring(0, maxSize) + "\n...[truncated]" : text;

        // Also store it on the original network log entry if we can find it
        if (msg.entryId) {
          const entry = networkLog.find((e) => e.id === msg.entryId);
          if (entry) entry.responseBody = body;
        }

        return { success: true, body, status: resp.status };
      } catch (err) {
        return { success: false, error: err.message };
      }
    }

    case "getCaptureStats": {
      return {
        networkLogSize: networkLog.length,
        pendingSize: pendingRequests.size,
        withResponseBody: networkLog.filter((e) => !!e.responseBody).length,
        captureCount: captureDebugCount,
      };
    }

    // ── Live capture control ─────────────────────────────────
    case "startLiveCapture":
      connectToDesktop();
      return { success: true };

    case "stopLiveCapture":
      disconnectFromDesktop();
      return { success: true };

    case "getLiveCaptureStatus":
      return {
        connected: isLiveConnected(),
        mode: liveMode,
      };

    default:
      return { error: "Unknown action" };
  }
}

/* ── Individual mutation handlers ──────────────────────────── */

async function handleAddRule(msg) {
  return withMutationLock(async () => {
    const rules = await getStoredRules();
    const id = await _nextIdUnsafe();

    const newRule = {
      id,
      type: msg.type,
      urlPattern: msg.urlPattern,
      enabled: true,
      ...(msg.type === "modify-headers" && { headers: msg.headers || [] }),
      ...(msg.type === "redirect" && { redirectUrl: msg.redirectUrl || "" }),
      methods: msg.methods || [],
    };

    rules.push(newRule);
    await saveRules(rules);
    await syncAllRules();

    return { success: true, rule: newRule };
  });
}

async function handleUpdateRule(msg) {
  return withMutationLock(async () => {
    const rules = await getStoredRules();
    const idx = rules.findIndex((r) => r.id === msg.ruleId);
    if (idx === -1) return { success: false, error: "Rule not found" };

    // Merge only the fields the caller provided
    Object.assign(rules[idx], msg.fields);
    await saveRules(rules);
    await syncAllRules();

    return { success: true };
  });
}

async function handleDeleteRule(msg) {
  return withMutationLock(async () => {
    let rules = await getStoredRules();
    rules = rules.filter((r) => r.id !== msg.ruleId);
    await saveRules(rules);
    await syncAllRules();

    return { success: true };
  });
}

async function handleToggleRule(msg) {
  return withMutationLock(async () => {
    const rules = await getStoredRules();
    const rule = rules.find((r) => r.id === msg.ruleId);
    if (!rule) return { success: false, error: "Rule not found" };

    rule.enabled = !rule.enabled;
    await saveRules(rules);
    await syncAllRules();

    return { success: true };
  });
}

async function handleImportRules(msg) {
  return withMutationLock(async () => {
    const existing = await getStoredRules();
    const incoming = msg.rules || [];

    // Assign fresh IDs to every imported rule
    for (const rule of incoming) {
      rule.id = await _nextIdUnsafe();
    }

    const merged = existing.concat(incoming);
    await saveRules(merged);
    await syncAllRules();

    return { success: true, count: incoming.length };
  });
}

/* ============================================================
 * Request log via declarativeNetRequest debug events
 * ============================================================
 * IMPORTANT: In MV3, event listeners MUST be registered at the
 * top level of the service worker script. Registering inside
 * onInstalled/onStartup callbacks breaks because the service
 * worker restarts without firing those events.
 * ========================================================== */

if (
  chrome.declarativeNetRequest &&
  chrome.declarativeNetRequest.onRuleMatchedDebug
) {
  chrome.declarativeNetRequest.onRuleMatchedDebug.addListener((info) => {
    const entry = {
      timestamp: Date.now(),
      ruleId: info.rule.ruleId,
      url: info.request.url,
      method: info.request.method,
      tabId: info.request.tabId,
    };

    matchLog.unshift(entry);

    if (matchLog.length > MAX_LOG_ENTRIES) {
      matchLog.length = MAX_LOG_ENTRIES;
    }
  });
}

/* ============================================================
 * Rich network request/response capture via webRequest API
 * ============================================================
 * IMPORTANT: In MV3, event listeners MUST be registered at the
 * top level of the service worker script.
 * ========================================================== */

// In-memory network log for request/response capture (newest first, max 200)
const networkLog = [];
const MAX_NETWORK_ENTRIES = 200;
const pendingRequests = new Map(); // requestId → partial entry

// Capture request body before headers are sent
chrome.webRequest.onBeforeRequest.addListener(
  (details) => {
    if (loggingPaused) return;
    let body = null;
    if (details.requestBody) {
      if (details.requestBody.formData) {
        body = { type: "formData", data: details.requestBody.formData };
      } else if (details.requestBody.raw) {
        try {
          const decoder = new TextDecoder("utf-8");
          const parts = details.requestBody.raw
            .filter((p) => p.bytes)
            .map((p) => decoder.decode(p.bytes));
          body = { type: "raw", data: parts.join("") };
        } catch {
          body = { type: "raw", data: "[binary data]" };
        }
      }
    }
    // Store body temporarily, will be merged in onSendHeaders
    if (body) {
      const existing = pendingRequests.get(details.requestId);
      if (existing) {
        existing.requestBody = body;
      } else {
        pendingRequests.set(details.requestId, { requestBody: body });
      }
    }
  },
  { urls: ["<all_urls>"] },
  ["requestBody"]
);

chrome.webRequest.onSendHeaders.addListener(
  (details) => {
    if (loggingPaused) return;
    const existing = pendingRequests.get(details.requestId) || {};
    pendingRequests.set(details.requestId, {
      ...existing,
      id: details.requestId,
      timestamp: details.timeStamp,
      url: details.url,
      method: details.method,
      type: details.type,
      tabId: details.tabId,
      requestHeaders: details.requestHeaders || [],
      statusCode: null,
      responseHeaders: [],
      requestBody: existing.requestBody || null,
      responseBody: null,
    });
  },
  { urls: ["<all_urls>"] },
  ["requestHeaders"]
);

chrome.webRequest.onCompleted.addListener(
  (details) => {
    if (loggingPaused) return;
    const entry = pendingRequests.get(details.requestId);
    if (entry) {
      entry.statusCode = details.statusCode;
      entry.responseHeaders = details.responseHeaders || [];
      entry.duration = Math.round(details.timeStamp - entry.timestamp);
      networkLog.unshift(entry);
      if (networkLog.length > MAX_NETWORK_ENTRIES) {
        networkLog.length = MAX_NETWORK_ENTRIES;
      }
      pendingRequests.delete(details.requestId);
      tryForwardEntry(entry);
    }
  },
  { urls: ["<all_urls>"] },
  ["responseHeaders"]
);

chrome.webRequest.onErrorOccurred.addListener(
  (details) => {
    if (loggingPaused) return;
    const entry = pendingRequests.get(details.requestId);
    if (entry) {
      entry.statusCode = 0;
      entry.error = details.error;
      entry.responseHeaders = [];
      networkLog.unshift(entry);
      if (networkLog.length > MAX_NETWORK_ENTRIES) {
        networkLog.length = MAX_NETWORK_ENTRIES;
      }
      pendingRequests.delete(details.requestId);
      tryForwardEntry(entry);
    }
  },
  { urls: ["<all_urls>"] }
);

// Clean up stale pending requests every 30 seconds
setInterval(() => {
  const cutoff = Date.now() - 30000;
  for (const [id, entry] of pendingRequests) {
    if (entry.timestamp < cutoff) pendingRequests.delete(id);
  }
}, 30000);

/* ============================================================
 * Bootstrap – run on install and on every browser startup
 * ========================================================== */

chrome.runtime.onInstalled.addListener(() => {
  syncAllRules();
});

chrome.runtime.onStartup.addListener(() => {
  syncAllRules();
});

// Attempt to connect to desktop app on startup (will silently retry if not running)
connectToDesktop();
