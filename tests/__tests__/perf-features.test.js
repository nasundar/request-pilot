/**
 * Performance features tests for desktop/ui/app.js and worker.js.
 *
 * Tests VirtualScroll, WorkerPool, offthread, buildDiffHunks,
 * renderJsonTreeFromRust / buildJsonNodeEl, renderRustCharHighlight,
 * and the pure functions from worker.js.
 */

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const APP_JS = path.resolve(__dirname, '../../desktop/ui/app.js');
const WORKER_JS = path.resolve(__dirname, '../../desktop/ui/worker.js');
const jsSource = fs.readFileSync(APP_JS, 'utf-8');

// ---------------------------------------------------------------------------
// Helper: extract a named function (or class) body from app.js source
// ---------------------------------------------------------------------------
function extractFunction(name) {
  const re = new RegExp(
    `(?:async\\s+)?function\\s+${name}\\s*\\([^)]*\\)\\s*\\{`,
    'm'
  );
  const match = re.exec(jsSource);
  if (!match) throw new Error(`Function "${name}" not found in app.js`);

  let depth = 0;
  let i = match.index + match[0].length - 1;
  for (; i < jsSource.length; i++) {
    if (jsSource[i] === '{') depth++;
    else if (jsSource[i] === '}') {
      depth--;
      if (depth === 0) break;
    }
  }
  return jsSource.slice(match.index, i + 1);
}

function extractClass(name) {
  const re = new RegExp(`class\\s+${name}\\s*\\{`, 'm');
  const match = re.exec(jsSource);
  if (!match) throw new Error(`Class "${name}" not found in app.js`);

  let depth = 0;
  let i = match.index + match[0].length - 1;
  for (; i < jsSource.length; i++) {
    if (jsSource[i] === '{') depth++;
    else if (jsSource[i] === '}') {
      depth--;
      if (depth === 0) break;
    }
  }
  return jsSource.slice(match.index, i + 1);
}

function evalCode(code, exportName, extraGlobals = {}) {
  const wrapped = `(function() { ${code}; return ${exportName}; })()`;
  const script = new vm.Script(wrapped, { filename: 'test-eval.js' });
  return script.runInNewContext({
    console, JSON, RegExp, Map, Set, Array, Object,
    parseInt, parseFloat, Math, String, Number, Error,
    Promise, setTimeout, clearTimeout,
    ...extraGlobals
  });
}

