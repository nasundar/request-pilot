/**
 * Regression tests for the compare-block response panel step tabs.
 *
 * Why these tests exist:
 *   `@@compare` blocks have one response per step (baseline / canary / etc.)
 *   stored in `block_results[i].step_results[j].response`. Before this
 *   change the response panel only ever rendered `step_results[0].response`
 *   (in `selectBlock`) — and the post-run paths in `runAllTests`,
 *   `runSingleBlock`, `runCompareStep` only updated the panel when
 *   `br.response` existed (which is `null` for compare blocks). Net
 *   effect: users could never see step 1+ responses, and re-running a
 *   compare block left the response panel showing whatever was there
 *   before the run.
 *
 *   The fix routes all five response-render entry points through a new
 *   `showResponseForBlock(fileIdx, blockIdx, preferredStepIdx)` central
 *   helper that:
 *     - For compare blocks: renders a `<div id="responseStepTabs">` strip
 *       above the existing Body/Headers/Assertions sub-tabs and shows
 *       the chosen step's response.
 *     - For non-compare blocks: hides the step strip and shows
 *       `br.response` directly.
 *
 *   These tests boot the desktop UI in jsdom, seed `loadedFiles` with a
 *   synthetic compare block that has results for multiple steps, and
 *   verify the panel renders the right tab as active, swaps content on
 *   click, hides the strip for non-compare blocks, and resets the
 *   selected step when switching blocks.
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

/**
 * Boot the desktop UI in jsdom AND inject a small "test bootstrap"
 * script after the main scripts. Top-level `let` declarations in
 * classic <script> tags share the global lexical environment within
 * the same realm, so the bootstrap can read/write `loadedFiles`,
 * `activeResponseStepIdx`, `selectBlock`, etc., even though they are
 * not exposed on `window`.
 */
