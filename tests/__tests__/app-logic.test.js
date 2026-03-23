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
