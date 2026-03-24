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
let loadedFiles = [];        // [{name, content, suite: TestSuite, results: TestRunResults|null}]
let activeFileIndex = -1;
let activeBlockIndex = -1;
let envVars = {};            // {name: value} — from .env file
let envFilePath = null;      // path to loaded .env file
let disabledBlocks = {};     // {"fileIdx-blockIdx": true} — disabled steps
let isRunning = false;
let lastResponse = null;
let currentMode = 'builder';    // 'builder' | 'code' | 'history' | 'logs'
let codeEditorContent = '';     // last saved content in code editor
let codeEditorModified = false;
let historyCache = [];
let historyExpandedGroups = new Set();
let historySelectedIds = new Set();  // Selected for comparison
// Tri-state viewed tracking: Maps key → { status, response_hash }
// States: unseen (not in map), seen (in map, hash matches), seen-mutated (in map, hash differs)
let viewedResults = new Map();       // "fileIdx-blockIdx" → { status, hash }
let viewedHistoryEntries = new Map(); // seq → { status, hash }
let runHistory = new Map(); // Map<fileName, [{timestamp, passed, failed, skipped, totalTime, blockResults: [{name, status, timeMs}]}]>

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
let telemetryEnabled = true; // user toggle — when false, skip OTEL export even if configured

// --- Live Capture ---
let liveCaptureMode = 'off';
let liveCaptureConnected = false;
let liveCapturedRequests = [];       // accumulated for current session
let liveCaptureFileIndex = -1;       // index in loadedFiles for the virtual .http file
let liveCaptureSessionId = null;

// --- Zoom ---
let zoomLevel = parseInt(localStorage.getItem('rp-zoom') || '100', 10);
const ZOOM_MIN = 50, ZOOM_MAX = 200, ZOOM_STEP = 10;

function applyZoom() {
  // Zoom on <html> so everything scales uniformly including the viewport.
  document.documentElement.style.zoom = `${zoomLevel}%`;
  // Clear any stale styles from previous implementations
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

// --- DOM Refs ---
const methodSelect    = $('#methodSelect');
const urlInput        = $('#urlInput');
const sendBtn         = $('#sendBtn');
const addHeaderBtn    = $('#addHeaderBtn');
const headersContainer= $('#headersContainer');
const bodyType        = $('#bodyType');
const bodyInput       = $('#bodyInput');
const bodyHighlight   = $('#bodyHighlight');
const bodyEditorWrap  = $('#bodyEditorWrapper');
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
const saveEnvBtn      = $('#saveEnvBtn');
const addEnvVarBtn    = $('#addEnvVarBtn');
const clearEnvBtn     = $('#clearEnvBtn');
const envFileInput    = $('#envFileInput');
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
const newFileBtn      = $('#newFileBtn');
const codeEditorHighlightCode = $('#codeEditorHighlightCode');
const codeEditorHighlight = $('#codeEditorHighlight');
const codeLineNumbers = $('#codeLineNumbers');
const splitPanels     = $('.split-panels');
const urlBar          = $('.url-bar');

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
});

// --- Method select color ---
const METHOD_COLORS = {
  GET: '#3fb950', POST: '#58a6ff', PUT: '#d29922', PATCH: '#d29922',
  DELETE: '#f85149', HEAD: '#bc8cff', OPTIONS: '#8b949e'
};

function updateMethodColor() {
  methodSelect.style.color = METHOD_COLORS[methodSelect.value] || '#e6edf3';
}
methodSelect.addEventListener('change', updateMethodColor);
updateMethodColor();

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
  });
  return row;
}

addHeaderBtn.addEventListener('click', () => {
  headersContainer.appendChild(createHeaderRow());
});

headersContainer.querySelector('.kv-remove')?.addEventListener('click', function() {
  this.closest('.kv-row').remove();
  if (headersContainer.children.length === 0) headersContainer.appendChild(createHeaderRow());
});

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
bodyType.addEventListener('change', () => {
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
  highlightBody();
});

// --- Body Syntax Highlighting ---
function detectBodyLang(text) {
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
  return text
    .replace(funcs, '<span class="hl-fn">$1</span>')
    .replace(mods, '<span class="hl-kw">$1</span>')
    .replace(/(&quot;)((?:[^&]|&(?!quot;))*)(&quot;)/g, '<span class="hl-str">$1$2$3</span>')
    .replace(/([\w]+)\s*(=~|!=|=|!~)/g, '<span class="hl-attr">$1</span> <span class="hl-pct">$2</span>')
    .replace(/\b(\d+\.?\d*)(s|m|h|d|w|y)?\b/g, '<span class="hl-num">$1$2</span>')
    .replace(/([{}[\]()])/g, '<span class="hl-bkt">$1</span>');
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
    default:       codeEl.innerHTML = escaped;
  }
  bodyHighlight.scrollTop = bodyInput.scrollTop;
  bodyHighlight.scrollLeft = bodyInput.scrollLeft;
}

bodyInput.addEventListener('input', highlightBody);
bodyInput.addEventListener('scroll', () => {
  bodyHighlight.scrollTop = bodyInput.scrollTop;
  bodyHighlight.scrollLeft = bodyInput.scrollLeft;
});

