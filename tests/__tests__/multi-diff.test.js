/**
 * Regression tests for multi-diff compare blocks.
 *
 * Why these tests exist:
 *   A `@@compare` block can declare any number of `# @@diff <a> <b>`
 *   directives. Each directive becomes one `NamedDiffResult` entry in
 *   the runner's output and one "View Full Diff" button in the
 *   sidebar. Before this change the desktop UI only ever rendered the
 *   first pair (the legacy `block.diff` / `br.diff_result` fields),
 *   collapsing v1↔v2 and v1↔v3 into a single button.
 *
 *   The fix adds two helpers — `getBlockDiffs(block)` and
 *   `getDiffResults(br, block)` — that prefer the new multi-pair
 *   `block.diffs` / `br.diff_results` fields and fall back to wrapping
 *   the legacy single fields when running against an old `.http` file
 *   or pre-multi-diff history record.
 *
 *   These tests boot the desktop UI in jsdom, seed a synthetic compare
 *   block with two diff pairs, and verify:
 *     - `renderAssertions` emits one "Comparison Result" section per
 *       pair, each with its own "View Full Diff" button.
 *     - The buttons pass the right `diffIdx` to `openDiffViewer`.
 *     - `getBlockDiffs` / `getDiffResults` fallback paths work when
 *       only the legacy fields are populated.
 */

const fs = require('fs');
const path = require('path');
if (typeof globalThis.TextEncoder === 'undefined') {
  globalThis.TextEncoder = require('util').TextEncoder;
}
if (typeof globalThis.TextDecoder === 'undefined') {
  globalThis.TextDecoder = require('util').TextDecoder;
}
const { JSDOM, VirtualConsole } = require('jsdom');

const DESKTOP_UI = path.join(__dirname, '..', '..', 'desktop', 'ui');

process.on('unhandledRejection', () => {});
process.on('uncaughtException', () => {});

function loadHtml() {
  return fs.readFileSync(path.join(DESKTOP_UI, 'index.html'), 'utf8');
}
function loadScript(name) {
  return fs.readFileSync(path.join(DESKTOP_UI, name), 'utf8');
}

function bootDesktopApp() {
  const html = loadHtml().replace(
    /<script[^>]*src=["'][^"']+["'][^>]*><\/script>/g,
    ''
  );
  const scripts = ['comparison.js', 'app.js', 'tour.js']
    .map(n => `<script data-src="${n}">\n${loadScript(n)}\n</script>`)
    .join('\n');

  // Bridge so tests can drive internal state and call helpers directly.
  // Top-level `let`/`function` decls in classic <script> tags share the
  // global lexical environment but aren't on `window`, so we attach a
  // bridge object that captures references in scope.
  const bridge = `<script>
    window.__rpTest = {
      setLoadedFiles(v) { loadedFiles = v; },
      selectBlock(fi, bi) { return selectBlock(fi, bi); },
      setActiveFileIndex(v) { activeFileIndex = v; },
      setActiveBlockIndex(v) { activeBlockIndex = v; },
      getBlockDiffs(b) { return getBlockDiffs(b); },
      getDiffResults(br, b) { return getDiffResults(br, b); },
    };
  </script>`;

  const htmlWithScripts = html.replace(
    '</body>',
    () => scripts + '\n' + bridge + '\n</body>'
  );

  const scriptErrors = [];
  const virtualConsole = new VirtualConsole();
  virtualConsole.on('jsdomError', (err) => {
    scriptErrors.push(err.detail || err);
  });

  const dom = new JSDOM(htmlWithScripts, {
    runScripts: 'dangerously',
    pretendToBeVisual: true,
    url: 'http://localhost/',
    virtualConsole,
    beforeParse(window) {
      const invokeCalls = window.__invokeCalls = [];
      const invoke = (cmd, args) => {
        invokeCalls.push({ cmd, args });
        return Promise.resolve(stubInvoke(cmd, args));
      };
      window.__TAURI__ = {
        core: { invoke },
        event: { listen: () => Promise.resolve(() => {}), emit: () => Promise.resolve() },
      };
      if (!window.crypto) {
        Object.defineProperty(window, 'crypto', { value: {}, configurable: true });
      }
      if (!window.crypto.randomUUID) {
        window.crypto.randomUUID = () =>
          '00000000-0000-4000-8000-' + Math.random().toString(16).slice(2, 14).padStart(12, '0');
      }
      window.matchMedia = window.matchMedia || (() => ({
        matches: false, addEventListener() {}, removeEventListener() {},
        addListener() {}, removeListener() {},
      }));
      window.ResizeObserver = window.ResizeObserver || class {
        observe() {} unobserve() {} disconnect() {}
      };
      window.requestIdleCallback = window.requestIdleCallback || ((cb) => setTimeout(cb, 0));
      window.cancelIdleCallback = window.cancelIdleCallback || ((id) => clearTimeout(id));
    },
  });

  return {
    window: dom.window,
    document: dom.window.document,
    bridge: dom.window.__rpTest,
    scriptErrors,
    cleanup: async () => {
      // Drain async microtasks so trailing `displayResponse` chains don't
      // crash on a closed document. Same pattern as compare-response-tabs.
      await new Promise((r) => setTimeout(r, 0));
      await new Promise((r) => setTimeout(r, 0));
      dom.window.close();
    },
  };
}

