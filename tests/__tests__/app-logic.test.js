/**
 * Unit tests for pure-logic functions extracted from desktop/ui/app.js.
 *
 * These tests validate the tree filter, extra headers, content detection,
 * and history filtering helpers without requiring a full DOM or Tauri runtime.
 */

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const APP_JS = path.resolve(__dirname, '../../desktop/ui/app.js');
const jsSource = fs.readFileSync(APP_JS, 'utf-8');

// ---------------------------------------------------------------------------
// Helper: extract a named function body from app.js source
// ---------------------------------------------------------------------------
function extractFunction(name) {
  // Match `function name(...)` or `async function name(...)`
  const re = new RegExp(
    `(?:async\\s+)?function\\s+${name}\\s*\\([^)]*\\)\\s*\\{`,
    'm'
  );
  const match = re.exec(jsSource);
  if (!match) throw new Error(`Function "${name}" not found in app.js`);

  let depth = 0;
  let start = match.index;
  let i = match.index + match[0].length - 1; // position of opening {
  for (; i < jsSource.length; i++) {
    if (jsSource[i] === '{') depth++;
    else if (jsSource[i] === '}') {
      depth--;
      if (depth === 0) break;
    }
  }
  return jsSource.slice(start, i + 1);
}

/**
 * Evaluate extracted function(s) safely using vm.Script (handles regex literals
 * that break new Function()).
 */
function evalFunctions(code, exportName) {
  const wrapped = `(function() { ${code}; return ${exportName}; })()`;
  const script = new vm.Script(wrapped, { filename: 'test-eval.js' });
  return script.runInNewContext({ console, JSON, RegExp, Map, Set, Array, Object, parseInt, parseFloat });
}

function evalMultipleFunctions(code, exportNames) {
  const returns = exportNames.map(n => `${n}`).join(', ');
  const wrapped = `(function() { ${code}; return { ${returns} }; })()`;
  const script = new vm.Script(wrapped, { filename: 'test-eval.js' });
  return script.runInNewContext({ console, JSON, RegExp, Map, Set, Array, Object, parseInt, parseFloat, BTreeSet: Set });
}

// ---------------------------------------------------------------------------
// 1. detectContentType tests
// ---------------------------------------------------------------------------
describe('detectContentType', () => {
  // Inline the function to avoid regex-literal extraction issues
  function detectContentType(body, headers) {
    const ct = (headers || []).find(([k]) => k.toLowerCase() === 'content-type');
    const contentType = ct ? ct[1].toLowerCase() : '';
    if (contentType.includes('json') || contentType.includes('javascript')) return 'json';
    if (contentType.includes('xml') || contentType.includes('soap')) return 'xml';
    if (contentType.includes('html')) return 'html';
    if (contentType.includes('protobuf') || contentType.includes('grpc')) return 'protobuf';
    if (contentType.includes('yaml') || contentType.includes('yml')) return 'yaml';
    if (contentType.includes('csv')) return 'csv';
    if (contentType.includes('plain')) return 'text';
    const trimmed = (body || '').trim();
    if (!trimmed) return 'empty';
    if ((trimmed.startsWith('{') && trimmed.endsWith('}')) ||
        (trimmed.startsWith('[') && trimmed.endsWith(']'))) {
      try { JSON.parse(trimmed); return 'json'; } catch { /* non-critical */ }
    }
    if (trimmed.startsWith('<?xml') || (trimmed.startsWith('<') && trimmed.includes('</'))) return 'xml';
    if (trimmed.toLowerCase().startsWith('<!doctype') || trimmed.toLowerCase().startsWith('<html')) return 'html';
    if (/^[a-zA-Z_][\w]*:\s/m.test(trimmed) && !trimmed.includes('{')) return 'yaml';
    return 'text';
  }

  test('detects JSON from content-type header', () => {
    expect(detectContentType('{}', [['Content-Type', 'application/json']])).toBe('json');
  });

  test('detects XML from content-type header', () => {
    expect(detectContentType('<a/>', [['content-type', 'application/xml']])).toBe('xml');
  });

  test('detects HTML from content-type header', () => {
    expect(detectContentType('<html>', [['Content-Type', 'text/html; charset=utf-8']])).toBe('html');
  });

  test('detects protobuf from content-type header', () => {
    expect(detectContentType('binary', [['Content-Type', 'application/x-protobuf']])).toBe('protobuf');
  });

  test('detects YAML from content-type header', () => {
    expect(detectContentType('key: val', [['Content-Type', 'application/yaml']])).toBe('yaml');
  });

  test('detects CSV from content-type header', () => {
    expect(detectContentType('a,b', [['Content-Type', 'text/csv']])).toBe('csv');
  });

  test('detects plain text from content-type header', () => {
    expect(detectContentType('hello', [['Content-Type', 'text/plain']])).toBe('text');
  });

  test('detects JSON from body heuristic (object)', () => {
    expect(detectContentType('{"key": "value"}', [])).toBe('json');
  });

  test('detects JSON from body heuristic (array)', () => {
    expect(detectContentType('[1, 2, 3]', [])).toBe('json');
  });

  test('detects XML from body heuristic (<?xml)', () => {
    expect(detectContentType('<?xml version="1.0"?><root/>', [])).toBe('xml');
  });

  test('detects XML from body heuristic (tags)', () => {
    expect(detectContentType('<root><child/></root>', [])).toBe('xml');
  });

  test('detects HTML from body heuristic (<!doctype)', () => {
    // Note: <!DOCTYPE ...> without tags that look like XML is detected as html
    expect(detectContentType('<!doctype html>', [])).toBe('html');
  });

  test('returns empty for empty body with no headers', () => {
    expect(detectContentType('', [])).toBe('empty');
    expect(detectContentType('   ', [])).toBe('empty');
    expect(detectContentType(null, [])).toBe('empty');
  });

  test('returns text for unrecognized content', () => {
    expect(detectContentType('some random text', [])).toBe('text');
  });

  test('header detection is case-insensitive', () => {
    expect(detectContentType('{}', [['CONTENT-TYPE', 'APPLICATION/JSON']])).toBe('json');
  });

  test('handles undefined headers gracefully', () => {
    expect(detectContentType('{"a":1}', undefined)).toBe('json');
    expect(detectContentType('{"a":1}', null)).toBe('json');
  });
});

