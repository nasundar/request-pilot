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

  // Response Body — placeholder for async Rust diff rendering
  const sizeA = entryA.response_body ? entryA.response_body.length : 0;
  const sizeB = entryB.response_body ? entryB.response_body.length : 0;
  const sizeLabel = typeof formatBytes === 'function'
    ? `${formatBytes(sizeA)} / ${formatBytes(sizeB)}`
    : `${sizeA} B / ${sizeB} B`;
  html += `<div class="compare-section" id="compareRustDiffSection">
    <div class="compare-section-title">Response Body Diff <span style="color:var(--text-muted); font-size:11px; font-weight:400;">(${sizeLabel})</span></div>
    <div class="body-loading"><span class="spinner-sm"></span> Computing diff…</div>
  </div>`;

  // JSON field diff (lightweight key-path comparison) — only for small payloads
  const FIELD_DIFF_LIMIT = 262144; // 256 KiB
  if (entryA.response_body && entryB.response_body &&
      sizeA < FIELD_DIFF_LIMIT && sizeB < FIELD_DIFF_LIMIT) {
    const jsonDiff = buildJsonDiff(entryA.response_body, entryB.response_body);
    if (jsonDiff) {
      html += `<div class="compare-section">
        <div class="compare-section-title">Response Body — Field Diff</div>
        ${jsonDiff}
      </div>`;
    }
  }

  return html;
}

/**
 * Open the comparison overlay for two history entries.
 * Computes response body diff via Rust (cached) and renders paginated hunks.
 */