// ===========================================================================
// 1. VirtualScroll Tests
// ===========================================================================
describe('VirtualScroll', () => {
  let container;
  let VirtualScroll;

  // Polyfill ResizeObserver for jsdom
  const origResizeObserver = global.ResizeObserver;
  beforeAll(() => {
    global.ResizeObserver = class {
      constructor() {}
      observe() {}
      unobserve() {}
      disconnect() {}
    };

    const classSource = extractClass('VirtualScroll');
    VirtualScroll = evalCode(classSource, 'VirtualScroll', {
      document, HTMLElement, requestAnimationFrame: (cb) => setTimeout(cb, 0),
      cancelAnimationFrame: clearTimeout,
      ResizeObserver: global.ResizeObserver,
    });
  });

  afterAll(() => {
    global.ResizeObserver = origResizeObserver;
  });

  beforeEach(() => {
    container = document.createElement('div');
    Object.defineProperty(container, 'clientHeight', { value: 200, configurable: true });
    container.scrollTop = 0;
    document.body.appendChild(container);
  });

  afterEach(() => {
    container.remove();
  });

  function makeVs(opts = {}) {
    return new VirtualScroll(container, {
      rowHeight: 20,
      bufferRows: 20,
      renderRow: (idx, data) => {
        const el = document.createElement('div');
        el.textContent = `row-${idx}`;
        el.dataset.idx = idx;
        return el;
      },
      ...opts,
    });
  }

  test('creates DOM structure (spacer + viewport)', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 100 }, (_, i) => ({ i })));
    const spacer = container.querySelector('.virtual-scroll-spacer');
    const viewport = container.querySelector('.virtual-scroll-viewport');
    expect(spacer).not.toBeNull();
    expect(viewport).not.toBeNull();
    vs.destroy();
  });

  test('renders only visible rows (not all 1000)', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 1000 }, (_, i) => ({ i })));
    const viewport = container.querySelector('.virtual-scroll-viewport');
    // visible = ceil(200/20) = 10, with buffer 20 each side → max ~50
    const childCount = viewport.children.length;
    expect(childCount).toBeGreaterThan(0);
    expect(childCount).toBeLessThan(100); // far fewer than 1000
    vs.destroy();
  });

  test('spacer height matches total items', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 1000 }, (_, i) => ({ i })));
    const spacer = container.querySelector('.virtual-scroll-spacer');
    expect(spacer.style.height).toBe('20000px'); // 1000 * 20
    vs.destroy();
  });

  test('scrollToIndex works', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 1000 }, (_, i) => ({ i })));
    vs.scrollToIndex(500);
    expect(container.scrollTop).toBe(500 * 20);
    vs.destroy();
  });

  test('setData clears previous content', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 50 }, (_, i) => ({ i })));
    vs.setData(Array.from({ length: 10 }, (_, i) => ({ i })));
    const spacer = container.querySelector('.virtual-scroll-spacer');
    expect(spacer.style.height).toBe('200px'); // 10 * 20
    vs.destroy();
  });

  test('destroy cleans up', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 100 }, (_, i) => ({ i })));
    vs.destroy();
    expect(container.querySelector('.virtual-scroll-spacer')).toBeNull();
    expect(container.querySelector('.virtual-scroll-viewport')).toBeNull();
    expect(container.classList.contains('virtual-scroll-container')).toBe(false);
  });

  test('refresh re-renders', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 100 }, (_, i) => ({ i })));
    const viewport = container.querySelector('.virtual-scroll-viewport');
    const countBefore = viewport.children.length;
    // Change clientHeight and refresh
    Object.defineProperty(container, 'clientHeight', { value: 400, configurable: true });
    vs.refresh();
    const countAfter = viewport.children.length;
    // With taller container, more rows should be rendered
    expect(countAfter).toBeGreaterThanOrEqual(countBefore);
    vs.destroy();
  });

  test('getVisibleRange returns correct range', () => {
    const vs = makeVs();
    vs.setData(Array.from({ length: 100 }, (_, i) => ({ i })));
    container.scrollTop = 0;
    const range = vs.getVisibleRange();
    expect(range.start).toBe(0);
    // ceil(200/20) = 10
    expect(range.end).toBe(10);
    vs.destroy();
  });

  test('handles empty data', () => {
    const vs = makeVs();
    expect(() => vs.setData([])).not.toThrow();
    const spacer = container.querySelector('.virtual-scroll-spacer');
    expect(spacer.style.height).toBe('0px');
    vs.destroy();
  });

  test('handles single item', () => {
    const vs = makeVs();
    vs.setData([{ x: 1 }]);
    const viewport = container.querySelector('.virtual-scroll-viewport');
    expect(viewport.children.length).toBe(1);
    expect(viewport.children[0].textContent).toBe('row-0');
    vs.destroy();
  });
});