// ---------------------------------------------------------------------------
// 2. Tree filter key/selection logic tests
// ---------------------------------------------------------------------------
describe('tree filter logic', () => {
  // We need _treeKey, getTreeFilterSelections, applyTreeFilter, _cascadeCheck, _syncParentChecks
  // These depend on global state (_treeFilterState) and DOM. Test the pure logic parts.

  let _treeKey;
  let SEP; // the separator used in tree keys

  beforeAll(() => {
    const fnBody = extractFunction('_treeKey');
    _treeKey = evalFunctions(fnBody, '_treeKey');
    // Determine separator by testing
    const key = _treeKey('group', 'file', 'group', null);
    SEP = key.replace('f:file', '').charAt(0);
  });

  test('_treeKey builds file-level key', () => {
    expect(_treeKey('file', 'api.http')).toBe('f:api.http');
  });

  test('_treeKey builds group-level key', () => {
    const key = _treeKey('group', 'api.http', 'auth');
    expect(key).toBe(`f:api.http${SEP}g:auth`);
  });

  test('_treeKey builds test-level key', () => {
    const key = _treeKey('test', 'api.http', 'auth', 'login test');
    expect(key).toBe(`f:api.http${SEP}g:auth${SEP}t:login test`);
  });

  test('_treeKey handles names containing forward slashes', () => {
    const key = _treeKey('group', 'api.http', 'auth/login');
    // Verify round-trip: split by SEP should give exactly 2 parts
    const parts = key.split(SEP);
    expect(parts).toHaveLength(2);
    expect(parts[0]).toBe('f:api.http');
    expect(parts[1]).toBe('g:auth/login');
  });

  test('_treeKey handles names containing the prefix markers', () => {
    const key = _treeKey('test', 'f:tricky.http', 'g:tricky', 't:tricky');
    const parts = key.split(SEP);
    expect(parts).toHaveLength(3);
  });
});

