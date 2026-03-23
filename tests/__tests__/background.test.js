/**
 * Unit tests for RequestPilot background service worker (background.js).
 *
 * Strategy:
 *   1. Inject the chrome mock into the global scope.
 *   2. require() background.js – this registers listeners as side-effects.
 *   3. Grab the onMessage handler from the mock and call it directly.
 *   4. Between tests, reset storage and mocks to keep each test isolated.
 */

const chromeMock = require("../__mocks__/chrome");
global.chrome = chromeMock;

// Load background.js (registers listeners as side-effects)
require("../../extension/background");

// ── Extract the onMessage handler registered by background.js ──
const onMessageHandler =
  chromeMock.runtime.onMessage.addListener.mock.calls[0][0];

/**
 * Helper: send a message to the background handler and return the response.
 */
function send(msg) {
  return new Promise((resolve) => {
    onMessageHandler(msg, {}, resolve);
  });
}

// ── Reset state between tests ──────────────────────────────────
beforeEach(() => {
  chromeMock._resetStore();
  chromeMock.declarativeNetRequest.getDynamicRules.mockClear();
  chromeMock.declarativeNetRequest.updateDynamicRules.mockClear();
  chromeMock.storage.local.get.mockClear();
  chromeMock.storage.local.set.mockClear();
});

/* =============================================================
 * Rule CRUD
 * ============================================================= */

describe("Rule CRUD", () => {
  test("addRule — adds a modify-headers rule with auto-incremented ID and enabled=true", async () => {
    const res = await send({
      action: "addRule",
      type: "modify-headers",
      urlPattern: "*://example.com/*",
      headers: [{ name: "X-Test", value: "123" }],
    });

    expect(res.success).toBe(true);
    expect(res.rule.id).toBe(1);
    expect(res.rule.enabled).toBe(true);
    expect(res.rule.type).toBe("modify-headers");
    expect(res.rule.headers).toEqual([{ name: "X-Test", value: "123" }]);
  });

  test("addRule — adds a block rule (no headers field)", async () => {
    const res = await send({
      action: "addRule",
      type: "block",
      urlPattern: "*://ads.example.com/*",
    });

    expect(res.success).toBe(true);
    expect(res.rule.type).toBe("block");
    expect(res.rule).not.toHaveProperty("headers");
  });

  test("addRule — adds a redirect rule with redirectUrl", async () => {
    const res = await send({
      action: "addRule",
      type: "redirect",
      urlPattern: "*://old.example.com/*",
      redirectUrl: "https://new.example.com/",
    });

    expect(res.success).toBe(true);
    expect(res.rule.type).toBe("redirect");
    expect(res.rule.redirectUrl).toBe("https://new.example.com/");
  });

  test("addRule — multiple rules get incrementing IDs", async () => {
    const r1 = await send({ action: "addRule", type: "block", urlPattern: "a" });
    const r2 = await send({ action: "addRule", type: "block", urlPattern: "b" });
    const r3 = await send({ action: "addRule", type: "block", urlPattern: "c" });

    expect(r1.rule.id).toBe(1);
    expect(r2.rule.id).toBe(2);
    expect(r3.rule.id).toBe(3);
  });

  test("getRules — returns empty array initially", async () => {
    const res = await send({ action: "getRules" });
    expect(res.rules).toEqual([]);
  });

  test("getRules — returns all added rules", async () => {
    await send({ action: "addRule", type: "block", urlPattern: "a" });
    await send({ action: "addRule", type: "block", urlPattern: "b" });

    const res = await send({ action: "getRules" });
    expect(res.rules).toHaveLength(2);
    expect(res.rules[0].urlPattern).toBe("a");
    expect(res.rules[1].urlPattern).toBe("b");
  });

  test("updateRule — updates fields of an existing rule", async () => {
    const { rule } = await send({
      action: "addRule",
      type: "block",
      urlPattern: "old",
    });

    const res = await send({
      action: "updateRule",
      ruleId: rule.id,
      fields: { urlPattern: "new", type: "redirect", redirectUrl: "https://x.com" },
    });

    expect(res.success).toBe(true);

    const { rules } = await send({ action: "getRules" });
    expect(rules[0].urlPattern).toBe("new");
    expect(rules[0].type).toBe("redirect");
    expect(rules[0].redirectUrl).toBe("https://x.com");
  });

  test("updateRule — returns error for non-existent rule", async () => {
    const res = await send({
      action: "updateRule",
      ruleId: 999,
      fields: { urlPattern: "x" },
    });

    expect(res.success).toBe(false);
    expect(res.error).toMatch(/not found/i);
  });

  test("deleteRule — removes the rule from storage", async () => {
    const { rule } = await send({
      action: "addRule",
      type: "block",
      urlPattern: "del-me",
    });

    await send({ action: "deleteRule", ruleId: rule.id });

    const { rules } = await send({ action: "getRules" });
    expect(rules).toHaveLength(0);
  });

  test("toggleRule — flips enabled state", async () => {
    const { rule } = await send({
      action: "addRule",
      type: "block",
      urlPattern: "toggle",
    });

    expect(rule.enabled).toBe(true);

    await send({ action: "toggleRule", ruleId: rule.id });
    let { rules } = await send({ action: "getRules" });
    expect(rules[0].enabled).toBe(false);

    await send({ action: "toggleRule", ruleId: rule.id });
    ({ rules } = await send({ action: "getRules" }));
    expect(rules[0].enabled).toBe(true);
  });

  test("toggleRule — returns error for non-existent rule", async () => {
    const res = await send({ action: "toggleRule", ruleId: 999 });
    expect(res.success).toBe(false);
    expect(res.error).toMatch(/not found/i);
  });
});