async function openComparisonOverlay(entryA, entryB) {
  const overlay = document.getElementById('compareOverlay');
  if (!overlay) return;

  // Populate tabs (response body shows spinner initially)
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

  // Async: compute response body diff using Rust + cache
  const bodyA = entryA.response_body || '';
  const bodyB = entryB.response_body || '';
  if (!bodyA && !bodyB) {
    const section = document.getElementById('compareRustDiffSection');
    if (section) section.innerHTML = `<div class="compare-section-title">Response Body Diff</div>
      <p class="compare-empty">Both responses are empty</p>`;
    return;
  }

  // Cache key uses sorted seq pair for consistency
  const seqA = entryA.seq != null ? entryA.seq : 0;
  const seqB = entryB.seq != null ? entryB.seq : 0;
  const cacheKey = `hist:${Math.min(seqA, seqB)}-${Math.max(seqA, seqB)}`;
  const invoke = window.__TAURI__?.core?.invoke;

  let diffResult = null;
  const cached = typeof diffCache !== 'undefined' && diffCache.get(cacheKey);
  if (cached) {
    diffResult = cached;
    if (typeof rpLog === 'function') rpLog('debug', `Diff cache hit: ${cacheKey}`);
  } else if (invoke) {
    try {
      let contentType = 'text';
      try { JSON.parse(bodyA); JSON.parse(bodyB); contentType = 'json'; } catch {}
      diffResult = await invoke('compute_diff', { textA: bodyA, textB: bodyB, contentType });
      // Cache
      if (typeof diffCache !== 'undefined') {
        const entrySize = JSON.stringify(diffResult).length * 2;
        diffCache.set(cacheKey, diffResult);
        if (typeof diffCacheBytes !== 'undefined') diffCacheBytes += entrySize;
        if (typeof rpLog === 'function') rpLog('debug', `Diff cached: ${cacheKey} (${typeof formatBytes === 'function' ? formatBytes(entrySize) : entrySize + ' B'})`);
      }
    } catch (err) {
      if (typeof rpLog === 'function') rpLog('error', 'Rust compute_diff failed for comparison', String(err));
    }
  }

  const section = document.getElementById('compareRustDiffSection');
  if (!section) return;

  if (!diffResult || !diffResult.ops || diffResult.ops.length === 0) {
    // Fallback: show raw side-by-side
    const fmtA = formatBodyForCompare(bodyA);
    const fmtB = formatBodyForCompare(bodyB);
    section.innerHTML = `<div class="compare-section-title">Response Body</div>
      <div class="compare-grid">
        <div class="compare-column">
          <div class="compare-column-label">Response A</div>
          <pre class="compare-body-pre">${escapeHtml(fmtA)}</pre>
        </div>
        <div class="compare-column">
          <div class="compare-column-label">Response B</div>
          <pre class="compare-body-pre">${escapeHtml(fmtB)}</pre>
        </div>
      </div>`;
    return;
  }

  // Render paginated diff hunks (reuse buildDiffHunks from app.js)
  const ops = diffResult.ops;
  const similarity = diffResult.similarity != null ? (diffResult.similarity * 100).toFixed(1) : '?';
  const HUNKS_PER_PAGE = 20;

  const hunks = typeof buildDiffHunks === 'function' ? buildDiffHunks(ops) : [{ type: 'hunk', ops }];
  let renderedCount = 0;
  let leftLineNum = 0, rightLineNum = 0;

  const highlightFn = (s) => s; // no syntax highlight in compare view

  const renderOp = (op) => {
    let leftLine = '', rightLine = '';
    switch (op.type) {
      case 'same': {
        leftLineNum++; rightLineNum++;
        const txt = escapeHtml(op.line || op.left || '');
        leftLine = `<div class="diff-line diff-same"><span class="diff-ln">${leftLineNum}</span><span class="diff-text">${txt}</span></div>`;
        rightLine = `<div class="diff-line diff-same"><span class="diff-ln">${rightLineNum}</span><span class="diff-text">${txt}</span></div>`;
        break;
      }
      case 'remove': {
        leftLineNum++;
        leftLine = `<div class="diff-line diff-removed"><span class="diff-ln">${leftLineNum}</span><span class="diff-text">${escapeHtml(op.line || op.left || '')}</span></div>`;
        rightLine = `<div class="diff-line diff-empty"><span class="diff-ln"></span><span class="diff-text"></span></div>`;
        break;
      }
      case 'add': {
        rightLineNum++;
        leftLine = `<div class="diff-line diff-empty"><span class="diff-ln"></span><span class="diff-text"></span></div>`;
        rightLine = `<div class="diff-line diff-added"><span class="diff-ln">${rightLineNum}</span><span class="diff-text">${escapeHtml(op.line || op.right || '')}</span></div>`;
        break;
      }
      case 'change': {
        leftLineNum++; rightLineNum++;
        let leftCharHtml, rightCharHtml;
        if (op.left_highlights || op.right_highlights) {
          leftCharHtml = typeof renderRustCharHighlight === 'function'
            ? renderRustCharHighlight(op.left, op.left_highlights, 'diff-char-rm', highlightFn)
            : escapeHtml(op.left || '');
          rightCharHtml = typeof renderRustCharHighlight === 'function'
            ? renderRustCharHighlight(op.right, op.right_highlights, 'diff-char-add', highlightFn)
            : escapeHtml(op.right || '');
        } else {
          leftCharHtml = typeof charDiffHighlight === 'function'
            ? charDiffHighlight(op.left, op.right, 'left', highlightFn)
            : escapeHtml(op.left || '');
          rightCharHtml = typeof charDiffHighlight === 'function'
            ? charDiffHighlight(op.left, op.right, 'right', highlightFn)
            : escapeHtml(op.right || '');
        }
        leftLine = `<div class="diff-line diff-changed"><span class="diff-ln">${leftLineNum}</span><span class="diff-text">${leftCharHtml}</span></div>`;
        rightLine = `<div class="diff-line diff-changed"><span class="diff-ln">${rightLineNum}</span><span class="diff-text">${rightCharHtml}</span></div>`;
        break;
      }
    }
    return { leftLine, rightLine };
  };

  const renderHunkBatch = (startIdx, count) => {
    let leftHtml = '', rightHtml = '';
    const end = Math.min(startIdx + count, hunks.length);
    for (let i = startIdx; i < end; i++) {
      const h = hunks[i];
      if (h.type === 'collapse') {
        const divider = `<div class="diff-collapse" data-hunk-idx="${i}">▸ ${h.count} unchanged lines</div>`;
        leftHtml += divider;
        rightHtml += divider;
        h.ops.forEach(() => { leftLineNum++; rightLineNum++; });
      } else {
        h.ops.forEach(op => {
          const { leftLine, rightLine } = renderOp(op);
          leftHtml += leftLine;
          rightHtml += rightLine;
        });
      }
      renderedCount = i + 1;
    }
    return { leftHtml, rightHtml };
  };

  // Initial render
  const { leftHtml, rightHtml } = renderHunkBatch(0, HUNKS_PER_PAGE);
  const remaining = hunks.length - renderedCount;

  section.innerHTML = `<div class="compare-section-title">Response Body Diff
    <span class="diff-badge ${diffResult.match_exact ? 'diff-match' : 'diff-mismatch'}" style="margin-left:8px;">
      ${diffResult.match_exact ? '✓ Match' : similarity + '% similar'}
    </span>
    <span style="color:var(--text-muted); font-size:11px; font-weight:400; margin-left:4px;">
      +${diffResult.added_count || 0} −${diffResult.removed_count || 0} ~${diffResult.changed_count || 0}
    </span>
  </div>
  <div class="diff-side-by-side" style="max-height:500px; overflow:auto;">
    <div class="diff-pane diff-pane-left" id="compareDiffLeft">${leftHtml}</div>
    <div class="diff-pane diff-pane-right" id="compareDiffRight">${rightHtml}</div>
  </div>
  ${remaining > 0 ? `<div class="diff-load-more-row" id="compareDiffLoadMore">
    <div class="diff-load-more-bar">
      <button class="btn btn-sm diff-load-next-btn" id="compareDiffLoadNext">▾ Load next ${Math.min(HUNKS_PER_PAGE, remaining)} hunks</button>
      <button class="btn btn-sm diff-load-all-btn" id="compareDiffLoadAll">Load all (${remaining} remaining)</button>
      <span class="diff-load-more-info">${renderedCount} of ${hunks.length} hunks shown</span>
    </div>
    ${remaining > 50 ? '<div class="diff-load-warn">⚠ Loading all may be slow due to DOM rendering</div>' : ''}
  </div>` : ''}`;

  // Wire collapse toggles
  section.querySelectorAll('.diff-collapse').forEach(el => {
    if (el.dataset.wired) return;
    el.dataset.wired = '1';
    el.addEventListener('click', () => {
      const idx = parseInt(el.dataset.hunkIdx, 10);
      const hunk = hunks[idx];
      if (!hunk || hunk.type !== 'collapse') return;
      let expandLeft = '', expandRight = '';
      let tmpL = leftLineNum, tmpR = rightLineNum;
      // Recalculate line numbers up to this hunk
      let ln = 0, rn = 0;
      for (let i = 0; i < idx; i++) {
        const h = hunks[i];
        h.ops.forEach(op => {
          if (op.type === 'same' || op.type === 'change') { ln++; rn++; }
          else if (op.type === 'remove') ln++;
          else if (op.type === 'add') rn++;
        });
      }
      hunk.ops.forEach(op => {
        ln++; rn++;
        const txt = escapeHtml(op.line || op.left || '');
        expandLeft += `<div class="diff-line diff-same"><span class="diff-ln">${ln}</span><span class="diff-text">${txt}</span></div>`;
        expandRight += `<div class="diff-line diff-same"><span class="diff-ln">${rn}</span><span class="diff-text">${txt}</span></div>`;
      });
      // Replace collapse dividers in both panes
      const leftPane = document.getElementById('compareDiffLeft');
      const rightPane = document.getElementById('compareDiffRight');
      [leftPane, rightPane].forEach(pane => {
        if (!pane) return;
        const collapseEl = pane.querySelector(`.diff-collapse[data-hunk-idx="${idx}"]`);
        if (collapseEl) {
          const frag = document.createRange().createContextualFragment(pane === leftPane ? expandLeft : expandRight);
          collapseEl.replaceWith(frag);
        }
      });
    });
  });

  // Wire load more buttons
  const loadNextBtn = document.getElementById('compareDiffLoadNext');
  const loadAllBtn = document.getElementById('compareDiffLoadAll');
  const loadMore = () => {
    const { leftHtml: moreLeft, rightHtml: moreRight } = renderHunkBatch(renderedCount, HUNKS_PER_PAGE);
    const leftPane = document.getElementById('compareDiffLeft');
    const rightPane = document.getElementById('compareDiffRight');
    if (leftPane) leftPane.insertAdjacentHTML('beforeend', moreLeft);
    if (rightPane) rightPane.insertAdjacentHTML('beforeend', moreRight);
    const rem = hunks.length - renderedCount;
    const bar = document.getElementById('compareDiffLoadMore');
    if (rem <= 0 && bar) { bar.remove(); return; }
    if (bar) {
      bar.innerHTML = `<div class="diff-load-more-bar">
        <button class="btn btn-sm diff-load-next-btn" id="compareDiffLoadNext">▾ Load next ${Math.min(HUNKS_PER_PAGE, rem)} hunks</button>
        <button class="btn btn-sm diff-load-all-btn" id="compareDiffLoadAll">Load all (${rem} remaining)</button>
        <span class="diff-load-more-info">${renderedCount} of ${hunks.length} hunks shown</span>
      </div>
      ${rem > 50 ? '<div class="diff-load-warn">⚠ Loading all may be slow due to DOM rendering</div>' : ''}`;
      document.getElementById('compareDiffLoadNext')?.addEventListener('click', loadMore);
      document.getElementById('compareDiffLoadAll')?.addEventListener('click', loadAll);
    }
  };
  const loadAll = () => {
    const { leftHtml: moreLeft, rightHtml: moreRight } = renderHunkBatch(renderedCount, hunks.length - renderedCount);
    const leftPane = document.getElementById('compareDiffLeft');
    const rightPane = document.getElementById('compareDiffRight');
    if (leftPane) leftPane.insertAdjacentHTML('beforeend', moreLeft);
    if (rightPane) rightPane.insertAdjacentHTML('beforeend', moreRight);
    const bar = document.getElementById('compareDiffLoadMore');
    if (bar) bar.remove();
  };
  if (loadNextBtn) loadNextBtn.addEventListener('click', loadMore);
  if (loadAllBtn) loadAllBtn.addEventListener('click', loadAll);
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