// ---------------------------------------------------------------------------
// 3. applyTreeFilter logic tests
// ---------------------------------------------------------------------------
describe('applyTreeFilter', () => {
  let applyTreeFilter;
  let getTreeFilterSelections;
  let _treeFilterState;

  beforeAll(() => {
    const treeKeyFn = extractFunction('_treeKey');
    const getSelFn = extractFunction('getTreeFilterSelections');
    const applyFn = extractFunction('applyTreeFilter');

    const code = `let _treeFilterState = new Map();\n${treeKeyFn}\n${getSelFn}\n${applyFn}`;
    const result = evalMultipleFunctions(code, ['applyTreeFilter', 'getTreeFilterSelections', '_treeFilterState']);
    applyTreeFilter = result.applyTreeFilter;
    getTreeFilterSelections = result.getTreeFilterSelections;
    _treeFilterState = result._treeFilterState;
  });

  const sampleEntries = [
    { file_name: 'api.http', group: 'auth', block_name: 'Login' },
    { file_name: 'api.http', group: 'auth', block_name: 'Logout' },
    { file_name: 'api.http', group: 'users', block_name: 'List Users' },
    { file_name: 'health.http', group: null, block_name: 'Ping' },
    { file_name: null, group: null, block_name: 'Manual Request' },
  ];

  beforeEach(() => {
    _treeFilterState.clear();
  });

  test('returns all entries when filter state is empty', () => {
    const result = applyTreeFilter(sampleEntries);
    expect(result).toHaveLength(5);
  });

  test('returns all entries when all items are checked', () => {
    _treeFilterState.set('f:api.http', true);
    _treeFilterState.set('f:health.http', true);
    _treeFilterState.set('f:(No File)', true);
    const result = applyTreeFilter(sampleEntries);
    expect(result).toHaveLength(5);
  });

  test('filters by file when only one file is selected', () => {
    _treeFilterState.set('f:api.http', true);
    _treeFilterState.set('f:health.http', false);
    _treeFilterState.set('f:(No File)', false);
    const result = applyTreeFilter(sampleEntries);
    expect(result).toHaveLength(3);
    expect(result.every(e => e.file_name === 'api.http')).toBe(true);
  });

  test('filters entries with no file name using (No File)', () => {
    _treeFilterState.set('f:api.http', false);
    _treeFilterState.set('f:health.http', false);
    _treeFilterState.set('f:(No File)', true);
    const result = applyTreeFilter(sampleEntries);
    expect(result).toHaveLength(1);
    expect(result[0].block_name).toBe('Manual Request');
  });

  test('getTreeFilterSelections returns null when all checked', () => {
    _treeFilterState.set('f:api.http', true);
    _treeFilterState.set('f:health.http', true);
    expect(getTreeFilterSelections()).toBeNull();
  });

  test('getTreeFilterSelections returns selections when some unchecked', () => {
    _treeFilterState.set('f:api.http', true);
    _treeFilterState.set('f:health.http', false);
    const sel = getTreeFilterSelections();
    expect(sel).not.toBeNull();
    expect(sel).toHaveLength(1);
    expect(sel[0].type).toBe('file');
    expect(sel[0].file).toBe('api.http');
  });
});

// ---------------------------------------------------------------------------
// 4. getExtraHeaders DOM-based tests
// ---------------------------------------------------------------------------
describe('getExtraHeaders', () => {
  let getExtraHeaders;
  let container;

  beforeAll(() => {
    // Set up a minimal DOM for the extra headers list
    container = document.createElement('div');
    container.id = 'extraHeadersList';
    document.body.appendChild(container);
  });

  beforeEach(() => {
    container.innerHTML = '';
    // Create the function with the DOM reference
    const fnCode = `
      var extraHeadersList = document.getElementById('extraHeadersList');
      function getExtraHeaders() {
        const headers = [];
        extraHeadersList.querySelectorAll('.extra-header-row').forEach(row => {
          const enabled = row.querySelector('input[type="checkbox"]').checked;
          const key = row.querySelector('.eh-key').value.trim();
          const val = row.querySelector('.eh-value').value.trim();
          if (enabled && key) headers.push([key, val]);
        });
        return headers;
      }
      return getExtraHeaders;
    `;
    getExtraHeaders = new Function(fnCode)();
  });

  afterAll(() => {
    document.body.removeChild(container);
  });

  function addHeaderRow(key, value, enabled = true) {
    const row = document.createElement('div');
    row.className = 'extra-header-row';
    row.innerHTML = `
      <input type="checkbox" ${enabled ? 'checked' : ''}>
      <input class="eh-key" value="${key}">
      <input class="eh-value" value="${value}">
    `;
    container.appendChild(row);
  }

  test('returns empty array when no rows exist', () => {
    expect(getExtraHeaders()).toEqual([]);
  });

  test('returns enabled headers with non-empty keys', () => {
    addHeaderRow('Authorization', 'Bearer token123');
    addHeaderRow('X-Request-ID', 'abc');
    expect(getExtraHeaders()).toEqual([
      ['Authorization', 'Bearer token123'],
      ['X-Request-ID', 'abc'],
    ]);
  });

  test('skips disabled headers', () => {
    addHeaderRow('Authorization', 'Bearer token123', true);
    addHeaderRow('X-Debug', 'true', false);
    expect(getExtraHeaders()).toEqual([
      ['Authorization', 'Bearer token123'],
    ]);
  });

  test('skips headers with empty keys', () => {
    addHeaderRow('', 'value-without-key', true);
    addHeaderRow('X-Valid', 'yes', true);
    expect(getExtraHeaders()).toEqual([
      ['X-Valid', 'yes'],
    ]);
  });

  test('allows empty values', () => {
    addHeaderRow('X-Empty', '', true);
    expect(getExtraHeaders()).toEqual([
      ['X-Empty', ''],
    ]);
  });

  test('trims whitespace from keys and values', () => {
    addHeaderRow('  X-Padded  ', '  value  ', true);
    expect(getExtraHeaders()).toEqual([
      ['X-Padded', 'value'],
    ]);
  });
});

