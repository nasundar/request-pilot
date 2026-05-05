/**
 * Tests for the `# @@for` Repeater UI: response-panel iteration tabs,
 * sidebar `× N` badge, per-iter sub-list, and the metadata-accordion
 * loop directive round-trip.
 *
 * Why these tests exist:
 *   The Rust runner produces `BlockResult.iterations[]` for any block
 *   that has a `# @@for iter_var in source_var` directive. The JS layer
 *   must:
 *     1. Render one tab per iteration in the response panel (reusing
 *        the same `#responseStepTabs` element previously used for
 *        @@compare steps).
 *     2. Show the selected iteration's response (or `(body omitted)`
 *        notice for trimmed iters).
 *     3. Show a `× N` badge in the sidebar tree on the parent block.
 *     4. Round-trip `block.for_loop` through the metadata-accordion
 *        inputs (`metaLoopIterVar` / `metaLoopSourceVar`) — typing
 *        into both inputs should set `block.for_loop`; clearing
 *        either should clear it back to `null`.
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
    case 'generate_http': return '';
    default: return null;
  }
}

function bootDesktopApp() {
  const html = loadHtml().replace(
    /<script[^>]*src=["'][^"']+["'][^>]*><\/script>/g,
    ''
  );
  const scripts = ['comparison.js', 'app.js', 'tour.js']
    .map(n => `<script data-src="${n}">\n${loadScript(n)}\n</script>`)
    .join('\n');

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
      hasLoopDirective(block) { return hasLoopDirective(block); },
      getBlockIterations(br) { return getBlockIterations(br); },
      renderFileTree() { return renderFileTree(); },
      populateMetadata(block) { return populateMetadata(block); },
      applyMetadataToBlock() { return applyMetadataToBlock(); },
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
      await new Promise((r) => setTimeout(r, 0));
      await new Promise((r) => setTimeout(r, 0));
      dom.window.close();
    },
  };
}

function mkResp(status, statusText, body, timeMs) {
  return {
    status,
    status_text: statusText,
    headers: [['Content-Type', 'text/plain']],
    body,
    time_ms: timeMs,
    size_bytes: body.length,
  };
}

function mkInnerBlockResult(iterIdx, status, body) {
  return {
    seq: null,
    name: 'LoopBlock',
    block_type: 'test',
    group: null,
    request_method: 'GET',
    request_url: `/users/u${iterIdx + 1}`,
    request_headers: [],
    request_body: null,
    status,
    response: status === 'error' ? null : mkResp(200, 'OK', body, 5),
    error: status === 'error' ? `iter ${iterIdx} err` : null,
    time_ms: 5,
    assertion_results: [], extract_results: [],
    step_results: [],
    diff_result: null,
    diff_results: [],
    iterations: [],
  };
}

/**
 * Synthetic loadedFile with one looped block that produced 3 iterations
 * (two passed + one failed) plus a sibling non-loop block for control.
 */
function makeLoopLoadedFile() {
  return {
    name: 'loop-test.http',
    path: '/tmp/loop-test.http',
    content: '',
    suite: {
      variables: [],
      blocks: [
        {
          block_type: 'test',
          name: 'LoopBlock',
          description: 'Iterate over user_ids',
          compare: false,
          steps: [],
          request: { method: 'GET', url: '/users/{{user_id}}', headers: [], body: null },
          assertions: [],
          extracts: [],
          diff: null,
          diffs: [],
          disabled: false,
          mode: null,
          dev_auth: null,
          group: null,
          depends: [],
          for_loop: { iter_var: 'user_id', source_var: 'user_ids' },
        },
        {
          block_type: 'test',
          name: 'PlainGet',
          description: 'Single non-loop request',
          compare: false,
          steps: [],
          request: { method: 'GET', url: '/single', headers: [], body: null },
          assertions: [],
          extracts: [],
          diff: null,
          diffs: [],
          disabled: false,
          mode: null,
          dev_auth: null,
          group: null,
          depends: [],
          for_loop: null,
        },
      ],
    },
    results: {
      passed: 0, failed: 1, skipped: 0, total_time_ms: 50,
      block_results: [
        {
          seq: 0, name: 'LoopBlock', block_type: 'test', group: null,
          request_method: 'GET', request_url: '/users/{{user_id}}',
          request_headers: [], request_body: null,
          status: 'failed', response: null, error: null, time_ms: 50,
          assertion_results: [], extract_results: [],
          step_results: [],
          diff_result: null, diff_results: [],
          iterations: [
            {
              index: 0, iter_value: 'u1', status: 'passed',
              block_result: mkInnerBlockResult(0, 'passed', 'body for u1'),
              body_omitted: false,
            },
            {
              index: 1, iter_value: 'u2', status: 'failed',
              block_result: mkInnerBlockResult(1, 'failed', 'body for u2'),
              body_omitted: false,
            },
            {
              index: 2, iter_value: 'u3', status: 'passed',
              block_result: mkInnerBlockResult(2, 'passed', 'body for u3'),
              body_omitted: true, // simulate storage cap trim on a middle success
            },
          ],
        },
        {
          seq: 1, name: 'PlainGet', block_type: 'test', group: null,
          request_method: 'GET', request_url: '/single',
          request_headers: [], request_body: null,
          status: 'passed',
          response: mkResp(418, "I'm a teapot", 'plain', 7),
          error: null, time_ms: 7,
          assertion_results: [], extract_results: [],
          step_results: [], diff_result: null, diff_results: [],
          iterations: [],
        },
      ],
      final_variables: {},
    },
  };
}

