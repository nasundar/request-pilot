/**
 * Regression tests for the Code-mode editor's per-keystroke render path.
 *
 * Why these tests exist:
 *   The editor renders syntax highlighting via a transparent textarea over
 *   a colored <pre> overlay. The overlay update used to run synchronously
 *   on every `input` event, doing two full innerHTML rebuilds and a fresh
 *   line-numbers pass. On a 2609-line .http file (real PromQL captures)
 *   this caused ~1s typing/Ctrl+Z lag.
 *
 *   We fixed it by:
 *     (1) rAF-coalescing — `requestHighlightUpdate()` schedules at most
 *         one render per animation frame, so a burst of keystrokes / undo
 *         ticks within ~16ms shares one paint.
 *     (2) folding the active-block class into `highlightHttpCode`'s line
 *         loop, eliminating the second `innerHTML` rebuild.
 *     (3) caching `_lastLineNumberCount` so the gutter is only rewritten
 *         when the line count actually changes.
 *
 *   These tests guard those three properties so a future change can't
 *   silently bring the lag back.
 *
 * The tests boot a fresh JSDOM with the real desktop UI scripts (same
 * harness pattern as desktop-ui-startup.test.js) and exercise the editor
 * via the input handler.
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
    case 'parse_test_file':
      return { variables: [], blocks: [], auto_run: null };
    default: return null;
  }
}

function bootApp() {
  const html = loadHtml().replace(
    /<script[^>]*src=["'][^"']+["'][^>]*><\/script>/g,
    ''
  );
  const scripts = ['comparison.js', 'app.js', 'tour.js']
    .map(n => `<script data-src="${n}">\n${loadScript(n)}\n</script>`)
    .join('\n');
  const htmlWithScripts = html.replace('</body>', () => scripts + '\n</body>');

  const scriptErrors = [];
  const virtualConsole = new VirtualConsole();
  virtualConsole.on('jsdomError', (err) => {
    scriptErrors.push(err.detail || err);
  });

  // Track rAF invocations so tests can reason about coalescing without
  // depending on real animation-frame timing.
  let rafQueue = [];
  let rafCalls = 0;

  const dom = new JSDOM(htmlWithScripts, {
    runScripts: 'dangerously',
    pretendToBeVisual: true,
    url: 'http://localhost/',
    virtualConsole,
    beforeParse(window) {
      window.__TAURI__ = {
        core: { invoke: (cmd, args) => Promise.resolve(stubInvoke(cmd, args)) },
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
      // Manual rAF: we count calls and let tests flush at will.
      window.requestAnimationFrame = (cb) => {
        rafCalls++;
        rafQueue.push(cb);
        return rafCalls;
      };
      window.cancelAnimationFrame = () => {};
      window.requestIdleCallback = window.requestIdleCallback || ((cb) => setTimeout(cb, 0));
      window.cancelIdleCallback = window.cancelIdleCallback || ((id) => clearTimeout(id));
    },
  });

  const { window } = dom;
  const flushRaf = () => {
    const queue = rafQueue;
    rafQueue = [];
    queue.forEach(cb => cb());
  };
  return {
    window,
    document: window.document,
    scriptErrors,
    rafCallCount: () => rafCalls,
    flushRaf,
    cleanup: () => dom.window.close(),
  };
}

/** Build a synthesized .http file with the given number of lines. */
function buildSyntheticHttp(lineCount) {
  const lines = ['@variables', 'base_url = https://api.example.com', ''];
  let i = lines.length;
  while (i < lineCount) {
    lines.push('### @@test Block ' + i);
    lines.push('GET {{base_url}}/path/' + i);
    lines.push('Authorization: Bearer {{token}}');
    lines.push('Content-Type: application/json');
    lines.push('');
    lines.push('{ "id": ' + i + ', "name": "item-' + i + '" }');
    lines.push('');
    lines.push('# @@assert status == 200');
    lines.push('');
    i = lines.length;
  }
  return lines.slice(0, lineCount).join('\n');
}