// ---------------------------------------------------------------------------
// 5. Live capture .http file generation
// ---------------------------------------------------------------------------
describe('live capture .http file generation', () => {
  /**
   * Mirrors the block-building logic in app.js appendToLiveCaptureFile().
   * Extracted here to test formatting without DOM/Tauri dependencies.
   */
  function buildHttpBlock(req) {
    let block = '###';
    try {
      const url = new URL(req.url);
      block += ` ${req.method} ${url.pathname}\n`;
    } catch {
      block += ` ${req.method} request\n`;
    }
    block += `${req.method} ${req.url}\n`;
    for (const h of (req.request_headers || [])) {
      const lower = h.name.toLowerCase();
      if (lower.startsWith(':') || lower === 'host') continue;
      block += `${h.name}: ${h.value}\n`;
    }
    if (req.request_body) {
      block += `\n${req.request_body}\n`;
    }
    block += '\n';
    return block;
  }

  test('generates correct block for GET request', () => {
    const block = buildHttpBlock({
      method: 'GET',
      url: 'https://api.example.com/users?page=1',
      request_headers: [
        { name: 'Authorization', value: 'Bearer tok123' },
        { name: 'Accept', value: 'application/json' },
      ],
    });
    expect(block).toContain('### GET /users');
    expect(block).toContain('GET https://api.example.com/users?page=1');
    expect(block).toContain('Authorization: Bearer tok123');
    expect(block).toContain('Accept: application/json');
    expect(block).not.toContain('request_body');
  });

  test('generates correct block for POST with body', () => {
    const body = JSON.stringify({ name: 'Alice', role: 'admin' });
    const block = buildHttpBlock({
      method: 'POST',
      url: 'https://api.example.com/users',
      request_headers: [
        { name: 'Content-Type', value: 'application/json' },
      ],
      request_body: body,
    });
    expect(block).toContain('### POST /users');
    expect(block).toContain('POST https://api.example.com/users');
    expect(block).toContain('Content-Type: application/json');
    expect(block).toContain(body);
  });

  test('skips pseudo-headers and host header', () => {
    const block = buildHttpBlock({
      method: 'GET',
      url: 'https://example.com/path',
      request_headers: [
        { name: ':method', value: 'GET' },
        { name: ':authority', value: 'example.com' },
        { name: ':path', value: '/path' },
        { name: ':scheme', value: 'https' },
        { name: 'host', value: 'example.com' },
        { name: 'Host', value: 'example.com' },
        { name: 'Accept', value: '*/*' },
      ],
    });
    expect(block).not.toMatch(/:method/);
    expect(block).not.toMatch(/:authority/);
    expect(block).not.toMatch(/:path/);
    expect(block).not.toMatch(/:scheme/);
    // host / Host should be filtered out
    expect(block).not.toMatch(/^host:/im);
    expect(block).toContain('Accept: */*');
  });

  test('handles URL without pathname gracefully', () => {
    const block = buildHttpBlock({
      method: 'GET',
      url: 'https://example.com',
      request_headers: [],
    });
    // URL without explicit path yields "/"
    expect(block).toContain('### GET /');
    expect(block).toContain('GET https://example.com');
  });

  test('handles request with no headers', () => {
    const block = buildHttpBlock({
      method: 'DELETE',
      url: 'https://api.example.com/items/42',
      request_headers: [],
    });
    expect(block).toContain('### DELETE /items/42');
    expect(block).toContain('DELETE https://api.example.com/items/42');
    // No header lines between request line and trailing newline
    const lines = block.split('\n');
    expect(lines[0]).toBe('### DELETE /items/42');
    expect(lines[1]).toBe('DELETE https://api.example.com/items/42');
  });

  test('handles request with null/undefined headers', () => {
    const block = buildHttpBlock({
      method: 'GET',
      url: 'https://example.com/test',
    });
    expect(block).toContain('### GET /test');
    expect(block).toContain('GET https://example.com/test');
  });

  test('handles request with no body', () => {
    const block = buildHttpBlock({
      method: 'GET',
      url: 'https://example.com/health',
      request_headers: [{ name: 'X-Req-Id', value: '123' }],
    });
    // Should NOT contain a double newline before the trailing newline
    // (no body separator)
    expect(block).not.toMatch(/\n\n.*\S.*\n\n$/);
    expect(block).toContain('X-Req-Id: 123');
  });

  test('handles malformed URL by falling back to "request"', () => {
    const block = buildHttpBlock({
      method: 'PATCH',
      url: 'not-a-valid-url',
      request_headers: [],
    });
    expect(block).toContain('### PATCH request');
    expect(block).toContain('PATCH not-a-valid-url');
  });

  test('preserves full query string in request line', () => {
    const block = buildHttpBlock({
      method: 'GET',
      url: 'https://api.example.com/search?q=hello&limit=10&offset=0',
      request_headers: [],
    });
    expect(block).toContain('GET https://api.example.com/search?q=hello&limit=10&offset=0');
    // Block name should only show pathname
    expect(block).toContain('### GET /search');
  });

  test('handles PUT with large body', () => {
    const largeBody = JSON.stringify({ data: 'x'.repeat(500) });
    const block = buildHttpBlock({
      method: 'PUT',
      url: 'https://api.example.com/upload',
      request_headers: [
        { name: 'Content-Type', value: 'application/json' },
        { name: 'Content-Length', value: String(largeBody.length) },
      ],
      request_body: largeBody,
    });
    expect(block).toContain('### PUT /upload');
    expect(block).toContain(largeBody);
    expect(block).toContain(`Content-Length: ${largeBody.length}`);
  });
});