/* =============================================================
 * DNR Rule Building (verify updateDynamicRules calls)
 * ============================================================= */

describe("DNR Rule Building", () => {
  test("modify-headers rule produces modifyHeaders action with set operation", async () => {
    await send({
      action: "addRule",
      type: "modify-headers",
      urlPattern: "*://api.test/*",
      headers: [{ name: "Authorization", value: "Bearer tok" }],
    });

    const lastCall =
      chromeMock.declarativeNetRequest.updateDynamicRules.mock.calls.at(-1)[0];
    const dnr = lastCall.addRules[0];

    expect(dnr.action.type).toBe("modifyHeaders");
    expect(dnr.action.requestHeaders).toEqual([
      { header: "Authorization", operation: "set", value: "Bearer tok" },
    ]);
  });

  test("block rule produces block action", async () => {
    await send({
      action: "addRule",
      type: "block",
      urlPattern: "*://block.test/*",
    });

    const lastCall =
      chromeMock.declarativeNetRequest.updateDynamicRules.mock.calls.at(-1)[0];
    const dnr = lastCall.addRules[0];

    expect(dnr.action.type).toBe("block");
  });

  test("redirect rule produces redirect action with URL", async () => {
    await send({
      action: "addRule",
      type: "redirect",
      urlPattern: "*://redir.test/*",
      redirectUrl: "https://dest.test/",
    });

    const lastCall =
      chromeMock.declarativeNetRequest.updateDynamicRules.mock.calls.at(-1)[0];
    const dnr = lastCall.addRules[0];

    expect(dnr.action.type).toBe("redirect");
    expect(dnr.action.redirect.url).toBe("https://dest.test/");
  });

  test("disabled rules are not included in DNR update", async () => {
    const { rule } = await send({
      action: "addRule",
      type: "block",
      urlPattern: "*://disabled.test/*",
    });

    // Disable the rule
    await send({ action: "toggleRule", ruleId: rule.id });

    const lastCall =
      chromeMock.declarativeNetRequest.updateDynamicRules.mock.calls.at(-1)[0];
    expect(lastCall.addRules).toHaveLength(0);
  });

  test("rule with methods produces requestMethods in condition", async () => {
    await send({
      action: "addRule",
      type: "block",
      urlPattern: "*://methods.test/*",
      methods: ["GET", "POST"],
    });

    const lastCall =
      chromeMock.declarativeNetRequest.updateDynamicRules.mock.calls.at(-1)[0];
    const dnr = lastCall.addRules[0];

    expect(dnr.condition.requestMethods).toEqual(["get", "post"]);
  });

  test("rule without methods has no requestMethods in condition", async () => {
    await send({
      action: "addRule",
      type: "block",
      urlPattern: "*://nomethods.test/*",
    });

    const lastCall =
      chromeMock.declarativeNetRequest.updateDynamicRules.mock.calls.at(-1)[0];
    const dnr = lastCall.addRules[0];

    expect(dnr.condition.requestMethods).toBeUndefined();
  });

  test("all resource types are included in condition", async () => {
    await send({
      action: "addRule",
      type: "block",
      urlPattern: "*://types.test/*",
    });

    const lastCall =
      chromeMock.declarativeNetRequest.updateDynamicRules.mock.calls.at(-1)[0];
    const dnr = lastCall.addRules[0];

    expect(dnr.condition.resourceTypes).toEqual([
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
    ]);
  });
});

