/* ============================================================
   Request Pilot - Desktop App Logic
   State management, Tauri IPC, DOM manipulation
   ============================================================ */

const { invoke } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;

// --- DOM helpers ---
const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => document.querySelectorAll(sel);

// --- Global State ---
// File entry: {name, content, suite, results, savedPath: string|null}
// savedPath: null = temp/unsaved, string = persisted to disk
let loadedFiles = [];
let activeFileIndex = -1;
let activeBlockIndex = -1;
let runtimeOverrides = {};   // {name: value} — populated by @@extract / post-run final_variables. Cleared at the start of every Run All.
let disabledBlocks = {};     // {"fileIdx-blockIdx": true} — disabled steps
let isRunning = false;
let lastResponse = null;
let currentMode = 'builder';    // 'builder' | 'code' | 'history' | 'logs'
let codeEditorContent = '';     // last saved content in code editor
let codeEditorModified = false;
let historyCache = [];
let historyExpandedGroups = new Set();
let historySelectedIds = new Set();  // Selected for comparison
let _selectedHistoryHeaders = new Set();  // Headers selected in multi-select dropdown
// Tri-state viewed tracking: Maps key → { status, response_hash }
// States: unseen (not in map), seen (in map, hash matches), seen-mutated (in map, hash differs)
let viewedResults = new Map();       // "fileIdx-blockIdx" → { status, hash }
let viewedHistoryEntries = new Map(); // seq → { status, hash }
let runHistory = new Map(); // Map<fileName, [{timestamp, passed, failed, skipped, totalTime, blockResults: [{name, status, timeMs}]}]>

// Diff cache — avoids recomputing expensive Rust diffs for same inputs
// Key: "assert:<fileIdx>-<blockIdx>" or "hist:<seqA>-<seqB>" → { ops, similarity, ... }
const diffCache = new Map();
let diffCacheBytes = 0; // rough byte estimate for memory tracking

/** Invalidate all assertion diff cache entries for a file (all blocks). */
function invalidateDiffCacheForFile(fileIdx) {
  const prefix = `assert:${fileIdx}-`;
  for (const key of [...diffCache.keys()]) {
    if (key.startsWith(prefix)) {
      diffCacheBytes -= JSON.stringify(diffCache.get(key)).length * 2;
      diffCache.delete(key);
    }
  }
}

/** Invalidate a single assertion diff cache entry. */
function invalidateDiffCacheEntry(fileIdx, blockIdx) {
  const key = `assert:${fileIdx}-${blockIdx}`;
  if (diffCache.has(key)) {
    diffCacheBytes -= JSON.stringify(diffCache.get(key)).length * 2;
    diffCache.delete(key);
  }
}

// --- Logging ---
const rpLogs = [];
const MAX_LOGS = 2000;
let logFilterLevel = 'all';

function rpLog(level, message, data) {
  const entry = {
    ts: new Date().toISOString(),
    level, // 'info' | 'warn' | 'error' | 'debug'
    message,
    data: data !== undefined ? data : null
  };
  rpLogs.push(entry);
  if (rpLogs.length > MAX_LOGS) rpLogs.splice(0, rpLogs.length - MAX_LOGS);
  if (typeof renderLogEntry === 'function') renderLogEntry(entry);
  if (typeof emitToLogPopout === 'function') emitToLogPopout(entry);
}

// --- Azure Auth ---
// States: 'off' | 'needs-auth' | 'authenticated' | 'expired'
let azureAuthState = 'off';
let azCliAvailable = false;

// --- OTEL Telemetry ---
// States: 'off' | 'configured' | 'sending' | 'active' | 'error'
let telemetryState = 'off';
let telemetryStats = []; // [{file, endpoint, traces, metrics, logs, errors, time_ms}]
let telemetryEnabled = false; // user toggle — when false, skip OTEL export even if configured

// --- Live Capture ---
let liveCaptureMode = 'off';
let liveCaptureConnected = false;
let liveCapturedRequests = [];       // accumulated for current session
let liveCaptureFileIndex = -1;       // index in loadedFiles for the virtual .http file
let liveCaptureSessionId = null;

// --- Zoom ---
let zoomLevel = parseInt(localStorage.getItem('rp-zoom') || '100', 10);
const ZOOM_MIN = 50, ZOOM_MAX = 200, ZOOM_STEP = 10;

// --- Auto-Run ---
let autoRunInterval = '';        // '' = off, '5m', '15m', etc.
let autoRunTimerId = null;       // setInterval handle
let autoRunNextDue = null;       // Date when next run is due
let autoRunCountdownId = null;   // countdown display interval

function applyZoom() {
  // Use the webview's native zoom (same mechanism as Chrome's Ctrl+/-).
  // This scales rendering AND adjusts the layout viewport so 100vh always
  // equals the visible window height — content never gets clipped at the
  // bottom and scrollable inner panels still scroll all the way to their
  // end. CSS `zoom` on <html> does NOT do this correctly: it scales render
  // size but leaves the layout viewport at its original size, so at 170%
  // the bottom 70% of any 100vh-anchored layout falls outside the window.
  invoke('set_webview_zoom', { scale: zoomLevel / 100 }).catch(e => {
    rpLog && rpLog('warn', 'set_webview_zoom failed', String(e));
  });
  // Clear any stale styles from previous CSS-zoom implementation (idempotent
  // on a fresh load — these properties may have been persisted on a prior
  // version of the app).
  document.documentElement.style.zoom = '';
  document.body.style.transform = '';
  document.body.style.transformOrigin = '';
  document.body.style.width = '';
  document.body.style.height = '';
  document.body.style.zoom = '';
  const indicator = document.getElementById('zoomIndicator');
  if (indicator) indicator.textContent = `${zoomLevel}%`;
  localStorage.setItem('rp-zoom', zoomLevel.toString());
}

function zoomIn() { if (zoomLevel < ZOOM_MAX) { zoomLevel += ZOOM_STEP; applyZoom(); } }
function zoomOut() { if (zoomLevel > ZOOM_MIN) { zoomLevel -= ZOOM_STEP; applyZoom(); } }
function zoomReset() { zoomLevel = 100; applyZoom(); }

// --- Auto-Run ---
function parseDurationMs(s) {
  if (!s) return null;
  const match = s.match(/^(\d+)(s|m|h|d)$/);
  if (!match) return null;
  const n = parseInt(match[1], 10);
  const unit = match[2];
  if (n <= 0) return null;
  const multipliers = { s: 1000, m: 60000, h: 3600000, d: 86400000 };
  return n * multipliers[unit];
}

function setAutoRun(interval) {
  // Clear existing timers
  if (autoRunTimerId) { clearInterval(autoRunTimerId); autoRunTimerId = null; }
  if (autoRunCountdownId) { clearInterval(autoRunCountdownId); autoRunCountdownId = null; }
  autoRunInterval = interval || '';
  autoRunNextDue = null;

  const selectEl = document.getElementById('autoRunSelect');
  if (selectEl) selectEl.value = autoRunInterval;

  // Store original option labels for restoration
  if (selectEl && !selectEl._origLabels) {
    selectEl._origLabels = {};
    for (const opt of selectEl.options) {
      selectEl._origLabels[opt.value] = opt.textContent;
    }
  }

  // Restore original label when turning off
  if (!interval) {
    if (selectEl && selectEl._origLabels) {
      for (const opt of selectEl.options) {
        opt.textContent = selectEl._origLabels[opt.value] || opt.textContent;
      }
    }
    rpLog('info', 'Auto-run disabled');
    return;
  }

  const ms = parseDurationMs(interval);
  if (!ms) {
    rpLog('warn', 'Invalid auto-run interval', { interval });
    return;
  }

  autoRunNextDue = Date.now() + ms;
  rpLog('info', `Auto-run enabled: every ${interval}`, { ms });

  autoRunTimerId = setInterval(() => {
    if (isRunning) {
      autoRunNextDue = Date.now() + ms;
      return;
    }
    rpLog('info', 'Auto-run triggered');
    runAllTests();
    autoRunNextDue = Date.now() + ms;
  }, ms);

  // Countdown embedded in the selected option text
  autoRunCountdownId = setInterval(() => {
    if (!autoRunNextDue || !selectEl) return;
    const remaining = Math.max(0, Math.ceil((autoRunNextDue - Date.now()) / 1000));
    const selectedOpt = selectEl.options[selectEl.selectedIndex];
    if (!selectedOpt || !selectedOpt.value) return;
    let countdown;
    if (remaining >= 3600) {
      const h = Math.floor(remaining / 3600);
      const m = Math.floor((remaining % 3600) / 60);
      const s = remaining % 60;
      countdown = `${h}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`;
    } else {
      const m = Math.floor(remaining / 60);
      const s = remaining % 60;
      countdown = `${m}:${String(s).padStart(2,'0')}`;
    }
    const origLabel = (selectEl._origLabels && selectEl._origLabels[selectedOpt.value]) || `⏲ Every ${selectedOpt.value}`;
    selectedOpt.textContent = `${origLabel}  [${countdown}]`;
  }, 1000);
}

function resetAutoRunTimer() {
  // Reset timer after manual run
  if (autoRunInterval) {
    const ms = parseDurationMs(autoRunInterval);
    if (ms) autoRunNextDue = Date.now() + ms;
  }
}

function recomputeAutoRun() {
  // Recompute auto-run from remaining loaded files
  const fileInterval = loadedFiles.find(f => f.suite && f.suite.auto_run);
  if (fileInterval) {
    if (autoRunInterval !== fileInterval.suite.auto_run) {
      setAutoRun(fileInterval.suite.auto_run);
    }
  } else if (autoRunInterval) {
    // No file declares auto_run — clear it
    setAutoRun('');
  }
}

// --- Theme ---
function initTheme() {
  const saved = localStorage.getItem('rp-theme');
  if (saved) {
    document.documentElement.setAttribute('data-theme', saved);
  } else {
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    document.documentElement.setAttribute('data-theme', prefersDark ? 'dark' : 'light');
  }
  updateThemeIcon();
}

function toggleTheme() {
  document.documentElement.classList.add('theme-transition');
  const current = document.documentElement.getAttribute('data-theme');
  const next = current === 'dark' ? 'light' : 'dark';
  document.documentElement.setAttribute('data-theme', next);
  localStorage.setItem('rp-theme', next);
  updateThemeIcon();
  setTimeout(() => document.documentElement.classList.remove('theme-transition'), 350);
}

function updateThemeIcon() {
  const theme = document.documentElement.getAttribute('data-theme');
  const btn = document.getElementById('themeToggle');
  if (btn) {
    btn.querySelector('.theme-icon').textContent = theme === 'dark' ? '🌙' : '☀️';
    btn.title = theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme';
  }
}

// Apply theme before DOM renders to prevent flash
initTheme();

// Simple hash for tri-state viewed detection
function resultHash(br) {
  if (!br) return '0';
  return `${br.status}:${br.time_ms}:${br.response?.status || 0}`;
}
function historyHash(entry) {
  return `${entry.status}:${entry.response_time_ms}:${entry.method}`;
}
function getViewedState(map, key, currentHash) {
  if (!map.has(key)) return 'unseen';
  const saved = map.get(key);
  return saved.hash === currentHash ? 'seen' : 'seen-mutated';
}

function pushRunHistory(fileName, results) {
  if (!runHistory.has(fileName)) runHistory.set(fileName, []);
  const history = runHistory.get(fileName);
  history.push({
    timestamp: Date.now(),
    passed: results.passed,
    failed: results.failed,
    skipped: results.skipped,
    totalTime: results.total_time_ms,
    blockResults: results.block_results
      .filter(br => br && br.name)
      .map(br => ({ name: br.name, status: br.status, timeMs: br.time_ms })),
  });
  if (history.length > 50) history.splice(0, history.length - 50);
}

function getBlockRunHistory(fileName, blockName) {
  const history = runHistory.get(fileName) || [];
  return history.map(run => {
    const br = run.blockResults.find(b => b.name === blockName);
    return br ? { timestamp: run.timestamp, status: br.status, timeMs: br.timeMs } : null;
  }).filter(Boolean);
}

function getFileRunStats(fileName) {
  const history = runHistory.get(fileName) || [];
  return {
    totalRuns: history.length,
    history: history.map(r => ({
      timestamp: r.timestamp,
      passed: r.passed,
      failed: r.failed,
      skipped: r.skipped,
      totalTime: r.totalTime,
    })),
  };
}

// --- Worker Pool ---
class WorkerPool {
  constructor(workerUrl, poolSize = 2) {
    this.workers = [];
    this.queue = [];
    this.pending = new Map();
    this.nextId = 0;

    for (let i = 0; i < poolSize; i++) {
      const w = new Worker(workerUrl);
      w.onmessage = (e) => this._handleMessage(e.data);
      w.onerror = (e) => this._handleError(e);
      w.busy = false;
      w.currentId = null;
      this.workers.push(w);
    }
  }

  post(type, payload) {
    return new Promise((resolve, reject) => {
      const id = String(this.nextId++);
      this.pending.set(id, { resolve, reject });
      const worker = this.workers.find(w => !w.busy);
      if (worker) {
        worker.busy = true;
        worker.currentId = id;
        worker.postMessage({ type, id, payload });
      } else {
        this.queue.push({ type, id, payload });
      }
    });
  }

  _handleMessage(data) {
    const { id, result, error } = data;
    const cb = this.pending.get(id);
    if (cb) {
      this.pending.delete(id);
      if (error) cb.reject(new Error(error));
      else cb.resolve(result);
    }
    const worker = this.workers.find(w => w.currentId === id);
    if (worker) {
      worker.busy = false;
      worker.currentId = null;
      if (this.queue.length > 0) {
        const next = this.queue.shift();
        worker.busy = true;
        worker.currentId = next.id;
        worker.postMessage(next);
      }
    }
  }

  _handleError(e) {
    console.error('Worker error:', e);
  }

  destroy() {
    this.workers.forEach(w => w.terminate());
    this.workers = [];
    this.pending.forEach(cb => cb.reject(new Error('Worker pool destroyed')));
    this.pending.clear();
    this.queue = [];
  }
}

let workerPool = null;
try {
  workerPool = new WorkerPool('worker.js', 2);
} catch (e) {
  console.warn('Web Workers not available, falling back to main thread:', e);
}

async function offthread(type, payload, fallback) {
  if (workerPool) {
    try { return await workerPool.post(type, payload); }
    catch { return fallback(); }
  }
  return fallback();
}

// --- DOM Refs ---
const methodSelect    = $('#methodSelect');
const urlInput        = $('#urlInput');
const sendBtn         = $('#sendBtn');
const addHeaderBtn    = $('#addHeaderBtn');
const headersContainer= $('#headersContainer');
const addParamBtn     = $('#addParamBtn');
const paramsContainer = $('#paramsContainer');
const bodyType        = $('#bodyType');
const bodyInput       = $('#bodyInput');
const bodyHighlight   = $('#bodyHighlight');
const bodyEditorWrap  = $('#bodyEditorWrapper');
const bodyFormEditor  = $('#bodyFormEditor');
const formFieldsList  = $('#formFieldsList');
const addFormFieldBtn = $('#addFormFieldBtn');
const toggleFormRawBtn = $('#toggleFormRawBtn');
const responseEmpty   = $('#responseEmpty');
const responseContent = $('#responseContent');
const responseMeta    = $('#responseMeta');
const statusBadge     = $('#responseStatusBadge');
const responseTime    = $('#responseTime');
const responseSize    = $('#responseSize');
const responseBody    = $('#responseBody');
const responseHeadersBody = $('#responseHeadersBody');
const copyBtn         = $('#copyBtn');
const loadingOverlay  = $('#loadingOverlay');
const fileInput       = $('#fileInput');
const openFileBtn     = $('#openFileBtn');
const runAllBtn       = $('#runAllBtn');
const extraHeadersBtn = $('#extraHeadersBtn');
const extraHeadersPanel = $('#extraHeadersPanel');
const extraHeadersList = $('#extraHeadersList');
const extraHeadersAddBtn = $('#extraHeadersAddBtn');
const extraHeadersCount = $('#extraHeadersCount');
const fileTree        = $('#fileTree');
const envList         = $('#envList');
const loadEnvBtn      = $('#loadEnvBtn');
const assertionsTab   = $('#assertionsTab');
const assertionsContent = $('#assertionsContent');
const testResultsBar  = $('#testResultsBar');
const testResultsSummary = $('#testResultsSummary');
const testResultsDetails = $('#testResultsDetails');
const modeToggle      = $('#modeToggle');
const codeEditorPanel = $('#codeEditorPanel');
const codeEditor      = $('#codeEditor');
const codeEditorFilename = $('#codeEditorFilename');
const codeSaveBtn     = $('#codeSaveBtn');
const codeRevertBtn   = $('#codeRevertBtn');
const codeRunBtn      = $('#codeRunBtn');
const newFileBtn      = $('#newFileBtn');
const newTestBtn      = $('#newTestBtn');
const codeEditorHighlightCode = $('#codeEditorHighlightCode');
const codeEditorHighlight = $('#codeEditorHighlight');
const codeLineNumbers = $('#codeLineNumbers');
const splitPanels     = $('.split-panels');
const urlBar          = $('.url-bar');

// Block metadata accordion refs
const blockMetaAccordion = $('#blockMetaAccordion');
const blockMetaHeader = $('#blockMetaHeader');
const blockMetaChevron = $('#blockMetaChevron');
const blockMetaBody   = $('#blockMetaBody');
const blockMetaSummary = $('#blockMetaSummary');
const metaName        = $('#metaName');
const metaDescription = $('#metaDescription');
const metaBlockType   = $('#metaBlockType');
const metaGroup       = $('#metaGroup');
const metaDepends     = $('#metaDepends');
const metaMode        = $('#metaMode');
const metaDevAuth     = $('#metaDevAuth');
const metaDisabled    = $('#metaDisabled');
const metaCompare     = $('#metaCompare');
const builderNormal   = $('#builderNormal');
const compareStepsView = $('#compareStepsView');
const compareStepsTabs = $('#compareStepsTabs');
const compareStepsContent = $('#compareStepsContent');
const builderDirectivesContent = $('#builderDirectivesContent');

// History panel refs
const historyPanel        = $('#historyPanel');
const historyStats        = $('#historyStats');
const historyLog          = $('#historyLog');
const historyCountBadge   = $('#historyCountBadge');
const historyMethodFilter = $('#historyMethodFilter');
const historyStatusFilter = $('#historyStatusFilter');
const historySourceFilter = $('#historySourceFilter');
const histTreeFilterWrapper = $('#histTreeFilterWrapper');
const histTreeFilterBtn = $('#histTreeFilterBtn');
const histTreeFilterLabel = $('#histTreeFilterLabel');
const histTreeFilterDropdown = $('#histTreeFilterDropdown');
const histTreeFilterList = $('#histTreeFilterList');
const histTreeSelectAll = $('#histTreeSelectAll');
const histTreeClearAll = $('#histTreeClearAll');
const historyUrlSearch    = $('#historyUrlSearch');
const historyGroupBySelect= $('#historyGroupBySelect');
const historyDetailOverlay= $('#historyDetailOverlay');
const historyDetailBody   = $('#historyDetailBody');
const historyDetailTitle  = $('#historyDetailTitle');

// Logs panel refs
const logsPanel  = $('#logsPanel');
const logList    = $('#logList');

// --- Sidebar section toggle ---
$$('.sidebar-section-header').forEach(header => {
  header.addEventListener('click', (e) => {
    if (e.target.closest('.sidebar-action')) return;
    const section = header.parentElement;
    section.classList.toggle('expanded');
  });
});

// Expand files and env by default
$('#filesSection').classList.add('expanded');
$('#envSection').classList.add('expanded');

// --- Run button state helpers ---
let abortRunController = null;

function setRunning(running) {
  isRunning = running;
  const label = runAllBtn.querySelector('.btn-label');
  if (running) {
    runAllBtn.classList.remove('btn-primary');
    runAllBtn.classList.add('btn-danger');
    label.textContent = '⏹ Stop';
    runAllBtn.title = 'Stop execution';
  } else {
    runAllBtn.classList.remove('btn-danger');
    runAllBtn.classList.add('btn-primary');
    label.textContent = '▶ Run All';
    runAllBtn.title = 'Run all tests (Ctrl+Shift+Enter)';
    abortRunController = null;
  }
}

// --- Lightweight active-block highlight (avoids full DOM rebuild) ---
function updateActiveHighlight() {
  fileTree.querySelectorAll('.block-item.active').forEach(el => el.classList.remove('active'));
  if (activeFileIndex < 0 || activeBlockIndex < 0) return;
  const fileNode = fileTree.querySelector(`.file-node[data-file-idx="${activeFileIndex}"]`);
  if (!fileNode) return;
  // Auto-expand file node if collapsed
  if (!fileNode.classList.contains('expanded')) {
    fileNode.classList.add('expanded');
    treeExpandState[`file-${activeFileIndex}`] = true;
  }
  const item = fileNode.querySelector(`.block-item[data-block-idx="${activeBlockIndex}"]`);
  if (item) {
    item.classList.add('active');
    // Also expand parent group node if needed
    const parentGroup = item.closest('.group-node');
    if (parentGroup && !parentGroup.classList.contains('expanded')) {
      parentGroup.classList.add('expanded');
      const expandKey = parentGroup.dataset.expandKey;
      if (expandKey) treeExpandState[expandKey] = true;
    }
  }
}

// --- Update status dots without full rebuild ---
function updateBlockStatuses() {
  loadedFiles.forEach((file, fileIdx) => {
    const fileNode = fileTree.querySelector(`.file-node[data-file-idx="${fileIdx}"]`);
    if (!fileNode) return;
    file.suite.blocks.forEach((_, blockIdx) => {
      const item = fileNode.querySelector(`.block-item[data-block-idx="${blockIdx}"]`);
      if (!item) return;
      const dot = item.querySelector('.block-status');
      if (!dot) return;
      const status = getBlockStatus(fileIdx, blockIdx);
      dot.className = `block-status ${status ? 'status-' + status : ''}`;
    });
    // Update group summary dots
    fileNode.querySelectorAll('.group-node').forEach(gn => {
      const groupBlocks = gn.querySelectorAll('.block-item');
      let allPassed = true, anyFailed = false, anyRunning = false;
      groupBlocks.forEach(bi => {
        const dot = bi.querySelector('.block-status');
        if (!dot) return;
        if (dot.classList.contains('status-failed')) anyFailed = true;
        else if (dot.classList.contains('status-running')) anyRunning = true;
        else if (!dot.classList.contains('status-passed')) allPassed = false;
      });
      const gDot = gn.querySelector('.group-status');
      if (gDot) {
        gDot.className = 'group-status';
        if (anyFailed) gDot.classList.add('status-failed');
        else if (anyRunning) gDot.classList.add('status-running');
        else if (allPassed && groupBlocks.length > 0) gDot.classList.add('status-passed');
      }
    });
  });
}

// --- Sidebar resize ---
{
  const handle = $('#sidebarResizeHandle');
  const sidebar = $('#sidebar');
  let resizing = false;

  handle.addEventListener('mousedown', (e) => {
    resizing = true;
    handle.classList.add('active');
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    e.preventDefault();
  });

  document.addEventListener('mousemove', (e) => {
    if (!resizing) return;
    const newWidth = Math.max(200, Math.min(400, e.clientX));
    sidebar.style.width = newWidth + 'px';
  });

  document.addEventListener('mouseup', () => {
    if (resizing) {
      resizing = false;
      handle.classList.remove('active');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    }
  });
}

// --- Panel resize (request/response split) ---
{
  const handle = $('#panelResizeHandle');
  let resizing = false;

  handle.addEventListener('mousedown', (e) => {
    resizing = true;
    handle.classList.add('active');
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    e.preventDefault();
  });

  document.addEventListener('mousemove', (e) => {
    if (!resizing) return;
    const splitPanels = $('.split-panels');
    const rect = splitPanels.getBoundingClientRect();
    const percent = ((e.clientX - rect.left) / rect.width) * 100;
    const clamped = Math.max(25, Math.min(75, percent));
    $('.request-panel').style.flex = `0 0 ${clamped}%`;
    $('.response-panel').style.flex = `0 0 ${100 - clamped}%`;
  });

  document.addEventListener('mouseup', () => {
    if (resizing) {
      resizing = false;
      handle.classList.remove('active');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    }
  });
}

// --- Sub-tabs (scoped) ---
document.addEventListener('click', (e) => {
  const subTab = e.target.closest('.sub-tab');
  if (!subTab) return;

  const tabGroup = subTab.parentElement;
  tabGroup.querySelectorAll('.sub-tab').forEach(t => t.classList.remove('active'));
  subTab.classList.add('active');

  const parent = tabGroup.parentElement;
  parent.querySelectorAll(':scope > .sub-content').forEach(sc => sc.classList.remove('active'));
  const target = parent.querySelector(`#subtab-${subTab.dataset.subtab}`);
  if (target) target.classList.add('active');

  // If the user just revealed the Body sub-tab and the form-table view is
  // showing, auto-size all value textareas. They may have been created while
  // the panel was display:none (scrollHeight == 0) and need a re-grow now
  // that they have a layout box.
  if (subTab.dataset.subtab === 'req-body' &&
      typeof bodyFormEditor !== 'undefined' && bodyFormEditor &&
      !bodyFormEditor.classList.contains('hidden') &&
      typeof growAllFormFieldRows === 'function') {
    requestAnimationFrame(() => growAllFormFieldRows());
  }
});

// --- Method select color ---
const METHOD_COLORS = {
  GET: '#3fb950', POST: '#58a6ff', PUT: '#d29922', PATCH: '#d29922',
  DELETE: '#f85149', HEAD: '#bc8cff', OPTIONS: '#8b949e'
};

function updateMethodColor() {
  methodSelect.style.color = METHOD_COLORS[methodSelect.value] || '#e6edf3';
}
methodSelect.addEventListener('change', () => { updateMethodColor(); scheduleLiveBuilderFlush(); });
updateMethodColor();

// --- Live Builder -> File Sync (silent) ---
// When the user edits any builder field, debounce-flush to the in-memory
// file.content/suite so that switching to code mode shows the latest edits
// immediately. We intentionally skip the codeEditor.value assignment and
// file-tree re-render here — those happen when the user actually switches
// modes or on explicit save/send.
let liveBuilderFlushTimer = null;
async function flushBuilderLive() {
  // Silent version of flushBuilderToFile used on every keystroke in the
  // builder. Auto-creates a temp file + appends a new block if none exist
  // yet, so users can start typing from a blank slate and the code view
  // will reflect the request without any explicit save action.
  const block = blockFromBuilder();
  if (!block) return; // URL empty — nothing meaningful yet
  await ensureBuilderFile();
  const file = loadedFiles[activeFileIndex];
  if (!file) return;

  const prevBlocks = [...file.suite.blocks];
  const prevBlockIndex = activeBlockIndex;
  let newBlockAppended = false;

  if (activeBlockIndex >= 0 && activeBlockIndex < file.suite.blocks.length) {
    file.suite.blocks[activeBlockIndex] = {
      ...file.suite.blocks[activeBlockIndex],
      request: block.request,
    };
  } else {
    file.suite.blocks.push(block);
    activeBlockIndex = file.suite.blocks.length - 1;
    newBlockAppended = true;
  }

  file.results = null;

  try {
    const content = await invoke('generate_http', {
      suite: buildSuiteWithDisabledFlags(activeFileIndex),
    });
    file.content = content;
    // Only refresh the file tree when we appended a new block — the badge
    // only changes in that case, so avoid the cost on every keystroke.
    if (newBlockAppended) renderFileTree();
  } catch (_err) {
    // Rollback suite mutation on generate failure — the user will get an
    // explicit error on the next Send/Save.
    file.suite.blocks = prevBlocks;
    activeBlockIndex = prevBlockIndex;
  }
}
function scheduleLiveBuilderFlush() {
  if (currentMode !== 'builder') return;
  clearTimeout(liveBuilderFlushTimer);
  liveBuilderFlushTimer = setTimeout(flushBuilderLive, 250);
}

// --- Headers key-value ---
function createHeaderRow(key = '', value = '', enabled = true) {
  const row = document.createElement('div');
  row.className = 'kv-row';
  row.innerHTML = `
    <input type="checkbox" class="kv-toggle" ${enabled ? 'checked' : ''} title="Enable this header">
    <input type="text" class="kv-key" placeholder="Header name" value="${escapeAttr(key)}" spellcheck="false">
    <input type="text" class="kv-value" placeholder="Header value" value="${escapeAttr(value)}" spellcheck="false">
    <button class="btn-icon kv-remove" title="Remove">&times;</button>
  `;
  row.querySelector('.kv-remove').addEventListener('click', () => {
    row.remove();
    if (headersContainer.children.length === 0) headersContainer.appendChild(createHeaderRow());
    scheduleLiveBuilderFlush();
  });
  row.querySelector('.kv-toggle').addEventListener('change', scheduleLiveBuilderFlush);
  attachVariableOverlay(row.querySelector('.kv-value'));
  return row;
}

addHeaderBtn.addEventListener('click', () => {
  headersContainer.appendChild(createHeaderRow());
  scheduleLiveBuilderFlush();
});

headersContainer.querySelector('.kv-remove')?.addEventListener('click', function() {
  this.closest('.kv-row').remove();
  if (headersContainer.children.length === 0) headersContainer.appendChild(createHeaderRow());
});

// --- URL Query Parameters (derived from URL, editable) ---
//
// The URL is the single source of truth. The Params tab is an ephemeral view
// derived from the URL; edits in Params rebuild the URL while preserving
// scheme/path/fragment and row order. We deliberately avoid URLSearchParams
// because it normalizes percent-encoding (e.g. %20 <-> + and %2B <-> +) which
// would silently mutate the user's URL.
//
// We're currently suppressing the refresh loop where URL changes trigger a
// params re-render (which would reset focus/order), so don't assign in both
// directions at once — use `paramsSyncLock`.
let paramsSyncLock = false;

function splitUrlParts(url) {
  // Returns { base, query, fragment }. Splits on FIRST '?' and FIRST '#' only.
  let fragment = '';
  let rest = url;
  const hashIdx = rest.indexOf('#');
  if (hashIdx >= 0) {
    fragment = rest.substring(hashIdx); // keeps leading '#'
    rest = rest.substring(0, hashIdx);
  }
  const qIdx = rest.indexOf('?');
  if (qIdx < 0) return { base: rest, query: null, fragment };
  return {
    base: rest.substring(0, qIdx),
    query: rest.substring(qIdx + 1),
    fragment,
  };
}

function parseQueryToParams(query) {
  // query may be null, '', or 'a=1&b=2&flag&c='.
  // Each pair split on FIRST '=' only. A missing '=' means hasEquals=false.
  if (query == null || query === '') return [];
  return query.split('&').map(pair => {
    const eq = pair.indexOf('=');
    if (eq < 0) return { key: pair, value: '', hasEquals: false };
    return { key: pair.substring(0, eq), value: pair.substring(eq + 1), hasEquals: true };
  });
}

function buildQueryFromParams(params) {
  const parts = params
    .filter(p => p.key !== '' || p.value !== '' || p.hasEquals)
    .map(p => p.hasEquals ? `${p.key}=${p.value}` : p.key);
  return parts.join('&');
}

function createParamRow(key = '', value = '', hasEquals = true) {
  const row = document.createElement('div');
  row.className = 'kv-row';
  row.dataset.hasEquals = hasEquals ? '1' : '0';
  row.innerHTML = `
    <input type="checkbox" class="kv-toggle" checked title="Include in URL">
    <input type="text" class="kv-key" placeholder="Parameter name" value="${escapeAttr(key)}" spellcheck="false">
    <input type="text" class="kv-value" placeholder="Value" value="${escapeAttr(value)}" spellcheck="false">
    <button class="btn-icon kv-remove" title="Remove">&times;</button>
  `;
  row.querySelector('.kv-remove').addEventListener('click', () => {
    row.remove();
    rebuildUrlFromParams();
    scheduleLiveBuilderFlush();
  });
  row.querySelector('.kv-toggle').addEventListener('change', () => {
    rebuildUrlFromParams();
    scheduleLiveBuilderFlush();
  });
  row.querySelector('.kv-key').addEventListener('input', () => {
    // User typed a key — they probably want a k=v pair now.
    if (row.querySelector('.kv-value').value !== '' ||
        row.querySelector('.kv-key').value !== '') {
      row.dataset.hasEquals = '1';
    }
    rebuildUrlFromParams();
    scheduleLiveBuilderFlush();
  });
  row.querySelector('.kv-value').addEventListener('input', () => {
    row.dataset.hasEquals = '1';
    rebuildUrlFromParams();
    scheduleLiveBuilderFlush();
  });
  attachVariableOverlay(row.querySelector('.kv-value'));
  return row;
}

function renderParamsFromUrl() {
  if (paramsSyncLock) return;
  paramsSyncLock = true;
  try {
    const { query } = splitUrlParts(urlInput.value || '');
    const params = parseQueryToParams(query);
    paramsContainer.innerHTML = '';
    if (params.length === 0) {
      // Keep one empty row so users can add a first param easily.
      paramsContainer.appendChild(createParamRow('', '', true));
      return;
    }
    for (const p of params) {
      paramsContainer.appendChild(createParamRow(p.key, p.value, p.hasEquals));
    }
  } finally {
    paramsSyncLock = false;
  }
}

function rebuildUrlFromParams() {
  if (paramsSyncLock) return;
  paramsSyncLock = true;
  try {
    const parts = splitUrlParts(urlInput.value || '');
    const params = [];
    paramsContainer.querySelectorAll('.kv-row').forEach(row => {
      const enabled = row.querySelector('.kv-toggle')?.checked ?? true;
      const key = row.querySelector('.kv-key').value;
      const value = row.querySelector('.kv-value').value;
      const hasEquals = row.dataset.hasEquals === '1';
      if (!enabled) return;
      if (key === '' && value === '' && !hasEquals) return;
      params.push({ key, value, hasEquals: hasEquals || value !== '' });
    });
    const query = buildQueryFromParams(params);
    const newUrl = parts.base + (query ? '?' + query : '') + (parts.fragment || '');
    if (newUrl !== urlInput.value) {
      urlInput.value = newUrl;
    }
  } finally {
    paramsSyncLock = false;
  }
}

if (addParamBtn) {
  addParamBtn.addEventListener('click', () => {
    paramsContainer.appendChild(createParamRow('', '', true));
    // Don't rebuild URL yet — empty rows contribute nothing.
  });
}

// When the user types in the URL field, re-derive the params view.
urlInput.addEventListener('input', () => {
  if (paramsSyncLock) return;
  renderParamsFromUrl();
});

// Initial render so the Params tab shows an empty-row placeholder before
// a block is selected.
renderParamsFromUrl();


// --- Extra Headers (injected into all test runs) ---
function createExtraHeaderRow(key = '', value = '', enabled = true) {
  const row = document.createElement('div');
  row.className = 'extra-header-row';
  row.innerHTML = `
    <input type="checkbox" ${enabled ? 'checked' : ''} title="Enable this header">
    <input type="text" class="eh-key" placeholder="Header name" value="${escapeAttr(key)}" spellcheck="false">
    <input type="text" class="eh-value" placeholder="Header value" value="${escapeAttr(value)}" spellcheck="false">
    <button class="btn-icon" title="Remove">&times;</button>
  `;
  row.querySelector('.btn-icon').addEventListener('click', () => {
    row.remove();
    updateExtraHeadersCount();
  });
  row.querySelectorAll('input').forEach(inp => inp.addEventListener('change', updateExtraHeadersCount));
  return row;
}

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

function updateExtraHeadersCount() {
  const count = getExtraHeaders().length;
  extraHeadersCount.textContent = count > 0 ? count : '';
}

extraHeadersBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  extraHeadersPanel.classList.toggle('hidden');
});

extraHeadersAddBtn.addEventListener('click', () => {
  extraHeadersList.appendChild(createExtraHeaderRow());
});

// Close panel when clicking outside
document.addEventListener('click', (e) => {
  if (!extraHeadersPanel.classList.contains('hidden') &&
      !extraHeadersPanel.contains(e.target) &&
      !extraHeadersBtn.contains(e.target)) {
    extraHeadersPanel.classList.add('hidden');
  }
});

// Add one empty row initially
extraHeadersList.appendChild(createExtraHeaderRow());

// --- Body type toggle ---
//
// The dropdown is the user's authoritative declaration of body format.
// When it changes (user-initiated), we:
//   1. Update the Content-Type header to match (json -> application/json,
//      form -> application/x-www-form-urlencoded). 'text' leaves any custom
//      CT alone (could be text/xml, text/plain, etc.); 'none' strips the CT
//      only if it was previously json/form so we don't clobber custom values.
//   2. Best-effort transform the body (JSON pretty-print; convert between
//      JSON and form-urlencoded when it makes sense).
//   3. Re-highlight using the new format.
// Synthetic events from `dispatchEvent(new Event('change'))` (e.g. during
// loadBlock) skip the mutation steps and just sync visual state.
function bodyTypeForContentType(ct) {
  if (!ct) return null;
  const v = String(ct).toLowerCase();
  if (v.includes('application/json')) return 'json';
  if (v.includes('application/x-www-form-urlencoded')) return 'form';
  if (v.startsWith('text/') || v.includes('xml') || v.includes('plain')) return 'text';
  return null;
}

function pickBodyType(headers, body) {
  if (!body || !body.trim()) return 'none';
  if (Array.isArray(headers)) {
    for (const h of headers) {
      const k = Array.isArray(h) ? h[0] : null;
      const v = Array.isArray(h) ? h[1] : null;
      if (k && k.toLowerCase() === 'content-type') {
        const bt = bodyTypeForContentType(v);
        if (bt) return bt;
      }
    }
  }
  const t = body.trim();
  if (t.startsWith('{') || t.startsWith('[')) return 'json';
  return 'text';
}

function findContentTypeRow() {
  const rows = headersContainer.querySelectorAll('.kv-row');
  for (const row of rows) {
    const k = row.querySelector('.kv-key')?.value?.trim().toLowerCase();
    if (k === 'content-type') return row;
  }
  return null;
}

function setContentTypeHeaderUI(value) {
  const existing = findContentTypeRow();
  if (existing) {
    const valInput = existing.querySelector('.kv-value');
    if (valInput && valInput.value !== value) valInput.value = value;
    const toggle = existing.querySelector('.kv-toggle');
    if (toggle && !toggle.checked) toggle.checked = true;
    return;
  }
  // Reuse the first empty row if present, else append a new one.
  const rows = headersContainer.querySelectorAll('.kv-row');
  for (const r of rows) {
    const key = r.querySelector('.kv-key')?.value?.trim();
    const v = r.querySelector('.kv-value')?.value?.trim();
    if (!key && !v) {
      r.querySelector('.kv-key').value = 'Content-Type';
      r.querySelector('.kv-value').value = value;
      const toggle = r.querySelector('.kv-toggle');
      if (toggle) toggle.checked = true;
      return;
    }
  }
  headersContainer.appendChild(createHeaderRow('Content-Type', value));
}

function removeJsonOrFormContentTypeUI() {
  const row = findContentTypeRow();
  if (!row) return;
  const v = row.querySelector('.kv-value')?.value?.trim().toLowerCase();
  if (!v) return;
  if (v.includes('application/json') || v.includes('application/x-www-form-urlencoded')) {
    row.remove();
    if (headersContainer.children.length === 0) headersContainer.appendChild(createHeaderRow());
  }
}

function tryFormToJsonString(text) {
  if (!text || !text.includes('=')) return null;
  const obj = {};
  let count = 0;
  for (const pair of text.split('&')) {
    if (!pair) continue;
    const idx = pair.indexOf('=');
    const k = idx >= 0 ? pair.slice(0, idx) : pair;
    const v = idx >= 0 ? pair.slice(idx + 1) : '';
    let dk = k, dv = v;
    try { dk = decodeURIComponent(k.replace(/\+/g, ' ')); } catch { /* keep raw */ }
    try { dv = decodeURIComponent(v.replace(/\+/g, ' ')); } catch { /* keep raw */ }
    if (!dk) continue;
    obj[dk] = dv;
    count++;
  }
  if (count === 0) return null;
  return JSON.stringify(obj, null, 2);
}

function tryJsonToFormString(text) {
  let obj;
  try { obj = JSON.parse(text); } catch { return null; }
  if (!obj || typeof obj !== 'object' || Array.isArray(obj)) return null;
  const parts = [];
  for (const [k, v] of Object.entries(obj)) {
    const sv = (v === null || v === undefined) ? ''
             : (typeof v === 'object') ? JSON.stringify(v)
             : String(v);
    // Preserve {{var}} template patterns so variable interpolation still matches
    parts.push(`${encodeFormValue(k)}=${encodeFormValue(sv)}`);
  }
  return parts.join('&');
}

// --- Form URL-Encoded body editor (key-value table view) ---
//
// Renders the body as a Postman-style kv-table when bodyType === 'form'.
// The textarea (`bodyInput`) remains the source of truth for everything
// else — send, save, variable interpolation, autosave. The table is just a
// view that round-trips through it: parse the wire-format string into rows
// on activation, serialize rows back into `bodyInput.value` on every edit.
//
// `{{var_name}}` template variables are exempted from URL-encoding so the
// existing variable-interpolation step (which runs on the body string before
// send) keeps matching them. They sit literally in the underlying body.

let formRawMode = false; // when true, show textarea even though type === 'form'

// Decode a URL-encoded form-urlencoded value (handles `+` as space).
// Falls back to the raw string if it contains malformed sequences (rare,
// only happens with hand-edited bodies that have stray `%` characters).
function decodeFormValue(s) {
  if (s == null) return '';
  try { return decodeURIComponent(String(s).replace(/\+/g, ' ')); }
  catch { return String(s); }
}

// Encode a value for a form-urlencoded body in "human-readable" form.
//
// Only the chars that have ambiguous semantics in form-urlencoded
// source files get encoded:
//   `&` (pair delimiter)  -> %26
//   `=` (kv delimiter)    -> %3D
//   `+` (means space)     -> %2B   (so literal `+` round-trips)
//   `%` (escape start)    -> %25   (so stray `%` doesn't look like an escape)
//
// Everything else (spaces, parens, brackets, braces, quotes, commas,
// newlines, etc.) stays LITERAL for readability. The Pilot runtime
// auto-encoder (`prepare_request_body`) finishes the job at send time:
// it preserves any `%XX` in the source and encodes the rest using the
// `application/x-www-form-urlencoded` rules.
//
// Template variables `{{name}}` pass through unencoded so the existing
// variable-interpolation step keeps matching them.
function encodeFormValue(s) {
  if (s == null || s === '') return '';
  const parts = String(s).split(/(\{\{[^}]+\}\})/g);
  return parts.map(p => {
    if (p.startsWith('{{') && p.endsWith('}}')) return p;
    return p
      .replace(/%/g, '%25')   // escape literal `%` first
      .replace(/&/g, '%26')   // pair delimiter
      .replace(/=/g, '%3D')   // kv delimiter
      .replace(/\+/g, '%2B'); // disambiguate from "encoded space"
  }).join('');
}

// Parse a form-urlencoded body string into [{key, value, enabled}] rows.
function parseFormBody(s) {
  if (!s) return [];
  const rows = [];
  for (const pair of String(s).split('&')) {
    if (!pair) continue;
    const eq = pair.indexOf('=');
    const k = eq >= 0 ? pair.slice(0, eq) : pair;
    const v = eq >= 0 ? pair.slice(eq + 1) : '';
    rows.push({ key: decodeFormValue(k), value: decodeFormValue(v), enabled: true });
  }
  return rows;
}

// Serialize [{key, value, enabled}] rows back into a form-urlencoded body.
function serializeFormBody(rows) {
  return rows
    .filter(r => r && r.enabled !== false && (r.key || '').length > 0)
    .map(r => `${encodeFormValue(r.key)}=${encodeFormValue(r.value)}`)
    .join('&');
}

// Read all rows currently in the DOM into row objects.
function readFormFieldRows() {
  const rows = [];
  formFieldsList.querySelectorAll('.kv-row').forEach(row => {
    rows.push({
      enabled: row.querySelector('.kv-toggle')?.checked ?? true,
      key:     row.querySelector('.kv-key')?.value ?? '',
      value:   row.querySelector('.kv-value-area')?.value ?? '',
    });
  });
  return rows;
}

// Push the current table state back into bodyInput.value and the file model.
function flushFormTableToBody() {
  const serialized = serializeFormBody(readFormFieldRows());
  if (bodyInput.value !== serialized) {
    bodyInput.value = serialized;
    // Don't re-highlight while user is typing in the table — the textarea is
    // hidden anyway. We do flush to the .http file model.
    scheduleLiveBuilderFlush();
  }
}

function autoGrowTextarea(ta) {
  // If the textarea has no layout box yet (e.g. its sub-tab is display:none
  // because the user is on a different sub-tab), scrollHeight will be 0 and
  // we'd collapse it. Bail out — growAllFormFieldRows() will re-run when
  // the panel becomes visible.
  if (!ta || !ta.isConnected) return;
  if (ta.offsetParent === null && ta.getClientRects().length === 0) return;
  ta.style.height = 'auto';
  // Cap height so a giant PromQL query doesn't push the action buttons off-screen
  ta.style.height = Math.min(ta.scrollHeight, 280) + 'px';
}

// Re-grow every value cell in the form editor. Called whenever the form
// editor becomes visible (sub-tab switch, dropdown change, etc.) so that
// rows created while the panel was hidden get sized correctly.
function growAllFormFieldRows() {
  if (!formFieldsList) return;
  formFieldsList.querySelectorAll('.kv-value-area').forEach(ta => autoGrowTextarea(ta));
}

function createFormFieldRow(key = '', value = '', enabled = true) {
  const row = document.createElement('div');
  row.className = 'kv-row';
  row.innerHTML = `
    <input type="checkbox" class="kv-toggle" ${enabled ? 'checked' : ''} title="Enable this field">
    <input type="text" class="kv-key" placeholder="Field name" value="${escapeAttr(key)}" spellcheck="false">
    <textarea class="kv-value-area" placeholder="Field value" spellcheck="false" rows="1"></textarea>
    <button class="btn-icon kv-remove" title="Remove">&times;</button>
  `;
  // Set textarea value via property (avoids HTML-escape gotchas with multi-line text)
  row.querySelector('.kv-value-area').value = value;
  row.querySelector('.kv-remove').addEventListener('click', () => {
    row.remove();
    if (formFieldsList.children.length === 0) formFieldsList.appendChild(createFormFieldRow());
    flushFormTableToBody();
  });
  row.querySelector('.kv-toggle').addEventListener('change', flushFormTableToBody);
  row.querySelector('.kv-key').addEventListener('input', flushFormTableToBody);
  const valEl = row.querySelector('.kv-value-area');
  valEl.addEventListener('input', () => { autoGrowTextarea(valEl); flushFormTableToBody(); });
  // Initial grow once we know the value's height
  requestAnimationFrame(() => autoGrowTextarea(valEl));
  attachVariableOverlay(valEl);
  return row;
}

function renderFormTableFromBody() {
  formFieldsList.innerHTML = '';
  const rows = parseFormBody(bodyInput.value);
  if (rows.length === 0) {
    formFieldsList.appendChild(createFormFieldRow());
  } else {
    rows.forEach(r => formFieldsList.appendChild(createFormFieldRow(r.key, r.value, r.enabled)));
  }
}

function showFormEditor() {
  if (formRawMode) {
    bodyEditorWrap.classList.remove('hidden');
    bodyFormEditor.classList.add('hidden');
    return;
  }
  bodyEditorWrap.classList.add('hidden');
  bodyFormEditor.classList.remove('hidden');
  renderFormTableFromBody();
  // Defer the grow until the next frame so layout has been computed for
  // the newly-visible panel (and so any sibling textareas get correct
  // scrollHeight values).
  requestAnimationFrame(() => growAllFormFieldRows());
}

function hideFormEditor() {
  bodyFormEditor.classList.add('hidden');
  bodyEditorWrap.classList.remove('hidden');
}

if (addFormFieldBtn) {
  addFormFieldBtn.addEventListener('click', () => {
    formFieldsList.appendChild(createFormFieldRow());
    flushFormTableToBody();
  });
}
if (toggleFormRawBtn) {
  toggleFormRawBtn.addEventListener('click', () => {
    if (bodyType.value !== 'form') return;
    formRawMode = !formRawMode;
    toggleFormRawBtn.classList.toggle('active', formRawMode);
    toggleFormRawBtn.textContent = formRawMode ? 'Edit fields' : 'Edit raw';
    if (formRawMode) {
      // Switching to raw — body is already in sync from the table edits.
      bodyFormEditor.classList.add('hidden');
      bodyEditorWrap.classList.remove('hidden');
      highlightBody();
    } else {
      // Switching back to table — re-parse the (possibly edited) raw body.
      bodyEditorWrap.classList.add('hidden');
      bodyFormEditor.classList.remove('hidden');
      renderFormTableFromBody();
    }
  });
}

bodyType.addEventListener('change', (event) => {
  bodyInput.disabled = bodyType.value === 'none';
  if (bodyType.value === 'json') {
    bodyInput.placeholder = '{\n  "key": "value"\n}';
  } else if (bodyType.value === 'form') {
    bodyInput.placeholder = 'key=value&another=value';
  } else if (bodyType.value === 'text') {
    bodyInput.placeholder = 'Raw text body...';
  } else {
    bodyInput.placeholder = 'Request body...';
  }

  // Only mutate headers / body when the user actually flipped the dropdown.
  // Synthetic events (during loadBlock / viewCompareStep) are visual-only.
  if (event && event.isTrusted) {
    const current = bodyInput.value.trim();
    if (bodyType.value === 'json') {
      setContentTypeHeaderUI('application/json');
      if (current) {
        try {
          bodyInput.value = JSON.stringify(JSON.parse(current), null, 2);
        } catch {
          const asJson = tryFormToJsonString(current);
          if (asJson) bodyInput.value = asJson;
        }
      }
    } else if (bodyType.value === 'form') {
      setContentTypeHeaderUI('application/x-www-form-urlencoded');
      if (current && (current.startsWith('{') || current.startsWith('['))) {
        const asForm = tryJsonToFormString(current);
        if (asForm !== null) bodyInput.value = asForm;
      }
    } else if (bodyType.value === 'none') {
      removeJsonOrFormContentTypeUI();
    }
    // 'text' leaves headers and body alone — user may want a custom CT.
    scheduleLiveBuilderFlush();
  }

  // Swap views: form-table for 'form' (unless raw mode is toggled), textarea otherwise.
  if (bodyType.value === 'form') {
    showFormEditor();
  } else {
    // Leaving form mode — reset the raw-toggle state so re-entering form
    // mode lands on the table view by default.
    if (formRawMode) {
      formRawMode = false;
      if (toggleFormRawBtn) {
        toggleFormRawBtn.classList.remove('active');
        toggleFormRawBtn.textContent = 'Edit raw';
      }
    }
    hideFormEditor();
  }

  highlightBody();
});

// --- Body Syntax Highlighting ---
function detectBodyLang(text) {
  // The body-type dropdown is authoritative for json/form. For 'text' or
  // 'none' we sniff from the body content (so SQL, XML, PromQL etc. still
  // get nice highlighting even though they all share the "Raw Text" type).
  const bt = (typeof bodyType !== 'undefined' && bodyType) ? bodyType.value : null;
  if (bt === 'json') return 'json';
  if (bt === 'form') return 'form';
  const t = text.trim();
  if (!t) return 'text';
  if (t.startsWith('{') || t.startsWith('[')) return 'json';
  if (t.startsWith('<') && t.includes('>')) return 'xml';
  if (/\b(SELECT|INSERT|UPDATE|DELETE|CREATE|ALTER|DROP|FROM|WHERE|JOIN)\b/i.test(t)) return 'sql';
  if (/[{}[\]()]/.test(t) && /\b(rate|sum|avg|count|histogram_quantile|topk|bottomk|by|without|on|group_left|group_right|offset)\b/i.test(t)) return 'promql';
  return 'text';
}

function hlJSON(text) {
  return text
    .replace(/(&quot;)((?:[^&]|&(?!quot;))*)(&quot;)/g, '<span class="hl-str">$1$2$3</span>')
    .replace(/\b(-?\d+\.?\d*([eE][+-]?\d+)?)\b/g, '<span class="hl-num">$1</span>')
    .replace(/\b(true|false|null)\b/g, '<span class="hl-kw">$1</span>')
    .replace(/([{}[\]])/g, '<span class="hl-bkt">$1</span>')
    .replace(/:/g, '<span class="hl-pct">:</span>')
    .replace(/,/g, '<span class="hl-pct">,</span>');
}

function hlXML(text) {
  return text
    .replace(/(&lt;!--)([\s\S]*?)(--&gt;)/g, '<span class="hl-cmt">$1$2$3</span>')
    .replace(/(&lt;\/?)([\w:.-]+)/g, '$1<span class="hl-tag">$2</span>')
    .replace(/([\w:.-]+)(=)/g, '<span class="hl-attr">$1</span>$2')
    .replace(/(&quot;)((?:[^&]|&(?!quot;))*)(&quot;)/g, '<span class="hl-str">$1$2$3</span>')
    .replace(/(\/?&gt;)/g, '<span class="hl-bkt">$1</span>');
}

function hlSQL(text) {
  const kw = /\b(SELECT|INSERT|UPDATE|DELETE|CREATE|ALTER|DROP|FROM|WHERE|JOIN|INNER|OUTER|LEFT|RIGHT|CROSS|ON|AND|OR|NOT|IN|BETWEEN|LIKE|IS|NULL|AS|ORDER|BY|GROUP|HAVING|LIMIT|OFFSET|UNION|ALL|DISTINCT|SET|INTO|VALUES|TABLE|INDEX|VIEW|EXISTS|CASE|WHEN|THEN|ELSE|END|ASC|DESC|COUNT|SUM|AVG|MIN|MAX|COALESCE|CAST)\b/gi;
  return text
    .replace(kw, '<span class="hl-kw">$1</span>')
    .replace(/(&#39;)((?:[^&]|&(?!#39;))*)(&#39;)/g, '<span class="hl-str">$1$2$3</span>')
    .replace(/\b(\d+\.?\d*)\b/g, '<span class="hl-num">$1</span>')
    .replace(/(--.*)/g, '<span class="hl-cmt">$1</span>');
}

function hlPromQL(text) {
  const funcs = /\b(rate|sum|avg|count|min|max|histogram_quantile|topk|bottomk|increase|irate|delta|idelta|deriv|predict_linear|resets|changes|label_replace|label_join|absent|absent_over_time|ceil|floor|round|clamp|clamp_max|clamp_min|exp|ln|log2|log10|sqrt|sgn|sort|sort_desc|time|timestamp|vector|scalar|quantile|stddev|stdvar|count_values|group|last_over_time|present_over_time|avg_over_time|min_over_time|max_over_time|sum_over_time|quantile_over_time|stddev_over_time|stdvar_over_time)\b/g;
  const mods = /\b(by|without|on|ignoring|group_left|group_right|offset|bool)\b/g;
  // Label matchers must run BEFORE function/keyword wrapping to avoid
  // matching inside injected HTML attributes (e.g. class="hl-fn")
  return text
    .replace(/([\w]+)\s*(=~|!=|=|!~)/g, '<span class="hl-attr">$1</span><span class="hl-pct">$2</span>')
    .replace(funcs, '<span class="hl-fn">$1</span>')
    .replace(mods, '<span class="hl-kw">$1</span>')
    .replace(/(&quot;)((?:[^&]|&(?!quot;))*)(&quot;)/g, '<span class="hl-str">$1$2$3</span>')
    .replace(/\b(\d+\.?\d*)(s|m|h|d|w|y)?\b/g, '<span class="hl-num">$1$2</span>')
    .replace(/([{}[\]()])/g, '<span class="hl-bkt">$1</span>');
}

function hlForm(text) {
  // Highlights `key=value&key=value` form-urlencoded bodies. `text` is
  // already HTML-escaped so '&' is '&amp;', '"' is '&quot;', etc.
  // Splitting on '&amp;' gives us per-pair fragments; each fragment is
  // colored as: key (attr) `=` (punctuation) value (string).
  const pairs = text.split('&amp;');
  const colored = pairs.map(pair => {
    const eqIdx = pair.indexOf('=');
    if (eqIdx < 0) return `<span class="hl-attr">${pair}</span>`;
    const key = pair.slice(0, eqIdx);
    const val = pair.slice(eqIdx + 1);
    return `<span class="hl-attr">${key}</span>` +
           `<span class="hl-pct">=</span>` +
           `<span class="hl-str">${val}</span>`;
  }).join('<span class="hl-pct">&amp;</span>');
  // Highlight {{template}} variables on top of the structural coloring.
  return colored.replace(/(\{\{[^}]+\}\})/g, '<span class="hl-num">$1</span>');
}

function highlightBody() {
  const text = bodyInput.value;
  const codeEl = bodyHighlight.querySelector('code');
  if (!text.trim() || bodyInput.disabled) {
    codeEl.innerHTML = '';
    bodyEditorWrap.classList.remove('highlighting');
    return;
  }
  const lang = detectBodyLang(text);
  if (lang === 'text') {
    codeEl.innerHTML = '';
    bodyEditorWrap.classList.remove('highlighting');
    return;
  }
  bodyEditorWrap.classList.add('highlighting');
  const escaped = escapeHtml(text);
  switch (lang) {
    case 'json':   codeEl.innerHTML = hlJSON(escaped); break;
    case 'xml':    codeEl.innerHTML = hlXML(escaped); break;
    case 'sql':    codeEl.innerHTML = hlSQL(escaped); break;
    case 'promql': codeEl.innerHTML = hlPromQL(escaped); break;
    case 'form':   codeEl.innerHTML = hlForm(escaped); break;
    default:       codeEl.innerHTML = escaped;
  }
  bodyHighlight.scrollTop = bodyInput.scrollTop;
  bodyHighlight.scrollLeft = bodyInput.scrollLeft;
}

bodyInput.addEventListener('input', () => { highlightBody(); scheduleLiveBuilderFlush(); });
bodyInput.addEventListener('scroll', () => {
  bodyHighlight.scrollTop = bodyInput.scrollTop;
  bodyHighlight.scrollLeft = bodyInput.scrollLeft;
});

// --- Variable interpolation ---
// Resolution stack (low → high precedence):
//   1. .http file `@variables` (defaults shipped in repo)
//   2. active .env file values (loaded user overrides)
//   3. runtimeOverrides (`@@extract` results, post-run final_variables)
function interpolateVariables(str) {
  if (!str) return str;
  const merged = buildMergedVarsObject();
  return str.replace(/\{\{(\w+)\}\}/g, (match, name) => {
    if (name === '$timestamp') return Date.now().toString();
    if (name === '$uuid') return crypto.randomUUID();
    if (name === '$randomInt') return Math.floor(Math.random() * 10000).toString();
    return merged[name] !== undefined ? merged[name] : match;
  });
}

// --- Variable overlay highlighting ---
//
// Renders `{{var}}` references in URL/header/param/form-field input boxes
// as colored chips. Implementation: a transparent-text mirror element is
// positioned behind the input. When the user types, we re-render the
// mirror with `<span class="hl-var">{{var}}</span>` chips. The user's
// actual text continues to be drawn by the input on top, so the chip
// backgrounds appear *underneath* the typed text. Each chip's `title`
// attribute carries the resolved value so hovering shows it as a tooltip.
//
// `attachVariableOverlay(input)` is idempotent — calling it twice on the
// same input is a no-op.

function escapeOverlayHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

// Render a string as HTML where `{{var}}` references become hl-var chips.
// Returns { html, varList } where varList is a deduped list of "name = value"
// lines for use as a title-attribute tooltip on the input itself.
function renderVariableChips(text) {
  if (!text) return { html: '', varList: [] };
  const merged = buildMergedVarsObject();
  let html = '';
  let i = 0;
  const re = /\{\{([\w$]+)\}\}/g;
  let m;
  const seen = new Set();
  const varList = [];
  while ((m = re.exec(text)) !== null) {
    if (m.index > i) html += escapeOverlayHtml(text.slice(i, m.index));
    const name = m[1];
    let resolved;
    let isResolved = true;
    if (name === '$timestamp') resolved = '(generated at run time)';
    else if (name === '$uuid') resolved = '(random UUID at run time)';
    else if (name === '$randomInt') resolved = '(random int at run time)';
    else if (Object.prototype.hasOwnProperty.call(merged, name)) resolved = merged[name];
    else { resolved = '(undefined)'; isResolved = false; }
    if (!seen.has(name)) {
      seen.add(name);
      varList.push(`{{${name}}} = ${resolved}`);
    }
    const cls = isResolved ? 'hl-var' : 'hl-var unresolved';
    html += `<span class="${cls}">${escapeOverlayHtml(m[0])}</span>`;
    i = m.index + m[0].length;
  }
  if (i < text.length) html += escapeOverlayHtml(text.slice(i));
  return { html, varList };
}

// Tracks every input/textarea that has an overlay attached so we can
// re-render their chips when the variable resolution changes (env file
// loaded, override changed, etc.).
const _variableOverlays = new WeakMap(); // input -> mirror element
const _variableOverlaySet = new Set();   // strong refs for iteration

function syncVariableOverlay(input) {
  const mirror = _variableOverlays.get(input);
  if (!mirror) return;
  const { html, varList } = renderVariableChips(input.value || '');
  mirror.innerHTML = html;
  // Show resolved values via the input's native tooltip — keeps the input
  // fully clickable (chips have pointer-events: none) and gives the user
  // a one-stop view of every {{var}} the field references.
  if (varList.length > 0) {
    if (input.dataset.titleOriginal == null) {
      input.dataset.titleOriginal = input.getAttribute('title') || '';
    }
    input.title = varList.join('\n');
  } else if (input.dataset.titleOriginal != null) {
    if (input.dataset.titleOriginal) input.title = input.dataset.titleOriginal;
    else input.removeAttribute('title');
  }
  // Sync scroll position so chips align with text when input is scrolled.
  const overlay = mirror.parentElement;
  if (overlay) {
    overlay.scrollLeft = input.scrollLeft;
    overlay.scrollTop = input.scrollTop;
  }
}

// Refresh ALL attached overlays. Cheap because each just re-runs the
// regex on its current value. Call when merged vars change so tooltips
// reflect the latest resolved values.
function refreshAllVariableOverlays() {
  _variableOverlaySet.forEach(input => {
    if (!input.isConnected) {
      _variableOverlaySet.delete(input);
      return;
    }
    syncVariableOverlay(input);
  });
}

function attachVariableOverlay(input) {
  if (!input || _variableOverlays.has(input)) return;
  // Build the overlay structure: insert a wrapper around the input and
  // place a transparent-text mirror behind it.
  const wrapper = document.createElement('span');
  wrapper.className = 'var-overlay-wrapper';
  // Preserve flex/sizing of the original input
  if (input.classList.contains('url-input')) wrapper.classList.add('url-overlay');
  else if (input.classList.contains('kv-key') || input.classList.contains('kv-value')) wrapper.classList.add('kv-overlay');
  else if (input.classList.contains('kv-value-area')) wrapper.classList.add('form-overlay');
  const parent = input.parentNode;
  if (!parent) return;
  parent.insertBefore(wrapper, input);
  const overlay = document.createElement('div');
  overlay.className = 'var-overlay';
  overlay.setAttribute('aria-hidden', 'true');
  const mirror = document.createElement('div');
  mirror.className = 'var-mirror';
  overlay.appendChild(mirror);
  wrapper.appendChild(overlay);
  wrapper.appendChild(input);
  _variableOverlays.set(input, mirror);
  _variableOverlaySet.add(input);
  syncVariableOverlay(input);
  input.addEventListener('input', () => syncVariableOverlay(input));
  input.addEventListener('scroll', () => syncVariableOverlay(input));
}

function collectVariablesArray() {
  return Object.entries(buildMergedVarsObject());
}

function buildMergedVarsObject() {
  const merged = {};
  // .http file defaults — lowest precedence
  loadedFiles.forEach(file => {
    if (file.suite && file.suite.variables) {
      file.suite.variables.forEach(([name, value]) => { merged[name] = value; });
    }
  });
  // Active .env vars override .http defaults
  if (activeEnvVars && activeEnvVars.length > 0) {
    activeEnvVars.forEach(([name, value]) => { merged[name] = value; });
  }
  // Runtime overrides (@@extract, post-run final_variables) — highest precedence
  Object.entries(runtimeOverrides).forEach(([name, value]) => {
    if (value !== '') merged[name] = value;
  });
  return merged;
}

function lookupVar(name) {
  if (Object.prototype.hasOwnProperty.call(runtimeOverrides, name)) {
    const v = runtimeOverrides[name];
    if (v !== undefined && v !== '') return v;
  }
  if (activeEnvVars && activeEnvVars.length > 0) {
    const hit = activeEnvVars.find(([k]) => k === name);
    if (hit && hit[1] !== '') return hit[1];
  }
  for (const f of loadedFiles) {
    if (!f.suite || !f.suite.variables) continue;
    const v = f.suite.variables.find(([k]) => k === name);
    if (v && v[1] !== '') return v[1];
  }
  return undefined;
}

// ── Environment (.env file list) ─────────────────────────────────────────
// Loaded .env files with exactly one active at a time. The full list + active
// index live in a persisted env config (managed by Rust; see core/env_config).
// Per-card UI state (expanded, dirty edit drafts, lazily-loaded vars for
// inactive cards) lives alongside the persisted config in `envCardState`.
let envConfig = { entries: [], active_index: null };
let activeEnvVars = []; // [[name, value], ...] for the active entry only
let envCardState = []; // parallel array: { vars: [[k,v]]?, draftVars: [[k,v]]?, expanded: bool, dirty: bool, loading: bool }

async function reloadEnvList(preserveCardState = true) {
  try {
    const cfg = await invoke('env_list');
    envConfig = normalizeEnvConfig(cfg);
    activeEnvVars = await invoke('env_resolve_active');
    syncEnvCardState(preserveCardState);
    seedActiveCardVars();
    renderEnvPanel();
  } catch (err) {
    rpLog('error', 'reloadEnvList failed', { error: String(err) });
  }
}

function normalizeEnvConfig(cfg) {
  return {
    entries: (cfg && cfg.entries) || [],
    // serde serializes None as null; ensure we coerce undefined -> null too
    active_index: (cfg && cfg.active_index != null) ? cfg.active_index : null,
  };
}

function defaultCardState() {
  return { vars: null, draftVars: null, expanded: false, dirty: false, loading: false };
}

// Rebuild envCardState to match current entries; preserve expanded/draft per
// path (entries can be reordered after add/remove).
function syncEnvCardState(preserve) {
  const byPath = new Map();
  if (preserve) {
    envCardState.forEach((s, i) => {
      const e = envConfig.entries[i];
      if (e) byPath.set(e.path, s);
    });
  }
  envCardState = envConfig.entries.map((e) => {
    return byPath.get(e.path) || defaultCardState();
  });
}

// When the active entry changes, ensure its card has up-to-date vars without a
// separate fetch (we already loaded them via env_resolve_active).
function seedActiveCardVars() {
  const i = envConfig.active_index;
  if (i == null) return;
  if (!envCardState[i]) envCardState[i] = defaultCardState();
  envCardState[i].vars = activeEnvVars.map(([k, v]) => [k, v]);
  if (!envCardState[i].dirty) {
    envCardState[i].draftVars = null;
  }
}

async function ensureCardVarsLoaded(index) {
  const state = envCardState[index];
  if (!state || state.vars || state.loading) return;
  state.loading = true;
  try {
    const vars = await invoke('env_resolve_entry', { index });
    state.vars = vars || [];
  } catch (err) {
    rpLog('error', 'env_resolve_entry failed', { index, error: String(err) });
    state.vars = [];
  } finally {
    state.loading = false;
  }
}

async function activateEnvIndex(index) {
  try {
    const res = await invoke('env_set_active', { index });
    envConfig = normalizeEnvConfig(res.config);
    activeEnvVars = res.active_vars || [];
    syncEnvCardState(true);
    seedActiveCardVars();
    renderEnvPanel();
  } catch (err) {
    rpLog('error', 'env_set_active failed', { error: String(err) });
    showToast('Failed to activate env: ' + String(err), 'error');
  }
}

async function removeEnvIndex(index) {
  try {
    const res = await invoke('env_remove', { index });
    envConfig = normalizeEnvConfig(res.config);
    activeEnvVars = res.active_vars || [];
    syncEnvCardState(true);
    seedActiveCardVars();
    renderEnvPanel();
  } catch (err) {
    rpLog('error', 'env_remove failed', { error: String(err) });
    showToast('Failed to remove env: ' + String(err), 'error');
  }
}

async function saveEnvIndex(index) {
  const state = envCardState[index];
  if (!state || !state.dirty) return;
  const vars = state.draftVars || state.vars || [];
  try {
    const res = await invoke('env_save', { index, vars });
    envConfig = normalizeEnvConfig(res.config);
    activeEnvVars = res.active_vars || [];
    state.vars = vars.map(([k, v]) => [k, v]);
    state.draftVars = null;
    state.dirty = false;
    seedActiveCardVars();
    renderEnvPanel();
    const entry = envConfig.entries[index];
    if (entry) showToast(`Saved ${entry.name}`, 'success');
  } catch (err) {
    rpLog('error', 'env_save failed', { index, error: String(err) });
    showToast('Failed to save .env: ' + String(err), 'error');
  }
}

function shortenPath(p) {
  if (!p) return '';
  const norm = p.replace(/\\/g, '/');
  const parts = norm.split('/');
  if (parts.length <= 2) return norm;
  return '…/' + parts.slice(-2).join('/');
}

function setToast(msg) {
  if (typeof showToast === 'function') { showToast(msg); return; }
  rpLog('info', msg);
}

// =================================================================
// Themed modal dialogs (replaces window.alert / confirm / prompt)
// =================================================================
//
// All three return Promises and use the same styling system so any popup in
// the app feels native to Request Pilot rather than the OS default.
//
//   showModalAlert({ title, message, type, okLabel })            → Promise<true>
//   showModalConfirm({ title, message, type, okLabel, cancelLabel, danger })
//                                                               → Promise<boolean>
//   showModalPrompt({ title, message, defaultValue, placeholder,
//                     okLabel, cancelLabel, hint })             → Promise<string|null>
//
// `type` controls the icon color: 'info' (default), 'success', 'warn', 'danger'.
// `danger: true` on confirm makes the OK button red.
// Esc cancels. Enter confirms (in prompts: only when input has focus).
// Click outside closes as cancel.

function _rpModalShow(opts) {
  const {
    title = '',
    message = '',
    type = 'info',
    icon: customIcon = null,
    inputDefault = null, // when string, render an <input> seeded with this
    placeholder = '',
    hint = '',
    okLabel = 'OK',
    cancelLabel = null, // when null, no Cancel button (alert mode)
    danger = false,
    details = null, // optional preformatted block (e.g. error stack)
  } = opts;

  const icons = { info: 'ℹ', success: '✓', warn: '⚠', danger: '⚠' };
  const iconChar = customIcon || icons[type] || icons.info;

  return new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.className = 'rp-modal-overlay';

    const modal = document.createElement('div');
    modal.className = 'rp-modal';
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');

    // ── Header ──
    const header = document.createElement('div');
    header.className = 'rp-modal-header';
    const iconEl = document.createElement('span');
    iconEl.className = `rp-modal-icon ${type}`;
    iconEl.textContent = iconChar;
    const titleEl = document.createElement('div');
    titleEl.className = 'rp-modal-title';
    titleEl.textContent = title;
    const closeBtn = document.createElement('button');
    closeBtn.className = 'rp-modal-close';
    closeBtn.type = 'button';
    closeBtn.title = 'Close';
    closeBtn.setAttribute('aria-label', 'Close');
    closeBtn.textContent = '✕';
    header.appendChild(iconEl);
    header.appendChild(titleEl);
    header.appendChild(closeBtn);

    // ── Body ──
    const body = document.createElement('div');
    body.className = 'rp-modal-body';
    if (message) {
      const msg = document.createElement('div');
      msg.className = 'rp-modal-message';
      msg.textContent = message;
      body.appendChild(msg);
    }
    let input = null;
    if (typeof inputDefault === 'string') {
      input = document.createElement('input');
      input.type = 'text';
      input.className = 'rp-modal-input';
      input.value = inputDefault;
      input.placeholder = placeholder || '';
      input.setAttribute('autocomplete', 'off');
      input.setAttribute('spellcheck', 'false');
      body.appendChild(input);
      if (hint) {
        const hintEl = document.createElement('div');
        hintEl.className = 'rp-modal-hint';
        hintEl.textContent = hint;
        body.appendChild(hintEl);
      }
    }
    if (details) {
      const det = document.createElement('div');
      det.className = 'rp-modal-details';
      det.textContent = details;
      body.appendChild(det);
    }

    // ── Footer ──
    const footer = document.createElement('div');
    footer.className = 'rp-modal-footer';
    let cancelBtn = null;
    if (cancelLabel !== null) {
      cancelBtn = document.createElement('button');
      cancelBtn.type = 'button';
      cancelBtn.className = 'btn btn-sm';
      cancelBtn.textContent = cancelLabel || 'Cancel';
      footer.appendChild(cancelBtn);
    }
    const okBtn = document.createElement('button');
    okBtn.type = 'button';
    okBtn.className = `btn btn-sm ${danger ? 'btn-danger' : 'btn-primary'}`;
    okBtn.textContent = okLabel;
    footer.appendChild(okBtn);

    modal.appendChild(header);
    modal.appendChild(body);
    modal.appendChild(footer);
    overlay.appendChild(modal);
    document.body.appendChild(overlay);

    const close = (result) => {
      document.removeEventListener('keydown', onKey);
      try { document.body.removeChild(overlay); } catch (_) { /* already gone */ }
      resolve(result);
    };

    const isPrompt = input !== null;
    const cancelResult = isPrompt ? null : false;
    const okResult = isPrompt ? () => input.value : true;
    const fireOk = () => close(typeof okResult === 'function' ? okResult() : okResult);
    const fireCancel = () => close(cancelResult);

    closeBtn.addEventListener('click', fireCancel);
    if (cancelBtn) cancelBtn.addEventListener('click', fireCancel);
    okBtn.addEventListener('click', fireOk);
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) fireCancel(); });

    const onKey = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        fireCancel();
      } else if (e.key === 'Enter') {
        // For prompts, fire OK on Enter only when input is focused (so users
        // can still tab to Cancel and press Enter on it). For non-prompts,
        // any Enter confirms.
        if (!isPrompt || document.activeElement === input) {
          e.preventDefault();
          fireOk();
        }
      }
    };
    document.addEventListener('keydown', onKey);

    // Auto-focus: input first if present, else the OK button.
    setTimeout(() => {
      if (input) {
        input.focus();
        input.select();
      } else {
        okBtn.focus();
      }
    }, 0);
  });
}

function showModalAlert(opts = {}) {
  const o = (typeof opts === 'string') ? { message: opts } : opts;
  return _rpModalShow({
    title: o.title || 'Notice',
    message: o.message || '',
    type: o.type || 'info',
    icon: o.icon,
    okLabel: o.okLabel || 'OK',
    cancelLabel: null,
    details: o.details || null,
  });
}

function showModalConfirm(opts = {}) {
  const o = (typeof opts === 'string') ? { message: opts } : opts;
  return _rpModalShow({
    title: o.title || 'Confirm',
    message: o.message || '',
    type: o.type || (o.danger ? 'warn' : 'info'),
    icon: o.icon,
    okLabel: o.okLabel || 'OK',
    cancelLabel: o.cancelLabel || 'Cancel',
    danger: !!o.danger,
    details: o.details || null,
  }).then(v => v === true);
}

function showModalPrompt(opts = {}) {
  const o = (typeof opts === 'string') ? { message: opts } : opts;
  return _rpModalShow({
    title: o.title || 'Input',
    message: o.message || '',
    type: o.type || 'info',
    icon: o.icon,
    inputDefault: typeof o.defaultValue === 'string' ? o.defaultValue : '',
    placeholder: o.placeholder || '',
    hint: o.hint || '',
    okLabel: o.okLabel || 'OK',
    cancelLabel: o.cancelLabel || 'Cancel',
  }).then(v => (v === null || v === false) ? null : v);
}

// Session-scoped cache of @prompt values: Map<fileName, Map<varName, value>>
const promptValueCache = new Map();

/// Ask the user for `@prompt VAR` values declared in a suite. Returns an array
/// of `[name, value]` pairs to merge into extraVariables, or `null` if the
/// user cancelled. If the suite has no prompts, returns [].
async function collectPromptVariables(suite, fileName) {
  const prompts = (suite && suite.prompts) || [];
  if (prompts.length === 0) return [];

  // Filter out prompts that already have a concrete value in loaded vars
  // (either from @variables block or env). Still show them as pre-filled.
  const existing = new Map(collectVariablesArray());
  const cache = promptValueCache.get(fileName) || new Map();

  return new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.className = 'rp-modal-overlay';
    const modal = document.createElement('div');
    modal.className = 'rp-modal';
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    modal.innerHTML = `
      <div class="rp-modal-header">
        <span class="rp-modal-icon info">?</span>
        <div class="rp-modal-title">Supply runtime values</div>
        <button type="button" class="rp-modal-close" id="pilot-prompt-x" title="Cancel" aria-label="Cancel">✕</button>
      </div>
      <div class="rp-modal-body">
        <div class="rp-modal-message"><strong>${fileName}</strong> declares <code>@prompt</code> variables. Provide values before the suite runs.</div>
        <form id="pilot-prompt-form" style="margin-top:12px;"></form>
      </div>
      <div class="rp-modal-footer">
        <button type="button" id="pilot-prompt-cancel" class="btn btn-sm">Cancel</button>
        <button type="submit" form="pilot-prompt-form" class="btn btn-sm btn-primary">Run</button>
      </div>
    `;
    const form = modal.querySelector('#pilot-prompt-form');
    prompts.forEach(p => {
      const row = document.createElement('div');
      row.style.cssText = 'margin-bottom:12px;';
      const label = document.createElement('label');
      label.textContent = p.name + (p.description ? ' — ' + p.description : '');
      label.style.cssText = 'display:block;font-size:12px;margin-bottom:4px;font-weight:500;color:var(--text-primary);';
      const input = document.createElement('input');
      input.type = 'text';
      input.name = p.name;
      input.value = cache.get(p.name) || existing.get(p.name) || '';
      input.className = 'rp-modal-input';
      input.style.marginTop = '0';
      row.appendChild(label);
      row.appendChild(input);
      form.appendChild(row);
    });
    overlay.appendChild(modal);
    document.body.appendChild(overlay);
    setTimeout(() => { const first = form.querySelector('input'); if (first) first.focus(); }, 0);

    const close = (result) => {
      document.removeEventListener('keydown', onKey);
      try { document.body.removeChild(overlay); } catch (_) { /* already gone */ }
      resolve(result);
    };
    const onKey = (e) => { if (e.key === 'Escape') { e.preventDefault(); close(null); } };
    document.addEventListener('keydown', onKey);
    modal.querySelector('#pilot-prompt-cancel').addEventListener('click', () => close(null));
    modal.querySelector('#pilot-prompt-x').addEventListener('click', () => close(null));
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) close(null); });
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      const values = [];
      const perFile = new Map();
      prompts.forEach(p => {
        const v = form.elements[p.name]?.value ?? '';
        values.push([p.name, v]);
        perFile.set(p.name, v);
      });
      promptValueCache.set(fileName, perFile);
      close(values);
    });
  });
}

// Build a suite copy with disabled blocks removed
function getEnabledSuite(fileIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return null;
  const enabledBlocks = file.suite.blocks.filter((_, blockIdx) => !disabledBlocks[`${fileIdx}-${blockIdx}`]);
  return {
    variables: file.suite.variables,
    blocks: enabledBlocks,
  };
}

// Map from enabled-suite result index → original block index
function getEnabledIndexMap(fileIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return [];
  const map = [];
  file.suite.blocks.forEach((_, blockIdx) => {
    if (!disabledBlocks[`${fileIdx}-${blockIdx}`]) map.push(blockIdx);
  });
  return map;
}

// Remap block_results from enabled-only indices to original block indices.
// Results are matched by block name because Rust sorts blocks by type
// (setup → test → teardown), so positional mapping would be wrong.
function remapResults(fileIdx, results) {
  const file = loadedFiles[fileIdx];
  if (!file || !results) return results;
  const enabledMap = getEnabledIndexMap(fileIdx);
  const remapped = new Array(file.suite.blocks.length).fill(null);

  // Track which enabled indices haven't been matched yet (handles duplicate names)
  const unmatchedIndices = [...enabledMap];
  results.block_results.forEach(br => {
    const matchPos = unmatchedIndices.findIndex(idx => file.suite.blocks[idx].name === br.name);
    if (matchPos >= 0) {
      const origIdx = unmatchedIndices[matchPos];
      remapped[origIdx] = br;
      unmatchedIndices.splice(matchPos, 1);
    }
  });

  return { ...results, block_results: remapped };
}

/** Build a request block object from the current builder UI state. */
function blockFromBuilder() {
  const method = methodSelect.value;
  const url = urlInput.value.trim();
  if (!url) return null;

  const headers = [];
  headersContainer.querySelectorAll('.kv-row').forEach(row => {
    const key = row.querySelector('.kv-key').value.trim();
    const val = row.querySelector('.kv-value').value.trim();
    if (key) headers.push([key, val]);
  });

  let body = null;
  if (bodyType.value !== 'none' && bodyInput.value.trim()) {
    body = bodyInput.value.trim();
  }

  // Read metadata from accordion
  const name = metaName.value.trim() || `${method} ${url.split('?')[0].split('/').slice(-2).join('/')}`;
  return {
    name,
    block_type: metaBlockType.value || 'test',
    description: metaDescription.value.trim(),
    request: { method, url, headers, body },
    assertions: [],
    extracts: [],
    disabled: metaDisabled.checked,
    group: metaGroup.value.trim() || null,
    depends: metaDepends.value.split(',').map(s => s.trim()).filter(Boolean),
    mode: metaMode.value || null,
    dev_auth: metaDevAuth.value.trim() || null,
    compare: metaCompare.checked,
    steps: [],
    diff: null,
  };
}

/** Ensure a file exists for the builder. Creates temp file if none loaded. */
async function ensureBuilderFile() {
  if (activeFileIndex >= 0 && loadedFiles[activeFileIndex]) return;

  // Create a minimal temp file
  const content = '@variables\n';
  const suite = await invoke('parse_test_file', { content });
  loadedFiles.push({ name: 'untitled.http', content, suite, results: null, savedPath: null });
  activeFileIndex = loadedFiles.length - 1;
  activeBlockIndex = -1;
  renderFileTree();
}

/** Flush builder UI state into the active file's suite and regenerate content.
 *  Returns true on success, false on failure. */
async function flushBuilderToFile() {
  const block = blockFromBuilder();
  if (!block) return true; // nothing to flush

  await ensureBuilderFile();
  const file = loadedFiles[activeFileIndex];

  // Snapshot blocks in case we need to rollback
  const prevBlocks = [...file.suite.blocks];
  const prevBlockIndex = activeBlockIndex;

  if (activeBlockIndex >= 0 && activeBlockIndex < file.suite.blocks.length) {
    // Update existing block in-place
    file.suite.blocks[activeBlockIndex] = {
      ...file.suite.blocks[activeBlockIndex],
      request: block.request,
    };
  } else {
    // Append new block
    file.suite.blocks.push(block);
    activeBlockIndex = file.suite.blocks.length - 1;
  }

  // Clear stale results since suite changed
  file.results = null;

  // Regenerate .http content from suite
  try {
    const content = await invoke('generate_http', { suite: file.suite });
    file.content = content;
    // Keep code editor in sync if visible
    if (currentMode === 'code') {
      codeEditor.value = content;
      codeEditorContent = content;
      codeEditorModified = false;
      codeEditor.classList.remove('modified');
      updateHighlight();
    }
    renderFileTree();
    return true;
  } catch (err) {
    // Rollback suite mutation on failure
    file.suite.blocks = prevBlocks;
    activeBlockIndex = prevBlockIndex;
    rpLog('warn', 'Failed to regenerate .http content', String(err));
    showToast('Failed to sync builder to file', 'error');
    return false;
  }
}

// --- Send Request ---
let sendInFlight = false;
async function sendRequest() {
  if (sendInFlight) return;
  sendInFlight = true;

  const method = methodSelect.value;
  let url = urlInput.value.trim();

  if (!url) {
    showToast('Please enter a URL', 'error');
    urlInput.focus();
    sendInFlight = false;
    return;
  }

  // Flush builder state to file so code and builder stay in sync
  const flushed = await flushBuilderToFile();
  if (!flushed) {
    sendInFlight = false;
    return;
  }

  url = interpolateVariables(url);

  const headers = [];
  headersContainer.querySelectorAll('.kv-row').forEach(row => {
    const enabled = row.querySelector('.kv-toggle')?.checked ?? true;
    const key = row.querySelector('.kv-key').value.trim();
    const val = interpolateVariables(row.querySelector('.kv-value').value.trim());
    if (enabled && key) headers.push([key, val]);
  });

  if (bodyType.value === 'json' && !headers.some(([k]) => k.toLowerCase() === 'content-type')) {
    headers.push(['Content-Type', 'application/json']);
  } else if (bodyType.value === 'form' && !headers.some(([k]) => k.toLowerCase() === 'content-type')) {
    headers.push(['Content-Type', 'application/x-www-form-urlencoded']);
  }

  let body = (bodyType.value !== 'none' && bodyInput.value.trim()) ? bodyInput.value.trim() : null;
  if (body) body = interpolateVariables(body);

  sendBtn.disabled = true;
  sendBtn.classList.add('loading');

  try {
    const resp = await invoke('send_request', { method, url, headers, body });
    lastResponse = resp;
    displayResponse(resp);
    showToast(`${resp.status} ${resp.status_text}`, resp.status < 400 ? 'success' : 'error');
    rpLog('info', 'Response: ' + resp.status, { url, timeMs: resp.time_ms });
  } catch (err) {
    displayError(err);
    showToast(String(err), 'error');
    rpLog('error', 'Command failed: send_request', String(err));
  } finally {
    sendBtn.disabled = false;
    sendBtn.classList.remove('loading');
    sendInFlight = false;
    refreshHistoryIfVisible();
  }
}

sendBtn.addEventListener('click', sendRequest);
urlInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.ctrlKey && !e.shiftKey) sendRequest();
});
urlInput.addEventListener('input', scheduleLiveBuilderFlush);
// Highlight `{{var}}` references in the URL bar with chip backgrounds
// + tooltips showing each variable's resolved value.
attachVariableOverlay(urlInput);

// --- Content-Type Detection ---
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
    try { JSON.parse(trimmed); return 'json'; } catch { /* non-critical: JSON probe */ }
  }

  if (trimmed.startsWith('<?xml') || (trimmed.startsWith('<') && trimmed.includes('</'))) return 'xml';
  if (trimmed.toLowerCase().startsWith('<!doctype') || trimmed.toLowerCase().startsWith('<html')) return 'html';
  if (/^[a-zA-Z_][\w]*:\s/m.test(trimmed) && !trimmed.includes('{')) return 'yaml';

  return 'text';
}

// --- XML Pretty-Printer ---
function prettyPrintXml(xml) {
  let formatted = '';
  let indent = 0;
  const parts = xml.replace(/>\s*</g, '><').split(/(<[^>]+>)/);
  for (const part of parts) {
    if (!part.trim()) continue;
    if (part.match(/^<\/\w/)) {
      indent = Math.max(indent - 1, 0);
      formatted += '  '.repeat(indent) + part + '\n';
    } else if (part.match(/^<\w[^>]*[^/]>$/)) {
      formatted += '  '.repeat(indent) + part + '\n';
      indent++;
    } else if (part.match(/^<\w[^>]*\/>$/)) {
      formatted += '  '.repeat(indent) + part + '\n';
    } else if (part.startsWith('<')) {
      formatted += '  '.repeat(indent) + part + '\n';
    } else {
      formatted += '  '.repeat(indent) + part + '\n';
    }
  }
  return formatted.trimEnd();
}

// --- XML/HTML Syntax Highlighter ---
function renderXmlHighlighted(xmlString) {
  const pre = document.createElement('pre');
  pre.className = 'syntax-plain';
  const formatted = prettyPrintXml(xmlString);
  const escaped = escapeHtml(formatted);
  const highlighted = escaped
    .replace(/&lt;!--[\s\S]*?--&gt;/g, m => `<span class="syntax-comment">${m}</span>`)
    .replace(/&lt;!\[CDATA\[[\s\S]*?\]\]&gt;/g, m => `<span class="syntax-cdata">${m}</span>`)
    .replace(/&lt;(!DOCTYPE[^&]*?)&gt;/gi, (m, inner) => `<span class="syntax-doctype">&lt;${inner}&gt;</span>`)
    .replace(/&lt;(\/?)([\w:-]+)((?:\s+[\s\S]*?)?)\s*(\/?)\s*&gt;/g, (m, slash, tag, attrs, selfClose) => {
      let result = `&lt;${slash}<span class="syntax-tag">${tag}</span>`;
      if (attrs) {
        result += attrs.replace(/([\w:-]+)=(&quot;(?:[^&]*?)&quot;)/g,
          (am, attr, val) => `<span class="syntax-attr">${attr}</span>=<span class="syntax-attr-value">${val}</span>`);
      }
      result += `${selfClose}&gt;`;
      return result;
    });
  pre.innerHTML = highlighted;
  return pre;
}

// --- YAML Syntax Highlighter ---
function renderYamlHighlighted(yamlString) {
  const pre = document.createElement('pre');
  pre.className = 'syntax-plain';
  const lines = yamlString.split('\n');
  const highlighted = lines.map(line => {
    const escaped = escapeHtml(line);
    if (/^\s*#/.test(line)) return `<span class="yaml-comment">${escaped}</span>`;
    const keyMatch = escaped.match(/^(\s*)(- )?([\w][\w.-]*)(:\s?)(.*)/);
    if (keyMatch) {
      const [, indent, listMarker, key, colon, val] = keyMatch;
      let formattedVal = escapeHtml('');
      const rawVal = val.trim();
      if (!rawVal) formattedVal = val;
      else if (rawVal === 'true' || rawVal === 'false') formattedVal = `<span class="yaml-boolean">${val}</span>`;
      else if (rawVal === 'null' || rawVal === '~') formattedVal = `<span class="yaml-null">${val}</span>`;
      else if (/^-?\d+(\.\d+)?$/.test(rawVal)) formattedVal = `<span class="yaml-number">${val}</span>`;
      else if (/^\s*#/.test(rawVal)) formattedVal = `<span class="yaml-comment">${val}</span>`;
      else formattedVal = `<span class="yaml-string">${val}</span>`;
      return `${indent}${listMarker ? `<span class="yaml-list-marker">${listMarker}</span>` : ''}<span class="yaml-key">${key}</span>${colon}${formattedVal}`;
    }
    if (/^\s*-\s/.test(line)) {
      return escaped.replace(/^(\s*)(- )(.*)/, (m, indent, marker, val) => {
        const rawVal = val.trim();
        let fv;
        if (rawVal === 'true' || rawVal === 'false') fv = `<span class="yaml-boolean">${val}</span>`;
        else if (/^-?\d+(\.\d+)?$/.test(rawVal)) fv = `<span class="yaml-number">${val}</span>`;
        else fv = `<span class="yaml-string">${val}</span>`;
        return `${indent}<span class="yaml-list-marker">${marker}</span>${fv}`;
      });
    }
    return escaped;
  }).join('\n');
  pre.innerHTML = highlighted;
  return pre;
}

// --- CSV Table Renderer ---
function renderCsvTable(csvString) {
  const lines = csvString.trim().split('\n').map(l => l.trim()).filter(Boolean);
  if (lines.length === 0) {
    const pre = document.createElement('pre');
    pre.className = 'syntax-plain';
    pre.textContent = csvString;
    return pre;
  }
  const parseCsvLine = (line) => {
    const cells = [];
    let current = '', inQuotes = false;
    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      if (inQuotes) {
        if (ch === '"' && line[i + 1] === '"') { current += '"'; i++; }
        else if (ch === '"') inQuotes = false;
        else current += ch;
      } else {
        if (ch === '"') inQuotes = true;
        else if (ch === ',') { cells.push(current); current = ''; }
        else current += ch;
      }
    }
    cells.push(current);
    return cells;
  };
  const headers = parseCsvLine(lines[0]);
  const table = document.createElement('table');
  table.className = 'csv-table';
  const thead = document.createElement('thead');
  thead.innerHTML = '<tr>' + headers.map(h => `<th>${escapeHtml(h)}</th>`).join('') + '</tr>';
  table.appendChild(thead);
  const tbody = document.createElement('tbody');
  for (let i = 1; i < lines.length; i++) {
    const cells = parseCsvLine(lines[i]);
    const tr = document.createElement('tr');
    tr.innerHTML = cells.map(c => `<td>${escapeHtml(c)}</td>`).join('');
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  return table;
}

// --- Render response body by content type into a container ---
const BODY_RENDER_THRESHOLD = 262144; // 256 KiB — above this, show preview + load button

function renderResponseBodyInto(container, body, headers) {
  const contentType = detectContentType(body, headers);
  container.innerHTML = '';
  container.dataset.contentType = contentType;

  const badge = document.createElement('span');
  badge.className = 'response-type-badge';
  badge.textContent = contentType.toUpperCase();
  container.appendChild(badge);

  const bodyLen = body ? body.length : 0;

  // Large body guard — show raw preview with option to load full
  if (bodyLen > BODY_RENDER_THRESHOLD) {
    const preview = body.substring(0, 8192); // first 8 KiB
    const pre = document.createElement('pre');
    pre.className = 'syntax-plain';
    pre.textContent = preview + '\n…';
    container.appendChild(pre);
    const info = document.createElement('div');
    info.className = 'body-truncated-info';
    info.innerHTML = `<span>⚠ Response is ${formatBytes(bodyLen)} — showing first 8 KiB preview</span>`;
    const loadBtn = document.createElement('button');
    loadBtn.className = 'btn btn-ghost btn-xs';
    loadBtn.textContent = '📄 Load Full Body (plain text)';
    loadBtn.onclick = () => {
      pre.textContent = body;
      info.remove();
    };
    const renderBtn = document.createElement('button');
    renderBtn.className = 'btn btn-ghost btn-xs';
    renderBtn.textContent = '🎨 Parse & Render (may be slow)';
    renderBtn.onclick = () => {
      info.remove();
      pre.remove();
      renderResponseBodyFull(container, body, contentType);
    };
    info.appendChild(loadBtn);
    info.appendChild(renderBtn);
    container.appendChild(info);
    return;
  }

  renderResponseBodyFull(container, body, contentType);
}

async function renderResponseBodyFull(container, body, contentType) {
  const spinner = document.createElement('div');
  spinner.className = 'body-loading';
  spinner.innerHTML = '<span class="spinner-sm"></span> Rendering...';
  container.appendChild(spinner);

  try {
    if (contentType === 'json') {
      const treeData = await invoke('build_json_tree', { json: body, maxDepth: 3, maxChildren: 100 });
      spinner.remove();
      container.appendChild(renderJsonTreeFromRust(treeData, body));
      try { container.dataset.rawJson = await invoke('sort_and_normalize', { body, contentType: 'json' }); }
      catch { container.dataset.rawJson = body; }
    } else if (contentType === 'xml' || contentType === 'html' || contentType === 'yaml' ||
               contentType === 'csv' || contentType === 'protobuf') {
      // Use Rust format_body for highlightable types
      try {
        const lines = await invoke('format_body', { body, contentType });
        spinner.remove();
        renderVirtualResponseBody(container, lines);
        container.dataset.rawJson = body;
      } catch {
        spinner.remove();
        // Fallback to original JS renderers
        switch (contentType) {
          case 'xml': case 'html': container.appendChild(renderXmlHighlighted(body)); break;
          case 'yaml': container.appendChild(renderYamlHighlighted(body)); break;
          case 'csv': container.appendChild(renderCsvTable(body)); break;
          case 'protobuf': {
            const pre = document.createElement('pre');
            pre.className = 'syntax-plain';
            pre.innerHTML = `<span class="syntax-comment">// Binary protobuf response</span>\n${escapeHtml(body || '')}`;
            container.appendChild(pre);
            break;
          }
        }
        container.dataset.rawJson = body;
      }
    } else {
      // Plain text — use Rust format_body
      try {
        const lines = await invoke('format_body', { body, contentType: 'text' });
        spinner.remove();
        renderVirtualResponseBody(container, lines);
      } catch {
        spinner.remove();
        const pre = document.createElement('pre');
        pre.className = 'syntax-plain';
        pre.textContent = body || '';
        container.appendChild(pre);
      }
      container.dataset.rawJson = body || '';
    }
  } catch (e) {
    spinner.remove();
    // Global fallback — render plain text
    const pre = document.createElement('pre');
    pre.className = 'syntax-plain';
    pre.textContent = body || '';
    container.appendChild(pre);
    container.dataset.rawJson = body || '';
  }
}

// --- Rust JSON Tree Rendering ---

function renderJsonTreeFromRust(node, rawJson) {
  const container = document.createElement('div');
  container.className = 'json-tree';
  container.appendChild(buildJsonNodeEl(node, rawJson));
  return container;
}

function buildJsonNodeEl(node, rawJson) {
  const isComplex = node.node_type === 'object' || node.node_type === 'array';
  const line = document.createElement('div');
  line.className = 'json-line';
  line.style.paddingLeft = `${node.depth * 18}px`;

  if (isComplex) {
    const open = node.node_type === 'array' ? '[' : '{';
    const close = node.node_type === 'array' ? ']' : '}';
    const comma = node.is_last ? '' : ',';
    const hasChildren = node.children.length > 0;

    const toggle = document.createElement('span');
    toggle.className = hasChildren ? 'json-toggle expanded' : 'json-toggle collapsed';
    toggle.textContent = hasChildren ? '▾' : '▸';

    const keySpan = node.key !== null ? `<span class="json-key">"${escapeHtml(String(node.key))}"</span><span class="json-colon">: </span>` : '';
    line.innerHTML = `${keySpan}<span class="json-bracket">${open}</span>`;
    line.insertBefore(toggle, line.firstChild);

    const preview = document.createElement('span');
    preview.className = hasChildren ? 'json-preview hidden' : 'json-preview';
    preview.textContent = ` ${node.child_count} ${node.node_type === 'array' ? 'items' : 'keys'} `;
    line.appendChild(preview);

    const childContainer = document.createElement('div');
    childContainer.className = hasChildren ? 'json-children' : 'json-children hidden';

    node.children.forEach(child => {
      childContainer.appendChild(buildJsonNodeEl(child, rawJson));
    });

    if (node.child_count > node.children.length) {
      const more = document.createElement('div');
      more.className = 'json-line json-load-more';
      more.style.paddingLeft = `${(node.depth + 1) * 18}px`;
      more.innerHTML = `<span class="json-more-btn">▸ ${node.child_count - node.children.length} more items...</span>`;
      more.querySelector('.json-more-btn').addEventListener('click', async () => {
        try {
          const expanded = await invoke('expand_json_node', {
            json: rawJson, path: node.path, maxDepth: 2, maxChildren: 200
          });
          childContainer.innerHTML = '';
          expanded.children.forEach(child => {
            childContainer.appendChild(buildJsonNodeEl(child, rawJson));
          });
        } catch (e) {
          more.innerHTML = `<span class="json-error">Error expanding: ${escapeHtml(String(e))}</span>`;
        }
      });
      childContainer.appendChild(more);
    }

    const closeLine = document.createElement('div');
    closeLine.className = hasChildren ? 'json-line json-close' : 'json-line json-close hidden';
    closeLine.style.paddingLeft = `${node.depth * 18}px`;
    closeLine.innerHTML = `<span class="json-bracket">${close}</span>${comma}`;

    toggle.addEventListener('click', async () => {
      const isExpanded = toggle.classList.contains('expanded');
      if (isExpanded) {
        toggle.classList.remove('expanded');
        toggle.classList.add('collapsed');
        toggle.textContent = '▸';
        childContainer.classList.add('hidden');
        closeLine.classList.add('hidden');
        preview.classList.remove('hidden');
      } else {
        if (childContainer.children.length === 0 && node.child_count > 0) {
          toggle.textContent = '⏳';
          try {
            const expanded = await invoke('expand_json_node', {
              json: rawJson, path: node.path, maxDepth: 2, maxChildren: 100
            });
            expanded.children.forEach(child => {
              childContainer.appendChild(buildJsonNodeEl(child, rawJson));
            });
          } catch (e) {
            console.error('Failed to expand node:', e);
          }
        }
        toggle.classList.add('expanded');
        toggle.classList.remove('collapsed');
        toggle.textContent = '▾';
        childContainer.classList.remove('hidden');
        closeLine.classList.remove('hidden');
        preview.classList.add('hidden');
      }
    });

    const wrapper = document.createDocumentFragment();
    wrapper.appendChild(line);
    wrapper.appendChild(childContainer);
    wrapper.appendChild(closeLine);
    return wrapper;
  } else {
    const keySpan = node.key !== null ? `<span class="json-key">"${escapeHtml(String(node.key))}"</span><span class="json-colon">: </span>` : '';
    const comma = node.is_last ? '' : ',';
    const val = node.value_preview || 'null';
    let cls = 'json-null';
    if (node.node_type === 'string') cls = 'json-string';
    else if (node.node_type === 'number') cls = 'json-number';
    else if (node.node_type === 'boolean') cls = 'json-boolean';
    const displayVal = node.node_type === 'string' ? `"${escapeHtml(val)}"` : escapeHtml(val);

    const spacer = document.createElement('span');
    spacer.className = 'json-toggle-spacer';
    line.innerHTML = `${keySpan}<span class="${cls}">${displayVal}</span>${comma}`;
    line.insertBefore(spacer, line.firstChild);
    return line;
  }
}

// --- Display Response ---
function displayResponse(resp) {
  responseEmpty.classList.add('hidden');
  responseContent.classList.remove('hidden');
  responseContent.classList.add('visible');
  responseMeta.classList.remove('hidden');

  const code = resp.status;
  statusBadge.textContent = `${code} ${resp.status_text}`;
  statusBadge.className = 'status-badge';
  if (code >= 200 && code < 300) statusBadge.classList.add('s2xx');
  else if (code >= 300 && code < 400) statusBadge.classList.add('s3xx');
  else if (code >= 400 && code < 500) statusBadge.classList.add('s4xx');
  else statusBadge.classList.add('s5xx');

  responseTime.textContent = `${resp.time_ms} ms`;
  responseSize.textContent = formatBytes(resp.size_bytes);

  renderResponseBodyInto(responseBody, resp.body, resp.headers);

  responseHeadersBody.innerHTML = '';
  resp.headers.forEach(([k, v]) => {
    const tr = document.createElement('tr');
    tr.innerHTML = `<td>${escapeHtml(k)}</td><td>${escapeHtml(v)}</td>`;
    responseHeadersBody.appendChild(tr);
  });
}

// --- JSON Tree Renderer ---
function renderJsonTree(data) {
  const container = document.createElement('div');
  container.className = 'json-tree';
  container.appendChild(renderJsonNode(data, null, 0, true));
  return container;
}

function renderJsonNode(value, key, depth, isLast) {
  const type = value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value;
  const isComplex = type === 'object' || type === 'array';

  const line = document.createElement('div');
  line.className = 'json-line';
  line.style.paddingLeft = `${depth * 18}px`;

  if (isComplex) {
    const entries = type === 'array' ? value : Object.entries(value);
    const count = type === 'array' ? value.length : Object.keys(value).length;
    const open = type === 'array' ? '[' : '{';
    const close = type === 'array' ? ']' : '}';
    const comma = isLast ? '' : ',';

    const toggle = document.createElement('span');
    toggle.className = 'json-toggle expanded';
    toggle.textContent = '▾';

    const keySpan = key !== null ? `<span class="json-key">"${escapeHtml(String(key))}"</span><span class="json-colon">: </span>` : '';

    line.innerHTML = `${keySpan}<span class="json-bracket">${open}</span>`;
    line.insertBefore(toggle, line.firstChild);

    const preview = document.createElement('span');
    preview.className = 'json-preview hidden';
    preview.textContent = ` ${count} ${type === 'array' ? 'items' : 'keys'} `;
    line.appendChild(preview);

    const childContainer = document.createElement('div');
    childContainer.className = 'json-children';

    if (type === 'array') {
      value.forEach((item, i) => {
        childContainer.appendChild(renderJsonNode(item, null, depth + 1, i === value.length - 1));
      });
    } else {
      const keys = Object.keys(value);
      keys.forEach((k, i) => {
        childContainer.appendChild(renderJsonNode(value[k], k, depth + 1, i === keys.length - 1));
      });
    }

    const closeLine = document.createElement('div');
    closeLine.className = 'json-line json-close';
    closeLine.style.paddingLeft = `${depth * 18}px`;
    closeLine.innerHTML = `<span class="json-bracket">${close}</span>${comma}`;

    toggle.addEventListener('click', () => {
      const isExpanded = toggle.classList.toggle('expanded');
      toggle.classList.toggle('collapsed', !isExpanded);
      toggle.textContent = isExpanded ? '▾' : '▸';
      childContainer.classList.toggle('hidden', !isExpanded);
      closeLine.classList.toggle('hidden', !isExpanded);
      preview.classList.toggle('hidden', isExpanded);
    });

    const wrapper = document.createDocumentFragment();
    wrapper.appendChild(line);
    wrapper.appendChild(childContainer);
    wrapper.appendChild(closeLine);
    return wrapper;
  } else {
    // Primitive value
    const keySpan = key !== null ? `<span class="json-key">"${escapeHtml(String(key))}"</span><span class="json-colon">: </span>` : '';
    const comma = isLast ? '' : ',';
    let valHtml;
    if (type === 'string') {
      valHtml = `<span class="json-string">"${escapeHtml(value)}"</span>`;
    } else if (type === 'number') {
      valHtml = `<span class="json-number">${value}</span>`;
    } else if (type === 'boolean') {
      valHtml = `<span class="json-boolean">${value}</span>`;
    } else {
      valHtml = `<span class="json-null">null</span>`;
    }

    const spacer = document.createElement('span');
    spacer.className = 'json-toggle-spacer';

    line.innerHTML = `${keySpan}${valHtml}${comma}`;
    line.insertBefore(spacer, line.firstChild);
    return line;
  }
}

function displayError(err) {
  responseEmpty.classList.add('hidden');
  responseContent.classList.remove('hidden');
  responseContent.classList.add('visible');
  responseMeta.classList.remove('hidden');

  statusBadge.textContent = 'ERROR';
  statusBadge.className = 'status-badge s-err';
  responseTime.textContent = '\u2014';
  responseSize.textContent = '\u2014';
  responseBody.innerHTML = '';
  responseBody.dataset.rawJson = String(err);
  const pre = document.createElement('pre');
  pre.className = 'json-plain json-error';
  pre.textContent = String(err);
  responseBody.appendChild(pre);
  responseHeadersBody.innerHTML = '';
}

// --- Response toolbar ---
$('#expandAllBtn').addEventListener('click', () => {
  responseBody.querySelectorAll('.json-toggle.collapsed').forEach(t => t.click());
});
$('#collapseAllBtn').addEventListener('click', () => {
  responseBody.querySelectorAll('.json-toggle.expanded').forEach(t => t.click());
});

copyBtn.addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(responseBody.dataset.rawJson || responseBody.dataset.rawBody || responseBody.textContent);
    showToast('Copied to clipboard', 'success');
  } catch {
    showToast('Failed to copy', 'error');
  }
});

// --- File Loading ---
openFileBtn.addEventListener('click', () => fileInput.click());

// Uber-level tooltip on FILES section header
const filesSectionHeader = document.querySelector('#filesSection > .sidebar-section-header');
if (filesSectionHeader) {
  filesSectionHeader.addEventListener('mouseenter', () => {
    clearTimeout(showTooltipTimer);
    clearTimeout(hideTooltipTimer);
    showTooltipTimer = setTimeout(() => showAllFilesTooltip(filesSectionHeader), 300);
  });
  filesSectionHeader.addEventListener('mouseleave', () => {
    hideBlockTooltip();
  });
}

fileInput.addEventListener('change', async (e) => {
  const files = Array.from(e.target.files);
  if (!files.length) return;

  for (const file of files) {
    await loadFile(file);
  }
  fileInput.value = '';
});

async function loadFile(file) {
  const content = await file.text();
  try {
    const suite = await invoke('parse_test_file', { content });

    const fileEntry = {
      name: file.name,
      content,
      suite,
      results: null,
      savedPath: file.path || null,
    };

    loadedFiles.push(fileEntry);
    const fileIdx = loadedFiles.length - 1;
    detectTelemetryConfig();
    detectAzureAuthNeeded();

    // Initialize auto-run from file directive if not already set
    if (!autoRunInterval && suite.auto_run) {
      setAutoRun(suite.auto_run);
    }

    // Sync disabled state from parsed # @disabled directives
    syncDisabledFromSuite(fileIdx);

    renderFileTree();
    renderEnvPanel();

    // Auto-select first request block
    if (suite.blocks && suite.blocks.length > 0) {
      activeFileIndex = fileIdx;
      const firstRequestIdx = suite.blocks.findIndex(b =>
        b.block_type === 'test' || b.block_type === 'request' || b.block_type === 'setup'
      );
      if (firstRequestIdx >= 0) {
        selectBlock(fileIdx, firstRequestIdx);
      }
    }

    showToast(`Loaded ${file.name} (${suite.blocks.length} blocks)`, 'success');
    rpLog('info', 'File loaded: ' + file.name, { blocks: suite.blocks.length });
  } catch (err) {
    showToast(`Parse error: ${err}`, 'error');
    rpLog('error', 'File load failed', err.message || String(err));
  }
}

// --- File Tree Rendering ---
function getBlockIcon(blockType) {
  switch (blockType) {
    case 'setup': return '\u2699\uFE0F';
    case 'test': return '\uD83E\uDDEA';
    case 'teardown': return '\uD83D\uDDD1\uFE0F';
    case 'request': return '\uD83D\uDCE4';
    default: return '\uD83D\uDCC4';
  }
}

function getBlockStatus(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  if (!file || !file.results) return '';
  const br = file.results.block_results?.[blockIdx];
  if (!br) return '';
  return br.status;
}

// --- Block Hover Tooltip ---
let showTooltipTimer = null;
let hideTooltipTimer = null;
const blockTooltip = $('#blockTooltip');

function showBlockTooltip(block, anchorEl, fileIdx, blockIdx) {
  try {
    if (!block || !anchorEl || !blockTooltip) {
      console.warn('[Tooltip] showBlockTooltip: missing', { block: !!block, anchorEl: !!anchorEl, blockTooltip: !!blockTooltip });
      return;
    }
    const typeIcon = block.compare ? '\u21C4' : getBlockIcon(block.block_type);
    const typeLabel = block.compare ? 'Compare' : (block.block_type || 'request').charAt(0).toUpperCase() + (block.block_type || 'request').slice(1);

  let html = `<div class="btt-header">
    <span class="btt-type-icon">${typeIcon}</span>
    <span class="btt-type-label">${escapeHtml(typeLabel)}</span>
    <span class="btt-name">${escapeHtml(block.name || 'Unnamed')}</span>
  </div>`;

  if (block.description) {
    html += `<div class="btt-desc">${escapeHtml(block.description)}</div>`;
  }

  if (block.request) {
    const methodClass = `method-${block.request.method.toLowerCase()}`;
    html += `<div class="btt-section">
      <span class="btt-method ${methodClass}">${escapeHtml(block.request.method)}</span>
      <span class="btt-url">${escapeHtml(block.request.url)}</span>
    </div>`;

    if (block.request.headers && block.request.headers.length > 0) {
      html += `<div class="btt-section">
        <div class="btt-section-title">Headers <span class="btt-count">${block.request.headers.length}</span></div>`;
      block.request.headers.forEach(([k, v]) => {
        html += `<div class="btt-kv"><span class="btt-key">${escapeHtml(k)}</span>: <span class="btt-val">${escapeHtml(v)}</span></div>`;
      });
      html += `</div>`;
    }

    if (block.request.body) {
      const bodyPreview = block.request.body.length > 200
        ? block.request.body.substring(0, 200) + '\u2026'
        : block.request.body;
      html += `<div class="btt-section">
        <div class="btt-section-title">Body</div>
        <pre class="btt-body">${escapeHtml(bodyPreview)}</pre>
      </div>`;
    }
  }

  // Compare block: show steps side-by-side and diff results
  if (block.compare && block.steps && block.steps.length > 0) {
    html += `<div class="btt-section"><div class="btt-section-title">\u21C4 Steps <span class="btt-count">${block.steps.length}</span></div>`;
    block.steps.forEach((step, si) => {
      const methodClass = step.request ? `method-${step.request.method.toLowerCase()}` : '';
      html += `<div class="btt-compare-step">
        <div class="btt-step-header">
          <span class="btt-step-name">${escapeHtml(step.name)}</span>
          ${step.request ? `<span class="btt-method ${methodClass}">${escapeHtml(step.request.method)}</span>` : ''}
        </div>
        ${step.request ? `<div class="btt-step-url">${escapeHtml(step.request.url)}</div>` : ''}`;
      if (step.assertions && step.assertions.length > 0) {
        html += `<div class="btt-step-asserts">${step.assertions.length} assertion${step.assertions.length > 1 ? 's' : ''}</div>`;
      }
      html += `</div>`;
    });
    html += `</div>`;

    // Show request differences between steps
    if (block.steps.length >= 2) {
      const s0 = block.steps[0];
      const s1 = block.steps[1];
      let hasDiffs = false;
      let diffHtml = `<div class="btt-section"><div class="btt-section-title">Request Differences</div><div class="btt-req-diff">`;

      if (s0.request && s1.request) {
        // Method
        if (s0.request.method !== s1.request.method) {
          hasDiffs = true;
          const mc0 = `method-${s0.request.method.toLowerCase()}`;
          const mc1 = `method-${s1.request.method.toLowerCase()}`;
          diffHtml += `<div class="btt-rd-row"><span class="btt-rd-label">Method</span><span class="btt-method ${mc0}">${escapeHtml(s0.request.method)}</span><span class="btt-rd-arrow">\u2192</span><span class="btt-method ${mc1}">${escapeHtml(s1.request.method)}</span></div>`;
        }
        // URL
        if (s0.request.url !== s1.request.url) {
          hasDiffs = true;
          diffHtml += `<div class="btt-rd-row btt-rd-url"><span class="btt-rd-label">URL</span><div class="btt-rd-urls"><div class="btt-rd-old">${escapeHtml(s0.request.url)}</div><div class="btt-rd-new">${escapeHtml(s1.request.url)}</div></div></div>`;
        }
        // Headers
        const h0 = new Map((s0.request.headers || []).map(([k,v]) => [k, v]));
        const h1 = new Map((s1.request.headers || []).map(([k,v]) => [k, v]));
        const allKeys = new Set([...h0.keys(), ...h1.keys()]);
        const headerDiffs = [];
        allKeys.forEach(k => {
          const v0 = h0.get(k), v1 = h1.get(k);
          if (v0 === undefined) headerDiffs.push({ key: k, type: 'added', val: v1 });
          else if (v1 === undefined) headerDiffs.push({ key: k, type: 'removed', val: v0 });
          else if (v0 !== v1) headerDiffs.push({ key: k, type: 'changed', from: v0, to: v1 });
        });
        if (headerDiffs.length > 0) {
          hasDiffs = true;
          diffHtml += `<div class="btt-rd-row"><span class="btt-rd-label">Headers</span><div class="btt-rd-headers">`;
          headerDiffs.forEach(hd => {
            if (hd.type === 'added') diffHtml += `<div class="btt-rd-h btt-rd-h-add"><span class="btt-rd-h-icon">+</span> ${escapeHtml(hd.key)}: ${escapeHtml(hd.val)}</div>`;
            else if (hd.type === 'removed') diffHtml += `<div class="btt-rd-h btt-rd-h-rm"><span class="btt-rd-h-icon">\u2212</span> ${escapeHtml(hd.key)}: ${escapeHtml(hd.val)}</div>`;
            else diffHtml += `<div class="btt-rd-h btt-rd-h-chg"><span class="btt-rd-h-icon">\u0394</span> ${escapeHtml(hd.key)}: ${escapeHtml(hd.from)} \u2192 ${escapeHtml(hd.to)}</div>`;
          });
          diffHtml += `</div></div>`;
        }
        // Body
        const hasBodyA = !!s0.request.body, hasBodyB = !!s1.request.body;
        if (hasBodyA !== hasBodyB) {
          hasDiffs = true;
          diffHtml += `<div class="btt-rd-row"><span class="btt-rd-label">Body</span><span class="${hasBodyA ? 'btt-rd-old' : 'btt-rd-new'}">${hasBodyA ? 'present \u2192 none' : 'none \u2192 present'}</span></div>`;
        } else if (hasBodyA && hasBodyB && s0.request.body !== s1.request.body) {
          hasDiffs = true;
          diffHtml += `<div class="btt-rd-row"><span class="btt-rd-label">Body</span><span class="btt-rd-chg">different content</span></div>`;
        }
      }

      diffHtml += `</div></div>`;
      if (hasDiffs) {
        html += diffHtml;
      } else {
        html += `<div class="btt-section"><div class="btt-section-title">Request Differences</div><div class="btt-diff-item" style="opacity:0.6">Identical requests (comparing response differences)</div></div>`;
      }
    }

    // Diff directive info
    if (block.diff) {
      html += `<div class="btt-section"><div class="btt-section-title">Diff</div>
        <div class="btt-diff-item">\u21C4 Compare: <strong>${escapeHtml(block.diff.step_a)}</strong> vs <strong>${escapeHtml(block.diff.step_b)}</strong></div>
      </div>`;
    }

    // Show diff results if block has been run
    const file = fileIdx !== undefined ? loadedFiles[fileIdx] : null;
    const br = file?.results?.block_results?.[blockIdx];
    if (br?.diff_result) {
      const diff = br.diff_result;
      html += `<div class="btt-section"><div class="btt-section-title">Comparison Result</div>
        <div class="btt-diff-result">
          <span class="btt-diff-badge ${diff.match_exact ? 'btt-match' : 'btt-mismatch'}">${diff.match_exact ? '\u2713 Exact Match' : `${(diff.similarity * 100).toFixed(1)}% Similar`}</span>
          <span class="btt-diff-type">${diff.is_json ? 'JSON' : 'Text'}</span>
        </div>`;
      if (!diff.match_exact) {
        const parts = [];
        if (diff.added_count) parts.push(`<span class="btt-diff-added">+${diff.added_count} added</span>`);
        if (diff.removed_count) parts.push(`<span class="btt-diff-removed">-${diff.removed_count} removed</span>`);
        if (diff.changed_count) parts.push(`<span class="btt-diff-changed">\u0394${diff.changed_count} changed</span>`);
        if (parts.length > 0) html += `<div class="btt-diff-counts">${parts.join(' ')}</div>`;
        // Show first few changed paths
        if (diff.changed_paths && diff.changed_paths.length > 0) {
          const preview = diff.changed_paths.slice(0, 5);
          html += `<div class="btt-diff-paths">`;
          preview.forEach(cp => {
            html += `<div class="btt-diff-path">${escapeHtml(cp.path)}: ${escapeHtml(String(cp.left ?? ''))} \u2192 ${escapeHtml(String(cp.right ?? ''))}</div>`;
          });
          if (diff.changed_paths.length > 5) html += `<div class="btt-diff-path" style="opacity:0.6">...and ${diff.changed_paths.length - 5} more</div>`;
          html += `</div>`;
        }
      }
      html += `</div>`;
    }
  }

  if (block.assertions && block.assertions.length > 0) {
    html += `<div class="btt-section">
      <div class="btt-section-title">Assertions <span class="btt-count">${block.assertions.length}</span></div>`;
    block.assertions.forEach(a => {
      html += `<div class="btt-assertion">
        <span class="btt-assert-icon">\u25CF</span>
        <span class="btt-assert-text">${escapeHtml(a.left)} ${escapeHtml(a.operator)} ${escapeHtml(a.right)}</span>
      </div>`;
    });
    html += `</div>`;
  }

  if (block.extracts && block.extracts.length > 0) {
    html += `<div class="btt-section">
      <div class="btt-section-title">Extracts <span class="btt-count">${block.extracts.length}</span></div>`;
    block.extracts.forEach(e => {
      html += `<div class="btt-extract">
        <span class="btt-extract-var">${escapeHtml(e.variable_name)}</span>
        <span class="btt-extract-arrow">\u2190</span>
        <span class="btt-extract-path">${escapeHtml(e.source_path)}</span>
      </div>`;
    });
    html += `</div>`;
  }

  // Run history trend for this block
  const fileForBlock = loadedFiles.find(f => f.suite.blocks.includes(block));
  if (fileForBlock) {
    const blockHist = getBlockRunHistory(fileForBlock.name, block.name);
    if (blockHist.length > 0) {
      const passed = blockHist.filter(h => h.status === 'passed').length;
      const failed = blockHist.filter(h => h.status === 'failed' || h.status === 'error').length;
      const passRate = Math.round((passed / blockHist.length) * 100);
      const avgTime = Math.round(blockHist.reduce((s, h) => s + h.timeMs, 0) / blockHist.length);

      html += `<div class="btt-section">
        <div class="btt-section-title">Run History <span class="btt-count">${blockHist.length} runs</span></div>
        <div class="btt-run-stats">
          <span class="btt-stat">✓ ${passed}</span>
          <span class="btt-stat btt-stat-fail">✗ ${failed}</span>
          <span class="btt-stat">${passRate}%</span>
          <span class="btt-stat">avg ${avgTime}ms</span>
        </div>
        <div class="btt-trend">${buildBlockTrendBar(blockHist)}</div>
      </div>`;
    }
  }

  if (block.group || (block.depends && block.depends.length > 0)) {
    html += `<div class="btt-section btt-meta">`;
    if (block.group) {
      html += `<div class="btt-meta-item"><span class="btt-meta-icon">\u229E</span> Group: <strong>${escapeHtml(block.group)}</strong></div>`;
    }
    if (block.depends && block.depends.length > 0) {
      html += `<div class="btt-meta-item"><span class="btt-meta-icon">\u2937</span> Depends: <strong>${escapeHtml(block.depends.join(', '))}</strong></div>`;
    }
    html += `</div>`;
  }

  blockTooltip.innerHTML = html;
  blockTooltip.classList.remove('hidden');
  blockTooltip.style.display = 'block';
  positionBlockTooltip(anchorEl);
  } catch (e) {
    console.error('[Tooltip] showBlockTooltip error:', e);
  }
}

function hideBlockTooltip() {
  clearTimeout(showTooltipTimer);
  clearTimeout(hideTooltipTimer);
  hideTooltipTimer = setTimeout(() => {
    if (blockTooltip && !blockTooltip.matches(':hover')) {
      blockTooltip.classList.add('hidden');
      blockTooltip.style.display = '';
    }
  }, 200);
}

// Keep tooltip open while mouse is over it (for scrolling)
if (blockTooltip) {
  blockTooltip.addEventListener('mouseleave', () => {
    blockTooltip.classList.add('hidden');
    blockTooltip.style.display = '';
  });
}

/**
 * Attach hover-to-preview tooltip handlers to a `.var-row` element. The
 * `getVar` callback is invoked on each hover and should return [name, value]
 * — this lets the tooltip read the *current* input values for active rows
 * (which the user may be editing) rather than a stale closure capture.
 */
function attachVarRowTooltip(row, getVar) {
  if (!row || !blockTooltip) return;
  row.addEventListener('mouseenter', () => {
    const [name, value] = getVar() || [];
    if (!value) return;
    clearTimeout(showTooltipTimer);
    clearTimeout(hideTooltipTimer);
    showTooltipTimer = setTimeout(() => {
      try {
        blockTooltip.innerHTML = buildVarTooltipHtml(name || '', String(value));
        blockTooltip.classList.remove('hidden');
        blockTooltip.style.display = 'block';
        positionBlockTooltip(row);
      } catch (e) {
        console.error('[Tooltip] var hover error:', e);
      }
    }, 300);
  });
  row.addEventListener('mouseleave', () => hideBlockTooltip());
}

function extractFileHeader(content) {
  const lines = content.split('\n');
  const headerLines = [];
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith('#')) {
      const clean = trimmed.replace(/^#+\s*/, '').replace(/^[=\-]+$/, '').trim();
      if (clean) headerLines.push(clean);
    } else if (trimmed === '' && headerLines.length > 0) {
      continue;
    } else if (trimmed === '') {
      continue;
    } else {
      break;
    }
  }
  return headerLines.slice(0, 5).join('\n') || null;
}

function buildTrendBar(history) {
  const recent = history.slice(-20);
  return `<div class="btt-trend-bar">${recent.map(r => {
    const allPassed = r.failed === 0;
    const cls = allPassed ? 'btt-trend-pass' : 'btt-trend-fail';
    const title = `${r.passed}✓ ${r.failed}✗ — ${r.totalTime}ms`;
    return `<span class="${cls}" title="${title}"></span>`;
  }).join('')}</div>`;
}

function buildBlockTrendBar(blockHist) {
  const recent = blockHist.slice(-20);
  return `<div class="btt-trend-bar">${recent.map(h => {
    const cls = h.status === 'passed' ? 'btt-trend-pass' : h.status === 'failed' ? 'btt-trend-fail' : 'btt-trend-skip';
    const title = `${h.status} — ${h.timeMs}ms`;
    return `<span class="${cls}" title="${title}"></span>`;
  }).join('')}</div>`;
}

function formatTimeAgo(timestamp) {
  const diff = Date.now() - timestamp;
  if (diff < 60000) return 'just now';
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
  if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
  return `${Math.floor(diff / 86400000)}d ago`;
}

function positionBlockTooltip(anchorEl) {
  const rect = anchorEl.getBoundingClientRect();
  // Force layout by reading dimensions after showing tooltip
  const tooltipRect = blockTooltip.getBoundingClientRect();
  let top = rect.top + (rect.height / 2) - (tooltipRect.height / 2);
  let left = rect.right + 8;
  if (top < 8) top = 8;
  if (top + tooltipRect.height > window.innerHeight - 8) top = window.innerHeight - tooltipRect.height - 8;
  if (left + tooltipRect.width > window.innerWidth - 8) left = rect.left - tooltipRect.width - 8;
  if (left < 8) left = 8;
  if (top < 8) top = 8;
  blockTooltip.style.top = `${top}px`;
  blockTooltip.style.left = `${left}px`;
}

function showGroupTooltip(file, fileIdx, groupName, groupBlocks, anchorEl) {
  const totalAssertions = groupBlocks.reduce((s, { block }) => s + (block.assertions?.length || 0), 0);
  const totalExtracts = groupBlocks.reduce((s, { block }) => s + (block.extracts?.length || 0), 0);
  const deps = [...new Set(groupBlocks.flatMap(({ block }) => block.depends || []))];

  let html = `<div class="btt-header">
    <span class="btt-type-icon">\u229E</span>
    <span class="btt-type-label">Group</span>
    <span class="btt-name">${escapeHtml(groupName)}</span>
  </div>`;

  html += `<div class="btt-section">
    <div class="btt-file-summary">
      <span class="btt-badge btt-badge-test">\uD83E\uDDEA ${groupBlocks.length} Tests</span>
      ${totalAssertions > 0 ? `<span class="btt-badge btt-badge-setup">✓ ${totalAssertions} Assertions</span>` : ''}
      ${totalExtracts > 0 ? `<span class="btt-badge btt-badge-group">↤ ${totalExtracts} Extracts</span>` : ''}
    </div>
  </div>`;

  if (deps.length > 0) {
    html += `<div class="btt-section btt-meta">
      <div class="btt-meta-item"><span class="btt-meta-icon">\u2937</span> Depends: <strong>${escapeHtml(deps.join(', '))}</strong></div>
    </div>`;
  }

  html += `<div class="btt-section"><div class="btt-section-title">Tests <span class="btt-count">${groupBlocks.length}</span></div>`;
  groupBlocks.forEach(({ block }) => {
    const blockHist = getBlockRunHistory(file.name, block.name);
    const lastStatus = blockHist.length > 0 ? blockHist[blockHist.length - 1].status : null;
    const statusDot = lastStatus === 'passed' ? '\uD83D\uDFE2' : lastStatus === 'failed' ? '\uD83D\uDD34' : lastStatus === 'error' ? '\uD83D\uDFE0' : '\u26AA';
    const timeStr = blockHist.length > 0 ? ` (${blockHist[blockHist.length - 1].timeMs}ms)` : '';
    html += `<div class="btt-test-item">${statusDot} <span class="btt-test-name">${escapeHtml(block.name)}</span><span class="btt-test-time">${timeStr}</span></div>`;
  });
  html += `</div>`;

  // Aggregate run history across all blocks in this group
  const allBlockHists = groupBlocks.map(({ block }) => getBlockRunHistory(file.name, block.name));
  const maxRuns = Math.max(...allBlockHists.map(h => h.length), 0);
  if (maxRuns > 0) {
    let totalPassed = 0, totalFailed = 0;
    allBlockHists.forEach(hist => {
      totalPassed += hist.filter(h => h.status === 'passed').length;
      totalFailed += hist.filter(h => h.status === 'failed' || h.status === 'error').length;
    });
    const total = totalPassed + totalFailed;
    const passRate = total > 0 ? Math.round((totalPassed / total) * 100) : 0;
    const avgTime = Math.round(allBlockHists.flat().reduce((s, h) => s + h.timeMs, 0) / (allBlockHists.flat().length || 1));

    // Build per-run trend (each bar = one run across all group blocks)
    const runTrend = [];
    for (let i = 0; i < maxRuns; i++) {
      let passed = 0, failed = 0;
      allBlockHists.forEach(hist => {
        if (i < hist.length) {
          if (hist[i].status === 'passed') passed++;
          else failed++;
        }
      });
      runTrend.push({ passed, failed, totalTime: 0 });
    }

    html += `<div class="btt-section">
      <div class="btt-section-title">Run History <span class="btt-count">${maxRuns} runs</span></div>
      <div class="btt-run-stats">
        <span class="btt-stat">✓ ${totalPassed}</span>
        <span class="btt-stat btt-stat-fail">✗ ${totalFailed}</span>
        <span class="btt-stat">${passRate}%</span>
        <span class="btt-stat">avg ${avgTime}ms</span>
      </div>
      <div class="btt-trend">${buildTrendBar(runTrend)}</div>
    </div>`;
  }

  blockTooltip.innerHTML = html;
  blockTooltip.classList.remove('hidden');
  positionBlockTooltip(anchorEl);
}

function showAllFilesTooltip(anchorEl) {
  if (loadedFiles.length === 0) return;

  let totalBlocks = 0, totalSetups = 0, totalTests = 0, totalTeardowns = 0;
  let totalGroups = new Set();
  let totalRuns = 0, totalPassed = 0, totalFailed = 0;

  loadedFiles.forEach(file => {
    const blocks = file.suite.blocks;
    totalBlocks += blocks.length;
    totalSetups += blocks.filter(b => b.block_type === 'setup').length;
    totalTests += blocks.filter(b => b.block_type === 'test').length;
    totalTeardowns += blocks.filter(b => b.block_type === 'teardown').length;
    blocks.forEach(b => { if (b.group) totalGroups.add(b.group); });

    const stats = getFileRunStats(file.name);
    totalRuns += stats.totalRuns;
    stats.history.forEach(r => { totalPassed += r.passed; totalFailed += r.failed; });
  });

  let html = `<div class="btt-header">
    <span class="btt-type-icon">\uD83D\uDCDA</span>
    <span class="btt-type-label">Workspace</span>
    <span class="btt-name">${loadedFiles.length} Files Loaded</span>
  </div>`;

  html += `<div class="btt-section">
    <div class="btt-file-summary">
      <span class="btt-badge btt-badge-setup">⚙ ${totalSetups} Setup</span>
      <span class="btt-badge btt-badge-test">\uD83E\uDDEA ${totalTests} Tests</span>
      <span class="btt-badge btt-badge-teardown">\uD83D\uDDD1 ${totalTeardowns} Teardown</span>
      ${totalGroups.size > 0 ? `<span class="btt-badge btt-badge-group">\u229E ${totalGroups.size} Groups</span>` : ''}
    </div>
  </div>`;

  html += `<div class="btt-section"><div class="btt-section-title">Files</div>`;
  loadedFiles.forEach(file => {
    const stats = getFileRunStats(file.name);
    const blockCount = file.suite.blocks.length;
    const lastStatus = stats.totalRuns > 0 ? (stats.history[stats.history.length - 1].failed === 0 ? '\uD83D\uDFE2' : '\uD83D\uDD34') : '\u26AA';
    html += `<div class="btt-test-item">
      ${lastStatus} <span class="btt-test-name">${escapeHtml(file.name)}</span>
      <span class="btt-test-time">${blockCount} blocks${stats.totalRuns > 0 ? ` · ${stats.totalRuns} runs` : ''}</span>
    </div>`;
  });
  html += `</div>`;

  if (totalRuns > 0) {
    const total = totalPassed + totalFailed;
    const passRate = total > 0 ? Math.round((totalPassed / total) * 100) : 0;

    // Aggregate per-run trend across all files
    const allHistory = loadedFiles.flatMap(f => (getFileRunStats(f.name).history || []).map(h => ({ ...h, file: f.name })));
    allHistory.sort((a, b) => a.timestamp - b.timestamp);

    html += `<div class="btt-section">
      <div class="btt-section-title">Total Run History <span class="btt-count">${totalRuns} runs</span></div>
      <div class="btt-run-stats">
        <span class="btt-stat">✓ ${totalPassed}</span>
        <span class="btt-stat btt-stat-fail">✗ ${totalFailed}</span>
        <span class="btt-stat">${passRate}% pass rate</span>
      </div>
      <div class="btt-trend">${buildTrendBar(allHistory.slice(-20))}</div>
    </div>`;
  }

  blockTooltip.innerHTML = html;
  blockTooltip.classList.remove('hidden');
  positionBlockTooltip(anchorEl);
}

function showFileTooltip(file, fileIdx, anchorEl) {
  const suite = file.suite;
  const stats = getFileRunStats(file.name);

  const headerComment = extractFileHeader(file.content);

  const setups = suite.blocks.filter(b => b.block_type === 'setup');
  const tests = suite.blocks.filter(b => b.block_type === 'test');
  const teardowns = suite.blocks.filter(b => b.block_type === 'teardown');
  const groups = [...new Set(tests.map(b => b.group).filter(Boolean))];

  let html = `<div class="btt-header">
    <span class="btt-type-icon">📄</span>
    <span class="btt-name">${escapeHtml(file.name)}</span>
  </div>`;

  if (headerComment) {
    html += `<div class="btt-desc">${escapeHtml(headerComment)}</div>`;
  }

  html += `<div class="btt-section">
    <div class="btt-section-title">Test Plan</div>
    <div class="btt-file-summary">
      ${setups.length > 0 ? `<span class="btt-badge btt-badge-setup">⚙ ${setups.length} Setup</span>` : ''}
      ${tests.length > 0 ? `<span class="btt-badge btt-badge-test">🧪 ${tests.length} Tests</span>` : ''}
      ${teardowns.length > 0 ? `<span class="btt-badge btt-badge-teardown">🧹 ${teardowns.length} Teardown</span>` : ''}
      ${groups.length > 0 ? `<span class="btt-badge btt-badge-group">⊞ ${groups.length} Groups</span>` : ''}
    </div>
  </div>`;

  if (tests.length > 0) {
    html += `<div class="btt-section"><div class="btt-section-title">Tests <span class="btt-count">${tests.length}</span></div>`;

    const grouped = {};
    const ungrouped = [];
    tests.forEach(t => {
      if (t.group) {
        if (!grouped[t.group]) grouped[t.group] = [];
        grouped[t.group].push(t);
      } else {
        ungrouped.push(t);
      }
    });

    for (const [groupName, groupTests] of Object.entries(grouped)) {
      html += `<div class="btt-test-group-label">⊞ ${escapeHtml(groupName)}</div>`;
      groupTests.forEach(t => {
        const blockHist = getBlockRunHistory(file.name, t.name);
        const lastStatus = blockHist.length > 0 ? blockHist[blockHist.length - 1].status : null;
        const statusDot = lastStatus === 'passed' ? '🟢' : lastStatus === 'failed' ? '🔴' : lastStatus === 'error' ? '🟠' : '⚪';
        html += `<div class="btt-test-item">${statusDot} <span class="btt-test-name">${escapeHtml(t.name)}</span></div>`;
      });
    }
    ungrouped.forEach(t => {
      const blockHist = getBlockRunHistory(file.name, t.name);
      const lastStatus = blockHist.length > 0 ? blockHist[blockHist.length - 1].status : null;
      const statusDot = lastStatus === 'passed' ? '🟢' : lastStatus === 'failed' ? '🔴' : lastStatus === 'error' ? '🟠' : '⚪';
      html += `<div class="btt-test-item">${statusDot} <span class="btt-test-name">${escapeHtml(t.name)}</span></div>`;
    });
    html += `</div>`;
  }

  if (stats.totalRuns > 0) {
    const lastRun = stats.history[stats.history.length - 1];
    const totalPassed = stats.history.reduce((sum, r) => sum + r.passed, 0);
    const totalFailed = stats.history.reduce((sum, r) => sum + r.failed, 0);
    const passRate = Math.round((totalPassed / (totalPassed + totalFailed || 1)) * 100);

    html += `<div class="btt-section">
      <div class="btt-section-title">Run History <span class="btt-count">${stats.totalRuns} runs</span></div>
      <div class="btt-run-stats">
        <span class="btt-stat">✓ ${totalPassed} passed</span>
        <span class="btt-stat btt-stat-fail">✗ ${totalFailed} failed</span>
        <span class="btt-stat">${passRate}% pass rate</span>
      </div>
      <div class="btt-trend">${buildTrendBar(stats.history)}</div>
      <div class="btt-last-run">Last: ${formatTimeAgo(lastRun.timestamp)} — ${lastRun.passed}✓ ${lastRun.failed}✗ in ${lastRun.totalTime}ms</div>
    </div>`;
  }

  blockTooltip.innerHTML = html;
  blockTooltip.classList.remove('hidden');
  positionBlockTooltip(anchorEl);
}

// Persistent expand/collapse state: { "file-0": true, "file-0-grp-auth": false, ... }
const treeExpandState = {};

function renderFileTree() {
  if (loadedFiles.length === 0) {
    fileTree.innerHTML = '<div class="sidebar-empty">No files loaded</div>';
    return;
  }

  // Save current expand state from DOM before rebuilding
  fileTree.querySelectorAll('.file-node').forEach(fn => {
    const key = `file-${fn.dataset.fileIdx}`;
    treeExpandState[key] = fn.classList.contains('expanded');
  });
  fileTree.querySelectorAll('.group-node').forEach(gn => {
    const key = gn.dataset.expandKey;
    if (key) treeExpandState[key] = gn.classList.contains('expanded');
  });

  fileTree.innerHTML = '';
  loadedFiles.forEach((file, fileIdx) => {
    const fileKey = `file-${fileIdx}`;
    const isFileExpanded = treeExpandState[fileKey] !== undefined ? treeExpandState[fileKey] : true;

    const node = document.createElement('div');
    node.className = `file-node${isFileExpanded ? ' expanded' : ''}`;
    node.dataset.fileIdx = fileIdx;

    const unsavedDot = file.savedPath ? '' : '<span class="node-unsaved" title="Unsaved">●</span>';
    const header = document.createElement('div');
    header.className = 'file-node-header';
    header.innerHTML = `
      <span class="node-chevron">\u25B8</span>
      <span class="node-icon">\uD83D\uDCC4</span>
      <span class="node-name" title="${escapeAttr(file.name)}">${escapeHtml(file.name)}${unsavedDot}</span>
      <button class="node-run" title="Run this file">\u25B6</button>
      <button class="node-close" title="Close file">\u2715</button>
    `;

    header.addEventListener('click', (e) => {
      if (e.target.closest('.node-run')) {
        runSingleFile(fileIdx);
        return;
      }
      if (e.target.closest('.node-close')) {
        closeFile(fileIdx);
        return;
      }
      if (currentMode === 'code') {
        activeFileIndex = fileIdx;
        syncBuilderToCode();
      }
      node.classList.toggle('expanded');
      treeExpandState[fileKey] = node.classList.contains('expanded');
    });

    header.addEventListener('mouseenter', () => {
      clearTimeout(showTooltipTimer);
      clearTimeout(hideTooltipTimer);
      showTooltipTimer = setTimeout(() => showFileTooltip(file, fileIdx, header), 300);
    });
    header.addEventListener('mouseleave', () => {
      hideBlockTooltip();
    });

    const children = document.createElement('div');
    children.className = 'file-node-children';

    // Variables row
    if (file.suite.variables && file.suite.variables.length > 0) {
      const varItem = document.createElement('div');
      varItem.className = 'block-item';
      varItem.innerHTML = `
        <span class="block-status"></span>
        <span class="block-icon">\uD83D\uDD27</span>
        <span class="block-name">@variables (${file.suite.variables.length})</span>
      `;
      varItem.addEventListener('click', () => {
        $('#envSection').classList.add('expanded');
      });
      children.appendChild(varItem);
    }

    // Categorize blocks: setup, teardown, grouped tests, ungrouped tests
    const setups = [], teardowns = [], ungroupedTests = [];
    const groupMap = new Map(); // groupName -> [{block, blockIdx}]
    file.suite.blocks.forEach((block, blockIdx) => {
      if (block.block_type === 'setup') {
        setups.push({ block, blockIdx });
      } else if (block.block_type === 'teardown') {
        teardowns.push({ block, blockIdx });
      } else if (block.group) {
        if (!groupMap.has(block.group)) groupMap.set(block.group, []);
        groupMap.get(block.group).push({ block, blockIdx });
      } else {
        ungroupedTests.push({ block, blockIdx });
      }
    });

    // Render setup blocks at top level
    setups.forEach(({ block, blockIdx }) => {
      children.appendChild(createBlockItem(file, fileIdx, block, blockIdx));
    });

    // Render grouped tests as collapsible group nodes
    for (const [groupName, groupBlocks] of groupMap) {
      const groupKey = `file-${fileIdx}-grp-${groupName}`;
      const isGroupExpanded = treeExpandState[groupKey] !== undefined ? treeExpandState[groupKey] : false;

      const groupNode = document.createElement('div');
      groupNode.className = `group-node${isGroupExpanded ? ' expanded' : ''}`;
      groupNode.dataset.expandKey = groupKey;
      groupNode.dataset.groupName = groupName;

      const groupHeader = document.createElement('div');
      groupHeader.className = 'group-node-header';
      groupHeader.innerHTML = `
        <span class="node-chevron">\u25B8</span>
        <span class="group-status"></span>
        <span class="group-icon">\u229E</span>
        <span class="group-name" title="${escapeAttr(groupName)}">${escapeHtml(groupName)}</span>
        <span class="group-count">${groupBlocks.length}</span>
        <button class="group-run" title="Run this group">\u25B6</button>
      `;

      groupHeader.addEventListener('click', (e) => {
        if (e.target.closest('.group-run')) {
          runGroup(fileIdx, groupName);
          return;
        }
        groupNode.classList.toggle('expanded');
        treeExpandState[groupKey] = groupNode.classList.contains('expanded');
      });

      groupHeader.addEventListener('mouseenter', () => {
        clearTimeout(showTooltipTimer);
        clearTimeout(hideTooltipTimer);
        showTooltipTimer = setTimeout(() => showGroupTooltip(file, fileIdx, groupName, groupBlocks, groupHeader), 300);
      });
      groupHeader.addEventListener('mouseleave', () => {
        hideBlockTooltip();
      });

      const groupChildren = document.createElement('div');
      groupChildren.className = 'group-node-children';
      groupBlocks.forEach(({ block, blockIdx }) => {
        groupChildren.appendChild(createBlockItem(file, fileIdx, block, blockIdx));
      });

      groupNode.appendChild(groupHeader);
      groupNode.appendChild(groupChildren);
      children.appendChild(groupNode);
    }

    // Render ungrouped test blocks
    ungroupedTests.forEach(({ block, blockIdx }) => {
      children.appendChild(createBlockItem(file, fileIdx, block, blockIdx));
    });

    // Render teardown blocks at bottom
    teardowns.forEach(({ block, blockIdx }) => {
      children.appendChild(createBlockItem(file, fileIdx, block, blockIdx));
    });

    node.appendChild(header);
    node.appendChild(children);
    fileTree.appendChild(node);
  });
}

function createBlockItem(file, fileIdx, block, blockIdx) {
  const item = document.createElement('div');
  item.className = 'block-item';
  item.dataset.blockIdx = blockIdx;
  const isDisabled = !!disabledBlocks[`${fileIdx}-${blockIdx}`];
  if (isDisabled) item.classList.add('disabled');
  if (fileIdx === activeFileIndex && blockIdx === activeBlockIndex) {
    item.classList.add('active');
  }

  // Mode badge and dimming
  if (block.mode) {
    item.dataset.blockMode = block.mode;
    const isAzureAuth = azureAuthState === 'authenticated' || azureAuthState === 'expired';
    if (isAzureAuth && block.mode === 'app') item.classList.add('mode-dimmed');
    if (!isAzureAuth && block.mode === 'dev') item.classList.add('mode-dimmed');
  }

  const status = getBlockStatus(fileIdx, blockIdx);
  const statusClass = status ? `status-${status}` : '';

  const modeBadgeHtml = block.mode ? `<span class="mode-badge mode-${block.mode}">${block.mode.toUpperCase()}</span>` : '';
  const descHtml = block.description ? `<span class="block-desc" title="${escapeAttr(block.description)}">${escapeHtml(block.description)}</span>` : '';
  const depsHtml = block.depends && block.depends.length > 0 ? `<span class="block-depends" title="Depends: ${escapeAttr(block.depends.join(', '))}">⤷ ${escapeHtml(block.depends.join(', '))}</span>` : '';
  item.innerHTML = `
    <input type="checkbox" class="block-toggle" title="Enable/disable this step" ${isDisabled ? '' : 'checked'}>
    <span class="block-status ${statusClass}"></span>
    <span class="block-icon">${block.compare ? '\u21C4' : getBlockIcon(block.block_type)}</span>
    <span class="block-name-group">
      <span class="block-name" title="${escapeAttr(block.name)}">${escapeHtml(block.name || block.block_type)}</span>
      ${modeBadgeHtml}
      ${descHtml}
      ${depsHtml}
    </span>
    <button class="block-play" title="Run this block">\u25B6</button>
  `;

  const toggle = item.querySelector('.block-toggle');
  toggle.addEventListener('click', (e) => {
    e.stopPropagation();
    const key = `${fileIdx}-${blockIdx}`;
    if (toggle.checked) {
      delete disabledBlocks[key];
    } else {
      disabledBlocks[key] = true;
    }
    item.classList.toggle('disabled', !toggle.checked);
    if (currentMode === 'code' && fileIdx === activeFileIndex) {
      syncBuilderToCode();
    }
  });

  item.addEventListener('click', (e) => {
    if (e.target.closest('.block-toggle')) return;
    if (e.target.closest('.block-play')) {
      runSingleBlock(fileIdx, blockIdx);
      return;
    }
    selectBlock(fileIdx, blockIdx);
  });

  // Native title fallback (always visible on hover after ~1s)
  item.title = `${block.block_type}: ${block.name || 'Unnamed'}${block.description ? ' — ' + block.description : ''}`;

  item.addEventListener('mouseenter', () => {
    clearTimeout(showTooltipTimer);
    clearTimeout(hideTooltipTimer);
    showTooltipTimer = setTimeout(() => showBlockTooltip(block, item, fileIdx, blockIdx), 300);
  });
  item.addEventListener('mouseleave', () => {
    hideBlockTooltip();
  });

  // Add compare step sub-items
  if (block.compare && block.steps && block.steps.length > 0) {
    const br = file.results?.block_results?.[blockIdx];
    const stepsContainer = document.createElement('div');
    stepsContainer.className = 'compare-steps';
    block.steps.forEach((step, si) => {
      const stepEl = document.createElement('div');
      stepEl.className = 'compare-step-item';
      const sr = br?.step_results?.[si];
      const stepStatus = sr ? (sr.error ? 'error' : sr.assertion_results?.every(a => a.passed) !== false ? 'passed' : 'failed') : '';
      stepEl.innerHTML = `
        <span class="block-status ${stepStatus ? 'status-' + stepStatus : ''}"></span>
        <span class="step-label">${escapeHtml(step.name)}</span>
        <span class="step-method">${escapeHtml(step.request?.method || '')}</span>
        <button class="step-play" title="Run this step">▶</button>
      `;
      const playBtn = stepEl.querySelector('.step-play');
      playBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        runCompareStep(fileIdx, blockIdx, si);
      });
      stepEl.addEventListener('click', (e) => {
        if (e.target.closest('.step-play')) return;
        viewCompareStep(fileIdx, blockIdx, si);
      });
      stepsContainer.appendChild(stepEl);
    });
    item.appendChild(stepsContainer);
  }

  return item;
}

function closeFile(fileIdx) {
  // Clean up disabled blocks for this file and re-index remaining
  const newDisabled = {};
  Object.keys(disabledBlocks).forEach(key => {
    const [fi, bi] = key.split('-').map(Number);
    if (fi === fileIdx) return; // remove entries for closed file
    const newFi = fi > fileIdx ? fi - 1 : fi;
    newDisabled[`${newFi}-${bi}`] = true;
  });
  disabledBlocks = newDisabled;

  loadedFiles.splice(fileIdx, 1);

  if (activeFileIndex === fileIdx) {
    activeFileIndex = -1;
    activeBlockIndex = -1;
  } else if (activeFileIndex > fileIdx) {
    activeFileIndex--;
  }

  // Clear test results to prevent stale index references
  testResultsDetails.innerHTML = '';
  testResultsBar.classList.add('hidden');

  // Re-index viewedResults: remove closed file entries, shift higher file indices down
  const newViewed = new Map();
  viewedResults.forEach((val, key) => {
    const [fi, bi] = key.split('-').map(Number);
    if (fi === fileIdx) return;
    const newFi = fi > fileIdx ? fi - 1 : fi;
    newViewed.set(`${newFi}-${bi}`, val);
  });
  viewedResults = newViewed;

  renderFileTree();
  renderEnvVars();
  detectAzureAuthNeeded();

  // Recompute auto-run from remaining files
  recomputeAutoRun();
}

// --- Block Metadata Accordion ---
blockMetaHeader.addEventListener('click', () => {
  const expanded = blockMetaAccordion.classList.toggle('expanded');
  blockMetaBody.classList.toggle('hidden', !expanded);
});

/** Populate metadata accordion from the given block. */
function populateMetadata(block) {
  metaName.value = block.name || '';
  metaDescription.value = block.description || '';
  metaBlockType.value = block.block_type || 'test';
  metaGroup.value = block.group || '';
  metaDepends.value = (block.depends || []).join(', ');
  metaMode.value = block.mode || '';
  metaDevAuth.value = block.dev_auth || '';
  metaDisabled.checked = !!block.disabled;
  metaCompare.checked = !!block.compare;

  // Build summary line
  const parts = [];
  if (block.name) parts.push(block.name);
  if (block.block_type && block.block_type !== 'test') parts.push(`[${block.block_type}]`);
  if (block.group) parts.push(`group: ${block.group}`);
  if (block.compare) parts.push('compare');
  blockMetaSummary.textContent = parts.join('  ·  ');
}

/** Read metadata fields back into the active block. Returns true if anything changed. */
function applyMetadataToBlock() {
  if (activeFileIndex < 0 || activeBlockIndex < 0) return false;
  const file = loadedFiles[activeFileIndex];
  if (!file) return false;
  const block = file.suite.blocks[activeBlockIndex];
  if (!block) return false;

  let changed = false;
  const set = (field, val) => { if (block[field] !== val) { block[field] = val; changed = true; } };

  set('name', metaName.value.trim());
  set('description', metaDescription.value.trim());
  set('block_type', metaBlockType.value);
  set('group', metaGroup.value.trim() || null);
  set('mode', metaMode.value || null);
  set('dev_auth', metaDevAuth.value.trim() || null);
  set('disabled', metaDisabled.checked);
  set('compare', metaCompare.checked);

  const newDeps = metaDepends.value.split(',').map(s => s.trim()).filter(Boolean);
  if (JSON.stringify(block.depends) !== JSON.stringify(newDeps)) {
    block.depends = newDeps;
    changed = true;
  }

  return changed;
}

// Auto-apply metadata on field change (debounced)
let metaDebounceTimer = null;
let stepFlushTimer = null;
let directiveFlushTimer = null;
function onMetaFieldChange() {
  clearTimeout(metaDebounceTimer);
  // Capture current indices to avoid race if user switches blocks
  const fi = activeFileIndex, bi = activeBlockIndex;
  metaDebounceTimer = setTimeout(async () => {
    if (fi !== activeFileIndex || bi !== activeBlockIndex) return;
    if (applyMetadataToBlock()) {
      const file = loadedFiles[fi];
      if (file) {
        file.results = null;
        // Regenerate .http content
        try {
          const content = await invoke('generate_http', { suite: file.suite });
          file.content = content;
          if (currentMode === 'code') {
            codeEditor.value = content;
            codeEditorContent = content;
            updateHighlight();
          }
        } catch (e) { rpLog('warn', 'Meta flush failed', String(e)); }
      }
      // Update summary in accordion
      const block = file?.suite?.blocks?.[bi];
      if (block) {
        const parts = [];
        if (block.name) parts.push(block.name);
        if (block.block_type && block.block_type !== 'test') parts.push(`[${block.block_type}]`);
        if (block.group) parts.push(`group: ${block.group}`);
        if (block.compare) parts.push('compare');
        blockMetaSummary.textContent = parts.join('  ·  ');
      }
      renderFileTree();
    }
  }, 300);
}

// Special handler for compare toggle — needs to initialize steps and switch view
metaCompare.addEventListener('change', async () => {
  if (activeFileIndex < 0 || activeBlockIndex < 0) return;
  const file = loadedFiles[activeFileIndex];
  const block = file?.suite?.blocks?.[activeBlockIndex];
  if (!block) return;

  block.compare = metaCompare.checked;
  file.results = null;

  if (block.compare && (!block.steps || block.steps.length === 0)) {
    // Initialize with two default steps from current request
    block.steps = [
      { name: 'baseline', request: { ...block.request, headers: [...(block.request.headers || [])] }, assertions: [], extracts: [] },
      { name: 'candidate', request: { method: block.request.method || 'GET', url: '', headers: [], body: null }, assertions: [], extracts: [] },
    ];
    block.diff = { step_a: 'baseline', step_b: 'candidate' };
    // Append default diff assertion, preserving any existing assertions
    if (!block.assertions) block.assertions = [];
    block.assertions.push({ left: '$diff.match', operator: '==', right: 'true' });
  } else if (!block.compare) {
    // Keep steps data but switch to normal view
    builderNormal.classList.remove('hidden');
    compareStepsView.classList.add('hidden');
  }

  if (block.compare && block.steps && block.steps.length > 0) {
    activeCompareStepIdx = 0;
    renderCompareSteps(activeFileIndex, activeBlockIndex);
    builderNormal.classList.add('hidden');
    compareStepsView.classList.remove('hidden');
  }

  // Flush content
  try {
    const content = await invoke('generate_http', { suite: file.suite });
    file.content = content;
    if (currentMode === 'code') {
      codeEditor.value = content;
      codeEditorContent = content;
      updateHighlight();
    }
  } catch (e) { rpLog('warn', 'Compare toggle flush failed', String(e)); }

  renderFileTree();
});
[metaName, metaDescription, metaGroup, metaDepends, metaDevAuth].forEach(el => {
  el.addEventListener('input', onMetaFieldChange);
});
[metaBlockType, metaMode, metaDisabled].forEach(el => {
  el.addEventListener('change', onMetaFieldChange);
});

// --- Compare Steps Rendering ---
let activeCompareStepIdx = 0;

/** Render the compare steps tabs and content for a compare block. */
function renderCompareSteps(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  const block = file.suite.blocks[blockIdx];
  const br = file.results?.block_results?.[blockIdx];

  if (!block.compare || !block.steps || block.steps.length === 0) {
    compareStepsView.classList.add('hidden');
    builderNormal.classList.remove('hidden');
    return;
  }

  builderNormal.classList.add('hidden');
  compareStepsView.classList.remove('hidden');

  // Build tabs: one per step + Comparison tab + Add Step
  let tabsHtml = '';
  block.steps.forEach((step, si) => {
    const sr = br?.step_results?.[si];
    let statusDot = '';
    if (sr) {
      const cls = sr.error ? 'failed' : 'passed';
      statusDot = `<span class="step-status ${cls}"></span>`;
    }
    const activeClass = si === activeCompareStepIdx ? 'active' : '';
    tabsHtml += `<button class="compare-step-tab ${activeClass}" data-step-idx="${si}">${escapeHtml(step.name)}${statusDot}</button>`;
  });

  // Comparison tab (always last real tab)
  const compIdx = block.steps.length;
  const compActive = activeCompareStepIdx === compIdx ? 'active' : '';
  tabsHtml += `<button class="compare-step-tab tab-comparison ${compActive}" data-step-idx="${compIdx}">⇄ Comparison</button>`;

  // Add step button
  tabsHtml += `<button class="compare-step-tab" data-action="add-step" title="Add a new step">+</button>`;

  compareStepsTabs.innerHTML = tabsHtml;

  // Wire tab clicks
  compareStepsTabs.querySelectorAll('.compare-step-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      if (tab.dataset.action === 'add-step') {
        addCompareStep(fileIdx, blockIdx);
        return;
      }
      activeCompareStepIdx = parseInt(tab.dataset.stepIdx);
      renderCompareSteps(fileIdx, blockIdx);
    });
  });

  // Render active tab content
  if (activeCompareStepIdx < block.steps.length) {
    renderStepPanel(fileIdx, blockIdx, activeCompareStepIdx);
  } else {
    renderComparisonPanel(fileIdx, blockIdx);
  }
}

/** Render a single step's editable panel. */
function renderStepPanel(fileIdx, blockIdx, stepIdx) {
  const file = loadedFiles[fileIdx];
  const block = file.suite.blocks[blockIdx];
  const step = block.steps[stepIdx];
  const sr = file.results?.block_results?.[blockIdx]?.step_results?.[stepIdx];

  let html = '<div class="compare-step-panel">';

  // Step name
  html += `<div class="step-field">
    <label>Step Name</label>
    <input type="text" class="meta-input step-name-input" value="${escapeAttr(step.name)}" data-step-idx="${stepIdx}" spellcheck="false">
  </div>`;

  // URL bar
  const methods = ['GET','POST','PUT','PATCH','DELETE','HEAD','OPTIONS'];
  html += `<div class="step-url-bar">
    <select class="step-method" data-step-idx="${stepIdx}">
      ${methods.map(m => `<option value="${m}"${step.request.method === m ? ' selected' : ''}>${m}</option>`).join('')}
    </select>
    <input type="text" class="step-url" value="${escapeAttr(step.request.url || '')}" data-step-idx="${stepIdx}" placeholder="https://..." spellcheck="false">
  </div>`;

  // Headers
  html += `<div class="step-field"><label>Headers</label><div class="step-kv-container" data-step-idx="${stepIdx}">`;
  if (step.request.headers && step.request.headers.length > 0) {
    step.request.headers.forEach(([k, v], hi) => {
      html += `<div class="step-kv-row">
        <input type="text" class="step-hdr-key" value="${escapeAttr(k)}" placeholder="Header" data-hi="${hi}" spellcheck="false">
        <input type="text" class="step-hdr-val" value="${escapeAttr(v)}" placeholder="Value" data-hi="${hi}" spellcheck="false">
        <button class="btn-icon step-hdr-remove" data-hi="${hi}" title="Remove">×</button>
      </div>`;
    });
  }
  html += `</div><button class="btn btn-ghost btn-xs step-add-btn step-add-header" data-step-idx="${stepIdx}">+ Header</button></div>`;

  // Body
  html += `<div class="step-field"><label>Body</label>
    <textarea class="step-body-area" data-step-idx="${stepIdx}" spellcheck="false" placeholder="Request body...">${escapeHtml(step.request.body || '')}</textarea>
  </div>`;

  // Step assertions
  html += `<div class="step-assertions-area"><label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Assertions</label>`;
  if (step.assertions && step.assertions.length > 0) {
    step.assertions.forEach((a, ai) => {
      const passed = sr?.assertion_results?.[ai]?.passed;
      const statusCls = passed === true ? 'style="border-left:2px solid var(--success-color)"' : passed === false ? 'style="border-left:2px solid var(--error-color)"' : '';
      html += `<div class="step-assertion-row" ${statusCls}>
        <input type="text" value="${escapeAttr(a.left)}" class="step-assert-left" data-ai="${ai}" style="flex:2" spellcheck="false">
        <select class="step-assert-op" data-ai="${ai}">
          ${['==','!=','>','<','>=','<=','contains'].map(op => `<option${a.operator === op ? ' selected' : ''}>${op}</option>`).join('')}
        </select>
        <input type="text" value="${escapeAttr(a.right)}" class="step-assert-right" data-ai="${ai}" style="flex:2" spellcheck="false">
        <button class="btn-icon step-assert-remove" data-ai="${ai}" title="Remove">×</button>
      </div>`;
    });
  }
  html += `<button class="btn btn-ghost btn-xs step-add-btn step-add-assertion" data-step-idx="${stepIdx}">+ Assertion</button></div>`;

  // Step extracts
  html += `<div class="step-assertions-area" style="margin-top:6px"><label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Extracts</label>`;
  if (step.extracts && step.extracts.length > 0) {
    step.extracts.forEach((ex, ei) => {
      html += `<div class="step-assertion-row">
        <input type="text" value="${escapeAttr(ex.variable_name)}" class="step-extract-var" data-ei="${ei}" style="flex:1" placeholder="variable" spellcheck="false">
        <span style="color:var(--text-muted)">=</span>
        <input type="text" value="${escapeAttr(ex.source_path)}" class="step-extract-path" data-ei="${ei}" style="flex:2" placeholder="$.json.path" spellcheck="false">
        <button class="btn-icon step-extract-remove" data-ei="${ei}" title="Remove">×</button>
      </div>`;
    });
  }
  html += `<button class="btn btn-ghost btn-xs step-add-btn step-add-extract" data-step-idx="${stepIdx}">+ Extract</button></div>`;

  // Remove step button
  if (block.steps.length > 1) {
    html += `<div style="margin-top:10px;text-align:right">
      <button class="btn btn-ghost btn-xs step-remove-btn" data-step-idx="${stepIdx}" style="color:var(--error-color)">🗑 Remove Step</button>
    </div>`;
  }

  // Step response preview (if run)
  if (sr?.response) {
    const r = sr.response;
    html += `<div style="margin-top:10px;border-top:1px solid var(--border);padding-top:8px">
      <label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Response</label>
      <div style="font-size:11px;color:var(--text-muted);margin-top:4px">
        <span class="status-badge status-${String(r.status)[0]}xx">${r.status} ${escapeHtml(r.status_text || '')}</span>
        · ${r.time_ms}ms · ${formatBytes(r.body?.length || 0)}
      </div>
    </div>`;
  }

  html += '</div>';
  compareStepsContent.innerHTML = html;

  // Wire step panel events
  wireStepPanelEvents(fileIdx, blockIdx, stepIdx);
}

/** Wire all interactive events within a step panel. */
function wireStepPanelEvents(fileIdx, blockIdx, stepIdx) {
  const file = loadedFiles[fileIdx];
  const block = file.suite.blocks[blockIdx];
  const step = block.steps[stepIdx];
  const panel = compareStepsContent;

  const scheduleFlush = () => {
    file.results = null;
    clearTimeout(stepFlushTimer);
    stepFlushTimer = setTimeout(async () => {
      try {
        const content = await invoke('generate_http', { suite: file.suite });
        file.content = content;
        if (currentMode === 'code') {
          codeEditor.value = content;
          codeEditorContent = content;
          updateHighlight();
        }
      } catch (e) { rpLog('warn', 'Step flush failed', String(e)); }
    }, 400);
  };

  // Step name
  panel.querySelector('.step-name-input')?.addEventListener('input', (e) => {
    const oldName = step.name;
    const newName = e.target.value.trim();
    step.name = newName;
    // Update diff directive references if they pointed to old name
    if (block.diff) {
      if (block.diff.step_a === oldName) block.diff.step_a = newName;
      if (block.diff.step_b === oldName) block.diff.step_b = newName;
    }
    scheduleFlush();
  });

  // Method + URL
  panel.querySelector('.step-method')?.addEventListener('change', (e) => {
    step.request.method = e.target.value;
    scheduleFlush();
  });
  panel.querySelector('.step-url')?.addEventListener('input', (e) => {
    step.request.url = e.target.value;
    scheduleFlush();
  });

  // Headers
  panel.querySelectorAll('.step-hdr-key, .step-hdr-val').forEach(inp => {
    inp.addEventListener('input', () => {
      const hi = parseInt(inp.dataset.hi);
      const row = inp.closest('.step-kv-row');
      step.request.headers[hi] = [
        row.querySelector('.step-hdr-key').value.trim(),
        row.querySelector('.step-hdr-val').value.trim(),
      ];
      scheduleFlush();
    });
  });
  panel.querySelectorAll('.step-hdr-remove').forEach(btn => {
    btn.addEventListener('click', () => {
      step.request.headers.splice(parseInt(btn.dataset.hi), 1);
      renderCompareSteps(fileIdx, blockIdx);
      scheduleFlush();
    });
  });
  panel.querySelector('.step-add-header')?.addEventListener('click', () => {
    if (!step.request.headers) step.request.headers = [];
    step.request.headers.push(['', '']);
    renderCompareSteps(fileIdx, blockIdx);
  });

  // Body
  panel.querySelector('.step-body-area')?.addEventListener('input', (e) => {
    step.request.body = e.target.value || null;
    scheduleFlush();
  });

  // Assertions
  panel.querySelectorAll('.step-assert-left, .step-assert-op, .step-assert-right').forEach(inp => {
    inp.addEventListener('input', () => {
      const ai = parseInt(inp.dataset.ai);
      const row = inp.closest('.step-assertion-row');
      step.assertions[ai] = {
        left: row.querySelector('.step-assert-left').value.trim(),
        operator: row.querySelector('.step-assert-op').value,
        right: row.querySelector('.step-assert-right').value.trim(),
      };
      scheduleFlush();
    });
    inp.addEventListener('change', () => inp.dispatchEvent(new Event('input')));
  });
  panel.querySelectorAll('.step-assert-remove').forEach(btn => {
    btn.addEventListener('click', () => {
      step.assertions.splice(parseInt(btn.dataset.ai), 1);
      renderCompareSteps(fileIdx, blockIdx);
      scheduleFlush();
    });
  });
  panel.querySelector('.step-add-assertion')?.addEventListener('click', () => {
    if (!step.assertions) step.assertions = [];
    step.assertions.push({ left: 'status', operator: '==', right: '200' });
    renderCompareSteps(fileIdx, blockIdx);
  });

  // Extracts
  panel.querySelectorAll('.step-extract-var, .step-extract-path').forEach(inp => {
    inp.addEventListener('input', () => {
      const ei = parseInt(inp.dataset.ei);
      const row = inp.closest('.step-assertion-row');
      step.extracts[ei] = {
        variable_name: row.querySelector('.step-extract-var').value.trim(),
        source_path: row.querySelector('.step-extract-path').value.trim(),
      };
      scheduleFlush();
    });
  });
  panel.querySelectorAll('.step-extract-remove').forEach(btn => {
    btn.addEventListener('click', () => {
      step.extracts.splice(parseInt(btn.dataset.ei), 1);
      renderCompareSteps(fileIdx, blockIdx);
      scheduleFlush();
    });
  });
  panel.querySelector('.step-add-extract')?.addEventListener('click', () => {
    if (!step.extracts) step.extracts = [];
    step.extracts.push({ variable_name: '', source_path: '' });
    renderCompareSteps(fileIdx, blockIdx);
  });

  // Remove step
  panel.querySelector('.step-remove-btn')?.addEventListener('click', () => {
    block.steps.splice(stepIdx, 1);
    // Fix diff directive if needed
    if (block.diff) {
      const names = block.steps.map(s => s.name);
      if (!names.includes(block.diff.step_a) || !names.includes(block.diff.step_b)) {
        block.diff = null;
      }
    }
    activeCompareStepIdx = Math.min(activeCompareStepIdx, block.steps.length - 1);
    renderCompareSteps(fileIdx, blockIdx);
    scheduleFlush();
  });
}

/** Add a new step to a compare block. */
function addCompareStep(fileIdx, blockIdx) {
  const block = loadedFiles[fileIdx].suite.blocks[blockIdx];
  const stepNum = block.steps.length + 1;
  block.steps.push({
    name: `step-${stepNum}`,
    request: { method: 'GET', url: '', headers: [], body: null },
    assertions: [],
    extracts: [],
  });
  activeCompareStepIdx = block.steps.length - 1;
  renderCompareSteps(fileIdx, blockIdx);
}

/** Render the Comparison tab for a compare block. */
function renderComparisonPanel(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  const block = file.suite.blocks[blockIdx];
  const br = file.results?.block_results?.[blockIdx];
  const diff = br?.diff_result;

  let html = '<div class="compare-diff-panel">';

  // Diff directive editor
  const stepNames = block.steps.map(s => s.name);
  html += `<div class="step-field"><label>Diff Steps</label>
    <div style="display:flex;gap:6px;align-items:center">
      <select class="meta-select diff-step-a">
        <option value="">Select step A</option>
        ${stepNames.map(n => `<option value="${escapeAttr(n)}"${block.diff?.step_a === n ? ' selected' : ''}>${escapeHtml(n)}</option>`).join('')}
      </select>
      <span style="color:var(--text-muted)">↔</span>
      <select class="meta-select diff-step-b">
        <option value="">Select step B</option>
        ${stepNames.map(n => `<option value="${escapeAttr(n)}"${block.diff?.step_b === n ? ' selected' : ''}>${escapeHtml(n)}</option>`).join('')}
      </select>
    </div>
  </div>`;

  // Block-level assertions editor
  html += `<div class="step-assertions-area" style="margin-top:10px"><label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Comparison Assertions</label>`;
  if (block.assertions && block.assertions.length > 0) {
    block.assertions.forEach((a, ai) => {
      const passed = br?.assertion_results?.[ai]?.passed;
      const statusCls = passed === true ? 'style="border-left:2px solid var(--success-color)"' : passed === false ? 'style="border-left:2px solid var(--error-color)"' : '';
      html += `<div class="step-assertion-row" ${statusCls}>
        <input type="text" value="${escapeAttr(a.left)}" class="comp-assert-left" data-ai="${ai}" style="flex:2" spellcheck="false">
        <select class="comp-assert-op" data-ai="${ai}">
          ${['==','!=','>','<','>=','<=','contains'].map(op => `<option${a.operator === op ? ' selected' : ''}>${op}</option>`).join('')}
        </select>
        <input type="text" value="${escapeAttr(a.right)}" class="comp-assert-right" data-ai="${ai}" style="flex:2" spellcheck="false">
        <button class="btn-icon comp-assert-remove" data-ai="${ai}" title="Remove">×</button>
      </div>`;
    });
  }
  html += `<button class="btn btn-ghost btn-xs step-add-btn comp-add-assertion">+ Assertion</button></div>`;

  // Block-level extracts editor
  html += `<div class="step-assertions-area" style="margin-top:6px"><label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Extracts</label>`;
  if (block.extracts && block.extracts.length > 0) {
    block.extracts.forEach((ex, ei) => {
      html += `<div class="step-assertion-row">
        <input type="text" value="${escapeAttr(ex.variable_name)}" class="comp-extract-var" data-ei="${ei}" style="flex:1" placeholder="variable" spellcheck="false">
        <span style="color:var(--text-muted)">=</span>
        <input type="text" value="${escapeAttr(ex.source_path)}" class="comp-extract-path" data-ei="${ei}" style="flex:2" placeholder="$diff.path" spellcheck="false">
        <button class="btn-icon comp-extract-remove" data-ei="${ei}" title="Remove">×</button>
      </div>`;
    });
  }
  html += `<button class="btn btn-ghost btn-xs step-add-btn comp-add-extract">+ Extract</button></div>`;

  // Diff results summary (if run)
  if (diff) {
    html += `<div style="margin-top:12px;border-top:1px solid var(--border);padding-top:10px">
      <label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Diff Results</label>
      <div class="diff-summary-inline" style="margin-top:6px">
        <span class="diff-badge ${diff.match_exact ? 'match' : 'mismatch'}">${diff.match_exact ? '✓ Exact Match' : (diff.similarity * 100).toFixed(1) + '% Similar'}</span>
        <span class="diff-badge">${diff.is_json ? 'JSON' : 'Text'}</span>
        ${diff.added_count ? `<span class="diff-badge added">+${diff.added_count} added</span>` : ''}
        ${diff.removed_count ? `<span class="diff-badge removed">-${diff.removed_count} removed</span>` : ''}
        ${diff.changed_count ? `<span class="diff-badge changed">Δ${diff.changed_count} changed</span>` : ''}
      </div>
      <button class="btn btn-primary btn-xs" onclick="openDiffViewer(${fileIdx}, ${blockIdx})" style="margin-top:6px">🔍 View Full Diff</button>
    </div>`;
  }

  html += '</div>';
  compareStepsContent.innerHTML = html;

  // Wire comparison panel events
  wireComparisonPanelEvents(fileIdx, blockIdx);
}

/** Wire events on the Comparison tab. */
function wireComparisonPanelEvents(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  const block = file.suite.blocks[blockIdx];
  const panel = compareStepsContent;

  const scheduleFlush = () => {
    file.results = null;
    clearTimeout(stepFlushTimer);
    stepFlushTimer = setTimeout(async () => {
      try {
        const content = await invoke('generate_http', { suite: file.suite });
        file.content = content;
        if (currentMode === 'code') {
          codeEditor.value = content;
          codeEditorContent = content;
          updateHighlight();
        }
      } catch (e) { rpLog('warn', 'Comparison flush failed', String(e)); }
    }, 400);
  };

  // Diff step selectors
  const diffA = panel.querySelector('.diff-step-a');
  const diffB = panel.querySelector('.diff-step-b');
  const onDiffChange = () => {
    const a = diffA?.value, b = diffB?.value;
    if (a && b && a !== b) {
      block.diff = { step_a: a, step_b: b };
    } else if (!a && !b) {
      block.diff = null;
    }
    scheduleFlush();
  };
  diffA?.addEventListener('change', onDiffChange);
  diffB?.addEventListener('change', onDiffChange);

  // Block assertions
  panel.querySelectorAll('.comp-assert-left, .comp-assert-op, .comp-assert-right').forEach(inp => {
    inp.addEventListener('input', () => {
      const ai = parseInt(inp.dataset.ai);
      const row = inp.closest('.step-assertion-row');
      block.assertions[ai] = {
        left: row.querySelector('.comp-assert-left').value.trim(),
        operator: row.querySelector('.comp-assert-op').value,
        right: row.querySelector('.comp-assert-right').value.trim(),
      };
      scheduleFlush();
    });
    inp.addEventListener('change', () => inp.dispatchEvent(new Event('input')));
  });
  panel.querySelectorAll('.comp-assert-remove').forEach(btn => {
    btn.addEventListener('click', () => {
      block.assertions.splice(parseInt(btn.dataset.ai), 1);
      renderComparisonPanel(fileIdx, blockIdx);
      scheduleFlush();
    });
  });
  panel.querySelector('.comp-add-assertion')?.addEventListener('click', () => {
    if (!block.assertions) block.assertions = [];
    block.assertions.push({ left: '$diff.match', operator: '==', right: 'true' });
    renderComparisonPanel(fileIdx, blockIdx);
  });

  // Block extracts
  panel.querySelectorAll('.comp-extract-var, .comp-extract-path').forEach(inp => {
    inp.addEventListener('input', () => {
      const ei = parseInt(inp.dataset.ei);
      const row = inp.closest('.step-assertion-row');
      block.extracts[ei] = {
        variable_name: row.querySelector('.comp-extract-var').value.trim(),
        source_path: row.querySelector('.comp-extract-path').value.trim(),
      };
      scheduleFlush();
    });
  });
  panel.querySelectorAll('.comp-extract-remove').forEach(btn => {
    btn.addEventListener('click', () => {
      block.extracts.splice(parseInt(btn.dataset.ei), 1);
      renderComparisonPanel(fileIdx, blockIdx);
      scheduleFlush();
    });
  });
  panel.querySelector('.comp-add-extract')?.addEventListener('click', () => {
    if (!block.extracts) block.extracts = [];
    block.extracts.push({ variable_name: '', source_path: '' });
    renderComparisonPanel(fileIdx, blockIdx);
  });
}

// --- Builder Directives (Assertions & Extracts for normal blocks) ---

/** Render editable assertions and extracts for a normal (non-compare) block. */
function renderBuilderDirectives(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return;
  const block = file.suite.blocks[blockIdx];
  if (!block) return;
  const br = file.results?.block_results?.[blockIdx];

  let html = '<div class="compare-step-panel">';

  // Assertions
  html += `<div class="step-assertions-area"><label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Assertions</label>`;
  if (block.assertions && block.assertions.length > 0) {
    block.assertions.forEach((a, ai) => {
      const passed = br?.assertion_results?.[ai]?.passed;
      const statusCls = passed === true ? 'style="border-left:2px solid var(--green)"' : passed === false ? 'style="border-left:2px solid var(--red)"' : '';
      html += `<div class="step-assertion-row" ${statusCls}>
        <input type="text" value="${escapeAttr(a.left)}" class="bldr-assert-left" data-ai="${ai}" style="flex:2" placeholder="status or $.path" spellcheck="false">
        <select class="bldr-assert-op" data-ai="${ai}">
          ${['==','!=','>','<','>=','<=','contains'].map(op => `<option${a.operator === op ? ' selected' : ''}>${op}</option>`).join('')}
        </select>
        <input type="text" value="${escapeAttr(a.right)}" class="bldr-assert-right" data-ai="${ai}" style="flex:2" placeholder="expected value" spellcheck="false">
        <button class="btn-icon bldr-assert-remove" data-ai="${ai}" title="Remove">×</button>
      </div>`;
    });
  } else {
    html += `<div style="font-size:11px;color:var(--text-muted);padding:4px 0">No assertions yet</div>`;
  }
  html += `<button class="btn btn-ghost btn-xs step-add-btn bldr-add-assertion">+ Assertion</button></div>`;

  // Extracts
  html += `<div class="step-assertions-area" style="margin-top:10px"><label style="font-size:10px;font-weight:600;color:var(--text-secondary);text-transform:uppercase;letter-spacing:.5px">Extracts</label>`;
  if (block.extracts && block.extracts.length > 0) {
    block.extracts.forEach((ex, ei) => {
      const er = br?.extract_results?.[ei];
      const valStr = er?.value ?? '';
      const statusCls = er ? (er.success ? 'style="border-left:2px solid var(--green)"' : 'style="border-left:2px solid var(--red)"') : '';
      html += `<div class="step-assertion-row" ${statusCls}>
        <input type="text" value="${escapeAttr(ex.variable_name)}" class="bldr-extract-var" data-ei="${ei}" style="flex:1" placeholder="variable_name" spellcheck="false">
        <span style="color:var(--text-muted)">=</span>
        <input type="text" value="${escapeAttr(ex.source_path)}" class="bldr-extract-path" data-ei="${ei}" style="flex:2" placeholder="$.json.path" spellcheck="false">
        <button class="btn-icon bldr-extract-remove" data-ei="${ei}" title="Remove">×</button>
      </div>`;
      if (er && er.value != null) {
        html += `<div style="font-size:10px;color:var(--text-muted);padding:0 0 2px 20px">→ ${escapeHtml(valStr)}</div>`;
      }
    });
  } else {
    html += `<div style="font-size:11px;color:var(--text-muted);padding:4px 0">No extracts yet</div>`;
  }
  html += `<button class="btn btn-ghost btn-xs step-add-btn bldr-add-extract">+ Extract</button></div>`;

  html += '</div>';
  builderDirectivesContent.innerHTML = html;

  // Wire events
  wireBuilderDirectiveEvents(fileIdx, blockIdx);
}

/** Wire events for the builder assertions/extracts panel. */
function wireBuilderDirectiveEvents(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  const block = file.suite.blocks[blockIdx];
  const panel = builderDirectivesContent;

  const scheduleFlush = () => {
    file.results = null;
    clearTimeout(directiveFlushTimer);
    directiveFlushTimer = setTimeout(async () => {
      try {
        const content = await invoke('generate_http', { suite: file.suite });
        file.content = content;
        if (currentMode === 'code') {
          codeEditor.value = content;
          codeEditorContent = content;
          updateHighlight();
        }
      } catch (e) { rpLog('warn', 'Directive flush failed', String(e)); }
    }, 400);
  };

  // Assertions
  panel.querySelectorAll('.bldr-assert-left, .bldr-assert-op, .bldr-assert-right').forEach(inp => {
    const handler = () => {
      const ai = parseInt(inp.dataset.ai);
      const row = inp.closest('.step-assertion-row');
      block.assertions[ai] = {
        left: row.querySelector('.bldr-assert-left').value.trim(),
        operator: row.querySelector('.bldr-assert-op').value,
        right: row.querySelector('.bldr-assert-right').value.trim(),
      };
      scheduleFlush();
    };
    inp.addEventListener('input', handler);
    inp.addEventListener('change', handler);
  });
  panel.querySelectorAll('.bldr-assert-remove').forEach(btn => {
    btn.addEventListener('click', () => {
      block.assertions.splice(parseInt(btn.dataset.ai), 1);
      file.results = null;
      renderBuilderDirectives(fileIdx, blockIdx);
      scheduleFlush();
    });
  });
  panel.querySelector('.bldr-add-assertion')?.addEventListener('click', () => {
    if (!block.assertions) block.assertions = [];
    block.assertions.push({ left: 'status', operator: '==', right: '200' });
    file.results = null;
    renderBuilderDirectives(fileIdx, blockIdx);
    scheduleFlush();
  });

  // Extracts
  panel.querySelectorAll('.bldr-extract-var, .bldr-extract-path').forEach(inp => {
    inp.addEventListener('input', () => {
      const ei = parseInt(inp.dataset.ei);
      const row = inp.closest('.step-assertion-row');
      block.extracts[ei] = {
        variable_name: row.querySelector('.bldr-extract-var').value.trim(),
        source_path: row.querySelector('.bldr-extract-path').value.trim(),
      };
      scheduleFlush();
    });
  });
  panel.querySelectorAll('.bldr-extract-remove').forEach(btn => {
    btn.addEventListener('click', () => {
      block.extracts.splice(parseInt(btn.dataset.ei), 1);
      file.results = null;
      renderBuilderDirectives(fileIdx, blockIdx);
      scheduleFlush();
    });
  });
  panel.querySelector('.bldr-add-extract')?.addEventListener('click', () => {
    if (!block.extracts) block.extracts = [];
    block.extracts.push({ variable_name: '', source_path: '' });
    file.results = null;
    renderBuilderDirectives(fileIdx, blockIdx);
    scheduleFlush();
  });
}

// --- Block Selection ---
function selectBlock(fileIdx, blockIdx) {
  activeFileIndex = fileIdx;
  activeBlockIndex = blockIdx;

  const file = loadedFiles[fileIdx];
  if (!file) return;
  const block = file.suite.blocks[blockIdx];
  if (!block) return;

  // Populate metadata accordion
  populateMetadata(block);

  // Determine if this is a compare block with steps
  const isCompare = block.compare && block.steps && block.steps.length > 0;

  if (isCompare) {
    // Show compare steps view, hide normal builder
    builderNormal.classList.add('hidden');
    compareStepsView.classList.remove('hidden');
    // Show url bar but with the first step info (or hide send for compare)
    if (block.steps[0]) {
      methodSelect.value = block.steps[0].request.method || 'GET';
      updateMethodColor();
      urlInput.value = block.steps[0].request.url || '';
    }
    activeCompareStepIdx = Math.min(activeCompareStepIdx, block.steps.length);
    renderCompareSteps(fileIdx, blockIdx);
  } else {
    // Normal block — show builder, hide compare view
    builderNormal.classList.remove('hidden');
    compareStepsView.classList.add('hidden');

    // Load request into editor
    const req = block.request;
    if (req) {
      methodSelect.value = req.method || 'GET';
      updateMethodColor();
      urlInput.value = req.url || '';

      headersContainer.innerHTML = '';
      if (req.headers && req.headers.length > 0) {
        req.headers.forEach(([k, v]) => headersContainer.appendChild(createHeaderRow(k, v)));
      } else {
        headersContainer.appendChild(createHeaderRow());
      }

      if (req.body) {
        bodyType.value = pickBodyType(req.headers, req.body);
        bodyInput.value = req.body;
        bodyInput.disabled = false;
      } else {
        bodyType.value = 'none';
        bodyInput.value = '';
        bodyInput.disabled = true;
      }
      bodyType.dispatchEvent(new Event('change'));
    }

    // Render assertions & extracts in builder sub-tab
    renderBuilderDirectives(fileIdx, blockIdx);
  }

  // Sync the Params tab to whatever URL just got loaded.
  renderParamsFromUrl();

  // Show response if block has been run
  const br = file.results?.block_results?.[blockIdx];
  if (isCompare && br?.step_results?.length > 0) {
    const firstStepResp = br.step_results[0].response;
    if (firstStepResp) {
      displayResponse(firstStepResp);
      lastResponse = firstStepResp;
    }
  } else if (br) {
    if (br.response) {
      displayResponse(br.response);
      lastResponse = br.response;
    } else if (br.error) {
      displayError(br.error);
      lastResponse = null;
    }
  }

  // Show assertions tab if block has assertions, extracts, or run results
  const hasDirectives = isCompare || (block.assertions && block.assertions.length > 0) || (block.extracts && block.extracts.length > 0);
  const hasResults = br && (br.assertion_results?.length > 0 || br.extract_results?.length > 0 || br.error);
  if (hasDirectives || hasResults) {
    assertionsTab.style.display = '';
    renderAssertions(fileIdx, blockIdx);
  } else {
    assertionsTab.style.display = 'none';
  }

  updateActiveHighlight();

  // If in code mode, refresh code editor with this file's content
  if (currentMode === 'code' && loadedFiles[fileIdx]) {
    syncBuilderToCode();
  }
}

// --- Assertions Rendering ---
function renderAssertions(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  const block = file.suite.blocks[blockIdx];
  const br = file.results?.block_results?.[blockIdx];

  let html = '';

  // Compare block: assertions first, then action button, then details
  if (block.compare && block.steps && block.steps.length > 0) {
    const diff = br?.diff_result;

    // 1. Comparison assertions (top priority — what the user cares about)
    if (block.assertions && block.assertions.length > 0) {
      html += '<div class="assertion-group"><div class="assertion-group-header">Comparison Assertions</div>';
      block.assertions.forEach((assertion, i) => {
        const assertionText = `${assertion.left} ${assertion.operator} ${assertion.right}`;
        const ar = br?.assertion_results?.[i];
        const passed = ar ? ar.passed : null;
        const statusClass = passed === true ? 'passed' : passed === false ? 'failed' : '';
        const icon = passed === true ? '\u2713' : passed === false ? '\u2717' : '\u25CB';
        let detail = '';
        if (ar && !ar.passed && ar.actual != null) {
          detail = `<span class="assert-detail">got: ${escapeHtml(ar.actual)}</span>`;
        }
        html += `<div class="assertion-row ${statusClass}"><span class="assert-icon">${icon}</span><span class="assert-text">${escapeHtml(assertionText)}</span>${detail}</div>`;
      });
      html += '</div>';
    }

    // 2. Per-step assertions & extracts
    block.steps.forEach((step, si) => {
      const sr = br?.step_results?.[si];
      const stepStatus = sr ? (sr.error ? 'error' : 'done') : 'pending';
      html += `<div class="assertion-group compare-step-group">
        <div class="assertion-group-header">Step: ${escapeHtml(step.name)} <span class="step-badge ${stepStatus}">${stepStatus}</span></div>`;

      // Step assertions
      if (step.assertions && step.assertions.length > 0) {
        step.assertions.forEach((assertion, i) => {
          const assertionText = `${assertion.left} ${assertion.operator} ${assertion.right}`;
          const ar = sr?.assertion_results?.[i];
          const passed = ar ? ar.passed : null;
          const statusClass = passed === true ? 'passed' : passed === false ? 'failed' : '';
          const icon = passed === true ? '\u2713' : passed === false ? '\u2717' : '\u25CB';
          let detail = '';
          if (ar && !ar.passed && ar.actual != null) {
            detail = `<span class="assert-detail">got: ${escapeHtml(ar.actual)}</span>`;
          }
          html += `<div class="assertion-row ${statusClass}"><span class="assert-icon">${icon}</span><span class="assert-text">${escapeHtml(assertionText)}</span>${detail}</div>`;
        });
      }

      // Step extracts
      if (step.extracts && step.extracts.length > 0) {
        step.extracts.forEach((extract, i) => {
          const er = sr?.extract_results?.[i];
          const success = er ? er.success : null;
          const stateClass = success === true ? 'extract-success' : success === false ? 'extract-failed' : '';
          const icon = success === true ? '\u2713' : success === false ? '\u2717' : '\u26A1';
          const val = er?.value ?? '\u2014';
          html += `<div class="extract-row ${stateClass}"><span class="extract-icon">${icon}</span><span class="extract-var">${escapeHtml(extract.variable_name)}</span><span class="var-sep">=</span><span class="extract-val">${escapeHtml(val)}</span></div>`;
        });
      }

      // Step error
      if (sr?.error) {
        html += `<div class="assertion-row failed"><span class="assert-icon">\u2717</span><span class="assert-text">${escapeHtml(sr.error)}</span></div>`;
      }

      html += '</div>';
    });

    // 3. View Full Diff button
    if (diff) {
      html += `<button class="diff-view-btn" onclick="openDiffViewer(${fileIdx}, ${blockIdx})">🔍 View Full Diff</button>`;
    }

    // 4. Comparison result details (summary, changed paths)
    if (diff) {
      html += `<div class="assertion-group diff-summary-group">
        <div class="assertion-group-header">\u21C4 Comparison Result</div>
        <div class="diff-summary">
          <div class="diff-stat"><span class="diff-label">Match:</span><span class="diff-value ${diff.match_exact ? 'diff-match' : 'diff-mismatch'}">${diff.match_exact ? 'Exact Match \u2713' : 'Differences Found'}</span></div>
          <div class="diff-stat"><span class="diff-label">Similarity:</span><span class="diff-value">${(diff.similarity * 100).toFixed(1)}%</span></div>
          <div class="diff-stat"><span class="diff-label">Type:</span><span class="diff-value">${diff.is_json ? 'JSON' : 'Text'}</span></div>
          ${diff.added_count ? `<div class="diff-stat"><span class="diff-label">Added:</span><span class="diff-value diff-added">+${diff.added_count}</span></div>` : ''}
          ${diff.removed_count ? `<div class="diff-stat"><span class="diff-label">Removed:</span><span class="diff-value diff-removed">-${diff.removed_count}</span></div>` : ''}
          ${diff.changed_count ? `<div class="diff-stat"><span class="diff-label">Changed:</span><span class="diff-value diff-changed">\u0394${diff.changed_count}</span></div>` : ''}
        </div>`;

      if (diff.changed_paths && diff.changed_paths.length > 0) {
        html += `<div class="diff-paths"><div class="diff-paths-header">Changed Paths</div>`;
        diff.changed_paths.slice(0, 50).forEach(cp => {
          html += `<div class="diff-path-row">
            <span class="diff-path-name">${escapeHtml(cp.path)}</span>
            <span class="diff-path-left" title="Step A">${escapeHtml(String(cp.left ?? ''))}</span>
            <span class="diff-path-arrow">\u2192</span>
            <span class="diff-path-right" title="Step B">${escapeHtml(String(cp.right ?? ''))}</span>
          </div>`;
        });
        if (diff.changed_paths.length > 50) {
          html += `<div class="diff-path-row">... and ${diff.changed_paths.length - 50} more</div>`;
        }
        html += '</div>';
      }

      if (diff.added_paths && diff.added_paths.length > 0) {
        html += `<div class="diff-paths"><div class="diff-paths-header">Added Paths</div>`;
        diff.added_paths.slice(0, 20).forEach(p => {
          html += `<div class="diff-path-row"><span class="diff-path-name diff-added">+ ${escapeHtml(p)}</span></div>`;
        });
        html += '</div>';
      }
      if (diff.removed_paths && diff.removed_paths.length > 0) {
        html += `<div class="diff-paths"><div class="diff-paths-header">Removed Paths</div>`;
        diff.removed_paths.slice(0, 20).forEach(p => {
          html += `<div class="diff-path-row"><span class="diff-path-name diff-removed">- ${escapeHtml(p)}</span></div>`;
        });
        html += '</div>';
      }

      html += '</div>';
    }

    // Block-level error
    if (br?.error) {
      html += `<div class="assertion-group"><div class="assertion-group-header">Error</div>
        <div class="assertion-row failed"><span class="assert-icon">\u2717</span><span class="assert-text">${escapeHtml(br.error)}</span></div></div>`;
    }

    if (!html) html = '<div class="sidebar-empty" style="padding:20px">No assertions or extractions</div>';
    assertionsContent.innerHTML = html;
    return;
  }

  // Assertions
  if (block.assertions && block.assertions.length > 0) {
    html += '<div class="assertion-group"><div class="assertion-group-header">Assertions</div>';
    block.assertions.forEach((assertion, i) => {
      const assertionText = `${assertion.left} ${assertion.operator} ${assertion.right}`;
      const ar = br?.assertion_results?.[i];
      const passed = ar ? ar.passed : null;
      const statusClass = passed === true ? 'passed' : passed === false ? 'failed' : '';
      const icon = passed === true ? '\u2713' : passed === false ? '\u2717' : '\u25CB';

      let detail = '';
      if (ar && !ar.passed && ar.actual != null) {
        detail = `<span class="assert-detail">got: ${escapeHtml(ar.actual)}</span>`;
      }

      html += `
        <div class="assertion-row ${statusClass}">
          <span class="assert-icon">${icon}</span>
          <span class="assert-text">${escapeHtml(assertionText)}</span>
          ${detail}
        </div>
      `;
    });
    html += '</div>';
  }

  // Extracts
  if (block.extracts && block.extracts.length > 0) {
    html += '<div class="assertion-group"><div class="assertion-group-header">Variable Extractions</div>';
    block.extracts.forEach((extract, i) => {
      const er = br?.extract_results?.[i];
      const success = er ? er.success : null;
      const stateClass = success === true ? 'extract-success' : success === false ? 'extract-failed' : '';
      const icon = success === true ? '\u2713' : success === false ? '\u2717' : '\u26A1';
      const val = er?.value ?? '\u2014';

      html += `
        <div class="extract-row ${stateClass}">
          <span class="extract-icon">${icon}</span>
          <span class="extract-var">${escapeHtml(extract.variable_name)}</span>
          <span class="var-sep">=</span>
          <span class="extract-val">${escapeHtml(val)}</span>
        </div>
      `;
    });
    html += '</div>';
  }

  // Error message from run
  if (br?.error) {
    html += `<div class="assertion-group"><div class="assertion-group-header">Error</div>
      <div class="assertion-row failed"><span class="assert-icon">\u2717</span><span class="assert-text">${escapeHtml(br.error)}</span></div>
    </div>`;
  }

  if (!html) {
    html = '<div class="sidebar-empty" style="padding:20px">No assertions or extractions</div>';
  }

  assertionsContent.innerHTML = html;
}

// --- Run All Tests ---
async function runAllTests() {
  // If already running, signal Rust to abort so the in-flight file stops
  // ASAP (skips remaining setup/test blocks; teardown still runs to clean up).
  if (isRunning) {
    if (abortRunController) abortRunController.abort();
    try {
      await invoke('request_run_abort');
    } catch (err) {
      rpLog('warn', 'request_run_abort failed', { error: String(err) });
    }
    showToast('Stopping… (current block will finish, then cleanup runs)', 'info');
    return;
  }

  // Reset auto-run timer on manual trigger
  resetAutoRunTimer();

  if (loadedFiles.length === 0) {
    showToast('No files loaded', 'info');
    return;
  }

  // Check all files for unresolved variables (only enabled blocks)
  let allUnresolved = [];
  for (let fi = 0; fi < loadedFiles.length; fi++) {
    try {
      const suite = getEnabledSuite(fi);
      const unresolved = await invoke('check_variables', {
        suite,
        envVars: collectVariablesArray()
      });
      allUnresolved.push(...unresolved);
    } catch (e) { console.warn('check_variables failed:', e); }
  }
  allUnresolved = [...new Set(allUnresolved)];
  if (allUnresolved.length > 0) {
    rpLog('warn', 'Unresolved variables found', allUnresolved);
    promptForVariables(allUnresolved);
    return;
  }

  abortRunController = new AbortController();
  const signal = abortRunController.signal;
  setRunning(true);
  // Each Run All starts with a clean runtime override slate so stale @@extract
  // values from previous runs don't leak into this one.
  runtimeOverrides = {};
  rpLog('info', 'Run All started', { fileCount: loadedFiles.length, azureAuth: azureAuthState });

  // Reset telemetry stats for this run
  if (telemetryState !== 'off' && telemetryEnabled) {
    telemetryStats = [];
    setTelemetryState('sending');
    rpLog('info', 'OTEL telemetry: export will begin after test execution');
  } else if (!telemetryEnabled && telemetryState !== 'off') {
    rpLog('info', 'OTEL telemetry: skipped (disabled by user)');
  }

  // Azure auth: use cached tokens if authenticated
  let azureExtraVars = [];
  const isAzureActive = azureAuthState === 'authenticated';
  const runMode = isAzureActive ? 'dev' : null;
  if (isAzureActive) {
    for (const f of loadedFiles) {
      const tokenVars = fetchDevModeToken(f.suite, f.content);
      azureExtraVars.push(...tokenVars);
    }

    // Inject monitor token for OTLP telemetry ingestion
    const monitorCached = azureTokenCache.get('https://monitor.azure.com/.default');
    if (monitorCached && Date.now() < monitorCached.expiresAt - 60000) {
      azureExtraVars.push(['__monitor_token', monitorCached.token]);
    }
  }

  // Reset results
  loadedFiles.forEach(f => f.results = null);
  renderFileTree();

  let totalPassed = 0;
  let totalFailed = 0;
  let totalSkipped = 0;
  let totalTimeMs = 0;
  const allBlockResults = [];

  // Pre-compute enabled blocks per file for pending display
  const pendingByFile = [];
  for (let fi = 0; fi < loadedFiles.length; fi++) {
    const file = loadedFiles[fi];
    const enabledMap = getEnabledIndexMap(fi);
    pendingByFile.push(enabledMap.map(origIdx => {
      const block = file.suite.blocks[origIdx];
      return {
        fileName: file.name,
        name: block.name || block.block_type,
        blockType: block.block_type,
        fileIdx: fi,
        blockIdx: origIdx
      };
    }));
  }

  // Only show current file's blocks as pending (not all future files)
  let currentFilePending = pendingByFile[0] || [];
  showTestResults(0, 0, 0, 0, [], currentFilePending);
  testResultsDetails.classList.remove('hidden');

  // Start listening for per-block progress events from Rust
  await startBlockProgressListener();
  await startBlockResultListener();

  try {
    for (let fi = 0; fi < loadedFiles.length; fi++) {
      if (signal.aborted) {
        showToast('Run stopped', 'info');
        break;
      }

      const file = loadedFiles[fi];
      const suite = getEnabledSuite(fi);
      if (!suite || suite.blocks.length === 0) continue;
      // Initialize live-streaming results for this file so per-block updates
      // populate file.results.block_results as each block completes.
      prepareLiveResults(fi);
      try {
        const fileExtraVars = [...collectVariablesArray(), ...azureExtraVars];
        // Prompt user for @prompt variables, if any
        const promptVars = await collectPromptVariables(file.suite, file.name);
        if (promptVars === null) continue;
        if (promptVars.length > 0) fileExtraVars.push(...promptVars);
        // When telemetry is disabled, strip telemetry endpoint variables so runner skips init
        let suiteCopy = suite;
        if (!telemetryEnabled) {
          suiteCopy = {
            ...suite,
            variables: suite.variables.filter(([k]) => !k.startsWith('telemetry_')),
          };
        } else {
          fileExtraVars.push(['__telemetry_file', file.name]);
        }
        const __sessions_started = Date.now();
        const results = await invoke('run_test_suite', {
          suite: suiteCopy,
          extraVariables: fileExtraVars,
          extraHeaders: getExtraHeaders(),
          runMode,
          fileName: file.name,
        });
        recordSessionRun(file, suiteCopy, file.content, results, fileExtraVars, runMode, __sessions_started).catch(()=>{});

        // Remap results to align with original block indices (this also
        // overwrites the live-streamed block_results with the canonical final
        // copy from Rust — same data, just guaranteed-complete).
        file.results = remapResults(fi, results);
        invalidateDiffCacheForFile(fi);
        pushRunHistory(file.name, results);
        processTelemetryResult(file.name, results);
        totalPassed += results.passed;
        totalFailed += results.failed;
        totalSkipped += results.skipped;
        totalTimeMs += results.total_time_ms;

        // Carry extracted variables forward to next files
        if (results.final_variables) {
          Object.entries(results.final_variables).forEach(([name, value]) => {
            runtimeOverrides[name] = value;
          });
        }

        file.results.block_results.forEach((br, origIdx) => {
          if (br) allBlockResults.push({ ...br, fileName: file.name, fileIdx: fi, blockIdx: origIdx });
        });
      } catch (err) {
        showToast(`Error running ${file.name}: ${err}`, 'error');
        rpLog('error', 'Command failed: run_test_suite', { file: file.name, error: String(err) });
        const errorResults = file.suite.blocks.map((b, blockIdx) => {
          if (disabledBlocks[`${fi}-${blockIdx}`]) return null;
          return {
            name: b.name,
            block_type: b.block_type,
            status: 'error',
            response: null,
            assertion_results: [],
            extract_results: [],
            error: String(err),
            time_ms: 0
          };
        });
        const enabledCount = errorResults.filter(Boolean).length;
        file.results = {
          passed: 0,
          failed: enabledCount,
          skipped: 0,
          total_time_ms: 0,
          block_results: errorResults,
          final_variables: {}
        };
        totalFailed += enabledCount;
        file.results.block_results.forEach((br, origIdx) => {
          if (br) allBlockResults.push({ ...br, fileName: file.name, fileIdx: fi, blockIdx: origIdx });
        });
      }

      // After file completes, show next file's blocks as pending
      const nextPending = (fi + 1 < pendingByFile.length) ? pendingByFile[fi + 1] : [];

      // Stream: update sidebar status dots + results bar after each file
      updateBlockStatuses();
      // Sync streaming counts with actual totals before rebuilding DOM
      streamingCounts = { passed: totalPassed, failed: totalFailed, skipped: totalSkipped, timeMs: totalTimeMs };
      showTestResults(totalPassed, totalFailed, totalSkipped, totalTimeMs, allBlockResults, nextPending);
      testResultsDetails.classList.remove('hidden');
    }

    renderEnvVars();

    // Re-render assertions for active block
    if (activeFileIndex >= 0 && activeBlockIndex >= 0) {
      renderAssertions(activeFileIndex, activeBlockIndex);
      const br = loadedFiles[activeFileIndex]?.results?.block_results?.[activeBlockIndex];
      if (br?.response) {
        displayResponse(br.response);
        lastResponse = br.response;
      }
    }

    if (!signal.aborted) {
      if (totalFailed === 0) {
        showToast(`All ${totalPassed} tests passed!`, 'success');
      } else {
        showToast(`${totalFailed} test(s) failed`, 'error');
      }
    }
  } finally {
    rpLog('info', 'Run All completed', { passed: totalPassed, failed: totalFailed, skipped: totalSkipped, timeMs: totalTimeMs });
    // If telemetry was "sending" but no results came back, update state
    if (telemetryState === 'sending') {
      if (telemetryStats.length === 0) {
        setTelemetryState('configured');
        rpLog('warn', 'OTEL telemetry: no telemetry data was returned from any file. Check that telemetry variables are correctly configured and populated.');
      }
    }
    // Re-check if monitor token is still valid → show ready instead of configured
    if (telemetryState === 'configured') {
      updateTelemetryAuthState();
    }
    stopBlockProgressListener();
    setRunning(false);
    refreshHistoryIfVisible();
  }
}

runAllBtn.addEventListener('click', runAllTests);

// Auto-run dropdown
const autoRunSelect = document.getElementById('autoRunSelect');
if (autoRunSelect) {
  autoRunSelect.addEventListener('change', () => {
    setAutoRun(autoRunSelect.value);
  });
}
// --- Run Single Block ---
async function runSingleBlock(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return;
  const block = file.suite.blocks[blockIdx];
  if (!block || (!block.request && !block.compare)) return;

  selectBlock(fileIdx, blockIdx);
  sendBtn.disabled = true;
  sendBtn.classList.add('loading');
  rpLog('info', `Single block run started: ${block.name || block.block_type}`, { file: file.name, blockIdx });

  try {
    // Run through the test suite engine so assertions, extracts, and variables work
    const singleSuite = { variables: file.suite.variables, blocks: [block] };
    let extraVars = collectVariablesArray();
    if (azureAuthState === 'authenticated') {
      const tokenVars = fetchDevModeToken(file.suite, file.content);
      if (tokenVars.length > 0) extraVars = [...extraVars, ...tokenVars];
    }
    const promptVars = await collectPromptVariables(file.suite, file.name);
    if (promptVars === null) return;
    if (promptVars.length > 0) extraVars = [...extraVars, ...promptVars];
    const __sessions_started1 = Date.now();
    const results = await invoke('run_test_suite', {
      suite: singleSuite,
      extraVariables: extraVars,
      extraHeaders: getExtraHeaders(),
      runMode: azureAuthState === 'authenticated' ? 'dev' : null,
      fileName: file.name,
    });
    recordSessionRun(file, singleSuite, file.content, results, extraVars, azureAuthState === 'authenticated' ? 'dev' : null, __sessions_started1).catch(()=>{});

    // Store result at the correct original block index
    if (!file.results) {
      file.results = {
        passed: 0, failed: 0, skipped: 0, total_time_ms: 0,
        block_results: new Array(file.suite.blocks.length).fill(null),
        final_variables: {}
      };
    }
    const br = results.block_results?.[0];
    if (br) {
      file.results.block_results[blockIdx] = br;
      invalidateDiffCacheEntry(fileIdx, blockIdx);
      // Carry extracted variables to env
      if (results.final_variables) {
        Object.entries(results.final_variables).forEach(([name, value]) => {
          runtimeOverrides[name] = value;
        });
      }
    }

    updateBlockStatuses();
    renderEnvPanel();

    if (br?.response) {
      lastResponse = br.response;
      displayResponse(br.response);
    } else if (br?.error) {
      displayError(br.error);
    }

    // Update assertions panel
    renderAssertions(fileIdx, blockIdx);

    const status = br?.status || 'error';
    showToast(`${block.name}: ${status}`, status === 'passed' ? 'success' : 'error');
    rpLog('info', `Single block run completed: ${block.name || block.block_type}`, { status, timeMs: br?.time_ms });
  } catch (err) {
    displayError(err);
    showToast(String(err), 'error');
    rpLog('error', 'Command failed: run_test_suite (single block)', String(err));
  } finally {
    sendBtn.disabled = false;
    sendBtn.classList.remove('loading');
    refreshHistoryIfVisible();
  }
}

// --- Run Single Compare Step ---
async function runCompareStep(fileIdx, blockIdx, stepIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return;
  const block = file.suite.blocks[blockIdx];
  if (!block || !block.compare || !block.steps?.[stepIdx]) return;

  const step = block.steps[stepIdx];

  // Construct a simple (non-compare) block from this step
  const tempBlock = {
    block_type: block.block_type,
    name: `${block.name} \u2192 ${step.name}`,
    description: `Step: ${step.name}`,
    request: step.request,
    assertions: step.assertions || [],
    extracts: step.extracts || [],
    compare: false,
    steps: [],
    diff: null,
    disabled: false,
    mode: block.mode || null,
    dev_auth_scope: block.dev_auth_scope || null,
    group: block.group || null,
    depends: [],
  };

  selectBlock(fileIdx, blockIdx);
  sendBtn.disabled = true;
  sendBtn.classList.add('loading');
  rpLog('info', `Compare step run started: ${block.name} → ${step.name}`, { file: file.name, blockIdx, stepIdx });

  try {
    const singleSuite = { variables: file.suite.variables, blocks: [tempBlock] };
    let extraVars = collectVariablesArray();
    if (azureAuthState === 'authenticated') {
      const tokenVars = fetchDevModeToken(file.suite, file.content);
      if (tokenVars.length > 0) extraVars = [...extraVars, ...tokenVars];
    }
    const promptVars = await collectPromptVariables(file.suite, file.name);
    if (promptVars === null) return;
    if (promptVars.length > 0) extraVars = [...extraVars, ...promptVars];
    const __sessions_started2 = Date.now();
    const results = await invoke('run_test_suite', {
      suite: singleSuite,
      extraVariables: extraVars,
      extraHeaders: getExtraHeaders(),
      runMode: azureAuthState === 'authenticated' ? 'dev' : null,
      fileName: file.name,
    });
    recordSessionRun(file, singleSuite, file.content, results, extraVars, azureAuthState === 'authenticated' ? 'dev' : null, __sessions_started2).catch(()=>{});

    const br = results.block_results?.[0];
    if (br) {
      if (!file.results) {
        file.results = {
          passed: 0, failed: 0, skipped: 0, total_time_ms: 0,
          block_results: new Array(file.suite.blocks.length).fill(null),
          final_variables: {}
        };
      }
      if (!file.results.block_results[blockIdx]) {
        file.results.block_results[blockIdx] = {
          seq: null, name: block.name, block_type: block.block_type,
          group: block.group, request_method: '', request_url: '',
          request_headers: [], request_body: null,
          status: 'pending', response: null,
          assertion_results: [], extract_results: [],
          error: null, time_ms: 0, step_results: [], diff_result: null
        };
      }
      const parentBr = file.results.block_results[blockIdx];
      while (parentBr.step_results.length <= stepIdx) {
        parentBr.step_results.push(null);
      }
      parentBr.step_results[stepIdx] = {
        name: step.name,
        request_method: br.request_method,
        request_url: br.request_url,
        request_headers: br.request_headers,
        request_body: br.request_body,
        response: br.response,
        assertion_results: br.assertion_results || [],
        extract_results: br.extract_results || [],
        time_ms: br.time_ms,
        error: br.error
      };

      if (results.final_variables) {
        Object.entries(results.final_variables).forEach(([name, value]) => {
          runtimeOverrides[name] = value;
        });
      }
      invalidateDiffCacheEntry(fileIdx, blockIdx);
    }

    updateBlockStatuses();
    renderFileTree();
    renderEnvPanel();

    if (br?.response) {
      lastResponse = br.response;
      displayResponse(br.response);
    } else if (br?.error) {
      displayError(br.error);
    }

    renderAssertions(fileIdx, blockIdx);

    const status = br?.status || 'done';
    showToast(`${step.name}: ${status}`, status === 'passed' ? 'success' : status === 'failed' ? 'error' : 'info');
    rpLog('info', `Compare step run completed: ${step.name}`, { status, timeMs: br?.time_ms });
  } catch (err) {
    displayError(err);
    showToast(String(err), 'error');
    rpLog('error', 'Command failed: run_test_suite (compare step)', String(err));
  } finally {
    sendBtn.disabled = false;
    sendBtn.classList.remove('loading');
    refreshHistoryIfVisible();
  }
}

// --- View Compare Step ---
function viewCompareStep(fileIdx, blockIdx, stepIdx) {
  const file = loadedFiles[fileIdx];
  const block = file?.suite?.blocks?.[blockIdx];
  const step = block?.steps?.[stepIdx];
  if (!step) return;

  activeFileIndex = fileIdx;
  activeBlockIndex = blockIdx;

  if (step.request) {
    methodSelect.value = step.request.method || 'GET';
    updateMethodColor();
    urlInput.value = step.request.url || '';
    headersContainer.innerHTML = '';
    if (step.request.headers?.length > 0) {
      step.request.headers.forEach(([k, v]) => headersContainer.appendChild(createHeaderRow(k, v)));
    } else {
      headersContainer.appendChild(createHeaderRow());
    }
    if (step.request.body) {
      bodyType.value = pickBodyType(step.request.headers, step.request.body);
      bodyInput.value = step.request.body;
      bodyInput.disabled = false;
    } else {
      bodyType.value = 'none';
      bodyInput.value = '';
      bodyInput.disabled = true;
    }
    bodyType.dispatchEvent(new Event('change'));
  }

  const br = file.results?.block_results?.[blockIdx];
  const sr = br?.step_results?.[stepIdx];
  if (sr?.response) {
    displayResponse(sr.response);
    lastResponse = sr.response;
  }

  assertionsTab.style.display = '';
  renderAssertions(fileIdx, blockIdx);

  updateActiveHighlight();

  if (typeof highlightBody === 'function') highlightBody();
}

// --- Test Results Bar ---
function showTestResults(passed, failed, skipped, timeMs, blockResults, pendingBlocks) {
  testResultsBar.classList.remove('hidden');

  const pendingCount = pendingBlocks ? pendingBlocks.length : 0;
  const total = passed + failed + skipped + pendingCount;

  $('#resultsPassed').textContent = passed;
  $('#resultsFailed').textContent = failed;
  $('#resultsSkipped').textContent = skipped;
  $('#resultsTotalTime').textContent = pendingCount > 0 ? '…' : timeMs;

  if (total > 0) {
    $('#progressPassed').style.width = `${(passed / total) * 100}%`;
    $('#progressFailed').style.width = `${(failed / total) * 100}%`;
    $('#progressSkipped').style.width = `${(skipped / total) * 100}%`;
  } else {
    $('#progressPassed').style.width = '0%';
    $('#progressFailed').style.width = '0%';
    $('#progressSkipped').style.width = '0%';
  }

  // Render details
  testResultsDetails.innerHTML = '';

  // Completed results
  blockResults.forEach(br => {
    const row = document.createElement('div');
    const viewKey = `${br.fileIdx}-${br.blockIdx}`;
    const hash = resultHash(br);
    const viewState = getViewedState(viewedResults, viewKey, hash);
    row.className = `result-detail-row${viewState !== 'unseen' ? ` ${viewState}` : ''}`;

    const assertionCount = br.assertion_results?.length || 0;
    const assertionPassed = br.assertion_results?.filter(a => a.passed).length || 0;
    const allPassed = assertionCount > 0 && assertionPassed === assertionCount;
    const assertClass = assertionCount === 0 ? '' : allPassed ? 'all-passed' : 'has-failed';
    const assertText = assertionCount > 0 ? `${assertionPassed}/${assertionCount}` : '';

    const extractCount = br.extract_results?.length || 0;
    const extractOk = br.extract_results?.filter(e => e.success).length || 0;
    const extractText = extractCount > 0 ? `${extractOk}/${extractCount} vars` : '';

    const stepCount = br.step_results?.length || 0;
    const stepText = stepCount > 0 ? `${stepCount} steps` : '';
    const diffMatch = br.diff_result ? (br.diff_result.match_exact ? '\u2713 match' : `${(br.diff_result.similarity * 100).toFixed(0)}% similar`) : '';

    // Determine failure reason
    let failReasonHtml = '';
    if (br.status === 'failed' || br.status === 'error') {
      const failedAssertions = br.assertion_results?.filter(a => !a.passed) || [];
      const hasAssertionFail = failedAssertions.length > 0;
      const hasError = !!br.error;
      const httpStatus = br.response?.status;
      const isHttpError = httpStatus && httpStatus >= 400;

      if (hasError) {
        failReasonHtml = `<span class="detail-fail-reason fail-error" title="${escapeAttr(br.error)}">⚠ Error</span>`;
      } else if (isHttpError && hasAssertionFail) {
        failReasonHtml = `<span class="detail-fail-reason fail-both" title="HTTP ${httpStatus} + ${failedAssertions.length} assertion(s) failed">${httpStatus} + ✗${failedAssertions.length}</span>`;
      } else if (isHttpError && !hasAssertionFail) {
        failReasonHtml = `<span class="detail-fail-reason fail-status" title="HTTP ${httpStatus}">HTTP ${httpStatus}</span>`;
      } else if (hasAssertionFail) {
        failReasonHtml = `<span class="detail-fail-reason fail-assertion" title="${escapeAttr(failedAssertions.map(a => a.assertion || `${a.actual} ≠ ${a.expected}`).join(', '))}">✗ ${failedAssertions.length} assert</span>`;
      }
    }

    const viewedIcon = viewState === 'seen' ? '<span class="detail-viewed" title="Viewed">👁</span>'
      : viewState === 'seen-mutated' ? '<span class="detail-viewed mutated" title="Changed since last viewed">👁✱</span>'
      : '';

    row.innerHTML = `
      <span class="detail-status ${br.status}"></span>
      <span class="detail-name">${escapeHtml(br.fileName ? br.fileName + ' \u2192 ' : '')}${escapeHtml(br.name)}</span>
      ${failReasonHtml}
      ${assertText ? `<span class="detail-assertions ${assertClass}">${assertText}</span>` : ''}
      ${extractText ? `<span class="detail-extracts">${extractText}</span>` : ''}
      ${stepText ? `<span class="detail-steps">${stepText}</span>` : ''}
      ${diffMatch ? `<span class="detail-diff ${br.diff_result?.match_exact ? 'diff-match' : 'diff-mismatch'}">${diffMatch}</span>` : ''}
      <span class="detail-time">${br.time_ms} ms</span>
      ${viewedIcon}
    `;

    if (br.fileIdx !== undefined && br.blockIdx !== undefined) {
      row.style.cursor = 'pointer';
      row.addEventListener('click', () => {
        viewedResults.set(viewKey, { hash });
        row.className = 'result-detail-row seen';
        const vi = row.querySelector('.detail-viewed');
        if (vi) { vi.textContent = '👁'; vi.title = 'Viewed'; vi.classList.remove('mutated'); }
        else row.insertAdjacentHTML('beforeend', '<span class="detail-viewed" title="Viewed">👁</span>');
        // Expand parent file node if collapsed
        const fileNode = fileTree.querySelector(`.file-node[data-file-idx="${br.fileIdx}"]`);
        if (fileNode && !fileNode.classList.contains('expanded')) {
          fileNode.classList.add('expanded');
          treeExpandState[`file-${br.fileIdx}`] = true;
        }

        // Also expand any group node containing this block
        const blockItem = fileNode?.querySelector(`.block-item[data-block-idx="${br.blockIdx}"]`);
        if (blockItem) {
          const parentGroup = blockItem.closest('.group-node');
          if (parentGroup && !parentGroup.classList.contains('expanded')) {
            parentGroup.classList.add('expanded');
            const expandKey = parentGroup.dataset.expandKey;
            if (expandKey) treeExpandState[expandKey] = true;
          }
        }

        if (currentMode !== 'builder') {
          switchMode('builder');
        }

        selectBlock(br.fileIdx, br.blockIdx);

        // Scroll the block into view in sidebar
        const selectedItem = fileNode?.querySelector(`.block-item[data-block-idx="${br.blockIdx}"]`);
        if (selectedItem) {
          selectedItem.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
        }
      });
    }

    testResultsDetails.appendChild(row);
  });

  // Pending / running rows — initially all shown as queued, Rust events will promote to running/completed
  if (pendingBlocks) {
    pendingBlocks.forEach(pb => {
      const row = document.createElement('div');
      row.className = 'result-detail-row queued';
      row.dataset.blockName = pb.name;
      row.dataset.fileIdx = pb.fileIdx;
      row.dataset.blockIdx = pb.blockIdx;
      row.innerHTML = `
        <span class="detail-status queued"></span>
        <span class="detail-name">${escapeHtml(pb.fileName ? pb.fileName + ' \u2192 ' : '')}${escapeHtml(pb.name)}</span>
        <span class="detail-time queued-label"></span>
      `;
      testResultsDetails.appendChild(row);
    });
  }

  // Auto-expand details if there are failures
  if (failed > 0) {
    testResultsDetails.classList.remove('hidden');
  }
}

// --- Block Progress Streaming ---
// Listens for Tauri "block-progress" events and updates result rows in-place.
let blockProgressUnlisten = null;
let streamingCounts = { passed: 0, failed: 0, skipped: 0, timeMs: 0 };

async function startBlockProgressListener() {
  streamingCounts = { passed: 0, failed: 0, skipped: 0, timeMs: 0 };
  blockProgressUnlisten = await listen('block-progress', (event) => {
    const p = event.payload;
    // Find the pending row by block name
    const row = testResultsDetails.querySelector(`.result-detail-row[data-block-name="${CSS.escape(p.name)}"]`);
    if (!row) return;

    if (p.status === 'running') {
      // Transition from queued → running
      row.className = 'result-detail-row pending';
      const dot = row.querySelector('.detail-status');
      if (dot) dot.className = 'detail-status running';
      const timeEl = row.querySelector('.detail-time');
      if (timeEl) { timeEl.className = 'detail-time pending-dots'; timeEl.textContent = ''; }
    } else {
      // Completed: passed/failed/error/skipped — update in place
      row.className = 'result-detail-row';
      row.style.cursor = 'pointer';

      const dot = row.querySelector('.detail-status');
      if (dot) dot.className = `detail-status ${p.status}`;

      const timeEl = row.querySelector('.detail-time');
      if (timeEl) { timeEl.className = 'detail-time'; timeEl.textContent = `${p.time_ms} ms`; }

      // Add failure reason badge
      if (p.status === 'failed' || p.status === 'error') {
        const failSpan = document.createElement('span');
        const hasAssertFail = p.assertion_total > 0 && p.assertion_passed < p.assertion_total;
        const isHttpError = p.http_status && p.http_status >= 400;
        const hasError = !!p.error;

        if (hasError) {
          failSpan.className = 'detail-fail-reason fail-error';
          failSpan.title = p.error;
          failSpan.textContent = '⚠ Error';
        } else if (isHttpError && hasAssertFail) {
          failSpan.className = 'detail-fail-reason fail-both';
          failSpan.title = `HTTP ${p.http_status} + ${p.assertion_total - p.assertion_passed} assertion(s) failed`;
          failSpan.textContent = `${p.http_status} + ✗${p.assertion_total - p.assertion_passed}`;
        } else if (isHttpError) {
          failSpan.className = 'detail-fail-reason fail-status';
          failSpan.title = `HTTP ${p.http_status}`;
          failSpan.textContent = `HTTP ${p.http_status}`;
        } else if (hasAssertFail) {
          failSpan.className = 'detail-fail-reason fail-assertion';
          failSpan.textContent = `✗ ${p.assertion_total - p.assertion_passed} assert`;
        }
        if (failSpan.textContent) {
          const nameEl = row.querySelector('.detail-name');
          if (nameEl) nameEl.after(failSpan);
        }
      }

      // Add assertion/extract info
      if (p.assertion_total > 0) {
        const allPassed = p.assertion_passed === p.assertion_total;
        const assertSpan = document.createElement('span');
        assertSpan.className = `detail-assertions ${allPassed ? 'all-passed' : 'has-failed'}`;
        assertSpan.textContent = `${p.assertion_passed}/${p.assertion_total}`;
        timeEl.parentNode.insertBefore(assertSpan, timeEl);
      }
      if (p.extract_total > 0) {
        const extSpan = document.createElement('span');
        extSpan.className = 'detail-extracts';
        extSpan.textContent = `${p.extract_ok}/${p.extract_total} vars`;
        timeEl.parentNode.insertBefore(extSpan, timeEl);
      }

      // Make clickable to inspect results and mark viewed
      const fIdx = parseInt(row.dataset.fileIdx);
      const bIdx = parseInt(row.dataset.blockIdx);
      row.addEventListener('click', () => {
        const vk = `${fIdx}-${bIdx}`;
        const h = `${p.status}:${p.time_ms}:${p.http_status || 0}`;
        viewedResults.set(vk, { hash: h });
        row.className = 'result-detail-row seen';
        const vi = row.querySelector('.detail-viewed');
        if (vi) { vi.textContent = '👁'; vi.title = 'Viewed'; vi.classList.remove('mutated'); }
        else row.insertAdjacentHTML('beforeend', '<span class="detail-viewed" title="Viewed">👁</span>');
        // Expand parent file node if collapsed
        const fileNode = fileTree.querySelector(`.file-node[data-file-idx="${fIdx}"]`);
        if (fileNode && !fileNode.classList.contains('expanded')) {
          fileNode.classList.add('expanded');
          treeExpandState[`file-${fIdx}`] = true;
        }

        // Also expand any group node containing this block
        const blockItem = fileNode?.querySelector(`.block-item[data-block-idx="${bIdx}"]`);
        if (blockItem) {
          const parentGroup = blockItem.closest('.group-node');
          if (parentGroup && !parentGroup.classList.contains('expanded')) {
            parentGroup.classList.add('expanded');
            const expandKey = parentGroup.dataset.expandKey;
            if (expandKey) treeExpandState[expandKey] = true;
          }
        }

        if (currentMode !== 'builder') {
          switchMode('builder');
        }

        selectBlock(fIdx, bIdx);

        // Scroll the block into view in sidebar
        const selectedItem = fileNode?.querySelector(`.block-item[data-block-idx="${bIdx}"]`);
        if (selectedItem) {
          selectedItem.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
        }
      });

      // Update streaming counts and progress bar
      if (p.status === 'passed') streamingCounts.passed++;
      else if (p.status === 'failed' || p.status === 'error') streamingCounts.failed++;
      else if (p.status === 'skipped') streamingCounts.skipped++;
      streamingCounts.timeMs += p.time_ms;

      const sc = streamingCounts;
      const pendingLeft = testResultsDetails.querySelectorAll('.result-detail-row.pending, .result-detail-row.queued').length;
      const total = sc.passed + sc.failed + sc.skipped + pendingLeft;
      $('#resultsPassed').textContent = sc.passed;
      $('#resultsFailed').textContent = sc.failed;
      $('#resultsSkipped').textContent = sc.skipped;
      $('#resultsTotalTime').textContent = pendingLeft > 0 ? '…' : sc.timeMs;
      if (total > 0) {
        $('#progressPassed').style.width = `${(sc.passed / total) * 100}%`;
        $('#progressFailed').style.width = `${(sc.failed / total) * 100}%`;
        $('#progressSkipped').style.width = `${(sc.skipped / total) * 100}%`;
      }

      // Update sidebar status dots
      updateBlockStatuses();
    }

    // Force browser repaint via requestAnimationFrame
    requestAnimationFrame(() => {});
  });
}

function stopBlockProgressListener() {
  if (blockProgressUnlisten) {
    blockProgressUnlisten();
    blockProgressUnlisten = null;
  }
  if (blockResultUnlisten) {
    blockResultUnlisten();
    blockResultUnlisten = null;
  }
  liveResultPendingByFile.clear();
}

// --- Block Result Streaming ---
// `block-result` carries the FULL BlockResult (response, assertions, extracts)
// per block as it completes. We stitch results into `file.results.block_results`
// live so the sidebar status dots, group rollups, and click-to-inspect work
// during an in-flight run — instead of waiting for the whole file to finish.
let blockResultUnlisten = null;

// Maps fileIdx → array of *unmatched* original block indices for the current
// run. Each block-result event consumes the first matching name, ensuring
// correct order even with duplicate block names. Set up by `prepareLiveResults`.
const liveResultPendingByFile = new Map();

/**
 * Register a file as participating in the live streaming results. Initializes
 * the file's results skeleton and pushes its enabled-block original indices
 * into `liveResultPendingByFile`. Call before invoking `run_test_suite`.
 */
function prepareLiveResults(fileIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return;
  const enabledMap = getEnabledIndexMap(fileIdx);
  // Reset the file's results to an empty skeleton so getBlockStatus() returns
  // '' for unfinished blocks (rather than the stale prior-run status).
  file.results = {
    passed: 0,
    failed: 0,
    skipped: 0,
    total_time_ms: 0,
    block_results: new Array(file.suite.blocks.length).fill(null),
    final_variables: {},
  };
  liveResultPendingByFile.set(fileIdx, [...enabledMap]);
  invalidateDiffCacheForFile(fileIdx);
}

async function startBlockResultListener() {
  blockResultUnlisten = await listen('block-result', (event) => {
    const br = event.payload;
    if (!br || !br.name) return;
    // Find which file this block belongs to. Match by name within the
    // unmatched-set for each running file (first match wins, then consume).
    for (const [fIdx, indices] of liveResultPendingByFile.entries()) {
      const f = loadedFiles[fIdx];
      if (!f || !f.suite) continue;
      const matchPos = indices.findIndex(i => f.suite.blocks[i] && f.suite.blocks[i].name === br.name);
      if (matchPos >= 0) {
        const origIdx = indices[matchPos];
        f.results.block_results[origIdx] = br;
        indices.splice(matchPos, 1);
        // Repaint sidebar status dots + group rollups
        updateBlockStatuses();
        // If user is currently viewing this exact block, refresh the response
        // panel so they see the just-arrived data without a manual click.
        if (activeFileIndex === fIdx && activeBlockIndex === origIdx) {
          selectBlock(fIdx, origIdx);
        }
        break;
      }
    }
  });
}

// Toggle test results details
testResultsSummary.addEventListener('click', () => {
  testResultsDetails.classList.toggle('hidden');
});

// --- Env File Management (collapsible cards in left sidebar) ---

// Per-loaded-file UI state for the read-only ".http variables" cards we
// render at the top of the env panel: { expanded: bool } keyed by file path.
let httpFileVarCardState = {};

// Public alias kept so the many existing `renderEnvVars()` call sites keep
// working without churn — they all need to redraw the env panel.
function renderEnvVars() { renderEnvPanel(); }

function renderEnvPanel() {
  if (!envList) return;
  envList.innerHTML = '';

  // ── Section 1: collapsed read-only card per loaded .http file with @variables ──
  // This shows the user "where their variables came from" without unrolling
  // 25 individual rows in the sidebar.
  const httpFilesWithVars = (loadedFiles || []).filter(f => f.suite && f.suite.variables && f.suite.variables.length > 0);
  if (httpFilesWithVars.length > 0) {
    const sectionLabel = document.createElement('div');
    sectionLabel.className = 'env-section-label';
    sectionLabel.textContent = 'From loaded .http files';
    envList.appendChild(sectionLabel);
    httpFilesWithVars.forEach((file) => {
      envList.appendChild(buildHttpFileVarCard(file));
    });
  }

  const entries = envConfig.entries || [];

  if (entries.length > 0) {
    const sectionLabel = document.createElement('div');
    sectionLabel.className = 'env-section-label';
    sectionLabel.textContent = '.env files';
    envList.appendChild(sectionLabel);
  }

  if (entries.length === 0 && httpFilesWithVars.length === 0) {
    envList.innerHTML = '<div class="sidebar-empty">No .env file loaded · click 📂 above to load one.</div>';
    return;
  }

  if (entries.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'sidebar-empty';
    empty.style.cssText = 'padding:6px 4px;font-size:11px;';
    empty.textContent = 'No .env file loaded · click 📂 above to load one.';
    envList.appendChild(empty);
  }

  entries.forEach((entry, i) => {
    const isActive = i === envConfig.active_index;
    const state = envCardState[i] || (envCardState[i] = defaultCardState());

    const card = document.createElement('div');
    card.className = 'env-card' + (isActive ? ' active' : '') + (state.expanded ? ' expanded' : '') + (state.dirty ? ' dirty' : '');

    // ── Header ──
    const header = document.createElement('div');
    header.className = 'env-card-header';
    const chevron = document.createElement('span');
    chevron.className = 'env-card-chevron';
    chevron.textContent = '▸';
    const radio = document.createElement('span');
    radio.className = 'env-card-radio';
    radio.title = isActive ? 'Active env (click to deactivate)' : 'Click to activate this env';
    const order = document.createElement('span');
    order.className = 'env-card-order';
    order.textContent = `${i + 1}.`;
    order.title = 'Load order — variables defined later override earlier ones when this env is active.';
    const name = document.createElement('span');
    name.className = 'env-card-name';
    name.textContent = entry.name || 'env';
    const path = document.createElement('span');
    path.className = 'env-card-path';
    path.textContent = shortenPath(entry.path);
    path.title = entry.path;
    const dirty = document.createElement('span');
    dirty.className = 'env-card-dirty';
    dirty.title = 'Unsaved edits';
    const save = document.createElement('button');
    save.className = 'env-card-action save';
    save.textContent = '💾';
    save.title = state.dirty ? 'Save edits to disk' : 'No unsaved edits';
    save.disabled = !state.dirty;
    const remove = document.createElement('button');
    remove.className = 'env-card-action remove';
    remove.textContent = '✕';
    remove.title = 'Remove from list';

    header.appendChild(chevron);
    header.appendChild(radio);
    header.appendChild(order);
    header.appendChild(name);
    header.appendChild(path);
    header.appendChild(dirty);
    header.appendChild(save);
    header.appendChild(remove);
    card.appendChild(header);

    // ── Body ──
    const body = document.createElement('div');
    body.className = 'env-card-body' + (isActive ? '' : ' readonly');
    card.appendChild(body);

    // ── Wire up handlers ──
    chevron.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      state.expanded = !state.expanded;
      if (state.expanded && !state.vars) {
        await ensureCardVarsLoaded(i);
      }
      renderEnvPanel();
    });

    radio.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (isActive) {
        await activateEnvIndex(null);
      } else {
        await activateEnvIndex(i);
      }
    });

    // Header click (anywhere except buttons) toggles expand
    header.addEventListener('click', async (ev) => {
      if (ev.target === radio || ev.target === save || ev.target === remove) return;
      if (ev.target === chevron) return;
      state.expanded = !state.expanded;
      if (state.expanded && !state.vars) {
        await ensureCardVarsLoaded(i);
      }
      renderEnvPanel();
    });

    save.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (state.dirty) await saveEnvIndex(i);
    });

    remove.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (state.dirty) {
        const ok = await showModalConfirm({
          title: 'Discard unsaved edits?',
          message: `${entry.name} has unsaved changes. Removing will discard them.`,
          okLabel: 'Discard & Remove',
          cancelLabel: 'Keep',
          danger: true,
        });
        if (!ok) return;
      }
      await removeEnvIndex(i);
    });

    // ── Body content ──
    if (state.expanded) {
      renderEnvCardBody(body, i, entry, isActive, state);
    }

    envList.appendChild(card);
  });

  // Footer: built-ins reminder
  const builtins = document.createElement('div');
  builtins.className = 'env-builtins-hint';
  builtins.style.cssText = 'font-size:10px;color:var(--text-muted);padding:6px 4px 0 4px;border-top:1px dashed var(--border);margin-top:6px;';
  builtins.innerHTML = 'Built-ins: <code>{{$timestamp}}</code> <code>{{$uuid}}</code> <code>{{$randomInt}}</code>';
  envList.appendChild(builtins);

  // Env vars just changed — refresh tooltips on every {{var}} chip in URL,
  // headers, params, and form-field cells so they show the new resolved
  // values (and switch from `(undefined)` to a value, or vice versa).
  refreshAllVariableOverlays();
}

function buildHttpFileVarCard(file) {
  const key = file.path || file.name || '';
  if (!httpFileVarCardState[key]) httpFileVarCardState[key] = { expanded: false };
  const state = httpFileVarCardState[key];
  const vars = file.suite.variables || [];

  const card = document.createElement('div');
  card.className = 'env-card http-file-card readonly' + (state.expanded ? ' expanded' : '');

  const header = document.createElement('div');
  header.className = 'env-card-header';
  const chevron = document.createElement('span');
  chevron.className = 'env-card-chevron';
  chevron.textContent = '▸';
  const icon = document.createElement('span');
  icon.className = 'env-card-radio';
  icon.style.cssText = 'border:none;background:transparent;color:var(--text-muted);font-size:12px;';
  icon.textContent = '📄';
  icon.title = '.http file variables (read-only)';
  const name = document.createElement('span');
  name.className = 'env-card-name';
  name.textContent = file.name || key.split(/[\\/]/).pop();
  const count = document.createElement('span');
  count.className = 'env-card-path';
  count.textContent = `${vars.length} var${vars.length === 1 ? '' : 's'}`;
  count.title = key;

  header.appendChild(chevron);
  header.appendChild(icon);
  header.appendChild(name);
  header.appendChild(count);
  card.appendChild(header);

  const body = document.createElement('div');
  body.className = 'env-card-body readonly';
  card.appendChild(body);

  if (state.expanded) {
    if (vars.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'env-card-loading';
      empty.textContent = '(no variables)';
      body.appendChild(empty);
    } else {
      vars.forEach(([varName, varValue]) => {
        const isPlaceholder = typeof varValue === 'string'
          && (varValue.startsWith('your-') || varValue.includes('your-'));
        const valueSet = varValue !== undefined && varValue !== '' && !isPlaceholder;
        const statusColor = valueSet ? 'var(--green)' : 'var(--red)';
        const row = document.createElement('div');
        row.className = 'var-row';
        row.innerHTML = `
          <span class="var-status" style="color:${statusColor}">●</span>
          <span class="var-icon" title=".http variable">📄</span>
          <input class="var-name" value="${escapeAttr(varName)}" disabled>
          <span class="var-sep">=</span>
          <input class="var-value" value="${escapeAttr(varValue || '')}" disabled placeholder="${isPlaceholder ? 'placeholder — override in .env' : 'unset'}">
          <button class="var-delete" disabled style="visibility:hidden;">✕</button>
        `;
        attachVarRowTooltip(row, () => [varName, varValue || '']);
        body.appendChild(row);
      });
    }
  }

  const toggle = (ev) => {
    ev.stopPropagation();
    state.expanded = !state.expanded;
    renderEnvPanel();
  };
  chevron.addEventListener('click', toggle);
  header.addEventListener('click', (ev) => {
    if (ev.target === chevron) return;
    toggle(ev);
  });

  return card;
}

function renderEnvCardBody(body, index, entry, isActive, state) {
  body.innerHTML = '';
  if (state.loading) {
    const loading = document.createElement('div');
    loading.className = 'env-card-loading';
    loading.textContent = 'Loading…';
    body.appendChild(loading);
    return;
  }

  // Edits live on draftVars when active+dirty; fall back to vars
  const source = (isActive && state.draftVars) ? state.draftVars : (state.vars || []);

  if (source.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'env-card-loading';
    empty.textContent = '(no variables in this file)';
    body.appendChild(empty);
  }

  source.forEach(([varName, varValue], rowIdx) => {
    const row = document.createElement('div');
    row.className = 'var-row';
    const valueSet = varValue !== undefined && varValue !== '';
    const isPlaceholder = typeof varValue === 'string' && (varValue.startsWith('your-') || varValue.includes('your-'));
    const statusColor = valueSet && !isPlaceholder ? 'var(--green)' : 'var(--red)';

    row.innerHTML = `
      <span class="var-status" style="color:${statusColor}">●</span>
      <span class="var-icon" title=".env variable">🔐</span>
      <input class="var-name" value="${escapeAttr(varName)}" spellcheck="false" ${isActive ? '' : 'disabled'}>
      <span class="var-sep">=</span>
      <input class="var-value" value="${escapeAttr(varValue || '')}" spellcheck="false" placeholder="value" ${isActive ? '' : 'disabled'}>
      <button class="var-delete" title="Delete variable" ${isActive ? '' : 'disabled'}>✕</button>
    `;

    // Hover tooltip — shows decoded JWT, base64, etc. for the value.
    attachVarRowTooltip(row, () => {
      const currentName = row.querySelector('.var-name')?.value || varName;
      const currentVal = row.querySelector('.var-value')?.value || varValue || '';
      return [currentName, currentVal];
    });

    if (isActive) {
      const nameInput = row.querySelector('.var-name');
      const valueInput = row.querySelector('.var-value');
      const deleteBtn = row.querySelector('.var-delete');
      nameInput.addEventListener('change', () => {
        const draft = ensureDraft(state);
        const newName = nameInput.value.trim();
        if (newName) {
          draft[rowIdx][0] = newName;
          markDirty(state);
          renderEnvPanel();
        }
      });
      valueInput.addEventListener('input', () => {
        const draft = ensureDraft(state);
        draft[rowIdx][1] = valueInput.value;
        markDirty(state);
        // Update header dirty indicator without full rerender (keeps focus)
        const headerEl = body.parentElement && body.parentElement.querySelector('.env-card-header');
        if (headerEl) {
          body.parentElement.classList.add('dirty');
          const saveBtn = headerEl.querySelector('.env-card-action.save');
          if (saveBtn) saveBtn.disabled = false;
        }
      });
      deleteBtn.addEventListener('click', () => {
        const draft = ensureDraft(state);
        draft.splice(rowIdx, 1);
        markDirty(state);
        renderEnvPanel();
      });
    }

    body.appendChild(row);
  });

  if (isActive) {
    const addBtn = document.createElement('button');
    addBtn.className = 'env-card-add';
    addBtn.textContent = '+ Add variable';
    addBtn.addEventListener('click', () => {
      const draft = ensureDraft(state);
      draft.push(['', '']);
      markDirty(state);
      renderEnvPanel();
      // focus the new name input
      setTimeout(() => {
        const inputs = envList.querySelectorAll('.env-card.active .var-row .var-name');
        const last = inputs[inputs.length - 1];
        if (last) { last.focus(); last.select(); }
      }, 0);
    });
    body.appendChild(addBtn);
  } else {
    const hint = document.createElement('div');
    hint.className = 'env-card-loading';
    hint.style.cssText = 'text-align:center;cursor:pointer;color:var(--text-muted);';
    hint.textContent = 'Click ○ to activate · these values are read-only preview';
    hint.addEventListener('click', () => activateEnvIndex(index));
    body.appendChild(hint);
  }
}

function ensureDraft(state) {
  if (!state.draftVars) {
    state.draftVars = (state.vars || []).map(([k, v]) => [k, v]);
  }
  return state.draftVars;
}

function markDirty(state) {
  state.dirty = true;
}

async function envPickerLoadFile() {
  // Use the lightweight HTML <input type="file"> picker, same as the .http file
  // open flow. On Tauri 2 / WebView2 this opens a smaller, native-feeling dialog
  // (the rfd::FileDialog crate opens the heavier full-screen Explorer-style
  // picker, which the user explicitly disliked).
  //
  // Tauri 2 augments the resulting File object with `.path` (absolute path) for
  // most local files. When that's present we forward it to env_add directly.
  // For the rare cases where `.path` is missing (e.g. files synthesized by other
  // sources), we fall back to reading content via file.text() and persisting it
  // to a managed location via the env_save_uploaded_content command — the entry
  // still has a real, readable file backing it, so env_add never fails with the
  // legacy "os error 2 — file not found".
  const input = document.getElementById('envFileInputHidden');
  if (!input) {
    showToast('Env file picker unavailable', 'error');
    return;
  }
  // Reset value so picking the same file twice still fires `change`.
  input.value = '';
  input.click();
}

async function _handleEnvFilePicked(file) {
  if (!file) return;
  let path = (file.path && typeof file.path === 'string' && file.path.trim()) || null;

  // Fallback: if the picked File object didn't expose an absolute path,
  // persist its content to a managed location and use that as the path.
  if (!path) {
    try {
      const content = (typeof file.text === 'function') ? await file.text() : '';
      const name = file.name || 'uploaded.env';
      path = await invoke('env_save_uploaded_content', { name, content });
      rpLog('info', 'env file path unavailable; persisted content to managed location', { name, path });
    } catch (err) {
      rpLog('error', 'env_save_uploaded_content failed', { error: String(err) });
      showToast('Failed to load .env: ' + String(err), 'error');
      return;
    }
  }

  try {
    const res = await invoke('env_add', { path });
    envConfig = normalizeEnvConfig(res.config);
    activeEnvVars = res.active_vars || [];
    syncEnvCardState(true);
    const addedIdx = envConfig.entries.length - 1;
    if (addedIdx >= 0) {
      if (!envCardState[addedIdx]) envCardState[addedIdx] = defaultCardState();
      envCardState[addedIdx].vars = (res.loaded_vars || []).map(([k, v]) => [k, v]);
      envCardState[addedIdx].expanded = true;
    }
    seedActiveCardVars();
    renderEnvPanel();
    const added = envConfig.entries[addedIdx];
    if (added) showToast(`Loaded env: ${added.name}`, 'success');
  } catch (err) {
    rpLog('error', 'env_add failed', { error: String(err) });
    showToast('Failed to load .env: ' + String(err), 'error');
  }
}

/**
 * Author a new .env file via the system save-file dialog.
 * Workflow: prompt for a friendly display name → save dialog → write empty
 * file with `# @@name <display>` directive header → env_add → activate.
 */
async function envCreateNewFile() {
  const displayName = await showModalPrompt({
    title: 'New environment file',
    message: 'Pick a friendly display name for this env. You can change it later by editing the file header.',
    defaultValue: '',
    placeholder: 'dev, staging, prod, …',
    hint: 'The name appears on the env card. Leave blank to use the filename.',
    okLabel: 'Continue',
  });
  if (displayName === null) return;
  const name = (displayName || '').trim();
  // Suggest a filename based on the display name; default to `.env` if empty.
  const suggested = name ? `${name.toLowerCase().replace(/[^a-z0-9._-]+/g, '-')}.env` : '.env';
  let path;
  try {
    path = await invoke('save_file_with_dialog', {
      title: 'Create new .env file',
      defaultName: suggested,
      content: '',
      filters: [['Env Files', 'env'], ['All Files', '*']],
    });
  } catch (err) {
    rpLog('error', 'save_file_with_dialog failed', { error: String(err) });
    showToast('Failed to open save dialog: ' + String(err), 'error');
    return;
  }
  if (!path) return;
  // The save dialog already wrote an empty file. Replace it with our header.
  try {
    await invoke('env_create_file', { path, displayName: name || null });
  } catch (err) {
    // env_create_file fails if the file already exists (save_file_with_dialog
    // wrote it). Fallback: write the directive directly via env_save.
    rpLog('warn', 'env_create_file rejected (already exists), falling back', { error: String(err) });
  }
  // Add to env config and activate. env_add will read whatever's on disk —
  // which may be empty. That's fine; user will fill in vars via the card UI.
  await _handleEnvFilePicked({ path });
}

// --- Load .env file (header button) ---
if (loadEnvBtn) {
  loadEnvBtn.addEventListener('click', async (e) => {
    e.stopPropagation();
    await envPickerLoadFile();
  });
}

const newEnvBtnEl = document.getElementById('newEnvBtn');
if (newEnvBtnEl) {
  newEnvBtnEl.addEventListener('click', async (e) => {
    e.stopPropagation();
    await envCreateNewFile();
  });
}

// Hidden HTML <input type="file"> for env files → forwards picked File to
// _handleEnvFilePicked which trusts .path when set, otherwise falls back to
// persisting file content via env_save_uploaded_content.
{
  const envFileInputHidden = document.getElementById('envFileInputHidden');
  if (envFileInputHidden) {
    envFileInputHidden.addEventListener('change', async (e) => {
      const file = e.target.files && e.target.files[0];
      e.target.value = '';
      if (!file) return;
      await _handleEnvFilePicked(file);
    });
  }
}



// --- Run Group ---
async function runGroup(fileIdx, groupName) {
  // If already running, abort.
  if (isRunning) {
    try {
      await invoke('request_run_abort');
    } catch (err) {
      rpLog('warn', 'request_run_abort failed', { error: String(err) });
    }
    showToast('Stopping… (current block will finish, then cleanup runs)', 'info');
    return;
  }

  const file = loadedFiles[fileIdx];
  if (!file) return;

  // Build a suite with setups + group blocks + teardowns (respecting disabled)
  const groupBlocks = [];
  const groupBlockIndices = [];
  file.suite.blocks.forEach((block, blockIdx) => {
    if (disabledBlocks[`${fileIdx}-${blockIdx}`]) return;
    if (block.block_type === 'setup' || block.block_type === 'teardown' || block.group === groupName) {
      groupBlocks.push(block);
      groupBlockIndices.push(blockIdx);
    }
  });

  if (groupBlocks.length === 0) {
    showToast('All steps in this group are disabled', 'info');
    return;
  }

  const suite = { variables: file.suite.variables, blocks: groupBlocks };

  try {
    const unresolved = await invoke('check_variables', { suite, envVars: collectVariablesArray() });
    if (unresolved && unresolved.length > 0) {
      promptForVariables(unresolved);
      return;
    }
  } catch (e) { console.warn('check_variables failed:', e); }

  setRunning(true);

  // Clear only group block results
  if (file.results) {
    groupBlockIndices.forEach(idx => {
      if (file.results.block_results) file.results.block_results[idx] = null;
    });
  }
  updateBlockStatuses();

  // Register only the group's blocks for live result streaming so we don't
  // accidentally overwrite prior runs of OTHER groups in this file.
  liveResultPendingByFile.set(fileIdx, [...groupBlockIndices]);

  // Show group blocks as pending in results panel
  const pendingBlocks = groupBlockIndices.map(origIdx => {
    const block = file.suite.blocks[origIdx];
    return {
      fileName: file.name,
      name: block.name || block.block_type,
      blockType: block.block_type,
      fileIdx,
      blockIdx: origIdx
    };
  });
  showTestResults(0, 0, 0, 0, [], pendingBlocks);
  testResultsDetails.classList.remove('hidden');

  await startBlockProgressListener();
  await startBlockResultListener();

  try {
    let extraVars = collectVariablesArray();
    if (azureAuthState === 'authenticated') {
      const tokenVars = fetchDevModeToken(file.suite, file.content);
      if (tokenVars.length > 0) extraVars = [...extraVars, ...tokenVars];
    }
    const promptVars = await collectPromptVariables(file.suite, file.name);
    if (promptVars === null) return;
    if (promptVars.length > 0) extraVars = [...extraVars, ...promptVars];
    const __sessions_started3 = Date.now();
    const results = await invoke('run_test_suite', { suite, extraVariables: extraVars, extraHeaders: getExtraHeaders(), runMode: azureAuthState === 'authenticated' ? 'dev' : null, fileName: file.name });
    recordSessionRun(file, suite, file.content, results, extraVars, azureAuthState === 'authenticated' ? 'dev' : null, __sessions_started3).catch(()=>{});

    // Merge results into existing file results
    if (!file.results) {
      file.results = { block_results: new Array(file.suite.blocks.length).fill(null), passed: 0, failed: 0, skipped: 0, total_time_ms: 0 };
    }
    // Duplicate-safe: splice matched indices to avoid double-mapping
    const unmatchedGroupIndices = [...groupBlockIndices];
    results.block_results.forEach(br => {
      const matchPos = unmatchedGroupIndices.findIndex(idx => file.suite.blocks[idx].name === br.name);
      if (matchPos >= 0) {
        const origIdx = unmatchedGroupIndices[matchPos];
        file.results.block_results[origIdx] = br;
        unmatchedGroupIndices.splice(matchPos, 1);
        invalidateDiffCacheEntry(fileIdx, origIdx);
      }
    });

    if (results.final_variables) {
      Object.entries(results.final_variables).forEach(([name, value]) => { runtimeOverrides[name] = value; });
    }

    updateBlockStatuses();
    renderEnvPanel();

    const blockResultsForDisplay = [];
    groupBlockIndices.forEach(origIdx => {
      const br = file.results.block_results[origIdx];
      if (br) blockResultsForDisplay.push({ ...br, fileName: file.name, fileIdx, blockIdx: origIdx });
    });
    showTestResults(results.passed, results.failed, results.skipped, results.total_time_ms, blockResultsForDisplay);
  } catch (err) {
    showToast(`Group run failed: ${err}`, 'error');
    rpLog('error', 'Command failed: run_test_suite (group)', String(err));
  } finally {
    stopBlockProgressListener();
    setRunning(false);
  }
}

// --- Run Single File ---
async function runSingleFile(fileIdx) {
  // If already running, abort the in-flight run.
  if (isRunning) {
    try {
      await invoke('request_run_abort');
    } catch (err) {
      rpLog('warn', 'request_run_abort failed', { error: String(err) });
    }
    showToast('Stopping… (current block will finish, then cleanup runs)', 'info');
    return;
  }

  const file = loadedFiles[fileIdx];
  if (!file) return;

  const suite = getEnabledSuite(fileIdx);
  if (!suite || suite.blocks.length === 0) {
    showToast('All steps are disabled', 'info');
    return;
  }

  // Check for unresolved variables (only enabled blocks)
  try {
    const unresolved = await invoke('check_variables', {
      suite,
      envVars: collectVariablesArray()
    });
    if (unresolved && unresolved.length > 0) {
      promptForVariables(unresolved);
      return;
    }
  } catch (e) { console.warn('check_variables failed:', e); }

  setRunning(true);

  file.results = null;
  invalidateDiffCacheForFile(fileIdx);
  updateBlockStatuses();

  // Show all enabled blocks as pending
  const enabledMap = getEnabledIndexMap(fileIdx);
  const pendingBlocks = enabledMap.map(origIdx => {
    const block = file.suite.blocks[origIdx];
    return {
      fileName: file.name,
      name: block.name || block.block_type,
      blockType: block.block_type,
      fileIdx,
      blockIdx: origIdx
    };
  });
  showTestResults(0, 0, 0, 0, [], pendingBlocks);
  testResultsDetails.classList.remove('hidden');

  // Initialize live-streaming results so per-block updates land in
  // file.results.block_results as each block finishes.
  prepareLiveResults(fileIdx);

  // Start listening for per-block progress
  await startBlockProgressListener();
  await startBlockResultListener();

  try {
    let extraVars = collectVariablesArray();
    if (azureAuthState === 'authenticated') {
      const tokenVars = fetchDevModeToken(file.suite, file.content);
      if (tokenVars.length > 0) extraVars = [...extraVars, ...tokenVars];
    }
    const promptVars = await collectPromptVariables(file.suite, file.name);
    if (promptVars === null) return;
    if (promptVars.length > 0) extraVars = [...extraVars, ...promptVars];
    const __sessions_started4 = Date.now();
    const results = await invoke('run_test_suite', {
      suite,
      extraVariables: extraVars,
      extraHeaders: getExtraHeaders(),
      runMode: azureAuthState === 'authenticated' ? 'dev' : null,
      fileName: file.name,
    });
    recordSessionRun(file, suite, file.content, results, extraVars, azureAuthState === 'authenticated' ? 'dev' : null, __sessions_started4).catch(()=>{});

    // Remap results to align with original block indices
    file.results = remapResults(fileIdx, results);
    pushRunHistory(file.name, results);

    // Carry extracted variables forward
    if (results.final_variables) {
      Object.entries(results.final_variables).forEach(([name, value]) => {
        runtimeOverrides[name] = value;
      });
    }

    updateBlockStatuses();
    renderEnvPanel();

    // Final reconciliation — rebuild results bar with full data
    const blockResultsForDisplay = [];
    file.results.block_results.forEach((br, origIdx) => {
      if (br) blockResultsForDisplay.push({ ...br, fileName: file.name, fileIdx, blockIdx: origIdx });
    });
    showTestResults(results.passed, results.failed, results.skipped, results.total_time_ms, blockResultsForDisplay);

    // Select first enabled block with results
    const firstResultIdx = file.results.block_results.findIndex(br => br != null);
    if (firstResultIdx >= 0) {
      selectBlock(fileIdx, firstResultIdx);
    }
  } catch (err) {
    showToast(`Error running ${file.name}: ${err}`, 'error');
    rpLog('error', 'Command failed: run_test_suite (file)', { file: file.name, error: String(err) });
  } finally {
    stopBlockProgressListener();
    setRunning(false);
    refreshHistoryIfVisible();
  }
}

// --- Variable Validation ---
async function checkUnresolvedVars(fileIdx) {
  const suite = getEnabledSuite(fileIdx);
  if (!suite) return [];

  try {
    const unresolved = await invoke('check_variables', {
      suite,
      envVars: collectVariablesArray()
    });
    return unresolved;
  } catch {
    return [];
  }
}

function promptForVariables(unresolved) {
  const names = unresolved.join(', ');
  showToast(`Missing variables: ${names}. Set them in the Environment panel or load a .env file.`, 'error');

  // Highlight the env section and scroll to it
  $('#envSection').classList.add('expanded');
  renderEnvPanel();
}

// --- Toast ---
function showToast(message, type = 'info') {
  const container = $('#toastContainer');
  const toast = document.createElement('div');
  toast.className = `toast ${type}`;
  toast.textContent = message;
  container.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = '0';
    toast.style.transform = 'translateY(10px)';
    toast.style.transition = 'all .25s ease';
    setTimeout(() => toast.remove(), 250);
  }, 3000);
}

// --- Utilities ---

function escapeAttr(str) {
  return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// --- Variable Autocomplete (Ctrl+Space / {{ trigger) ---

const varAutocomplete = $('#varAutocomplete');
const varAutocompleteList = $('#varAutocompleteList');

const acState = {
  open: false,
  cursor: 0,
  items: [],        // [{name, value}]
  filtered: [],     // [{name, value}]
  prefix: '',       // text after {{ for filtering
  targetEl: null,   // the input/textarea being completed
  replaceStart: -1, // char index where {{ starts (for replacement)
  isCodeEditor: false,
};

function collectAllVarNames() {
  const vars = new Map();
  // Suite variables from all loaded files
  loadedFiles.forEach(file => {
    if (file.suite.variables) {
      file.suite.variables.forEach(([name, value]) => vars.set(name, value));
    }
  });
  // Active .env vars
  (activeEnvVars || []).forEach(([name, value]) => {
    if (value !== '') vars.set(name, value);
  });
  // Runtime overrides (from @@extract)
  Object.entries(runtimeOverrides).forEach(([name, value]) => {
    if (value !== '') vars.set(name, value);
  });
  // Built-ins
  vars.set('$timestamp', '(Unix timestamp)');
  vars.set('$uuid', '(Random UUID v4)');
  vars.set('$randomInt', '(Random 0–9999)');
  return Array.from(vars.entries()).map(([name, value]) => ({ name, value }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function openVarAutocomplete(targetEl, prefix, replaceStart, isCodeEditor) {
  acState.targetEl = targetEl;
  acState.prefix = prefix;
  acState.replaceStart = replaceStart;
  acState.isCodeEditor = isCodeEditor;
  acState.items = collectAllVarNames();
  acState.cursor = 0;
  acState.open = true;
  filterAndRenderAC();
  if (!acState.open) return; // filterAndRenderAC closed it (no matches)
  positionAutocomplete(targetEl, isCodeEditor);
  varAutocomplete.classList.remove('hidden');
}

function positionAutocomplete(el, isCodeEditor) {
  // Use a hidden caret-measuring span to approximate cursor position
  const rect = el.getBoundingClientRect();
  let left, top;

  if (el.tagName === 'TEXTAREA' || isCodeEditor) {
    // For textareas, position below the element near cursor
    const style = window.getComputedStyle(el);
    const lineHeight = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.4;
    const paddingTop = parseFloat(style.paddingTop) || 0;
    const paddingLeft = parseFloat(style.paddingLeft) || 0;

    // Approximate cursor line from selectionStart
    const text = el.value.substring(0, el.selectionStart);
    const lines = text.split('\n');
    const cursorLine = lines.length - 1;
    const cursorCol = lines[cursorLine].length;

    // Estimate position
    const charWidth = measureCharWidth(style.fontFamily, style.fontSize);
    top = rect.top + paddingTop + (cursorLine + 1) * lineHeight - el.scrollTop;
    left = rect.left + paddingLeft + cursorCol * charWidth - el.scrollLeft;
  } else {
    // For regular inputs, position below the input
    top = rect.bottom + 2;
    left = rect.left;
  }

  // Clamp to viewport
  const maxLeft = window.innerWidth - 400;
  const maxTop = window.innerHeight - 260;
  left = Math.max(4, Math.min(left, maxLeft));
  top = Math.min(top, maxTop);

  varAutocomplete.style.left = `${left}px`;
  varAutocomplete.style.top = `${top}px`;
}

let _cachedCharWidth = null;
function measureCharWidth(fontFamily, fontSize) {
  if (_cachedCharWidth) return _cachedCharWidth;
  const span = document.createElement('span');
  span.style.cssText = `position:absolute;visibility:hidden;white-space:pre;font-family:${fontFamily};font-size:${fontSize}`;
  span.textContent = 'x'.repeat(100);
  document.body.appendChild(span);
  _cachedCharWidth = span.offsetWidth / 100;
  document.body.removeChild(span);
  return _cachedCharWidth;
}

function filterAndRenderAC() {
  const q = acState.prefix.toLowerCase();
  acState.filtered = q
    ? acState.items.filter(v => v.name.toLowerCase().includes(q))
    : acState.items;

  if (acState.filtered.length === 0) {
    closeVarAutocomplete();
    return;
  }
  acState.cursor = Math.min(acState.cursor, acState.filtered.length - 1);

  varAutocompleteList.innerHTML = acState.filtered.map((v, i) => {
    const cls = i === acState.cursor ? 'var-autocomplete-item active' : 'var-autocomplete-item';
    const valDisplay = v.value.length > 40 ? v.value.slice(0, 37) + '…' : v.value;
    return `<div class="${cls}" data-idx="${i}">
      <span class="var-name">${escapeHtml(v.name)}</span>
      <span class="var-value">${escapeHtml(valDisplay)}</span>
    </div>`;
  }).join('');

  // Click handlers
  varAutocompleteList.querySelectorAll('.var-autocomplete-item').forEach(item => {
    item.addEventListener('mousedown', (e) => {
      e.preventDefault();
      acState.cursor = parseInt(item.dataset.idx);
      insertSelectedVar();
    });
  });

  // Scroll active item into view
  const activeItem = varAutocompleteList.querySelector('.active');
  if (activeItem) activeItem.scrollIntoView({ block: 'nearest' });
}

function insertSelectedVar() {
  const selected = acState.filtered[acState.cursor];
  if (!selected || !acState.targetEl) { closeVarAutocomplete(); return; }

  const el = acState.targetEl;
  const insertion = `{{${selected.name}}}`;
  const start = acState.replaceStart;
  const cursorPos = el.selectionStart;

  // Replace from {{ start to current cursor with the full variable reference
  // Also consume trailing `}}` if cursor is inside an existing placeholder
  const before = el.value.substring(0, start);
  let afterCursor = el.value.substring(cursorPos);
  if (afterCursor.startsWith('}}')) {
    afterCursor = afterCursor.substring(2);
  } else if (afterCursor.match(/^[^{}]*\}\}/)) {
    // Cursor inside {{prefix|suffix}} — consume through the closing }}
    const closeIdx = afterCursor.indexOf('}}');
    if (closeIdx >= 0) afterCursor = afterCursor.substring(closeIdx + 2);
  }
  el.value = before + insertion + afterCursor;

  // Set cursor after the inserted variable
  const newPos = start + insertion.length;
  el.selectionStart = el.selectionEnd = newPos;

  // Fire input event so listeners pick up the change
  el.dispatchEvent(new Event('input', { bubbles: true }));

  closeVarAutocomplete();
  el.focus();
}

function closeVarAutocomplete() {
  acState.open = false;
  acState.targetEl = null;
  varAutocomplete.classList.add('hidden');
}

function handleACKeydown(e) {
  if (!acState.open) return false;

  switch (e.key) {
    case 'ArrowDown':
      e.preventDefault();
      acState.cursor = Math.min(acState.cursor + 1, acState.filtered.length - 1);
      filterAndRenderAC();
      return true;
    case 'ArrowUp':
      e.preventDefault();
      acState.cursor = Math.max(acState.cursor - 1, 0);
      filterAndRenderAC();
      return true;
    case 'Enter':
    case 'Tab':
      e.preventDefault();
      insertSelectedVar();
      return true;
    case 'Escape':
      e.preventDefault();
      closeVarAutocomplete();
      return true;
    default:
      return false;
  }
}

/** Detect {{ trigger or Ctrl+Space in an input/textarea. */
function wireVarAutocomplete(el, isCodeEditor = false) {
  el.addEventListener('keydown', (e) => {
    // If autocomplete is open, let it handle keys first
    if (handleACKeydown(e)) return;

    // Ctrl+Space trigger
    if (e.key === ' ' && e.ctrlKey) {
      e.preventDefault();
      const pos = el.selectionStart;
      const textBefore = el.value.substring(0, pos);
      // Check if we're already inside {{ }}
      const lastOpen = textBefore.lastIndexOf('{{');
      const lastClose = textBefore.lastIndexOf('}}');
      let prefix = '';
      let replaceStart = pos;
      if (lastOpen > lastClose && lastOpen >= 0) {
        prefix = textBefore.substring(lastOpen + 2);
        replaceStart = lastOpen;
      }
      openVarAutocomplete(el, prefix, replaceStart, isCodeEditor);
      return;
    }
  });

  el.addEventListener('input', () => {
    const pos = el.selectionStart;
    const textBefore = el.value.substring(0, pos);
    const lastOpen = textBefore.lastIndexOf('{{');
    const lastClose = textBefore.lastIndexOf('}}');

    if (lastOpen > lastClose && lastOpen >= 0) {
      const prefix = textBefore.substring(lastOpen + 2);
      // Only auto-open if prefix is reasonable (no newlines, not too long)
      if (prefix.length <= 40 && !prefix.includes('\n')) {
        if (!acState.open) {
          openVarAutocomplete(el, prefix, lastOpen, isCodeEditor);
        } else {
          acState.prefix = prefix;
          acState.cursor = 0;
          filterAndRenderAC();
        }
        return;
      }
    }

    // Close if no longer inside {{
    if (acState.open && acState.targetEl === el) {
      closeVarAutocomplete();
    }
  });

  el.addEventListener('blur', () => {
    // Small delay to allow click on dropdown item
    setTimeout(() => {
      if (acState.open && acState.targetEl === el) closeVarAutocomplete();
    }, 200);
  });
}

// Wire autocomplete to main inputs
wireVarAutocomplete(urlInput);
wireVarAutocomplete(bodyInput);
wireVarAutocomplete(codeEditor, true);

// Wire to header value inputs dynamically (they're created/destroyed)
// We use event delegation on headersContainer
headersContainer.addEventListener('keydown', (e) => {
  if (e.target.classList.contains('kv-value') || e.target.classList.contains('kv-key')) {
    if (handleACKeydown(e)) return;
    if (e.key === ' ' && e.ctrlKey) {
      e.preventDefault();
      const el = e.target;
      const pos = el.selectionStart;
      const textBefore = el.value.substring(0, pos);
      const lastOpen = textBefore.lastIndexOf('{{');
      const lastClose = textBefore.lastIndexOf('}}');
      let prefix = '';
      let replaceStart = pos;
      if (lastOpen > lastClose && lastOpen >= 0) {
        prefix = textBefore.substring(lastOpen + 2);
        replaceStart = lastOpen;
      }
      openVarAutocomplete(el, prefix, replaceStart, false);
    }
  }
});
headersContainer.addEventListener('input', (e) => {
  if (e.target.classList.contains('kv-value') || e.target.classList.contains('kv-key')) {
    scheduleLiveBuilderFlush();
    const el = e.target;
    const pos = el.selectionStart;
    const textBefore = el.value.substring(0, pos);
    const lastOpen = textBefore.lastIndexOf('{{');
    const lastClose = textBefore.lastIndexOf('}}');
    if (lastOpen > lastClose && lastOpen >= 0) {
      const prefix = textBefore.substring(lastOpen + 2);
      if (prefix.length <= 40) {
        if (!acState.open) {
          openVarAutocomplete(el, prefix, lastOpen, false);
        } else if (acState.targetEl === el) {
          acState.prefix = prefix;
          acState.cursor = 0;
          filterAndRenderAC();
        }
        return;
      }
    }
    if (acState.open && acState.targetEl === el) closeVarAutocomplete();
  }
});

// Close autocomplete on any click outside
document.addEventListener('mousedown', (e) => {
  if (acState.open && !varAutocomplete.contains(e.target) && e.target !== acState.targetEl) {
    closeVarAutocomplete();
  }
});

// --- Variable Tooltip: JWT & Token Detection ---

function isJwt(value) {
  if (!value || typeof value !== 'string') return false;
  const parts = value.split('.');
  if (parts.length !== 3) return false;
  try {
    atob(parts[0].replace(/-/g, '+').replace(/_/g, '/'));
    atob(parts[1].replace(/-/g, '+').replace(/_/g, '/'));
    return true;
  } catch {
    return false;
  }
}

function decodeJwt(token) {
  const parts = token.split('.');
  const decode = (s) => {
    try {
      const padded = s.replace(/-/g, '+').replace(/_/g, '/');
      return JSON.parse(atob(padded));
    } catch {
      return null;
    }
  };
  const header = decode(parts[0]);
  const payload = decode(parts[1]);

  let tokenType = 'JWT';
  if (header) {
    if (header.typ === 'at+jwt' || (payload && payload.aud)) tokenType = 'Access Token';
    if (header.typ === 'JWT' && payload && payload.refresh_token) tokenType = 'Refresh Token';
    if (payload && payload.nonce) tokenType = 'ID Token';
  }

  let expiryInfo = null;
  if (payload && payload.exp) {
    const expDate = new Date(payload.exp * 1000);
    const now = new Date();
    const isExpired = expDate < now;
    const timeLeft = isExpired ? 'EXPIRED' : formatTimeUntil(expDate);
    expiryInfo = { expDate, isExpired, timeLeft };
  }

  return { header, payload, tokenType, expiryInfo, signature: parts[2] };
}

function formatTimeUntil(date) {
  const diff = date - new Date();
  const mins = Math.floor(diff / 60000);
  const hours = Math.floor(mins / 60);
  if (hours > 0) return `${hours}h ${mins % 60}m`;
  return `${mins}m`;
}

function detectValueType(name, value) {
  if (!value) return { type: 'empty' };

  if (isJwt(value)) {
    return { type: 'jwt', decoded: decodeJwt(value) };
  }

  if (value.startsWith('Bearer ')) {
    const inner = value.substring(7);
    if (isJwt(inner)) {
      return { type: 'jwt', decoded: decodeJwt(inner) };
    }
    return { type: 'bearer', value: inner };
  }

  if (value.length > 40 && /^[A-Za-z0-9+/=]+$/.test(value)) {
    try {
      const decoded = atob(value);
      if (decoded.length > 0 && /^[\x20-\x7E\s]+$/.test(decoded)) {
        return { type: 'base64', decoded };
      }
    } catch { /* non-critical: base64 decode probe */ }
  }

  if (value.startsWith('http://') || value.startsWith('https://')) {
    return { type: 'url' };
  }

  if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)) {
    return { type: 'uuid' };
  }

  if (/^\d{10,13}$/.test(value)) {
    const ts = parseInt(value);
    const date = new Date(ts > 9999999999 ? ts : ts * 1000);
    if (!isNaN(date.getTime())) {
      return { type: 'timestamp', date };
    }
  }

  return { type: 'text' };
}

function syntaxHighlightJson(jsonStr) {
  return escapeHtml(jsonStr)
    .replace(/"([^"]+)"(?=\s*:)/g, '<span class="json-key">"$1"</span>')
    .replace(/:\s*"([^"]*)"(?=[,\n\r\}])/g, ': <span class="json-string">"$1"</span>')
    .replace(/:\s*(\d+\.?\d*)(?=[,\n\r\}])/g, ': <span class="json-number">$1</span>')
    .replace(/:\s*(true|false)(?=[,\n\r\}])/g, ': <span class="json-boolean">$1</span>')
    .replace(/:\s*(null)(?=[,\n\r\}])/g, ': <span class="json-null">$1</span>');
}

function buildVarTooltipHtml(name, value) {
  const detected = detectValueType(name, value);
  let html = '';

  html += `<div class="btt-header">
    <span class="btt-type-icon">\uD83D\uDD10</span>
    <span class="btt-type-label">Variable</span>
    <span class="btt-name">${escapeHtml(name)}</span>
  </div>`;

  const displayValue = value.length > 80 ? value.substring(0, 80) + '\u2026' : value;
  html += `<div class="btt-section">
    <div class="btt-section-title">Value</div>
    <pre class="btt-body var-tooltip-value">${escapeHtml(displayValue)}</pre>
  </div>`;

  switch (detected.type) {
    case 'jwt': {
      const { header, payload, tokenType, expiryInfo } = detected.decoded;

      html += `<div class="btt-section">
        <div class="btt-section-title">
          Token Info
          <span class="var-token-badge">${escapeHtml(tokenType)}</span>
          ${expiryInfo ? `<span class="var-token-expiry ${expiryInfo.isExpired ? 'expired' : 'valid'}">${expiryInfo.isExpired ? '\u26A0 EXPIRED' : '\u2713 ' + expiryInfo.timeLeft}</span>` : ''}
        </div>
      </div>`;

      if (header) {
        html += `<div class="btt-section">
          <div class="btt-section-title">Header <span class="btt-count">${Object.keys(header).length}</span></div>
          <pre class="var-tooltip-json">${syntaxHighlightJson(JSON.stringify(header, null, 2))}</pre>
        </div>`;
      }

      if (payload) {
        html += `<div class="btt-section">
          <div class="btt-section-title">Claims <span class="btt-count">${Object.keys(payload).length}</span></div>
          <pre class="var-tooltip-json">${syntaxHighlightJson(JSON.stringify(payload, null, 2))}</pre>
        </div>`;

        const summary = [];
        if (payload.iss) summary.push(`<div class="var-claim"><span class="var-claim-key">Issuer:</span> ${escapeHtml(payload.iss)}</div>`);
        if (payload.sub) summary.push(`<div class="var-claim"><span class="var-claim-key">Subject:</span> ${escapeHtml(payload.sub)}</div>`);
        if (payload.aud) summary.push(`<div class="var-claim"><span class="var-claim-key">Audience:</span> ${escapeHtml(typeof payload.aud === 'string' ? payload.aud : JSON.stringify(payload.aud))}</div>`);
        if (payload.name) summary.push(`<div class="var-claim"><span class="var-claim-key">Name:</span> ${escapeHtml(payload.name)}</div>`);
        if (payload.preferred_username || payload.email) summary.push(`<div class="var-claim"><span class="var-claim-key">User:</span> ${escapeHtml(payload.preferred_username || payload.email)}</div>`);
        if (payload.scp || payload.scope) summary.push(`<div class="var-claim"><span class="var-claim-key">Scope:</span> ${escapeHtml(payload.scp || payload.scope)}</div>`);
        if (payload.roles) summary.push(`<div class="var-claim"><span class="var-claim-key">Roles:</span> ${escapeHtml(payload.roles.join(', '))}</div>`);

        if (summary.length > 0) {
          html += `<div class="btt-section">
            <div class="btt-section-title">Key Claims</div>
            ${summary.join('')}
          </div>`;
        }
      }

      html += `<div class="btt-section">
        <div class="btt-section-title">Signature</div>
        <span class="var-tooltip-sig">${escapeHtml(detected.decoded.signature.substring(0, 32))}\u2026</span>
      </div>`;
      break;
    }

    case 'bearer':
      html += `<div class="btt-section">
        <div class="btt-section-title">Type</div>
        <span class="var-token-badge">Bearer Token (Opaque)</span>
        <pre class="btt-body">${escapeHtml(detected.value.substring(0, 100))}${detected.value.length > 100 ? '\u2026' : ''}</pre>
      </div>`;
      break;

    case 'base64':
      html += `<div class="btt-section">
        <div class="btt-section-title">Decoded (Base64)</div>
        <pre class="btt-body">${escapeHtml(detected.decoded.substring(0, 500))}</pre>
      </div>`;
      break;

    case 'url':
      try {
        const url = new URL(value);
        html += `<div class="btt-section">
          <div class="btt-section-title">URL Parts</div>
          <div class="var-claim"><span class="var-claim-key">Host:</span> ${escapeHtml(url.hostname)}</div>
          <div class="var-claim"><span class="var-claim-key">Path:</span> ${escapeHtml(url.pathname)}</div>
          ${url.search ? `<div class="var-claim"><span class="var-claim-key">Query:</span> ${escapeHtml(url.search)}</div>` : ''}
        </div>`;
      } catch { /* non-critical: URL parse */ }
      break;

    case 'uuid':
      html += `<div class="btt-section">
        <div class="btt-section-title">Type</div>
        <span class="var-token-badge">UUID</span>
      </div>`;
      break;

    case 'timestamp':
      html += `<div class="btt-section">
        <div class="btt-section-title">Timestamp</div>
        <div class="var-claim"><span class="var-claim-key">Date:</span> ${detected.date.toISOString()}</div>
        <div class="var-claim"><span class="var-claim-key">Local:</span> ${detected.date.toLocaleString()}</div>
      </div>`;
      break;
  }

  return html;
}

function formatBytes(bytes) {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return `${(bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
}

function truncateUrl(url) {
  try {
    const u = new URL(url);
    const path = u.pathname + u.search;
    return path.length > 35 ? path.slice(0, 35) + '\u2026' : path || url;
  } catch {
    return url.length > 40 ? url.slice(0, 40) + '\u2026' : url;
  }
}

// --- Mode Toggle (Builder / Code) ---
modeToggle.addEventListener('click', (e) => {
  const btn = e.target.closest('.mode-btn');
  if (!btn || btn.dataset.mode === currentMode) return;
  switchMode(btn.dataset.mode);
});

async function switchMode(mode) {
  if (mode === currentMode) return;

  // When leaving code mode, sync code back to builder
  if (currentMode === 'code') {
    // Cancel any pending live code parse — we're about to do a full sync.
    clearTimeout(liveCodeParseTimer);
    const ok = await syncCodeToBuilder();
    if (!ok) return;
  }

  // When leaving history mode, close detail panel
  if (currentMode === 'history') {
    closeHistoryDetail();
  }

  // When entering code mode, flush any pending builder edits first so the
  // generated .http content reflects the latest state.
  if (mode === 'code') {
    if (liveBuilderFlushTimer) {
      clearTimeout(liveBuilderFlushTimer);
      liveBuilderFlushTimer = null;
      await flushBuilderLive();
    }
    await syncBuilderToCode();
  }

  currentMode = mode;

  modeToggle.querySelectorAll('.mode-btn').forEach(b => {
    b.classList.toggle('active', b.dataset.mode === mode);
  });

  // Hide all mode-specific panels
  urlBar.classList.add('hidden');
  splitPanels.classList.add('hidden');
  codeEditorPanel.classList.add('hidden');
  historyPanel.classList.add('hidden');
  logsPanel.classList.add('hidden');
  if (sessionsPanel) sessionsPanel.classList.add('hidden');

  // Show the appropriate panels
  if (mode === 'builder') {
    urlBar.classList.remove('hidden');
    splitPanels.classList.remove('hidden');
    // Re-render assertions for active block if it has results
    if (activeFileIndex >= 0 && activeBlockIndex >= 0) {
      const file = loadedFiles[activeFileIndex];
      const block = file?.suite?.blocks?.[activeBlockIndex];
      const br = file?.results?.block_results?.[activeBlockIndex];
      if (block && br) {
        renderAssertions(activeFileIndex, activeBlockIndex);
      }
    }
  } else if (mode === 'code') {
    codeEditorPanel.classList.remove('hidden');
  } else if (mode === 'history') {
    historyPanel.classList.remove('hidden');
    loadHistory();
  } else if (mode === 'logs') {
    logsPanel.classList.remove('hidden');
    renderAllLogs();
  } else if (mode === 'sessions') {
    if (sessionsPanel) sessionsPanel.classList.remove('hidden');
    if (typeof loadSessionsTree === 'function') loadSessionsTree();
  }
}

async function syncBuilderToCode() {
  if (activeFileIndex >= 0 && loadedFiles[activeFileIndex]) {
    const file = loadedFiles[activeFileIndex];
    try {
      // Use raw file content as base, then apply disabled markers
      if (file.content) {
        const content = applyDisabledFlags(file.content, activeFileIndex);
        codeEditor.value = content;
        codeEditorContent = content;
      } else {
        // No raw content — generate from suite
        const content = await invoke('generate_http', { suite: buildSuiteWithDisabledFlags(activeFileIndex) });
        codeEditor.value = content;
        codeEditorContent = content;
      }
      codeEditorFilename.textContent = file.name;
    } catch (err) {
      showToast(`Failed to generate: ${err}`, 'error');
      rpLog('error', 'Command failed: generate_http', String(err));
    }
  } else {
    codeEditor.value = '';
    codeEditorContent = '';
    codeEditorFilename.textContent = 'untitled.http';
  }
  codeEditorModified = false;
  codeEditor.classList.remove('modified');
  updateHighlight();
  // Scroll to and highlight the active block
  scrollCodeEditorToActiveBlock();
}

// Apply # @disabled markers to raw file content based on disabledBlocks state
function applyDisabledFlags(content, fileIdx) {
  const lines = content.split('\n');
  const result = [];
  let blockIdx = -1;
  let needsDisabledMarker = false;
  let hasDisabledMarker = false;
  let inVariablesBlock = false;

  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();

    if (trimmed.startsWith('@variables')) {
      inVariablesBlock = true;
    }

    // Detect block separator
    if (trimmed.startsWith('###')) {
      inVariablesBlock = false;
      blockIdx++;
      needsDisabledMarker = !!disabledBlocks[`${fileIdx}-${blockIdx}`];
      hasDisabledMarker = false;
      result.push(lines[i]);
      continue;
    }

    // Check if this line is an existing disabled marker (new or legacy)
    if (trimmed === '# @@disabled' || trimmed === '# @disabled' || trimmed === '// @@disabled' || trimmed === '// @disabled') {
      hasDisabledMarker = true;
      if (needsDisabledMarker) {
        result.push(lines[i]); // keep it
      }
      // else skip it (block was re-enabled)
      continue;
    }

    // If we're at a non-comment, non-empty line after ### and need a disabled marker, inject it
    if (needsDisabledMarker && !hasDisabledMarker && blockIdx >= 0 &&
        trimmed !== '' && !trimmed.startsWith('#') && !trimmed.startsWith('//') && !trimmed.startsWith('@') && !inVariablesBlock) {
      // Insert # @@disabled before the request line (new syntax)
      result.push('# @@disabled');
      hasDisabledMarker = true;
    }

    result.push(lines[i]);
  }
  return result.join('\n');
}

// Build suite with disabled flags set on blocks for code generation
function buildSuiteWithDisabledFlags(fileIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return { variables: [], blocks: [] };
  const blocks = file.suite.blocks.map((block, blockIdx) => ({
    ...block,
    disabled: !!disabledBlocks[`${fileIdx}-${blockIdx}`]
  }));
  return { variables: file.suite.variables, blocks };
}

// Preserve prior block_results across a re-parse when the block list is
// structurally compatible (same count + same names in same order). If the
// shape changed, indices no longer line up so we must clear to avoid
// showing stale/misaligned status dots.
function preserveResultsIfCompatible(prevResults, prevSuite, newSuite) {
  if (!prevResults || !prevSuite || !newSuite) return null;
  const prevBlocks = prevSuite.blocks || [];
  const newBlocks = newSuite.blocks || [];
  if (prevBlocks.length !== newBlocks.length) return null;
  for (let i = 0; i < prevBlocks.length; i++) {
    if ((prevBlocks[i].name || '') !== (newBlocks[i].name || '')) return null;
  }
  return prevResults;
}

async function syncCodeToBuilder() {
  const content = codeEditor.value;
  if (!content.trim()) {
    // Empty content — update in-memory state but don't parse
    if (activeFileIndex >= 0 && loadedFiles[activeFileIndex]) {
      loadedFiles[activeFileIndex].content = content;
      loadedFiles[activeFileIndex].suite = { variables: {}, blocks: [], auto_run: null };
      loadedFiles[activeFileIndex].results = null;
    }
    codeEditorContent = content;
    codeEditorModified = false;
    codeEditor.classList.remove('modified');
    renderFileTree();
    return true;
  }

  try {
    const suite = await invoke('parse_test_file', { content });

    if (activeFileIndex >= 0 && loadedFiles[activeFileIndex]) {
      const prev = loadedFiles[activeFileIndex];
      const preserved = preserveResultsIfCompatible(prev.results, prev.suite, suite);
      prev.suite = suite;
      prev.content = content;
      prev.results = preserved;
    } else {
      const name = codeEditorFilename.textContent || 'untitled.http';
      loadedFiles.push({ name, content, suite, results: null, savedPath: null });
      activeFileIndex = loadedFiles.length - 1;
    }

    // Sync disabled state from parsed # @disabled directives
    syncDisabledFromSuite(activeFileIndex);

    renderFileTree();
    renderEnvVars();

    if (suite.blocks.length > 0) {
      selectBlock(activeFileIndex, 0);
    }

    codeEditorContent = content;
    codeEditorModified = false;
    codeEditor.classList.remove('modified');
    detectAzureAuthNeeded();
    return true;
  } catch (err) {
    showToast(`Parse error: ${err}`, 'error');
    rpLog('error', 'Command failed: parse_test_file (code editor)', String(err));
    return false;
  }
}

// Sync disabledBlocks state from suite's block.disabled flags
function syncDisabledFromSuite(fileIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return;
  file.suite.blocks.forEach((block, blockIdx) => {
    const key = `${fileIdx}-${blockIdx}`;
    if (block.disabled) {
      disabledBlocks[key] = true;
    } else {
      delete disabledBlocks[key];
    }
  });
}

// --- Syntax Highlighting ---
function escapeHtml(str) {
  return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function highlightVariables(html) {
  return html.replace(/(\{\{)(.*?)(\}\})/g, (_, open, name, close) => {
    return `<span class="hl-variable">${open}${name}${close}</span>`;
  });
}

function highlightJsonLine(line) {
  const escaped = escapeHtml(line);
  let result = escaped
    .replace(/("(?:[^"\\]|\\.)*")\s*:/g, '<span class="hl-string">$1</span>:')
    .replace(/:\s*("(?:[^"\\]|\\.)*")/g, ': <span class="hl-string">$1</span>')
    .replace(/:\s*(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b/g, ': <span class="hl-number">$1</span>')
    .replace(/:\s*\b(true|false|null)\b/g, ': <span class="hl-keyword">$1</span>')
    .replace(/([{}\[\]])/g, '<span class="hl-brace">$1</span>');
  // Standalone values in arrays
  result = result
    .replace(/(,\s*)("(?:[^"\\]|\\.)*")/g, '$1<span class="hl-string">$2</span>')
    .replace(/(,\s*)(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b/g, '$1<span class="hl-number">$2</span>')
    .replace(/(,\s*)\b(true|false|null)\b/g, '$1<span class="hl-keyword">$2</span>');
  return highlightVariables(result);
}

function highlightHttpCode(text) {
  const lines = text.split('\n');
  let inJsonBody = false;
  let inVariablesBlock = false;

  return lines.map(line => {
    // Separator lines: ### or --- (line-anchored, followed by space or EOL)
    if (/^###(\s|$)/.test(line) || /^---(\s|$)/.test(line)) {
      inJsonBody = false;
      inVariablesBlock = false;
      const escaped = escapeHtml(line);
      // Check for block types — accept both new (@@setup) and legacy (@setup)
      const btMatch = escaped.match(/(@@?(?:setup|test|teardown|variables))\b/);
      if (btMatch) {
        const highlighted = escaped.replace(btMatch[1], `<span class="hl-block-type">${btMatch[1]}</span>`);
        return `<span class="hl-separator">${highlightVariables(highlighted)}</span>`;
      }
      return `<span class="hl-separator">${highlightVariables(escaped)}</span>`;
    }

    // @variables at start of line (legacy block header)
    if (/^@variables\b/.test(line)) {
      inJsonBody = false;
      inVariablesBlock = true;
      const escaped = escapeHtml(line);
      return escaped.replace(/^(@variables)/, '<span class="hl-block-type">$1</span>');
    }

    // Variable assignment lines inside legacy @variables block: "name = value"
    if (inVariablesBlock && /^\w+\s*=/.test(line)) {
      const escaped = escapeHtml(line);
      return highlightVariables(escaped.replace(/^(\w+)(\s*=\s*)(.*)$/, '<span class="hl-header-name">$1</span><span class="hl-operator">$2</span><span class="hl-header-value">$3</span>'));
    }

    // Pilot directive comment lines: # @x, # @@x, // @x, // @@x
    //   for any known Pilot directive. Accept both legacy and new @@ syntax.
    const directiveRe = /^(?:#|\/\/)\s*@@?(assert|extract|name|description|note|group|depends|mode|dev_auth|disabled|type|compare|step|diff|auto_run|telemetry|telemetry_token|telemetry_service|prompt)\b/;
    if (directiveRe.test(line)) {
      inJsonBody = false;
      const escaped = escapeHtml(line);
      // @@disabled gets a distinct style
      if (/^(?:#|\/\/)\s*@@?disabled\s*$/.test(line)) {
        return `<span class="hl-disabled">${escaped}</span>`;
      }
      const withOps = escaped.replace(/(==|!=|&gt;=|&lt;=|&gt;|&lt;|contains|matches|exists|isType)/g, '<span class="hl-operator">$1</span>');
      return highlightVariables(`<span class="hl-directive">${withOps}</span>`);
    }

    // Comment lines: # or // (non-directive)
    if (/^#/.test(line) || /^\/\//.test(line)) {
      inJsonBody = false;
      return `<span class="hl-comment">${highlightVariables(escapeHtml(line))}</span>`;
    }

    // Top-level variable assignment: @name = value  (REST Client native)
    if (/^@\w+\s*=/.test(line)) {
      inJsonBody = false;
      const escaped = escapeHtml(line);
      return highlightVariables(escaped.replace(/^(@\w+)(\s*=\s*)(.*)$/, '<span class="hl-header-name">$1</span><span class="hl-operator">$2</span><span class="hl-header-value">$3</span>'));
    }

    // Bare REST Client directive: @name foo, @description ..., @note ..., @prompt VAR description (no '=')
    if (/^@(name|description|note|prompt)\b/.test(line)) {
      inJsonBody = false;
      return `<span class="hl-directive">${highlightVariables(escapeHtml(line))}</span>`;
    }

    // HTTP method lines: METHOD URL
    const methodMatch = line.match(/^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+(.*)$/);
    if (methodMatch) {
      inJsonBody = false;
      inVariablesBlock = false;
      return `<span class="hl-method">${escapeHtml(methodMatch[1])}</span> ${highlightVariables(escapeHtml(methodMatch[2]).replace(/^(\S+)/, '<span class="hl-url">$1</span>'))}`;
    }

    // Header lines: Name: value (only when not in JSON body)
    if (!inJsonBody && /^[A-Za-z][\w-]*\s*:/.test(line)) {
      const colonIdx = line.indexOf(':');
      const name = line.substring(0, colonIdx);
      const value = line.substring(colonIdx + 1);
      return `<span class="hl-header-name">${escapeHtml(name)}</span>:${highlightVariables(`<span class="hl-header-value">${escapeHtml(value)}</span>`)}`;
    }

    // Detect start of JSON body
    if (/^\s*[\[{]/.test(line)) {
      inJsonBody = true;
    }

    // JSON body or blank lines
    if (inJsonBody) {
      return highlightJsonLine(line);
    }

    // Blank or unrecognized lines
    if (line.trim() === '') {
      inJsonBody = false;
      if (inVariablesBlock) inVariablesBlock = false;
    }
    return highlightVariables(escapeHtml(line));
  }).join('\n');
}

function updateLineNumbers(text) {
  const count = text.split('\n').length;
  const nums = [];
  for (let i = 1; i <= count; i++) nums.push(i);
  codeLineNumbers.textContent = nums.join('\n');
}

function updateHighlight() {
  const text = codeEditor.value;
  codeEditorHighlightCode.innerHTML = highlightHttpCode(text) + '\n';
  applyBlockHighlight();
  updateLineNumbers(text);
  syncEditorScroll();
}

function syncEditorScroll() {
  codeEditorHighlight.scrollTop = codeEditor.scrollTop;
  codeEditorHighlight.scrollLeft = codeEditor.scrollLeft;
  codeLineNumbers.scrollTop = codeEditor.scrollTop;
  updateJumpBackIndicator();
}

// --- Active block scroll & highlight in code editor ---
let activeBlockLineStart = -1;
let activeBlockLineEnd = -1;

function getBlockLineRanges(text) {
  const lines = text.split('\n');
  const ranges = [];
  let blockIdx = -1;
  let blockStart = -1;
  let inVariables = false;

  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (trimmed.startsWith('@variables')) {
      inVariables = true;
      continue;
    }
    if (trimmed.startsWith('###')) {
      if (inVariables) inVariables = false;
      if (blockIdx >= 0) {
        ranges[blockIdx] = { start: blockStart, end: i - 1 };
      }
      blockIdx++;
      blockStart = i;
    }
  }
  if (blockIdx >= 0) {
    ranges[blockIdx] = { start: blockStart, end: lines.length - 1 };
  }
  return ranges;
}

function scrollCodeEditorToActiveBlock() {
  if (activeBlockIndex < 0) {
    activeBlockLineStart = -1;
    activeBlockLineEnd = -1;
    clearBlockHighlight();
    hideJumpBack();
    return;
  }

  const text = codeEditor.value;
  const ranges = getBlockLineRanges(text);
  const range = ranges[activeBlockIndex];
  if (!range) {
    activeBlockLineStart = -1;
    activeBlockLineEnd = -1;
    clearBlockHighlight();
    hideJumpBack();
    return;
  }

  activeBlockLineStart = range.start;
  activeBlockLineEnd = range.end;

  // Compute scroll position: line-height = 13px * 1.7 = 22.1px, padding = 16px
  const lineHeight = 13 * 1.7;
  const padding = 16;
  const targetScroll = (range.start * lineHeight) + padding - 40; // 40px breathing room above
  codeEditor.scrollTop = Math.max(0, targetScroll);
  syncEditorScroll();

  applyBlockHighlight();
}

function applyBlockHighlight() {
  if (activeBlockLineStart < 0) return;
  const codeEl = codeEditorHighlightCode;
  const lines = codeEl.innerHTML.split('\n');
  for (let i = activeBlockLineStart; i <= activeBlockLineEnd && i < lines.length; i++) {
    // Wrap the line in a highlight span if it's the separator line
    if (i === activeBlockLineStart) {
      lines[i] = `<span class="hl-active-block-start">${lines[i]}</span>`;
    }
  }
  codeEl.innerHTML = lines.join('\n');
}

function clearBlockHighlight() {
  const codeEl = codeEditorHighlightCode;
  codeEl.innerHTML = codeEl.innerHTML.replace(/<span class="hl-active-block-start">([\s\S]*?)<\/span>/g, '$1');
}

function updateJumpBackIndicator() {
  const indicator = document.getElementById('codeJumpBack');
  if (!indicator || activeBlockLineStart < 0 || currentMode !== 'code') {
    if (indicator) indicator.classList.add('hidden');
    return;
  }

  const lineHeight = 13 * 1.7;
  const padding = 16;
  const blockTopPx = (activeBlockLineStart * lineHeight) + padding;
  const blockBottomPx = (activeBlockLineEnd * lineHeight) + padding + lineHeight;
  const viewTop = codeEditor.scrollTop;
  const viewBottom = viewTop + codeEditor.clientHeight;

  // If block separator line is out of view, show indicator
  if (blockTopPx < viewTop || blockTopPx > viewBottom) {
    const blockName = getActiveBlockName();
    const direction = blockTopPx < viewTop ? '↑' : '↓';
    indicator.innerHTML = `${direction} <span class="jump-back-name">${escapeHtml(blockName)}</span>`;
    indicator.classList.remove('hidden');
  } else {
    indicator.classList.add('hidden');
  }
}

function getActiveBlockName() {
  if (activeFileIndex < 0 || activeBlockIndex < 0) return 'Selected block';
  const file = loadedFiles[activeFileIndex];
  if (!file) return 'Selected block';
  const block = file.suite.blocks[activeBlockIndex];
  if (!block) return 'Selected block';
  return block.name || `${(block.block_type || 'request')} #${activeBlockIndex + 1}`;
}

function hideJumpBack() {
  const indicator = document.getElementById('codeJumpBack');
  if (indicator) indicator.classList.add('hidden');
}

// Jump back click handler (set up once)
document.addEventListener('click', (e) => {
  const jumpBack = e.target.closest('#codeJumpBack');
  if (jumpBack && activeBlockLineStart >= 0) {
    const lineHeight = 13 * 1.7;
    const padding = 16;
    const targetScroll = (activeBlockLineStart * lineHeight) + padding - 40;
    codeEditor.scrollTop = Math.max(0, targetScroll);
    syncEditorScroll();
  }
});

// --- Live Code <-> Suite Sync (silent) ---
// When the user types in the code editor we keep the in-memory suite
// up-to-date on a debounce, so switching to builder mode reflects the
// latest edits immediately without needing a save. This path is silent:
// no block selection change, no toasts, no modified-flag changes.
let liveCodeParseTimer = null;
let liveCodeParseRev = 0;
function scheduleLiveCodeParse() {
  clearTimeout(liveCodeParseTimer);
  liveCodeParseTimer = setTimeout(async () => {
    if (activeFileIndex < 0 || !loadedFiles[activeFileIndex]) return;
    const rev = ++liveCodeParseRev;
    const content = codeEditor.value;
    // Always update raw content immediately (so builder rebuild reads latest).
    loadedFiles[activeFileIndex].content = content;
    if (!content.trim()) {
      if (rev === liveCodeParseRev) {
        loadedFiles[activeFileIndex].suite = { variables: [], blocks: [], auto_run: null };
      }
      return;
    }
    try {
      const suite = await invoke('parse_test_file', { content });
      // Only apply if this is still the latest parse request.
      if (rev !== liveCodeParseRev) return;
      const file = loadedFiles[activeFileIndex];
      if (!file) return;
      // If block shape changed, drop stale results so dots don't misalign.
      if (file.results && !preserveResultsIfCompatible(file.results, file.suite, suite)) {
        file.results = null;
      }
      file.suite = suite;
      syncDisabledFromSuite(activeFileIndex);
      renderFileTree();
    } catch (_err) {
      // Silent — keep previous suite on parse failure. Ctrl+Enter / Save
      // will surface the error if the user explicitly tries to run/save.
    }
  }, 250);
}

// --- Code Editor Events ---
codeEditor.addEventListener('input', () => {
  codeEditorModified = codeEditor.value !== codeEditorContent;
  codeEditor.classList.toggle('modified', codeEditorModified);
  updateHighlight();
  scheduleLiveCodeParse();
});

codeEditor.addEventListener('scroll', syncEditorScroll);

codeEditor.addEventListener('keydown', (e) => {
  if (e.key === 'Tab') {
    e.preventDefault();
    const start = codeEditor.selectionStart;
    const end = codeEditor.selectionEnd;
    codeEditor.value = codeEditor.value.substring(0, start) + '  ' + codeEditor.value.substring(end);
    codeEditor.selectionStart = codeEditor.selectionEnd = start + 2;
    codeEditor.dispatchEvent(new Event('input'));
  }
});

codeSaveBtn.addEventListener('click', async () => {
  // Sync code editor content to builder state first
  const ok = await syncCodeToBuilder();
  if (!ok) return;

  if (activeFileIndex < 0 || !loadedFiles[activeFileIndex]) {
    showToast('No file to save', 'error');
    return;
  }

  const file = loadedFiles[activeFileIndex];

  if (file.savedPath) {
    // File already has a disk location — write directly
    try {
      await invoke('write_file', { path: file.savedPath, content: file.content });
      showToast(`Saved to ${file.savedPath.split(/[\\/]/).pop()}`, 'success');
    } catch (err) {
      showToast('Save failed: ' + err, 'error');
    }
  } else {
    // Temp file — open Save As dialog
    try {
      const path = await invoke('save_file_with_dialog', {
        defaultName: file.name,
        content: file.content,
        title: 'Save HTTP File',
        filters: [['HTTP Files', 'http']],
      });
      if (!path) return; // cancelled
      file.savedPath = path;
      file.name = path.split(/[\\/]/).pop();
      codeEditorFilename.textContent = file.name;
      renderFileTree();
      showToast(`Saved to ${file.name}`, 'success');
    } catch (err) {
      showToast('Save failed: ' + err, 'error');
    }
  }
});

codeRevertBtn.addEventListener('click', () => {
  codeEditor.value = codeEditorContent;
  codeEditorModified = false;
  codeEditor.classList.remove('modified');
  updateHighlight();
  showToast('Reverted to last saved', 'info');
});

// Run the block the cursor is currently on (from code mode).
// If there are unsaved changes, parse+sync them first; if parsing fails,
// surface the error and don't run (would otherwise execute stale block).
async function runBlockAtCursor() {
  if (activeFileIndex < 0 || !loadedFiles[activeFileIndex]) {
    showToast('No file loaded', 'error');
    return;
  }

  if (codeEditorModified) {
    const ok = await syncCodeToBuilder();
    if (!ok) return; // parse error toast already shown
  }

  const file = loadedFiles[activeFileIndex];
  if (!file.suite || !file.suite.blocks || file.suite.blocks.length === 0) {
    showToast('No runnable blocks in file', 'error');
    return;
  }

  const content = codeEditor.value;
  let starts;
  try {
    starts = await invoke('block_start_lines', { content });
  } catch (err) {
    showToast(`Could not locate block: ${err}`, 'error');
    return;
  }

  if (!Array.isArray(starts) || starts.length === 0) {
    showToast('No runnable blocks in file', 'error');
    return;
  }

  // Figure out cursor line (0-indexed).
  const caret = codeEditor.selectionStart || 0;
  const cursorLine = (codeEditor.value.substring(0, caret).match(/\n/g) || []).length;

  // Find the block whose start_line is the greatest that is <= cursorLine.
  // If cursor is before the first block, use the first block.
  let blockIdx = 0;
  for (let i = 0; i < starts.length; i++) {
    if (starts[i] <= cursorLine) blockIdx = i;
    else break;
  }

  // Guard: blockIdx must be a real runnable block in the suite.
  if (blockIdx >= file.suite.blocks.length) {
    showToast('Block mapping mismatch — try saving first', 'error');
    return;
  }

  await runSingleBlock(activeFileIndex, blockIdx);
}

if (codeRunBtn) {
  codeRunBtn.addEventListener('click', () => { runBlockAtCursor(); });
}

// --- New File ---
newFileBtn.addEventListener('click', () => {
  const template = `@baseUrl = https://api.example.com

### @@test Health Check
GET {{baseUrl}}/health

# @@assert status == 200
`;

  const name = 'untitled.http';

  invoke('parse_test_file', { content: template }).then(parsedSuite => {
    loadedFiles.push({ name, content: template, suite: parsedSuite, results: null, savedPath: null });
    activeFileIndex = loadedFiles.length - 1;
    activeBlockIndex = -1;

    renderFileTree();
    renderEnvVars();

    // Switch to code mode for editing
    switchMode('code');
    codeEditor.value = template;
    codeEditorContent = template;
    codeEditorFilename.textContent = name;
    codeEditorModified = false;
    codeEditor.classList.remove('modified');
    updateHighlight();
    detectAzureAuthNeeded();

    codeEditor.focus();

    showToast('New file created — edit in code mode', 'success');
  }).catch(err => {
    showToast(`Error: ${err}`, 'error');
  });
});

// Start a new blank test in the builder. Appends an empty block to the
// current file (creating a scratch file if none is loaded) and resets the
// builder fields so the user can start typing. The live builder->code flush
// then keeps the scratch file in sync automatically.
if (newTestBtn) {
  newTestBtn.addEventListener('click', async () => {
    await ensureBuilderFile();
    const file = loadedFiles[activeFileIndex];
    if (!file) return;

    const blankBlock = {
      name: '',
      block_type: 'test',
      description: '',
      request: { method: 'GET', url: '', headers: [], body: null },
      assertions: [],
      extracts: [],
      disabled: false,
      group: null,
      depends: [],
      mode: null,
      dev_auth: null,
      compare: false,
      steps: [],
      diff: null,
    };
    file.suite.blocks.push(blankBlock);
    activeBlockIndex = file.suite.blocks.length - 1;

    // Switch to builder mode if not already there so the new block is visible.
    if (currentMode !== 'builder') {
      await switchMode('builder');
    }

    // Reset the builder fields and focus URL so typing starts immediately.
    methodSelect.value = 'GET';
    updateMethodColor();
    urlInput.value = '';
    headersContainer.innerHTML = '';
    headersContainer.appendChild(createHeaderRow());
    bodyInput.value = '';
    bodyType.value = 'none';
    bodyInput.disabled = true;
    if (typeof renderParamsFromUrl === 'function') renderParamsFromUrl();
    if (typeof renderBuilderDirectives === 'function') renderBuilderDirectives(activeFileIndex, activeBlockIndex);
    if (typeof populateMetadata === 'function') populateMetadata(blankBlock);

    renderFileTree();
    urlInput.focus();
  });
}

// --- History Tab ---

function refreshHistoryIfVisible() {
  if (currentMode === 'history') loadHistory();
}

function buildHistoryFilter() {
  const filter = {};
  const method = historyMethodFilter.value;
  if (method) filter.method = method;
  const status = historyStatusFilter.value;
  if (status === '2xx') { filter.status_min = 200; filter.status_max = 299; }
  else if (status === '3xx') { filter.status_min = 300; filter.status_max = 399; }
  else if (status === '4xx') { filter.status_min = 400; filter.status_max = 499; }
  else if (status === '5xx') { filter.status_min = 500; filter.status_max = 599; }
  const source = historySourceFilter.value;
  if (source) filter.source = source;
  const urlSearch = historyUrlSearch.value.trim();
  if (urlSearch) filter.url_contains = urlSearch;
  return filter;
}

/** Apply tree filter selections client-side (multi-select). */
function applyTreeFilter(entries) {
  const sel = getTreeFilterSelections();
  if (!sel) return entries; // nothing selected = show all
  return entries.filter(e => {
    const file = e.file_name || null;
    const group = e.group || null;
    const block = e.block_name || null;
    // Check if this entry matches any selected leaf
    for (const s of sel) {
      if (s.type === 'file' && file === s.file) return true;
      if (s.type === 'group' && file === s.file && group === s.group) return true;
      if (s.type === 'test' && file === s.file && group === s.group && block === s.test) return true;
    }
    return false;
  });
}

async function loadHistory() {
  // Show loading state
  historyLog.innerHTML = `<div class="history-loading"><div class="loading-spinner"></div><span>Loading history…</span></div>`;
  historyStats.innerHTML = '<div class="history-loading"><div class="loading-spinner"></div></div>';
  try {
    const filter = buildHistoryFilter();
    const hasFilter = Object.keys(filter).length > 0;
    // Use summary commands — no request/response bodies transferred to JS
    let entries = hasFilter
      ? await invoke('get_filtered_history_summary', { filter })
      : await invoke('get_history_summary');
    // Rebuild tree filter options from full dataset, then apply selection
    rebuildTreeFilter(entries);
    entries = applyTreeFilter(entries);
    historyCache = entries;
    renderHistoryStats(historyCache);
    renderHistoryLog(historyCache);
    historyCountBadge.textContent = historyCache.length;
    rpLog('debug', 'History loaded (summary)', { count: historyCache.length });
  } catch (err) {
    historyCache = [];
    renderHistoryStats([]);
    renderHistoryLog([]);
    historyCountBadge.textContent = '0';
    rpLog('error', 'Command failed: get_history', String(err));
  }
}

// ── Hierarchical tree filter ──

let _treeFilterState = new Map(); // key -> checked boolean

function _treeKey(type, file, group, test) {
  if (type === 'file') return `f:${file}`;
  if (type === 'group') return `f:${file}\x1Fg:${group}`;
  return `f:${file}\x1Fg:${group}\x1Ft:${test}`;
}

function rebuildTreeFilter(entries) {
  // Build hierarchy: file -> group -> test -> count
  const tree = new Map(); // file -> Map<group, Map<test, count>>
  entries.forEach(e => {
    const file = e.file_name || '(No File)';
    const group = e.group || '(Ungrouped)';
    const test = e.block_name || '(Unnamed)';
    if (!tree.has(file)) tree.set(file, new Map());
    const gm = tree.get(file);
    if (!gm.has(group)) gm.set(group, new Map());
    const tm = gm.get(group);
    tm.set(test, (tm.get(test) || 0) + 1);
  });

  // Preserve existing checked state, default all checked for new items
  const oldState = new Map(_treeFilterState);
  _treeFilterState.clear();
  const allKeysInTree = new Set();

  histTreeFilterList.innerHTML = '';

  for (const [file, groupMap] of [...tree.entries()].sort((a,b) => a[0].localeCompare(b[0]))) {
    const fileKey = _treeKey('file', file);
    allKeysInTree.add(fileKey);
    const fileCount = [...groupMap.values()].reduce((s, tm) => s + [...tm.values()].reduce((s2, c) => s2 + c, 0), 0);
    const fileChecked = oldState.has(fileKey) ? oldState.get(fileKey) : true;
    _treeFilterState.set(fileKey, fileChecked);
    histTreeFilterList.appendChild(_createTreeNode('file', file, fileCount, fileChecked, fileKey, '📄'));

    for (const [group, testMap] of [...groupMap.entries()].sort((a,b) => a[0].localeCompare(b[0]))) {
      const groupKey = _treeKey('group', file, group);
      allKeysInTree.add(groupKey);
      const groupCount = [...testMap.values()].reduce((s, c) => s + c, 0);
      const groupChecked = oldState.has(groupKey) ? oldState.get(groupKey) : true;
      _treeFilterState.set(groupKey, groupChecked);
      histTreeFilterList.appendChild(_createTreeNode('group', group, groupCount, groupChecked, groupKey, '📁'));

      for (const [test, count] of [...testMap.entries()].sort((a,b) => a[0].localeCompare(b[0]))) {
        const testKey = _treeKey('test', file, group, test);
        allKeysInTree.add(testKey);
        const testChecked = oldState.has(testKey) ? oldState.get(testKey) : true;
        _treeFilterState.set(testKey, testChecked);
        histTreeFilterList.appendChild(_createTreeNode('test', test, count, testChecked, testKey, '🧪'));
      }
    }
  }

  updateTreeFilterLabel();
}

function _createTreeNode(level, label, count, checked, key, icon) {
  const node = document.createElement('div');
  node.className = `hist-tree-node level-${level}`;
  node.dataset.key = key;
  node.dataset.level = level;
  node.innerHTML = `
    <input type="checkbox" ${checked ? 'checked' : ''}>
    <span class="hist-tree-node-icon">${icon}</span>
    <span class="hist-tree-node-label" title="${escapeAttr(label)}">${escapeHtml(label)}</span>
    <span class="hist-tree-node-count">${count}</span>
  `;
  const cb = node.querySelector('input');
  cb.addEventListener('change', () => {
    _treeFilterState.set(key, cb.checked);
    // Cascade: if file/group toggled, toggle all children
    if (level === 'file') {
      _cascadeCheck(key + '\x1F', cb.checked);
    } else if (level === 'group') {
      _cascadeCheck(key + '\x1F', cb.checked);
    }
    // Also update parent state
    _syncParentChecks();
    updateTreeFilterLabel();
    loadHistory();
  });
  node.addEventListener('click', (e) => {
    if (e.target === cb) return;
    cb.checked = !cb.checked;
    cb.dispatchEvent(new Event('change'));
  });
  return node;
}

function _cascadeCheck(prefix, checked) {
  for (const [key] of _treeFilterState) {
    if (key.startsWith(prefix)) {
      _treeFilterState.set(key, checked);
    }
  }
  // Update DOM checkboxes
  histTreeFilterList.querySelectorAll('.hist-tree-node').forEach(node => {
    if (node.dataset.key.startsWith(prefix)) {
      node.querySelector('input').checked = checked;
    }
  });
}

function _syncParentChecks() {
  // For each file node, check if all children are checked
  histTreeFilterList.querySelectorAll('.hist-tree-node[data-level="file"]').forEach(fileNode => {
    const fileKey = fileNode.dataset.key;
    const children = [..._treeFilterState.entries()].filter(([k]) => k.startsWith(fileKey + '\x1F'));
    if (children.length > 0) {
      const allChecked = children.every(([, v]) => v);
      fileNode.querySelector('input').checked = allChecked;
      _treeFilterState.set(fileKey, allChecked);
    }
  });
  histTreeFilterList.querySelectorAll('.hist-tree-node[data-level="group"]').forEach(groupNode => {
    const groupKey = groupNode.dataset.key;
    const children = [..._treeFilterState.entries()].filter(([k]) => k.startsWith(groupKey + '\x1F'));
    if (children.length > 0) {
      const allChecked = children.every(([, v]) => v);
      groupNode.querySelector('input').checked = allChecked;
      _treeFilterState.set(groupKey, allChecked);
    }
  });
}

function getTreeFilterSelections() {
  // If all are checked, return null (no filter)
  const allChecked = [..._treeFilterState.values()].every(v => v);
  if (allChecked || _treeFilterState.size === 0) return null;

  // Collect selected nodes — return the most specific checked items
  const selections = [];
  for (const [key, checked] of _treeFilterState) {
    if (!checked) continue;
    const parts = key.split('\x1F');
    if (parts.length === 1) {
      // File level: f:filename
      const file = parts[0].slice(2);
      selections.push({ type: 'file', file: file === '(No File)' ? null : file });
    } else if (parts.length === 2) {
      const file = parts[0].slice(2);
      const group = parts[1].slice(2);
      selections.push({ type: 'group', file: file === '(No File)' ? null : file, group: group === '(Ungrouped)' ? null : group });
    } else if (parts.length === 3) {
      const file = parts[0].slice(2);
      const group = parts[1].slice(2);
      const test = parts[2].slice(2);
      selections.push({ type: 'test', file: file === '(No File)' ? null : file, group: group === '(Ungrouped)' ? null : group, test: test === '(Unnamed)' ? null : test });
    }
  }
  // Deduplicate: if a file is fully selected, remove its children
  const fileKeys = new Set(selections.filter(s => s.type === 'file').map(s => s.file));
  const groupKeys = new Set(selections.filter(s => s.type === 'group').map(s => `${s.file}/${s.group}`));
  return selections.filter(s => {
    if (s.type === 'group' && fileKeys.has(s.file)) return false;
    if (s.type === 'test' && fileKeys.has(s.file)) return false;
    if (s.type === 'test' && groupKeys.has(`${s.file}/${s.group}`)) return false;
    return true;
  });
}

function updateTreeFilterLabel() {
  const allChecked = [..._treeFilterState.values()].every(v => v);
  if (allChecked || _treeFilterState.size === 0) {
    histTreeFilterLabel.textContent = 'All Files / Groups / Tests';
    return;
  }
  const noneChecked = [..._treeFilterState.values()].every(v => !v);
  if (noneChecked) {
    histTreeFilterLabel.textContent = 'None selected';
    return;
  }
  // Count selected leaf nodes
  const checkedLeaves = [..._treeFilterState.entries()].filter(([k, v]) => v && k.includes('/g:') && k.includes('/t:'));
  const totalLeaves = [..._treeFilterState.entries()].filter(([k]) => k.includes('/g:') && k.includes('/t:'));
  if (checkedLeaves.length === totalLeaves.length) {
    histTreeFilterLabel.textContent = 'All Files / Groups / Tests';
  } else {
    histTreeFilterLabel.textContent = `${checkedLeaves.length} of ${totalLeaves.length} tests`;
  }
}

// Toggle dropdown
histTreeFilterBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  histTreeFilterDropdown.classList.toggle('hidden');
});
document.addEventListener('click', (e) => {
  if (!histTreeFilterDropdown.classList.contains('hidden') &&
      !histTreeFilterDropdown.contains(e.target) &&
      !histTreeFilterBtn.contains(e.target)) {
    histTreeFilterDropdown.classList.add('hidden');
  }
});
histTreeSelectAll.addEventListener('click', () => {
  for (const key of _treeFilterState.keys()) _treeFilterState.set(key, true);
  histTreeFilterList.querySelectorAll('input[type="checkbox"]').forEach(cb => cb.checked = true);
  updateTreeFilterLabel();
  loadHistory();
});
histTreeClearAll.addEventListener('click', () => {
  for (const key of _treeFilterState.keys()) _treeFilterState.set(key, false);
  histTreeFilterList.querySelectorAll('input[type="checkbox"]').forEach(cb => cb.checked = false);
  updateTreeFilterLabel();
  loadHistory();
});

function historyPercentile(arr, p) {
  if (arr.length === 0) return 0;
  const sorted = [...arr].sort((a, b) => a - b);
  const idx = Math.ceil(p / 100 * sorted.length) - 1;
  return sorted[Math.max(0, idx)];
}

function renderPercentileBar(label, value, max) {
  const pct = max > 0 ? (value / max) * 100 : 0;
  const color = value < 200 ? 'var(--green)' : value < 500 ? 'var(--orange)' : 'var(--red)';
  return `
    <div class="hist-perc-bar">
      <span class="hist-perc-label">${label}</span>
      <div class="hist-perc-track">
        <div class="hist-perc-fill" style="width:${pct}%;background:${color}"></div>
      </div>
      <span class="hist-perc-value">${value}ms</span>
    </div>`;
}

function renderHistoryStats(entries) {
  if (entries.length === 0) {
    historyStats.innerHTML = '<div class="history-stats-empty">No history entries</div>';
    return;
  }
  const total = entries.length;
  const success = entries.filter(e => e.status >= 200 && e.status < 400).length;
  const client4xx = entries.filter(e => e.status >= 400 && e.status < 500).length;
  const server5xx = entries.filter(e => e.status >= 500).length;
  const times = entries.map(e => e.response_time_ms).filter(t => t > 0);
  const avgTime = times.length > 0 ? Math.round(times.reduce((a, b) => a + b, 0) / times.length) : 0;
  const p50 = historyPercentile(times, 50);
  const p90 = historyPercentile(times, 90);
  const p95 = historyPercentile(times, 95);
  const p99 = historyPercentile(times, 99);
  const maxTime = times.length > 0 ? Math.max(...times) : 0;
  const methods = {};
  entries.forEach(e => { methods[e.method] = (methods[e.method] || 0) + 1; });
  const methodChips = Object.entries(methods)
    .sort((a, b) => b[1] - a[1])
    .map(([m, count]) => `<span class="hist-method-chip method-${m}">${m} <span class="method-chip-count">${count}</span></span>`)
    .join('');
  const maxForBar = maxTime || 1;
  historyStats.innerHTML = `
    <div class="history-stat-cards">
      <div class="hist-stat-card">
        <div class="hist-stat-value">${total}</div>
        <div class="hist-stat-label">Total Requests</div>
      </div>
      <div class="hist-stat-card">
        <div class="hist-stat-value hist-stat-success">${success}</div>
        <div class="hist-stat-label">Success (2xx/3xx)</div>
      </div>
      <div class="hist-stat-card">
        <div class="hist-stat-value hist-stat-warning">${client4xx}</div>
        <div class="hist-stat-label">Client Errors (4xx)</div>
      </div>
      <div class="hist-stat-card">
        <div class="hist-stat-value hist-stat-danger">${server5xx}</div>
        <div class="hist-stat-label">Server Errors (5xx)</div>
      </div>
      <div class="hist-stat-card">
        <div class="hist-stat-value">${avgTime}<span class="hist-stat-unit">ms</span></div>
        <div class="hist-stat-label">Avg Response Time</div>
      </div>
    </div>
    <div class="history-stat-details">
      <div class="history-percentiles">
        <div class="hist-section-title">Response Time Percentiles</div>
        ${renderPercentileBar('P50', p50, maxForBar)}
        ${renderPercentileBar('P90', p90, maxForBar)}
        ${renderPercentileBar('P95', p95, maxForBar)}
        ${renderPercentileBar('P99', p99, maxForBar)}
        ${renderPercentileBar('Max', maxTime, maxForBar)}
      </div>
      <div class="history-methods">
        <div class="hist-section-title">HTTP Methods</div>
        <div class="hist-method-chips">${methodChips}</div>
      </div>
    </div>`;
}

function renderHistoryLog(entries) {
  historyLog.innerHTML = '';
  // Reset selection state on re-render
  historySelectedIds.clear();
  updateHistoryCompareBtn();

  if (entries.length === 0) {
    historyLog.innerHTML = `<div class="history-empty"><div class="empty-icon">📊</div><p>No history entries match your filters</p></div>`;
    return;
  }

  const groupBy = historyGroupBySelect.value;
  switch (groupBy) {
    case 'domain-path': renderGroupedByDomainPath(entries); break;
    case 'url': renderGroupedByUrl(entries); break;
    case 'status': renderGroupedByStatus(entries); break;
    case 'source': renderGroupedBySource(entries); break;
    case 'file-group-test': renderGroupedByFileGroupTest(entries); break;
    case 'compare-steps': renderGroupedByCompareStep(entries); break;
    default: renderFlatList(entries); break;
  }
}

function renderGroupedByCompareStep(entries) {
  const compareEntries = entries.filter(e => e.compare_step);
  const regularEntries = entries.filter(e => !e.compare_step);

  // Group compare entries by block_name
  const blocks = new Map();
  compareEntries.forEach(entry => {
    const key = entry.block_name || 'Unknown Compare';
    if (!blocks.has(key)) blocks.set(key, new Map());
    const steps = blocks.get(key);
    const stepKey = entry.compare_step;
    if (!steps.has(stepKey)) steps.set(stepKey, []);
    steps.get(stepKey).push(entry);
  });

  // Render compare groups
  blocks.forEach((steps, blockName) => {
    const group = document.createElement('div');
    group.className = 'hist-group';
    const key = `compare:${blockName}`;
    group.dataset.groupKey = key;
    const isExpanded = historyExpandedGroups.has(key);
    if (isExpanded) group.classList.add('expanded');

    const totalEntries = Array.from(steps.values()).reduce((sum, arr) => sum + arr.length, 0);
    const header = document.createElement('div');
    header.className = 'hist-group-header';
    header.innerHTML = `
      <span class="hist-group-toggle">\u25B6</span>
      <span class="hist-group-icon">\u21C4</span>
      <span class="hist-group-label">${escapeHtml(blockName)}</span>
      <span class="hist-group-count">${totalEntries} requests across ${steps.size} steps</span>
    `;
    header.addEventListener('click', () => {
      group.classList.toggle('expanded');
      if (group.classList.contains('expanded')) historyExpandedGroups.add(key);
      else historyExpandedGroups.delete(key);
    });
    group.appendChild(header);

    const body = document.createElement('div');
    body.className = 'hist-group-body';

    steps.forEach((stepEntries, stepName) => {
      const stepHeader = document.createElement('div');
      stepHeader.className = 'hist-step-header';
      stepHeader.innerHTML = `<span class="step-label">\u2500 ${escapeHtml(stepName)}</span> <span class="hist-group-count">${stepEntries.length}</span>`;
      body.appendChild(stepHeader);
      stepEntries.forEach(entry => body.appendChild(createHistoryEntryRow(entry)));
    });

    group.appendChild(body);
    historyLog.appendChild(group);
  });

  // Render remaining non-compare entries as flat
  if (regularEntries.length > 0) {
    const sep = document.createElement('div');
    sep.className = 'hist-group-header';
    sep.innerHTML = `<span class="hist-group-label">Regular Requests</span> <span class="hist-group-count">${regularEntries.length}</span>`;
    historyLog.appendChild(sep);
    regularEntries.forEach(entry => historyLog.appendChild(createHistoryEntryRow(entry)));
  }
}

function renderGroupedByUrl(entries) {
  const groups = new Map();
  entries.forEach(entry => {
    const key = `${entry.method} ${entry.url}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(entry);
  });
  groups.forEach((groupEntries, key) => {
    const group = document.createElement('div');
    group.className = 'hist-group';
    group.dataset.groupKey = key;
    const isExpanded = historyExpandedGroups.has(key);
    if (isExpanded) group.classList.add('expanded');
    const successCount = groupEntries.filter(e => e.status >= 200 && e.status < 400).length;
    const failCount = groupEntries.length - successCount;
    const avgTime = Math.round(groupEntries.reduce((sum, e) => sum + e.response_time_ms, 0) / groupEntries.length);
    const firstEntry = groupEntries[0];
    const header = document.createElement('div');
    header.className = 'hist-group-header';
    header.innerHTML = `
      <span class="hist-group-arrow">${isExpanded ? '▾' : '▸'}</span>
      <span class="hist-entry-method method-${firstEntry.method}">${firstEntry.method}</span>
      <span class="hist-group-url" title="${escapeAttr(firstEntry.url)}">${escapeHtml(firstEntry.url)}</span>
      <span class="hist-group-count">${groupEntries.length}</span>
      <span class="hist-group-time">${avgTime}ms</span>
      <span class="hist-group-success">✓${successCount}</span>
      ${failCount > 0 ? `<span class="hist-group-fail">✗${failCount}</span>` : ''}
    `;
    header.addEventListener('click', () => {
      if (historyExpandedGroups.has(key)) {
        historyExpandedGroups.delete(key);
      } else {
        historyExpandedGroups.add(key);
      }
      group.classList.toggle('expanded');
      header.querySelector('.hist-group-arrow').textContent = group.classList.contains('expanded') ? '▾' : '▸';
    });
    const body = document.createElement('div');
    body.className = 'hist-group-body';
    groupEntries.forEach(entry => body.appendChild(createHistoryEntryRow(entry)));
    group.appendChild(header);
    group.appendChild(body);
    historyLog.appendChild(group);
  });
}

function renderFlatList(entries) {
  entries.forEach(entry => historyLog.appendChild(createHistoryEntryRow(entry)));
}

function renderGroupedByDomainPath(entries) {
  const domainMap = new Map();

  entries.forEach(entry => {
    let domain, path;
    try {
      const u = new URL(entry.url);
      domain = u.origin;
      path = u.pathname;
    } catch {
      domain = '(invalid URL)';
      path = entry.url;
    }
    if (!domainMap.has(domain)) domainMap.set(domain, new Map());
    const pathMap = domainMap.get(domain);
    if (!pathMap.has(path)) pathMap.set(path, []);
    pathMap.get(path).push(entry);
  });

  const sortedDomains = [...domainMap.entries()].sort((a, b) => {
    const countA = [...a[1].values()].reduce((s, arr) => s + arr.length, 0);
    const countB = [...b[1].values()].reduce((s, arr) => s + arr.length, 0);
    return countB - countA;
  });

  for (const [domain, pathMap] of sortedDomains) {
    const domainEntries = [...pathMap.values()].flat();
    const domainKey = `domain:${domain}`;
    const isDomainExpanded = historyExpandedGroups.has(domainKey);

    const successCount = domainEntries.filter(e => e.status >= 200 && e.status < 400).length;
    const failCount = domainEntries.length - successCount;
    const avgTime = Math.round(domainEntries.reduce((s, e) => s + e.response_time_ms, 0) / domainEntries.length);

    const domainGroup = document.createElement('div');
    domainGroup.className = 'hist-domain-group' + (isDomainExpanded ? ' expanded' : '');
    domainGroup.dataset.groupKey = domainKey;

    const domainHeader = document.createElement('div');
    domainHeader.className = 'hist-domain-header';
    domainHeader.innerHTML = `
      <span class="hist-group-arrow">${isDomainExpanded ? '▾' : '▸'}</span>
      <span class="hist-domain-name">${escapeHtml(domain)}</span>
      <span class="hist-group-count">${domainEntries.length}</span>
      <span class="hist-group-time">${avgTime}ms</span>
      <span class="hist-group-success">✓${successCount}</span>
      ${failCount > 0 ? `<span class="hist-group-fail">✗${failCount}</span>` : ''}
    `;

    domainHeader.addEventListener('click', () => {
      if (historyExpandedGroups.has(domainKey)) {
        historyExpandedGroups.delete(domainKey);
      } else {
        historyExpandedGroups.add(domainKey);
      }
      domainGroup.classList.toggle('expanded');
      domainHeader.querySelector('.hist-group-arrow').textContent =
        domainGroup.classList.contains('expanded') ? '▾' : '▸';
    });

    const domainBody = document.createElement('div');
    domainBody.className = 'hist-domain-body';

    const sortedPaths = [...pathMap.entries()].sort((a, b) => b[1].length - a[1].length);

    for (const [path, pathEntries] of sortedPaths) {
      const pathKey = `path:${domain}${path}`;
      const isPathExpanded = historyExpandedGroups.has(pathKey);
      const pathSuccess = pathEntries.filter(e => e.status >= 200 && e.status < 400).length;
      const pathFail = pathEntries.length - pathSuccess;
      const pathAvg = Math.round(pathEntries.reduce((s, e) => s + e.response_time_ms, 0) / pathEntries.length);

      const pathGroup = document.createElement('div');
      pathGroup.className = 'hist-path-group' + (isPathExpanded ? ' expanded' : '');
      pathGroup.dataset.groupKey = pathKey;

      const pathHeader = document.createElement('div');
      pathHeader.className = 'hist-path-header';
      pathHeader.innerHTML = `
        <span class="hist-group-arrow">${isPathExpanded ? '▾' : '▸'}</span>
        <span class="hist-path-name">${escapeHtml(path)}</span>
        <span class="hist-group-count">${pathEntries.length}</span>
        <span class="hist-group-time">${pathAvg}ms</span>
        <span class="hist-group-success">✓${pathSuccess}</span>
        ${pathFail > 0 ? `<span class="hist-group-fail">✗${pathFail}</span>` : ''}
      `;

      pathHeader.addEventListener('click', (e) => {
        e.stopPropagation();
        if (historyExpandedGroups.has(pathKey)) {
          historyExpandedGroups.delete(pathKey);
        } else {
          historyExpandedGroups.add(pathKey);
        }
        pathGroup.classList.toggle('expanded');
        pathHeader.querySelector('.hist-group-arrow').textContent =
          pathGroup.classList.contains('expanded') ? '▾' : '▸';
      });

      const pathBody = document.createElement('div');
      pathBody.className = 'hist-path-body';
      pathEntries.forEach(entry => pathBody.appendChild(createHistoryEntryRow(entry)));

      pathGroup.appendChild(pathHeader);
      pathGroup.appendChild(pathBody);
      domainBody.appendChild(pathGroup);
    }

    domainGroup.appendChild(domainHeader);
    domainGroup.appendChild(domainBody);
    historyLog.appendChild(domainGroup);
  }
}

function renderGroupedByStatus(entries) {
  const groups = new Map();
  entries.forEach(entry => {
    const key = entry.status >= 200 && entry.status < 300 ? '2xx Success'
      : entry.status >= 300 && entry.status < 400 ? '3xx Redirect'
      : entry.status >= 400 && entry.status < 500 ? '4xx Client Error'
      : entry.status >= 500 ? '5xx Server Error'
      : 'Error/Unknown';
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(entry);
  });

  const order = ['2xx Success', '3xx Redirect', '4xx Client Error', '5xx Server Error', 'Error/Unknown'];
  for (const statusGroup of order) {
    const groupEntries = groups.get(statusGroup);
    if (!groupEntries || groupEntries.length === 0) continue;
    renderGenericGroup(statusGroup, groupEntries, `status:${statusGroup}`);
  }
}

function renderGroupedBySource(entries) {
  const groups = new Map();
  entries.forEach(entry => {
    const key = entry.source === 'test-run' ? 'Test Run' : entry.source === 'extension-live' ? 'Live Capture' : 'Manual';
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(entry);
  });
  for (const [name, groupEntries] of groups) {
    renderGenericGroup(name, groupEntries, `source:${name}`);
  }
}

function renderGroupedByFileGroupTest(entries) {
  // Build 3-level map: file -> group -> test -> entries[]
  const fileMap = new Map();
  entries.forEach(entry => {
    const file = entry.file_name || '(No File)';
    const group = entry.group || '(Ungrouped)';
    const test = entry.block_name || '(Unnamed)';
    if (!fileMap.has(file)) fileMap.set(file, new Map());
    const groupMap = fileMap.get(file);
    if (!groupMap.has(group)) groupMap.set(group, new Map());
    const testMap = groupMap.get(group);
    if (!testMap.has(test)) testMap.set(test, []);
    testMap.get(test).push(entry);
  });

  // Sort files by entry count descending
  const sortedFiles = [...fileMap.entries()].sort((a, b) => {
    const countA = [...a[1].values()].reduce((s, gm) => s + [...gm.values()].reduce((s2, arr) => s2 + arr.length, 0), 0);
    const countB = [...b[1].values()].reduce((s, gm) => s + [...gm.values()].reduce((s2, arr) => s2 + arr.length, 0), 0);
    return countB - countA;
  });

  for (const [fileName, groupMap] of sortedFiles) {
    const fileEntries = [...groupMap.values()].flatMap(gm => [...gm.values()].flat());
    const fileKey = `fgt-file:${fileName}`;
    const isFileExpanded = historyExpandedGroups.has(fileKey);
    const fileSuccessCount = fileEntries.filter(e => e.status >= 200 && e.status < 400).length;
    const fileFailCount = fileEntries.length - fileSuccessCount;
    const fileAvgTime = Math.round(fileEntries.reduce((s, e) => s + e.response_time_ms, 0) / fileEntries.length);

    const fileGroup = document.createElement('div');
    fileGroup.className = 'hist-domain-group' + (isFileExpanded ? ' expanded' : '');
    fileGroup.dataset.groupKey = fileKey;

    const fileHeader = document.createElement('div');
    fileHeader.className = 'hist-domain-header';
    fileHeader.innerHTML = `
      <span class="hist-group-arrow">${isFileExpanded ? '▾' : '▸'}</span>
      <span class="hist-domain-name">📄 ${escapeHtml(fileName)}</span>
      <span class="hist-group-count">${fileEntries.length}</span>
      <span class="hist-group-time">${fileAvgTime}ms</span>
      <span class="hist-group-success">✓${fileSuccessCount}</span>
      ${fileFailCount > 0 ? `<span class="hist-group-fail">✗${fileFailCount}</span>` : ''}
    `;
    fileHeader.addEventListener('click', () => {
      if (historyExpandedGroups.has(fileKey)) historyExpandedGroups.delete(fileKey);
      else historyExpandedGroups.add(fileKey);
      fileGroup.classList.toggle('expanded');
      fileHeader.querySelector('.hist-group-arrow').textContent = fileGroup.classList.contains('expanded') ? '▾' : '▸';
    });

    const fileBody = document.createElement('div');
    fileBody.className = 'hist-domain-body';

    for (const [groupName, testMap] of groupMap) {
      const groupEntries = [...testMap.values()].flat();
      const groupKey = `fgt-group:${fileName}/${groupName}`;
      const isGroupExpanded = historyExpandedGroups.has(groupKey);
      const groupSuccessCount = groupEntries.filter(e => e.status >= 200 && e.status < 400).length;
      const groupFailCount = groupEntries.length - groupSuccessCount;
      const groupAvgTime = Math.round(groupEntries.reduce((s, e) => s + e.response_time_ms, 0) / groupEntries.length);

      const grpDiv = document.createElement('div');
      grpDiv.className = 'hist-path-group' + (isGroupExpanded ? ' expanded' : '');
      grpDiv.dataset.groupKey = groupKey;

      const grpHeader = document.createElement('div');
      grpHeader.className = 'hist-path-header';
      grpHeader.innerHTML = `
        <span class="hist-group-arrow">${isGroupExpanded ? '▾' : '▸'}</span>
        <span class="hist-path-name">📁 ${escapeHtml(groupName)}</span>
        <span class="hist-group-count">${groupEntries.length}</span>
        <span class="hist-group-time">${groupAvgTime}ms</span>
        <span class="hist-group-success">✓${groupSuccessCount}</span>
        ${groupFailCount > 0 ? `<span class="hist-group-fail">✗${groupFailCount}</span>` : ''}
      `;
      grpHeader.addEventListener('click', (e) => {
        e.stopPropagation();
        if (historyExpandedGroups.has(groupKey)) historyExpandedGroups.delete(groupKey);
        else historyExpandedGroups.add(groupKey);
        grpDiv.classList.toggle('expanded');
        grpHeader.querySelector('.hist-group-arrow').textContent = grpDiv.classList.contains('expanded') ? '▾' : '▸';
      });

      const grpBody = document.createElement('div');
      grpBody.className = 'hist-path-body';

      for (const [testName, testEntries] of testMap) {
        if (testEntries.length === 1) {
          // Single entry — render directly without extra nesting
          grpBody.appendChild(createHistoryEntryRow(testEntries[0]));
        } else {
          // Multiple entries for same test name — nest them
          const testKey = `fgt-test:${fileName}/${groupName}/${testName}`;
          const isTestExpanded = historyExpandedGroups.has(testKey);
          const testGroup = document.createElement('div');
          testGroup.className = 'hist-group' + (isTestExpanded ? ' expanded' : '');
          testGroup.dataset.groupKey = testKey;
          const testSuccessCount = testEntries.filter(e => e.status >= 200 && e.status < 400).length;
          const testFailCount = testEntries.length - testSuccessCount;
          const testAvgTime = Math.round(testEntries.reduce((s, e) => s + e.response_time_ms, 0) / testEntries.length);
          const testHeader = document.createElement('div');
          testHeader.className = 'hist-group-header';
          testHeader.innerHTML = `
            <span class="hist-group-arrow">${isTestExpanded ? '▾' : '▸'}</span>
            <span class="hist-group-title">🧪 ${escapeHtml(testName)}</span>
            <span class="hist-group-count">${testEntries.length}</span>
            <span class="hist-group-time">${testAvgTime}ms</span>
            <span class="hist-group-success">✓${testSuccessCount}</span>
            ${testFailCount > 0 ? `<span class="hist-group-fail">✗${testFailCount}</span>` : ''}
          `;
          testHeader.addEventListener('click', (e) => {
            e.stopPropagation();
            if (historyExpandedGroups.has(testKey)) historyExpandedGroups.delete(testKey);
            else historyExpandedGroups.add(testKey);
            testGroup.classList.toggle('expanded');
            testHeader.querySelector('.hist-group-arrow').textContent = testGroup.classList.contains('expanded') ? '▾' : '▸';
          });
          const testBody = document.createElement('div');
          testBody.className = 'hist-group-body';
          testEntries.forEach(entry => testBody.appendChild(createHistoryEntryRow(entry)));
          testGroup.appendChild(testHeader);
          testGroup.appendChild(testBody);
          grpBody.appendChild(testGroup);
        }
      }

      grpDiv.appendChild(grpHeader);
      grpDiv.appendChild(grpBody);
      fileBody.appendChild(grpDiv);
    }

    fileGroup.appendChild(fileHeader);
    fileGroup.appendChild(fileBody);
    historyLog.appendChild(fileGroup);
  }
}

function renderGenericGroup(title, groupEntries, groupKey) {
  const group = document.createElement('div');
  group.className = 'hist-group';
  group.dataset.groupKey = groupKey;
  const isExpanded = historyExpandedGroups.has(groupKey);
  if (isExpanded) group.classList.add('expanded');

  const successCount = groupEntries.filter(e => e.status >= 200 && e.status < 400).length;
  const failCount = groupEntries.length - successCount;
  const avgTime = Math.round(groupEntries.reduce((sum, e) => sum + e.response_time_ms, 0) / groupEntries.length);

  const header = document.createElement('div');
  header.className = 'hist-group-header';
  header.innerHTML = `
    <span class="hist-group-arrow">${isExpanded ? '▾' : '▸'}</span>
    <span class="hist-group-title">${escapeHtml(title)}</span>
    <span class="hist-group-count">${groupEntries.length}</span>
    <span class="hist-group-time">${avgTime}ms</span>
    <span class="hist-group-success">✓${successCount}</span>
    ${failCount > 0 ? `<span class="hist-group-fail">✗${failCount}</span>` : ''}
  `;

  header.addEventListener('click', () => {
    if (historyExpandedGroups.has(groupKey)) {
      historyExpandedGroups.delete(groupKey);
    } else {
      historyExpandedGroups.add(groupKey);
    }
    group.classList.toggle('expanded');
    header.querySelector('.hist-group-arrow').textContent =
      group.classList.contains('expanded') ? '▾' : '▸';
  });

  const body = document.createElement('div');
  body.className = 'hist-group-body';
  groupEntries.forEach(entry => body.appendChild(createHistoryEntryRow(entry)));

  group.appendChild(header);
  group.appendChild(body);
  historyLog.appendChild(group);
}

function createHistoryEntryRow(entry) {
  const row = document.createElement('div');
  const isViewed = viewedHistoryEntries.has(entry.seq);
  const hash = historyHash(entry);
  const viewState = getViewedState(viewedHistoryEntries, entry.seq, hash);
  row.className = `hist-entry-row${viewState !== 'unseen' ? ` ${viewState}` : ''}`;
  const statusClass = getHistoryStatusClass(entry.status);
  const sourceBadge = entry.source === 'test-run'
    ? '<span class="hist-source-badge test-run">Test Run</span>'
    : entry.source === 'extension-live'
    ? '<span class="hist-source-badge extension-live">📡 Live</span>'
    : '<span class="hist-source-badge manual">Manual</span>';
  const blockName = entry.block_name ? `<span class="hist-block-name">${escapeHtml(entry.block_name)}</span>` : '';
  const stepName = entry.compare_step ? `<span class="hist-step-name">${escapeHtml(entry.compare_step)}</span>` : '';
  const timeColor = entry.response_time_ms < 200 ? 'var(--green)' : entry.response_time_ms < 500 ? 'var(--orange)' : 'var(--red)';
  const viewedIcon = viewState === 'seen' ? '<span class="hist-viewed" title="Viewed">👁</span>'
    : viewState === 'seen-mutated' ? '<span class="hist-viewed mutated" title="Changed since last viewed">👁✱</span>'
    : '';
  // Build selected header values for this entry
  let headerTags = '';
  if (_selectedHistoryHeaders.size > 0) {
    const reqMap = new Map((entry.request_headers || []).map(([k, v]) => [k.toLowerCase(), v]));
    const respMap = new Map((entry.response_headers || []).map(([k, v]) => [k.toLowerCase(), v]));
    _selectedHistoryHeaders.forEach(h => {
      const key = h.toLowerCase();
      const val = reqMap.get(key) ?? respMap.get(key);
      if (val !== undefined) {
        headerTags += `<span class="hist-header-tag" title="${escapeAttr(h)}: ${escapeAttr(val)}"><span class="hist-header-key">${escapeHtml(h)}:</span> ${escapeHtml(val.length > 40 ? val.substring(0, 37) + '…' : val)}</span>`;
      }
    });
  }
  row.innerHTML = `
    <input type="checkbox" class="hist-entry-checkbox" data-seq="${entry.seq}" title="Select for comparison">
    <span class="hist-entry-seq">#${entry.seq}</span>
    <span class="hist-entry-status ${statusClass}">${entry.status || 'ERR'}</span>
    <span class="hist-entry-method method-${entry.method}">${entry.method}</span>
    <span class="hist-entry-url" title="${escapeAttr(entry.url)}">${escapeHtml(truncateUrl(entry.url))}</span>
    <span class="hist-entry-time" style="color:${timeColor}">${entry.response_time_ms}ms</span>
    ${sourceBadge}
    ${blockName}
    ${stepName}
    <span class="hist-entry-timestamp">${formatHistoryTime(entry.timestamp)}</span>
    ${viewedIcon}
    ${headerTags ? `<div class="hist-entry-headers">${headerTags}</div>` : ''}
  `;
  // Checkbox for comparison selection
  const checkbox = row.querySelector('.hist-entry-checkbox');
  checkbox.checked = historySelectedIds.has(entry.seq);
  checkbox.addEventListener('click', (e) => {
    e.stopPropagation();
    if (checkbox.checked) {
      if (historySelectedIds.size >= 2) {
        // Deselect oldest, keep latest + new
        const oldest = [...historySelectedIds][0];
        historySelectedIds.delete(oldest);
        historyLog.querySelector(`.hist-entry-checkbox[data-seq="${oldest}"]`)
          && (historyLog.querySelector(`.hist-entry-checkbox[data-seq="${oldest}"]`).checked = false);
      }
      historySelectedIds.add(entry.seq);
    } else {
      historySelectedIds.delete(entry.seq);
    }
    updateHistoryCompareBtn();
  });
  row.addEventListener('click', (e) => {
    if (e.target.closest('.hist-entry-checkbox')) return;
    viewedHistoryEntries.set(entry.seq, { hash });
    row.className = 'hist-entry-row seen';
    const vi = row.querySelector('.hist-viewed');
    if (vi) { vi.textContent = '👁'; vi.title = 'Viewed'; vi.classList.remove('mutated'); }
    else row.insertAdjacentHTML('beforeend', '<span class="hist-viewed" title="Viewed">👁</span>');
    showHistoryDetail(entry);
  });
  return row;
}

function getHistoryStatusClass(status) {
  if (!status || status === 0) return 'hist-status-err';
  if (status >= 200 && status < 300) return 'hist-status-2xx';
  if (status >= 300 && status < 400) return 'hist-status-3xx';
  if (status >= 400 && status < 500) return 'hist-status-4xx';
  return 'hist-status-5xx';
}

function formatHistoryTime(isoString) {
  if (!isoString) return '';
  const date = new Date(isoString);
  const now = new Date();
  const diffMs = now - date;
  const diffMin = Math.floor(diffMs / 60000);
  const diffHr = Math.floor(diffMs / 3600000);
  if (diffMin < 1) return 'just now';
  if (diffMin < 60) return `${diffMin}m ago`;
  if (diffHr < 24) return `${diffHr}h ago`;
  return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

async function showHistoryDetail(entry) {
  historyDetailOverlay.classList.remove('hidden');
  const statusClass = getHistoryStatusClass(entry.status);
  const timeColor = entry.response_time_ms < 200 ? 'var(--green)' : entry.response_time_ms < 500 ? 'var(--orange)' : 'var(--red)';
  historyDetailTitle.innerHTML = `
    <span class="hist-entry-method method-${entry.method}">${entry.method}</span>
    <span class="hist-entry-status ${statusClass}">${entry.status || 'ERR'}</span>
    <span style="color:${timeColor};font-family:var(--font-mono);font-size:12px">${entry.response_time_ms}ms</span>
    <span style="color:var(--text-muted);font-size:12px">${formatBytes(entry.response_size_bytes || 0)}</span>
  `;
  const reqHeaders = (entry.request_headers || []).map(([k, v]) =>
    `<tr><td>${escapeHtml(k)}</td><td>${escapeHtml(v)}</td></tr>`
  ).join('');
  const respHeaders = (entry.response_headers || []).map(([k, v]) =>
    `<tr><td>${escapeHtml(k)}</td><td>${escapeHtml(v)}</td></tr>`
  ).join('');

  // Show headers immediately; bodies load on demand from Rust
  historyDetailBody.innerHTML = `
    <div class="hist-detail-url">
      <span class="hist-entry-method method-${entry.method}">${entry.method}</span>
      <span class="hist-detail-full-url">${escapeHtml(entry.url)}</span>
    </div>
    <div class="hist-detail-meta">
      <span class="hist-entry-seq">#${entry.seq}</span>
      <span class="hist-source-badge ${entry.source === 'test-run' ? 'test-run' : entry.source === 'extension-live' ? 'extension-live' : 'manual'}">${entry.source === 'test-run' ? 'Test Run' : entry.source === 'extension-live' ? '📡 Live' : 'Manual'}</span>
      ${entry.block_name ? `<span class="hist-block-name">${escapeHtml(entry.block_name)}</span>` : ''}
      ${entry.compare_step ? `<span class="hist-step-name">${escapeHtml(entry.compare_step)}</span>` : ''}
      <span class="hist-entry-timestamp">${formatHistoryTime(entry.timestamp)}</span>
      ${entry.run_id ? `<span class="hist-run-id" title="Run ID: ${escapeAttr(entry.run_id)}">🔗 Run</span>` : ''}
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Request Headers</div>
      ${reqHeaders ? `<table class="hist-detail-headers"><thead><tr><th>Header</th><th>Value</th></tr></thead><tbody>${reqHeaders}</tbody></table>` : '<div class="hist-detail-empty">No headers</div>'}
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Request Body</div>
      <div class="hist-detail-req-body"><div class="history-loading"><div class="loading-spinner"></div><span>Loading…</span></div></div>
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Response Headers</div>
      ${respHeaders ? `<table class="hist-detail-headers"><thead><tr><th>Header</th><th>Value</th></tr></thead><tbody>${respHeaders}</tbody></table>` : '<div class="hist-detail-empty">No headers</div>'}
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Response Body</div>
      <div class="hist-detail-resp-body"><div class="history-loading"><div class="loading-spinner"></div><span>Loading…</span></div></div>
    </div>`;

  // Fetch full entry with bodies from Rust (on demand)
  try {
    const full = await invoke('get_history_entry', { seq: entry.seq });
    if (!full) return;
    const reqBodyContainer = historyDetailBody.querySelector('.hist-detail-req-body');
    const respBodyContainer = historyDetailBody.querySelector('.hist-detail-resp-body');
    // Request body
    if (full.request_body) {
      let reqBody = full.request_body;
      try { reqBody = JSON.stringify(JSON.parse(full.request_body), null, 2); } catch { /* keep raw */ }
      reqBodyContainer.innerHTML = `<pre class="hist-detail-body-pre">${escapeHtml(reqBody)}</pre>`;
    } else {
      reqBodyContainer.innerHTML = '<pre class="hist-detail-body-pre">(no body)</pre>';
    }
    // Response body — use size-aware rendering
    if (full.response_body) {
      renderResponseBodyInto(respBodyContainer, full.response_body, full.response_headers || []);
    } else {
      respBodyContainer.innerHTML = '<pre class="hist-detail-body-pre">(no body)</pre>';
    }
  } catch (err) {
    rpLog('error', 'Failed to load history entry body', String(err));
  }
}

function closeHistoryDetail() {
  historyDetailOverlay.classList.add('hidden');
  // Free DOM memory — clear large rendered bodies
  historyDetailBody.innerHTML = '';
}

function updateHistoryCompareBtn() {
  const btn = document.getElementById('historyCompareBtn');
  const count = document.getElementById('historySelectedCount');
  if (btn) {
    btn.classList.toggle('hidden', historySelectedIds.size < 2);
  }
  if (count) {
    count.textContent = historySelectedIds.size > 0 ? `${historySelectedIds.size} selected` : '';
    count.classList.toggle('hidden', historySelectedIds.size === 0);
  }
}

async function openHistoryComparison() {
  if (historySelectedIds.size < 2) return;
  const [seqA, seqB] = [...historySelectedIds];
  const summaryA = historyCache.find(e => e.seq === seqA);
  const summaryB = historyCache.find(e => e.seq === seqB);
  if (!summaryA || !summaryB) return;

  showToast('Loading response bodies…', 'info');
  try {
    const [fullA, fullB] = await Promise.all([
      invoke('get_history_entry', { seq: seqA }),
      invoke('get_history_entry', { seq: seqB }),
    ]);
    if (!fullA || !fullB) {
      showToast('Could not load one or both entries', 'error');
      return;
    }

    openComparisonOverlay(fullA, fullB);
  } catch (err) {
    showToast(`Failed to load entries: ${err}`, 'error');
    rpLog('error', 'Failed to load history entries for comparison', String(err));
  }
}

// --- Autocomplete State ---
let acSuggestions = [];
let acDomainPaths = [];
let acActiveIndex = -1;
let acDebounceTimer = null;
const autocompleteDropdown = $('#autocompleteDropdown');

async function fetchAutocompleteSuggestions(prefix) {
  if (!prefix || prefix.length < 2) {
    hideAutocomplete();
    return;
  }
  try {
    const [urlsResult, pathsResult] = await Promise.allSettled([
      invoke('suggest_urls', { prefix }),
      invoke('suggest_domain_paths', { prefix }),
    ]);
    acSuggestions = urlsResult.status === 'fulfilled' ? urlsResult.value : [];
    acDomainPaths = pathsResult.status === 'fulfilled' ? pathsResult.value : [];
    if (acSuggestions.length === 0 && acDomainPaths.length === 0) {
      hideAutocomplete();
      return;
    }
    acActiveIndex = acDomainPaths.length > 0 ? -1 : 0;
    renderAutocomplete(prefix);
  } catch {
    hideAutocomplete();
  }
}

function renderAutocomplete(prefix) {
  const prefixLower = prefix.toLowerCase();
  let html = '';
  let totalItems = 0;

  // --- Quick Paths section (top 5 domain→path) ---
  if (acDomainPaths.length > 0) {
    html += '<div class="ac-section-label">Top Paths</div>';
    acDomainPaths.forEach((dp, idx) => {
      const dpUrl = dp.domain_path;
      const matchEnd = dpUrl.toLowerCase().indexOf(prefixLower) !== -1
        ? dpUrl.toLowerCase().indexOf(prefixLower) + prefix.length
        : prefix.length;
      const matchPart = dpUrl.substring(0, Math.min(matchEnd, dpUrl.length));
      const restPart = dpUrl.substring(Math.min(matchEnd, dpUrl.length));
      const itemIdx = idx;

      html += `<div class="autocomplete-item ac-domain-path${itemIdx === acActiveIndex ? ' active' : ''}" data-index="${itemIdx}" data-type="path">
        <span class="autocomplete-url"><span class="ac-match">${escapeHtml(matchPart)}</span><span class="ac-completion">${escapeHtml(restPart)}</span></span>
        <span class="ac-path-meta">${dp.frequency}× · ${dp.url_count} URL${dp.url_count !== 1 ? 's' : ''}</span>
      </div>`;
      totalItems++;
    });
  }

  // --- Full URL suggestions ---
  if (acSuggestions.length > 0) {
    if (acDomainPaths.length > 0) {
      html += '<div class="ac-section-divider"></div>';
      html += '<div class="ac-section-label">URLs</div>';
    }
    acSuggestions.forEach((s, idx) => {
      const url = s.url;
      const matchEnd = url.toLowerCase().indexOf(prefixLower) !== -1
        ? url.toLowerCase().indexOf(prefixLower) + prefix.length
        : prefix.length;
      const matchPart = url.substring(0, Math.min(matchEnd, url.length));
      const restPart = url.substring(Math.min(matchEnd, url.length));
      const itemIdx = acDomainPaths.length + idx;

      html += `<div class="autocomplete-item${itemIdx === acActiveIndex ? ' active' : ''}" data-index="${itemIdx}" data-type="url">
        <span class="autocomplete-url"><span class="ac-match">${escapeHtml(matchPart)}</span><span class="ac-completion">${escapeHtml(restPart)}</span></span>
        <span class="autocomplete-freq">${s.frequency}×</span>
      </div>`;
      totalItems++;
    });
  }

  html += '<div class="autocomplete-hint">↑↓ Navigate · Tab Accept segment · Enter Accept · Esc Dismiss</div>';

  autocompleteDropdown.innerHTML = html;
  autocompleteDropdown.classList.remove('hidden');

  autocompleteDropdown.querySelectorAll('.autocomplete-item').forEach(item => {
    item.addEventListener('mousedown', (e) => {
      e.preventDefault();
      const idx = parseInt(item.dataset.index);
      const type = item.dataset.type;
      if (type === 'path') {
        acceptDomainPathSuggestion(idx);
      } else {
        acceptSuggestion(idx - acDomainPaths.length);
      }
    });
  });

  updateGhostText(prefix);
}

function hideAutocomplete() {
  autocompleteDropdown.classList.add('hidden');
  acSuggestions = [];
  acDomainPaths = [];
  acActiveIndex = -1;
  removeGhostText();
}

function acceptSuggestion(index) {
  if (index < 0 || index >= acSuggestions.length) return;
  const suggestion = acSuggestions[index];
  const fullUrl = suggestion.url;
  const nextSepIdx = findNextSeparatorBoundary(fullUrl, historyUrlSearch.value.length);
  const accepted = fullUrl.substring(0, nextSepIdx);

  historyUrlSearch.value = accepted;
  historyUrlSearch.focus();

  if (accepted === fullUrl) {
    hideAutocomplete();
    loadHistory();
  } else {
    fetchAutocompleteSuggestions(accepted);
  }
}

function acceptFullSuggestion(index) {
  if (index < 0 || index >= acSuggestions.length) return;
  historyUrlSearch.value = acSuggestions[index].url;
  hideAutocomplete();
  loadHistory();
}

function acceptDomainPathSuggestion(index) {
  if (index < 0 || index >= acDomainPaths.length) return;
  const domainPath = acDomainPaths[index].domain_path;
  historyUrlSearch.value = domainPath;
  historyUrlSearch.focus();
  fetchAutocompleteSuggestions(domainPath);
}

function findNextSeparatorBoundary(url, fromIndex) {
  const separators = ['/', '.', ':', '?', '&', '='];
  let i = fromIndex;
  while (i < url.length && separators.includes(url[i])) i++;
  while (i < url.length && !separators.includes(url[i])) i++;
  if (i < url.length && separators.includes(url[i])) i++;
  return i;
}

function updateGhostText(prefix) {
  removeGhostText();
  if (!prefix) return;

  // Pick the best suggestion for ghost text:
  // 1. If user has actively selected something, use that
  // 2. Otherwise use the first URL suggestion (most relevant for inline completion)
  // 3. Fall back to first domain path
  let suggestion;
  if (acActiveIndex >= 0 && acActiveIndex < acDomainPaths.length) {
    suggestion = acDomainPaths[acActiveIndex].domain_path;
  } else if (acActiveIndex >= acDomainPaths.length) {
    const urlIdx = acActiveIndex - acDomainPaths.length;
    if (urlIdx >= 0 && urlIdx < acSuggestions.length) {
      suggestion = acSuggestions[urlIdx].url;
    }
  } else if (acSuggestions.length > 0) {
    suggestion = acSuggestions[0].url;
  } else if (acDomainPaths.length > 0) {
    suggestion = acDomainPaths[0].domain_path;
  }
  if (!suggestion) return;

  if (!suggestion.toLowerCase().startsWith(prefix.toLowerCase())) return;
  const completion = suggestion.substring(prefix.length);
  if (!completion) return;

  const ghost = document.createElement('div');
  ghost.className = 'ghost-text';
  ghost.innerHTML = `<span class="ghost-prefix">${escapeHtml(prefix)}</span><span class="ghost-completion">${escapeHtml(completion)}</span>`;

  const wrapper = historyUrlSearch.closest('.history-search-wrapper');
  if (wrapper) wrapper.appendChild(ghost);
}

function removeGhostText() {
  const wrapper = historyUrlSearch.closest('.history-search-wrapper');
  if (wrapper) {
    const ghost = wrapper.querySelector('.ghost-text');
    if (ghost) ghost.remove();
  }
}

// --- History Event Listeners ---
historyMethodFilter.addEventListener('change', loadHistory);
historyStatusFilter.addEventListener('change', loadHistory);
historySourceFilter.addEventListener('change', loadHistory);
historyGroupBySelect.addEventListener('change', () => renderHistoryLog(historyCache));

// --- Header picker multi-select ---
const histHeaderPickerBtn = $('#histHeaderPickerBtn');
const histHeaderPickerDropdown = $('#histHeaderPickerDropdown');
const histHeaderPickerSearch = $('#histHeaderPickerSearch');
const histHeaderPickerList = $('#histHeaderPickerList');

histHeaderPickerBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  const isHidden = histHeaderPickerDropdown.classList.contains('hidden');
  histHeaderPickerDropdown.classList.toggle('hidden');
  if (isHidden) {
    populateHeaderPickerList();
    histHeaderPickerSearch.value = '';
    histHeaderPickerSearch.focus();
  }
});

document.addEventListener('click', (e) => {
  if (!e.target.closest('#histHeaderPicker')) {
    histHeaderPickerDropdown.classList.add('hidden');
  }
});

function collectAllHeaders() {
  const reqSet = new Set();
  const respSet = new Set();
  historyCache.forEach(entry => {
    (entry.request_headers || []).forEach(([k]) => reqSet.add(k));
    (entry.response_headers || []).forEach(([k]) => respSet.add(k));
  });
  return { req: [...reqSet].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase())),
           resp: [...respSet].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase())) };
}

function populateHeaderPickerList(filter) {
  const { req, resp } = collectAllHeaders();
  const f = (filter || '').toLowerCase();
  let html = '';
  const filteredReq = req.filter(h => !f || h.toLowerCase().includes(f));
  const filteredResp = resp.filter(h => !f || h.toLowerCase().includes(f));
  if (filteredReq.length > 0) {
    html += '<div class="hhp-section">Request Headers</div>';
    filteredReq.forEach(h => {
      const checked = _selectedHistoryHeaders.has(h) ? 'checked' : '';
      html += `<label class="hhp-item"><input type="checkbox" value="${escapeAttr(h)}" data-type="req" ${checked}><span>${escapeHtml(h)}</span></label>`;
    });
  }
  if (filteredResp.length > 0) {
    html += '<div class="hhp-section">Response Headers</div>';
    filteredResp.forEach(h => {
      const checked = _selectedHistoryHeaders.has(h) ? 'checked' : '';
      html += `<label class="hhp-item"><input type="checkbox" value="${escapeAttr(h)}" data-type="resp" ${checked}><span>${escapeHtml(h)}</span></label>`;
    });
  }
  if (!html) html = '<div class="hhp-empty">No headers found</div>';
  histHeaderPickerList.innerHTML = html;
}

histHeaderPickerSearch.addEventListener('input', () => {
  populateHeaderPickerList(histHeaderPickerSearch.value);
});

histHeaderPickerList.addEventListener('change', (e) => {
  const cb = e.target;
  if (cb.type !== 'checkbox') return;
  const header = cb.value;
  if (cb.checked) _selectedHistoryHeaders.add(header);
  else _selectedHistoryHeaders.delete(header);
  // Update button label
  histHeaderPickerBtn.textContent = _selectedHistoryHeaders.size > 0
    ? `Headers (${_selectedHistoryHeaders.size}) ▾` : 'Headers ▾';
  renderHistoryLog(historyCache);
});
let historySearchTimeout;
historyUrlSearch.addEventListener('input', () => {
  const value = historyUrlSearch.value.trim();

  clearTimeout(historySearchTimeout);
  historySearchTimeout = setTimeout(loadHistory, 300);

  clearTimeout(acDebounceTimer);
  acDebounceTimer = setTimeout(() => fetchAutocompleteSuggestions(value), 150);
});

historyUrlSearch.addEventListener('keydown', (e) => {
  if (autocompleteDropdown.classList.contains('hidden')) return;
  const totalItems = acDomainPaths.length + acSuggestions.length;

  if (e.key === 'ArrowDown') {
    e.preventDefault();
    acActiveIndex = Math.min(acActiveIndex + 1, totalItems - 1);
    renderAutocomplete(historyUrlSearch.value);
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    acActiveIndex = Math.max(acActiveIndex - 1, -1);
    renderAutocomplete(historyUrlSearch.value);
  } else if (e.key === 'Tab') {
    e.preventDefault();
    if (acActiveIndex >= 0 && acActiveIndex < acDomainPaths.length) {
      acceptDomainPathSuggestion(acActiveIndex);
    } else if (acActiveIndex >= acDomainPaths.length) {
      acceptSuggestion(acActiveIndex - acDomainPaths.length);
    } else if (acActiveIndex === -1) {
      // No selection — accept ghost text (first URL suggestion, or first domain path)
      if (acSuggestions.length > 0) {
        acceptSuggestion(0);
      } else if (acDomainPaths.length > 0) {
        acceptDomainPathSuggestion(0);
      }
    }
  } else if (e.key === 'Enter') {
    e.preventDefault();
    if (acActiveIndex >= 0 && acActiveIndex < acDomainPaths.length) {
      acceptDomainPathSuggestion(acActiveIndex);
      loadHistory();
    } else if (acActiveIndex >= acDomainPaths.length) {
      acceptFullSuggestion(acActiveIndex - acDomainPaths.length);
    } else if (acActiveIndex === -1) {
      // No selection — accept full first suggestion
      if (acSuggestions.length > 0) {
        acceptFullSuggestion(0);
      } else if (acDomainPaths.length > 0) {
        acceptDomainPathSuggestion(0);
        loadHistory();
      }
    }
  } else if (e.key === 'Escape') {
    hideAutocomplete();
  }
});

historyUrlSearch.addEventListener('blur', () => {
  setTimeout(hideAutocomplete, 200);
});

historyUrlSearch.addEventListener('focus', () => {
  const value = historyUrlSearch.value.trim();
  if (value.length >= 2) {
    fetchAutocompleteSuggestions(value);
  }
});

$('#historyCompareBtn').addEventListener('click', openHistoryComparison);

$('#historyExpandAllBtn').addEventListener('click', () => {
  historyLog.querySelectorAll('.hist-group, .hist-domain-group, .hist-path-group').forEach(g => {
    g.classList.add('expanded');
    const arrow = g.querySelector(':scope > .hist-group-header .hist-group-arrow, :scope > .hist-domain-header .hist-group-arrow, :scope > .hist-path-header .hist-group-arrow');
    if (arrow) arrow.textContent = '▾';
    if (g.dataset.groupKey) historyExpandedGroups.add(g.dataset.groupKey);
  });
});

$('#historyCollapseAllBtn').addEventListener('click', () => {
  historyExpandedGroups.clear();
  historyLog.querySelectorAll('.hist-group, .hist-domain-group, .hist-path-group').forEach(g => {
    g.classList.remove('expanded');
    const arrow = g.querySelector(':scope > .hist-group-header .hist-group-arrow, :scope > .hist-domain-header .hist-group-arrow, :scope > .hist-path-header .hist-group-arrow');
    if (arrow) arrow.textContent = '▸';
  });
});

// Estimate UI memory usage for clear dropdown
function estimateMemoryUsage() {
  let bytes = 0;
  loadedFiles.forEach(f => {
    if (f.results) bytes += JSON.stringify(f.results).length * 2;
  });
  if (lastResponse) bytes += JSON.stringify(lastResponse).length * 2;
  bytes += rpLogs.length * 200;
  if (typeof liveCapturedRequests !== 'undefined') bytes += liveCapturedRequests.length * 500;
  bytes += historyCache.length * 300;
  bytes += diffCacheBytes;
  return bytes;
}

// Clear toolbar dropdown
{
  const clearWrapper = document.querySelector('.clear-wrapper');
  const clearDropdown = $('#clearDropdown');
  let clearHideTimer = null;
  if (clearWrapper && clearDropdown) {
    clearWrapper.addEventListener('mouseenter', () => {
      clearTimeout(clearHideTimer);
      // Update memory display on hover
      const memEl = $('#clearMemValue');
      if (memEl) memEl.textContent = formatBytes(estimateMemoryUsage());
      clearDropdown.style.display = 'block';
    });
    clearWrapper.addEventListener('mouseleave', () => {
      clearHideTimer = setTimeout(() => { clearDropdown.style.display = 'none'; }, 200);
    });
  }
}

$('#clearAllAction').addEventListener('click', async () => {
  try {
    // Clear Rust-side history store
    await invoke('clear_history');

    // Clear all JS state — reset as if nothing was ever run
    historyCache = [];
    historyExpandedGroups.clear();
    historySelectedIds.clear();
    _selectedHistoryHeaders = new Set();
    viewedResults.clear();
    viewedHistoryEntries.clear();
    diffCache.clear();
    diffCacheBytes = 0;
    runHistory.clear();
    liveCapturedRequests = [];

    // Clear all test results and block statuses
    loadedFiles.forEach(f => { f.results = null; });
    updateBlockStatuses();

    // Reset response panel
    lastResponse = null;
    responseBody.innerHTML = '';
    responseHeadersBody.innerHTML = '';
    responseEmpty.classList.remove('hidden');
    responseContent.classList.add('hidden');

    // Reset history UI
    renderHistoryStats([]);
    renderHistoryLog([]);
    historyCountBadge.textContent = '0';

    // Reset test results bar
    testResultsBar.classList.add('hidden');
    testResultsDetails.innerHTML = '';

    // Clear viewed indicators in DOM
    document.querySelectorAll('.result-detail-row').forEach(r => {
      r.classList.remove('seen', 'seen-mutated');
      const v = r.querySelector('.detail-viewed');
      if (v) v.remove();
    });

    showToast('All data cleared', 'success');
    rpLog('info', 'Clear All: history, test results, caches, viewed state reset');
  } catch (err) {
    showToast(`Failed to clear: ${err}`, 'error');
  }
  $('#clearDropdown').style.display = 'none';
});

$('#historyDetailCloseBtn').addEventListener('click', closeHistoryDetail);
historyDetailOverlay.addEventListener('click', (e) => {
  if (e.target === historyDetailOverlay) closeHistoryDetail();
});

// --- Diff Viewer ---

/**
 * Compute line-level diff using LCS (O(nm) but fine for typical API responses).
 * Returns array of ops: { type: 'same'|'add'|'remove'|'change', left?: string, right?: string }
 */
function computeLineDiff(textA, textB) {
  const linesA = textA.split('\n');
  const linesB = textB.split('\n');
  const m = linesA.length, n = linesB.length;

  // Build LCS table
  const dp = Array.from({ length: m + 1 }, () => new Array(n + 1).fill(0));
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      if (linesA[i - 1] === linesB[j - 1]) dp[i][j] = dp[i - 1][j - 1] + 1;
      else dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1]);
    }
  }

  // Backtrack to get diff ops
  const ops = [];
  let i = m, j = n;
  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && linesA[i - 1] === linesB[j - 1]) {
      ops.push({ type: 'same', left: linesA[i - 1], right: linesB[j - 1] });
      i--; j--;
    } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
      ops.push({ type: 'add', right: linesB[j - 1] });
      j--;
    } else {
      ops.push({ type: 'remove', left: linesA[i - 1] });
      i--;
    }
  }
  ops.reverse();

  // Merge adjacent remove+add into 'change' ops
  const merged = [];
  let idx = 0;
  while (idx < ops.length) {
    if (ops[idx].type === 'remove' && idx + 1 < ops.length && ops[idx + 1].type === 'add') {
      merged.push({ type: 'change', left: ops[idx].left, right: ops[idx + 1].right });
      idx += 2;
    } else {
      merged.push(ops[idx]);
      idx++;
    }
  }

  return merged;
}

/**
 * Compute character-level diff within two strings.
 * Returns HTML with <span class="diff-char-rm/add"> around differing chars.
 */
function charDiffHighlight(lineA, lineB, side, hlFn) {
  const a = lineA, b = lineB;

  // Find common prefix and suffix, mark the middle as changed
  let prefixLen = 0;
  while (prefixLen < a.length && prefixLen < b.length && a[prefixLen] === b[prefixLen]) prefixLen++;

  let suffixLen = 0;
  while (suffixLen < a.length - prefixLen && suffixLen < b.length - prefixLen &&
         a[a.length - 1 - suffixLen] === b[b.length - 1 - suffixLen]) suffixLen++;

  const text = side === 'left' ? a : b;
  const start = prefixLen;
  const end = text.length - suffixLen;

  if (start >= end) return hlFn(escapeHtml(text));

  const cls = side === 'left' ? 'diff-char-rm' : 'diff-char-add';

  // Apply syntax highlighting to each segment, wrap changed part
  const prefixHtml = hlFn(escapeHtml(text.substring(0, start)));
  const changedHtml = `<span class="${cls}">${hlFn(escapeHtml(text.substring(start, end)))}</span>`;
  const suffixHtml = hlFn(escapeHtml(text.substring(end)));

  return prefixHtml + changedHtml + suffixHtml;
}

// --- Diff Hunk Collapsing ---

function buildDiffHunks(ops, contextLines = 3) {
  const changeIndices = [];
  ops.forEach((op, i) => { if (op.type !== 'same') changeIndices.push(i); });

  if (changeIndices.length === 0) {
    return [{ type: 'collapse', startIdx: 0, endIdx: ops.length - 1, count: ops.length }];
  }

  const ranges = [];
  changeIndices.forEach(i => {
    const start = Math.max(0, i - contextLines);
    const end = Math.min(ops.length - 1, i + contextLines);
    if (ranges.length > 0 && start <= ranges[ranges.length - 1].end + 1) {
      ranges[ranges.length - 1].end = end;
    } else {
      ranges.push({ start, end });
    }
  });

  const hunks = [];
  let pos = 0;
  ranges.forEach(range => {
    if (pos < range.start) {
      hunks.push({ type: 'collapse', startIdx: pos, endIdx: range.start - 1, count: range.start - pos });
    }
    hunks.push({ type: 'hunk', startIdx: range.start, endIdx: range.end });
    pos = range.end + 1;
  });
  if (pos < ops.length) {
    hunks.push({ type: 'collapse', startIdx: pos, endIdx: ops.length - 1, count: ops.length - pos });
  }
  return hunks;
}

function renderRustCharHighlight(text, highlights, cls, hlFn) {
  if (!highlights || highlights.length === 0) return hlFn(escapeHtml(text));
  let result = '';
  let pos = 0;
  highlights.forEach(span => {
    if (span.start > pos) result += hlFn(escapeHtml(text.substring(pos, span.start)));
    result += `<span class="${cls}">${hlFn(escapeHtml(text.substring(span.start, span.end)))}</span>`;
    pos = span.end;
  });
  if (pos < text.length) result += hlFn(escapeHtml(text.substring(pos)));
  return result;
}

async function openDiffViewer(fileIdx, blockIdx) {
  const file = loadedFiles[fileIdx];
  const block = file?.suite?.blocks?.[blockIdx];
  const br = file?.results?.block_results?.[blockIdx];
  if (!br || !br.diff_result) return;

  const diff = br.diff_result;
  const steps = br.step_results || [];

  const stepAName = block?.diff?.step_a || steps[0]?.name || 'Step A';
  const stepBName = block?.diff?.step_b || steps[1]?.name || 'Step B';

  const stepA = steps.find(s => s.name === stepAName);
  const stepB = steps.find(s => s.name === stepBName);
  const bodyA = stepA?.response?.body || '(no response)';
  const bodyB = stepB?.response?.body || '(no response)';

  // Summary badges
  const summaryEl = $('#diffViewerSummary');
  summaryEl.innerHTML = `
    <span class="diff-badge ${diff.match_exact ? 'diff-match' : 'diff-mismatch'}">
      ${diff.match_exact ? '✓ Exact Match' : `${(diff.similarity * 100).toFixed(1)}% Similar`}
    </span>
    <span class="diff-type">${diff.is_json ? 'JSON' : 'Text'}</span>
    ${diff.added_count ? `<span class="diff-stat-badge added">+${diff.added_count} added</span>` : ''}
    ${diff.removed_count ? `<span class="diff-stat-badge removed">-${diff.removed_count} removed</span>` : ''}
    ${diff.changed_count ? `<span class="diff-stat-badge changed">Δ${diff.changed_count} changed</span>` : ''}
  `;

  // Side-by-side headers
  $('#diffLeftHeader').textContent = stepAName;
  $('#diffRightHeader').textContent = stepBName;

  // Pretty-print JSON bodies
  let formattedA = bodyA;
  let formattedB = bodyB;
  try { formattedA = JSON.stringify(JSON.parse(bodyA), null, 2); } catch {}
  try { formattedB = JSON.stringify(JSON.parse(bodyB), null, 2); } catch {}

  const lang = detectBodyLang(formattedA);
  const highlightFn = lang === 'json' ? hlJSON : lang === 'xml' ? hlXML : lang === 'sql' ? hlSQL : lang === 'promql' ? hlPromQL : (t) => t;

  const leftCode = $('#diffLeftBody').querySelector('code');
  const rightCode = $('#diffRightBody').querySelector('code');
  const minimap = document.getElementById('diffMinimap') || (() => {
    const m = document.createElement('div');
    m.id = 'diffMinimap';
    m.className = 'diff-minimap';
    document.querySelector('.diff-side-by-side').appendChild(m);
    return m;
  })();
  minimap.innerHTML = '';
  minimap.onclick = null;

  // Show overlay + loading spinner immediately
  leftCode.innerHTML = '<div class="body-loading"><span class="spinner-sm"></span> Computing diff…</div>';
  rightCode.innerHTML = '';
  $('#diffViewerOverlay').classList.remove('hidden');

  // Check diff cache first
  const cacheKey = `assert:${fileIdx}-${blockIdx}`;
  const contentType = lang === 'json' ? 'json' : 'text';
  let diffOps = null;

  const cached = diffCache.get(cacheKey);
  if (cached) {
    diffOps = cached.ops;
    rpLog('debug', `Diff cache hit: ${cacheKey}`);
  } else {
    try {
      const rustDiff = await invoke('compute_diff', { textA: formattedA, textB: formattedB, contentType });
      diffOps = rustDiff.ops;
      // Cache the result
      const entrySize = JSON.stringify(rustDiff).length * 2;
      diffCache.set(cacheKey, rustDiff);
      diffCacheBytes += entrySize;
      rpLog('debug', `Diff cached: ${cacheKey} (${formatBytes(entrySize)})`);
    } catch {
      // Fallback to JS diff with performance guard
      const linesA = formattedA.split('\n');
      const linesB = formattedB.split('\n');
      const DIFF_LINE_LIMIT = 1500;
      const DIFF_BYTE_LIMIT = 131072;
      if (linesA.length > DIFF_LINE_LIMIT || linesB.length > DIFF_LINE_LIMIT ||
          formattedA.length > DIFF_BYTE_LIMIT || formattedB.length > DIFF_BYTE_LIMIT) {
        const warnMsg = `Response too large for JS fallback diff (${linesA.length}/${linesB.length} lines). Showing raw text.`;
        leftCode.innerHTML = `<div class="diff-line diff-same" style="color:var(--orange)"><span class="diff-text">${warnMsg}</span></div>` + `<div class="diff-line diff-same"><span class="diff-text">${highlightFn(escapeHtml(formattedA))}</span></div>`;
        rightCode.innerHTML = `<div class="diff-line diff-same" style="color:var(--orange)"><span class="diff-text">...</span></div>` + `<div class="diff-line diff-same"><span class="diff-text">${highlightFn(escapeHtml(formattedB))}</span></div>`;
        diffOps = null;
      } else {
        diffOps = computeLineDiff(formattedA, formattedB);
      }
    }
  }

  if (diffOps) {
    const hunks = buildDiffHunks(diffOps);
    let leftLineNum = 0, rightLineNum = 0;

    // Pre-count line numbers per op for collapsed sections
    const lineNums = [];
    let tmpL = 0, tmpR = 0;
    diffOps.forEach(op => {
      switch (op.type) {
        case 'same': tmpL++; tmpR++; break;
        case 'remove': tmpL++; break;
        case 'add': tmpR++; break;
        case 'change': tmpL++; tmpR++; break;
      }
      lineNums.push({ left: tmpL, right: tmpR });
    });

    const renderOp = (op) => {
      let leftLine = '', rightLine = '';
      switch (op.type) {
        case 'same': {
          leftLineNum++; rightLineNum++;
          const sameLine = highlightFn(escapeHtml(op.line || op.left || ''));
          leftLine = `<div class="diff-line diff-same"><span class="diff-ln">${leftLineNum}</span><span class="diff-text">${sameLine}</span></div>`;
          rightLine = `<div class="diff-line diff-same"><span class="diff-ln">${rightLineNum}</span><span class="diff-text">${sameLine}</span></div>`;
          break;
        }
        case 'remove': {
          leftLineNum++;
          leftLine = `<div class="diff-line diff-removed"><span class="diff-ln">${leftLineNum}</span><span class="diff-text">${highlightFn(escapeHtml(op.line || op.left || ''))}</span></div>`;
          rightLine = `<div class="diff-line diff-empty"><span class="diff-ln"></span><span class="diff-text"></span></div>`;
          break;
        }
        case 'add': {
          rightLineNum++;
          leftLine = `<div class="diff-line diff-empty"><span class="diff-ln"></span><span class="diff-text"></span></div>`;
          rightLine = `<div class="diff-line diff-added"><span class="diff-ln">${rightLineNum}</span><span class="diff-text">${highlightFn(escapeHtml(op.line || op.right || ''))}</span></div>`;
          break;
        }
        case 'change': {
          leftLineNum++; rightLineNum++;
          // Use Rust char highlights if available, else fall back to JS charDiffHighlight
          let leftCharHtml, rightCharHtml;
          if (op.left_highlights || op.right_highlights) {
            leftCharHtml = renderRustCharHighlight(op.left, op.left_highlights, 'diff-char-rm', highlightFn);
            rightCharHtml = renderRustCharHighlight(op.right, op.right_highlights, 'diff-char-add', highlightFn);
          } else {
            leftCharHtml = charDiffHighlight(op.left, op.right, 'left', highlightFn);
            rightCharHtml = charDiffHighlight(op.left, op.right, 'right', highlightFn);
          }
          leftLine = `<div class="diff-line diff-changed"><span class="diff-ln">${leftLineNum}</span><span class="diff-text">${leftCharHtml}</span></div>`;
          rightLine = `<div class="diff-line diff-changed"><span class="diff-ln">${rightLineNum}</span><span class="diff-text">${rightCharHtml}</span></div>`;
          break;
        }
      }
      return { leftLine, rightLine };
    };

    const HUNKS_PER_PAGE = 20;
    let renderedHunkIdx = 0;

    // Renders a batch of hunks, returns {leftHtml, rightHtml}
    const renderHunkBatch = (startIdx, count) => {
      let batchLeft = '', batchRight = '';
      const end = Math.min(startIdx + count, hunks.length);
      for (let hi = startIdx; hi < end; hi++) {
        const hunk = hunks[hi];
        if (hunk.type === 'collapse') {
          const collapseId = `diff-collapse-${hunk.startIdx}`;
          const divider = `<div class="diff-line diff-collapse" data-collapse-id="${collapseId}">▸ ${hunk.count} unchanged lines</div>`;
          batchLeft += divider;
          batchRight += divider;
          for (let i = hunk.startIdx; i <= hunk.endIdx; i++) {
            const op = diffOps[i];
            if (op.type === 'same') { leftLineNum++; rightLineNum++; }
            else if (op.type === 'remove') { leftLineNum++; }
            else if (op.type === 'add') { rightLineNum++; }
            else if (op.type === 'change') { leftLineNum++; rightLineNum++; }
          }
        } else {
          for (let i = hunk.startIdx; i <= hunk.endIdx; i++) {
            const { leftLine, rightLine } = renderOp(diffOps[i]);
            batchLeft += leftLine;
            batchRight += rightLine;
          }
        }
      }
      return { leftHtml: batchLeft, rightHtml: batchRight };
    };

    // Wire collapse expand handlers on newly added elements
    const wireCollapseHandlers = () => {
      leftCode.querySelectorAll('.diff-collapse:not([data-wired])').forEach(el => {
        el.dataset.wired = '1';
        el.addEventListener('click', () => {
          const cid = el.dataset.collapseId;
          const hunk = hunks.find(h => h.type === 'collapse' && `diff-collapse-${h.startIdx}` === cid);
          if (!hunk) return;
          let expandLeftHtml = '';
          let expandRightHtml = '';
          let eL = hunk.startIdx > 0 ? lineNums[hunk.startIdx - 1].left : 0;
          let eR = hunk.startIdx > 0 ? lineNums[hunk.startIdx - 1].right : 0;
          for (let i = hunk.startIdx; i <= hunk.endIdx; i++) {
            const op = diffOps[i];
            if (op.type === 'same') { eL++; eR++; }
            else if (op.type === 'remove') { eL++; }
            else if (op.type === 'add') { eR++; }
            else if (op.type === 'change') { eL++; eR++; }
            const sameLine = highlightFn(escapeHtml(op.line || op.left || ''));
            expandLeftHtml += `<div class="diff-line diff-same"><span class="diff-ln">${eL}</span><span class="diff-text">${sameLine}</span></div>`;
            expandRightHtml += `<div class="diff-line diff-same"><span class="diff-ln">${eR}</span><span class="diff-text">${sameLine}</span></div>`;
          }
          const tmpL = document.createElement('div');
          tmpL.innerHTML = expandLeftHtml;
          el.replaceWith(...tmpL.children);
          const rightEl = rightCode.querySelector(`[data-collapse-id="${cid}"]`);
          if (rightEl) {
            const tmpR = document.createElement('div');
            tmpR.innerHTML = expandRightHtml;
            rightEl.replaceWith(...tmpR.children);
          }
        });
      });
      rightCode.querySelectorAll('.diff-collapse:not([data-wired])').forEach(el => {
        el.dataset.wired = '1';
        el.addEventListener('click', () => {
          const cid = el.dataset.collapseId;
          const leftEl = leftCode.querySelector(`[data-collapse-id="${cid}"]`);
          if (leftEl) leftEl.click();
        });
      });
    };

    // Adds or updates the "Load more" bar with next-page + load-all options
    const updateLoadMore = () => {
      leftCode.querySelector('.diff-load-more-row')?.remove();
      rightCode.querySelector('.diff-load-more-row')?.remove();
      if (renderedHunkIdx < hunks.length) {
        const remaining = hunks.length - renderedHunkIdx;
        const remainingChanges = hunks.slice(renderedHunkIdx).filter(h => h.type === 'hunk').length;
        const nextBatch = Math.min(HUNKS_PER_PAGE, remaining);
        const loadMoreHtml = `<div class="diff-line diff-load-more-row">
          <div class="diff-text diff-load-more-bar">
            <button class="btn btn-primary btn-xs diff-load-next-btn">▾ Load next ${nextBatch} hunks</button>
            <button class="btn btn-ghost btn-xs diff-load-all-btn">Load all (${remaining} remaining)</button>
            <span class="diff-load-more-info">${remainingChanges} change regions left</span>
          </div>
          ${remaining > 50 ? '<div class="diff-text diff-load-warn">⚠ Loading all may be slow due to DOM rendering</div>' : ''}
        </div>`;
        leftCode.insertAdjacentHTML('beforeend', loadMoreHtml);
        rightCode.insertAdjacentHTML('beforeend', '<div class="diff-line diff-load-more-row"><span class="diff-text"></span></div>');
        leftCode.querySelector('.diff-load-next-btn').addEventListener('click', loadNextPage);
        leftCode.querySelector('.diff-load-all-btn').addEventListener('click', loadAllRemaining);
      }
    };

    const loadNextPage = () => {
      leftCode.querySelector('.diff-load-more-row')?.remove();
      rightCode.querySelector('.diff-load-more-row')?.remove();
      const batch = renderHunkBatch(renderedHunkIdx, HUNKS_PER_PAGE);
      renderedHunkIdx = Math.min(renderedHunkIdx + HUNKS_PER_PAGE, hunks.length);
      leftCode.insertAdjacentHTML('beforeend', batch.leftHtml);
      rightCode.insertAdjacentHTML('beforeend', batch.rightHtml);
      wireCollapseHandlers();
      updateLoadMore();
    };

    const loadAllRemaining = () => {
      leftCode.querySelector('.diff-load-more-row')?.remove();
      rightCode.querySelector('.diff-load-more-row')?.remove();
      const batch = renderHunkBatch(renderedHunkIdx, hunks.length - renderedHunkIdx);
      renderedHunkIdx = hunks.length;
      leftCode.insertAdjacentHTML('beforeend', batch.leftHtml);
      rightCode.insertAdjacentHTML('beforeend', batch.rightHtml);
      wireCollapseHandlers();
    };

    // Initial render — first page of hunks
    leftCode.innerHTML = '';
    rightCode.innerHTML = '';
    const initial = renderHunkBatch(0, HUNKS_PER_PAGE);
    renderedHunkIdx = Math.min(HUNKS_PER_PAGE, hunks.length);
    leftCode.innerHTML = initial.leftHtml;
    rightCode.innerHTML = initial.rightHtml;
    wireCollapseHandlers();
    updateLoadMore();

    // Minimap
    const totalLines = diffOps.length || 1;
    let marksHtml = '';
    diffOps.forEach((op, idx) => {
      if (op.type === 'same') return;
      const pct = (idx / totalLines) * 100;
      const cls = op.type === 'remove' ? 'mm-removed' : op.type === 'add' ? 'mm-added' : 'mm-changed';
      marksHtml += `<div class="mm-mark ${cls}" style="top:${pct}%"></div>`;
    });
    marksHtml += '<div class="mm-thumb" id="mmThumb"></div>';
    minimap.innerHTML = `<div class="diff-minimap-header"></div><div class="diff-minimap-track" id="mmTrack">${marksHtml}</div>`;

    const mmTrack = document.getElementById('mmTrack');
    mmTrack.onclick = (e) => {
      const rect = mmTrack.getBoundingClientRect();
      const pct = Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height));
      const leftPane = document.getElementById('diffLeftBody');
      const scrollTarget = pct * (leftPane.scrollHeight - leftPane.clientHeight);
      leftPane.scrollTop = scrollTarget;
    };

    const updateMmThumb = () => {
      const lp = document.getElementById('diffLeftBody');
      const thumb = document.getElementById('mmThumb');
      const track = document.getElementById('mmTrack');
      if (!lp || !thumb || !track) return;
      const trackH = track.clientHeight;
      const scrollRatio = lp.scrollHeight > lp.clientHeight
        ? lp.scrollTop / (lp.scrollHeight - lp.clientHeight) : 0;
      const viewRatio = lp.clientHeight / lp.scrollHeight;
      const thumbH = Math.max(8, viewRatio * trackH);
      const thumbTop = scrollRatio * (trackH - thumbH);
      thumb.style.height = thumbH + 'px';
      thumb.style.top = thumbTop + 'px';
    };
    const leftPaneRef = document.getElementById('diffLeftBody');
    leftPaneRef.addEventListener('scroll', updateMmThumb);
    requestAnimationFrame(updateMmThumb);
    requestAnimationFrame(updateMmThumb);
  }

  renderChangesOnly(diff);

  // Sync scroll between panes
  const leftPane = $('#diffLeftBody');
  const rightPane = $('#diffRightBody');
  leftPane.onscroll = () => { rightPane.scrollTop = leftPane.scrollTop; };
  rightPane.onscroll = () => { leftPane.scrollTop = rightPane.scrollTop; };

  // Reset tabs to side-by-side
  document.querySelectorAll('.diff-tab').forEach(t => t.classList.remove('active'));
  document.querySelector('.diff-tab[data-diff-tab="side-by-side"]')?.classList.add('active');
  $('#diffSideBySide').classList.remove('hidden');
  $('#diffChangesOnly').classList.add('hidden');

  $('#diffViewerOverlay').classList.remove('hidden');
}

function renderChangesOnly(diff) {
  const container = $('#diffChangesOnly');
  let html = '';

  if (diff.changed_paths && diff.changed_paths.length > 0) {
    html += '<div class="diff-section"><div class="diff-section-header">Changed Fields</div>';
    diff.changed_paths.forEach(cp => {
      html += `<div class="diff-change-row">
        <span class="diff-change-path">${escapeHtml(cp.path)}</span>
        <div class="diff-change-values">
          <span class="diff-change-left">${escapeHtml(String(cp.left ?? ''))}</span>
          <span class="diff-change-arrow">→</span>
          <span class="diff-change-right">${escapeHtml(String(cp.right ?? ''))}</span>
        </div>
      </div>`;
    });
    html += '</div>';
  }

  if (diff.added_paths && diff.added_paths.length > 0) {
    html += '<div class="diff-section"><div class="diff-section-header">Added Paths (only in Step B)</div>';
    diff.added_paths.forEach(p => {
      html += `<div class="diff-change-row added"><span class="diff-change-path">+ ${escapeHtml(p)}</span></div>`;
    });
    html += '</div>';
  }

  if (diff.removed_paths && diff.removed_paths.length > 0) {
    html += '<div class="diff-section"><div class="diff-section-header">Removed Paths (only in Step A)</div>';
    diff.removed_paths.forEach(p => {
      html += `<div class="diff-change-row removed"><span class="diff-change-path">- ${escapeHtml(p)}</span></div>`;
    });
    html += '</div>';
  }

  if (!html) html = '<div class="diff-empty">No differences found</div>';
  container.innerHTML = html;
}

function closeDiffViewer() {
  $('#diffViewerOverlay').classList.add('hidden');
  // Free DOM memory — clear large diff content
  const leftCode = $('#diffLeftBody')?.querySelector('code');
  const rightCode = $('#diffRightBody')?.querySelector('code');
  if (leftCode) leftCode.innerHTML = '';
  if (rightCode) rightCode.innerHTML = '';
  const minimap = document.getElementById('diffMinimap');
  if (minimap) minimap.innerHTML = '';
  const changesOnly = $('#diffChangesOnly');
  if (changesOnly) changesOnly.innerHTML = '';
}

$('#diffViewerClose').addEventListener('click', closeDiffViewer);

document.querySelector('.diff-viewer-tabs')?.addEventListener('click', (e) => {
  const tab = e.target.closest('.diff-tab');
  if (!tab) return;
  document.querySelectorAll('.diff-tab').forEach(t => t.classList.remove('active'));
  tab.classList.add('active');
  const mode = tab.dataset.diffTab;
  $('#diffSideBySide').classList.toggle('hidden', mode !== 'side-by-side');
  $('#diffChangesOnly').classList.toggle('hidden', mode !== 'changes');
});

// --- Keyboard Shortcuts ---
document.addEventListener('keydown', (e) => {
  // Ctrl+Enter -> Send (builder) / Run block at cursor (code mode)
  if (e.ctrlKey && !e.shiftKey && e.key === 'Enter') {
    e.preventDefault();
    if (currentMode === 'code') {
      runBlockAtCursor();
    } else {
      sendRequest();
    }
  }
  // Ctrl+Shift+Enter -> Run All
  if (e.ctrlKey && e.shiftKey && e.key === 'Enter') {
    e.preventDefault();
    runAllTests();
  }
  // Ctrl+L -> Focus URL
  if (e.ctrlKey && e.key === 'l') {
    e.preventDefault();
    urlInput.focus();
    urlInput.select();
  }
  // Ctrl+O -> Open file
  if (e.ctrlKey && e.key === 'o') {
    e.preventDefault();
    fileInput.click();
  }
  // Ctrl+S -> Save (code mode)
  if (e.ctrlKey && e.key === 's') {
    e.preventDefault();
    if (currentMode === 'code') {
      codeSaveBtn.click();
    }
  }
  // Zoom shortcuts
  if (e.ctrlKey && (e.key === '=' || e.key === '+')) { e.preventDefault(); zoomIn(); }
  else if (e.ctrlKey && e.key === '-') { e.preventDefault(); zoomOut(); }
  else if (e.ctrlKey && e.key === '0') { e.preventDefault(); zoomReset(); }
  // Escape -> close diff viewer, history detail, or leave history
  if (e.key === 'Escape') {
    if (!$('#diffViewerOverlay').classList.contains('hidden')) {
      closeDiffViewer();
    } else if (currentMode === 'history') {
      if (!historyDetailOverlay.classList.contains('hidden')) {
        closeHistoryDetail();
      } else {
        switchMode('builder');
      }
    }
  }
});

// --- Zoom controls ---
window.addEventListener('wheel', (e) => {
  if (e.ctrlKey) {
    e.preventDefault();
    if (e.deltaY < 0) zoomIn();
    else if (e.deltaY > 0) zoomOut();
  }
}, { passive: false });

applyZoom();

// --- Azure Auth ---
const azureAuthBtn = $('#azureAuthBtn');
const azureAuthIcon = $('#azureAuthIcon');
const azureAuthLabel = $('#azureAuthLabel');
const azureAuthStatus = $('#azureAuthStatus');

function updateAzureAuthButton() {
  if (!azureAuthBtn) return;
  
  // Remove all state classes
  azureAuthBtn.classList.remove('visible', 'needs-auth', 'authenticated', 'expired');
  
  if (azureAuthState === 'off') {
    return;
  }
  
  azureAuthBtn.classList.add('visible');
  
  if (azureAuthState === 'needs-auth') {
    azureAuthBtn.classList.add('needs-auth');
    azureAuthIcon.textContent = '⚠';
    azureAuthLabel.textContent = 'Azure: Sign In';
    azureAuthStatus.textContent = '';
    azureAuthBtn.title = 'Click to authenticate with Azure (device code flow)';
  } else if (azureAuthState === 'authenticated') {
    azureAuthBtn.classList.add('authenticated');
    azureAuthIcon.textContent = '✓';
    azureAuthLabel.textContent = 'Azure';
    const expiresIn = Math.min(...azureAuthScopes.map(s => {
      const c = azureTokenCache.get(s);
      return c ? Math.round((c.expiresAt - Date.now()) / 60000) : 0;
    }));
    azureAuthStatus.textContent = `${azureAuthScopes.length} scope${azureAuthScopes.length > 1 ? 's' : ''} · ${Math.max(0, expiresIn)}m`;
    // Build detailed tooltip with each scope
    const scopeLines = azureAuthScopes.map(s => {
      const c = azureTokenCache.get(s);
      const mins = c ? Math.max(0, Math.round((c.expiresAt - Date.now()) / 60000)) : 0;
      const short = s.replace('https://', '').replace('/.default', '');
      return `  ✓ ${short} (${mins}m)`;
    }).join('\n');
    azureAuthBtn.title = `Authenticated — ${azureAuthScopes.length} scope(s)\n${scopeLines}\n\nClick to re-authenticate.`;
  } else if (azureAuthState === 'expired') {
    azureAuthBtn.classList.add('expired');
    azureAuthIcon.textContent = '⟳';
    azureAuthLabel.textContent = 'Azure: Expired';
    azureAuthStatus.textContent = '';
    azureAuthBtn.title = 'Token expired — click to re-authenticate';
  }
  
  updateBlockModeDimming();
}

function setAzureAuthState(state) {
  azureAuthState = state;
  updateAzureAuthButton();
  // Update OTEL state based on Azure auth
  updateTelemetryAuthState();
}

async function initAzureAuth() {
  // Check Azure CLI availability
  try {
    azCliAvailable = await invoke('check_azure_cli');
    rpLog('info', `Azure CLI available: ${azCliAvailable}`);
  } catch (e) {
    azCliAvailable = false;
    rpLog('warn', `Azure CLI check failed: ${e}`);
  }
  
  // Check if any loaded file has dev_auth scopes
  detectAzureAuthNeeded();
  
  // Button click handler — opens auth dialog
  if (azureAuthBtn) {
    azureAuthBtn.addEventListener('click', handleAzureAuthClick);
  }
  
  // Periodically check token expiry
  setInterval(checkTokenExpiry, 30000);
}

function detectAzureAuthNeeded() {
  // Scan all loaded files for blocks with dev_auth scopes
  const scopes = new Set();
  for (const f of loadedFiles) {
    for (const block of f.suite.blocks) {
      if (block.dev_auth) scopes.add(block.dev_auth);
    }
    // Fallback: parse raw content for # @dev_auth directives (works before binary rebuild)
    if (f.content) {
      const matches = f.content.matchAll(/^#\s*@dev_auth\s+(.+)$/gm);
      for (const m of matches) {
        const scope = m[1].trim();
        if (scope) scopes.add(scope);
      }
    }
  }

  azureAuthScopes = [...scopes]; // Store needed scopes

  if (azureAuthScopes.length === 0) {
    setAzureAuthState('off');
    return;
  }
  
  // Check if we have valid tokens for ALL scopes
  const allCached = azureAuthScopes.every(scope => {
    const cached = azureTokenCache.get(scope);
    return cached && Date.now() < cached.expiresAt - 60000;
  });
  
  if (allCached) {
    setAzureAuthState('authenticated');
  } else if (azureTokenCache.size > 0) {
    // Some tokens exist but not all / some expired
    const anyValid = azureAuthScopes.some(scope => {
      const cached = azureTokenCache.get(scope);
      return cached && Date.now() < cached.expiresAt - 60000;
    });
    setAzureAuthState(anyValid ? 'expired' : 'needs-auth');
  } else {
    setAzureAuthState('needs-auth');
  }
}

function checkTokenExpiry() {
  if (azureAuthState !== 'authenticated') return;
  
  const anyExpired = azureAuthScopes.some(scope => {
    const cached = azureTokenCache.get(scope);
    return !cached || Date.now() >= cached.expiresAt - 60000;
  });
  
  if (anyExpired) {
    setAzureAuthState('expired');
    rpLog('warn', 'Azure token(s) expired');
    showToast('Azure token expired — click Azure button to re-authenticate', 'warn');
  } else {
    updateAzureAuthButton();
  }
}

async function handleAzureAuthClick() {
  if (azureAuthState === 'off') return;
  if (azureAuthBtn.classList.contains('loading')) return; // Prevent double-click
  
  if (azureAuthState === 'authenticated') {
    const ok = await showModalConfirm({
      title: 'Re-authenticate Azure?',
      message: 'You\'re already signed in. Re-authenticating will start the device-code flow again and refresh your tokens.',
      okLabel: 'Re-authenticate',
      cancelLabel: 'Stay signed in',
    });
    if (!ok) return;
  }
  
  // Show loading state
  azureAuthBtn.classList.add('loading');
  const prevIcon = azureAuthIcon.textContent;
  const prevLabel = azureAuthLabel.textContent;
  azureAuthIcon.textContent = '⏳';
  azureAuthLabel.textContent = 'Azure: Authenticating…';
  azureAuthBtn.disabled = true;
  
  // Resolve tenant and client from env vars → file variables (skip placeholders)
  let tenantId = 'organizations';
  let fileClientId = null;
  
  const resolveVar = (name) => {
    const v = lookupVar(name);
    if (v && !v.startsWith('your-')) return v;
    return null;
  };
  
  tenantId = resolveVar('tenant_id') || 'organizations';
  fileClientId = resolveVar('client_id');
  
  // Authenticate for each needed scope
  try {
    for (const scope of azureAuthScopes) {
      const cached = azureTokenCache.get(scope);
      if (cached && Date.now() < cached.expiresAt - 60000) continue;
      
      const resource = scope.replace(/\/.default$/, '');
      let token = null;
      
      azureAuthLabel.textContent = `Fetching: ${resource.split('/')[2] || resource}…`;
      
      // Strategy 1: az CLI (fastest — uses existing az login session)
      if (azCliAvailable) {
        try {
          rpLog('info', `[Strategy 1] az CLI token for: ${resource}`);
          const result = await invoke('fetch_azure_token', { resource });
          token = result.access_token;
          rpLog('info', `✓ az CLI token succeeded for: ${scope}`);
        } catch (e) {
          rpLog('warn', `✗ az CLI token failed for ${scope}: ${e}`);
        }
      } else {
        rpLog('info', '[Strategy 1] Skipped — az CLI not available');
      }
      
      // Strategy 2: Device code flow with file's client_id (enterprise-safe)
      if (!token && fileClientId) {
        try {
          rpLog('info', `Azure device code auth with app client_id for: ${scope}`);
          const fullScope = scope.endsWith('/.default') ? scope : scope + '/.default';
          token = await authenticateWithDeviceCode(tenantId, fileClientId, fullScope);
          rpLog('info', `Azure auth succeeded (device code, app client) for: ${scope}`);
        } catch (e) {
          rpLog('warn', `Device code with app client_id failed for ${scope}: ${e.message || e}`);
        }
      }
      
      // Strategy 3: Device code flow with Azure CLI public client (works for non-enterprise tenants)
      if (!token) {
        try {
          rpLog('info', `Azure device code auth with public client for: ${scope}`);
          const fullScope = scope.endsWith('/.default') ? scope : scope + '/.default';
          token = await authenticateWithDeviceCode(tenantId, '04b07795-a816-b338-ac5e-747c5dca11b3', fullScope);
          rpLog('info', `Azure auth succeeded (device code, public client) for: ${scope}`);
        } catch (e) {
          rpLog('error', `All auth strategies failed for ${scope}: ${e.message || e}`);
          showToast(`Azure auth failed for ${scope}. Run "az login" first, or ensure your app registration allows public client flows.`, 'error');
          detectAzureAuthNeeded();
          return;
        }
      }
      
      azureTokenCache.set(scope, { token, expiresAt: Date.now() + 3600000 });
    }
    
    setAzureAuthState('authenticated');
    showToast(`Azure: Authenticated for ${azureAuthScopes.length} scope(s)`, 'success');
  } finally {
    azureAuthBtn.classList.remove('loading');
    azureAuthBtn.disabled = false;
    updateAzureAuthButton();
  }
}

function updateBlockModeDimming() {
  const isAzureAuth = azureAuthState === 'authenticated' || azureAuthState === 'expired';
  document.querySelectorAll('.block-item[data-block-mode]').forEach(item => {
    const mode = item.dataset.blockMode;
    if (isAzureAuth) {
      item.classList.toggle('mode-dimmed', mode === 'app');
    } else {
      item.classList.toggle('mode-dimmed', mode === 'dev');
    }
  });
}

// Multi-scope Azure token cache (scope → { token, expiresAt })
const azureTokenCache = new Map();
let azureAuthScopes = []; // scopes needed by loaded files

function fetchDevModeToken(suite, fileContent) {
  if (azureAuthState !== 'authenticated') return [];
  
  // Collect all tokens for blocks with dev_auth
  const tokens = [];
  for (const block of (suite?.blocks || [])) {
    if (block.dev_auth) {
      const cached = azureTokenCache.get(block.dev_auth);
      if (cached && Date.now() < cached.expiresAt - 60000) {
        for (const ext of (block.extracts || [])) {
          tokens.push([ext.variable_name, cached.token]);
        }
      }
    }
  }
  
  // Fallback: parse raw content to match @@dev_auth (or legacy @dev_auth) scopes with @@extract variables
  if (tokens.length === 0 && fileContent) {
    const blocks = fileContent.split(/^###/m);
    for (const rawBlock of blocks) {
      const devAuthMatch = rawBlock.match(/^(?:#|\/\/)\s*@@?dev_auth\s+(.+)$/m);
      if (!devAuthMatch) continue;
      const scope = devAuthMatch[1].trim();
      const cached = azureTokenCache.get(scope);
      if (!cached || Date.now() >= cached.expiresAt - 60000) continue;
      const extractMatches = rawBlock.matchAll(/^(?:#|\/\/)\s*@@?extract\s+(\w+)\s*=\s*.+$/gm);
      for (const em of extractMatches) {
        tokens.push([em[1], cached.token]);
      }
    }
  }
  
  return tokens;
}

async function authenticateWithDeviceCode(tenantId, clientId, scope) {
  rpLog('info', 'Device code flow initiated', { tenantId, clientId, scope });
  // Step 1: Request device code
  const deviceCode = await invoke('start_device_code', {
    tenantId, clientId, scope
  });

  // Step 2: Show modal
  const overlay = $('#deviceCodeOverlay');
  const urlEl = $('#deviceCodeUrl');
  const codeEl = $('#deviceCodeValue');
  const statusEl = $('#deviceCodeStatus');
  const cancelBtn = $('#deviceCodeCancel');
  const copyBtn = $('#deviceCodeCopy');

  urlEl.href = deviceCode.verification_uri;
  urlEl.textContent = deviceCode.verification_uri;
  codeEl.textContent = deviceCode.user_code;
  statusEl.innerHTML = '<span class="device-code-spinner">⠋</span> Waiting for authentication...';
  statusEl.className = 'device-code-status';
  overlay.classList.remove('hidden');

  // Auto-open browser
  window.open(deviceCode.verification_uri, '_blank');

  // Copy button
  const copyHandler = () => {
    navigator.clipboard.writeText(deviceCode.user_code);
    copyBtn.textContent = '✓';
    setTimeout(() => copyBtn.textContent = '📋', 2000);
  };
  copyBtn.addEventListener('click', copyHandler);

  // Step 3: Poll for token
  let cancelled = false;
  const cancelHandler = () => { cancelled = true; };
  cancelBtn.addEventListener('click', cancelHandler);

  const interval = (deviceCode.interval || 5) * 1000;
  const maxAttempts = Math.ceil(deviceCode.expires_in / (deviceCode.interval || 5));

  try {
    for (let i = 0; i < maxAttempts && !cancelled; i++) {
      await new Promise(r => setTimeout(r, interval));
      if (cancelled) break;

      try {
        const token = await invoke('poll_device_code', {
          tenantId, clientId, deviceCode: deviceCode.device_code
        });
        // Success!
        statusEl.innerHTML = '✓ Authenticated successfully!';
        statusEl.className = 'device-code-status success';
        await new Promise(r => setTimeout(r, 1000));
        overlay.classList.add('hidden');
        showToast('Dev mode: Authenticated via browser', 'success');
        return token.access_token;
      } catch (e) {
        if (e === 'authorization_pending') continue;
        if (e === 'slow_down') {
          await new Promise(r => setTimeout(r, 5000));
          continue;
        }
        throw new Error(e);
      }
    }

    if (cancelled) {
      throw new Error('Authentication cancelled by user');
    }
    throw new Error('Device code expired — please try again');
  } finally {
    overlay.classList.add('hidden');
    cancelBtn.removeEventListener('click', cancelHandler);
    copyBtn.removeEventListener('click', copyHandler);
  }
}

// --- Log Viewer ---
let logAutoScroll = true;

function formatLogTime(isoStr) {
  const d = new Date(isoStr);
  return d.toLocaleTimeString('en-US', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' }) + '.' + String(d.getMilliseconds()).padStart(3, '0');
}

function createLogEntryEl(entry) {
  if (logFilterLevel !== 'all' && entry.level !== logFilterLevel) return null;
  const div = document.createElement('div');
  div.className = `log-entry log-${entry.level}`;
  div.innerHTML = `<span class="log-ts">${formatLogTime(entry.ts)}</span> <span class="log-level">${entry.level}</span> <span class="log-msg">${escapeHtml(entry.message)}</span>`;
  if (entry.data !== null) {
    const dataDiv = document.createElement('div');
    dataDiv.className = 'log-data';
    dataDiv.textContent = typeof entry.data === 'string' ? entry.data : JSON.stringify(entry.data, null, 2);
    dataDiv.style.display = 'none';
    div.style.cursor = 'pointer';
    div.addEventListener('click', () => {
      dataDiv.style.display = dataDiv.style.display === 'none' ? 'block' : 'none';
    });
    const wrapper = document.createDocumentFragment();
    wrapper.appendChild(div);
    wrapper.appendChild(dataDiv);
    return wrapper;
  }
  return div;
}

function renderAllLogs() {
  if (!logList) return;
  logList.innerHTML = '';
  rpLogs.forEach(entry => {
    const el = createLogEntryEl(entry);
    if (el) logList.appendChild(el);
  });
  if (logAutoScroll) logList.scrollTop = logList.scrollHeight;
}

function renderLogEntry(entry) {
  if (currentMode !== 'logs' || !logList) return;
  const el = createLogEntryEl(entry);
  if (el) {
    logList.appendChild(el);
    if (logAutoScroll) logList.scrollTop = logList.scrollHeight;
  }
}

// Auto-scroll detection
if (logList) {
  logList.addEventListener('scroll', () => {
    const threshold = 50;
    logAutoScroll = (logList.scrollHeight - logList.scrollTop - logList.clientHeight) < threshold;
  });
}

// Log filter buttons
document.querySelectorAll('.log-filter-btn').forEach(btn => {
  btn.addEventListener('click', () => {
    document.querySelectorAll('.log-filter-btn').forEach(b => b.classList.remove('active'));
    btn.classList.add('active');
    logFilterLevel = btn.dataset.logLevel;
    renderAllLogs();
  });
});

// Log clear button
const logsClearBtn = $('#logsClearBtn');
if (logsClearBtn) {
  logsClearBtn.addEventListener('click', () => {
    rpLogs.length = 0;
    if (logList) logList.innerHTML = '';
  });
}

// --- Pop-Out Windows (Tauri native) ---
const popoutWindows = new Map(); // panelId → { placeholder }

async function popOutPanel(panelId, title) {
  if (popoutWindows.has(panelId)) {
    // Already popped out — just focus it
    try {
      const { WebviewWindow } = window.__TAURI__.webviewWindow;
      const existing = await WebviewWindow.getByLabel(`popout-${panelId}`);
      if (existing) await existing.setFocus();
    } catch (e) { console.warn('popout focus failed:', e); }
    return;
  }

  const sourceEl = document.getElementById(panelId);
  if (!sourceEl) return;

  // Snapshot the HTML content before hiding
  const htmlSnapshot = sourceEl.innerHTML;

  // Create a placeholder in the main window
  const placeholder = document.createElement('div');
  placeholder.className = 'popout-placeholder';
  placeholder.innerHTML = `<span class="popout-notice">📌 ${title} — popped out to separate window</span><button class="btn btn-sm" onclick="popInPanel('${panelId}')">Pop Back In</button>`;
  sourceEl.parentNode.insertBefore(placeholder, sourceEl);
  sourceEl.style.display = 'none';

  // Store state
  popoutWindows.set(panelId, { placeholder, sourceEl });

  try {
    const { WebviewWindow } = window.__TAURI__.webviewWindow;
    const webview = new WebviewWindow(`popout-${panelId}`, {
      url: `popout.html?panel=${panelId}`,
      title: `${title} — Request Pilot`,
      width: 800,
      height: 600,
      resizable: true,
      decorations: true,
      center: true,
    });

    // When the popout is ready, send content
    const unlistenReady = await listen('popout-ready', async (event) => {
      if (event.payload.panelId === panelId) {
        await emit('popout-content', { panelId, html: htmlSnapshot });
        unlistenReady();
      }
    });

    // When popout window is closed
    const unlistenClose = await listen('popout-closed', (event) => {
      if (event.payload.panelId === panelId) {
        restorePanel(panelId);
        unlistenClose();
      }
    });

    // Also detect via Tauri window destroy event
    webview.once('tauri://destroyed', () => {
      restorePanel(panelId);
    });

    rpLog('info', `Panel popped out: ${title}`);
  } catch (e) {
    // If Tauri window creation fails, restore
    placeholder.remove();
    sourceEl.style.display = '';
    popoutWindows.delete(panelId);
    rpLog('error', `Pop-out failed: ${e.message || e}`);
    showToast(`Pop-out failed: ${e.message || e}`, 'error');
  }
}

function restorePanel(panelId) {
  const state = popoutWindows.get(panelId);
  if (!state) return;
  state.sourceEl.style.display = '';
  if (state.placeholder.parentNode) state.placeholder.remove();
  popoutWindows.delete(panelId);
  rpLog('info', `Panel popped back in: ${panelId}`);
}

async function popInPanel(panelId) {
  try {
    const { WebviewWindow } = window.__TAURI__.webviewWindow;
    const win = await WebviewWindow.getByLabel(`popout-${panelId}`);
    if (win) await win.close();
  } catch (e) { console.warn('popout close failed:', e); }
  restorePanel(panelId);
}

// Send live log entries to popout window if open
function emitToLogPopout(entry) {
  if (popoutWindows.has('logsPanel')) {
    emit('popout-log-entry', entry).catch(() => {});
  }
}

// ── OTEL Telemetry ──

function updateTelemetryAuthState() {
  // Only relevant if telemetry is configured but not yet running/completed
  if (telemetryState === 'off' || telemetryState === 'sending' || 
      telemetryState === 'active' || telemetryState === 'error') return;
  
  // Check if Azure auth has monitor scope token
  const monitorCached = azureTokenCache.get('https://monitor.azure.com/.default');
  const hasMonitorToken = monitorCached && Date.now() < monitorCached.expiresAt - 60000;
  
  if (hasMonitorToken && (telemetryState === 'configured' || telemetryState === 'ready')) {
    setTelemetryState('ready');
    rpLog('info', 'OTEL: Ready to send — monitor.azure.com token acquired');
  } else if (!hasMonitorToken && telemetryState === 'ready') {
    setTelemetryState('configured');
  }
}

function detectTelemetryConfig() {
  let hasTelemetry = false;
  let telemetryFiles = [];
  for (const f of loadedFiles) {
    const vars = f.suite.variables || [];
    const hasEndpoint = vars.some(([k, v]) =>
      (k === 'telemetry_traces_endpoint' || k === 'telemetry_metrics_endpoint' || k === 'telemetry_logs_endpoint') && v
    );
    if (hasEndpoint) {
      hasTelemetry = true;
      telemetryFiles.push(f.name);
    }
  }
  const btn = $('#telemetryBtn');
  if (hasTelemetry) {
    btn.classList.add('visible');
    if (telemetryState === 'off') {
      setTelemetryState('configured');
      rpLog('info', `OTEL telemetry configured`, { files: telemetryFiles });
      updateTelemetryAuthState();
    }
  } else {
    btn.classList.remove('visible');
    setTelemetryState('off');
  }
}

function setTelemetryState(state) {
  telemetryState = state;
  const btn = $('#telemetryBtn');
  const icon = $('#telemetryIcon');
  const label = $('#telemetryLabel');
  btn.classList.remove('active', 'sending', 'error', 'ready');

  switch (state) {
    case 'off':
      icon.textContent = '📡';
      label.textContent = 'OTEL';
      break;
    case 'configured':
      btn.classList.add('visible');
      icon.textContent = '📡';
      label.textContent = 'OTEL';
      break;
    case 'ready':
      btn.classList.add('visible', 'ready');
      icon.textContent = '📡';
      label.textContent = 'OTEL ✓';
      break;
    case 'sending':
      btn.classList.add('visible', 'sending');
      icon.textContent = '📡';
      label.textContent = 'Sending…';
      break;
    case 'active':
      btn.classList.add('visible', 'active');
      icon.textContent = '📡';
      label.textContent = 'OTEL ✓';
      break;
    case 'error':
      btn.classList.add('visible', 'error');
      icon.textContent = '📡';
      label.textContent = 'OTEL ✗';
      break;
  }
  renderTelemetryTooltip();
}

function processTelemetryResult(fileName, results) {
  if (!results.telemetry) {
    rpLog('debug', `No telemetry data returned for ${fileName} (telemetry not configured or init failed)`);
    return;
  }
  const t = results.telemetry;
  const entry = {
    file: fileName,
    endpoint: t.endpoint || '',
    traces: t.traces_sent || 0,
    metrics: t.metrics_sent || 0,
    logs: t.logs_sent || 0,
    errors: t.errors || [],
    time_ms: t.export_time_ms || 0,
  };
  telemetryStats.push(entry);

  const hasErrors = entry.errors.length > 0;
  if (hasErrors) {
    setTelemetryState('error');
    for (const err of entry.errors) {
      rpLog('error', `OTEL export error [${fileName}]: ${err}`);
    }
  } else {
    setTelemetryState('active');
    rpLog('info', `OTEL export succeeded [${fileName}]: ${entry.traces} traces, ${entry.metrics} metrics, ${entry.logs} logs (${entry.time_ms}ms)`);
  }
  renderTelemetryTooltip();
}

function renderTelemetryTooltip() {
  const tooltip = $('#telemetryTooltip');
  if (!tooltip) return;

  const checked = telemetryEnabled ? 'checked' : '';
  const dimClass = telemetryEnabled ? '' : ' style="opacity: 0.5;"';
  let html = '<div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:8px;">'
    + '<h4 style="margin:0;">📡 OTEL Telemetry</h4>'
    + '<label class="tt-toggle" title="Enable/disable telemetry export"><input type="checkbox" id="telemetryToggle" ' + checked + '><span class="tt-toggle-slider"></span></label>'
    + '</div>';

  html += '<div' + dimClass + '>';

  // Show state-specific info
  if (!telemetryEnabled) {
    html += '<div class="tt-row"><span class="tt-key">Status</span><span class="tt-val" style="color:var(--text-muted);">Paused — toggle to resume</span></div>';
  } else if (telemetryState === 'configured') {
    const hasAzureAuth = azureAuthState === 'authenticated';
    const statusText = hasAzureAuth ? 'Configured — ready to export' : 'Configured — awaiting test run';
    html += '<div class="tt-row"><span class="tt-key">Status</span><span class="tt-val">' + statusText + '</span></div>';
    // Show which files have telemetry
    const telFiles = loadedFiles.filter(f => {
      const vars = f.suite.variables || [];
      return vars.some(([k, v]) => (k === 'telemetry_traces_endpoint' || k === 'telemetry_metrics_endpoint' || k === 'telemetry_logs_endpoint') && v);
    });
    if (telFiles.length > 0) {
      html += '<div class="tt-section"><div class="tt-section-title">Configured Files</div>';
      for (const f of telFiles) {
        const vars = f.suite.variables || [];
        const trEndpoint = vars.find(([k]) => k === 'telemetry_traces_endpoint')?.[1] || '';
        const display = trEndpoint.length > 50 ? trEndpoint.slice(0, 40) + '…' : (trEndpoint || '(endpoints configured)');
        html += '<div class="tt-row"><span class="tt-key">' + escHtml(f.name) + '</span><span class="tt-val" title="' + escHtml(trEndpoint) + '">' + escHtml(display) + '</span></div>';
      }
      html += '</div>';
    }
    html += '<div class="tt-section"><div class="tt-section-title">Next Step</div><div style="color: var(--text-muted); font-size: 11px;">Run tests — telemetry will be exported automatically</div></div>';
  } else if (telemetryState === 'ready') {
    html += '<div class="tt-row"><span class="tt-key">Status</span><span class="tt-val success">Ready — authenticated</span></div>';
    const telFiles = loadedFiles.filter(f => {
      const vars = f.suite.variables || [];
      return vars.some(([k, v]) => (k === 'telemetry_traces_endpoint' || k === 'telemetry_metrics_endpoint' || k === 'telemetry_logs_endpoint') && v);
    });
    if (telFiles.length > 0) {
      html += '<div class="tt-section"><div class="tt-section-title">Files</div>';
      for (const f of telFiles) {
        html += '<div class="tt-row"><span class="tt-key">' + escHtml(f.name) + '</span><span class="tt-val success">✓ Ready</span></div>';
      }
      html += '</div>';
    }
    html += '<div class="tt-section"><div class="tt-section-title">Next Step</div><div style="color: var(--text-muted); font-size: 11px;">Run tests — telemetry will be exported automatically</div></div>';
    html += '<div class="tt-section"><div class="tt-section-title">⚠ RBAC Required</div><div style="color: var(--text-muted); font-size: 11px;">Your identity needs <b>Monitoring Metrics Publisher</b> role on the Data Collection Rule (DCR) referenced by the App Insights resource to ingest telemetry.</div></div>';
  } else if (telemetryState === 'sending') {
    html += '<div class="tt-row"><span class="tt-key">Status</span><span class="tt-val" style="color: #00bcd4;">Exporting telemetry…</span></div>';
    if (telemetryStats.length > 0) {
      // Show progress so far
      let totalTraces = 0, totalMetrics = 0, totalLogs = 0;
      for (const s of telemetryStats) {
        totalTraces += s.traces;
        totalMetrics += s.metrics;
        totalLogs += s.logs;
      }
      html += '<div class="tt-row"><span class="tt-key">Traces sent</span><span class="tt-val success">' + totalTraces + '</span></div>';
      html += '<div class="tt-row"><span class="tt-key">Metrics sent</span><span class="tt-val success">' + totalMetrics + '</span></div>';
      html += '<div class="tt-row"><span class="tt-key">Logs sent</span><span class="tt-val success">' + totalLogs + '</span></div>';
      html += '<div class="tt-row"><span class="tt-key">Files completed</span><span class="tt-val">' + telemetryStats.length + '</span></div>';
    } else {
      html += '<div class="tt-row"><span class="tt-key">Progress</span><span class="tt-val">Waiting for first file to complete…</span></div>';
    }
  } else if (telemetryState === 'active' || telemetryState === 'error') {
    // Show final stats
    if (telemetryStats.length === 0) {
      html += '<div class="tt-row"><span class="tt-key">No data exported</span></div>';
    } else {
      let totalTraces = 0, totalMetrics = 0, totalLogs = 0, totalErrors = 0, totalTime = 0;
      const allErrors = [];
      for (const s of telemetryStats) {
        totalTraces += s.traces;
        totalMetrics += s.metrics;
        totalLogs += s.logs;
        totalErrors += s.errors.length;
        totalTime += s.time_ms;
        allErrors.push(...s.errors);
      }
      const endpoint = telemetryStats[0]?.endpoint || 'N/A';
      const maskedEndpoint = endpoint.length > 40 ? endpoint.slice(0, 30) + '…' + endpoint.slice(-10) : endpoint;

      html += '<div class="tt-row"><span class="tt-key">Endpoint</span><span class="tt-val" title="' + escHtml(endpoint) + '">' + escHtml(maskedEndpoint) + '</span></div>';
      html += '<div class="tt-row"><span class="tt-key">Traces</span><span class="tt-val success">' + totalTraces + '</span></div>';
      html += '<div class="tt-row"><span class="tt-key">Metrics</span><span class="tt-val success">' + totalMetrics + '</span></div>';
      html += '<div class="tt-row"><span class="tt-key">Logs</span><span class="tt-val success">' + totalLogs + '</span></div>';
      html += '<div class="tt-row"><span class="tt-key">Export time</span><span class="tt-val">' + totalTime + 'ms</span></div>';

      if (telemetryStats.length > 1) {
        html += '<div class="tt-section"><div class="tt-section-title">Per File</div>';
        for (const s of telemetryStats) {
          const status = s.errors.length > 0 ? '✗' : '✓';
          const cls = s.errors.length > 0 ? 'error' : 'success';
          html += '<div class="tt-row"><span class="tt-key">' + escHtml(s.file) + '</span><span class="tt-val ' + cls + '">' + status + ' ' + s.traces + 'T ' + s.metrics + 'M ' + s.logs + 'L</span></div>';
        }
        html += '</div>';
      }

      if (allErrors.length > 0) {
        html += '<div class="tt-section"><div class="tt-section-title">Errors (' + allErrors.length + ')</div><div class="tt-error-list">';
        for (const e of allErrors.slice(0, 5)) {
          html += '<div>' + escHtml(e) + '</div>';
        }
        if (allErrors.length > 5) html += '<div>… and ' + (allErrors.length - 5) + ' more</div>';
        html += '</div></div>';
        const has403 = allErrors.some(e => /403|Forbidden|Unauthorized/i.test(e));
        if (has403) {
          html += '<div class="tt-section"><div class="tt-section-title">💡 Fix</div><div style="color: var(--text-muted); font-size: 11px;">Assign <b>Monitoring Metrics Publisher</b> role to your identity on the Data Collection Rule (DCR) associated with the App Insights resource.</div></div>';
        }
      }
    }
  } else {
    html += '<div class="tt-row"><span class="tt-key">Status</span><span class="tt-val">Not configured</span></div>';
  }
  html += '</div>'; // close dimClass wrapper

  tooltip.innerHTML = html;

  // Bind toggle event
  const toggle = document.getElementById('telemetryToggle');
  if (toggle) {
    toggle.addEventListener('change', () => {
      telemetryEnabled = toggle.checked;
      rpLog('info', `OTEL telemetry ${telemetryEnabled ? 'enabled' : 'disabled'} by user`);
      renderTelemetryTooltip();
      // Update button visual
      const btn = $('#telemetryBtn');
      if (!telemetryEnabled) {
        btn.classList.add('paused');
      } else {
        btn.classList.remove('paused');
      }
    });
  }
}

function escHtml(s) {
  return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

// --- Initialize ---
renderEnvPanel();
initAzureAuth();
reloadEnvList();

// Telemetry tooltip: stay open when hovering from button into tooltip
{
  const btn = $('#telemetryBtn');
  const tooltip = $('#telemetryTooltip');
  let hideTimer = null;
  const show = () => { clearTimeout(hideTimer); btn.classList.add('tooltip-visible'); };
  const scheduleHide = () => { hideTimer = setTimeout(() => btn.classList.remove('tooltip-visible'), 350); };
  if (btn && tooltip) {
    btn.addEventListener('mouseenter', show);
    btn.addEventListener('mouseleave', scheduleHide);
    tooltip.addEventListener('mouseenter', show);
    tooltip.addEventListener('mouseleave', scheduleHide);
  }
}

rpLog('info', 'Request Pilot initialized');

// --- Theme toggle ---
document.getElementById('themeToggle').addEventListener('click', toggleTheme);
window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (e) => {
  if (!localStorage.getItem('rp-theme')) {
    document.documentElement.setAttribute('data-theme', e.matches ? 'dark' : 'light');
    updateThemeIcon();
  }
});

// --- Live Capture ---

// Toggle dropdown
document.getElementById('liveCaptureToggle').addEventListener('click', (e) => {
  e.stopPropagation();
  const dd = document.getElementById('liveCaptureDropdown');
  dd.style.display = dd.style.display === 'none' ? 'block' : 'none';
});
// Close on outside click
document.addEventListener('click', (e) => {
  const wrapper = document.querySelector('.live-capture-wrapper');
  if (wrapper && !wrapper.contains(e.target)) {
    document.getElementById('liveCaptureDropdown').style.display = 'none';
  }
});

// Mode switching
document.querySelectorAll('input[name="liveMode"]').forEach(radio => {
  radio.addEventListener('change', async (e) => {
    const mode = e.target.value;
    liveCaptureMode = mode;

    try {
      // Always ensure server is running (no-op if already running)
      if (mode !== 'off') {
        await invoke('start_live_capture');
      }
      await invoke('set_live_capture_mode', { mode });

      if (mode !== 'off' && !liveCaptureSessionId) {
        startNewCaptureSession();
      }
      document.getElementById('liveNewSessionBtn').disabled = (mode === 'off');
    } catch (err) {
      rpLog('error', 'Live capture mode switch failed', String(err));
    }

    updateLiveCaptureUI();
  });
});

// Listen for live requests from extension
(async () => {
  await listen('live-request', (event) => {
    const req = event.payload;
    liveCapturedRequests.push(req);

    document.getElementById('liveCaptureCount').textContent = liveCapturedRequests.length;
    document.getElementById('liveSaveBtn').disabled = liveCapturedRequests.length === 0;

    addLiveRequestToHistory(req);
    appendToLiveCaptureFile(req);

    rpLog('debug', `Live capture: ${req.method} ${req.url}`, { status: req.status_code, duration: req.duration });
  });

  // Listen for connection status changes
  await listen('live-connection-status', (event) => {
    const { status } = event.payload;
    const dot = document.getElementById('liveDot');
    const statusText = document.getElementById('liveStatusText');

    // If mode is off, keep showing "Off" regardless of connection events
    if (liveCaptureMode === 'off') {
      dot.className = 'live-dot';
      statusText.className = 'live-status';
      statusText.textContent = 'Off';
      liveCaptureConnected = (status === 'connected');
      return;
    }

    dot.className = 'live-dot';
    statusText.className = 'live-status';

    switch (status) {
      case 'listening':
        dot.classList.add('listening');
        statusText.classList.add('listening');
        statusText.textContent = 'Waiting for extension…';
        liveCaptureConnected = false;
        break;
      case 'connected':
        dot.classList.add('connected');
        statusText.classList.add('connected');
        statusText.textContent = 'Connected';
        liveCaptureConnected = true;
        break;
      case 'disconnected':
        dot.classList.add('error');
        statusText.classList.add('error');
        statusText.textContent = 'Disconnected';
        liveCaptureConnected = false;
        break;
      case 'stopped':
        statusText.textContent = 'Off';
        liveCaptureConnected = false;
        break;
    }

    rpLog('info', `Live capture status: ${status}`);
  });

  // Listen for parse errors from the WS server (diagnostic)
  await listen('live-capture-error', (event) => {
    const { error, preview } = event.payload;
    rpLog('error', `Live capture parse error: ${error}`, { preview });
  });
})();

// Add captured request to history and persist to Rust HistoryStore
function addLiveRequestToHistory(req) {
  let blockName;
  try {
    const u = new URL(req.url);
    blockName = `${req.method} ${u.origin}${u.pathname}`;
  } catch {
    blockName = `${req.method} request`;
  }

  const sessionTime = (liveCaptureSessionId
    ? new Date(liveCaptureSessionId).toLocaleTimeString('en-US', { hour12: false })
    : new Date().toLocaleTimeString('en-US', { hour12: false })).replace(/:/g, '-');

  const entry = {
    seq: 0, // assigned by Rust
    id: crypto.randomUUID ? crypto.randomUUID() : Math.random().toString(36).slice(2),
    run_id: String(liveCaptureSessionId),
    source: 'extension-live',
    file_name: `Live Capture - ${sessionTime}`,
    group: null,
    block_name: blockName,
    method: req.method || 'GET',
    url: req.url,
    request_headers: (req.request_headers || []).map(h => [h.name, h.value]),
    request_body: req.request_body || null,
    status: req.status_code || 0,
    response_headers: (req.response_headers || []).map(h => [h.name, h.value]),
    response_body: req.response_body || null,
    response_time_ms: req.duration || 0,
    response_size_bytes: req.response_body ? req.response_body.length : 0,
    timestamp: req.timestamp ? new Date(req.timestamp).toISOString() : new Date().toISOString(),
  };

  // Persist to Rust HistoryStore
  invoke('add_history_entry', { entry }).then(seq => {
    entry.seq = seq;
  }).catch(err => {
    rpLog('warn', 'Failed to persist live capture to history', String(err));
  });

  historyCache.unshift(entry);

  if (currentMode === 'history') {
    loadHistory();
  }
}

// Virtual .http file session management
function startNewCaptureSession() {
  liveCaptureSessionId = Date.now();
  liveCapturedRequests = [];
  document.getElementById('liveCaptureCount').textContent = '0';
  document.getElementById('liveSaveBtn').disabled = true;

  const sessionTime = new Date().toLocaleTimeString('en-US', { hour12: false }).replace(/:/g, '-');
  const fileName = `Live Capture - ${sessionTime}.http`;
  const initialContent = `# Live Capture Session\n# Started: ${new Date().toISOString()}\n# Source: Request Pilot Extension\n\n`;

  try {
    const suite = { variables: [], blocks: [] };
    loadedFiles.push({
      name: fileName,
      content: initialContent,
      suite,
      results: null,
      savedPath: null,
    });
    liveCaptureFileIndex = loadedFiles.length - 1;
    activeFileIndex = liveCaptureFileIndex;
    activeBlockIndex = -1;
    renderFileTree();
    // Switch to builder mode so user sees the file
    if (currentMode !== 'builder' && currentMode !== 'code') {
      switchMode('builder');
    }
    rpLog('info', `Live capture session started: ${fileName}`);
  } catch (e) {
    rpLog('warn', 'Failed to create live capture file', String(e));
  }
}

function appendToLiveCaptureFile(req) {
  if (liveCaptureFileIndex < 0 || liveCaptureFileIndex >= loadedFiles.length) return;

  const file = loadedFiles[liveCaptureFileIndex];

  // Build descriptive block name from host + pathname
  let label;
  try {
    const u = new URL(req.url);
    label = `${req.method} ${u.hostname}${u.pathname}`;
  } catch {
    label = `${req.method} request`;
  }

  let block = `###\n`;
  block += `@name ${label}\n`;
  block += `${req.method} ${req.url}\n`;

  // Only include meaningful headers, skip browser-internal and sensitive ones
  const skipPrefixes = [':', 'sec-ch-', 'sec-fetch-'];
  const skipNames = new Set([
    'host', 'connection', 'accept-encoding', 'accept-language',
    'upgrade-insecure-requests', 'priority', 'pragma', 'cache-control',
    'user-agent', 'dnt', 'origin', 'referer',
    // Sensitive — redact by default to avoid persisting credentials to disk
    'authorization', 'cookie', 'set-cookie', 'proxy-authorization',
    'x-api-key', 'x-auth-token',
  ]);

  for (const h of (req.request_headers || [])) {
    const lower = h.name.toLowerCase();
    if (skipNames.has(lower)) continue;
    if (skipPrefixes.some(p => lower.startsWith(p))) continue;
    block += `${h.name}: ${h.value}\n`;
  }

  if (req.request_body) {
    block += `\n${req.request_body}\n`;
  }

  block += '\n';

  file.content += block;

  // Re-parse to update sidebar blocks and code editor
  invoke('parse_test_file', { content: file.content }).then(suite => {
    file.suite = suite;
    if (activeFileIndex === liveCaptureFileIndex) {
      renderFileTree();
      // Update code editor if in code mode
      if (currentMode === 'code') {
        const editor = document.getElementById('codeEditor');
        if (editor) {
          editor.value = file.content;
          codeEditorContent = file.content;
        }
      }
    }
  }).catch(() => {});
}

// Save captured .http file via native save dialog
document.getElementById('liveSaveBtn').addEventListener('click', async () => {
  if (liveCaptureFileIndex < 0 || liveCaptureFileIndex >= loadedFiles.length) return;

  const file = loadedFiles[liveCaptureFileIndex];
  try {
    const path = await invoke('save_file_with_dialog', {
      defaultName: file.name,
      content: file.content,
      title: 'Save Live Capture',
      filters: [['HTTP Files', 'http']],
    });
    if (!path) return; // cancelled
    showToast(`Saved to ${path.split(/[\\/]/).pop()}`, 'success');
    rpLog('info', `Live capture file saved: ${path}`);
  } catch (err) {
    showToast('Save failed: ' + err, 'error');
    rpLog('error', 'Live capture save failed', String(err));
  }
});

// New capture session
document.getElementById('liveNewSessionBtn').addEventListener('click', () => {
  startNewCaptureSession();
});

function updateLiveCaptureUI() {
  const dot = document.getElementById('liveDot');
  const statusText = document.getElementById('liveStatusText');

  dot.className = 'live-dot';
  statusText.className = 'live-status';

  if (liveCaptureMode === 'off') {
    statusText.textContent = 'Off';
  } else if (liveCaptureConnected) {
    dot.classList.add('connected');
    statusText.classList.add('connected');
    statusText.textContent = 'Connected';
  } else {
    dot.classList.add('listening');
    statusText.classList.add('listening');
    statusText.textContent = 'Waiting for extension…';
  }
}

// Restore live capture status on startup
(async function initLiveCapture() {
  try {
    const status = await invoke('get_live_capture_status');
    if (status && status.mode && status.mode !== 'off') {
      liveCaptureMode = status.mode;
      const radio = document.querySelector(`input[name="liveMode"][value="${status.mode}"]`);
      if (radio) radio.checked = true;
      if (!liveCaptureSessionId) startNewCaptureSession();
      document.getElementById('liveNewSessionBtn').disabled = false;
    }
    if (status && status.connected) {
      liveCaptureConnected = true;
      document.getElementById('liveDot').className = 'live-dot connected';
      document.getElementById('liveStatusText').textContent = 'Connected';
      document.getElementById('liveStatusText').className = 'live-status connected';
    } else if (status && status.running) {
      document.getElementById('liveDot').className = 'live-dot listening';
      document.getElementById('liveStatusText').textContent = 'Waiting for extension…';
      document.getElementById('liveStatusText').className = 'live-status listening';
    }
  } catch {
    // Backend may not support live capture yet — silently ignore
  }
})();

/* ============================================================
   VirtualScroll — High-performance virtual scrolling component
   Only renders visible DOM nodes + a configurable buffer.
   ============================================================ */
class VirtualScroll {
  /**
   * @param {HTMLElement} container - The scrollable container element
   * @param {Object} options
   * @param {number}   [options.rowHeight=20]    - Estimated row height in px
   * @param {number}   [options.bufferRows=20]   - Extra rows rendered above/below viewport
   * @param {Function} options.renderRow         - (index, data) => HTMLElement
   * @param {Function} [options.onRangeChange]   - (startIdx, endIdx) => void
   */
  constructor(container, options = {}) {
    this._container = container;
    this._rowHeight = options.rowHeight || 20;
    this._bufferRows = options.bufferRows != null ? options.bufferRows : 20;
    this._renderRow = options.renderRow;
    this._onRangeChange = options.onRangeChange || null;
    this._items = [];
    this._renderedStart = -1;
    this._renderedEnd = -1;
    this._nodePool = [];       // recycled DOM nodes
    this._activeNodes = [];    // currently displayed DOM nodes
    this._rafId = null;
    this._destroyed = false;

    // Build DOM structure
    this._container.classList.add('virtual-scroll-container');
    this._spacer = document.createElement('div');
    this._spacer.className = 'virtual-scroll-spacer';
    this._viewport = document.createElement('div');
    this._viewport.className = 'virtual-scroll-viewport';
    this._container.appendChild(this._spacer);
    this._container.appendChild(this._viewport);

    // Bind handlers
    this._onScroll = this._onScroll.bind(this);
    this._onResize = this._onResize.bind(this);
    this._container.addEventListener('scroll', this._onScroll, { passive: true });

    this._resizeObserver = new ResizeObserver(this._onResize);
    this._resizeObserver.observe(this._container);
  }

  /* --- Public API --- */

  setData(items) {
    this._items = items || [];
    this._spacer.style.height = (this._items.length * this._rowHeight) + 'px';
    this._renderedStart = -1;
    this._renderedEnd = -1;
    this._recycleAll();
    this._render();
  }

  updateItem(index, newData) {
    if (index < 0 || index >= this._items.length) return;
    this._items[index] = newData;
    if (index >= this._renderedStart && index < this._renderedEnd) {
      const nodeIdx = index - this._renderedStart;
      const node = this._activeNodes[nodeIdx];
      if (node) {
        const fresh = this._renderRow(index, newData);
        fresh.style.position = 'absolute';
        fresh.style.top = (index * this._rowHeight) + 'px';
        fresh.style.width = '100%';
        this._viewport.replaceChild(fresh, node);
        this._activeNodes[nodeIdx] = fresh;
      }
    }
  }

  scrollToIndex(index) {
    const clamped = Math.max(0, Math.min(index, this._items.length - 1));
    this._container.scrollTop = clamped * this._rowHeight;
  }

  destroy() {
    if (this._destroyed) return;
    this._destroyed = true;
    this._container.removeEventListener('scroll', this._onScroll);
    if (this._resizeObserver) {
      this._resizeObserver.disconnect();
      this._resizeObserver = null;
    }
    if (this._rafId) {
      cancelAnimationFrame(this._rafId);
      this._rafId = null;
    }
    this._viewport.remove();
    this._spacer.remove();
    this._container.classList.remove('virtual-scroll-container');
    this._nodePool.length = 0;
    this._activeNodes.length = 0;
    this._items = [];
  }

  refresh() {
    this._renderedStart = -1;
    this._renderedEnd = -1;
    this._recycleAll();
    this._render();
  }

  getVisibleRange() {
    const scrollTop = this._container.scrollTop;
    const h = this._container.clientHeight;
    const start = Math.floor(scrollTop / this._rowHeight);
    const end = Math.min(start + Math.ceil(h / this._rowHeight), this._items.length);
    return { start, end };
  }

  /* --- Internal --- */

  _onScroll() {
    if (this._rafId || this._destroyed) return;
    this._rafId = requestAnimationFrame(() => {
      this._rafId = null;
      this._render();
    });
  }

  _onResize() {
    if (this._destroyed) return;
    this._render();
  }

  _render() {
    if (this._destroyed || !this._items.length) return;

    const scrollTop = this._container.scrollTop;
    const containerH = this._container.clientHeight;
    if (containerH === 0) return;

    const totalRows = this._items.length;
    const visibleCount = Math.ceil(containerH / this._rowHeight);
    const firstVisible = Math.floor(scrollTop / this._rowHeight);

    let newStart = Math.max(0, firstVisible - this._bufferRows);
    let newEnd = Math.min(totalRows, firstVisible + visibleCount + this._bufferRows);

    // Skip render if range hasn't changed
    if (newStart === this._renderedStart && newEnd === this._renderedEnd) return;

    const oldStart = this._renderedStart;
    const oldEnd = this._renderedEnd;
    this._renderedStart = newStart;
    this._renderedEnd = newEnd;

    // Recycle nodes that left the range
    if (oldStart >= 0) {
      const recycleNodes = [];
      for (let i = 0; i < this._activeNodes.length; i++) {
        const dataIdx = oldStart + i;
        if (dataIdx < newStart || dataIdx >= newEnd) {
          const node = this._activeNodes[i];
          if (node) {
            this._viewport.removeChild(node);
            this._nodePool.push(node);
          }
          recycleNodes.push(i);
        }
      }
    }

    // Build new active list
    const newCount = newEnd - newStart;
    const freshActive = new Array(newCount);

    for (let i = 0; i < newCount; i++) {
      const dataIdx = newStart + i;
      // Reuse if already rendered
      if (oldStart >= 0 && dataIdx >= oldStart && dataIdx < oldEnd) {
        const oldSlot = dataIdx - oldStart;
        freshActive[i] = this._activeNodes[oldSlot];
      } else {
        // Create or recycle a node
        const el = this._renderRow(dataIdx, this._items[dataIdx]);
        el.style.position = 'absolute';
        el.style.top = (dataIdx * this._rowHeight) + 'px';
        el.style.width = '100%';
        this._viewport.appendChild(el);
        freshActive[i] = el;
      }
    }

    this._activeNodes = freshActive;

    // Drain pool
    this._nodePool.length = 0;

    if (this._onRangeChange) {
      this._onRangeChange(newStart, newEnd);
    }
  }

  _recycleAll() {
    while (this._viewport.firstChild) {
      this._viewport.removeChild(this._viewport.firstChild);
    }
    this._activeNodes.length = 0;
    this._nodePool.length = 0;
  }
}

/* ============================================================
   renderVirtualResponseBody — Virtual-scroll for response bodies
   ============================================================ */

/**
 * Render a large formatted response body using virtual scrolling.
 * @param {HTMLElement} container - The responseBody element
 * @param {Array<{spans: Array<{class: string, text: string}>, raw: string}>} lines
 * @returns {VirtualScroll} The VirtualScroll instance (for later cleanup)
 */
function renderVirtualResponseBody(container, lines) {
  container.innerHTML = '';

  const vs = new VirtualScroll(container, {
    rowHeight: 20,
    bufferRows: 20,
    renderRow(index, line) {
      const div = document.createElement('div');
      div.className = 'vline';
      if (line.spans && line.spans.length) {
        for (const span of line.spans) {
          const s = document.createElement('span');
          if (span.class) s.className = span.class;
          s.textContent = span.text;
          div.appendChild(s);
        }
      } else {
        div.textContent = line.raw || '';
      }
      return div;
    },
  });

  vs.setData(lines);
  return vs;
}

/* ============================================================
   renderVirtualDiff — Synced virtual-scroll diff viewer
   ============================================================ */

/**
 * Render two synced virtual-scroll diff panels.
 * @param {HTMLElement} leftContainer  - Left diff panel element
 * @param {HTMLElement} rightContainer - Right diff panel element
 * @param {Array<{type: string, left?: string, right?: string}>} diffOps - from computeLineDiff
 * @param {Object} [options]
 * @param {Function} [options.highlightFn] - syntax highlight function (text) => html
 * @returns {{left: VirtualScroll, right: VirtualScroll, destroy: Function}}
 */
function renderVirtualDiff(leftContainer, rightContainer, diffOps, options) {
  const highlightFn = (options && options.highlightFn) ? options.highlightFn : (t) => t;

  leftContainer.innerHTML = '';
  rightContainer.innerHTML = '';

  // Build parallel row data for left and right
  const leftRows = [];
  const rightRows = [];
  let leftLn = 0;
  let rightLn = 0;

  for (const op of diffOps) {
    switch (op.type) {
      case 'same':
        leftLn++;
        rightLn++;
        leftRows.push({ type: 'same', ln: leftLn, text: op.left || '' });
        rightRows.push({ type: 'same', ln: rightLn, text: op.right || op.left || '' });
        break;
      case 'remove':
        leftLn++;
        leftRows.push({ type: 'removed', ln: leftLn, text: op.left || '' });
        rightRows.push({ type: 'empty', ln: null, text: '' });
        break;
      case 'add':
        rightLn++;
        leftRows.push({ type: 'empty', ln: null, text: '' });
        rightRows.push({ type: 'added', ln: rightLn, text: op.right || '' });
        break;
      case 'change':
        leftLn++;
        rightLn++;
        leftRows.push({ type: 'changed', ln: leftLn, text: op.left || '', charSide: 'left', otherText: op.right || '' });
        rightRows.push({ type: 'changed', ln: rightLn, text: op.right || '', charSide: 'right', otherText: op.left || '' });
        break;
    }
  }

  function makeRow(rowData) {
    const div = document.createElement('div');
    let cls = 'diff-line';
    if (rowData.type === 'same') cls += ' diff-same';
    else if (rowData.type === 'removed') cls += ' diff-removed';
    else if (rowData.type === 'added') cls += ' diff-added';
    else if (rowData.type === 'changed') cls += ' diff-changed';
    else if (rowData.type === 'empty') cls += ' diff-empty';
    div.className = cls;

    const lnSpan = document.createElement('span');
    lnSpan.className = 'diff-ln';
    lnSpan.textContent = rowData.ln != null ? String(rowData.ln) : '';
    div.appendChild(lnSpan);

    const textSpan = document.createElement('span');
    textSpan.className = 'diff-text';

    if (rowData.type === 'changed' && rowData.charSide && rowData.otherText !== undefined) {
      const lineA = rowData.charSide === 'left' ? rowData.text : rowData.otherText;
      const lineB = rowData.charSide === 'left' ? rowData.otherText : rowData.text;
      textSpan.innerHTML = charDiffHighlight(lineA, lineB, rowData.charSide, highlightFn);
    } else {
      textSpan.textContent = rowData.text;
    }

    div.appendChild(textSpan);
    return div;
  }

  // Synced scroll state
  let syncing = false;

  function syncScroll(source, target) {
    if (syncing) return;
    syncing = true;
    target._container.scrollTop = source._container.scrollTop;
    target._render();
    syncing = false;
  }

  const leftVs = new VirtualScroll(leftContainer, {
    rowHeight: 20,
    bufferRows: 20,
    renderRow(_index, rowData) { return makeRow(rowData); },
  });

  const rightVs = new VirtualScroll(rightContainer, {
    rowHeight: 20,
    bufferRows: 20,
    renderRow(_index, rowData) { return makeRow(rowData); },
  });

  // Wire up synced scrolling
  leftContainer.addEventListener('scroll', () => syncScroll(leftVs, rightVs), { passive: true });
  rightContainer.addEventListener('scroll', () => syncScroll(rightVs, leftVs), { passive: true });

  leftVs.setData(leftRows);
  rightVs.setData(rightRows);

  return {
    left: leftVs,
    right: rightVs,
    destroy() {
      leftVs.destroy();
      rightVs.destroy();
    },
  };
}

/* ============================================================
   renderVirtualHistoryList — Virtual-scroll for history entries
   ============================================================ */

/**
 * Render a virtual-scrolled history list.
 * @param {HTMLElement} container - The historyLog element
 * @param {Array} entries - Array of history entry objects
 * @returns {VirtualScroll} The VirtualScroll instance
 */
function renderVirtualHistoryList(container, entries) {
  container.innerHTML = '';

  const vs = new VirtualScroll(container, {
    rowHeight: 60,
    bufferRows: 10,
    renderRow(index, entry) {
      return createHistoryEntryRow(entry);
    },
  });

  vs.setData(entries);
  return vs;
}

// ============================================================================
// Sessions tab — persisted test-run sessions (Phase 1.5)
// ============================================================================
//
// Design notes (incorporating UX review):
//  - Default grouping is by file (not 3-level tree). Version is a chip.
//  - Row click opens snapshot (primary action). Replay requires a confirm
//    dialog showing env/mode/host before firing requests.
//  - Snapshot mode adds a left-edge accent stripe to the editor so users
//    don't lose context when scrolling past the top banner.
//  - Detach has a confirm dialog (clears snapshot marker, allows editing).
//  - Empty/error states have explicit, friendly copy.

const sessionsPanel        = document.getElementById('sessionsPanel');
const sessionsTree         = document.getElementById('sessionsTree');
const sessionsStatusEl     = document.getElementById('sessionsStatus');
const sessionsCountBadge   = document.getElementById('sessionsCountBadge');
const sessionsRefreshBtn   = document.getElementById('sessionsRefreshBtn');
const sessionsSettingsBtn  = document.getElementById('sessionsSettingsBtn');
const sessionsSettingsRow  = document.getElementById('sessionsSettingsRow');
const sessionsRootInput    = document.getElementById('sessionsRootInput');
const sessionsRootSaveBtn  = document.getElementById('sessionsRootSaveBtn');
const sessionsRootClearBtn = document.getElementById('sessionsRootClearBtn');
const sessionsRootBrowseBtn= document.getElementById('sessionsRootBrowseBtn');
const sessionsAutoChk      = document.getElementById('sessionsAutoRecordChk');
const sessionsSearch       = document.getElementById('sessionsSearch');
const sessionsStatusFilter = document.getElementById('sessionsStatusFilter');
const sessionsGroupBy      = document.getElementById('sessionsGroupBy');
const sessionsDetailOverlay= document.getElementById('sessionsDetailOverlay');
const sessionsDetailTitle  = document.getElementById('sessionsDetailTitle');
const sessionsDetailBody   = document.getElementById('sessionsDetailBody');
const sessionsDetailClose  = document.getElementById('sessionsDetailCloseBtn');
const sessionsDetailOpenSourceBtn   = document.getElementById('sessionsDetailOpenSourceBtn');
const sessionsDetailLoadSnapshotBtn = document.getElementById('sessionsDetailLoadSnapshotBtn');
const sessionsDetailReplayBtn       = document.getElementById('sessionsDetailReplayBtn');
const capturePresetRadios  = document.querySelectorAll('input[name="capturePreset"]');

let sessionsStatusCache = { root: null, auto_record: true, active: false };
let sessionsCapturePolicy = { preset: 'snapshot', policy: null };
// In-memory cache of all sessions across files: [{file, version, session}, ...]
let sessionsAllRows = [];
let sessionsCurrentLoaded = null; // { fileId, sha, runId, source, suite, record }

// ─── Status / config ──────────────────────────────────────────────────────

async function refreshSessionsStatus() {
  try {
    sessionsStatusCache = await invoke('sessions_get_status');
  } catch (e) {
    sessionsStatusCache = { root: null, auto_record: true, active: false };
  }
  try {
    sessionsCapturePolicy = await invoke('sessions_get_capture_policy');
  } catch (_) { /* command may be missing in older builds */ }
  if (sessionsRootInput) sessionsRootInput.value = sessionsStatusCache.root || '';
  if (sessionsAutoChk)   sessionsAutoChk.checked = !!sessionsStatusCache.auto_record;
  capturePresetRadios.forEach(r => { r.checked = (r.value === sessionsCapturePolicy.preset); });
  renderSessionsStatusLine();
}

function renderSessionsStatusLine() {
  if (!sessionsStatusEl) return;
  if (!sessionsStatusCache.active) {
    sessionsStatusEl.innerHTML = `<span style="color:var(--text-muted)">No sessions folder configured. Click <strong>⚙ Settings</strong> to choose one.</span>`;
    return;
  }
  const auto = sessionsStatusCache.auto_record ? '✓ auto-record on' : '⏸ auto-record off';
  const preset = sessionsCapturePolicy.preset || 'snapshot';
  sessionsStatusEl.innerHTML = `Root: <code>${escapeHtml(sessionsStatusCache.root)}</code> · ${auto} · capture: <strong>${escapeHtml(preset)}</strong>`;
}

// ─── Tree loader (flat list grouped by file) ──────────────────────────────

async function loadSessionsTree() {
  if (!sessionsTree) return;
  invalidateSessionsStatsCache();
  await refreshSessionsStatus();
  if (!sessionsStatusCache.active) {
    sessionsTree.innerHTML = renderSessionsEmptyNoRoot();
    sessionsCountBadge.textContent = '0';
    sessionsAllRows = [];
    return;
  }
  let files = [];
  try {
    files = await invoke('sessions_list_files');
  } catch (e) {
    sessionsTree.innerHTML = `<div class="sessions-error">Failed to list files: ${escapeHtml(String(e))}</div>`;
    return;
  }
  // Eagerly fetch versions+sessions so we can render a flat, filterable list.
  // Most stores are small (≤ 1000 sessions); this is fine for v1.
  const rows = [];
  for (const f of files) {
    let versions = [];
    try { versions = await invoke('sessions_list_versions', { fileId: f.file_id }); } catch (_) {}
    for (const v of versions) {
      let sessions = [];
      try { sessions = await invoke('sessions_list_sessions', { fileId: f.file_id, sha256: v.sha256 }); } catch (_) {}
      for (const s of sessions) {
        rows.push({ file: f, version: v, session: s });
      }
    }
  }
  // Newest first
  rows.sort((a, b) => (b.session.started_at || '').localeCompare(a.session.started_at || ''));
  sessionsAllRows = rows;
  sessionsCountBadge.textContent = String(rows.length);
  renderSessionsList();
}

function renderSessionsEmptyNoRoot() {
  return `
    <div class="sessions-empty">
      <div class="sessions-empty-icon">🗂</div>
      <h3>No sessions folder yet</h3>
      <p>Pick a folder and Request Pilot will record every test run there. You can browse, replay, and load runs back as snapshots.</p>
      <button class="btn btn-primary" id="sessionsEmptyBrowseBtn">📁 Choose folder…</button>
    </div>`;
}

function renderSessionsList() {
  const q = (sessionsSearch?.value || '').toLowerCase().trim();
  const statusFilter = sessionsStatusFilter?.value || 'all';
  const groupBy = sessionsGroupBy?.value || 'file';

  let rows = sessionsAllRows.filter(r => {
    if (q) {
      const hay = `${r.file.display_name} ${r.session.run_id} ${r.version.sha256}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    if (statusFilter !== 'all') {
      const passed = r.session.passed || 0;
      const failed = r.session.failed || 0;
      if (statusFilter === 'passed' && failed > 0) return false;
      if (statusFilter === 'failed' && failed === 0) return false;
      if (statusFilter === 'mixed' && (failed === 0 || passed === 0)) return false;
    }
    return true;
  });

  if (rows.length === 0) {
    if (sessionsAllRows.length === 0) {
      sessionsTree.innerHTML = `
        <div class="sessions-empty">
          <div class="sessions-empty-icon">📭</div>
          <h3>No sessions recorded yet</h3>
          <p>Run any <code>.http</code> file with auto-record on and your first session will appear here.</p>
        </div>`;
    } else {
      sessionsTree.innerHTML = `<div class="sessions-empty-small">No sessions match your filters.</div>`;
    }
    return;
  }

  // Group rows
  const groups = new Map(); // groupKey -> { label, sublabel, rows[] }
  for (const r of rows) {
    let key, label, sublabel;
    if (groupBy === 'file') {
      key = r.file.file_id;
      label = r.file.display_name;
      sublabel = `last seen ${formatRelative(r.file.last_seen)}`;
    } else if (groupBy === 'date') {
      key = (r.session.started_at || '').slice(0, 10) || 'unknown';
      label = formatDateBucket(key);
      sublabel = '';
    } else { // version
      key = `${r.file.file_id}::${r.version.sha256}`;
      label = `${r.file.display_name}`;
      sublabel = `⌬ ${r.version.sha256.slice(0,12)} · ${r.version.session_count} run${r.version.session_count===1?'':'s'}`;
    }
    if (!groups.has(key)) groups.set(key, { label, sublabel, rows: [] });
    groups.get(key).rows.push(r);
  }

  const html = [];
  const stripTargets = []; // {key, fileId, sha|null}
  for (const [key, g] of groups.entries()) {
    let stripFileId = null, stripSha = null, includeStrip = false;
    if (groupBy === 'version') {
      stripFileId = g.rows[0].file.file_id;
      stripSha = g.rows[0].version.sha256;
      includeStrip = true;
    } else if (groupBy === 'file') {
      stripFileId = g.rows[0].file.file_id;
      stripSha = null;
      includeStrip = true;
    }
    const stripHtml = includeStrip
      ? `<div class="sessions-stats-strip" data-strip-key="${escapeHtml(key)}" data-loading="1"><span class="sessions-stats-strip-placeholder">📊 Loading stats…</span></div>`
      : '';
    if (includeStrip) stripTargets.push({ key, fileId: stripFileId, sha: stripSha });
    html.push(`<div class="sessions-group">
      <div class="sessions-group-header">
        <span class="sessions-group-label">${escapeHtml(g.label)}</span>
        <span class="sessions-group-sub">${escapeHtml(g.sublabel)}</span>
        <span class="sessions-group-count">${g.rows.length}</span>
      </div>
      ${stripHtml}
      <div class="sessions-group-body">
        ${g.rows.map(renderSessionRow).join('')}
      </div>
    </div>`);
  }
  sessionsTree.innerHTML = html.join('');

  // Lazily fetch + fill stats strips (one per group). Failures are silent.
  for (const t of stripTargets) {
    getSessionStats(t.fileId, t.sha)
      .then((stats) => {
        const el = sessionsTree.querySelector(`[data-strip-key="${cssEscape(t.key)}"]`);
        if (!el) return;
        el.removeAttribute('data-loading');
        el.innerHTML = renderStatsStrip(stats);
      })
      .catch(() => {
        const el = sessionsTree.querySelector(`[data-strip-key="${cssEscape(t.key)}"]`);
        if (el) { el.removeAttribute('data-loading'); el.innerHTML = ''; }
      });
  }

  // Bind row clicks (primary action: open snapshot)
  sessionsTree.querySelectorAll('[data-session-row]').forEach(el => {
    el.addEventListener('click', (ev) => {
      const fileId = el.getAttribute('data-file-id');
      const sha = el.getAttribute('data-sha');
      const runId = el.getAttribute('data-run-id');
      // If a button inside the row was clicked, let it handle.
      if (ev.target.closest('button')) return;
      openSessionDetail(fileId, sha, runId);
    });
  });
  sessionsTree.querySelectorAll('[data-action="load-snapshot"]').forEach(b => {
    b.addEventListener('click', (ev) => {
      ev.stopPropagation();
      const r = b.closest('[data-session-row]');
      loadAsSnapshotFlow(r.getAttribute('data-file-id'), r.getAttribute('data-sha'), r.getAttribute('data-run-id'));
    });
  });
  sessionsTree.querySelectorAll('[data-action="open-detail"]').forEach(b => {
    b.addEventListener('click', (ev) => {
      ev.stopPropagation();
      const r = b.closest('[data-session-row]');
      openSessionDetail(r.getAttribute('data-file-id'), r.getAttribute('data-sha'), r.getAttribute('data-run-id'));
    });
  });
}

function renderSessionRow(r) {
  const passed = r.session.passed || 0;
  const failed = r.session.failed || 0;
  const skipped = r.session.skipped || 0;
  const totalMs = r.session.total_time_ms || 0;
  const when = formatRelative(r.session.started_at);
  const pillClass = failed > 0 ? 'fail' : (passed > 0 ? 'pass' : 'skip');
  const pillIcon  = failed > 0 ? '✗' : (passed > 0 ? '✓' : '⊘');
  const pillLabel = failed > 0 ? 'Failed' : (passed > 0 ? 'Passed' : 'Skipped');
  const versionChip = `<span class="sessions-chip" title="Source SHA ${r.version.sha256}">⌬ ${r.version.sha256.slice(0,8)}</span>`;
  const latencyClass = totalMs > 3000 ? 'slow' : (totalMs > 1000 ? 'warn' : '');
  return `
    <div class="sessions-row" data-session-row data-file-id="${escapeHtml(r.file.file_id)}" data-sha="${escapeHtml(r.version.sha256)}" data-run-id="${escapeHtml(r.session.run_id)}" tabindex="0" role="button" aria-label="Session ${r.session.run_id} — ${pillLabel}">
      <span class="sessions-status-pill ${pillClass}" aria-label="${pillLabel}">${pillIcon} ${pillLabel}</span>
      <span class="sessions-counts">✓${passed} ✗${failed} ⊘${skipped}</span>
      ${versionChip}
      <span class="sessions-runid" title="run-id ${r.session.run_id}">${escapeHtml(r.session.run_id.slice(0,8))}</span>
      <span class="sessions-when">${escapeHtml(when)}</span>
      <span class="sessions-latency ${latencyClass}">${totalMs}ms</span>
      <span class="sessions-row-actions">
        <button class="btn btn-ghost btn-xs" data-action="open-detail" title="View details">👁</button>
        <button class="btn btn-primary btn-xs" data-action="load-snapshot" title="Open snapshot in editor">📸 Open</button>
      </span>
    </div>`;
}

function formatRelative(iso) {
  if (!iso) return '—';
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const diff = (Date.now() - d.getTime()) / 1000;
  if (diff < 60) return `${Math.round(diff)}s ago`;
  if (diff < 3600) return `${Math.round(diff/60)}m ago`;
  if (diff < 86400) return `${Math.round(diff/3600)}h ago`;
  if (diff < 86400*7) return `${Math.round(diff/86400)}d ago`;
  return d.toLocaleDateString();
}

function formatDateBucket(yyyymmdd) {
  if (!yyyymmdd || yyyymmdd === 'unknown') return 'Unknown date';
  const today = new Date().toISOString().slice(0,10);
  if (yyyymmdd === today) return 'Today';
  const y = new Date(); y.setDate(y.getDate()-1);
  if (yyyymmdd === y.toISOString().slice(0,10)) return 'Yesterday';
  return yyyymmdd;
}

// ─── Stats (sparklines, strips, tab content) ──────────────────────────────

// Cache stats per (fileId, sha|''). One entry per key holds the in-flight
// promise (or resolved value) so concurrent callers share a single fetch.
const sessionsStatsCache = new Map();

function statsCacheKey(fileId, sha) { return `${fileId}:${sha || ''}`; }

function getSessionStats(fileId, sha) {
  const key = statsCacheKey(fileId, sha);
  if (sessionsStatsCache.has(key)) return sessionsStatsCache.get(key);
  const p = invoke('sessions_get_stats', { fileId, sha: sha || null })
    .catch((e) => { console.warn('[sessions] get_stats failed', e); return null; });
  sessionsStatsCache.set(key, p);
  return p;
}

function invalidateSessionsStatsCache() { sessionsStatsCache.clear(); }

// Unicode block sparkline. Maps numeric values into 8 levels.
function sparkline(values, min, max) {
  const chars = '▁▂▃▄▅▆▇█';
  if (!Array.isArray(values) || values.length === 0) return '';
  if (min == null || max == null) {
    const nums = values.filter(v => v != null && !isNaN(v));
    if (nums.length === 0) return '';
    min = Math.min(...nums);
    max = Math.max(...nums);
  }
  const range = (max - min) || 1;
  return values.map(v => {
    if (v == null || isNaN(v)) return ' ';
    const idx = Math.min(7, Math.max(0, Math.floor(((v - min) / range) * 7)));
    return chars[idx];
  }).join('');
}

function safeCssIdent(s) { return String(s).replace(/[^a-zA-Z0-9_-]/g, '_'); }
function cssEscape(s) {
  if (window.CSS && typeof window.CSS.escape === 'function') return window.CSS.escape(s);
  return String(s).replace(/[^a-zA-Z0-9_-]/g, ch => '\\' + ch);
}

function renderStatsStrip(stats) {
  if (!stats) {
    return `<span class="sessions-stats-strip-empty">📊 No stats yet</span>`;
  }
  const totals = stats.totals || {};
  const passed = totals.passed || 0;
  const failed = totals.failed || 0;
  const sessions = stats.session_count || 0;
  const denom = passed + failed;
  const passRate = denom > 0 ? Math.round((passed / denom) * 100) : null;
  const buckets = (stats.buckets || []).slice(-24);
  const passRates = buckets.map(b => {
    const d = (b.passed || 0) + (b.failed || 0);
    return d > 0 ? (b.passed / d) * 100 : null;
  });
  const sparkPass = sparkline(passRates, 0, 100);
  const lat = stats.latency || {};
  const p50 = lat.p50_ms || 0;
  const p95 = lat.p95_ms || 0;
  const tooltip = buckets.length
    ? `Recent buckets:\n` + buckets.map(b => `${b.start} (${b.window}) ${b.passed||0}/${(b.passed||0)+(b.failed||0)} pass · p95 ${b.p95_ms||0}ms`).join('\n')
    : 'No bucketed runs yet.';
  const passLabel = passRate == null ? '—' : `${passRate}%`;
  return `
    <span class="sessions-stats-strip-label">📊 Pass-rate</span>
    <span class="sessions-stats-spark" title="${escapeHtml(tooltip)}">${escapeHtml(sparkPass) || '—'}</span>
    <span class="sessions-stats-strip-num">${passLabel}</span>
    <span class="sessions-stats-strip-sep">·</span>
    <span class="sessions-stats-strip-num">p50 ${p50}ms</span>
    <span class="sessions-stats-strip-num">p95 ${p95}ms</span>
    <span class="sessions-stats-strip-sep">·</span>
    <span class="sessions-stats-strip-num">${sessions} session${sessions === 1 ? '' : 's'}</span>
  `;
}

function renderStatsTab(stats, fileId, sha) {
  if (!stats) {
    return `<div class="sessions-stats-empty">📊 No stats yet — run this file to populate stats.</div>`;
  }
  const totals = stats.totals || {};
  const lat = stats.latency || {};
  const passed = totals.passed || 0;
  const failed = totals.failed || 0;
  const mixed = totals.mixed || 0;
  const skipped = totals.skipped_blocks || 0;
  const sessions = stats.session_count || 0;
  const denom = passed + failed;
  const passRate = denom > 0 ? Math.round((passed / denom) * 100) : null;

  const buckets = (stats.buckets || []).slice();
  const bucketsRecent = buckets.slice(-30);
  const passRates = bucketsRecent.map(b => {
    const d = (b.passed || 0) + (b.failed || 0);
    return d > 0 ? (b.passed / d) * 100 : null;
  });
  const p95Series = bucketsRecent.map(b => b.p95_ms || 0);
  const sparkPass = sparkline(passRates, 0, 100);
  const sparkP95 = sparkline(p95Series);

  // Top-5 flakiest blocks: rank by failure rate × (passed+failed) and require both >0.
  const byBlock = stats.by_block || {};
  const flaky = Object.keys(byBlock).map(name => {
    const b = byBlock[name] || {};
    const total = (b.passed || 0) + (b.failed || 0);
    const failRate = total > 0 ? (b.failed || 0) / total : 0;
    return { name, runs: b.runs || 0, passed: b.passed || 0, failed: b.failed || 0, p50: b.p50_ms || 0, p95: b.p95_ms || 0, failRate };
  }).filter(x => x.failed > 0)
    .sort((a, b) => (b.failRate - a.failRate) || (b.failed - a.failed))
    .slice(0, 5);

  const flakyRows = flaky.length === 0
    ? `<tr><td colspan="6" style="color:var(--text-muted);text-align:center">No flaky blocks 🎉</td></tr>`
    : flaky.map(x => `<tr>
        <td>${escapeHtml(x.name)}</td>
        <td>${x.runs}</td>
        <td><span class="sessions-stats-pass">${x.passed}</span> / <span class="sessions-stats-fail">${x.failed}</span></td>
        <td>${Math.round(x.failRate * 100)}%</td>
        <td>${x.p50}ms</td>
        <td>${x.p95}ms</td>
      </tr>`).join('');

  const versionsHtml = (stats.versions && stats.versions.length)
    ? `<div class="sessions-stats-section">
        <h4>Versions (${stats.versions.length})</h4>
        <table class="sessions-stats-table">
          <thead><tr><th>SHA</th><th>Sessions</th><th>First seen</th><th>Last seen</th></tr></thead>
          <tbody>${stats.versions.map(v => `<tr>
            <td><code>${escapeHtml(v.sha256.slice(0,12))}</code></td>
            <td>${v.session_count}</td>
            <td>${escapeHtml(formatRelative(v.first_seen))}</td>
            <td>${escapeHtml(formatRelative(v.last_seen))}</td>
          </tr>`).join('')}</tbody>
        </table>
      </div>`
    : '';

  const scope = sha ? `version <code>${escapeHtml(sha.slice(0,12))}</code>` : 'all versions (file rollup)';
  const passRateLabel = passRate == null ? '—' : `${passRate}%`;
  const minP95 = p95Series.length ? Math.min(...p95Series) : 0;
  const maxP95 = p95Series.length ? Math.max(...p95Series) : 0;

  return `
    <div class="sessions-stats-tab-content">
      <div class="sessions-stats-scope">Scope: ${scope} · first seen ${escapeHtml(formatRelative(stats.first_seen))} · last seen ${escapeHtml(formatRelative(stats.last_seen))}</div>

      <div class="sessions-stats-totals">
        <div class="sessions-stats-card">
          <div class="sessions-stats-card-label">Sessions</div>
          <div class="sessions-stats-card-value">${sessions}</div>
        </div>
        <div class="sessions-stats-card">
          <div class="sessions-stats-card-label">Pass rate</div>
          <div class="sessions-stats-card-value">${passRateLabel}</div>
          <div class="sessions-stats-card-sub"><span class="sessions-stats-pass">✓ ${passed}</span> · <span class="sessions-stats-fail">✗ ${failed}</span> · ⚠ ${mixed} · ⊘ ${skipped}</div>
        </div>
        <div class="sessions-stats-card">
          <div class="sessions-stats-card-label">Latency p50 / p95 / p99</div>
          <div class="sessions-stats-card-value">${lat.p50_ms||0} / ${lat.p95_ms||0} / ${lat.p99_ms||0} <span class="sessions-stats-card-unit">ms</span></div>
          <div class="sessions-stats-card-sub">max ${lat.max_ms||0}ms · ${(lat.samples||[]).length} samples</div>
        </div>
      </div>

      <div class="sessions-stats-section">
        <h4>Pass-rate trend (last ${bucketsRecent.length} buckets)</h4>
        <div class="sessions-stats-spark sessions-stats-spark-large" title="0–100% pass-rate per bucket">${escapeHtml(sparkPass) || '<span style="color:var(--text-muted)">no bucketed runs yet</span>'}</div>
      </div>

      <div class="sessions-stats-section">
        <h4>Latency p95 trend</h4>
        <div class="sessions-stats-spark sessions-stats-spark-large" title="${minP95}ms – ${maxP95}ms per bucket">${escapeHtml(sparkP95) || '<span style="color:var(--text-muted)">no bucketed runs yet</span>'}</div>
        <div class="sessions-stats-card-sub">range ${minP95}ms – ${maxP95}ms</div>
      </div>

      <div class="sessions-stats-section">
        <h4>Top-5 flakiest blocks</h4>
        <table class="sessions-stats-table">
          <thead><tr><th>Block</th><th>Runs</th><th>Pass / Fail</th><th>Fail rate</th><th>p50</th><th>p95</th></tr></thead>
          <tbody>${flakyRows}</tbody>
        </table>
      </div>

      ${versionsHtml}
    </div>
  `;
}

// ─── Detail overlay ───────────────────────────────────────────────────────

async function openSessionDetail(fileId, sha, runId) {
  try {
    const loaded = await invoke('sessions_load_session', { fileId, sha256: sha, runId });
    sessionsCurrentLoaded = { fileId, sha, runId, ...loaded };
    sessionsDetailTitle.textContent = `${loaded.record.run_id.slice(0,12)} · ${new Date(loaded.record.started_at).toLocaleString()}`;
    sessionsDetailBody.innerHTML = renderSessionDetailHtml(loaded);
    sessionsDetailOverlay.classList.remove('hidden');
    bindSessionDetailTabs(fileId, sha);
  } catch (e) {
    showToast('Failed to load session: ' + e, 'error');
  }
}

function bindSessionDetailTabs(fileId, sha) {
  const root = sessionsDetailBody;
  if (!root) return;
  const tabs = root.querySelectorAll('[data-sd-tab]');
  const panels = root.querySelectorAll('[data-sd-panel]');
  tabs.forEach(t => {
    t.addEventListener('click', () => {
      const id = t.getAttribute('data-sd-tab');
      tabs.forEach(x => x.classList.toggle('active', x === t));
      panels.forEach(p => p.classList.toggle('active', p.getAttribute('data-sd-panel') === id));
      if (id === 'stats') ensureStatsPanelLoaded(fileId, sha);
    });
  });
}

async function ensureStatsPanelLoaded(fileId, sha) {
  const panel = sessionsDetailBody?.querySelector('[data-sd-panel="stats"]');
  if (!panel || panel.dataset.loaded === '1') return;
  const stats = await getSessionStats(fileId, sha);
  panel.dataset.loaded = '1';
  panel.innerHTML = renderStatsTab(stats, fileId, sha);
}

function renderSessionDetailHtml(loaded) {
  const r = loaded.record;
  const blocks = (r.block_summaries || []).map(b => {
    const cls = b.status === 'passed' ? 'success' : (b.status === 'failed' ? 'error' : 'muted');
    return `<tr>
      <td><span style="color:var(--accent-${cls})">${escapeHtml(b.status)}</span></td>
      <td>${escapeHtml(b.name)}</td>
      <td>${escapeHtml(b.block_type)}</td>
      <td>${b.assertion_passed}/${b.assertion_total}</td>
      <td>${b.extract_ok}/${b.extract_total}</td>
      <td>${b.time_ms}ms</td>
    </tr>`;
  }).join('');
  const rr = r.redaction_report || {};
  return `
    <div class="sessions-detail-tabs" role="tablist">
      <button class="sessions-detail-tab active" data-sd-tab="blocks" role="tab">Blocks</button>
      <button class="sessions-detail-tab" data-sd-tab="redaction" role="tab">Redaction</button>
      <button class="sessions-detail-tab" data-sd-tab="source" role="tab">Source</button>
      <button class="sessions-detail-tab sessions-stats-tab" data-sd-tab="stats" role="tab">📊 Stats</button>
    </div>

    <div class="sessions-detail-panel active" data-sd-panel="blocks">
      <div style="padding:14px;font-size:13px">
        <div class="session-detail-meta">
          <div><strong>Trigger</strong> ${escapeHtml(JSON.stringify(r.trigger))}</div>
          <div><strong>Component</strong> ${escapeHtml(JSON.stringify(r.component))}</div>
          <div><strong>Host</strong> ${escapeHtml(r.host)} <span style="color:var(--text-muted)">(${escapeHtml(r.os)})</span></div>
          <div><strong>Duration</strong> ${r.results?.total_time_ms || 0}ms</div>
          <div><strong>Mode</strong> ${escapeHtml(r.mode || '—')}</div>
          <div><strong>Env</strong> ${escapeHtml(r.env_file || '—')}</div>
          <div style="grid-column:1/-1"><strong>Source SHA</strong> <code>${escapeHtml(r.source_sha256.slice(0,16))}…</code></div>
        </div>
        <div style="margin-top:10px"><strong>Results:</strong> ✓ ${r.results?.passed||0} · ✗ ${r.results?.failed||0} · ⊘ ${r.results?.skipped||0}</div>
        <table class="sessions-block-table">
          <thead><tr><th>Status</th><th>Name</th><th>Type</th><th>Assert</th><th>Extract</th><th>Time</th></tr></thead>
          <tbody>${blocks}</tbody>
        </table>
      </div>
    </div>

    <div class="sessions-detail-panel" data-sd-panel="redaction">
      <div style="padding:14px;font-size:13px">
        <h4 style="margin-top:0">Redaction report</h4>
        <table class="sessions-stats-table">
          <thead><tr><th>Field</th><th>Count</th></tr></thead>
          <tbody>
            <tr><td>Headers redacted</td><td>${rr.headers_redacted||0}</td></tr>
            <tr><td>Query params redacted</td><td>${rr.query_params_redacted||0}</td></tr>
            <tr><td>Variables dropped</td><td>${rr.variables_dropped||0}</td></tr>
            <tr><td>Bodies dropped</td><td>${rr.bodies_dropped||0}</td></tr>
            <tr><td>Bodies truncated</td><td>${rr.bodies_truncated||0}</td></tr>
          </tbody>
        </table>
      </div>
    </div>

    <div class="sessions-detail-panel" data-sd-panel="source">
      <div style="padding:14px;font-size:13px">
        <div style="color:var(--text-muted);margin-bottom:6px">${loaded.source.length} bytes</div>
        <pre style="max-height:60vh;overflow:auto;font-size:11px;background:var(--bg-secondary);padding:8px;border-radius:4px">${escapeHtml(loaded.source)}</pre>
      </div>
    </div>

    <div class="sessions-detail-panel" data-sd-panel="stats">
      <div class="sessions-stats-loading">📊 Loading stats…</div>
    </div>`;
}

// ─── Snapshot loading (reconstitute test state) ───────────────────────────

async function loadAsSnapshotFlow(fileId, sha, runId) {
  let loaded;
  try {
    loaded = await invoke('sessions_load_as_snapshot', { fileId, sha256: sha, runId });
  } catch (e) {
    showToast('Failed to load snapshot: ' + e, 'error');
    return;
  }
  const r = loaded.record;
  const runShort = r.run_id.slice(0, 8);
  const startedShort = new Date(r.started_at).toLocaleString();
  const fileEntry = {
    name: `📸 snapshot ${runShort}`,
    content: loaded.source,
    suite: loaded.suite,
    results: r.results || null,
    savedPath: null,
    snapshot: {
      runId: r.run_id,
      fileId,
      sha256: sha,
      startedAt: r.started_at,
      mode: r.mode,
      envFile: r.env_file,
      capturePolicy: r.capture_policy,
      redactionReport: r.redaction_report,
      sourceSha256: r.source_sha256,
      locked: true,
    },
  };
  loadedFiles.push(fileEntry);
  activeFileIndex = loadedFiles.length - 1;
  activeBlockIndex = -1;
  sessionsDetailOverlay.classList.add('hidden');
  if (typeof renderFileTree === 'function') renderFileTree();
  // Roll the per-block statuses up to group dots so users see at-a-glance
  // pass/fail summaries on group nodes immediately after loading.
  if (typeof updateBlockStatuses === 'function') updateBlockStatuses();
  // Re-scan loaded files for dev_auth blocks so the Azure auth state
  // reflects what the snapshot needs to replay (it may have been recorded
  // in `dev` mode against scopes the user is not currently authenticated for).
  if (typeof detectAzureAuthNeeded === 'function') detectAzureAuthNeeded();
  // Switch to builder mode so block statuses render immediately.
  if (typeof switchMode === 'function') switchMode('builder');
  applySnapshotLockUI();
  // Surface the recorded mode in the toast so users know whether replay
  // will need Azure auth (mode='dev') or run as app (mode='app'/null).
  const modeLabel = r.mode ? ` · mode: ${r.mode}` : '';
  showToast(`Snapshot loaded · ${runShort} · ${startedShort}${modeLabel}`, 'success');
}

function activeSnapshot() {
  const f = loadedFiles[activeFileIndex];
  return (f && f.snapshot) ? f.snapshot : null;
}

/// Apply / refresh the snapshot lock visuals — left-edge stripe on editor,
/// banner above the builder, code-editor readonly.
function applySnapshotLockUI() {
  const snap = activeSnapshot();
  const editorPanel = codeEditorPanel;
  if (snap && snap.locked) {
    if (editorPanel) editorPanel.classList.add('snapshot-locked');
    if (codeEditor) codeEditor.setAttribute('readonly', 'true');
    renderSnapshotBanner(snap);
  } else {
    if (editorPanel) editorPanel.classList.remove('snapshot-locked');
    if (codeEditor) codeEditor.removeAttribute('readonly');
    removeSnapshotBanner();
  }
}

function renderSnapshotBanner(snap) {
  removeSnapshotBanner();
  const banner = document.createElement('div');
  banner.id = 'snapshotBanner';
  banner.className = 'snapshot-banner';
  banner.setAttribute('role', 'status');
  banner.innerHTML = `
    <span class="snap-icon">📸</span>
    <span class="snap-text">
      <strong>Snapshot</strong> · run <code>${escapeHtml(snap.runId.slice(0,12))}</code>
      · ${escapeHtml(new Date(snap.startedAt).toLocaleString())}
      · mode: <strong>${escapeHtml(snap.mode || '—')}</strong>
      · env: <strong>${escapeHtml(snap.envFile || '—')}</strong>
    </span>
    <span class="snap-spacer"></span>
    <button class="btn btn-ghost btn-xs" id="snapDetachBtn" title="Make this file editable">🔓 Detach to edit</button>
    <button class="btn btn-primary btn-xs" id="snapReplayBtn" title="Re-run all blocks (creates a new session)">▶ Replay all</button>`;
  // Insert at top of the main content area.
  const host = document.querySelector('.main-content') || document.body;
  host.insertBefore(banner, host.firstChild);
  document.getElementById('snapDetachBtn')?.addEventListener('click', detachSnapshotFlow);
  document.getElementById('snapReplayBtn')?.addEventListener('click', replaySnapshotFlow);
}

function removeSnapshotBanner() {
  const b = document.getElementById('snapshotBanner');
  if (b) b.remove();
}

async function detachSnapshotFlow() {
  const f = loadedFiles[activeFileIndex];
  if (!f || !f.snapshot) return;
  const ok = await showModalConfirm({
    title: 'Detach snapshot?',
    message:
      'This unlocks the editor so you can edit the file. The original snapshot stays in Sessions and is unaffected.',
    okLabel: 'Detach & Edit',
    cancelLabel: 'Keep locked',
  });
  if (!ok) return;
  delete f.snapshot;
  applySnapshotLockUI();
  showToast('Snapshot detached — file is now editable', 'info');
}

async function replaySnapshotFlow() {
  const f = loadedFiles[activeFileIndex];
  if (!f || !f.snapshot) return;
  // Best-effort host extraction for confirmation
  const firstBlock = f.suite?.blocks?.[0];
  const targetHost = firstBlock?.url?.match(/^https?:\/\/([^\/]+)/)?.[1] || '(varies by block)';
  const ok = await showModalConfirm({
    title: `Replay ${f.suite?.blocks?.length || '?'} block(s)?`,
    message:
      `Mode: ${f.snapshot.mode || '—'}\n` +
      `Env file: ${f.snapshot.envFile || '— (current env will be used)'}\n` +
      `First target host: ${targetHost}\n\n` +
      `This fires real network requests against the current environment and creates a new session. ` +
      `The snapshot you're viewing is not modified.`,
    okLabel: '▶ Replay all',
    cancelLabel: 'Cancel',
    type: 'warn',
  });
  if (!ok) return;
  // Trigger the standard run flow — code path differs slightly between modes.
  // Use the existing "Run all" affordance when present.
  const runAllBtn = document.getElementById('runAllBtn') || document.getElementById('codeRunBtn');
  if (runAllBtn) {
    runAllBtn.click();
    showToast('Replay started…', 'info');
  } else {
    showToast('Use the Run button in the toolbar to replay.', 'info');
  }
}

// ─── Settings handlers ────────────────────────────────────────────────────

if (sessionsRefreshBtn)  sessionsRefreshBtn.addEventListener('click', loadSessionsTree);
if (sessionsSettingsBtn) sessionsSettingsBtn.addEventListener('click', () => sessionsSettingsRow.classList.toggle('hidden'));

async function browseForRoot() {
  try {
    const picked = await invoke('sessions_pick_root_dir', { startDir: sessionsRootInput?.value || null });
    if (picked) sessionsRootInput.value = picked;
  } catch (e) {
    showToast('Folder picker failed: ' + e, 'error');
  }
}
if (sessionsRootBrowseBtn) sessionsRootBrowseBtn.addEventListener('click', browseForRoot);

document.addEventListener('click', (ev) => {
  if (ev.target?.id === 'sessionsEmptyBrowseBtn') {
    sessionsSettingsRow?.classList.remove('hidden');
    browseForRoot();
  }
});

if (sessionsRootSaveBtn) sessionsRootSaveBtn.addEventListener('click', async () => {
  const v = sessionsRootInput.value.trim();
  try {
    sessionsStatusCache = await invoke('sessions_set_root', { root: v || null });
    showToast('Sessions folder updated', 'success');
    await loadSessionsTree();
  } catch (e) { showToast('Failed: ' + e, 'error'); }
});

if (sessionsRootClearBtn) sessionsRootClearBtn.addEventListener('click', async () => {
  try {
    sessionsStatusCache = await invoke('sessions_set_root', { root: null });
    sessionsRootInput.value = '';
    showToast('Sessions disabled', 'info');
    await loadSessionsTree();
  } catch (e) { showToast('Failed: ' + e, 'error'); }
});

if (sessionsAutoChk) sessionsAutoChk.addEventListener('change', async () => {
  try {
    sessionsStatusCache = await invoke('sessions_set_auto_record', { enabled: sessionsAutoChk.checked });
    renderSessionsStatusLine();
  } catch (e) { showToast('Failed: ' + e, 'error'); }
});

capturePresetRadios.forEach(r => r.addEventListener('change', async () => {
  if (!r.checked) return;
  try {
    sessionsCapturePolicy = await invoke('sessions_set_capture_preset', { preset: r.value, policy: null });
    renderSessionsStatusLine();
    showToast(`Capture preset: ${r.value}`, 'success');
  } catch (e) { showToast('Failed: ' + e, 'error'); }
}));

if (sessionsSearch)       sessionsSearch.addEventListener('input', renderSessionsList);
if (sessionsStatusFilter) sessionsStatusFilter.addEventListener('change', renderSessionsList);
if (sessionsGroupBy)      sessionsGroupBy.addEventListener('change', renderSessionsList);

if (sessionsDetailClose) sessionsDetailClose.addEventListener('click', () => sessionsDetailOverlay.classList.add('hidden'));
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !sessionsDetailOverlay.classList.contains('hidden')) {
    sessionsDetailOverlay.classList.add('hidden');
  }
});

if (sessionsDetailLoadSnapshotBtn) sessionsDetailLoadSnapshotBtn.addEventListener('click', () => {
  if (!sessionsCurrentLoaded) return;
  loadAsSnapshotFlow(sessionsCurrentLoaded.fileId, sessionsCurrentLoaded.sha, sessionsCurrentLoaded.runId);
});

if (sessionsDetailReplayBtn) sessionsDetailReplayBtn.addEventListener('click', () => {
  // Replay = load snapshot then immediately replay, with the same confirm guard.
  if (!sessionsCurrentLoaded) return;
  loadAsSnapshotFlow(sessionsCurrentLoaded.fileId, sessionsCurrentLoaded.sha, sessionsCurrentLoaded.runId)
    .then(() => setTimeout(replaySnapshotFlow, 80));
});

if (sessionsDetailOpenSourceBtn) sessionsDetailOpenSourceBtn.addEventListener('click', () => {
  if (!sessionsCurrentLoaded) return;
  const src = sessionsCurrentLoaded.source;
  const name = `session-${sessionsCurrentLoaded.runId.slice(0,8)}.http`;
  try {
    loadedFiles.push({ name, content: src, suite: { variables: [], blocks: [] }, results: null, savedPath: null });
    activeFileIndex = loadedFiles.length - 1;
    activeBlockIndex = -1;
    sessionsDetailOverlay.classList.add('hidden');
    if (typeof renderFileTree === 'function') renderFileTree();
    if (typeof switchMode === 'function') switchMode('code');
    showToast('Source loaded as new buffer', 'success');
  } catch (e) {
    showToast('Failed to open source: ' + e, 'error');
  }
});

// Whenever the active file changes, re-evaluate snapshot lock UI.
const __origSwitchMode = window.switchMode;
// Hook into renderFileTree's invocations of switchMode by polling on the
// active-file index changing. Safe because applySnapshotLockUI is idempotent.
setInterval(() => { try { applySnapshotLockUI(); } catch (_) {} }, 400);

// Best-effort recorder. Never throws into the run path.
async function recordSessionRun(file, suite, sourceContent, results, extraVars, mode, startedAtMs) {
  if (!sessionsStatusCache || !sessionsStatusCache.active || !sessionsStatusCache.auto_record) return;
  try {
    await invoke('sessions_record_run', {
      suite,
      filePath: file?.savedPath || null,
      alias: null,
      sourceContent: sourceContent || file?.content || '',
      results,
      variables: extraVars || [],
      mode: mode || null,
      envFile: (typeof activeEnvPath !== 'undefined') ? activeEnvPath : null,
      startedAtMs: startedAtMs || Date.now(),
    });
  } catch (e) {
    console.warn('[sessions] record_run failed', e);
  }
}

// Refresh status on startup
refreshSessionsStatus().catch(() => {});