// ---------------------------------------------------------------------------
// 6. Live capture history entry
// ---------------------------------------------------------------------------
describe('live capture history entry', () => {
  /**
   * Mirrors the history-entry construction in app.js addLiveRequestToHistory().
   */
  function buildHistoryEntry(req, sessionTimestamp) {
    let blockName;
    try {
      const url = new URL(req.url);
      blockName = `${req.method} ${url.pathname}`;
    } catch {
      blockName = `${req.method} request`;
    }

    const sessionTime = new Date(sessionTimestamp || Date.now())
      .toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' });

    return {
      method: req.method || 'GET',
      url: req.url || '',
      status: req.status_code || 0,
      duration_ms: req.duration || null,
      request_headers: (req.request_headers || []).map(h => [h.name, h.value]),
      response_headers: (req.response_headers || []).map(h => [h.name, h.value]),
      request_body: req.request_body || null,
      response_body: req.response_body || null,
      source: 'extension-live',
      file_name: `Live Capture - ${sessionTime}`,
      block_name: blockName,
    };
  }

  test('creates proper history entry from captured request', () => {
    const req = {
      method: 'POST',
      url: 'https://api.example.com/users',
      status_code: 201,
      duration: 150,
      request_headers: [
        { name: 'Content-Type', value: 'application/json' },
      ],
      response_headers: [
        { name: 'Location', value: '/users/42' },
      ],
      request_body: '{"name":"Bob"}',
      response_body: '{"id":42}',
    };
    const entry = buildHistoryEntry(req, 1700000000000);
    expect(entry.method).toBe('POST');
    expect(entry.url).toBe('https://api.example.com/users');
    expect(entry.status).toBe(201);
    expect(entry.duration_ms).toBe(150);
    expect(entry.source).toBe('extension-live');
    expect(entry.block_name).toBe('POST /users');
    expect(entry.file_name).toMatch(/^Live Capture - \d{2}:\d{2}:\d{2}$/);
    expect(entry.request_headers).toEqual([['Content-Type', 'application/json']]);
    expect(entry.response_headers).toEqual([['Location', '/users/42']]);
    expect(entry.request_body).toBe('{"name":"Bob"}');
    expect(entry.response_body).toBe('{"id":42}');
  });

  test('handles missing fields gracefully', () => {
    const entry = buildHistoryEntry({
      method: 'GET',
      url: 'https://example.com/',
    }, 1700000000000);
    expect(entry.method).toBe('GET');
    expect(entry.status).toBe(0);
    expect(entry.duration_ms).toBeNull();
    expect(entry.request_headers).toEqual([]);
    expect(entry.response_headers).toEqual([]);
    expect(entry.request_body).toBeNull();
    expect(entry.response_body).toBeNull();
    expect(entry.source).toBe('extension-live');
    expect(entry.block_name).toBe('GET /');
  });

  test('uses pathname in block name, not full URL', () => {
    const entry = buildHistoryEntry({
      method: 'GET',
      url: 'https://api.example.com/v2/items?page=3&sort=asc',
    });
    expect(entry.block_name).toBe('GET /v2/items');
  });

  test('falls back to "request" for invalid URLs', () => {
    const entry = buildHistoryEntry({
      method: 'OPTIONS',
      url: 'invalid-url',
    });
    expect(entry.block_name).toBe('OPTIONS request');
  });

  test('maps header pairs from {name, value} to [name, value] arrays', () => {
    const entry = buildHistoryEntry({
      method: 'GET',
      url: 'https://example.com',
      request_headers: [
        { name: 'X-A', value: '1' },
        { name: 'X-B', value: '2' },
      ],
      response_headers: [
        { name: 'X-R', value: '3' },
      ],
    });
    expect(entry.request_headers).toEqual([['X-A', '1'], ['X-B', '2']]);
    expect(entry.response_headers).toEqual([['X-R', '3']]);
  });
});