function stubInvoke(cmd) {
  switch (cmd) {
    case 'env_list': return { entries: [], active_index: null };
    case 'env_resolve_active': return [];
    case 'sessions_list': return [];
    case 'extra_headers_get': return [];
    case 'azure_auth_status': return { authenticated: false, expires_at: null };
    case 'get_webview_zoom': return 1.0;
    case 'format_body': return [];
    case 'sort_and_normalize': return '';
    case 'build_json_tree':
      return { node_type: 'string', depth: 0, is_last: true, value: '', key: null, children: [] };
    case 'app_log':
    case 'set_webview_zoom':
    default: return null;
  }
}

/**
 * Build a synthetic loadedFiles entry for a compare block with TWO
 * diff directives (v1↔v2 and v1↔v3). Each pair has its own
 * `NamedDiffResult` with realistic counts so the rendered summary
 * shows distinct content per pair.
 */
function makeMultiDiffLoadedFile() {
  const mkResp = (status, body) => ({
    status,
    status_text: 'OK',
    headers: [['Content-Type', 'application/json']],
    body,
    time_ms: 10,
    size_bytes: body.length,
  });
  const mkDiff = (similarity, changedCount) => ({
    match_exact: changedCount === 0,
    similarity,
    is_json: true,
    added_count: 0,
    removed_count: 0,
    changed_count: changedCount,
    added_paths: [],
    removed_paths: [],
    changed_paths: changedCount > 0
      ? [{ path: '$.field', left: 'a', right: 'b' }]
      : [],
    ops: [],
  });
  return {
    name: 'multi-diff.http',
    path: '/tmp/multi-diff.http',
    content: '',
    suite: {
      variables: [],
      blocks: [
        {
          block_type: 'test',
          name: 'CompareV1V2V3',
          description: 'Compare three versions',
          compare: true,
          steps: [
            { name: 'v1', request: { method: 'GET', url: '/v1', headers: [], body: null }, assertions: [], extracts: [] },
            { name: 'v2', request: { method: 'GET', url: '/v2', headers: [], body: null }, assertions: [], extracts: [] },
            { name: 'v3', request: { method: 'GET', url: '/v3', headers: [], body: null }, assertions: [], extracts: [] },
          ],
          assertions: [],
          extracts: [],
          // Multi-diff: declare two pairs. The legacy `diff` field
          // mirrors `diffs[0]` so older readers still see the primary.
          diff: { step_a: 'v1', step_b: 'v2' },
          diffs: [
            { step_a: 'v1', step_b: 'v2' },
            { step_a: 'v1', step_b: 'v3' },
          ],
          disabled: false,
          mode: null,
          dev_auth_scope: null,
          group: null,
          depends: [],
        },
      ],
    },
    results: {
      passed: 1, failed: 0, skipped: 0, total_time_ms: 100,
      block_results: [
        {
          seq: 0, name: 'CompareV1V2V3', block_type: 'test', group: null,
          request_method: '', request_url: '', request_headers: [], request_body: null,
          status: 'passed', response: null, error: null, time_ms: 100,
          assertion_results: [], extract_results: [],
          step_results: [
            { name: 'v1', request_method: 'GET', request_url: '/v1', request_headers: [], request_body: null, response: mkResp(200, '{"a":1}'), assertion_results: [], extract_results: [], time_ms: 10, error: null },
            { name: 'v2', request_method: 'GET', request_url: '/v2', request_headers: [], request_body: null, response: mkResp(200, '{"a":1}'), assertion_results: [], extract_results: [], time_ms: 10, error: null },
            { name: 'v3', request_method: 'GET', request_url: '/v3', request_headers: [], request_body: null, response: mkResp(200, '{"a":2}'), assertion_results: [], extract_results: [], time_ms: 10, error: null },
          ],
          // Legacy field mirrors pair 0 for old-history compat.
          diff_result: mkDiff(1.0, 0),
          diff_results: [
            { step_a: 'v1', step_b: 'v2', diff: mkDiff(1.0, 0), error: null },
            { step_a: 'v1', step_b: 'v3', diff: mkDiff(0.5, 1), error: null },
          ],
        },
      ],
      final_variables: {},
    },
  };
}