// --- Variable interpolation ---
function interpolateVariables(str) {
  if (!str) return str;
  const merged = {};
  loadedFiles.forEach(file => {
    if (file.suite.variables) {
      file.suite.variables.forEach(([name, value]) => { merged[name] = value; });
    }
  });
  Object.entries(envVars).forEach(([name, value]) => {
    if (value !== '') merged[name] = value;
  });
  return str.replace(/\{\{(\w+)\}\}/g, (match, name) => {
    if (name === '$timestamp') return Date.now().toString();
    if (name === '$uuid') return crypto.randomUUID();
    if (name === '$randomInt') return Math.floor(Math.random() * 10000).toString();
    return merged[name] !== undefined ? merged[name] : match;
  });
}

function collectVariablesArray() {
  const merged = {};
  loadedFiles.forEach(file => {
    if (file.suite.variables) {
      file.suite.variables.forEach(([name, value]) => { merged[name] = value; });
    }
  });
  Object.entries(envVars).forEach(([name, value]) => {
    if (value !== '') merged[name] = value;
  });
  return Object.entries(merged);
}

// Build a suite copy with disabled blocks removed
function getEnabledSuite(fileIdx) {
  const file = loadedFiles[fileIdx];
  if (!file) return null;
  const enabledBlocks = file.suite.blocks.filter((_, blockIdx) => !disabledBlocks[`${fileIdx}-${blockIdx}`]);
  return {
    variables: file.suite.variables,
    blocks: enabledBlocks,
    telemetry_var: file.suite.telemetry_var || null,
    telemetry_service: file.suite.telemetry_service || null,
    telemetry_token: file.suite.telemetry_token || null,
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

// --- Send Request ---
async function sendRequest() {
  const method = methodSelect.value;
  let url = urlInput.value.trim();

  if (!url) {
    showToast('Please enter a URL', 'error');
    urlInput.focus();
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
    refreshHistoryIfVisible();
  }
}

sendBtn.addEventListener('click', sendRequest);
urlInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.ctrlKey && !e.shiftKey) sendRequest();
});

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
function renderResponseBodyInto(container, body, headers) {
  const contentType = detectContentType(body, headers);
  container.innerHTML = '';
  container.dataset.rawBody = body || '';
  container.dataset.contentType = contentType;

  const badge = document.createElement('span');
  badge.className = 'response-type-badge';
  badge.textContent = contentType.toUpperCase();
  container.appendChild(badge);

  switch (contentType) {
    case 'json':
      try {
        const parsed = JSON.parse(body);
        container.appendChild(renderJsonTree(parsed));
        container.dataset.rawJson = JSON.stringify(parsed, null, 2);
      } catch {
        container.dataset.rawJson = body;
        const pre = document.createElement('pre');
        pre.className = 'syntax-plain';
        pre.textContent = body;
        container.appendChild(pre);
      }
      break;
    case 'xml':
    case 'html':
      container.appendChild(renderXmlHighlighted(body));
      break;
    case 'yaml':
      container.appendChild(renderYamlHighlighted(body));
      break;
    case 'csv':
      container.appendChild(renderCsvTable(body));
      break;
    case 'protobuf': {
      const pre = document.createElement('pre');
      pre.className = 'syntax-plain';
      pre.innerHTML = `<span class="syntax-comment">// Binary protobuf response</span>\n${escapeHtml(body || '')}`;
      container.appendChild(pre);
      break;
    }
    default: {
      const pre = document.createElement('pre');
      pre.className = 'syntax-plain';
      pre.textContent = body || '';
      container.appendChild(pre);
    }
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
      results: null
    };

    loadedFiles.push(fileEntry);
    const fileIdx = loadedFiles.length - 1;
    detectTelemetryConfig();
    detectAzureAuthNeeded();

    // Populate envVars from .http file variable values (non-placeholders)
    if (suite.variables) {
      suite.variables.forEach(([name, value]) => {
        if (envVars[name] === undefined || envVars[name] === '') {
          if (value && !value.startsWith('your-') && !value.includes('your-')) {
            envVars[name] = value;
          }
        }
      });
    }

    // Sync disabled state from parsed # @disabled directives
    syncDisabledFromSuite(fileIdx);

    renderFileTree();
    renderEnvVars();

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
      const diffs = [];
      const s0 = block.steps[0];
      const s1 = block.steps[1];
      if (s0.request && s1.request) {
        if (s0.request.method !== s1.request.method) diffs.push(`Method: ${s0.request.method} \u2192 ${s1.request.method}`);
        if (s0.request.url !== s1.request.url) diffs.push(`URL: ${escapeHtml(s0.request.url)} \u2192 ${escapeHtml(s1.request.url)}`);
        const h0 = (s0.request.headers || []).map(([k,v]) => `${k}: ${v}`).sort();
        const h1 = (s1.request.headers || []).map(([k,v]) => `${k}: ${v}`).sort();
        const addedH = h1.filter(h => !h0.includes(h));
        const removedH = h0.filter(h => !h1.includes(h));
        if (addedH.length > 0) diffs.push(`Headers added: ${addedH.join(', ')}`);
        if (removedH.length > 0) diffs.push(`Headers removed: ${removedH.join(', ')}`);
        const hasBodyA = !!s0.request.body;
        const hasBodyB = !!s1.request.body;
        if (hasBodyA !== hasBodyB) diffs.push(`Body: ${hasBodyA ? 'present' : 'none'} \u2192 ${hasBodyB ? 'present' : 'none'}`);
        else if (hasBodyA && hasBodyB && s0.request.body !== s1.request.body) diffs.push('Body: different content');
      }
      if (diffs.length > 0) {
        html += `<div class="btt-section"><div class="btt-section-title">Request Differences</div>`;
        diffs.forEach(d => { html += `<div class="btt-diff-item">\u0394 ${d}</div>`; });
        html += `</div>`;
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

    const header = document.createElement('div');
    header.className = 'file-node-header';
    header.innerHTML = `
      <span class="node-chevron">\u25B8</span>
      <span class="node-icon">\uD83D\uDCC4</span>
      <span class="node-name" title="${escapeAttr(file.name)}">${escapeHtml(file.name)}</span>
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
}

// --- Block Selection ---
function selectBlock(fileIdx, blockIdx) {
  activeFileIndex = fileIdx;
  activeBlockIndex = blockIdx;

  const file = loadedFiles[fileIdx];
  if (!file) return;
  const block = file.suite.blocks[blockIdx];
  if (!block) return;

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
      const isJson = req.body.trim().startsWith('{') || req.body.trim().startsWith('[');
      bodyType.value = isJson ? 'json' : 'text';
      bodyInput.value = req.body;
      bodyInput.disabled = false;
    } else {
      bodyType.value = 'none';
      bodyInput.value = '';
      bodyInput.disabled = true;
    }
    bodyType.dispatchEvent(new Event('change'));
  }

  // Show response if block has been run
  const br = file.results?.block_results?.[blockIdx];
  if (block.compare && br?.step_results?.length > 0) {
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
  const isCompare = block.compare && block.steps && block.steps.length > 0;
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

  // Compare block: render per-step + diff
  if (block.compare && block.steps && block.steps.length > 0) {
    // Per-step results
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

    // Diff summary
    const diff = br?.diff_result;
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

      // Changed paths detail
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

      // Added/removed paths
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
      html += `<button class="diff-view-btn" onclick="openDiffViewer(${fileIdx}, ${blockIdx})">🔍 View Full Diff</button>`;
    }

    // Comparison assertions (stored on block.assertions for compare blocks)
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
  // If already running, abort
  if (isRunning) {
    if (abortRunController) abortRunController.abort();
    return;
  }

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

    // Inject ARM token into telemetry_token variable for files with resource ID telemetry
    const armCached = azureTokenCache.get('https://management.azure.com/.default');
    if (armCached && Date.now() < armCached.expiresAt - 60000) {
      for (const f of loadedFiles) {
        const tokenVar = f.suite.telemetry_token;
        if (tokenVar) {
          azureExtraVars.push([tokenVar, armCached.token]);
        }
      }
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

  try {
    for (let fi = 0; fi < loadedFiles.length; fi++) {
      if (signal.aborted) {
        showToast('Run stopped', 'info');
        break;
      }

      const file = loadedFiles[fi];
      const suite = getEnabledSuite(fi);
      if (!suite || suite.blocks.length === 0) continue;
      try {
        const fileExtraVars = [...collectVariablesArray(), ...azureExtraVars];
        if (file.suite.telemetry_var && telemetryEnabled) {
          fileExtraVars.push(['__telemetry_file', file.name]);
        }
        // Strip telemetry fields when disabled so Rust runner skips telemetry init
        const suiteCopy = telemetryEnabled ? suite : { ...suite, telemetry_var: null, telemetry_token: null, telemetry_service: null };
        const results = await invoke('run_test_suite', {
          suite: suiteCopy,
          extraVariables: fileExtraVars,
          extraHeaders: getExtraHeaders(),
          runMode,
          fileName: file.name,
        });

        // Remap results to align with original block indices
        file.results = remapResults(fi, results);
        pushRunHistory(file.name, results);
        processTelemetryResult(file.name, results);
        totalPassed += results.passed;
        totalFailed += results.failed;
        totalSkipped += results.skipped;
        totalTimeMs += results.total_time_ms;

        // Carry extracted variables forward to next files
        if (results.final_variables) {
          Object.entries(results.final_variables).forEach(([name, value]) => {
            envVars[name] = value;
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
    const results = await invoke('run_test_suite', {
      suite: singleSuite,
      extraVariables: extraVars,
      extraHeaders: getExtraHeaders(),
      runMode: azureAuthState === 'authenticated' ? 'dev' : null,
      fileName: file.name,
    });

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
      // Carry extracted variables to env
      if (results.final_variables) {
        Object.entries(results.final_variables).forEach(([name, value]) => {
          envVars[name] = value;
        });
      }
    }

    updateBlockStatuses();
    renderEnvVars();

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
    const results = await invoke('run_test_suite', {
      suite: singleSuite,
      extraVariables: extraVars,
      extraHeaders: getExtraHeaders(),
      runMode: azureAuthState === 'authenticated' ? 'dev' : null,
      fileName: file.name,
    });

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
          envVars[name] = value;
        });
      }
    }

    updateBlockStatuses();
    renderFileTree();
    renderEnvVars();

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
      const isJson = step.request.body.trim().startsWith('{') || step.request.body.trim().startsWith('[');
      bodyType.value = isJson ? 'json' : 'text';
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
}

// Toggle test results details
testResultsSummary.addEventListener('click', () => {
  testResultsDetails.classList.toggle('hidden');
});

// --- Env File Management ---

function renderEnvVars() {
  // Collect all variables defined across all loaded .http files
  const definedVars = {};
  loadedFiles.forEach(file => {
    if (file.suite.variables) {
      file.suite.variables.forEach(([name, value]) => {
        definedVars[name] = value;
      });
    }
  });

  // Merge with env vars (env overrides .http defaults)
  const merged = {};
  Object.entries(definedVars).forEach(([name, httpDefault]) => {
    const envVal = envVars[name];
    if (envVal !== undefined && envVal !== '') {
      merged[name] = { value: envVal, source: 'env', status: 'set' };
    } else {
      const isPlaceholder = httpDefault.startsWith('your-') || httpDefault === '' || httpDefault.includes('your-');
      merged[name] = { value: httpDefault, source: 'http', status: isPlaceholder ? 'empty' : 'set' };
    }
  });

  // Also show env vars not in any .http file
  Object.entries(envVars).forEach(([name, value]) => {
    if (!merged[name]) {
      merged[name] = { value, source: 'env', status: value ? 'set' : 'empty' };
    }
  });

  const entries = Object.entries(merged);

  if (entries.length === 0) {
    envList.innerHTML = '<div class="sidebar-empty">No .env file loaded</div>';
    return;
  }

  // Show env file path if loaded
  let html = '';
  if (envFilePath) {
    const shortPath = envFilePath.split(/[/\\]/).pop();
    html += `<div class="env-file-path" title="${escapeAttr(envFilePath)}">\uD83D\uDCC4 ${escapeHtml(shortPath)}</div>`;
  }

  envList.innerHTML = html;

  entries.forEach(([name, v]) => {
    const row = document.createElement('div');
    row.className = 'var-row';

    const statusColor = v.status === 'set' ? 'var(--green)' : 'var(--red)';
    const statusTitle = v.status === 'set' ? 'Value set' : 'Value not set';
    const sourceIcon = v.source === 'env' ? '\uD83D\uDD10' : '\uD83D\uDCC4';
    const sourceTitle = v.source === 'env' ? 'From .env file' : 'Default from .http file';

    row.innerHTML = `
      <span class="var-status" style="color:${statusColor}" title="${statusTitle}">\u25CF</span>
      <span class="var-icon" title="${sourceTitle}">${sourceIcon}</span>
      <input class="var-name" value="${escapeAttr(name)}" spellcheck="false" data-old-name="${escapeAttr(name)}">
      <span class="var-sep">=</span>
      <input class="var-value" value="${escapeAttr(v.value)}" spellcheck="false" placeholder="enter value..." data-var-name="${escapeAttr(name)}">
      <button class="var-delete" title="Delete variable">\u2715</button>
    `;

    const nameInput = row.querySelector('.var-name');
    const valueInput = row.querySelector('.var-value');
    const deleteBtn = row.querySelector('.var-delete');

    nameInput.addEventListener('change', () => {
      const oldName = nameInput.dataset.oldName;
      const newName = nameInput.value.trim();
      if (newName && newName !== oldName) {
        envVars[newName] = envVars[oldName] !== undefined ? envVars[oldName] : v.value;
        delete envVars[oldName];
        renderEnvVars();
      }
    });

    valueInput.addEventListener('change', () => {
      envVars[name] = valueInput.value;
      renderEnvVars();
    });

    deleteBtn.addEventListener('click', () => {
      delete envVars[name];
      renderEnvVars();
    });

    // Variable hover tooltip
    row.addEventListener('mouseenter', () => {
      const varValue = v.value || '';
      if (!varValue) return;
      clearTimeout(showTooltipTimer);
      clearTimeout(hideTooltipTimer);
      showTooltipTimer = setTimeout(() => {
        blockTooltip.innerHTML = buildVarTooltipHtml(name, varValue);
        blockTooltip.classList.remove('hidden');
        blockTooltip.style.display = 'block';
        positionBlockTooltip(row);
      }, 300);
    });

    row.addEventListener('mouseleave', () => {
      hideBlockTooltip();
    });

    envList.appendChild(row);
  });

  // Add built-in variables at bottom
  const builtins = ['$timestamp', '$uuid', '$randomInt'];
  builtins.forEach(name => {
    const row = document.createElement('div');
    row.className = 'var-row builtin';
    row.innerHTML = `
      <span class="var-status" style="color:var(--text-muted)">\u25CF</span>
      <span class="var-icon">\u2699</span>
      <span class="var-name" style="cursor:default;border:none">${name}</span>
      <span class="var-sep">=</span>
      <span class="var-value" style="cursor:default;border:none;font-style:italic">(auto)</span>
    `;
    envList.appendChild(row);
  });
}

// --- Load .env file ---
loadEnvBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  envFileInput.click();
});

envFileInput.addEventListener('change', async (e) => {
  const file = e.target.files[0];
  if (!file) return;

  const content = await file.text();
  try {
    envVars = {};
    content.split('\n').forEach(line => {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) return;
      const eqIdx = trimmed.indexOf('=');
      if (eqIdx > 0) {
        const key = trimmed.substring(0, eqIdx).trim();
        let value = trimmed.substring(eqIdx + 1).trim();
        // Strip quotes
        if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
          value = value.slice(1, -1);
        }
        envVars[key] = value;
      }
    });
    envFilePath = file.name;
    renderEnvVars();
    showToast(`Loaded ${Object.keys(envVars).length} variables from ${file.name}`, 'success');
    rpLog('info', 'Env file loaded: ' + file.name, { varCount: Object.keys(envVars).length });
  } catch (err) {
    showToast(`Failed to load .env: ${err}`, 'error');
    rpLog('error', 'Env file load failed', String(err));
  }
  envFileInput.value = '';
});

// --- Save .env file ---
saveEnvBtn.addEventListener('click', async (e) => {
  e.stopPropagation();

  // Collect all variables: from .http files + env overrides + manually added
  const allVars = {};
  loadedFiles.forEach(file => {
    if (file.suite.variables) {
      file.suite.variables.forEach(([name, value]) => {
        allVars[name] = envVars[name] !== undefined ? envVars[name] : value;
      });
    }
  });
  // Include any env-only vars (manually added or from .env file)
  Object.entries(envVars).forEach(([name, value]) => {
    allVars[name] = value;
  });

  // Generate .env content
  let content = '# Request Pilot environment variables\n# Edit values below and save\n\n';
  Object.entries(allVars).sort(([a], [b]) => a.localeCompare(b)).forEach(([key, value]) => {
    if (value.includes(' ') || value.includes('#') || value.includes('=')) {
      content += `${key}="${value}"\n`;
    } else {
      content += `${key}=${value}\n`;
    }
  });

  // Save via native dialog
  try {
    const defaultName = (envFilePath && envFilePath.endsWith('.env'))
      ? envFilePath.split(/[\\/]/).pop()
      : '.env';
    const path = await invoke('save_file_with_dialog', {
      defaultName,
      content,
      title: 'Save Environment File',
      filters: [['Env Files', 'env']],
    });
    if (!path) return; // cancelled
    envFilePath = path;
    showToast(`Saved to ${path.split(/[\\/]/).pop()}`, 'success');
  } catch (err) {
    showToast('Save failed: ' + err, 'error');
  }
});

addEnvVarBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  const name = `NEW_VAR_${Object.keys(envVars).length + 1}`;
  envVars[name] = '';
  renderEnvVars();
  $('#envSection').classList.add('expanded');
  // Focus the newly added name input
  const inputs = envList.querySelectorAll('.var-name');
  const lastInput = inputs[inputs.length - 1];
  if (lastInput && lastInput.tagName === 'INPUT') {
    lastInput.focus();
    lastInput.select();
  }
});

clearEnvBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  if (Object.keys(envVars).length === 0) {
    showToast('No variables to clear', 'info');
    return;
  }
  envVars = {};
  envFilePath = '';
  renderEnvVars();
  showToast('All variables cleared', 'success');
});

// --- Run Group ---
async function runGroup(fileIdx, groupName) {
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

  try {
    let extraVars = collectVariablesArray();
    if (azureAuthState === 'authenticated') {
      const tokenVars = fetchDevModeToken(file.suite, file.content);
      if (tokenVars.length > 0) extraVars = [...extraVars, ...tokenVars];
    }
    const results = await invoke('run_test_suite', { suite, extraVariables: extraVars, extraHeaders: getExtraHeaders(), runMode: azureAuthState === 'authenticated' ? 'dev' : null, fileName: file.name });

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
      }
    });

    if (results.final_variables) {
      Object.entries(results.final_variables).forEach(([name, value]) => { envVars[name] = value; });
    }

    updateBlockStatuses();
    renderEnvVars();

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

  // Start listening for per-block progress
  await startBlockProgressListener();

  try {
    let extraVars = collectVariablesArray();
    if (azureAuthState === 'authenticated') {
      const tokenVars = fetchDevModeToken(file.suite, file.content);
      if (tokenVars.length > 0) extraVars = [...extraVars, ...tokenVars];
    }
    const results = await invoke('run_test_suite', {
      suite,
      extraVariables: extraVars,
      extraHeaders: getExtraHeaders(),
      runMode: azureAuthState === 'authenticated' ? 'dev' : null,
      fileName: file.name,
    });

    // Remap results to align with original block indices
    file.results = remapResults(fileIdx, results);
    pushRunHistory(file.name, results);

    // Carry extracted variables forward
    if (results.final_variables) {
      Object.entries(results.final_variables).forEach(([name, value]) => {
        envVars[name] = value;
      });
    }

    updateBlockStatuses();
    renderEnvVars();

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

  // Add the missing variables to envVars with empty values so they show up red
  unresolved.forEach(name => {
    if (envVars[name] === undefined) {
      envVars[name] = '';
    }
  });
  renderEnvVars();
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
  return String(str).replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

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
    const ok = await syncCodeToBuilder();
    if (!ok) return;
  }

  // When leaving history mode, close detail panel
  if (currentMode === 'history') {
    closeHistoryDetail();
  }

  // When entering code mode, sync builder to code editor
  if (mode === 'code') {
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

    // Check if this line is an existing # @disabled marker
    if (trimmed === '# @disabled') {
      hasDisabledMarker = true;
      if (needsDisabledMarker) {
        result.push(lines[i]); // keep it
      }
      // else skip it (block was re-enabled)
      continue;
    }

    // If we're at a non-comment, non-empty line after ### and need a disabled marker, inject it
    if (needsDisabledMarker && !hasDisabledMarker && blockIdx >= 0 &&
        trimmed !== '' && !trimmed.startsWith('#') && !trimmed.startsWith('@') && !inVariablesBlock) {
      // Insert # @disabled before the request line
      result.push('# @disabled');
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

async function syncCodeToBuilder() {
  const content = codeEditor.value;
  if (!content.trim()) return true;

  try {
    const suite = await invoke('parse_test_file', { content });

    if (activeFileIndex >= 0 && loadedFiles[activeFileIndex]) {
      loadedFiles[activeFileIndex].suite = suite;
      loadedFiles[activeFileIndex].content = content;
      loadedFiles[activeFileIndex].results = null;
    } else {
      const name = codeEditorFilename.textContent || 'untitled.http';
      loadedFiles.push({ name, content, suite, results: null });
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
    // Separator lines: ###
    if (/^###/.test(line)) {
      inJsonBody = false;
      inVariablesBlock = false;
      const escaped = escapeHtml(line);
      // Check for block types
      const btMatch = escaped.match(/(@(?:setup|test|teardown|variables))\b/);
      if (btMatch) {
        const highlighted = escaped.replace(btMatch[1], `<span class="hl-block-type">${btMatch[1]}</span>`);
        return `<span class="hl-separator">${highlightVariables(highlighted)}</span>`;
      }
      return `<span class="hl-separator">${highlightVariables(escaped)}</span>`;
    }

    // @variables at start of line
    if (/^@variables\b/.test(line)) {
      inJsonBody = false;
      inVariablesBlock = true;
      const escaped = escapeHtml(line);
      return escaped.replace(/^(@variables)/, '<span class="hl-block-type">$1</span>');
    }

    // Variable assignment lines (both "@name = value" and "name = value" inside variables block)
    if (inVariablesBlock && /^\w+\s*=/.test(line)) {
      const escaped = escapeHtml(line);
      return highlightVariables(escaped.replace(/^(\w+)(\s*=\s*)(.*)$/, '<span class="hl-header-name">$1</span><span class="hl-operator">$2</span><span class="hl-header-value">$3</span>'));
    }

    // Directive lines: # @assert, # @extract, # @name, # @description, # @group, # @depends
    if (/^#\s*@(assert|extract|name|description|group|depends)\b/.test(line)) {
      inJsonBody = false;
      const escaped = escapeHtml(line);
      const withOps = escaped.replace(/(==|!=|&gt;=|&lt;=|&gt;|&lt;|contains|matches|exists|isType)/g, '<span class="hl-operator">$1</span>');
      return highlightVariables(`<span class="hl-directive">${withOps}</span>`);
    }

    // Disabled directive: # @disabled
    if (/^#\s*@disabled\s*$/.test(line)) {
      inJsonBody = false;
      return `<span class="hl-disabled">${escapeHtml(line)}</span>`;
    }

    // Comment lines: # (but not directives)
    if (/^#/.test(line)) {
      inJsonBody = false;
      return `<span class="hl-comment">${highlightVariables(escapeHtml(line))}</span>`;
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

    // Variable assignment lines like @baseUrl = ...
    if (/^@\w+/.test(line)) {
      const escaped = escapeHtml(line);
      return highlightVariables(escaped.replace(/^(@\w+)(\s*=\s*)(.*)$/, '<span class="hl-header-name">$1</span><span class="hl-operator">$2</span><span class="hl-header-value">$3</span>'));
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

// --- Code Editor Events ---
codeEditor.addEventListener('input', () => {
  codeEditorModified = codeEditor.value !== codeEditorContent;
  codeEditor.classList.toggle('modified', codeEditorModified);
  updateHighlight();
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
  await syncCodeToBuilder();
  showToast('File saved', 'success');
});

codeRevertBtn.addEventListener('click', () => {
  codeEditor.value = codeEditorContent;
  codeEditorModified = false;
  codeEditor.classList.remove('modified');
  updateHighlight();
  showToast('Reverted to last saved', 'info');
});

// --- New File ---
newFileBtn.addEventListener('click', () => {
  const template = `### @variables
@baseUrl = https://api.example.com

### @test Health Check
GET {{baseUrl}}/health

# @assert response.status == 200
`;

  const name = 'untitled.http';

  invoke('parse_test_file', { content: template }).then(parsedSuite => {
    loadedFiles.push({ name, content: template, suite: parsedSuite, results: null });
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
    let entries = hasFilter
      ? await invoke('get_filtered_history', { filter })
      : await invoke('get_history');
    // Rebuild tree filter options from full dataset, then apply selection
    rebuildTreeFilter(entries);
    entries = applyTreeFilter(entries);
    historyCache = entries;
    renderHistoryStats(historyCache);
    renderHistoryLog(historyCache);
    historyCountBadge.textContent = historyCache.length;
    rpLog('debug', 'History loaded', { count: historyCache.length });
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
  const timeColor = entry.response_time_ms < 200 ? 'var(--green)' : entry.response_time_ms < 500 ? 'var(--orange)' : 'var(--red)';
  const viewedIcon = viewState === 'seen' ? '<span class="hist-viewed" title="Viewed">👁</span>'
    : viewState === 'seen-mutated' ? '<span class="hist-viewed mutated" title="Changed since last viewed">👁✱</span>'
    : '';
  row.innerHTML = `
    <input type="checkbox" class="hist-entry-checkbox" data-seq="${entry.seq}" title="Select for comparison">
    <span class="hist-entry-seq">#${entry.seq}</span>
    <span class="hist-entry-status ${statusClass}">${entry.status || 'ERR'}</span>
    <span class="hist-entry-method method-${entry.method}">${entry.method}</span>
    <span class="hist-entry-url" title="${escapeAttr(entry.url)}">${escapeHtml(truncateUrl(entry.url))}</span>
    <span class="hist-entry-time" style="color:${timeColor}">${entry.response_time_ms}ms</span>
    ${sourceBadge}
    ${blockName}
    <span class="hist-entry-timestamp">${formatHistoryTime(entry.timestamp)}</span>
    ${viewedIcon}
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

function showHistoryDetail(entry) {
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
  let reqBody = entry.request_body || '(no body)';
  try { if (entry.request_body) reqBody = JSON.stringify(JSON.parse(entry.request_body), null, 2); } catch { /* non-critical: JSON format */ }
  historyDetailBody.innerHTML = `
    <div class="hist-detail-url">
      <span class="hist-entry-method method-${entry.method}">${entry.method}</span>
      <span class="hist-detail-full-url">${escapeHtml(entry.url)}</span>
    </div>
    <div class="hist-detail-meta">
      <span class="hist-entry-seq">#${entry.seq}</span>
      <span class="hist-source-badge ${entry.source === 'test-run' ? 'test-run' : entry.source === 'extension-live' ? 'extension-live' : 'manual'}">${entry.source === 'test-run' ? 'Test Run' : entry.source === 'extension-live' ? '📡 Live' : 'Manual'}</span>
      ${entry.block_name ? `<span class="hist-block-name">${escapeHtml(entry.block_name)}</span>` : ''}
      <span class="hist-entry-timestamp">${formatHistoryTime(entry.timestamp)}</span>
      ${entry.run_id ? `<span class="hist-run-id" title="Run ID: ${escapeAttr(entry.run_id)}">🔗 Run</span>` : ''}
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Request Headers</div>
      ${reqHeaders ? `<table class="hist-detail-headers"><thead><tr><th>Header</th><th>Value</th></tr></thead><tbody>${reqHeaders}</tbody></table>` : '<div class="hist-detail-empty">No headers</div>'}
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Request Body</div>
      <pre class="hist-detail-body-pre">${escapeHtml(reqBody)}</pre>
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Response Headers</div>
      ${respHeaders ? `<table class="hist-detail-headers"><thead><tr><th>Header</th><th>Value</th></tr></thead><tbody>${respHeaders}</tbody></table>` : '<div class="hist-detail-empty">No headers</div>'}
    </div>
    <div class="hist-detail-section">
      <div class="hist-detail-section-title">Response Body</div>
      <div class="hist-detail-resp-body"></div>
    </div>`;
  const histRespBodyContainer = historyDetailBody.querySelector('.hist-detail-resp-body');
  if (entry.response_body) {
    renderResponseBodyInto(histRespBodyContainer, entry.response_body, entry.response_headers || []);
  } else {
    histRespBodyContainer.innerHTML = '<pre class="hist-detail-body-pre">(no body)</pre>';
  }
}

function closeHistoryDetail() {
  historyDetailOverlay.classList.add('hidden');
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

function openHistoryComparison() {
  if (historySelectedIds.size < 2) return;
  const [seqA, seqB] = [...historySelectedIds];
  const entryA = historyCache.find(e => e.seq === seqA);
  const entryB = historyCache.find(e => e.seq === seqB);
  if (entryA && entryB) openComparisonOverlay(entryA, entryB);
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

// Clear viewed state for history
$('#historyClearViewedBtn').addEventListener('click', () => {
  viewedHistoryEntries.clear();
  historyLog.querySelectorAll('.hist-entry-row').forEach(r => {
    r.classList.remove('seen', 'seen-mutated');
    const v = r.querySelector('.hist-viewed');
    if (v) v.remove();
  });
});

// Clear viewed state for test results
$('#clearViewedBtn').addEventListener('click', () => {
  viewedResults.clear();
  document.querySelectorAll('.result-detail-row').forEach(r => {
    r.classList.remove('seen', 'seen-mutated');
    const v = r.querySelector('.detail-viewed');
    if (v) v.remove();
  });
});

$('#historyClearBtn').addEventListener('click', async () => {
  if (historyCache.length === 0) {
    showToast('No history to clear', 'info');
    return;
  }
  try {
    await invoke('clear_history');
    historyCache = [];
    historyExpandedGroups.clear();
    renderHistoryStats([]);
    renderHistoryLog([]);
    historyCountBadge.textContent = '0';
    showToast('History cleared', 'success');
    rpLog('info', 'History cleared');
  } catch (err) {
    showToast(`Failed to clear history: ${err}`, 'error');
    rpLog('error', 'Command failed: clear_history', String(err));
  }
});

$('#historyDetailCloseBtn').addEventListener('click', closeHistoryDetail);
historyDetailOverlay.addEventListener('click', (e) => {
  if (e.target === historyDetailOverlay) closeHistoryDetail();
});

// --- Diff Viewer ---
function openDiffViewer(fileIdx, blockIdx) {
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

  if (diff.is_json && diff.changed_paths?.length > 0) {
    leftCode.innerHTML = highlightWithDiffMarkers(formattedA, highlightFn, diff, 'left');
    rightCode.innerHTML = highlightWithDiffMarkers(formattedB, highlightFn, diff, 'right');
  } else {
    leftCode.innerHTML = highlightFn(escapeHtml(formattedA));
    rightCode.innerHTML = highlightFn(escapeHtml(formattedB));
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

function highlightWithDiffMarkers(text, highlightFn, diff, side) {
  let highlighted = highlightFn(escapeHtml(text));
  const lines = highlighted.split('\n');
  const changedKeys = new Set();

  if (diff.changed_paths) {
    diff.changed_paths.forEach(cp => {
      const parts = cp.path.split('.');
      changedKeys.add(parts[parts.length - 1]);
    });
  }
  if (side === 'left' && diff.removed_paths) {
    diff.removed_paths.forEach(p => {
      const parts = p.split('.');
      changedKeys.add(parts[parts.length - 1]);
    });
  }
  if (side === 'right' && diff.added_paths) {
    diff.added_paths.forEach(p => {
      const parts = p.split('.');
      changedKeys.add(parts[parts.length - 1]);
    });
  }

  const markedLines = lines.map(line => {
    const isChanged = Array.from(changedKeys).some(key => line.includes(key));
    if (isChanged) {
      const markerClass = side === 'left' ? 'diff-line-removed' : 'diff-line-added';
      return `<span class="${markerClass}">${line}</span>`;
    }
    return line;
  });

  return markedLines.join('\n');
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
  // Ctrl+Enter -> Send
  if (e.ctrlKey && !e.shiftKey && e.key === 'Enter') {
    e.preventDefault();
    sendRequest();
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
      syncCodeToBuilder();
      showToast('File saved', 'success');
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

  // Auto-add monitor ingestion scope if any file uses telemetry with a resource ID
  for (const f of loadedFiles) {
    if (f.suite.telemetry_var) {
      // Check if the variable looks like a resource ID (will need monitor token for OTLP)
      const tVal = f.suite.variables?.find(([k]) => k === f.suite.telemetry_var)?.[1] || '';
      if (tVal.startsWith('/subscriptions/') || f.suite.telemetry_token) {
        scopes.add('https://monitor.azure.com/.default');
        break;
      }
    }
  }
  
  azureAuthScopes = [...scopes]; // Store needed scopes
  
  // Log if monitor scope was auto-added for telemetry
  if (azureAuthScopes.includes('https://monitor.azure.com/.default')) {
    rpLog('info', 'Azure auth: monitor.azure.com scope auto-added for OTEL ingestion');
  }

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
    if (!confirm('You are already authenticated. Re-authenticate?')) return;
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
    if (envVars[name] && !envVars[name].startsWith('your-')) return envVars[name];
    for (const f of loadedFiles) {
      const v = f.suite.variables.find(([k]) => k === name);
      if (v?.[1] && !v[1].startsWith('your-')) return v[1];
    }
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
  
  // Fallback: parse raw content to match @dev_auth scopes with @extract variables
  if (tokens.length === 0 && fileContent) {
    const blocks = fileContent.split(/^###/m);
    for (const rawBlock of blocks) {
      const devAuthMatch = rawBlock.match(/^#\s*@dev_auth\s+(.+)$/m);
      if (!devAuthMatch) continue;
      const scope = devAuthMatch[1].trim();
      const cached = azureTokenCache.get(scope);
      if (!cached || Date.now() >= cached.expiresAt - 60000) continue;
      const extractMatches = rawBlock.matchAll(/^#\s*@extract\s+(\w+)\s*=\s*.+$/gm);
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
    if (f.suite.telemetry_var) {
      hasTelemetry = true;
      telemetryFiles.push(f.name);
    } else if (f.content && /^#\s*@telemetry\s+\S/m.test(f.content)) {
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
      // Check if Azure auth already has monitor token (e.g. auth happened before file load)
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
    const statusText = hasAzureAuth ? 'Configured — awaiting monitor.azure.com token' : 'Configured — awaiting Azure auth';
    html += '<div class="tt-row"><span class="tt-key">Status</span><span class="tt-val">' + statusText + '</span></div>';
    // Show which files have telemetry
    const telFiles = loadedFiles.filter(f => f.suite.telemetry_var);
    if (telFiles.length > 0) {
      html += '<div class="tt-section"><div class="tt-section-title">Configured Files</div>';
      for (const f of telFiles) {
        const varName = f.suite.telemetry_var;
        const varVal = f.suite.variables?.find(([k]) => k === varName)?.[1] || '(empty)';
        const display = varVal.length > 50 ? varVal.slice(0, 40) + '…' : varVal;
        html += '<div class="tt-row"><span class="tt-key">' + escHtml(f.name) + '</span><span class="tt-val" title="' + escHtml(varVal) + '">' + escHtml(display) + '</span></div>';
      }
      html += '</div>';
    }
    const nextStep = hasAzureAuth
      ? 'Re-authenticate to include monitor.azure.com scope'
      : 'Click Azure Auth — monitor.azure.com scope will be auto-added';
    html += '<div class="tt-section"><div class="tt-section-title">Next Step</div><div style="color: var(--text-muted); font-size: 11px;">' + nextStep + '</div></div>';
  } else if (telemetryState === 'ready') {
    html += '<div class="tt-row"><span class="tt-key">Status</span><span class="tt-val success">Ready — authenticated</span></div>';
    const telFiles = loadedFiles.filter(f => f.suite.telemetry_var);
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
renderEnvVars();
initAzureAuth();

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
  block += `# @name ${label}\n`;
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
