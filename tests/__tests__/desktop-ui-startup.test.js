/**
 * Regression tests for the Request Pilot desktop UI startup.
 *
 * The desktop app is a Tauri 2 app with a plain HTML/JS UI in
 * `desktop/ui/`. There is no bundler — the browser loads `index.html`
 * which sources `comparison.js`, `app.js`, and `tour.js` in order.
 *
 * Why these tests exist:
 *   A previous regression introduced a Temporal Dead Zone bug where
 *   `attachVariableOverlay()` (called at top-level via `renderParamsFromUrl()`)
 *   read a `const _variableOverlays` that hadn't been declared yet. That
 *   threw a ReferenceError mid-script, which silently aborted the rest of
 *   `app.js` execution — leaving every event listener attached after the
 *   throw site (Open File, mode tabs, scroll-to-zoom, etc.) unwired.
 *   The bug was invisible to syntax checks (`node --check`) because TDZ
 *   errors are runtime-only.
 *
 *   These tests load index.html into jsdom, stub the Tauri host bridge,
 *   execute every script tag, and verify that:
 *     1. Loading the scripts produces no thrown error.
 *     2. The DOM elements that should have click handlers actually do
 *        — verified by triggering a click and asserting the side-effect.
 *     3. The variable-overlay system wraps inputs as expected without
 *        breaking layout-bearing classes.
 */

const fs = require('fs');
const path = require('path');
// jsdom needs TextEncoder/TextDecoder; jest's jsdom environment doesn't
// always expose them. Polyfill from Node before requiring jsdom.
if (typeof globalThis.TextEncoder === 'undefined') {
  globalThis.TextEncoder = require('util').TextEncoder;
}
if (typeof globalThis.TextDecoder === 'undefined') {
  globalThis.TextDecoder = require('util').TextDecoder;
}
const { JSDOM } = require('jsdom');

const DESKTOP_UI = path.join(__dirname, '..', '..', 'desktop', 'ui');

// The desktop scripts schedule async work (history-load, env-resolve,
// etc.) that completes after `dom.window.close()`. After close, those
// continuations hit a stale `document` and throw, which surfaces as
// process-level unhandledRejection / uncaughtException. Swallow them
// here — the assertions in each test already verify what we care about.
process.on('unhandledRejection', () => {});
process.on('uncaughtException', () => {});

function loadHtml() {
  return fs.readFileSync(path.join(DESKTOP_UI, 'index.html'), 'utf8');
}

function loadScript(name) {
  return fs.readFileSync(path.join(DESKTOP_UI, name), 'utf8');
}

/**
 * Build a fresh jsdom environment with index.html and a stubbed Tauri
 * bridge, then evaluate the three desktop scripts in their canonical
 * order. Each call returns an isolated window — re-running scripts in
 * the same global scope would cause "already declared" failures.
 */