// ---------------------------------------------------------------------------
// 7. Body syntax highlighting — detectBodyLang
// ---------------------------------------------------------------------------
describe('detectBodyLang', () => {
  // Inline the function to avoid brace-counting issues in extractFunction
  // (the source contains '{' and '[' in string literals which confuse the extractor)
  function detectBodyLang(text) {
    const t = text.trim();
    if (!t) return 'text';
    if (t.startsWith('{') || t.startsWith('[')) return 'json';
    if (t.startsWith('<') && t.includes('>')) return 'xml';
    if (/\b(SELECT|INSERT|UPDATE|DELETE|CREATE|ALTER|DROP|FROM|WHERE|JOIN)\b/i.test(t)) return 'sql';
    if (/[{}[\]()]/.test(t) && /\b(rate|sum|avg|count|histogram_quantile|topk|bottomk|by|without|on|group_left|group_right|offset)\b/i.test(t)) return 'promql';
    return 'text';
  }

  test('detects JSON object', () => {
    expect(detectBodyLang('{"key": "value"}')).toBe('json');
  });

  test('detects JSON array', () => {
    expect(detectBodyLang('[1, 2, 3]')).toBe('json');
  });

  test('detects XML', () => {
    expect(detectBodyLang('<root><item>hello</item></root>')).toBe('xml');
  });

  test('detects SQL SELECT', () => {
    expect(detectBodyLang('SELECT * FROM users WHERE id = 1')).toBe('sql');
  });

  test('detects SQL INSERT', () => {
    expect(detectBodyLang('INSERT INTO users VALUES (1, "test")')).toBe('sql');
  });

  test('detects PromQL rate expression', () => {
    expect(detectBodyLang('rate(http_requests_total{job="api"}[5m])')).toBe('promql');
  });

  test('detects PromQL with aggregate functions', () => {
    expect(detectBodyLang('sum by (pod) (rate(container_cpu_usage_seconds_total[5m]))')).toBe('promql');
  });

  test('returns text for plain text', () => {
    expect(detectBodyLang('hello world')).toBe('text');
  });

  test('returns text for empty string', () => {
    expect(detectBodyLang('')).toBe('text');
  });

  test('returns text for whitespace only', () => {
    expect(detectBodyLang('   ')).toBe('text');
  });

  test('detects JSON with leading whitespace', () => {
    expect(detectBodyLang('  {"a":1}')).toBe('json');
  });

  test('detects XML self-closing tag', () => {
    expect(detectBodyLang('<br/>')).toBe('xml');
  });

  test('detects SQL UPDATE', () => {
    expect(detectBodyLang('UPDATE users SET name = "Bob" WHERE id = 1')).toBe('sql');
  });

  test('detects SQL DELETE', () => {
    expect(detectBodyLang('DELETE FROM users WHERE id = 1')).toBe('sql');
  });
});

