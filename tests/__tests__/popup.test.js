/**
 * Unit tests for RequestPilot popup utility functions (popup.js).
 *
 * popup.js is tightly coupled to the DOM (queries elements on load), so we
 * mirror the pure utility functions here and test them directly. This avoids
 * the fragility of setting up the full popup DOM just to test escaping /
 * formatting helpers.
 *
 * The implementations below are copied verbatim from popup.js so the tests
 * validate the exact logic used in production.
 */

/* ============================================================
 * Utility functions (mirrored from popup.js)
 * ============================================================ */

/** Escape HTML entities to prevent XSS in rendered content. */
function escapeHtml(str) {
  const div = document.createElement("div");
  div.textContent = str;
  return div.innerHTML;
}

/** Escape a string for safe use inside HTML attributes. */
function escapeAttr(str) {
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** Format a timestamp (ms) as HH:MM:SS. */
function formatTime(timestamp) {
  const d = new Date(timestamp);
  return d.toTimeString().split(" ")[0]; // "HH:MM:SS"
}

/* ============================================================
 * Tests
 * ============================================================ */

describe("escapeHtml", () => {
  test("escapes <, >, &, and \" characters", () => {
    const input = '<script>alert("xss")&</script>';
    const result = escapeHtml(input);

    expect(result).not.toContain("<script>");
    expect(result).not.toContain("</script>");
    expect(result).toContain("&lt;");
    expect(result).toContain("&gt;");
    expect(result).toContain("&amp;");
  });

  test("handles empty string", () => {
    expect(escapeHtml("")).toBe("");
  });

  test("handles strings with no special characters", () => {
    expect(escapeHtml("hello world")).toBe("hello world");
  });

  test("preserves normal text while escaping mixed content", () => {
    const result = escapeHtml("Hello <b>World</b> & 'friends'");
    expect(result).toContain("Hello ");
    expect(result).toContain("&lt;b&gt;");
    expect(result).toContain("&amp;");
  });
});

describe("escapeAttr", () => {
  test("escapes quotes and angle brackets", () => {
    const result = escapeAttr('value "with" <angles> & \'quotes\'');

    expect(result).toContain("&quot;");
    expect(result).toContain("&lt;");
    expect(result).toContain("&gt;");
    expect(result).toContain("&amp;");
    expect(result).toContain("&#39;");
  });

  test("handles empty string", () => {
    expect(escapeAttr("")).toBe("");
  });

  test("handles strings with no special characters", () => {
    expect(escapeAttr("plain")).toBe("plain");
  });

  test("converts non-string values to string", () => {
    expect(escapeAttr(123)).toBe("123");
    expect(escapeAttr(null)).toBe("null");
  });
});

describe("formatTime", () => {
  test("formats timestamps as HH:MM:SS", () => {
    // Use a known timestamp and check format pattern
    const result = formatTime(1700000000000);
    expect(result).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  test("handles midnight (00:00:00 UTC)", () => {
    // Create a midnight UTC timestamp
    const midnight = new Date("2024-01-15T00:00:00Z").getTime();
    const result = formatTime(midnight);
    expect(result).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // In any timezone the format should be valid HH:MM:SS
    const parts = result.split(":");
    expect(parts).toHaveLength(3);
    expect(Number(parts[0])).toBeGreaterThanOrEqual(0);
    expect(Number(parts[0])).toBeLessThan(24);
    expect(Number(parts[1])).toBeGreaterThanOrEqual(0);
    expect(Number(parts[1])).toBeLessThan(60);
    expect(Number(parts[2])).toBeGreaterThanOrEqual(0);
    expect(Number(parts[2])).toBeLessThan(60);
  });

  test("handles noon (12:00:00 UTC)", () => {
    const noon = new Date("2024-01-15T12:00:00Z").getTime();
    const result = formatTime(noon);
    expect(result).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  test("handles epoch zero", () => {
    const result = formatTime(0);
    expect(result).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });
});

/* ============================================================
 * Additional utility functions (mirrored from popup.js)
 * ============================================================ */

function urlPatternToRegex(pattern) {
  const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*');
  return new RegExp(escaped, 'i');
}

function computeSimilarityForTest(entryA, entryB, selHeaders, selPayload) {
  if (selHeaders.length === 0 && selPayload.length === 0) return 0;

  let score = 0;
  let total = 0;

  if (selHeaders.length > 0) {
    total += 50;
    const mapA = new Map((entryA.requestHeaders || []).map((h) => [h.name.toLowerCase(), h.value]));
    const mapB = new Map((entryB.requestHeaders || []).map((h) => [h.name.toLowerCase(), h.value]));
    let matching = 0;
    selHeaders.forEach((key) => {
      const a = mapA.get(key);
      const b = mapB.get(key);
      if (a !== undefined && b !== undefined) {
        matching += a === b ? 1 : 0.5;
      } else if (a === undefined && b === undefined) {
        matching += 1;
      }
    });
    score += (matching / selHeaders.length) * 50;
  }

  if (selPayload.length > 0) {
    total += 50;
    const getObj = (entry) => {
      if (!entry.requestBody?.data) return {};
      try {
        const d = typeof entry.requestBody.data === "string" ? JSON.parse(entry.requestBody.data) : entry.requestBody.data;
        return typeof d === "object" && d !== null ? d : {};
      } catch { return {}; }
    };
    const objA = getObj(entryA);
    const objB = getObj(entryB);
    let matching = 0;
    selPayload.forEach((key) => {
      const a = JSON.stringify(objA[key] ?? null);
      const b = JSON.stringify(objB[key] ?? null);
      if (a === b) matching += 1;
      else if (objA[key] !== undefined && objB[key] !== undefined) matching += 0.3;
    });
    score += (matching / selPayload.length) * 50;
  }

  if (total === 0) return 0;
  const result = Math.round((score / total) * 100);
  return isNaN(result) ? 0 : result;
}

function convertHarEntries(harEntries) {
  return harEntries.map((entry, i) => {
    const req = entry.request || {};
    const res = entry.response || {};
    let requestBody = null;
    if (req.postData) {
      if (req.postData.mimeType && req.postData.mimeType.includes("form")) {
        const formData = {};
        (req.postData.params || []).forEach((p) => {
          formData[p.name] = formData[p.name] ? [].concat(formData[p.name], p.value) : [p.value];
        });
        requestBody = { type: "formData", data: formData };
      } else {
        requestBody = { type: "raw", data: req.postData.text || "" };
      }
    }
    let responseBody = null;
    if (res.content && res.content.text) {
      responseBody = res.content.text;
    }
    return {
      id: `har-${i}-${Date.now()}`,
      timestamp: entry.startedDateTime ? new Date(entry.startedDateTime).getTime() : Date.now(),
      url: req.url || "",
      method: req.method || "GET",
      type: (req.headers || []).find((h) => h.name.toLowerCase() === "sec-fetch-dest")?.value || "other",
      tabId: -1,
      requestHeaders: (req.headers || []).map((h) => ({ name: h.name, value: h.value })),
      responseHeaders: (res.headers || []).map((h) => ({ name: h.name, value: h.value })),
      statusCode: res.status || 0,
      duration: entry.time ? Math.round(entry.time) : null,
      requestBody,
      responseBody,
    };
  });
}

function percentile(sortedArr, p) {
  if (sortedArr.length === 0) return 0;
  const idx = Math.ceil((p / 100) * sortedArr.length) - 1;
  return sortedArr[Math.max(0, idx)];
}

function getStatusClass(code) {
  if (!code) return "status-err";
  if (code < 300) return "status-2xx";
  if (code < 400) return "status-3xx";
  if (code < 500) return "status-4xx";
  return "status-5xx";
}

/* ============================================================
 * urlPatternToRegex tests
 * ============================================================ */

describe("urlPatternToRegex", () => {
  test("matches exact URL", () => {
    const re = urlPatternToRegex("https://api.example.com/data");
    expect(re.test("https://api.example.com/data")).toBe(true);
    expect(re.test("https://api.example.com/other")).toBe(false);
  });

  test("wildcard * matches any characters", () => {
    const re = urlPatternToRegex("https://api.example.com/*");
    expect(re.test("https://api.example.com/")).toBe(true);
    expect(re.test("https://api.example.com/users/123")).toBe(true);
    expect(re.test("https://other.com/")).toBe(false);
  });

  test("double wildcard for scheme", () => {
    const re = urlPatternToRegex("*://example.com/*");
    expect(re.test("https://example.com/page")).toBe(true);
    expect(re.test("http://example.com/page")).toBe(true);
  });

  test("wildcard in middle of URL", () => {
    const re = urlPatternToRegex("*api*");
    expect(re.test("https://api.example.com/data")).toBe(true);
    expect(re.test("https://example.com/api/v2")).toBe(true);
    expect(re.test("https://example.com/page")).toBe(false);
  });

  test("case insensitive matching", () => {
    const re = urlPatternToRegex("https://API.Example.COM/*");
    expect(re.test("https://api.example.com/data")).toBe(true);
  });

  test("escapes regex special characters", () => {
    const re = urlPatternToRegex("https://example.com/path?key=value");
    expect(re.test("https://example.com/path?key=value")).toBe(true);
    expect(re.test("https://example.com/pathXkeyYvalue")).toBe(false);
  });
});

/* ============================================================
 * computeSimilarity tests
 * ============================================================ */

describe("computeSimilarity", () => {
  test("identical headers give 100%", () => {
    const a = { requestHeaders: [{ name: "X-Cmd", value: "test" }] };
    const b = { requestHeaders: [{ name: "X-Cmd", value: "test" }] };
    expect(computeSimilarityForTest(a, b, ["x-cmd"], [])).toBe(100);
  });

  test("same header name different value gives 50% per header", () => {
    const a = { requestHeaders: [{ name: "Auth", value: "token1" }] };
    const b = { requestHeaders: [{ name: "Auth", value: "token2" }] };
    expect(computeSimilarityForTest(a, b, ["auth"], [])).toBe(50);
  });

  test("missing header in one side gives 0 for that header", () => {
    const a = { requestHeaders: [{ name: "X-Cmd", value: "test" }] };
    const b = { requestHeaders: [] };
    expect(computeSimilarityForTest(a, b, ["x-cmd"], [])).toBe(0);
  });

  test("both missing a header counts as match", () => {
    const a = { requestHeaders: [] };
    const b = { requestHeaders: [] };
    expect(computeSimilarityForTest(a, b, ["x-nonexistent"], [])).toBe(100);
  });

  test("identical JSON payload gives 100%", () => {
    const a = { requestBody: { type: "raw", data: '{"key":"val"}' } };
    const b = { requestBody: { type: "raw", data: '{"key":"val"}' } };
    expect(computeSimilarityForTest(a, b, [], ["key"])).toBe(100);
  });

  test("different payload values gives partial score", () => {
    const a = { requestBody: { type: "raw", data: '{"key":"val1"}' } };
    const b = { requestBody: { type: "raw", data: '{"key":"val2"}' } };
    expect(computeSimilarityForTest(a, b, [], ["key"])).toBe(30);
  });

  test("no selected keys returns 0", () => {
    const a = { requestHeaders: [{ name: "X", value: "1" }] };
    const b = { requestHeaders: [{ name: "X", value: "1" }] };
    expect(computeSimilarityForTest(a, b, [], [])).toBe(0);
  });

  test("combined headers and payload scoring", () => {
    const a = {
      requestHeaders: [{ name: "Auth", value: "same" }],
      requestBody: { type: "raw", data: '{"id":1}' },
    };
    const b = {
      requestHeaders: [{ name: "Auth", value: "same" }],
      requestBody: { type: "raw", data: '{"id":1}' },
    };
    expect(computeSimilarityForTest(a, b, ["auth"], ["id"])).toBe(100);
  });

  test("handles formData object body without NaN", () => {
    const a = { requestBody: { type: "formData", data: { key: ["val"] } } };
    const b = { requestBody: { type: "formData", data: { key: ["val"] } } };
    const result = computeSimilarityForTest(a, b, [], ["key"]);
    expect(result).not.toBeNaN();
    expect(result).toBeGreaterThanOrEqual(0);
  });

  test("null requestBody on both sides gives 100% for payload keys", () => {
    const a = {};
    const b = {};
    expect(computeSimilarityForTest(a, b, [], ["anykey"])).toBe(100);
  });
});

/* ============================================================
 * HAR conversion tests
 * ============================================================ */

describe("convertHarEntries", () => {
  test("converts a basic HAR entry", () => {
    const har = [{
      startedDateTime: "2024-01-15T10:00:00Z",
      time: 150,
      request: {
        method: "GET",
        url: "https://example.com/api",
        headers: [{ name: "Accept", value: "application/json" }],
      },
      response: {
        status: 200,
        headers: [{ name: "Content-Type", value: "application/json" }],
        content: { text: '{"ok":true}' },
      },
    }];

    const result = convertHarEntries(har);
    expect(result).toHaveLength(1);
    expect(result[0].url).toBe("https://example.com/api");
    expect(result[0].method).toBe("GET");
    expect(result[0].statusCode).toBe(200);
    expect(result[0].duration).toBe(150);
    expect(result[0].requestHeaders).toEqual([{ name: "Accept", value: "application/json" }]);
    expect(result[0].responseHeaders).toEqual([{ name: "Content-Type", value: "application/json" }]);
    expect(result[0].responseBody).toBe('{"ok":true}');
    expect(result[0].requestBody).toBeNull();
  });

  test("converts HAR entry with POST JSON body", () => {
    const har = [{
      request: {
        method: "POST",
        url: "https://example.com/api",
        headers: [],
        postData: { mimeType: "application/json", text: '{"key":"value"}' },
      },
      response: { status: 201, headers: [] },
    }];

    const result = convertHarEntries(har);
    expect(result[0].requestBody).toEqual({ type: "raw", data: '{"key":"value"}' });
  });

  test("converts HAR entry with form data body", () => {
    const har = [{
      request: {
        method: "POST",
        url: "https://example.com/login",
        headers: [],
        postData: {
          mimeType: "application/x-www-form-urlencoded",
          params: [
            { name: "username", value: "admin" },
            { name: "password", value: "secret" },
          ],
        },
      },
      response: { status: 302, headers: [] },
    }];

    const result = convertHarEntries(har);
    expect(result[0].requestBody.type).toBe("formData");
    expect(result[0].requestBody.data.username).toEqual(["admin"]);
    expect(result[0].requestBody.data.password).toEqual(["secret"]);
  });

  test("handles missing response content", () => {
    const har = [{
      request: { method: "GET", url: "https://example.com", headers: [] },
      response: { status: 204, headers: [] },
    }];

    const result = convertHarEntries(har);
    expect(result[0].responseBody).toBeNull();
  });

  test("handles empty HAR array", () => {
    expect(convertHarEntries([])).toEqual([]);
  });

  test("assigns unique IDs to each entry", () => {
    const har = [
      { request: { method: "GET", url: "https://a.com", headers: [] }, response: { status: 200, headers: [] } },
      { request: { method: "GET", url: "https://b.com", headers: [] }, response: { status: 200, headers: [] } },
    ];

    const result = convertHarEntries(har);
    expect(result[0].id).not.toBe(result[1].id);
  });
});

/* ============================================================
 * percentile tests
 * ============================================================ */

describe("percentile", () => {
  test("returns 0 for empty array", () => {
    expect(percentile([], 50)).toBe(0);
  });

  test("P50 of sorted array", () => {
    expect(percentile([10, 20, 30, 40, 50], 50)).toBe(30);
  });

  test("P90 of sorted array", () => {
    expect(percentile([10, 20, 30, 40, 50, 60, 70, 80, 90, 100], 90)).toBe(90);
  });

  test("P99 of single element", () => {
    expect(percentile([42], 99)).toBe(42);
  });

  test("P0 returns first element", () => {
    expect(percentile([10, 20, 30], 0)).toBe(10);
  });
});

/* ============================================================
 * getStatusClass tests
 * ============================================================ */

describe("getStatusClass", () => {
  test("2xx returns status-2xx", () => {
    expect(getStatusClass(200)).toBe("status-2xx");
    expect(getStatusClass(201)).toBe("status-2xx");
    expect(getStatusClass(299)).toBe("status-2xx");
  });

  test("3xx returns status-3xx", () => {
    expect(getStatusClass(301)).toBe("status-3xx");
    expect(getStatusClass(304)).toBe("status-3xx");
  });

  test("4xx returns status-4xx", () => {
    expect(getStatusClass(400)).toBe("status-4xx");
    expect(getStatusClass(404)).toBe("status-4xx");
  });

  test("5xx returns status-5xx", () => {
    expect(getStatusClass(500)).toBe("status-5xx");
    expect(getStatusClass(503)).toBe("status-5xx");
  });

  test("null/0/undefined returns status-err", () => {
    expect(getStatusClass(null)).toBe("status-err");
    expect(getStatusClass(0)).toBe("status-err");
    expect(getStatusClass(undefined)).toBe("status-err");
  });
});
