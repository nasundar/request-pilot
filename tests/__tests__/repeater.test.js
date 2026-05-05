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
      setActiveResponseStepIdx(v) { activeResponseStepIdx = v; },
      setActiveFileIndex(v) { activeFileIndex = v; },
      setActiveBlockIndex(v) { activeBlockIndex = v; },
      getActiveFileIndex() { return activeFileIndex; },
      getActiveBlockIndex() { return activeBlockIndex; },
      hasLoopDirective(block) { return hasLoopDirective(block); },
      getBlockIterations(br) { return getBlockIterations(br); },
      renderFileTree() { return renderFileTree(); },
      populateMetadata(block) { return populateMetadata(block); },
      applyMetadataToBlock() { return applyMetadataToBlock(); },
      iterFailureCount(br) { return iterFailureCount(br); },
      formatIterLabel(v) { return formatIterLabel(v); },
      findAdjacentFailedIter(iters, fromIdx, dir) { return findAdjacentFailedIter(iters, fromIdx, dir); },
      renderAssertions(fi, bi) { return renderAssertions(fi, bi); },
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

function mkInnerBlockResult(iterIdx, status, body, assertionResults) {
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
    assertion_results: assertionResults || [],
    extract_results: [],
    step_results: [],
    diff_result: null,
    diff_results: [],
    iterations: [],
  };
}

/**
 * Build a 20-iteration loaded-file fixture with N failures spread out.
 * Used for tests that exercise the failure-introspection UX:
 *   - pass/fail badge math
 *   - show-failed-only filter
 *   - keyboard nav next/prev failure
 *   - sidebar overflow handling at >10 iters
 *   - per-iter assertion-tab routing
 *
 * The block has two assertions defined; each iter's block_result carries
 * the per-iter results so the assertion tab can be tested per-iter.
 */