describe('multi-diff helpers and rendering', () => {
  let env;
  beforeEach(() => {
    env = bootDesktopApp();
  });
  afterEach(async () => {
    if (env) await env.cleanup();
  });

  test('getBlockDiffs returns the multi-pair vec when present', () => {
    const block = {
      diff: { step_a: 'v1', step_b: 'v2' },
      diffs: [
        { step_a: 'v1', step_b: 'v2' },
        { step_a: 'v1', step_b: 'v3' },
      ],
    };
    const pairs = env.bridge.getBlockDiffs(block);
    expect(pairs).toHaveLength(2);
    expect(pairs[0].step_b).toBe('v2');
    expect(pairs[1].step_b).toBe('v3');
  });

  test('getBlockDiffs falls back to legacy `diff` field when `diffs` is empty', () => {
    const block = { diff: { step_a: 'v1', step_b: 'v2' }, diffs: [] };
    const pairs = env.bridge.getBlockDiffs(block);
    expect(pairs).toHaveLength(1);
    expect(pairs[0].step_a).toBe('v1');
    expect(pairs[0].step_b).toBe('v2');
  });

  test('getBlockDiffs returns [] when neither field is populated', () => {
    expect(env.bridge.getBlockDiffs({ diff: null, diffs: [] })).toEqual([]);
    expect(env.bridge.getBlockDiffs({})).toEqual([]);
    expect(env.bridge.getBlockDiffs(null)).toEqual([]);
  });

  test('getDiffResults prefers `diff_results` array over legacy `diff_result`', () => {
    const br = {
      diff_result: { match_exact: true, similarity: 1, is_json: true, added_count: 0, removed_count: 0, changed_count: 0, added_paths: [], removed_paths: [], changed_paths: [], ops: [] },
      diff_results: [
        { step_a: 'v1', step_b: 'v2', diff: { match_exact: true }, error: null },
        { step_a: 'v1', step_b: 'v3', diff: { match_exact: false }, error: null },
      ],
    };
    const pairs = env.bridge.getDiffResults(br, null);
    expect(pairs).toHaveLength(2);
    expect(pairs[1].diff.match_exact).toBe(false);
  });

  test('getDiffResults synthesizes envelope from legacy diff_result + block.diff', () => {
    // Old history record: only `diff_result` populated. The helper must
    // wrap it in a single-element envelope so callers don't crash on
    // `.step_a`.
    const br = {
      diff_result: { match_exact: true, similarity: 1 },
      diff_results: [],
    };
    const block = { diff: { step_a: 'v1', step_b: 'v2' } };
    const pairs = env.bridge.getDiffResults(br, block);
    expect(pairs).toHaveLength(1);
    expect(pairs[0].step_a).toBe('v1');
    expect(pairs[0].step_b).toBe('v2');
    expect(pairs[0].diff.match_exact).toBe(true);
  });

  test('renderAssertions renders one "View Full Diff" button per pair with correct diffIdx', async () => {
    env.bridge.setLoadedFiles([makeMultiDiffLoadedFile()]);
    env.bridge.selectBlock(0, 0);
    // Wait a tick for any async render work.
    await new Promise((r) => setTimeout(r, 0));

    const buttons = env.document.querySelectorAll('.diff-view-btn');
    expect(buttons.length).toBe(2);
    // Each button should call openDiffViewer with the right pair index.
    expect(buttons[0].getAttribute('onclick')).toContain('openDiffViewer(0, 0, 0)');
    expect(buttons[1].getAttribute('onclick')).toContain('openDiffViewer(0, 0, 1)');
    // And the labels should disambiguate the pairs.
    expect(buttons[0].textContent).toMatch(/v1/);
    expect(buttons[0].textContent).toMatch(/v2/);
    expect(buttons[1].textContent).toMatch(/v1/);
    expect(buttons[1].textContent).toMatch(/v3/);
  });

  test('renderAssertions surfaces per-pair error without dropping good pairs', async () => {
    const file = makeMultiDiffLoadedFile();
    // Replace the second pair with an error (simulates a typo'd step ref).
    file.results.block_results[0].diff_results[1] = {
      step_a: 'v1',
      step_b: 'nonexistent',
      diff: null,
      error: "@diff step not found in responses: 'nonexistent'",
    };
    env.bridge.setLoadedFiles([file]);
    env.bridge.selectBlock(0, 0);
    await new Promise((r) => setTimeout(r, 0));

    // Pair 0 still gets its View Full Diff button. Pair 1 doesn't (no
    // diff to view) but its error message must be visible somewhere on
    // the page so the user can fix the typo.
    const buttons = env.document.querySelectorAll('.diff-view-btn');
    expect(buttons.length).toBe(1);
    expect(env.document.body.textContent).toMatch(/nonexistent/);
  });
});
