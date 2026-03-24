/* ============================================================
   Request Pilot — Comparison & Similarity Module
   Standalone functions for request comparison overlay and
   similarity scoring. Loaded before app.js.
   ============================================================ */

/* ---------- Helpers ---------- */
// escapeHtml() is defined in app.js (loaded after this file).
// All functions here are called at runtime, after both scripts load.

function formatCompareTime(isoString) {
  try { return new Date(isoString).toLocaleString(); }
  catch { return isoString || '—'; }
}

function formatBodyForCompare(bodyStr) {
  if (!bodyStr) return '(empty)';
  try {
    const parsed = JSON.parse(bodyStr);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return bodyStr;
  }
}

/* ---------- Headers Diff ---------- */

/**
 * Build a diff table comparing two header arrays of [name, value] tuples.
 * Returns HTML string for a <table>.
 */
function buildHeadersDiff(headersA, headersB) {
  const mapA = new Map((headersA || []).map(([n, v]) => [n.toLowerCase(), { name: n, value: v }]));
  const mapB = new Map((headersB || []).map(([n, v]) => [n.toLowerCase(), { name: n, value: v }]));

  const allKeys = new Set([...mapA.keys(), ...mapB.keys()]);
  const rows = [];

  for (const key of [...allKeys].sort()) {
    const a = mapA.get(key);
    const b = mapB.get(key);

    if (a && b) {
      if (a.value === b.value) {
        rows.push({ cls: '', indicator: '=', indicatorCls: '', name: a.name, valA: a.value, valB: b.value });
      } else {
        rows.push({ cls: 'diff-changed', indicator: '≠', indicatorCls: 'changed', name: a.name, valA: a.value, valB: b.value });
      }
    } else if (a && !b) {
      rows.push({ cls: 'diff-removed', indicator: '−', indicatorCls: 'removed', name: a.name, valA: a.value, valB: '' });
    } else {
      rows.push({ cls: 'diff-added', indicator: '+', indicatorCls: 'added', name: b.name, valA: '', valB: b.value });
    }
  }

  if (rows.length === 0) {
    return '<p class="compare-empty">No headers</p>';
  }

  let html = `<table class="compare-headers-table">
    <thead><tr><th></th><th>Header</th><th>Request A</th><th>Request B</th></tr></thead><tbody>`;
  for (const r of rows) {
    html += `<tr class="${r.cls}">
      <td><span class="diff-indicator ${r.indicatorCls}">${r.indicator}</span></td>
      <td class="header-name-cell">${escapeHtml(r.name)}</td>
      <td>${escapeHtml(r.valA)}</td>
      <td>${escapeHtml(r.valB)}</td>
    </tr>`;
  }
  html += '</tbody></table>';
  return html;
}

/* ---------- JSON Diff ---------- */

/**
 * Flatten a JSON object into key-value pairs with dot-notation keys.
 */
function flattenJson(obj, prefix) {
  const result = {};
  if (obj === null || obj === undefined || typeof obj !== 'object') {
    result[prefix || '(root)'] = obj;
    return result;
  }
  if (Array.isArray(obj)) {
    if (obj.length === 0) {
      result[prefix || '(root)'] = '[]';
    } else {
      obj.forEach((item, i) => {
        Object.assign(result, flattenJson(item, prefix ? `${prefix}[${i}]` : `[${i}]`));
      });
    }
    return result;
  }
  const keys = Object.keys(obj);
  if (keys.length === 0) {
    result[prefix || '(root)'] = '{}';
  }
  for (const k of keys) {
    const path = prefix ? `${prefix}.${k}` : k;
    Object.assign(result, flattenJson(obj[k], path));
  }
  return result;
}

function buildDiffRow(fullKey, valA, valB) {
  const strA = valA === undefined ? '' : JSON.stringify(valA);
  const strB = valB === undefined ? '' : JSON.stringify(valB);

  let cls = '', indicator = '=', indicatorCls = '';
  if (valA === undefined) {
    cls = 'diff-added'; indicator = '+'; indicatorCls = 'added';
  } else if (valB === undefined) {
    cls = 'diff-removed'; indicator = '−'; indicatorCls = 'removed';
  } else if (strA !== strB) {
    cls = 'diff-changed'; indicator = '≠'; indicatorCls = 'changed';
  }

  return `<tr class="${cls}">
    <td><span class="diff-indicator ${indicatorCls}">${indicator}</span></td>
    <td class="header-name-cell">${escapeHtml(fullKey)}</td>
    <td>${escapeHtml(strA)}</td>
    <td>${escapeHtml(strB)}</td>
  </tr>`;
}