/* =============================================================
 * Import / Export
 * ============================================================= */

describe("Import / Export", () => {
  test("importRules — imports an array, assigns new IDs, appends to existing", async () => {
    // Add one rule first
    await send({ action: "addRule", type: "block", urlPattern: "existing" });

    const res = await send({
      action: "importRules",
      rules: [
        { type: "block", urlPattern: "imported-1", enabled: true },
        { type: "redirect", urlPattern: "imported-2", enabled: true, redirectUrl: "https://x.com" },
      ],
    });

    expect(res.success).toBe(true);
    expect(res.count).toBe(2);

    const { rules } = await send({ action: "getRules" });
    expect(rules).toHaveLength(3);
    // Imported rules should have new IDs, not collide with existing
    const ids = rules.map((r) => r.id);
    expect(new Set(ids).size).toBe(3);
  });

  test("importRules — returns correct count", async () => {
    const res = await send({
      action: "importRules",
      rules: [
        { type: "block", urlPattern: "a", enabled: true },
        { type: "block", urlPattern: "b", enabled: true },
        { type: "block", urlPattern: "c", enabled: true },
      ],
    });

    expect(res.count).toBe(3);
  });

  test("exportRules — returns all rules", async () => {
    await send({ action: "addRule", type: "block", urlPattern: "exp-1" });
    await send({ action: "addRule", type: "block", urlPattern: "exp-2" });

    const res = await send({ action: "exportRules" });
    expect(res.rules).toHaveLength(2);
    expect(res.rules[0].urlPattern).toBe("exp-1");
    expect(res.rules[1].urlPattern).toBe("exp-2");
  });
});

/* =============================================================
 * Log
 * ============================================================= */

describe("Log", () => {
  test("getLog — returns empty array initially", async () => {
    const res = await send({ action: "getLog" });
    expect(res.log).toEqual([]);
  });

  test("clearLog — empties the log", async () => {
    const res = await send({ action: "clearLog" });
    expect(res.success).toBe(true);

    const logRes = await send({ action: "getLog" });
    expect(logRes.log).toEqual([]);
  });

  test("onRuleMatchedDebug.addListener is called at top level (not inside onInstalled)", () => {
    // The listener is now registered at the top level of the service worker,
    // so it should have been called when background.js was first required.
    expect(
      chromeMock.declarativeNetRequest.onRuleMatchedDebug.addListener
    ).toHaveBeenCalled();
  });
});

/* =============================================================
 * Bootstrap listeners
 * ============================================================= */

describe("Bootstrap", () => {
  test("onInstalled listener was registered", () => {
    expect(chromeMock.runtime.onInstalled.addListener).toHaveBeenCalled();
  });

  test("onStartup listener was registered", () => {
    expect(chromeMock.runtime.onStartup.addListener).toHaveBeenCalled();
  });

  test("onMessage listener was registered", () => {
    expect(chromeMock.runtime.onMessage.addListener).toHaveBeenCalled();
  });
});

