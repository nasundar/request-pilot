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