/**
 * Build diff rows comparing two JSON values recursively.
 * Returns array of HTML <tr> strings.
 */
function buildJsonDiffRows(objA, objB, prefix) {
  const flatA = flattenJson(objA, prefix || '');
  const flatB = flattenJson(objB, prefix || '');
  const allKeys = new Set([...Object.keys(flatA), ...Object.keys(flatB)]);
  const rows = [];
  for (const key of [...allKeys].sort()) {
    rows.push(buildDiffRow(key, flatA[key], flatB[key]));
  }
  return rows;
}

/**
 * Build a full JSON diff table for two body strings.
 * Returns HTML string or empty if not both JSON.
 */
function buildJsonDiff(bodyA, bodyB) {
  let objA, objB;
  try { objA = JSON.parse(bodyA); } catch { return ''; }
  try { objB = JSON.parse(bodyB); } catch { return ''; }

  const rows = buildJsonDiffRows(objA, objB, '');
  if (rows.length === 0) return '<p class="compare-empty">Empty JSON</p>';

  return `<table class="compare-headers-table">
    <thead><tr><th></th><th>Key Path</th><th>Value A</th><th>Value B</th></tr></thead>
    <tbody>${rows.join('')}</tbody></table>`;
}

/* ---------- Comparison Overlay ---------- */

function buildGeneralInfoColumn(entry, label) {
  return `<div class="compare-column">
    <div class="compare-column-label">${escapeHtml(label)}</div>
    <div class="compare-info-row"><span class="compare-info-label">URL</span><span class="compare-info-value">${escapeHtml(entry.url)}</span></div>
    <div class="compare-info-row"><span class="compare-info-label">Method</span><span class="compare-info-value">${escapeHtml(entry.method)}</span></div>
    <div class="compare-info-row"><span class="compare-info-label">Timestamp</span><span class="compare-info-value">${formatCompareTime(entry.timestamp)}</span></div>
    <div class="compare-info-row"><span class="compare-info-label">Duration</span><span class="compare-info-value">${entry.response_time_ms != null ? entry.response_time_ms + ' ms' : '—'}</span></div>
    <div class="compare-info-row"><span class="compare-info-label">Status</span><span class="compare-info-value">${entry.status != null ? entry.status : '—'}</span></div>
    <div class="compare-info-row"><span class="compare-info-label">Source</span><span class="compare-info-value">${escapeHtml(entry.source || '—')}</span></div>
  </div>`;
}

function buildRequestTabContent(entryA, entryB) {
  let html = '';

  // General Info
  html += `<div class="compare-section">
    <div class="compare-section-title">General Info</div>
    <div class="compare-grid">
      ${buildGeneralInfoColumn(entryA, 'Request A')}
      ${buildGeneralInfoColumn(entryB, 'Request B')}
    </div>
  </div>`;

  // Request Headers Diff
  html += `<div class="compare-section">
    <div class="compare-section-title">Request Headers Diff</div>
    ${buildHeadersDiff(entryA.request_headers, entryB.request_headers)}
  </div>`;

  // Request Body
  const bodyA = formatBodyForCompare(entryA.request_body);
  const bodyB = formatBodyForCompare(entryB.request_body);
  html += `<div class="compare-section">
    <div class="compare-section-title">Request Body</div>
    <div class="compare-grid">
      <div class="compare-column">
        <div class="compare-column-label">Request A</div>
        <pre class="compare-body-pre">${escapeHtml(bodyA)}</pre>
      </div>
      <div class="compare-column">
        <div class="compare-column-label">Request B</div>
        <pre class="compare-body-pre">${escapeHtml(bodyB)}</pre>
      </div>
    </div>
  </div>`;

  // JSON Diff for request body
  if (entryA.request_body && entryB.request_body) {
    const jsonDiff = buildJsonDiff(entryA.request_body, entryB.request_body);
    if (jsonDiff) {
      html += `<div class="compare-section">
        <div class="compare-section-title">Request Body — JSON Diff</div>
        ${jsonDiff}
      </div>`;
    }
  }

  return html;
}

function buildResponseStatusColumn(entry, label) {
  return `<div class="compare-column">
    <div class="compare-column-label">${escapeHtml(label)}</div>
    <div class="compare-info-row"><span class="compare-info-label">Status</span><span class="compare-info-value">${entry.status != null ? entry.status : '—'}</span></div>
    <div class="compare-info-row"><span class="compare-info-label">Time</span><span class="compare-info-value">${entry.response_time_ms != null ? entry.response_time_ms + ' ms' : '—'}</span></div>
    <div class="compare-info-row"><span class="compare-info-label">Size</span><span class="compare-info-value">${entry.response_size_bytes != null ? entry.response_size_bytes + ' B' : '—'}</span></div>
  </div>`;
}