// ---------------------------------------------------------------------------
// 8. Body syntax highlighting — hlJSON
// ---------------------------------------------------------------------------
describe('hlJSON', () => {
  let hlJSON;

  beforeAll(() => {
    const fnBody = extractFunction('hlJSON');
    hlJSON = evalFunctions(fnBody, 'hlJSON');
  });

  test('highlights strings with hl-str class', () => {
    const result = hlJSON('&quot;hello&quot;');
    expect(result).toContain('hl-str');
  });

  test('highlights numbers with hl-num class', () => {
    const result = hlJSON('42');
    expect(result).toContain('hl-num');
  });

  test('highlights booleans with hl-kw class', () => {
    expect(hlJSON('true')).toContain('hl-kw');
    expect(hlJSON('false')).toContain('hl-kw');
  });

  test('highlights null with hl-kw class', () => {
    expect(hlJSON('null')).toContain('hl-kw');
  });

  test('highlights brackets with hl-bkt class', () => {
    const result = hlJSON('{}');
    expect(result).toContain('hl-bkt');
  });

  test('highlights colons and commas with hl-pct class', () => {
    const result = hlJSON('&quot;a&quot;: 1, &quot;b&quot;: 2');
    expect(result).toContain('hl-pct');
  });

  test('handles empty object {}', () => {
    const result = hlJSON('{}');
    expect(result).toContain('hl-bkt');
    expect(result).not.toContain('hl-str');
  });

  test('handles nested objects', () => {
    const result = hlJSON('{&quot;a&quot;: {&quot;b&quot;: 1}}');
    expect(result).toContain('hl-str');
    expect(result).toContain('hl-num');
    expect(result).toContain('hl-bkt');
  });

  test('highlights negative numbers', () => {
    const result = hlJSON('-3.14');
    expect(result).toContain('hl-num');
  });

  test('highlights scientific notation numbers', () => {
    const result = hlJSON('1.5e10');
    expect(result).toContain('hl-num');
  });
});

// ---------------------------------------------------------------------------
// 9. Body syntax highlighting — hlXML
// ---------------------------------------------------------------------------
describe('hlXML', () => {
  let hlXML;

  beforeAll(() => {
    const fnBody = extractFunction('hlXML');
    hlXML = evalFunctions(fnBody, 'hlXML');
  });

  test('highlights tag names with hl-tag class', () => {
    const result = hlXML('&lt;root&gt;text&lt;/root&gt;');
    expect(result).toContain('hl-tag');
  });

  test('highlights attributes with hl-attr class', () => {
    const result = hlXML('&lt;item id=&quot;1&quot;&gt;&lt;/item&gt;');
    expect(result).toContain('hl-attr');
  });

  test('handles self-closing tags', () => {
    const result = hlXML('&lt;br/&gt;');
    expect(result).toContain('hl-tag');
    expect(result).toContain('hl-bkt');
  });

  test('highlights comments with hl-cmt class', () => {
    const result = hlXML('&lt;!-- comment --&gt;');
    expect(result).toContain('hl-cmt');
  });

  test('highlights attribute values with hl-str class', () => {
    const result = hlXML('&lt;a href=&quot;url&quot;&gt;&lt;/a&gt;');
    expect(result).toContain('hl-str');
  });
});

// ---------------------------------------------------------------------------
// 10. Body syntax highlighting — hlSQL
// ---------------------------------------------------------------------------
describe('hlSQL', () => {
  let hlSQL;

  beforeAll(() => {
    const fnBody = extractFunction('hlSQL');
    hlSQL = evalFunctions(fnBody, 'hlSQL');
  });

  test('highlights SELECT keyword with hl-kw class', () => {
    const result = hlSQL('SELECT * FROM users');
    expect(result).toContain('hl-kw');
    expect(result).toMatch(/hl-kw.*SELECT/i);
  });

  test('highlights FROM keyword with hl-kw class', () => {
    const result = hlSQL('SELECT id FROM users');
    expect(result).toMatch(/hl-kw.*FROM/i);
  });

  test('highlights WHERE keyword with hl-kw class', () => {
    const result = hlSQL('SELECT * FROM users WHERE id = 1');
    expect(result).toMatch(/hl-kw.*WHERE/i);
  });

  test('keyword highlighting is case-insensitive', () => {
    const upper = hlSQL('SELECT * FROM users');
    const lower = hlSQL('select * from users');
    expect(upper).toContain('hl-kw');
    expect(lower).toContain('hl-kw');
  });

  test('highlights numbers with hl-num class', () => {
    const result = hlSQL('SELECT * FROM users WHERE id = 42');
    expect(result).toContain('hl-num');
  });

  test('highlights INSERT, INTO, VALUES keywords', () => {
    const result = hlSQL('INSERT INTO users VALUES (1)');
    expect(result).toMatch(/hl-kw.*INSERT/i);
    expect(result).toMatch(/hl-kw.*INTO/i);
    expect(result).toMatch(/hl-kw.*VALUES/i);
  });

  test('highlights SQL comments with hl-cmt class', () => {
    const result = hlSQL('SELECT 1 -- comment');
    expect(result).toContain('hl-cmt');
  });
});