function makeBigLoopLoadedFile(failedIndexes = [3, 7, 11, 15, 19]) {
  const failedSet = new Set(failedIndexes);
  const iterations = [];
  for (let i = 0; i < 20; i += 1) {
    const isFail = failedSet.has(i);
    const status = isFail ? 'failed' : 'passed';
    const ar = [
      { passed: !isFail, actual: isFail ? '500' : '200' },
      { passed: true, actual: 'ok' },
    ];
    iterations.push({
      index: i, iter_value: `u${i + 1}`, status,
      block_result: mkInnerBlockResult(i, status, `body for u${i + 1}`, ar),
      body_omitted: false,
    });
  }
  return {
    name: 'big-loop.http', path: '/tmp/big-loop.http', content: '',
    suite: {
      variables: [],
      blocks: [
        {
          block_type: 'test', name: 'BigLoop', description: 'twenty iters',
          compare: false, steps: [],
          request: { method: 'GET', url: '/users/{{u}}', headers: [], body: null },
          assertions: [
            { left: 'status', operator: '==', right: '200' },
            { left: '$.status', operator: '==', right: 'ok' },
          ],
          extracts: [],
          diff: null, diffs: [],
          disabled: false, mode: null, dev_auth: null,
          group: null, depends: [],
          for_loop: { iter_var: 'u', source_var: 'us' },
        },
      ],
    },
    results: {
      passed: 0, failed: failedIndexes.length, skipped: 0, total_time_ms: 100,
      block_results: [
        {
          seq: 0, name: 'BigLoop', block_type: 'test', group: null,
          request_method: 'GET', request_url: '/users/{{u}}',
          request_headers: [], request_body: null,
          status: 'failed', response: null, error: null, time_ms: 100,
          assertion_results: [], extract_results: [],
          step_results: [],
          diff_result: null, diff_results: [],
          iterations,
        },
      ],
      final_variables: {},
    },
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

  // ============================================================
  // V1.1 — failure introspection polish tests
  // ============================================================

  test('iterFailureCount returns total/passed/failed/omitted breakdown', async () => {
    const app = bootDesktopApp();
    try {
      const file = makeBigLoopLoadedFile();
      app.bridge.setLoadedFiles([file]);
      const br = file.results.block_results[0];
      const breakdown = app.bridge.iterFailureCount(br);
      expect(breakdown).toEqual({ total: 20, passed: 15, failed: 5, omitted: 0 });
    } finally {
      await app.cleanup();
    }
  });

  test('loop badge shows "× 20 (5 ✗)" with has-failures class when iters fail', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile()]);
      app.bridge.renderFileTree();
      const badge = app.document.querySelector('.block-loop-badge');
      expect(badge).toBeTruthy();
      expect(badge.classList.contains('has-failures')).toBe(true);
      expect(badge.textContent).toMatch(/×\s*20/);
      expect(badge.textContent).toContain('5');
      expect(badge.textContent).toMatch(/✗/);
    } finally {
      await app.cleanup();
    }
  });

  test('loop badge omits failure suffix when all iters pass', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([])]);
      app.bridge.renderFileTree();
      const badge = app.document.querySelector('.block-loop-badge');
      expect(badge).toBeTruthy();
      expect(badge.classList.contains('has-failures')).toBe(false);
      expect(badge.textContent).not.toMatch(/✗/);
    } finally {
      await app.cleanup();
    }
  });

  test('"show failed only" filter chip renders when failures exist', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile()]);
      app.bridge.selectBlock(0, 0);
      const filterChip = app.document.querySelector('#responseStepTabs .iter-tab-filter');
      expect(filterChip).toBeTruthy();
      const buttons = filterChip.querySelectorAll('.iter-filter-btn');
      expect(buttons.length).toBe(2);
      expect(buttons[0].textContent).toMatch(/All\s*20/);
      expect(buttons[1].textContent).toMatch(/Failed\s*5/);
      expect(buttons[0].classList.contains('active')).toBe(true);
    } finally {
      await app.cleanup();
    }
  });

  test('"show failed only" toggle hides passed iters and jumps to first failure', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([3, 7])]);
      app.bridge.selectBlock(0, 0);

      const filterChip = app.document.querySelector('#responseStepTabs .iter-tab-filter');
      const failedBtn = filterChip.querySelectorAll('.iter-filter-btn')[1];
      failedBtn.click();

      const tabs = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      // Iter 0 (passed) should be hidden; iter 3 (failed) should be visible.
      expect(tabs[0].style.display).toBe('none');
      expect(tabs[3].style.display).not.toBe('none');
      expect(tabs[7].style.display).not.toBe('none');
      // Active iter auto-jumped to first failure (index 3).
      expect(app.bridge.getActiveResponseStepIdx()).toBe(3);
    } finally {
      await app.cleanup();
    }
  });

  test('filter chip is hidden when no iters fail', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([])]);
      app.bridge.selectBlock(0, 0);
      const filterChip = app.document.querySelector('#responseStepTabs .iter-tab-filter');
      expect(filterChip).toBeFalsy();
    } finally {
      await app.cleanup();
    }
  });

  test('findAdjacentFailedIter walks forward and wraps around', async () => {
    const app = bootDesktopApp();
    try {
      const iters = [
        { status: 'passed' },
        { status: 'failed' },
        { status: 'passed' },
        { status: 'failed' },
        { status: 'passed' },
      ];
      // From idx 0, next failure is idx 1.
      expect(app.bridge.findAdjacentFailedIter(iters, 0, 1)).toBe(1);
      // From idx 1, next failure is idx 3.
      expect(app.bridge.findAdjacentFailedIter(iters, 1, 1)).toBe(3);
      // From idx 3 forward wraps to idx 1.
      expect(app.bridge.findAdjacentFailedIter(iters, 3, 1)).toBe(1);
      // From idx 0 backward wraps to idx 3 (last failure).
      expect(app.bridge.findAdjacentFailedIter(iters, 0, -1)).toBe(3);
      // From idx -1 (no selection) forward jumps to first failure.
      expect(app.bridge.findAdjacentFailedIter(iters, -1, 1)).toBe(1);
    } finally {
      await app.cleanup();
    }
  });

  test('keyboard "n" jumps to next failed iteration', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([3, 7, 11, 15, 19])]);
      app.bridge.selectBlock(0, 0);
      app.bridge.setActiveResponseStepIdx(0);

      const ev = new app.window.KeyboardEvent('keydown', {
        key: 'n', bubbles: true, cancelable: true,
      });
      app.document.dispatchEvent(ev);

      expect(app.bridge.getActiveResponseStepIdx()).toBe(3);
    } finally {
      await app.cleanup();
    }
  });

  test('keyboard "Shift+N" jumps to previous failed iteration', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([3, 7, 11, 15, 19])]);
      app.bridge.selectBlock(0, 0);
      app.bridge.setActiveResponseStepIdx(15);

      const ev = new app.window.KeyboardEvent('keydown', {
        key: 'N', shiftKey: true, bubbles: true, cancelable: true,
      });
      app.document.dispatchEvent(ev);

      expect(app.bridge.getActiveResponseStepIdx()).toBe(11);
    } finally {
      await app.cleanup();
    }
  });

  test('keyboard "n" is ignored when typing in an input field', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([3])]);
      app.bridge.selectBlock(0, 0);
      app.bridge.setActiveResponseStepIdx(0);

      // Inject an input element and focus it; keydown should not trigger nav.
      const input = app.document.createElement('input');
      input.type = 'text';
      app.document.body.appendChild(input);
      input.focus();

      const ev = new app.window.KeyboardEvent('keydown', {
        key: 'n', bubbles: true, cancelable: true,
      });
      input.dispatchEvent(ev);

      expect(app.bridge.getActiveResponseStepIdx()).toBe(0);
    } finally {
      await app.cleanup();
    }
  });

  test('formatIterLabel: scalars, objects, preferred keys, fallback', async () => {
    const app = bootDesktopApp();
    try {
      const fmt = app.bridge.formatIterLabel;
      // Scalars (strings — that's how the runner serializes scalar iter_values).
      expect(fmt('u_abc')).toBe('u_abc');
      expect(fmt('42')).toBe('42');
      // Object inputs arrive as JSON-stringified text from the runner.
      expect(fmt('{"id":"u_abc","email":"foo@x.com"}')).toContain('u_abc');
      // Object without id but with `name`
      expect(fmt('{"name":"alice","age":30}')).toContain('alice');
      // Object with no preferred keys — falls back to first scalar field
      expect(fmt('{"foo":"bar"}')).toContain('bar');
      // null / undefined / non-string non-object — should produce a non-empty
      // fallback string, not crash.
      expect(typeof fmt(null)).toBe('string');
      expect(typeof fmt(undefined)).toBe('string');
      expect(typeof fmt(42)).toBe('string');
    } finally {
      await app.cleanup();
    }
  });

  test('iter breadcrumb badge renders when focusing a loop iteration', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([3])]);
      app.bridge.selectBlock(0, 0);
      app.bridge.showResponseForBlock(0, 0, 3);

      const badge = app.document.getElementById('responseIterBadge');
      expect(badge).toBeTruthy();
      expect(badge.classList.contains('hidden')).toBe(false);
      // Should mention the iter number (4 of 20) and the iter value.
      expect(badge.textContent).toMatch(/4\s*\/\s*20/);
      expect(badge.textContent).toContain('u4');
    } finally {
      await app.cleanup();
    }
  });

  test('iter breadcrumb is hidden for non-loop blocks', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeLoopLoadedFile()]);
      // Block index 1 is the plain non-loop block in this fixture.
      app.bridge.selectBlock(0, 1);
      const badge = app.document.getElementById('responseIterBadge');
      expect(badge.classList.contains('hidden')).toBe(true);
    } finally {
      await app.cleanup();
    }
  });

  test('sidebar shows "+N more" link when iters > 10 and hides overflow rows', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([3, 7])]);
      app.bridge.renderFileTree();

      // The sidebar block must be expanded so the iters are in the DOM.
      const blockItem = app.document.querySelector('.block-item');
      blockItem.click();

      const moreLink = app.document.querySelector('.loop-iterations-more');
      expect(moreLink).toBeTruthy();
      expect(moreLink.textContent).toMatch(/\+\s*\d+\s*more/);

      // Overflow class should be on (passed-only) rows beyond the first 5.
      const overflow = app.document.querySelectorAll('.loop-iter-overflow');
      expect(overflow.length).toBeGreaterThan(0);

      // Clicking expands.
      moreLink.click();
      const container = app.document.querySelector('.loop-iterations');
      expect(container.classList.contains('expanded')).toBe(true);
    } finally {
      await app.cleanup();
    }
  });

  test('sidebar does NOT render "+N more" when iters <= 10', async () => {
    const app = bootDesktopApp();
    try {
      // makeLoopLoadedFile has only 3 iters — well under the threshold.
      app.bridge.setLoadedFiles([makeLoopLoadedFile()]);
      app.bridge.renderFileTree();
      const blockItem = app.document.querySelector('.block-item');
      blockItem.click();

      const moreLink = app.document.querySelector('.loop-iterations-more');
      expect(moreLink).toBeFalsy();
    } finally {
      await app.cleanup();
    }
  });

  test('renderAssertions for a focused loop iter routes to per-iter results', async () => {
    const app = bootDesktopApp();
    try {
      const file = makeBigLoopLoadedFile([3, 7]);
      app.bridge.setLoadedFiles([file]);
      app.bridge.selectBlock(0, 0);

      // Focus iter 3 (failed) — assertion tab should show that iter's results,
      // not the parent block's empty array.
      app.bridge.showResponseForBlock(0, 0, 3);
      app.bridge.renderAssertions(0, 0);

      const list = app.document.getElementById('assertionsContent');
      expect(list).toBeTruthy();
      // The fixture's iter-3 first assertion fails with actual=500.
      expect(list.textContent).toContain('500');

      // Focus iter 0 (passed) — same assertion row but now passing.
      app.bridge.showResponseForBlock(0, 0, 0);
      app.bridge.renderAssertions(0, 0);
      const list2 = app.document.getElementById('assertionsContent');
      expect(list2.textContent).toContain('200');
    } finally {
      await app.cleanup();
    }
  });

  // --- Regression tests against rubber-duck-identified blind spots ---

  test('failed-only filter is cleared if rerun produces all-pass result', async () => {
    const app = bootDesktopApp();
    try {
      // First run: 3 failures.
      const file = makeBigLoopLoadedFile([3, 7, 11]);
      app.bridge.setLoadedFiles([file]);
      app.bridge.selectBlock(0, 0);

      // Activate Failed-only filter.
      const failedBtn = app.document
        .querySelector('#responseStepTabs .iter-tab-filter')
        .querySelectorAll('.iter-filter-btn')[1];
      failedBtn.click();

      // Simulate a rerun that fixes all failures.
      file.results.block_results[0].iterations.forEach(it => {
        it.status = 'passed';
        it.block_result.status = 'passed';
        it.block_result.assertion_results.forEach(ar => {
          ar.passed = true;
          ar.actual = '200';
        });
      });
      file.results.block_results[0].status = 'passed';
      // Re-select the block to trigger a fresh tab render.
      app.bridge.selectBlock(0, 0);

      // The chip should be gone (no failures). All tabs should be visible.
      const chip = app.document.querySelector('#responseStepTabs .iter-tab-filter');
      expect(chip).toBeFalsy();
      const tabs = app.document.querySelectorAll('#responseStepTabs .response-step-tab');
      expect(tabs.length).toBe(20);
      tabs.forEach(t => {
        expect(t.style.display).not.toBe('none');
      });
    } finally {
      await app.cleanup();
    }
  });

  test('updateBlockStatuses refreshes loop badge + sub-list after a rerun', async () => {
    const app = bootDesktopApp();
    try {
      // Initial run: all-pass.
      const file = makeBigLoopLoadedFile([]);
      app.bridge.setLoadedFiles([file]);
      app.bridge.renderFileTree();

      let badge = app.document.querySelector('.block-loop-badge');
      expect(badge.textContent).toMatch(/×\s*20/);
      expect(badge.classList.contains('has-failures')).toBe(false);

      // Mutate results to simulate a rerun with 3 new failures, then call
      // updateBlockStatuses() (NOT renderFileTree()) — the same call path
      // the runner uses after a real run.
      file.results.block_results[0].iterations.forEach((it, i) => {
        if ([2, 5, 9].includes(i)) {
          it.status = 'failed';
          it.block_result.status = 'failed';
          it.block_result.assertion_results[0].passed = false;
          it.block_result.assertion_results[0].actual = '500';
        }
      });
      file.results.block_results[0].status = 'failed';
      // Find updateBlockStatuses on the bridge or call via renderFileTree replacement.
      // We don't expose updateBlockStatuses on the bridge — so this test
      // verifies the sync-in-place helpers (mounted on the bridge) work.
      // Equivalent to what updateBlockStatuses does for loop blocks.
      const blockItem = app.document.querySelector('.block-item');
      // Use the helper directly since it's part of the bridge surface
      // exposed through window in the page itself. This mirrors what
      // updateBlockStatuses does internally.
      app.window.eval(`
        const item = document.querySelector('.block-item');
        syncSidebarLoopBadge(item, 0, 0);
        syncSidebarLoopIterList(item, 0, 0);
      `);

      badge = app.document.querySelector('.block-loop-badge');
      expect(badge.textContent).toContain('20');
      expect(badge.textContent).toContain('3');
      expect(badge.textContent).toMatch(/✗/);
      expect(badge.classList.contains('has-failures')).toBe(true);

      // Sub-list should have 3 failed dots and the rest passed.
      const failedDots = app.document.querySelectorAll('.loop-iteration-item .block-status.status-failed');
      expect(failedDots.length).toBe(3);
    } finally {
      await app.cleanup();
    }
  });

  test('keyboard nav is ignored when focused on a <select>', async () => {
    const app = bootDesktopApp();
    try {
      app.bridge.setLoadedFiles([makeBigLoopLoadedFile([3])]);
      app.bridge.selectBlock(0, 0);
      app.bridge.setActiveResponseStepIdx(0);

      const sel = app.document.createElement('select');
      sel.innerHTML = '<option>a</option><option>b</option>';
      app.document.body.appendChild(sel);
      sel.focus();

      const ev = new app.window.KeyboardEvent('keydown', {
        key: 'n', bubbles: true, cancelable: true,
      });
      sel.dispatchEvent(ev);

      expect(app.bridge.getActiveResponseStepIdx()).toBe(0);
    } finally {
      await app.cleanup();
    }
  });
});