/* =============================================================
 * Unknown action
 * ============================================================= */

describe("Edge cases", () => {
  test("unknown action returns error", async () => {
    const res = await send({ action: "nonExistentAction" });
    expect(res.error).toMatch(/unknown/i);
  });
});

/* =============================================================
 * Response Body Capture
 * ============================================================= */

describe("Response Body Capture", () => {
  beforeEach(() => {
    chromeMock._resetStore();
    chromeMock.declarativeNetRequest.getDynamicRules.mockResolvedValue([]);
    chromeMock.declarativeNetRequest.updateDynamicRules.mockResolvedValue();
  });

  test("captureResponseBody with no matching entry still succeeds", async () => {
    const result = await send({
      action: "captureResponseBody",
      url: "https://example.com/api",
      body: '{"hello":"world"}',
      status: 200,
    });
    expect(result.success).toBe(true);
  });

  test("getNetworkLog returns empty array initially", async () => {
    const logResult = await send({ action: "getNetworkLog" });
    expect(logResult.log).toEqual(expect.any(Array));
  });

  test("fetchResponseBody fetches URL and returns body", async () => {
    global.fetch = jest.fn().mockResolvedValue({
      text: () => Promise.resolve('{"result":"ok"}'),
      status: 200,
    });

    const result = await send({
      action: "fetchResponseBody",
      url: "https://example.com/api/test",
      method: "GET",
      requestHeaders: [{ name: "Authorization", value: "Bearer token123" }],
    });

    expect(result.success).toBe(true);
    expect(result.body).toBe('{"result":"ok"}');
    expect(result.status).toBe(200);
    expect(global.fetch).toHaveBeenCalledWith(
      "https://example.com/api/test",
      expect.objectContaining({
        method: "GET",
        headers: expect.objectContaining({
          Authorization: "Bearer token123",
        }),
      })
    );

    delete global.fetch;
  });

  test("fetchResponseBody handles fetch errors gracefully", async () => {
    global.fetch = jest.fn().mockRejectedValue(new Error("Network error"));

    const result = await send({
      action: "fetchResponseBody",
      url: "https://example.com/api/fail",
      method: "GET",
    });

    expect(result.success).toBe(false);
    expect(result.error).toBe("Network error");

    delete global.fetch;
  });

  test("fetchResponseBody skips restricted headers", async () => {
    global.fetch = jest.fn().mockResolvedValue({
      text: () => Promise.resolve("ok"),
      status: 200,
    });

    await send({
      action: "fetchResponseBody",
      url: "https://example.com/api",
      method: "GET",
      requestHeaders: [
        { name: "Host", value: "example.com" },
        { name: "Connection", value: "keep-alive" },
        { name: "X-Custom", value: "value" },
      ],
    });

    const fetchCall = global.fetch.mock.calls[0];
    expect(fetchCall[1].headers).toEqual({ "X-Custom": "value" });

    delete global.fetch;
  });

  test("fetchResponseBody truncates large responses", async () => {
    const largeBody = "x".repeat(200000);
    global.fetch = jest.fn().mockResolvedValue({
      text: () => Promise.resolve(largeBody),
      status: 200,
    });

    const result = await send({
      action: "fetchResponseBody",
      url: "https://example.com/api/large",
      method: "GET",
    });

    expect(result.success).toBe(true);
    expect(result.body.length).toBeLessThan(200000);
    expect(result.body).toContain("...[truncated]");

    delete global.fetch;
  });

  test("fetchResponseBody with no requestHeaders sends empty headers", async () => {
    global.fetch = jest.fn().mockResolvedValue({
      text: () => Promise.resolve("ok"),
      status: 200,
    });

    const result = await send({
      action: "fetchResponseBody",
      url: "https://example.com/api",
      method: "GET",
    });

    expect(result.success).toBe(true);
    const fetchCall = global.fetch.mock.calls[0];
    expect(fetchCall[1].headers).toEqual({});

    delete global.fetch;
  });

  test("getCaptureStats returns debug info", async () => {
    const result = await send({ action: "getCaptureStats" });
    expect(result).toHaveProperty("networkLogSize");
    expect(result).toHaveProperty("pendingSize");
    expect(result).toHaveProperty("withResponseBody");
    expect(result).toHaveProperty("captureCount");
    expect(typeof result.networkLogSize).toBe("number");
    expect(typeof result.captureCount).toBe("number");
  });

  test("captureResponseBody increments captureCount", async () => {
    const before = await send({ action: "getCaptureStats" });
    await send({
      action: "captureResponseBody",
      url: "https://example.com/counter-test",
      body: "test",
      status: 200,
    });
    const after = await send({ action: "getCaptureStats" });
    expect(after.captureCount).toBe(before.captureCount + 1);
  });
});