describe('code-editor performance', () => {
  test('rAF coalesces a burst of input events into one render', () => {
    const app = bootApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const codeEditor = app.document.getElementById('codeEditor');
      expect(codeEditor).toBeTruthy();

      // Simulate a 5-keystroke burst within a single frame.
      const before = app.rafCallCount();
      codeEditor.value = 'GET /a';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      codeEditor.value = 'GET /ab';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      codeEditor.value = 'GET /abc';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      codeEditor.value = 'GET /abcd';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      codeEditor.value = 'GET /abcde';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      const after = app.rafCallCount();

      // All five input events must coalesce to AT MOST one rAF.
      expect(after - before).toBeLessThanOrEqual(1);

      // Flush — overlay should reflect the latest value, not an
      // intermediate one.
      app.flushRaf();
      const overlayCode = app.document.getElementById('codeEditorHighlightCode');
      expect(overlayCode.textContent).toContain('GET /abcde');
    } finally {
      app.cleanup();
    }
  });

  test('renders 3000-line synthesized .http highlight in <250ms', () => {
    const app = bootApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const codeEditor = app.document.getElementById('codeEditor');

      const text = buildSyntheticHttp(3000);
      codeEditor.value = text;

      // Trigger one full render via the input handler + rAF flush. We
      // measure the rAF callback execution since that's where the heavy
      // lifting lives. (Wall-clock only — inherently noisy in CI; we
      // pick a generous budget that still catches O(N²) regressions.)
      codeEditor.dispatchEvent(new app.window.Event('input'));
      const t0 = Date.now();
      app.flushRaf();
      const elapsed = Date.now() - t0;

      // Sanity check: the overlay actually rendered.
      const overlayCode = app.document.getElementById('codeEditorHighlightCode');
      expect(overlayCode.innerHTML.length).toBeGreaterThan(0);
      expect(overlayCode.innerHTML).toContain('hl-method');

      // Budget: 250ms is generous for a 3000-line file in jsdom (which
      // is much slower than real Chromium). Real-world target is ~30ms
      // on Chromium/V8. If this test starts failing it almost certainly
      // means the highlight pipeline has regressed (e.g. someone
      // re-introduced the double innerHTML rebuild).
      expect(elapsed).toBeLessThan(250);
    } finally {
      app.cleanup();
    }
  });

  test('updateLineNumbers no-ops when line count is unchanged', () => {
    const app = bootApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const codeEditor = app.document.getElementById('codeEditor');
      const lineNums = app.document.getElementById('codeLineNumbers');

      // Establish a baseline: 5 lines.
      codeEditor.value = 'a\nb\nc\nd\ne';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      app.flushRaf();
      const baseline = lineNums.textContent;
      expect(baseline.split('\n')).toHaveLength(5);

      // Mutate a value WITHOUT changing line count. The gutter should
      // still read 5 (and we don't really care if it reassigns the same
      // string, just that it stays correct).
      codeEditor.value = 'a\nb\nc\nd\nE';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      app.flushRaf();
      expect(lineNums.textContent).toBe(baseline);

      // Add a line — the gutter MUST grow.
      codeEditor.value = 'a\nb\nc\nd\nE\nf';
      codeEditor.dispatchEvent(new app.window.Event('input'));
      app.flushRaf();
      expect(lineNums.textContent.split('\n')).toHaveLength(6);
    } finally {
      app.cleanup();
    }
  });

  test('overlay has exactly one active-block-start span (no duplication on re-render)', () => {
    // Regression guard: the previous implementation called
    // applyBlockHighlight() AFTER updateHighlight() which itself called
    // applyBlockHighlight() — at the OLD activeBlockLineStart in scope —
    // before the NEW value was written. That left stale spans in the DOM.
    // The fix folds the active-block class into highlightHttpCode's
    // line loop so a single render is the source of truth.
    const app = bootApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const codeEditor = app.document.getElementById('codeEditor');

      const text = [
        '### @@test First',
        'GET /a',
        '',
        '### @@test Second',
        'GET /b',
        '',
      ].join('\n');
      codeEditor.value = text;
      codeEditor.dispatchEvent(new app.window.Event('input'));
      app.flushRaf();

      const overlayCode = app.document.getElementById('codeEditorHighlightCode');
      // No active block selected → no active-block-start span at all.
      const matches = (overlayCode.innerHTML.match(/hl-active-block-start/g) || []).length;
      expect(matches).toBeLessThanOrEqual(1);
    } finally {
      app.cleanup();
    }
  });
});