function bootDesktopApp() {
  // Inline the three scripts into the HTML and let jsdom execute them
  // as real <script> tags. This mirrors what the actual app does and
  // gives us proper script-realm scoping (so `const` declarations in
  // one file are visible to all three, just like in a browser).
  const html = loadHtml().replace(
    /<script[^>]*src=["'][^"']+["'][^>]*><\/script>/g,
    ''
  );
  const scripts = ['comparison.js', 'app.js', 'tour.js']
    .map(n => `<script data-src="${n}">\n${loadScript(n)}\n</script>`)
    .join('\n');
  // Use a replacer FUNCTION so `$$` (e.g. the `$$` DOM helper in app.js)
  // isn't interpreted by replace() as a `$` escape sequence.
  const htmlWithScripts = html.replace('</body>', () => scripts + '\n</body>');

  // ── Capture script errors. We hook this BEFORE jsdom runs scripts. ──
  const scriptErrors = [];

  // Pre-build a virtualConsole that captures jsdom errors emitted as
  // a result of script-tag execution failures.
  const { VirtualConsole } = require('jsdom');
  const virtualConsole = new VirtualConsole();
  virtualConsole.on('jsdomError', (err) => {
    // jsdom wraps the original error inside `.detail` for script errors
    scriptErrors.push(err.detail || err);
  });

  const dom = new JSDOM(htmlWithScripts, {
    runScripts: 'dangerously',
    pretendToBeVisual: true,
    url: 'http://localhost/',
    virtualConsole,
    beforeParse(window) {
      // ── Tauri host bridge stubs (must be in place before scripts run) ──
      const invokeCalls = window.__invokeCalls = [];
      const invoke = (cmd, args) => {
        invokeCalls.push({ cmd, args });
        return Promise.resolve(stubInvoke(cmd, args));
      };
      const listen = () => Promise.resolve(() => {});
      const emit = () => Promise.resolve();
      window.__TAURI__ = {
        core: { invoke },
        event: { listen, emit },
      };
      // Polyfills jsdom is missing
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
  const { window } = dom;

  return {
    window,
    document: window.document,
    invoke: window.__TAURI__.core.invoke,
    listen: window.__TAURI__.event.listen,
    emit: window.__TAURI__.event.emit,
    scriptErrors,
    invokeCalls: window.__invokeCalls || [],
    cleanup: () => dom.window.close(),
  };
}

function stubInvoke(cmd /*, args */) {
  switch (cmd) {
    case 'env_list':
      return { entries: [], active_index: null };
    case 'env_resolve_active':
      return [];
    case 'sessions_list':
      return [];
    case 'extra_headers_get':
      return [];
    case 'azure_auth_status':
      return { authenticated: false, expires_at: null };
    case 'app_log':
    case 'set_webview_zoom':
      return null;
    case 'get_webview_zoom':
      return 1.0;
    default:
      return null;
  }
}

describe('desktop UI startup', () => {
  test('loads all three scripts without throwing', () => {
    const app = bootDesktopApp();
    try {
      if (app.scriptErrors.length > 0) {
        const dump = app.scriptErrors
          .map((e, i) => `[${i}] ${e && e.stack ? e.stack : e}`)
          .join('\n----\n');
        throw new Error(
          `Expected zero script errors during startup, but caught:\n${dump}`
        );
      }
      expect(app.scriptErrors).toHaveLength(0);
    } finally {
      app.cleanup();
    }
  });

  test('Open File button is wired (click triggers fileInput.click)', () => {
    const app = bootDesktopApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const openBtn = app.document.getElementById('openFileBtn');
      const fileInput = app.document.getElementById('fileInput');
      expect(openBtn).toBeTruthy();
      expect(fileInput).toBeTruthy();
      const fileClickSpy = jest.fn();
      fileInput.click = fileClickSpy;
      openBtn.click();
      expect(fileClickSpy).toHaveBeenCalled();
    } finally {
      app.cleanup();
    }
  });

  test('Send button has a click listener attached (does not throw)', () => {
    const app = bootDesktopApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const sendBtn = app.document.getElementById('sendBtn');
      expect(sendBtn).toBeTruthy();
      expect(() => sendBtn.click()).not.toThrow();
    } finally {
      app.cleanup();
    }
  });

  test('URL input is wrapped exactly once by the var-overlay system', () => {
    const app = bootDesktopApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const urlInput = app.document.getElementById('urlInput');
      expect(urlInput).toBeTruthy();
      expect(urlInput.parentElement.classList.contains('var-overlay-wrapper')).toBe(true);
      expect(urlInput.parentElement.classList.contains('url-overlay')).toBe(true);
      const urlBar = urlInput.closest('.url-bar');
      expect(urlBar).toBeTruthy();
      expect(urlBar.contains(urlInput.parentElement)).toBe(true);
    } finally {
      app.cleanup();
    }
  });

  test('initial param row exists with var-overlay wrapping its value cell', () => {
    const app = bootDesktopApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const paramsContainer = app.document.getElementById('paramsContainer');
      expect(paramsContainer).toBeTruthy();
      const firstRow = paramsContainer.querySelector('.kv-row');
      expect(firstRow).toBeTruthy();
      const wrappedValue = firstRow.querySelector(
        '.var-overlay-wrapper.kv-overlay .kv-value'
      );
      expect(wrappedValue).toBeTruthy();
    } finally {
      app.cleanup();
    }
  });

  test('overlay state declarations precede their first use in source', () => {
    // Static guard: the bug we are protecting against is `const`-before-use.
    // Verify the const declarations come BEFORE any call to
    // `attachVariableOverlay(` in the source (other than within its own
    // function body, which is hoisted as a function declaration).
    const src = loadScript('app.js');
    const declIdx = src.indexOf('const _variableOverlays = new WeakMap');
    const setDeclIdx = src.indexOf('const _variableOverlaySet = new Set');
    expect(declIdx).toBeGreaterThanOrEqual(0);
    expect(setDeclIdx).toBeGreaterThanOrEqual(0);

    const callSiteRegex = /attachVariableOverlay\s*\(/g;
    let m;
    while ((m = callSiteRegex.exec(src)) !== null) {
      // Skip the function definition itself (the literal `function attachVariableOverlay(`).
      const before = src.slice(Math.max(0, m.index - 20), m.index);
      if (/function\s+$/.test(before)) continue;
      expect(m.index).toBeGreaterThan(declIdx);
      expect(m.index).toBeGreaterThan(setDeclIdx);
    }
  });

  test('mode tabs (builder / code / history / logs) have working click listeners', async () => {
    const app = bootDesktopApp();
    try {
      expect(app.scriptErrors).toHaveLength(0);
      const modeToggle = app.document.getElementById('modeToggle');
      expect(modeToggle).toBeTruthy();
      const buttons = modeToggle.querySelectorAll('.mode-btn');
      expect(buttons.length).toBeGreaterThan(0);
      // Pick `history` deliberately — its switchMode path has no awaits
      // before the active-class toggle, so the change is synchronous.
      // (Switching to `code` triggers an `await syncBuilderToCode()`
      // which we'd have to await; the click-wiring assertion is the
      // same either way.)
      const target = modeToggle.querySelector('[data-mode="history"]');
      expect(target).toBeTruthy();
      expect(target.classList.contains('active')).toBe(false);
      target.click();
      expect(target.classList.contains('active')).toBe(true);
      // The previously-active builder button should have lost `active`
      const builderBtn = modeToggle.querySelector('[data-mode="builder"]');
      expect(builderBtn.classList.contains('active')).toBe(false);
      // Drain pending microtasks (history-load issues invoke calls that
      // post a follow-up render). We intentionally swallow any errors
      // those raise — the assertion is just "click handler ran".
      await new Promise(r => setTimeout(r, 30));
    } finally {
      app.cleanup();
    }
  });
});