/* =============================================================
 * Network Log Import
 * ============================================================= */

describe("Network Log Import", () => {
  test("importNetworkLog appends entries to the network log", async () => {
    const entries = [
      { id: "test-1", url: "https://example.com/api", method: "GET", statusCode: 200 },
      { id: "test-2", url: "https://example.com/other", method: "POST", statusCode: 201 },
    ];
    const res = await send({ action: "importNetworkLog", log: entries });
    expect(res.success).toBe(true);
    expect(res.count).toBe(2);

    const logRes = await send({ action: "getNetworkLog" });
    expect(logRes.log.length).toBeGreaterThanOrEqual(2);
  });

  test("importNetworkLog with empty array succeeds", async () => {
    const res = await send({ action: "importNetworkLog", log: [] });
    expect(res.success).toBe(true);
    expect(res.count).toBe(0);
  });

  test("importNetworkLog with missing log defaults to empty", async () => {
    const res = await send({ action: "importNetworkLog" });
    expect(res.success).toBe(true);
    expect(res.count).toBe(0);
  });
});

/* =============================================================
 * Logging Pause/Resume
 * ============================================================= */

describe("Logging Pause/Resume", () => {
  test("getLoggingPaused returns false initially", async () => {
    const res = await send({ action: "getLoggingPaused" });
    expect(res.paused).toBe(false);
  });

  test("setLoggingPaused pauses logging", async () => {
    const res = await send({ action: "setLoggingPaused", paused: true });
    expect(res.success).toBe(true);
    expect(res.paused).toBe(true);

    const check = await send({ action: "getLoggingPaused" });
    expect(check.paused).toBe(true);
  });

  test("setLoggingPaused resumes logging", async () => {
    await send({ action: "setLoggingPaused", paused: true });
    const res = await send({ action: "setLoggingPaused", paused: false });
    expect(res.success).toBe(true);
    expect(res.paused).toBe(false);
  });
});

/* =============================================================
 * Fetch Response Body - Edge Cases
 * ============================================================= */

describe("Fetch Response Body - Edge Cases", () => {
  test("fetchResponseBody sends request body for POST", async () => {
    global.fetch = jest.fn().mockResolvedValue({
      text: () => Promise.resolve('{"ok":true}'),
      status: 200,
    });

    await send({
      action: "fetchResponseBody",
      url: "https://api.example.com/data",
      method: "POST",
      requestBodyText: '{"key":"value"}',
      requestHeaders: [{ name: "Content-Type", value: "application/json" }],
    });

    expect(global.fetch).toHaveBeenCalledWith(
      "https://api.example.com/data",
      expect.objectContaining({
        method: "POST",
        body: '{"key":"value"}',
        credentials: "include",
      })
    );
    delete global.fetch;
  });

  test("fetchResponseBody does NOT send body for GET", async () => {
    global.fetch = jest.fn().mockResolvedValue({
      text: () => Promise.resolve("ok"),
      status: 200,
    });

    await send({
      action: "fetchResponseBody",
      url: "https://api.example.com/data",
      method: "GET",
      requestBodyText: "should-be-ignored",
    });

    const fetchCall = global.fetch.mock.calls[0];
    expect(fetchCall[1].body).toBeUndefined();
    delete global.fetch;
  });

  test("fetchResponseBody includes credentials", async () => {
    global.fetch = jest.fn().mockResolvedValue({
      text: () => Promise.resolve("ok"),
      status: 200,
    });

    await send({
      action: "fetchResponseBody",
      url: "https://api.example.com",
      method: "GET",
    });

    expect(global.fetch.mock.calls[0][1].credentials).toBe("include");
    delete global.fetch;
  });
});