function buildResponseTabContent(entryA, entryB) {
  let html = '';

  // Status Comparison
  html += `<div class="compare-section">
    <div class="compare-section-title">Status Comparison</div>
    <div class="compare-grid">
      ${buildResponseStatusColumn(entryA, 'Response A')}
      ${buildResponseStatusColumn(entryB, 'Response B')}
    </div>
  </div>`;

  // Response Headers Diff
  html += `<div class="compare-section">
    <div class="compare-section-title">Response Headers Diff</div>
    ${buildHeadersDiff(entryA.response_headers, entryB.response_headers)}
  </div>`;

  // Large body guard — show deferred load button instead of rendering
  const largeInfo = entryA._large_body_info || entryB._large_body_info;
  if (largeInfo) {
    const sizeA = (entryA._large_body_info || entryB._large_body_info).size || 0;
    const sizeB = (entryB._large_body_info || entryA._large_body_info).size || 0;
    const stored = (entryA._large_body_info || entryB._large_body_info).stored;

    html += `<div class="compare-section" id="compareResponseBodySection">
      <div class="compare-section-title">Response Body</div>
      <div class="body-truncated-info" style="padding:24px; text-align:center;">
        <div style="margin-bottom:12px;">
          <span style="font-size:24px;">⚠</span>
        </div>
        <div style="margin-bottom:8px; color:var(--text-primary); font-weight:600;">
          Large Response Bodies
        </div>
        <div style="margin-bottom:16px; color:var(--text-muted); font-size:12px;">
          Response A: ${typeof formatBytes === 'function' ? formatBytes(sizeA) : sizeA + ' B'} · Response B: ${typeof formatBytes === 'function' ? formatBytes(sizeB) : sizeB + ' B'}
        </div>
        <button class="btn btn-primary btn-sm" id="compareLoadBodies">📄 Load & Compare Response Bodies</button>
        <div style="margin-top:8px; color:var(--text-muted); font-size:11px;">
          This may take a moment for large responses
        </div>
      </div>
    </div>`;

    // Wire up the load button after DOM insertion via setTimeout
    setTimeout(() => {
      const loadBtn = document.getElementById('compareLoadBodies');
      if (!loadBtn) return;
      loadBtn.addEventListener('click', () => {
        loadBtn.textContent = '⏳ Rendering…';
        loadBtn.disabled = true;
        setTimeout(() => {
          // Restore bodies and re-render the section
          const bodyA = formatBodyForCompare(stored.a);
          const bodyB = formatBodyForCompare(stored.b);
          const section = document.getElementById('compareResponseBodySection');
          if (!section) return;

          let innerHtml = `<div class="compare-section-title">Response Body</div>
            <div class="compare-grid">
              <div class="compare-column">
                <div class="compare-column-label">Response A</div>
                <pre class="compare-body-pre">${escapeHtml(bodyA)}</pre>
              </div>
              <div class="compare-column">
                <div class="compare-column-label">Response B</div>
                <pre class="compare-body-pre">${escapeHtml(bodyB)}</pre>
              </div>
            </div>`;

          // JSON Diff for response body
          if (stored.a && stored.b) {
            const jsonDiff = buildJsonDiff(stored.a, stored.b);
            if (jsonDiff) {
              innerHtml += `<div class="compare-section" style="margin-top:16px;">
                <div class="compare-section-title">Response Body — JSON Diff</div>
                ${jsonDiff}
              </div>`;
            }
          }

          section.innerHTML = innerHtml;
        }, 50);
      });
    }, 0);

    return html;
  }

  // Response Body
  const bodyA = formatBodyForCompare(entryA.response_body);
  const bodyB = formatBodyForCompare(entryB.response_body);
  html += `<div class="compare-section">
    <div class="compare-section-title">Response Body</div>
    <div class="compare-grid">
      <div class="compare-column">
        <div class="compare-column-label">Response A</div>
        <pre class="compare-body-pre">${escapeHtml(bodyA)}</pre>
      </div>
      <div class="compare-column">
        <div class="compare-column-label">Response B</div>
        <pre class="compare-body-pre">${escapeHtml(bodyB)}</pre>
      </div>
    </div>
  </div>`;

  // JSON Diff for response body
  if (entryA.response_body && entryB.response_body) {
    const jsonDiff = buildJsonDiff(entryA.response_body, entryB.response_body);
    if (jsonDiff) {
      html += `<div class="compare-section">
        <div class="compare-section-title">Response Body — JSON Diff</div>
        ${jsonDiff}
      </div>`;
    }
  }

  return html;
}