// ===========================================================================
// 2. buildDiffHunks Tests
// ===========================================================================
describe('buildDiffHunks', () => {
  let buildDiffHunks;

  beforeAll(() => {
    const code = extractFunction('buildDiffHunks');
    buildDiffHunks = evalCode(code, 'buildDiffHunks');
  });

  function sameOp() { return { type: 'same' }; }
  function changeOp() { return { type: 'change' }; }
  function makeOps(length, changeAt = []) {
    return Array.from({ length }, (_, i) =>
      changeAt.includes(i) ? changeOp() : sameOp()
    );
  }

  test('identical files produce single collapse', () => {
    const ops = makeOps(100);
    const hunks = buildDiffHunks(ops);
    expect(hunks).toHaveLength(1);
    expect(hunks[0].type).toBe('collapse');
    expect(hunks[0].count).toBe(100);
  });

  test('single change with context', () => {
    const ops = makeOps(100, [50]);
    const hunks = buildDiffHunks(ops, 3);
    // collapse(0-46) + hunk(47-53) + collapse(54-99)
    expect(hunks).toHaveLength(3);
    expect(hunks[0].type).toBe('collapse');
    expect(hunks[1].type).toBe('hunk');
    expect(hunks[1].startIdx).toBe(47);
    expect(hunks[1].endIdx).toBe(53);
    expect(hunks[2].type).toBe('collapse');
  });

  test('change at start', () => {
    const ops = makeOps(100, [0]);
    const hunks = buildDiffHunks(ops, 3);
    expect(hunks[0].type).toBe('hunk');
    expect(hunks[0].startIdx).toBe(0);
    expect(hunks[0].endIdx).toBe(3);
    expect(hunks[1].type).toBe('collapse');
  });

  test('change at end', () => {
    const ops = makeOps(100, [99]);
    const hunks = buildDiffHunks(ops, 3);
    const last = hunks[hunks.length - 1];
    expect(last.type).toBe('hunk');
    expect(last.endIdx).toBe(99);
    expect(hunks[0].type).toBe('collapse');
  });

  test('adjacent changes merge', () => {
    // Changes at 5 and 8 — their context windows overlap (5±3 and 8±3 → 2..11)
    const ops = makeOps(50, [5, 8]);
    const hunks = buildDiffHunks(ops, 3);
    const hunkItems = hunks.filter(h => h.type === 'hunk');
    expect(hunkItems).toHaveLength(1); // merged into one hunk
  });

  test('distant changes stay separate', () => {
    const ops = makeOps(100, [10, 50]);
    const hunks = buildDiffHunks(ops, 3);
    const hunkItems = hunks.filter(h => h.type === 'hunk');
    expect(hunkItems).toHaveLength(2);
    const collapseItems = hunks.filter(h => h.type === 'collapse');
    expect(collapseItems).toHaveLength(3); // before, between, after
  });

  test('all changes no collapse', () => {
    const ops = makeOps(10, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    const hunks = buildDiffHunks(ops, 3);
    const collapseItems = hunks.filter(h => h.type === 'collapse');
    expect(collapseItems).toHaveLength(0);
    const hunkItems = hunks.filter(h => h.type === 'hunk');
    expect(hunkItems).toHaveLength(1);
    expect(hunkItems[0].startIdx).toBe(0);
    expect(hunkItems[0].endIdx).toBe(9);
  });

  test('context lines count is correct', () => {
    const ops = makeOps(200, [100]);
    const hunks = buildDiffHunks(ops, 3);
    const hunk = hunks.find(h => h.type === 'hunk');
    // 3 context before change, 3 after → range 97..103
    expect(hunk.startIdx).toBe(97);
    expect(hunk.endIdx).toBe(103);
  });
});

// ===========================================================================
// 3. WorkerPool Tests
// ===========================================================================
describe('WorkerPool', () => {
  let WorkerPool;

  beforeAll(() => {
    // Mock Worker
    global.Worker = class MockWorker {
      constructor() {
        this.onmessage = null;
        this.onerror = null;
        this.busy = false;
        this.currentId = null;
        this._terminated = false;
      }
      postMessage(data) {
        setTimeout(() => {
          if (!this._terminated && this.onmessage) {
            this.onmessage({ data: { id: data.id, result: `result-${data.id}` } });
          }
        }, 10);
      }
      terminate() {
        this._terminated = true;
      }
    };

    const classSource = extractClass('WorkerPool');
    WorkerPool = evalCode(classSource, 'WorkerPool', {
      Worker: global.Worker, Map, Promise, console, String, Error,
    });
  });

  afterAll(() => {
    delete global.Worker;
  });

  test('post returns a Promise that resolves', async () => {
    const pool = new WorkerPool('test.js', 2);
    const result = await pool.post('escapeHtml', { text: 'hi' });
    expect(result).toBe('result-0');
    pool.destroy();
  });

  test('handles worker errors', async () => {
    // Create a pool with error-producing workers
    const OrigWorker = global.Worker;
    global.Worker = class ErrorWorker {
      constructor() {
        this.onmessage = null;
        this.onerror = null;
        this.busy = false;
        this.currentId = null;
      }
      postMessage(data) {
        setTimeout(() => {
          if (this.onmessage) {
            this.onmessage({ data: { id: data.id, error: 'test error' } });
          }
        }, 10);
      }
      terminate() {}
    };

    const ErrPool = evalCode(extractClass('WorkerPool'), 'WorkerPool', {
      Worker: global.Worker, Map, Promise, console, String, Error,
    });
    const pool = new ErrPool('test.js', 1);
    await expect(pool.post('test', {})).rejects.toThrow('test error');
    pool.destroy();
    global.Worker = OrigWorker;
  });

  test('queues tasks when all workers busy', async () => {
    // Slow workers — 50ms response time
    const OrigWorker = global.Worker;
    global.Worker = class SlowWorker {
      constructor() {
        this.onmessage = null;
        this.onerror = null;
        this.busy = false;
        this.currentId = null;
      }
      postMessage(data) {
        setTimeout(() => {
          if (this.onmessage) {
            this.onmessage({ data: { id: data.id, result: `slow-${data.id}` } });
          }
        }, 50);
      }
      terminate() {}
    };

    const SlowPool = evalCode(extractClass('WorkerPool'), 'WorkerPool', {
      Worker: global.Worker, Map, Promise, console, String, Error,
    });
    const pool = new SlowPool('test.js', 2);
    const results = await Promise.all([
      pool.post('a', {}),
      pool.post('b', {}),
      pool.post('c', {}), // this one must queue since pool size = 2
    ]);
    expect(results).toHaveLength(3);
    expect(results[0]).toBe('slow-0');
    expect(results[1]).toBe('slow-1');
    expect(results[2]).toBe('slow-2');
    pool.destroy();
    global.Worker = OrigWorker;
  });

  test('destroy terminates workers', () => {
    const pool = new WorkerPool('test.js', 2);
    pool.destroy();
    expect(pool.workers).toHaveLength(0);
    expect(pool.pending.size).toBe(0);
  });

  test('offthread uses worker when available', async () => {
    const offthreadCode = extractFunction('offthread');
    // Simulate workerPool global
    const mockPost = jest.fn().mockResolvedValue('worker-result');
    const offthread = evalCode(
      `var workerPool = { post: mockPost };\n${offthreadCode}`,
      'offthread',
      { Promise, mockPost }
    );
    const result = await offthread('test', {}, () => 'fallback');
    expect(mockPost).toHaveBeenCalledWith('test', {});
    expect(result).toBe('worker-result');
  });

  test('offthread falls back on failure', async () => {
    const offthreadCode = extractFunction('offthread');
    const mockPost = jest.fn().mockRejectedValue(new Error('fail'));
    const offthread = evalCode(
      `var workerPool = { post: mockPost };\n${offthreadCode}`,
      'offthread',
      { Promise, mockPost, Error }
    );
    const result = await offthread('test', {}, () => 'fallback-value');
    expect(result).toBe('fallback-value');
  });
});

// ===========================================================================
// 4. renderJsonTreeFromRust / buildJsonNodeEl Tests
// ===========================================================================
describe('renderJsonTreeFromRust / buildJsonNodeEl', () => {
  let renderJsonTreeFromRust;

  beforeAll(() => {
    const escapeCode = extractFunction('escapeHtml');
    const buildCode = extractFunction('buildJsonNodeEl');
    const renderCode = extractFunction('renderJsonTreeFromRust');
    const combined = `${escapeCode}\n${buildCode}\n${renderCode}`;

    // Provide a mock invoke (for "load more" clicks — not triggered in these tests)
    const mockInvoke = jest.fn();

    renderJsonTreeFromRust = evalCode(combined, 'renderJsonTreeFromRust', {
      document, HTMLElement, invoke: mockInvoke,
    });
  });

  function makeNode(overrides) {
    return {
      node_type: 'string',
      key: null,
      value_preview: 'hello',
      depth: 0,
      is_last: true,
      child_count: 0,
      children: [],
      path: '',
      ...overrides,
    };
  }

  test('renders primitive string node', () => {
    const node = makeNode({ node_type: 'string', value_preview: 'world' });
    const el = renderJsonTreeFromRust(node, '{}');
    expect(el.querySelector('.json-string')).not.toBeNull();
  });

  test('renders object node with toggle', () => {
    const child = makeNode({ key: 'a', is_last: true, depth: 1 });
    const node = makeNode({
      node_type: 'object',
      child_count: 1,
      children: [child],
      value_preview: null,
    });
    const el = renderJsonTreeFromRust(node, '{}');
    expect(el.querySelector('.json-toggle')).not.toBeNull();
    expect(el.querySelector('.json-children')).not.toBeNull();
  });

  test('renders collapsed deep node', () => {
    const node = makeNode({
      node_type: 'object',
      child_count: 5,
      children: [],
      value_preview: null,
    });
    const el = renderJsonTreeFromRust(node, '{}');
    const toggle = el.querySelector('.json-toggle');
    expect(toggle).not.toBeNull();
    expect(toggle.classList.contains('collapsed')).toBe(true);
  });

  test('load more button appears when truncated', () => {
    const kids = Array.from({ length: 100 }, (_, i) =>
      makeNode({ key: `k${i}`, depth: 1, is_last: i === 99 })
    );
    const node = makeNode({
      node_type: 'object',
      child_count: 200,
      children: kids,
      value_preview: null,
    });
    const el = renderJsonTreeFromRust(node, '{}');
    const moreBtn = el.querySelector('.json-more-btn');
    expect(moreBtn).not.toBeNull();
    expect(moreBtn.textContent).toContain('100 more');
  });

  test('is_last affects comma', () => {
    const lastNode = makeNode({ node_type: 'number', value_preview: '42', is_last: true });
    const nonLastNode = makeNode({ node_type: 'number', value_preview: '42', is_last: false });
    const elLast = renderJsonTreeFromRust(lastNode, '{}');
    const elNonLast = renderJsonTreeFromRust(nonLastNode, '{}');
    // Last node: no trailing comma
    expect(elLast.textContent).not.toMatch(/42,/);
    // Non-last node: has trailing comma
    expect(elNonLast.textContent).toMatch(/42,/);
  });
});

// ===========================================================================
// 5. renderRustCharHighlight Tests
// ===========================================================================
describe('renderRustCharHighlight', () => {
  let renderRustCharHighlight;

  beforeAll(() => {
    const escapeCode = extractFunction('escapeHtml');
    const highlightCode = extractFunction('renderRustCharHighlight');
    renderRustCharHighlight = evalCode(
      `${escapeCode}\n${highlightCode}`,
      'renderRustCharHighlight'
    );
  });

  test('returns highlighted text with no highlights', () => {
    const result = renderRustCharHighlight('hello', [], 'hl', (t) => t);
    expect(result).toBe('hello');
  });

  test('wraps highlighted spans', () => {
    const result = renderRustCharHighlight('abcdef', [{ start: 2, end: 4 }], 'hl', (t) => t);
    expect(result).toContain('<span class="hl">cd</span>');
    expect(result).toContain('ab');
    expect(result).toContain('ef');
  });

  test('handles null highlights', () => {
    const result = renderRustCharHighlight('hello', null, 'hl', (t) => t);
    expect(result).toBe('hello');
  });
});

// ===========================================================================
// 6. worker.js Pure Function Tests
// ===========================================================================
describe('worker.js functions', () => {
  let escapeHtmlWorker, sortKeys, jsonStringifySorted, formatBodyWorker, searchTextWorker;

  beforeAll(() => {
    // Evaluate worker.js functions in an isolated context (no self.onmessage)
    const workerSource = fs.readFileSync(WORKER_JS, 'utf-8');
    // Remove the self.onmessage handler to avoid errors in non-worker context
    const cleanedSource = workerSource.replace(/self\.onmessage\s*=[\s\S]*$/, '');
    const wrapped = `(function() {
      ${cleanedSource}
      return { escapeHtmlWorker, sortKeys, jsonStringifySorted, formatBodyWorker, searchTextWorker };
    })()`;
    const script = new vm.Script(wrapped, { filename: 'worker-test.js' });
    const result = script.runInNewContext({ JSON, Object, Array, console, RegExp, String, Error });
    escapeHtmlWorker = result.escapeHtmlWorker;
    sortKeys = result.sortKeys;
    jsonStringifySorted = result.jsonStringifySorted;
    formatBodyWorker = result.formatBodyWorker;
    searchTextWorker = result.searchTextWorker;
  });

  test('escapeHtml escapes all entities', () => {
    const input = '&<>"\'';
    const output = escapeHtmlWorker(input);
    expect(output).toBe('&amp;&lt;&gt;&quot;&#039;');
  });

  test('escapeHtml returns empty string for non-string', () => {
    expect(escapeHtmlWorker(null)).toBe('');
    expect(escapeHtmlWorker(42)).toBe('');
  });

  test('sortKeys sorts recursively', () => {
    const input = { z: 1, a: { m: 2, b: 3 } };
    const sorted = sortKeys(input);
    expect(Object.keys(sorted)).toEqual(['a', 'z']);
    expect(Object.keys(sorted.a)).toEqual(['b', 'm']);
  });

  test('jsonStringifySorted produces sorted output', () => {
    const result = jsonStringifySorted({ c: 1, a: 2, b: 3 }, 0);
    const parsed = JSON.parse(result);
    expect(Object.keys(parsed)).toEqual(['a', 'b', 'c']);
  });

  test('formatBody splits JSON into lines', () => {
    const body = '{"b":1,"a":2}';
    const lines = formatBodyWorker(body, 'application/json');
    expect(lines.length).toBeGreaterThan(1);
    // Should be sorted and pretty-printed
    const joined = lines.join('\n');
    expect(joined).toContain('"a"');
    expect(joined).toContain('"b"');
  });

  test('formatBody handles invalid JSON', () => {
    const body = 'not{json';
    const lines = formatBodyWorker(body, 'application/json');
    expect(lines).toEqual(['not{json']);
  });

  test('formatBody handles null/empty body', () => {
    expect(formatBodyWorker(null, 'text/plain')).toEqual([]);
    expect(formatBodyWorker('', 'text/plain')).toEqual([]);
  });

  test('searchText finds matches with positions', () => {
    const text = 'line one\nline two\nfind me here\nanother line';
    const results = searchTextWorker(text, 'find', 100);
    expect(results).toHaveLength(1);
    expect(results[0].lineNumber).toBe(3);
    expect(results[0].matchStart).toBe(0);
    expect(results[0].matchEnd).toBe(4);
    expect(results[0].lineText).toBe('find me here');
  });

  test('searchText is case insensitive', () => {
    const text = 'Hello World\nhello again';
    const results = searchTextWorker(text, 'HELLO', 100);
    expect(results).toHaveLength(2);
  });

  test('searchText returns empty for no matches', () => {
    const results = searchTextWorker('abc', 'xyz', 100);
    expect(results).toEqual([]);
  });
});