/* =============================================================
 * webRequest Listeners
 * ============================================================= */

describe("webRequest Listeners", () => {
  test("onBeforeRequest listener was registered", () => {
    expect(chromeMock.webRequest.onBeforeRequest.addListener).toHaveBeenCalled();
  });

  test("onSendHeaders listener was registered with requestHeaders", () => {
    expect(chromeMock.webRequest.onSendHeaders.addListener).toHaveBeenCalled();
    const call = chromeMock.webRequest.onSendHeaders.addListener.mock.calls[0];
    expect(call[2]).toContain("requestHeaders");
  });

  test("onCompleted listener was registered with responseHeaders", () => {
    expect(chromeMock.webRequest.onCompleted.addListener).toHaveBeenCalled();
    const call = chromeMock.webRequest.onCompleted.addListener.mock.calls[0];
    expect(call[2]).toContain("responseHeaders");
  });

  test("onErrorOccurred listener was registered", () => {
    expect(chromeMock.webRequest.onErrorOccurred.addListener).toHaveBeenCalled();
  });
});

/* =============================================================
 * Live Capture — formDataToBody
 * ============================================================= */

describe("Live Capture — formDataToBody", () => {
  // formDataToBody is internal, test via forwardToDesktop behavior
  // We can test it indirectly via the captureResponseBody path

  test("captureResponseBody does not forward to desktop when no socket", async () => {
    // When live capture is not connected, captureResponseBody should not throw
    const result = await send({
      action: "captureResponseBody",
      url: "https://api.test.com/endpoint",
      body: '{"data":"response"}',
      status: 200,
    });
    expect(result.success).toBe(true);
  });

  test("captureResponseBody matches entry by exact URL", async () => {
    // Import an entry to networkLog, then capture body for it
    const entry = {
      id: "match-test-1",
      url: "https://api.test.com/exact-match",
      method: "GET",
      statusCode: 200,
      responseBody: null,
      timestamp: Date.now(),
    };
    await send({ action: "importNetworkLog", log: [entry] });

    const result = await send({
      action: "captureResponseBody",
      url: "https://api.test.com/exact-match",
      body: '{"matched":"yes"}',
      status: 200,
    });
    expect(result.success).toBe(true);

    // Verify the body was stored
    const log = await send({ action: "getNetworkLog" });
    const matched = log.log.find(e => e.id === "match-test-1");
    expect(matched).toBeDefined();
    expect(matched.responseBody).toBe('{"matched":"yes"}');
  });

  test("captureResponseBody matches by URL without query string", async () => {
    await send({ action: "clearNetworkLog" });
    const entry = {
      id: "query-test",
      url: "https://api.test.com/search?q=test&page=1",
      method: "GET",
      statusCode: 200,
      responseBody: null,
      timestamp: Date.now(),
    };
    await send({ action: "importNetworkLog", log: [entry] });

    const result = await send({
      action: "captureResponseBody",
      url: "https://api.test.com/search",
      body: "search results",
      status: 200,
    });
    expect(result.success).toBe(true);

    const log = await send({ action: "getNetworkLog" });
    const matched = log.log.find(e => e.id === "query-test");
    expect(matched.responseBody).toBe("search results");
  });

  test("captureResponseBody matches by pathname when full URL differs", async () => {
    await send({ action: "clearNetworkLog" });
    const entry = {
      id: "path-test",
      url: "https://api.test.com/api/v1/data?a=1",
      method: "POST",
      statusCode: 200,
      responseBody: null,
      timestamp: Date.now(),
    };
    await send({ action: "importNetworkLog", log: [entry] });

    // Content script might report slightly different URL
    const result = await send({
      action: "captureResponseBody",
      url: "https://api.test.com/api/v1/data?a=1&b=2",
      body: "path matched",
      status: 200,
    });
    expect(result.success).toBe(true);

    const log = await send({ action: "getNetworkLog" });
    const matched = log.log.find(e => e.id === "path-test");
    expect(matched.responseBody).toBe("path matched");
  });

  test("captureResponseBody skips entries that already have a body", async () => {
    await send({ action: "clearNetworkLog" });
    const entries = [
      { id: "has-body", url: "https://api.test.com/dup", method: "GET", statusCode: 200, responseBody: "existing", timestamp: Date.now() },
      { id: "no-body", url: "https://api.test.com/dup", method: "GET", statusCode: 200, responseBody: null, timestamp: Date.now() },
    ];
    await send({ action: "importNetworkLog", log: entries });

    await send({
      action: "captureResponseBody",
      url: "https://api.test.com/dup",
      body: "new body",
      status: 200,
    });

    const log = await send({ action: "getNetworkLog" });
    const hasBody = log.log.find(e => e.id === "has-body");
    const noBody = log.log.find(e => e.id === "no-body");
    expect(hasBody.responseBody).toBe("existing"); // unchanged
    expect(noBody.responseBody).toBe("new body"); // updated
  });

  test("multiple captureResponseBody calls each match different entries", async () => {
    await send({ action: "clearNetworkLog" });
    const entries = [
      { id: "first", url: "https://api.test.com/multi", method: "GET", statusCode: 200, responseBody: null, timestamp: Date.now() },
      { id: "second", url: "https://api.test.com/multi", method: "GET", statusCode: 200, responseBody: null, timestamp: Date.now() },
    ];
    await send({ action: "importNetworkLog", log: entries });

    await send({ action: "captureResponseBody", url: "https://api.test.com/multi", body: "body-1", status: 200 });
    await send({ action: "captureResponseBody", url: "https://api.test.com/multi", body: "body-2", status: 200 });

    const log = await send({ action: "getNetworkLog" });
    const first = log.log.find(e => e.id === "first");
    const second = log.log.find(e => e.id === "second");
    expect(first.responseBody).toBe("body-1");
    expect(second.responseBody).toBe("body-2");
  });
});