describe('repeater V1 — # @@for UI', () => {
  test('hasLoopDirective + getBlockIterations helpers behave', async () => {
    const app = bootDesktopApp();
    try {
      const file = makeLoopLoadedFile();
      const block = file.suite.blocks[0];
      const plain = file.suite.blocks[1];
      expect(app.bridge.hasLoopDirective(block)).toBe(true);
      expect(app.bridge.hasLoopDirective(plain)).toBe(false);
      expect(app.bridge.hasLoopDirective({ for_loop: { iter_var: '', source_var: 'x' } })).toBe(false);
      expect(app.bridge.hasLoopDirective(null)).toBe(false);

      expect(app.bridge.getBlockIterations(file.results.block_results[0]).length).toBe(3);
      expect(app.bridge.getBlockIterations(file.results.block_results[1]).length).toBe(0);
      expect(app.bridge.getBlockIterations(null)).toEqual([]);
    } finally {
      await app.cleanup();
    }
  });

  test('response-step tabs render one button per iteration', async () => {
    const app = bootDesktopApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      app.bridge.setLoadedFiles([makeLoopLoadedFile()]);
      app.bridge.selectBlock(0, 0);

      const tabs = app.document.getElementById('responseStepTabs');
      expect(tabs).toBeTruthy();
      expect(tabs.classList.contains('hidden')).toBe(false);

      const buttons = tabs.querySelectorAll('.response-step-tab');
      expect(buttons.length).toBe(3);
      expect(buttons[0].textContent).toContain('u1');
      expect(buttons[1].textContent).toContain('u2');
      expect(buttons[2].textContent).toContain('u3');

      // First iter is active by default; status dots reflect status.
      expect(buttons[0].classList.contains('active')).toBe(true);
      expect(buttons[0].querySelector('.step-status.passed')).toBeTruthy();
      expect(buttons[1].querySelector('.step-status.failed')).toBeTruthy();
      expect(buttons[2].querySelector('.step-status.passed')).toBeTruthy();

      // The status badge reflects iter 0 (u1 -> 200 OK).
      const badge = app.document.getElementById('responseStatusBadge');
      expect(badge.textContent).toMatch(/^200 /);
    } finally {
      await app.cleanup();
    }
  });

  test('clicking an iteration tab swaps the displayed response', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeLoopLoadedFile()]);
      app.bridge.selectBlock(0, 0);

      const buttons = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      // Click iter 1 (u2 — failed). displayError() path runs.
      buttons[1].click();
      expect(app.bridge.getActiveResponseStepIdx()).toBe(1);

      // Iter 2 (u3) was body_omitted — clicking it shows the error/notice.
      const after = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      after[2].click();
      expect(app.bridge.getActiveResponseStepIdx()).toBe(2);

      // Click back to iter 0 — should restore the 200 OK body view.
      after[0].click();
      const badge = app.document.getElementById('responseStatusBadge');
      expect(badge.textContent).toMatch(/^200 /);
      expect(app.bridge.getActiveResponseStepIdx()).toBe(0);
    } finally {
      await app.cleanup();
    }
  });

  test('hides iteration-tabs strip for non-loop blocks', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeLoopLoadedFile()]);
      app.bridge.selectBlock(0, 1); // PlainGet

      const tabs = app.document.getElementById('responseStepTabs');
      expect(tabs.classList.contains('hidden')).toBe(true);
      expect(tabs.querySelectorAll('.response-step-tab').length).toBe(0);

      expect(app.document.getElementById('responseStatusBadge').textContent).toMatch(/^418 /);
    } finally {
      await app.cleanup();
    }
  });

  test('sidebar shows × N badge on looped block, none on plain', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeLoopLoadedFile()]);
      app.bridge.renderFileTree();

      const allBlockItems = app.document.querySelectorAll('.block-item');
      expect(allBlockItems.length).toBeGreaterThanOrEqual(2);

      // Find LoopBlock by its name
      let loopItem = null, plainItem = null;
      allBlockItems.forEach(el => {
        const n = el.querySelector('.block-name')?.textContent || '';
        if (n === 'LoopBlock') loopItem = el;
        if (n === 'PlainGet') plainItem = el;
      });
      expect(loopItem).toBeTruthy();
      expect(plainItem).toBeTruthy();

      const loopBadge = loopItem.querySelector('.block-loop-badge');
      expect(loopBadge).toBeTruthy();
      expect(loopBadge.textContent).toContain('3'); // 3 iterations
      expect(plainItem.querySelector('.block-loop-badge')).toBeFalsy();
    } finally {
      await app.cleanup();
    }
  });

  test('sidebar renders one sub-item per iteration with status', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeLoopLoadedFile()]);
      app.bridge.renderFileTree();

      const iterItems = app.document.querySelectorAll('.loop-iteration-item');
      expect(iterItems.length).toBe(3);
      expect(iterItems[0].querySelector('.status-passed')).toBeTruthy();
      expect(iterItems[1].querySelector('.status-failed')).toBeTruthy();
      expect(iterItems[2].querySelector('.status-passed')).toBeTruthy();
      // Iter 2 was body_omitted; the omitted-flag indicator should render.
      expect(iterItems[2].querySelector('.loop-omitted-flag')).toBeTruthy();
      expect(iterItems[0].querySelector('.loop-omitted-flag')).toBeFalsy();
    } finally {
      await app.cleanup();
    }
  });

  test('metadata-accordion loop inputs round-trip block.for_loop', async () => {
    const app = bootDesktopApp();
    try {
      const file = makeLoopLoadedFile();
      app.bridge.setLoadedFiles([file]);
      app.bridge.setActiveFileIndex(0);
      app.bridge.setActiveBlockIndex(0);

      // populate from block -> inputs
      const block = file.suite.blocks[0];
      app.bridge.populateMetadata(block);
      const iterInput = app.document.getElementById('metaLoopIterVar');
      const srcInput = app.document.getElementById('metaLoopSourceVar');
      expect(iterInput.value).toBe('user_id');
      expect(srcInput.value).toBe('user_ids');

      // Edit inputs -> apply -> block.for_loop reflects the edit.
      iterInput.value = 'pid';
      srcInput.value = 'project_ids';
      const changed = app.bridge.applyMetadataToBlock();
      expect(changed).toBe(true);
      expect(block.for_loop).toEqual({ iter_var: 'pid', source_var: 'project_ids' });

      // Clearing one of the inputs clears the directive entirely.
      iterInput.value = '';
      app.bridge.applyMetadataToBlock();
      expect(block.for_loop).toBeNull();

      // Re-typing both restores it.
      iterInput.value = 'x';
      srcInput.value = 'xs';
      app.bridge.applyMetadataToBlock();
      expect(block.for_loop).toEqual({ iter_var: 'x', source_var: 'xs' });
    } finally {
      await app.cleanup();
    }
  });

  test('compare-mode cleared when toggled on a loop block (parser conflict)', async () => {
    const app = bootDesktopApp();
    try {
      const file = makeLoopLoadedFile();
      app.bridge.setLoadedFiles([file]);
      app.bridge.setActiveFileIndex(0);
      app.bridge.setActiveBlockIndex(0);

      const block = file.suite.blocks[0];
      app.bridge.populateMetadata(block);

      // Simulate user toggling Compare mode ON via the metadata accordion.
      const compareCheckbox = app.document.getElementById('metaCompare');
      compareCheckbox.checked = true;
      app.bridge.applyMetadataToBlock();
      // for_loop should be cleared because @@compare + @@for is parser-rejected.
      expect(block.compare).toBe(true);
      expect(block.for_loop).toBeNull();
    } finally {
      await app.cleanup();
    }
  });
});