/**
 * Open the comparison overlay for two history entries.
 */
function openComparisonOverlay(entryA, entryB) {
  const overlay = document.getElementById('compareOverlay');
  if (!overlay) return;

  // Populate tabs
  const reqTab = document.getElementById('compareTabRequest');
  const respTab = document.getElementById('compareTabResponse');

  if (reqTab) reqTab.innerHTML = buildRequestTabContent(entryA, entryB);
  if (respTab) respTab.innerHTML = buildResponseTabContent(entryA, entryB);

  // Reset to Request tab
  overlay.querySelectorAll('.compare-tab-btn').forEach(btn => {
    btn.classList.toggle('active', btn.dataset.ctab === 'request');
  });
  overlay.querySelectorAll('.compare-tab-content').forEach(el => {
    el.classList.toggle('active', el.id === 'compareTabRequest');
    el.classList.toggle('hidden', el.id !== 'compareTabRequest');
  });

  overlay.classList.remove('hidden');
}

/**
 * Close the comparison overlay.
 */
function closeComparisonOverlay() {
  const overlay = document.getElementById('compareOverlay');
  if (overlay) overlay.classList.add('hidden');
}

/* ---------- Overlay Event Wiring ---------- */

document.addEventListener('DOMContentLoaded', () => {
  // Close button
  const closeBtn = document.getElementById('btnCloseCompare');
  if (closeBtn) closeBtn.addEventListener('click', closeComparisonOverlay);

  // Tab switching
  const overlay = document.getElementById('compareOverlay');
  if (overlay) {
    overlay.addEventListener('click', (e) => {
      const tabBtn = e.target.closest('.compare-tab-btn');
      if (!tabBtn) return;
      const tab = tabBtn.dataset.ctab;

      overlay.querySelectorAll('.compare-tab-btn').forEach(b => b.classList.toggle('active', b === tabBtn));
      overlay.querySelectorAll('.compare-tab-content').forEach(el => {
        const isTarget = (tab === 'request' && el.id === 'compareTabRequest') ||
                         (tab === 'response' && el.id === 'compareTabResponse');
        el.classList.toggle('active', isTarget);
        el.classList.toggle('hidden', !isTarget);
      });
    });

    // Escape key closes overlay
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !overlay.classList.contains('hidden')) {
        closeComparisonOverlay();
      }
    });
  }
});

/* ---------- Similarity Scoring ---------- */

/**
 * Compute similarity between two entries based on selected keys.
 * @param {Object} entryA - First history entry
 * @param {Object} entryB - Second history entry
 * @param {Set<string>} selectedKeys - Keys like "header:content-type", "payload:userId"
 * @returns {number} Similarity score 0–100
 */
function computeSimilarity(entryA, entryB, selectedKeys) {
  const selHeaders = [...selectedKeys].filter(k => k.startsWith('header:')).map(k => k.slice(7));
  const selPayload = [...selectedKeys].filter(k => k.startsWith('payload:')).map(k => k.slice(8));

  if (selHeaders.length === 0 && selPayload.length === 0) return 0;

  let score = 0, total = 0;

  // Header comparison (50% weight)
  if (selHeaders.length > 0) {
    total += 50;
    const mapA = new Map((entryA.request_headers || []).map(([n, v]) => [n.toLowerCase(), v]));
    const mapB = new Map((entryB.request_headers || []).map(([n, v]) => [n.toLowerCase(), v]));
    let matching = 0;
    for (const key of selHeaders) {
      const a = mapA.get(key), b = mapB.get(key);
      if (a !== undefined && b !== undefined) matching += a === b ? 1 : 0.5;
      else if (a === undefined && b === undefined) matching += 1;
    }
    score += (matching / selHeaders.length) * 50;
  }

  // Payload comparison (50% weight)
  if (selPayload.length > 0) {
    total += 50;
    const getObj = (entry) => {
      try {
        const d = JSON.parse(entry.request_body || '{}');
        return typeof d === 'object' && d ? d : {};
      } catch { return {}; }
    };
    const objA = getObj(entryA), objB = getObj(entryB);
    let matching = 0;
    for (const key of selPayload) {
      const a = JSON.stringify(objA[key] ?? null);
      const b = JSON.stringify(objB[key] ?? null);
      if (a === b) matching += 1;
      else if (objA[key] !== undefined && objB[key] !== undefined) matching += 0.3;
    }
    score += (matching / selPayload.length) * 50;
  }

  return total === 0 ? 0 : Math.round((score / total) * 100);
}