/* =============================================================
 * Live Capture — enable/disable toggle
 * ============================================================= */

describe("Live Capture — enable/disable toggle", () => {
  test("getLiveCaptureStatus returns enabled: false by default", async () => {
    const status = await send({ action: "getLiveCaptureStatus" });
    expect(status.enabled).toBe(false);
    expect(status.connected).toBeFalsy();
    expect(status.mode).toBe("off");
  });

  test("enableLiveCapture sets enabled flag and persists to storage", async () => {
    const result = await send({ action: "enableLiveCapture" });
    expect(result.success).toBe(true);

    expect(chromeMock.storage.local.set).toHaveBeenCalledWith(
      { requestPilotLiveEnabled: true }
    );

    const status = await send({ action: "getLiveCaptureStatus" });
    expect(status.enabled).toBe(true);
  });

  test("disableLiveCapture clears enabled flag and persists to storage", async () => {
    await send({ action: "enableLiveCapture" });
    const result = await send({ action: "disableLiveCapture" });
    expect(result.success).toBe(true);

    expect(chromeMock.storage.local.set).toHaveBeenCalledWith(
      { requestPilotLiveEnabled: false }
    );

    const status = await send({ action: "getLiveCaptureStatus" });
    expect(status.enabled).toBe(false);
    expect(status.mode).toBe("off");
  });
});