function bootDesktopApp() {
  const html = loadHtml().replace(
    /<script[^>]*src=["'][^"']+["'][^>]*><\/script>/g,
    ''
  );
  const scripts = ['comparison.js', 'app.js', 'tour.js']
    .map(n => `<script data-src="${n}">\n${loadScript(n)}\n</script>`)
    .join('\n');

  // Test-only bridge so the test can drive internal state.
  const bridge = `<script>
    window.__rpTest = {
      setLoadedFiles(v) { loadedFiles = v; },
      getLoadedFiles() { return loadedFiles; },
      selectBlock(fi, bi) { return selectBlock(fi, bi); },
      showResponseForBlock(fi, bi, si) { return showResponseForBlock(fi, bi, si); },
      getActiveResponseStepIdx() { return activeResponseStepIdx; },
      setActiveFileIndex(v) { activeFileIndex = v; },
      setActiveBlockIndex(v) { activeBlockIndex = v; },
      getActiveFileIndex() { return activeFileIndex; },
      getActiveBlockIndex() { return activeBlockIndex; },
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
    /**
     * Yield a tick before tearing down the window so async response-render
     * microtasks (which call `await invoke(...)`) finish before document
     * goes away. Otherwise the test still passes its assertions but the
     * trailing microtask crashes with "document is undefined", and node
     * surfaces it as an unhandledRejection that fails the test runner.
     */
    cleanup: async () => {
      // Allow async response-render microtasks to complete before tearing
      // down the window. setTimeout(0) gives the macrotask queue a turn,
      // which drains pending await chains. Two ticks in case a chained
      // promise schedules another microtask. (jsdom's `setImmediate` is
      // not exposed to the Node test realm in jest's jsdom environment.)
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
    // Pretty-printing pipeline. Body rendering is async; if the stubs
    // return `null` the desktop code falls through to a catch branch
    // that calls `document.createElement` — which crashes if the test
    // has already torn down the jsdom window. Return safe shapes so
    // the success branches complete before cleanup.
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
 * Build a synthetic loadedFiles entry with one compare block that has
 * three steps and three step results. Each step has distinct status,
 * time, and body content so the tests can tell which one is on screen.
 */
function makeCompareLoadedFile() {
  const mkResp = (status, statusText, body, timeMs) => ({
    status,
    status_text: statusText,
    headers: [['Content-Type', 'text/plain']],
    body,
    time_ms: timeMs,
    size_bytes: body.length,
  });
  return {
    name: 'compare-test.http',
    path: '/tmp/compare-test.http',
    content: '',
    suite: {
      variables: [],
      blocks: [
        {
          block_type: 'test',
          name: 'CompareQueries',
          description: 'Compare across regions',
          compare: true,
          steps: [
            { name: 'baseline', request: { method: 'GET', url: '/v1/q', headers: [], body: null }, assertions: [], extracts: [] },
            { name: 'canary',   request: { method: 'GET', url: '/v2/q', headers: [], body: null }, assertions: [], extracts: [] },
            { name: 'eastus',   request: { method: 'GET', url: '/v3/q', headers: [], body: null }, assertions: [], extracts: [] },
          ],
          assertions: [],
          extracts: [],
          diff: null,
          disabled: false,
          mode: null,
          dev_auth_scope: null,
          group: null,
          depends: [],
        },
        {
          block_type: 'test',
          name: 'PlainGet',
          description: 'Single non-compare request',
          compare: false,
          steps: [],
          request: { method: 'GET', url: '/single', headers: [], body: null },
          assertions: [],
          extracts: [],
          diff: null,
          disabled: false,
          mode: null,
          dev_auth_scope: null,
          group: null,
          depends: [],
        },
      ],
    },
    results: {
      passed: 1, failed: 0, skipped: 0, total_time_ms: 999,
      block_results: [
        {
          seq: 0, name: 'CompareQueries', block_type: 'test', group: null,
          request_method: '', request_url: '', request_headers: [], request_body: null,
          status: 'passed', response: null, error: null, time_ms: 999,
          assertion_results: [], extract_results: [],
          step_results: [
            {
              name: 'baseline',
              request_method: 'GET', request_url: '/v1/q',
              request_headers: [], request_body: null,
              response: mkResp(200, 'OK', 'baseline body', 11),
              assertion_results: [], extract_results: [],
              time_ms: 11, error: null,
            },
            {
              name: 'canary',
              request_method: 'GET', request_url: '/v2/q',
              request_headers: [], request_body: null,
              response: mkResp(201, 'Created', 'canary body', 22),
              assertion_results: [], extract_results: [],
              time_ms: 22, error: null,
            },
            {
              name: 'eastus',
              request_method: 'GET', request_url: '/v3/q',
              request_headers: [], request_body: null,
              response: mkResp(404, 'Not Found', 'eastus body', 33),
              assertion_results: [], extract_results: [],
              time_ms: 33, error: null,
            },
          ],
          diff_result: null,
        },
        {
          seq: 1, name: 'PlainGet', block_type: 'test', group: null,
          request_method: 'GET', request_url: '/single',
          request_headers: [], request_body: null,
          status: 'passed',
          response: {
            status: 418, status_text: "I'm a teapot",
            headers: [['Content-Type', 'text/plain']],
            body: 'plain', time_ms: 7, size_bytes: 5,
          },
          error: null, time_ms: 7,
          assertion_results: [], extract_results: [],
          step_results: [],
          diff_result: null,
        },
      ],
      final_variables: {},
    },
  };
}

describe('compare-block response panel step tabs', () => {
  test('renders a step tab per step for compare blocks', async () => {
    const app = bootDesktopApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      app.bridge.setLoadedFiles([makeCompareLoadedFile()]);
      app.bridge.selectBlock(0, 0);

      const tabs = app.document.getElementById('responseStepTabs');
      expect(tabs).toBeTruthy();
      expect(tabs.classList.contains('hidden')).toBe(false);

      const buttons = tabs.querySelectorAll('.response-step-tab');
      expect(buttons.length).toBe(3);
      expect(buttons[0].textContent).toContain('baseline');
      expect(buttons[1].textContent).toContain('canary');
      expect(buttons[2].textContent).toContain('eastus');

      // First tab is active by default; the others are not.
      expect(buttons[0].classList.contains('active')).toBe(true);
      expect(buttons[1].classList.contains('active')).toBe(false);
      expect(buttons[2].classList.contains('active')).toBe(false);

      // The status badge reflects step 0 (baseline -> 200 OK).
      const badge = app.document.getElementById('responseStatusBadge');
      expect(badge.textContent).toMatch(/^200 /);
      expect(app.document.getElementById('responseTime').textContent).toContain('11');
    } finally {
      await app.cleanup();
    }
  });

  test('clicking a step tab swaps the displayed response', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeCompareLoadedFile()]);
      app.bridge.selectBlock(0, 0);

      const buttons = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      expect(buttons.length).toBe(3);

      // Click "canary" (step 1).
      buttons[1].click();

      const badge = app.document.getElementById('responseStatusBadge');
      expect(badge.textContent).toMatch(/^201 /);
      expect(app.document.getElementById('responseTime').textContent).toContain('22');

      // After click, only step 1 is marked active.
      const after = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      expect(after[0].classList.contains('active')).toBe(false);
      expect(after[1].classList.contains('active')).toBe(true);
      expect(after[2].classList.contains('active')).toBe(false);

      // Internal state matches the click.
      expect(app.bridge.getActiveResponseStepIdx()).toBe(1);

      // Click "eastus" (step 2).
      after[2].click();
      expect(app.document.getElementById('responseStatusBadge').textContent).toMatch(/^404 /);
      expect(app.document.getElementById('responseTime').textContent).toContain('33');
      expect(app.bridge.getActiveResponseStepIdx()).toBe(2);
    } finally {
      await app.cleanup();
    }
  });

  test('hides the step-tabs strip for non-compare blocks', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeCompareLoadedFile()]);
      app.bridge.selectBlock(0, 1); // PlainGet — not a compare block

      const tabs = app.document.getElementById('responseStepTabs');
      expect(tabs.classList.contains('hidden')).toBe(true);
      expect(tabs.querySelectorAll('.response-step-tab').length).toBe(0);

      // Status badge reflects the single br.response (418).
      expect(app.document.getElementById('responseStatusBadge').textContent).toMatch(/^418 /);
    } finally {
      await app.cleanup();
    }
  });

  test('switching active block resets activeResponseStepIdx to step 0', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeCompareLoadedFile()]);
      app.bridge.selectBlock(0, 0);

      // Move to step 2 (eastus).
      const buttons = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      buttons[2].click();
      expect(app.bridge.getActiveResponseStepIdx()).toBe(2);

      // Switch to a different block (the non-compare one) and back.
      app.bridge.selectBlock(0, 1);
      app.bridge.selectBlock(0, 0);

      // Should be back on step 0, not lingering on step 2.
      expect(app.bridge.getActiveResponseStepIdx()).toBe(0);
      const after = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      expect(after[0].classList.contains('active')).toBe(true);
      expect(after[2].classList.contains('active')).toBe(false);
      // And the displayed response is step 0 (baseline -> 200).
      expect(app.document.getElementById('responseStatusBadge').textContent).toMatch(/^200 /);
    } finally {
      await app.cleanup();
    }
  });

  test('showResponseForBlock honors a preferredStepIdx argument', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeCompareLoadedFile()]);
      app.bridge.selectBlock(0, 0);

      // Caller asks for step 2 (eastus) explicitly — used by viewCompareStep
      // and by runCompareStep after running an individual step.
      app.bridge.showResponseForBlock(0, 0, 2);

      expect(app.bridge.getActiveResponseStepIdx()).toBe(2);
      expect(app.document.getElementById('responseStatusBadge').textContent).toMatch(/^404 /);
      const buttons = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      expect(buttons[2].classList.contains('active')).toBe(true);
    } finally {
      await app.cleanup();
    }
  });

  test('clamps an out-of-range preferredStepIdx to step 0', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeCompareLoadedFile()]);
      app.bridge.selectBlock(0, 0);

      // Out-of-range index: only 3 steps exist.
      app.bridge.showResponseForBlock(0, 0, 99);

      expect(app.bridge.getActiveResponseStepIdx()).toBe(0);
      expect(app.document.getElementById('responseStatusBadge').textContent).toMatch(/^200 /);
    } finally {
      await app.cleanup();
    }
  });
});