/**
 * Get the CSS class for a similarity score.
 */
function similarityClass(score) {
  if (score >= 70) return 'high';
  if (score >= 40) return 'medium';
  return 'low';
}

/**
 * Collect available header and payload keys from a set of entries.
 */
function collectAvailableKeys(entries) {
  const headerKeys = new Set();
  const payloadKeys = new Set();
  for (const entry of entries) {
    for (const [name] of (entry.request_headers || [])) {
      headerKeys.add(name.toLowerCase());
    }
    try {
      const obj = JSON.parse(entry.request_body || '{}');
      if (typeof obj === 'object' && obj) {
        Object.keys(obj).forEach(k => payloadKeys.add(k));
      }
    } catch { /* ignore non-JSON bodies */ }
  }
  return { headerKeys: [...headerKeys].sort(), payloadKeys: [...payloadKeys].sort() };
}

/**
 * Build the HTML for a multi-select key picker dropdown.
 * @param {Array} entries - History entries to extract keys from
 * @param {Set<string>} selectedKeys - Currently selected keys
 * @returns {string} HTML string
 */
function buildKeyPickerHTML(entries, selectedKeys) {
  const { headerKeys, payloadKeys } = collectAvailableKeys(entries);
  const sel = selectedKeys || new Set();

  let html = '<div class="key-picker-dropdown">';

  // Action bar
  html += `<div class="key-picker-actions">
    <button class="btn btn-ghost btn-xs" onclick="keyPickerSelectAll()">Select All</button>
    <button class="btn btn-ghost btn-xs" onclick="keyPickerClearAll()">Clear All</button>
  </div>`;

  html += '<div class="key-picker-list">';

  // Header keys section
  if (headerKeys.length > 0) {
    html += '<div class="key-picker-section">Request Headers</div>';
    for (const key of headerKeys) {
      const fullKey = `header:${key}`;
      const checked = sel.has(fullKey) ? 'checked' : '';
      html += `<label class="key-picker-item">
        <input type="checkbox" value="${escapeHtml(fullKey)}" ${checked} onchange="onKeyPickerChange(this)">
        <span>${escapeHtml(key)}</span>
      </label>`;
    }
  }

  // Payload keys section
  if (payloadKeys.length > 0) {
    html += '<div class="key-picker-section">Payload Keys</div>';
    for (const key of payloadKeys) {
      const fullKey = `payload:${key}`;
      const checked = sel.has(fullKey) ? 'checked' : '';
      html += `<label class="key-picker-item">
        <input type="checkbox" value="${escapeHtml(fullKey)}" ${checked} onchange="onKeyPickerChange(this)">
        <span>${escapeHtml(key)}</span>
      </label>`;
    }
  }

  if (headerKeys.length === 0 && payloadKeys.length === 0) {
    html += '<div class="key-picker-section" style="padding:12px 8px;color:var(--text-muted)">No keys available</div>';
  }

  html += '</div></div>';
  return html;
}

/* Key picker global state — managed by consumer (app.js / history UI) */
let _keyPickerSelectedKeys = new Set();
let _keyPickerOnChange = null;

function keyPickerSelectAll() {
  document.querySelectorAll('.key-picker-dropdown input[type="checkbox"]').forEach(cb => {
    cb.checked = true;
    _keyPickerSelectedKeys.add(cb.value);
  });
  if (_keyPickerOnChange) _keyPickerOnChange(_keyPickerSelectedKeys);
}

function keyPickerClearAll() {
  document.querySelectorAll('.key-picker-dropdown input[type="checkbox"]').forEach(cb => {
    cb.checked = false;
    _keyPickerSelectedKeys.delete(cb.value);
  });
  if (_keyPickerOnChange) _keyPickerOnChange(_keyPickerSelectedKeys);
}

function onKeyPickerChange(checkbox) {
  if (checkbox.checked) {
    _keyPickerSelectedKeys.add(checkbox.value);
  } else {
    _keyPickerSelectedKeys.delete(checkbox.value);
  }
  if (_keyPickerOnChange) _keyPickerOnChange(_keyPickerSelectedKeys);
}

/**
 * Initialize the key picker with a callback for changes.
 * @param {Set<string>} initialKeys - Initially selected keys
 * @param {Function} onChange - Called with updated Set<string> on changes
 */
function initKeyPicker(initialKeys, onChange) {
  _keyPickerSelectedKeys = new Set(initialKeys || []);
  _keyPickerOnChange = onChange || null;
}