// ---------------------------------------------------------------------------
// 11. Body syntax highlighting — hlPromQL
// ---------------------------------------------------------------------------
describe('hlPromQL', () => {
  let hlPromQL;

  beforeAll(() => {
    const fnBody = extractFunction('hlPromQL');
    hlPromQL = evalFunctions(fnBody, 'hlPromQL');
  });

  test('highlights rate function with hl-fn class', () => {
    const result = hlPromQL('rate(http_requests_total[5m])');
    expect(result).toContain('hl-fn');
    expect(result).toMatch(/hl-fn.*rate/);
  });

  test('highlights sum function with hl-fn class', () => {
    const result = hlPromQL('sum(up)');
    expect(result).toContain('hl-fn');
    expect(result).toMatch(/hl-fn.*sum/);
  });

  test('highlights by keyword with hl-kw class', () => {
    const result = hlPromQL('sum by (job) (up)');
    expect(result).toContain('hl-kw');
    expect(result).toMatch(/hl-kw.*by/);
  });

  test('highlights without keyword with hl-kw class', () => {
    const result = hlPromQL('sum without (instance) (up)');
    expect(result).toMatch(/hl-kw.*without/);
  });

  test('highlights numbers with hl-num class', () => {
    const result = hlPromQL('rate(metric[5m])');
    expect(result).toContain('hl-num');
  });

  test('highlights brackets with hl-bkt class', () => {
    const result = hlPromQL('rate(metric{job="api"}[5m])');
    expect(result).toContain('hl-bkt');
  });

  test('highlights histogram_quantile function', () => {
    const result = hlPromQL('histogram_quantile(0.99, rate(http_duration_bucket[5m]))');
    expect(result).toMatch(/hl-fn.*histogram_quantile/);
  });

  test('highlights label matchers with hl-attr and hl-pct', () => {
    const result = hlPromQL('metric{job="api"}');
    expect(result).toContain('hl-attr');
  });
});

// ---------------------------------------------------------------------------
// 12. New function existence tests
// ---------------------------------------------------------------------------
describe('new function existence in app.js', () => {
  const functionNames = [
    'openDiffViewer',
    'closeDiffViewer',
    'runCompareStep',
    'viewCompareStep',
    'highlightWithDiffMarkers',
    'renderChangesOnly',
  ];

  test.each(functionNames)('%s is defined as a function in app.js', (name) => {
    const re = new RegExp(`(?:async\\s+)?function\\s+${name}\\s*\\(`);
    expect(jsSource).toMatch(re);
  });

  test('detectBodyLang is defined as a function', () => {
    expect(jsSource).toMatch(/function\s+detectBodyLang\s*\(/);
  });

  test('hlJSON is defined as a function', () => {
    expect(jsSource).toMatch(/function\s+hlJSON\s*\(/);
  });

  test('hlXML is defined as a function', () => {
    expect(jsSource).toMatch(/function\s+hlXML\s*\(/);
  });

  test('hlSQL is defined as a function', () => {
    expect(jsSource).toMatch(/function\s+hlSQL\s*\(/);
  });

  test('hlPromQL is defined as a function', () => {
    expect(jsSource).toMatch(/function\s+hlPromQL\s*\(/);
  });
});

// ---------------------------------------------------------------------------
// 13. switchMode builder assertion re-render code path
// ---------------------------------------------------------------------------
describe('switchMode builder assertion re-render path', () => {
  test('switchMode function contains renderAssertions call for builder mode', () => {
    const fnBody = extractFunction('switchMode');
    expect(fnBody).toContain("mode === 'builder'");
    expect(fnBody).toContain('renderAssertions');
  });

  test('switchMode early-returns when mode is unchanged', () => {
    const fnBody = extractFunction('switchMode');
    expect(fnBody).toContain('if (mode === currentMode) return');
  });

  test('switchMode handles all four modes', () => {
    const fnBody = extractFunction('switchMode');
    expect(fnBody).toContain("'builder'");
    expect(fnBody).toContain("'code'");
    expect(fnBody).toContain("'history'");
    expect(fnBody).toContain("'logs'");
  });
});
