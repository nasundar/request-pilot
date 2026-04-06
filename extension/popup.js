/* ============================================================
 * RequestPilot – Popup UI Logic
 * ============================================================
 * Drives the popup interface: rule CRUD, import/export,
 * request log display, and modal form handling.
 * ========================================================== */

// ── State ────────────────────────────────────────────────────
let editingRuleId = null;
let cachedRules = [];
let logRefreshInterval = null;

// ── SVG icon templates ───────────────────────────────────────
const ICON_PENCIL = `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
  <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
</svg>`;

const ICON_TRASH = `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
  <path d="M10 11v6"/><path d="M14 11v6"/><path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/>
</svg>`;

const ICON_REMOVE = `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
  <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
</svg>`;

const ICON_DUPLICATE = `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
</svg>`;

// ── Type label mapping ───────────────────────────────────────
const TYPE_LABELS = {
  "modify-headers": "Headers",
  "block": "Block",
  "redirect": "Redirect",
  "forward": "Forward",
};

/* ============================================================
 * Utility Functions
 * ========================================================== */

/** Escape HTML entities to prevent XSS in rendered content. */
function escapeHtml(str) {
  const div = document.createElement("div");
  div.textContent = str;
  return div.innerHTML;
}

/** Escape a string for safe use inside HTML attributes. */
function escapeAttr(str) {
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** Send a message to the background service worker. */
function sendMessage(msg) {
  return new Promise((resolve) => {
    chrome.runtime.sendMessage(msg, resolve);
  });
}

/** Format a timestamp (ms) as HH:MM:SS. */
function formatTime(timestamp) {
  const d = new Date(timestamp);
  return d.toTimeString().split(" ")[0]; // "HH:MM:SS"
}

/** Convert a declarativeNetRequest urlFilter pattern to a RegExp. */
function urlPatternToRegex(pattern) {
  const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*');
  return new RegExp(escaped, 'i');
}

/* ============================================================
 * DOM References
 * ========================================================== */

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => document.querySelectorAll(sel);

const btnAdd = $("#btn-add");
const btnImport = $("#btn-import");
const btnExport = $("#btn-export");
const fileImport = $("#file-import");
const btnClearLog = $("#btn-clear-log");

const rulesContainer = $("#rules-container");
const emptyState = $("#empty-state");
const logContainer = $("#log-container");
const logEmpty = $("#log-empty");

const modalOverlay = $("#modal-overlay");
const modalTitle = $("#modal-title");
const ruleForm = $("#rule-form");

const inputType = $("#input-type");
const inputUrl = $("#input-url");
const inputRedirect = $("#input-redirect");
const redirectSection = $("#redirect-section");
const headersSection = $("#headers-section");
const headersList = $("#headers-list");
const btnAddHeader = $("#btn-add-header");
const methodFilter = $("#method-filter");
const btnCancel = $("#btn-cancel");

/* ============================================================
 * 1. Initialization
 * ========================================================== */

document.addEventListener("DOMContentLoaded", async () => {
  detectStandaloneMode();
  await applyTheme();
  await loadRules();
  attachEventListeners();
  updateDesktopStatus();
  updateSniffingStatus();
  // Poll desktop connection status every 2 seconds
  setInterval(updateDesktopStatus, 2000);
});

async function updateDesktopStatus() {
  try {
    const status = await sendMessage({ action: 'getLiveCaptureStatus' });
    const dot = document.getElementById('desktopDot');
    const text = document.getElementById('desktopStatusText');
    const modeLabel = document.getElementById('desktopModeLabel');
    const toggle = document.getElementById('liveToggle');
    if (!dot || !text || !modeLabel) return;

    dot.className = 'desktop-dot';
    modeLabel.className = 'desktop-mode-label';
    modeLabel.textContent = '';

    // Sync toggle checkbox with backend state (avoid re-triggering change event)
    if (toggle && toggle.checked !== status.enabled) {
      toggle._updating = true;
      toggle.checked = status.enabled;
      toggle._updating = false;
    }

    if (!status.enabled) {
      dot.classList.add('disabled');
      text.textContent = 'Desktop: Disabled';
    } else if (status.connected) {
      if (status.mode === 'all') {
        dot.classList.add('capturing');
        text.textContent = 'Desktop: Capturing';
        modeLabel.textContent = 'ALL';
        modeLabel.classList.add('mode-all');
      } else if (status.mode === 'filtered') {
        dot.classList.add('capturing');
        text.textContent = 'Desktop: Capturing';
        modeLabel.textContent = 'FILTERED';
        modeLabel.classList.add('mode-filtered');
      } else {
        dot.classList.add('connected');
        text.textContent = 'Desktop: Connected';
        modeLabel.textContent = 'PAUSED';
        modeLabel.classList.add('mode-off');
      }
    } else {
      dot.classList.add('connecting');
      text.textContent = 'Desktop: Connecting…';
    }
  } catch {
    /* extension context may not have live capture support */
  }
}

async function updateSniffingStatus() {
  try {
    const status = await sendMessage({ action: 'getSniffingStatus' });
    const dot = document.getElementById('sniffingDot');
    const text = document.getElementById('sniffingStatusText');
    const toggle = document.getElementById('sniffingToggle');
    if (!dot || !text) return;

    if (toggle && toggle.checked !== status.enabled) {
      toggle._updating = true;
      toggle.checked = status.enabled;
      toggle._updating = false;
    }

    dot.className = 'sniffing-dot';
    if (status.enabled) {
      text.textContent = 'Sniffing: Active';
    } else {
      dot.classList.add('inactive');
      text.textContent = 'Sniffing: Off';
    }
  } catch {}
}

function attachEventListeners() {
  // Tab switching
  $$(".tab-btn").forEach((btn) => {
    btn.addEventListener("click", () => switchTab(btn.dataset.tab));
  });

  // Header buttons
  btnAdd.addEventListener("click", openAddModal);
  btnImport.addEventListener("click", () => fileImport.click());
  btnExport.addEventListener("click", exportRules);
  fileImport.addEventListener("change", importRules);
  btnClearLog.addEventListener("click", clearLog);

  // Live capture toggle
  const liveToggle = document.getElementById('liveToggle');
  if (liveToggle) {
    liveToggle.addEventListener('change', async () => {
      if (liveToggle._updating) return; // skip programmatic changes
      const action = liveToggle.checked ? 'enableLiveCapture' : 'disableLiveCapture';
      await sendMessage({ action });
      updateDesktopStatus();
    });
  }

  // Sniffing toggle
  const sniffingToggle = document.getElementById('sniffingToggle');
  if (sniffingToggle) {
    sniffingToggle.addEventListener('change', async () => {
      if (sniffingToggle._updating) return;
      await sendMessage({ action: 'setSniffingEnabled', enabled: sniffingToggle.checked });
      updateSniffingStatus();
    });
  }

  // Log export/import
  const btnExportLog = $("#btn-export-log");
  if (btnExportLog) btnExportLog.addEventListener("click", exportLog);
  const btnImportLog = $("#btn-import-log");
  const fileImportLog = $("#file-import-log");
  if (btnImportLog && fileImportLog) {
    btnImportLog.addEventListener("click", () => fileImportLog.click());
    fileImportLog.addEventListener("change", importLog);
  }

  // Comparison buttons
  const btnCompare = $("#btn-compare");
  if (btnCompare) btnCompare.addEventListener("click", openComparison);
  const btnCloseCompare = $("#btn-close-compare");
  if (btnCloseCompare) btnCloseCompare.addEventListener("click", closeComparison);

  // Pause/resume button
  const btnPauseLog = $("#btn-pause-log");
  if (btnPauseLog) btnPauseLog.addEventListener("click", togglePauseLogging);

  // Comparison tab switching
  $$(".compare-tab-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      $$(".compare-tab-btn").forEach((b) => b.classList.toggle("active", b === btn));
      $$(".compare-tab-content").forEach((c) => {
        c.classList.toggle("active", c.id === `compare-tab-${btn.dataset.ctab}`);
        c.classList.toggle("hidden", c.id !== `compare-tab-${btn.dataset.ctab}`);
      });
    });
  });

  // Theme toggle
  const btnTheme = $("#btn-theme");
  if (btnTheme) btnTheme.addEventListener("click", toggleTheme);

  // Pop-out to new tab
  const btnPopout = $("#btn-popout");
  if (btnPopout) btnPopout.addEventListener("click", popOutToTab);

  // Log filter dropdowns
  const filterMethod = $("#filter-method");
  const filterStatus = $("#filter-status");
  const filterSimilarity = $("#filter-similarity");
  const groupByHeader = $("#group-by-header");
  if (filterMethod) filterMethod.addEventListener("change", renderLog);
  if (filterStatus) filterStatus.addEventListener("change", renderLog);
  if (filterSimilarity) filterSimilarity.addEventListener("change", renderLog);
  if (groupByHeader) groupByHeader.addEventListener("change", renderLog);
  const filterType = $("#filter-type");
  if (filterType) filterType.addEventListener("change", renderLog);
  const hideUrlCb = $("#hide-url");
  if (hideUrlCb) hideUrlCb.addEventListener("change", renderLog);

  // Display headers picker toggle
  const btnDisplayHeaders = $("#btn-display-headers");
  if (btnDisplayHeaders) {
    btnDisplayHeaders.addEventListener("click", (e) => {
      e.stopPropagation();
      $("#display-headers-dropdown").classList.toggle("hidden");
    });
  }
  const btnDhAll = $("#btn-dh-all");
  if (btnDhAll) btnDhAll.addEventListener("click", () => {
    document.querySelectorAll("#display-headers-list input[type='checkbox']").forEach((cb) => {
      cb.checked = true;
      selectedDisplayHeaders.add(cb.value);
    });
    updateDisplayHeadersButtonText();
    renderLog();
  });
  const btnDhNone = $("#btn-dh-none");
  if (btnDhNone) btnDhNone.addEventListener("click", () => {
    selectedDisplayHeaders.clear();
    updateDisplayHeadersButtonText();
    renderLog();
  });

  // Rule picker toggle
  const btnRulePicker = $("#btn-rule-picker");
  if (btnRulePicker) {
    btnRulePicker.addEventListener("click", (e) => {
      e.stopPropagation();
      $("#rule-picker-dropdown").classList.toggle("hidden");
    });
  }
  const btnRulesAll = $("#btn-rules-all");
  if (btnRulesAll) btnRulesAll.addEventListener("click", () => {
    selectedRuleIds = new Set(allRuleIds);
    renderLog();
  });
  const btnRulesNone = $("#btn-rules-none");
  if (btnRulesNone) btnRulesNone.addEventListener("click", () => {
    selectedRuleIds = new Set();
    renderLog();
  });

  // Key picker toggle
  const btnKeyPicker = $("#btn-key-picker");
  if (btnKeyPicker) {
    btnKeyPicker.addEventListener("click", (e) => {
      e.stopPropagation();
      const dd = $("#key-picker-dropdown");
      dd.classList.toggle("hidden");
    });
  }
  // Close all dropdowns on outside click
  document.addEventListener("click", (e) => {
    const wrapper = e.target.closest(".key-picker-wrapper");
    $$(".key-picker-dropdown").forEach((dd) => {
      if (!wrapper || !wrapper.contains(dd)) dd.classList.add("hidden");
    });
  });
  // Select all / deselect all
  const btnKeysAll = $("#btn-keys-all");
  if (btnKeysAll) btnKeysAll.addEventListener("click", () => {
    selectedSimilarityKeys = new Set(allSimilarityKeys);
    updateKeyPicker(cachedNetworkLog);
    renderLog();
  });
  const btnKeysNone = $("#btn-keys-none");
  if (btnKeysNone) btnKeysNone.addEventListener("click", () => {
    selectedSimilarityKeys.clear();
    updateKeyPicker(cachedNetworkLog);
    renderLog();
  });

  // Modal controls
  btnCancel.addEventListener("click", closeModal);
  modalOverlay.addEventListener("click", (e) => {
    if (e.target === modalOverlay) closeModal();
  });

  // Type selector buttons
  $$("#type-selector .type-btn").forEach((btn) => {
    btn.addEventListener("click", () => selectType(btn.dataset.type));
  });

  // Add header row button
  btnAddHeader.addEventListener("click", () => addHeaderRow("", ""));

  // Form submission
  ruleForm.addEventListener("submit", saveRule);
}

/* ============================================================
 * Tab Switching
 * ========================================================== */

function switchTab(tabName) {
  // Update tab button active state
  $$(".tab-btn").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.tab === tabName);
  });

  // Show/hide tab content
  $$(".tab-content").forEach((section) => {
    section.classList.toggle("active", section.id === `tab-${tabName}`);
    section.classList.toggle("hidden", section.id !== `tab-${tabName}`);
  });

  // Manage log auto-refresh interval
  if (tabName === "log") {
    renderLog();
    logRefreshInterval = setInterval(renderLog, 2000);
  } else {
    if (logRefreshInterval) {
      clearInterval(logRefreshInterval);
      logRefreshInterval = null;
    }
  }

  if (tabName === "stats") {
    renderStats();
  }
}

/* ============================================================
 * 2. Rules Tab — Loading & Rendering
 * ========================================================== */

async function loadRules() {
  const response = await sendMessage({ action: "getRules" });
  cachedRules = response.rules || [];
  renderRules();
}

function renderRules() {
  if (cachedRules.length === 0) {
    rulesContainer.innerHTML = "";
    emptyState.classList.remove("hidden");
    return;
  }

  emptyState.classList.add("hidden");
  rulesContainer.innerHTML = cachedRules.map(renderRuleCard).join("");

  // Attach card event listeners after rendering
  rulesContainer.querySelectorAll(".edit-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const rule = cachedRules.find((r) => r.id === Number(btn.dataset.id));
      if (rule) openEditModal(rule);
    });
  });

  rulesContainer.querySelectorAll(".duplicate-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const rule = cachedRules.find((r) => r.id === Number(btn.dataset.id));
      if (rule) duplicateRule(rule);
    });
  });

  rulesContainer.querySelectorAll(".toggle-input").forEach((input) => {
    input.addEventListener("change", () => toggleRule(Number(input.dataset.id)));
  });

  rulesContainer.querySelectorAll(".delete-btn").forEach((btn) => {
    btn.addEventListener("click", () => deleteRule(Number(btn.dataset.id)));
  });
}

/** Build the HTML string for a single rule card. */
function renderRuleCard(rule) {
  const disabledClass = rule.enabled ? "" : " disabled";
  const typeLabel = TYPE_LABELS[rule.type] || rule.type;
  const checkedAttr = rule.enabled ? "checked" : "";

  // Method tags
  const methodTags = (rule.methods || [])
    .map((m) => `<span class="method-tag">${escapeHtml(m.toUpperCase())}</span>`)
    .join("");

  // Type-specific content
  let typeContent = "";
  if (rule.type === "modify-headers" && rule.headers && rule.headers.length) {
    const pills = rule.headers
      .map(
        (h) =>
          `<span class="header-pill">${escapeHtml(h.name)}: ${escapeHtml(h.value)}</span>`
      )
      .join("");
    typeContent = `<div class="rule-headers">${pills}</div>`;
  } else if (rule.type === "redirect" && rule.redirectUrl) {
    typeContent = `<div class="rule-redirect">\u2192 <span class="redirect-target">${escapeHtml(rule.redirectUrl)}</span></div>`;
  }

  return `
    <div class="rule-card${disabledClass}">
      <div class="rule-card-top">
        <span class="rule-url">${escapeHtml(rule.urlPattern)}</span>
        <div class="rule-actions">
          <button class="btn-icon edit-btn" data-id="${rule.id}" title="Edit">
            ${ICON_PENCIL}
          </button>
          <button class="btn-icon duplicate-btn" data-id="${rule.id}" title="Duplicate rule">
            ${ICON_DUPLICATE}
          </button>
          <label class="toggle" title="Enable/Disable">
            <input type="checkbox" ${checkedAttr} class="toggle-input" data-id="${rule.id}" />
            <span class="toggle-slider"></span>
          </label>
          <button class="btn-icon danger delete-btn" data-id="${rule.id}" title="Delete">
            ${ICON_TRASH}
          </button>
        </div>
      </div>
      <div class="rule-meta">
        <span class="rule-type-badge ${escapeAttr(rule.type)}">${escapeHtml(typeLabel)}</span>
        ${methodTags}
      </div>
      ${typeContent}
    </div>`;
}

/* ============================================================
 * 3. Modal — Type Selector
 * ========================================================== */

function selectType(type) {
  // Update active state on type buttons
  $$("#type-selector .type-btn").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.type === type);
  });

  // Update hidden input value
  inputType.value = type;

  // Show/hide sections based on type
  if (type === "modify-headers") {
    headersSection.classList.remove("hidden");
    redirectSection.classList.add("hidden");
  } else if (type === "redirect") {
    headersSection.classList.add("hidden");
    redirectSection.classList.remove("hidden");
  } else {
    // "block" or "forward" — hide both
    headersSection.classList.add("hidden");
    redirectSection.classList.add("hidden");
  }
}

/* ============================================================
 * 4. Modal — Open Add
 * ========================================================== */

function openAddModal() {
  openAddModalWithUrl("");
}

/** Open the Add Rule modal pre-populated with a URL pattern. */
function openAddModalWithUrl(urlPattern) {
  editingRuleId = null;
  modalTitle.textContent = "Add Rule";

  inputUrl.value = urlPattern || "";
  inputRedirect.value = "";

  headersList.innerHTML = "";
  addHeaderRow("", "");

  methodFilter.querySelectorAll("input[type='checkbox']").forEach((cb) => {
    cb.checked = false;
  });

  selectType("modify-headers");
  showModal();
}

/* ============================================================
 * 5. Modal — Open Edit
 * ========================================================== */

function openEditModal(rule) {
  editingRuleId = rule.id;
  modalTitle.textContent = "Edit Rule";

  // Populate URL pattern
  inputUrl.value = rule.urlPattern || "";

  // Set and activate the correct type
  selectType(rule.type);

  // Populate headers for modify-headers type
  headersList.innerHTML = "";
  if (rule.type === "modify-headers" && rule.headers && rule.headers.length) {
    rule.headers.forEach((h) => addHeaderRow(h.name, h.value));
  } else {
    addHeaderRow("", "");
  }

  // Populate redirect URL
  inputRedirect.value = rule.redirectUrl || "";

  // Check the correct method checkboxes
  const selectedMethods = (rule.methods || []).map((m) => m.toLowerCase());
  methodFilter.querySelectorAll("input[type='checkbox']").forEach((cb) => {
    cb.checked = selectedMethods.includes(cb.value);
  });

  showModal();
}

/* ============================================================
 * 6. Modal — Save
 * ========================================================== */

async function saveRule(e) {
  e.preventDefault();

  const type = inputType.value;
  const urlPattern = inputUrl.value.trim();

  if (!urlPattern) {
    alert("URL pattern is required.");
    return;
  }

  // Collect headers from the form rows (only for modify-headers)
  let headers = [];
  if (type === "modify-headers") {
    headersList.querySelectorAll(".header-row").forEach((row) => {
      const name = row.querySelector(".header-name").value.trim();
      const value = row.querySelector(".header-value").value.trim();
      if (name) headers.push({ name, value });
    });

    if (headers.length === 0) {
      alert("At least one header is required for modify-headers rules.");
      return;
    }
  }

  // Collect redirect URL (only for redirect)
  let redirectUrl = "";
  if (type === "redirect") {
    redirectUrl = inputRedirect.value.trim();
    if (!redirectUrl) {
      alert("Redirect URL is required for redirect rules.");
      return;
    }
  }

  // Collect checked methods
  const methods = [];
  methodFilter.querySelectorAll("input[type='checkbox']:checked").forEach((cb) => {
    methods.push(cb.value);
  });

  if (editingRuleId) {
    // Update existing rule
    await sendMessage({
      action: "updateRule",
      ruleId: editingRuleId,
      fields: { type, urlPattern, headers, redirectUrl, methods },
    });
  } else {
    // Create new rule
    await sendMessage({
      action: "addRule",
      type,
      urlPattern,
      headers,
      redirectUrl,
      methods,
    });
  }

  closeModal();
  await loadRules();
}

/* ============================================================
 * 7. Header Rows
 * ========================================================== */

/** Add a header name/value row to the modal form. */
function addHeaderRow(name = "", value = "") {
  const row = document.createElement("div");
  row.className = "header-row";

  const nameInput = document.createElement("input");
  nameInput.type = "text";
  nameInput.className = "header-name";
  nameInput.placeholder = "Header name";
  nameInput.value = name;

  const valueInput = document.createElement("input");
  valueInput.type = "text";
  valueInput.className = "header-value";
  valueInput.placeholder = "Value";
  valueInput.value = value;

  const removeBtn = document.createElement("button");
  removeBtn.type = "button";
  removeBtn.className = "btn-icon danger";
  removeBtn.title = "Remove";
  removeBtn.innerHTML = ICON_REMOVE;
  removeBtn.addEventListener("click", () => {
    row.remove();
    // Ensure at least one empty row remains
    if (headersList.querySelectorAll(".header-row").length === 0) {
      addHeaderRow("", "");
    }
  });

  row.appendChild(nameInput);
  row.appendChild(valueInput);
  row.appendChild(removeBtn);
  headersList.appendChild(row);
}

/* ============================================================
 * Modal Visibility Helpers
 * ========================================================== */

function showModal() {
  modalOverlay.classList.remove("hidden");
}

function closeModal() {
  modalOverlay.classList.add("hidden");
  editingRuleId = null;
}

/* ============================================================
 * 8. Toggle & Delete
 * ========================================================== */

async function toggleRule(ruleId) {
  await sendMessage({ action: "toggleRule", ruleId });
  await loadRules();
}

async function deleteRule(ruleId) {
  if (!confirm("Delete this rule?")) return;
  await sendMessage({ action: "deleteRule", ruleId });
  await loadRules();
}

async function duplicateRule(rule) {
  // Open the Add modal pre-populated with the rule's data — user must click Save
  editingRuleId = null;
  modalTitle.textContent = "Add Rule (Duplicate)";

  inputUrl.value = rule.urlPattern || "";
  inputRedirect.value = rule.redirectUrl || "";

  selectType(rule.type);

  headersList.innerHTML = "";
  if (rule.type === "modify-headers" && rule.headers && rule.headers.length) {
    rule.headers.forEach((h) => addHeaderRow(h.name, h.value));
  } else {
    addHeaderRow("", "");
  }

  const selectedMethods = (rule.methods || []).map((m) => m.toLowerCase());
  methodFilter.querySelectorAll("input[type='checkbox']").forEach((cb) => {
    cb.checked = selectedMethods.includes(cb.value);
  });

  showModal();
}

/* ============================================================
 * 9. Import / Export
 * ========================================================== */

async function exportRules() {
  const response = await sendMessage({ action: "exportRules" });
  const rules = response.rules || [];
  const blob = new Blob([JSON.stringify(rules, null, 2)], {
    type: "application/json",
  });

  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = "request-pilot-rules.json";
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

async function importRules() {
  const file = fileImport.files[0];
  if (!file) return;

  try {
    const text = await file.text();
    const rules = JSON.parse(text);

    if (!Array.isArray(rules)) {
      alert("Invalid file: expected a JSON array of rules.");
      return;
    }

    const response = await sendMessage({ action: "importRules", rules });
    await loadRules();
    alert(`Successfully imported ${response.count} rule(s).`);
  } catch (err) {
    alert("Failed to import rules: " + err.message);
  }

  // Reset file input so the same file can be re-imported
  fileImport.value = "";
}

/* ============================================================
 * 10. Log Tab — Network Log with Comparison
 * ========================================================== */

let selectedRequests = new Set();
let cachedNetworkLog = [];
let viewedEntries = new Set();

let allSimilarityKeys = new Set();
let selectedSimilarityKeys = new Set();
let keyPickerInitialized = false;

let selectedRuleIds = null; // null = all rules selected (initial state)
let allRuleIds = new Set();

// Stats tab: persist expand/collapse state across re-renders
const statsExpandedDomains = new Set();
const statsExpandedPaths = new Set();

/** Build the HTML string for a single log entry. */
function renderLogEntryHTML(entry, similarities) {
  const checked = selectedRequests.has(entry.id) ? "checked" : "";
  const selectedClass = selectedRequests.has(entry.id) ? " selected" : "";
  const statusClass = getStatusClass(entry.statusCode);
  const statusText = entry.statusCode
    ? entry.statusCode
    : entry.error
      ? "ERR"
      : "…";
  const simBadge = similarities[entry.id] !== undefined
    ? `<span class="similarity-badge ${similarities[entry.id] >= 80 ? 'high' : similarities[entry.id] >= 50 ? 'medium' : 'low'}">${similarities[entry.id]}% match</span>`
    : '';

  // Show selected header values
  let headerDisplay = "";
  if (selectedDisplayHeaders.size > 0) {
    const tags = [];
    for (const key of selectedDisplayHeaders) {
      const isReq = key.startsWith("req:");
      const name = key.slice(4);
      const headers = isReq ? (entry.requestHeaders || []) : (entry.responseHeaders || []);
      const hdr = headers.find((h) => h.name.toLowerCase() === name.toLowerCase());
      if (hdr) {
        tags.push(`<span class="log-header-value" title="${escapeAttr(key)}"><span class="log-header-key">${escapeHtml(name)}:</span> ${escapeHtml(hdr.value)}</span>`);
      }
    }
    if (tags.length) headerDisplay = `<div class="log-header-tags">${tags.join("")}</div>`;
  }

  const hideUrl = ($("#hide-url") || {}).checked || false;

  return `
    <div class="log-entry compact${selectedClass}" data-id="${escapeAttr(entry.id)}">
      <input type="checkbox" class="log-checkbox" ${checked} data-id="${escapeAttr(entry.id)}" />
      <div class="log-entry-content">
        ${!hideUrl ? `<div class="log-url">${escapeHtml(entry.url)}</div>` : ''}
        ${headerDisplay}
        <div class="log-meta">
          <span class="log-status ${statusClass}">${statusText}</span>
          <span class="log-method">${escapeHtml(entry.method)}</span>
          <span class="log-type">${escapeHtml(entry.type || "")}</span>
          ${entry.duration != null ? `<span class="log-duration">${entry.duration}ms</span>` : ''}
          <span class="log-time">${formatTime(entry.timestamp)}</span>
          ${simBadge}
        </div>
      </div>
      <button class="btn-view-detail log-view-btn${viewedEntries.has(entry.id) ? ' viewed' : ''}" data-entry-id="${escapeAttr(entry.id)}" title="View details">${viewedEntries.has(entry.id) ? '✓ Viewed' : '👁 View'}</button>
    </div>`;
}

async function renderLog() {
  const [netResponse, rulesResponse, pauseResponse, statsResponse] = await Promise.all([
    sendMessage({ action: "getNetworkLog" }),
    sendMessage({ action: "getRules" }),
    sendMessage({ action: "getLoggingPaused" }),
    sendMessage({ action: "getCaptureStats" }),
  ]);

  // Update pause button state
  updatePauseButton(pauseResponse.paused);

  const allEntries = netResponse.log || [];
  const rules = rulesResponse.rules || [];

  // Update rule picker dropdown
  updateRulePicker(rules);

  // Filter by selected rules only
  const activeRules = selectedRuleIds === null
    ? rules
    : rules.filter((r) => selectedRuleIds.has(r.id));
  const patterns = activeRules.map((r) => urlPatternToRegex(r.urlPattern));

  // Filter: only show entries whose URL matches at least one selected rule pattern
  cachedNetworkLog = patterns.length > 0
    ? allEntries.filter((entry) => patterns.some((re) => re.test(entry.url)))
    : [];

  // Apply method filter
  const methodFilterVal = ($("#filter-method") || {}).value || "";
  if (methodFilterVal) {
    cachedNetworkLog = cachedNetworkLog.filter((e) => e.method === methodFilterVal);
  }

  // Apply status filter
  const statusFilter = ($("#filter-status") || {}).value || "";
  if (statusFilter) {
    cachedNetworkLog = cachedNetworkLog.filter((e) => {
      if (statusFilter === "err") return !e.statusCode || e.error;
      const code = e.statusCode;
      if (statusFilter === "2xx") return code >= 200 && code < 300;
      if (statusFilter === "3xx") return code >= 300 && code < 400;
      if (statusFilter === "4xx") return code >= 400 && code < 500;
      if (statusFilter === "5xx") return code >= 500;
      return true;
    });
  }

  // Apply resource type filter
  const typeFilterVal = ($("#filter-type") || {}).value || "";
  if (typeFilterVal) {
    cachedNetworkLog = cachedNetworkLog.filter((e) => classifyResourceType(e.type) === typeFilterVal);
  }

  // Update captured request count
  const totalMatching = cachedNetworkLog.length;
  const withBody = cachedNetworkLog.filter((e) => e.responseBody).length;
  const stats = statsResponse || {};
  const logCountEl = $("#log-count");
  if (logCountEl) {
    logCountEl.textContent = `(${totalMatching} captured${withBody > 0 ? `, ${withBody} with response body` : ""}, ${stats.captureCount || 0} bodies received)`;
  }

  // Clear stale selections
  const validIds = new Set(cachedNetworkLog.map((e) => e.id));
  for (const id of selectedRequests) {
    if (!validIds.has(id)) selectedRequests.delete(id);
  }

  updateSelectionUI();

  // Update key picker with current entries
  updateKeyPicker(cachedNetworkLog);

  if (cachedNetworkLog.length === 0) {
    logContainer.innerHTML = "";
    logContainer.classList.add("hidden");
    logEmpty.classList.remove("hidden");
    return;
  }

  logEmpty.classList.add("hidden");
  logContainer.classList.remove("hidden");

  // Compute similarity scores when at least 1 is selected (use first selection as anchor)
  let similarities = {};
  let anchorId = null;
  if (selectedRequests.size >= 1) {
    anchorId = [...selectedRequests][0];
    const anchorEntry = cachedNetworkLog.find((e) => e.id === anchorId);
    if (anchorEntry) {
      cachedNetworkLog.forEach((entry) => {
        if (entry.id !== anchorId) {
          similarities[entry.id] = computeSimilarity(anchorEntry, entry);
        }
      });
    }
  }

  // Apply similarity score filter (active whenever scores exist)
  const simThreshold = parseInt(($("#filter-similarity") || {}).value || "0", 10);
  if (simThreshold > 0 && Object.keys(similarities).length > 0) {
    cachedNetworkLog = cachedNetworkLog.filter(
      (e) => selectedRequests.has(e.id) || (similarities[e.id] !== undefined && similarities[e.id] >= simThreshold)
    );
  }

  // Populate header dropdowns with unique header names from visible entries
  updateHeaderDropdowns(cachedNetworkLog);

  // Get group-by value
  const groupByRaw = ($("#group-by-header") || {}).value || "";
  const groupByIsUrl = groupByRaw === "url";
  const groupByIsRes = groupByRaw.startsWith("res:");
  const groupByHeaderName = groupByIsUrl ? "URL" : (groupByRaw ? groupByRaw.slice(4) : "");

  // Render entries — grouped or flat
  if (groupByRaw) {
    // Group entries by the selected field
    const groups = new Map();
    cachedNetworkLog.forEach((entry) => {
      let groupKey;
      if (groupByIsUrl) {
        groupKey = entry.url;
      } else {
        const headers = groupByIsRes ? (entry.responseHeaders || []) : (entry.requestHeaders || []);
        const hdr = headers.find(
          (h) => h.name.toLowerCase() === groupByHeaderName.toLowerCase()
        );
        groupKey = hdr ? hdr.value : "(no value)";
      }
      if (!groups.has(groupKey)) groups.set(groupKey, []);
      groups.get(groupKey).push(entry);
    });

    let html = "";
    let groupIdx = 0;
    for (const [groupValue, entries] of groups) {
      const compareBtn = entries.length >= 2
        ? `<button class="btn btn-primary btn-sm group-compare-btn" data-group="${groupIdx}" title="Compare top 2 in this group">Compare</button>`
        : '';
      html += `<div class="log-group-header">${compareBtn}<span class="group-key">${escapeHtml(groupByHeaderName)}:</span> <span class="group-value">${escapeHtml(groupValue)}</span> <span class="group-count">(${entries.length})</span></div>`;
      html += entries.map((e) => renderLogEntryHTML(e, similarities)).join("");
      groupIdx++;
    }
    logContainer.innerHTML = html;

    // Wire up group-level compare buttons
    const groupEntries = [...groups.values()];
    logContainer.querySelectorAll(".group-compare-btn").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const idx = parseInt(btn.dataset.group, 10);
        const entries = groupEntries[idx];
        if (!entries || entries.length < 2) return;
        // Auto-select the first 2 entries in this group
        selectedRequests.clear();
        selectedRequests.add(entries[0].id);
        selectedRequests.add(entries[1].id);
        updateSelectionUI();
        openComparison();
      });
    });
  } else {
    logContainer.innerHTML = cachedNetworkLog
      .map((entry) => renderLogEntryHTML(entry, similarities))
      .join("");
  }

  // Attach checkbox listeners
  logContainer.querySelectorAll(".log-checkbox").forEach((cb) => {
    cb.addEventListener("change", (e) => {
      const id = e.target.dataset.id;
      if (e.target.checked) {
        if (selectedRequests.size >= 2) {
          e.target.checked = false; // prevent selecting more than 2
          return;
        }
        selectedRequests.add(id);
      } else {
        selectedRequests.delete(id);
      }
      renderLog();
    });
  });

  // Disable unchecked checkboxes when 2 are already selected
  if (selectedRequests.size >= 2) {
    logContainer.querySelectorAll(".log-checkbox").forEach((cb) => {
      if (!cb.checked) cb.disabled = true;
    });
  }

  // Allow clicking the row (not just checkbox) to toggle
  logContainer.querySelectorAll(".log-entry").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.classList.contains("log-checkbox")) return;
      if (e.target.closest(".btn-view-detail")) return;
      const cb = row.querySelector(".log-checkbox");
      cb.checked = !cb.checked;
      cb.dispatchEvent(new Event("change"));
    });
  });

  // Wire up view detail buttons
  logContainer.querySelectorAll(".log-view-btn").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const entryId = btn.dataset.entryId;
      const entry = cachedNetworkLog.find((e) => e.id === entryId);
      if (entry) showRequestDetail(entry);
    });
  });
}

function getStatusClass(code) {
  if (!code) return "status-err";
  if (code < 300) return "status-2xx";
  if (code < 400) return "status-3xx";
  if (code < 500) return "status-4xx";
  return "status-5xx";
}

/** Map Chrome ResourceType to DevTools-style category */
function classifyResourceType(type) {
  switch (type) {
    case "xmlhttprequest": case "fetch": return "fetch";
    case "main_frame": case "sub_frame": return "doc";
    case "script": return "js";
    case "stylesheet": return "css";
    case "image": return "img";
    case "media": return "media";
    case "font": return "font";
    case "websocket": return "ws";
    default: return "other";
  }
}

function updateSelectionUI() {
  const countEl = $("#selection-count");
  const compareBtn = $("#btn-compare");

  if (selectedRequests.size > 0) {
    countEl.textContent = `${selectedRequests.size} selected`;
    countEl.classList.remove("hidden");
  } else {
    countEl.classList.add("hidden");
  }

  if (selectedRequests.size === 2) {
    compareBtn.classList.remove("hidden");
  } else {
    compareBtn.classList.add("hidden");
  }
}

async function clearLog() {
  await sendMessage({ action: "clearNetworkLog" });
  selectedRequests.clear();
  await renderLog();
}

async function exportLog() {
  const response = await sendMessage({ action: "getNetworkLog" });
  const log = response.log || [];
  const blob = new Blob([JSON.stringify(log, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `request-pilot-log-${new Date().toISOString().slice(0, 10)}.json`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

async function importLog() {
  const fileInput = $("#file-import-log");
  const file = fileInput.files[0];
  if (!file) return;
  try {
    const text = await file.text();
    const parsed = JSON.parse(text);

    let log;
    // Detect HAR format
    if (parsed.log && parsed.log.entries && Array.isArray(parsed.log.entries)) {
      log = convertHarEntries(parsed.log.entries);
    } else if (Array.isArray(parsed)) {
      log = parsed;
    } else {
      alert("Invalid file: expected a JSON array or a HAR file.");
      return;
    }

    const response = await sendMessage({ action: "importNetworkLog", log });
    await renderLog();
    alert(`Imported ${response.count} log entries.`);
  } catch (err) {
    alert("Failed to import log: " + err.message);
  }
  fileInput.value = "";
}

/** Convert HAR entries to RequestPilot's network log format. */
function convertHarEntries(harEntries) {
  return harEntries.map((entry, i) => {
    const req = entry.request || {};
    const res = entry.response || {};

    // Request body
    let requestBody = null;
    if (req.postData) {
      if (req.postData.mimeType && req.postData.mimeType.includes("form")) {
        const formData = {};
        (req.postData.params || []).forEach((p) => {
          formData[p.name] = formData[p.name] ? [].concat(formData[p.name], p.value) : [p.value];
        });
        requestBody = { type: "formData", data: formData };
      } else {
        requestBody = { type: "raw", data: req.postData.text || "" };
      }
    }

    // Response body
    let responseBody = null;
    if (res.content && res.content.text) {
      responseBody = res.content.text;
    }

    return {
      id: `har-${i}-${Date.now()}`,
      timestamp: entry.startedDateTime ? new Date(entry.startedDateTime).getTime() : Date.now(),
      url: req.url || "",
      method: req.method || "GET",
      type: (req.headers || []).find((h) => h.name.toLowerCase() === "sec-fetch-dest")?.value || "other",
      tabId: -1,
      requestHeaders: (req.headers || []).map((h) => ({ name: h.name, value: h.value })),
      responseHeaders: (res.headers || []).map((h) => ({ name: h.name, value: h.value })),
      statusCode: res.status || 0,
      duration: entry.time ? Math.round(entry.time) : null,
      requestBody,
      responseBody,
    };
  });
}

/* ============================================================
 * 10a. Similarity Scoring & Key Picker
 * ========================================================== */

/** Scan visible log entries and collect all unique header names and payload keys. */
function collectAvailableKeys(entries) {
  const headerKeys = new Set();
  const payloadKeys = new Set();

  entries.forEach((entry) => {
    (entry.requestHeaders || []).forEach((h) => headerKeys.add(h.name.toLowerCase()));
    if (entry.requestBody?.data) {
      try {
        const data = typeof entry.requestBody.data === "string"
          ? JSON.parse(entry.requestBody.data)
          : entry.requestBody.data;
        if (typeof data === "object" && data !== null) {
          Object.keys(data).forEach((k) => payloadKeys.add(k));
        }
      } catch { /* not JSON */ }
    }
  });

  return { headerKeys: [...headerKeys].sort(), payloadKeys: [...payloadKeys].sort() };
}

/** Populate the key picker dropdown. */
function updateKeyPicker(entries) {
  const { headerKeys, payloadKeys } = collectAvailableKeys(entries);
  const allKeys = [...headerKeys.map((k) => `header:${k}`), ...payloadKeys.map((k) => `payload:${k}`)];

  // On first run, select all
  if (!keyPickerInitialized) {
    selectedSimilarityKeys = new Set(allKeys);
    keyPickerInitialized = true;
  }
  allSimilarityKeys = new Set(allKeys);

  // Update button text
  const btn = $("#btn-key-picker");
  if (btn) {
    const total = allKeys.length;
    const selected = [...selectedSimilarityKeys].filter((k) => allSimilarityKeys.has(k)).length;
    btn.textContent = selected === total ? `All keys (${total}) ▾` : `${selected}/${total} keys ▾`;
  }

  // Render the dropdown list
  const list = $("#key-picker-list");
  if (!list) return;

  let html = "";
  if (headerKeys.length > 0) {
    html += `<div class="key-picker-section">Request Headers</div>`;
    headerKeys.forEach((k) => {
      const key = `header:${k}`;
      const checked = selectedSimilarityKeys.has(key) ? "checked" : "";
      html += `<label class="key-picker-item"><input type="checkbox" value="${escapeAttr(key)}" ${checked} /> ${escapeHtml(k)}</label>`;
    });
  }
  if (payloadKeys.length > 0) {
    html += `<div class="key-picker-section">Payload Keys</div>`;
    payloadKeys.forEach((k) => {
      const key = `payload:${k}`;
      const checked = selectedSimilarityKeys.has(key) ? "checked" : "";
      html += `<label class="key-picker-item"><input type="checkbox" value="${escapeAttr(key)}" ${checked} /> ${escapeHtml(k)}</label>`;
    });
  }
  if (!html) {
    html = `<div style="padding:12px;color:var(--text-muted);font-size:12px;text-align:center;">No keys available</div>`;
  }
  list.innerHTML = html;

  // Wire up checkboxes
  list.querySelectorAll("input[type='checkbox']").forEach((cb) => {
    cb.addEventListener("change", () => {
      if (cb.checked) {
        selectedSimilarityKeys.add(cb.value);
      } else {
        selectedSimilarityKeys.delete(cb.value);
      }
      updateKeyPickerButtonText();
      renderLog();
    });
  });
}

function updateKeyPickerButtonText() {
  const btn = $("#btn-key-picker");
  if (!btn) return;
  const total = allSimilarityKeys.size;
  const selected = [...selectedSimilarityKeys].filter((k) => allSimilarityKeys.has(k)).length;
  btn.textContent = selected === total ? `All keys (${total}) ▾` : `${selected}/${total} keys ▾`;
}

/** Populate the "Show Header" and "Group By" dropdowns with unique header names. */
let selectedDisplayHeaders = new Set();

function updateHeaderDropdowns(entries) {
  const reqHeaders = new Set();
  const resHeaders = new Set();
  entries.forEach((entry) => {
    (entry.requestHeaders || []).forEach((h) => reqHeaders.add(h.name));
    (entry.responseHeaders || []).forEach((h) => resHeaders.add(h.name));
  });

  const allHeaders = [...new Set([...reqHeaders, ...resHeaders])].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
  const sortedReq = [...reqHeaders].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
  const sortedRes = [...resHeaders].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));

  // Update group-by dropdown (single select — uses all headers)
  const groupBy = $("#group-by-header");
  if (groupBy) {
    const currentVal = groupBy.value;
    groupBy.innerHTML = '<option value="">None</option><option value="url">URL</option>';
    if (sortedReq.length) {
      const optGroup1 = document.createElement("optgroup");
      optGroup1.label = "Request Headers";
      sortedReq.forEach((name) => {
        const opt = document.createElement("option");
        opt.value = `req:${name}`;
        opt.textContent = name;
        optGroup1.appendChild(opt);
      });
      groupBy.appendChild(optGroup1);
    }
    if (sortedRes.length) {
      const optGroup2 = document.createElement("optgroup");
      optGroup2.label = "Response Headers";
      sortedRes.forEach((name) => {
        const opt = document.createElement("option");
        opt.value = `res:${name}`;
        opt.textContent = name;
        optGroup2.appendChild(opt);
      });
      groupBy.appendChild(optGroup2);
    }
    if (currentVal) groupBy.value = currentVal;
  }

  // Update display headers multi-select
  const list = $("#display-headers-list");
  if (!list) return;

  let html = "";
  if (sortedReq.length) {
    html += `<div class="key-picker-section">Request Headers</div>`;
    sortedReq.forEach((name) => {
      const key = `req:${name}`;
      const checked = selectedDisplayHeaders.has(key) ? "checked" : "";
      html += `<label class="key-picker-item"><input type="checkbox" value="${escapeAttr(key)}" ${checked} /> ${escapeHtml(name)}</label>`;
    });
  }
  if (sortedRes.length) {
    html += `<div class="key-picker-section">Response Headers</div>`;
    sortedRes.forEach((name) => {
      const key = `res:${name}`;
      const checked = selectedDisplayHeaders.has(key) ? "checked" : "";
      html += `<label class="key-picker-item"><input type="checkbox" value="${escapeAttr(key)}" ${checked} /> ${escapeHtml(name)}</label>`;
    });
  }
  if (!html) html = `<div style="padding:12px;color:var(--text-muted);font-size:12px;text-align:center;">No headers available</div>`;
  list.innerHTML = html;

  list.querySelectorAll("input[type='checkbox']").forEach((cb) => {
    cb.addEventListener("change", () => {
      if (cb.checked) selectedDisplayHeaders.add(cb.value);
      else selectedDisplayHeaders.delete(cb.value);
      updateDisplayHeadersButtonText();
      renderLog();
    });
  });

  updateDisplayHeadersButtonText();
}

function updateDisplayHeadersButtonText() {
  const btn = $("#btn-display-headers");
  if (!btn) return;
  const count = selectedDisplayHeaders.size;
  btn.textContent = count === 0 ? "None ▾" : `${count} header${count > 1 ? "s" : ""} ▾`;
}

/** Populate the rule filter multi-select dropdown. */
function updateRulePicker(rules) {
  allRuleIds = new Set(rules.map((r) => r.id));

  // First time: select all
  if (selectedRuleIds === null) {
    selectedRuleIds = new Set(allRuleIds);
  }

  const btn = $("#btn-rule-picker");
  if (btn) {
    const total = allRuleIds.size;
    const selected = [...selectedRuleIds].filter((id) => allRuleIds.has(id)).length;
    btn.textContent = selected === total ? `All rules (${total}) ▾` : `${selected}/${total} rules ▾`;
  }

  const list = $("#rule-picker-list");
  if (!list) return;

  let html = "";
  rules.forEach((r) => {
    const checked = selectedRuleIds.has(r.id) ? "checked" : "";
    const badge = r.type === "block" ? "🚫" : r.type === "redirect" ? "↗" : "📝";
    const enabledTag = r.enabled ? "" : ' <span style="opacity:0.5">(disabled)</span>';
    html += `<label class="key-picker-item"><input type="checkbox" value="${r.id}" ${checked} /> ${badge} ${escapeHtml(r.urlPattern)}${enabledTag}</label>`;
  });
  if (!html) html = `<div style="padding:12px;color:var(--text-muted);font-size:12px;text-align:center;">No rules defined</div>`;
  list.innerHTML = html;

  list.querySelectorAll("input[type='checkbox']").forEach((cb) => {
    cb.addEventListener("change", () => {
      const id = Number(cb.value);
      if (cb.checked) selectedRuleIds.add(id);
      else selectedRuleIds.delete(id);
      renderLog();
    });
  });
}

function computeSimilarity(entryA, entryB) {
  // Determine which header and payload keys to compare
  const selHeaders = [...selectedSimilarityKeys].filter((k) => k.startsWith("header:")).map((k) => k.slice(7));
  const selPayload = [...selectedSimilarityKeys].filter((k) => k.startsWith("payload:")).map((k) => k.slice(8));

  if (selHeaders.length === 0 && selPayload.length === 0) return 0;

  let score = 0;
  let total = 0;

  // Header comparison (only selected header keys)
  if (selHeaders.length > 0) {
    total += 50;
    const mapA = new Map((entryA.requestHeaders || []).map((h) => [h.name.toLowerCase(), h.value]));
    const mapB = new Map((entryB.requestHeaders || []).map((h) => [h.name.toLowerCase(), h.value]));
    let matching = 0;
    selHeaders.forEach((key) => {
      const a = mapA.get(key);
      const b = mapB.get(key);
      if (a !== undefined && b !== undefined) {
        matching += a === b ? 1 : 0.5;
      } else if (a === undefined && b === undefined) {
        matching += 1; // Both missing = match
      }
    });
    score += (matching / selHeaders.length) * 50;
  }

  // Payload comparison (only selected payload keys)
  if (selPayload.length > 0) {
    total += 50;
    const getPayloadObj = (entry) => {
      if (!entry.requestBody?.data) return {};
      try {
        const d = typeof entry.requestBody.data === "string" ? JSON.parse(entry.requestBody.data) : entry.requestBody.data;
        return typeof d === "object" && d !== null ? d : {};
      } catch { return {}; }
    };
    const objA = getPayloadObj(entryA);
    const objB = getPayloadObj(entryB);
    let matching = 0;
    selPayload.forEach((key) => {
      const a = JSON.stringify(objA[key] ?? null);
      const b = JSON.stringify(objB[key] ?? null);
      if (a === b) matching += 1;
      else if (objA[key] !== undefined && objB[key] !== undefined) matching += 0.3;
    });
    score += (matching / selPayload.length) * 50;
  }

  if (total === 0) return 0;
  const result = Math.round((score / total) * 100);
  return isNaN(result) ? 0 : result;
}

/* ============================================================
 * 10b. Pause / Resume Logging
 * ========================================================== */

async function togglePauseLogging() {
  const response = await sendMessage({ action: "getLoggingPaused" });
  const newState = !response.paused;
  await sendMessage({ action: "setLoggingPaused", paused: newState });
  updatePauseButton(newState);
}

function updatePauseButton(isPaused) {
  const btn = $("#btn-pause-log");
  if (!btn) return;
  if (isPaused) {
    btn.textContent = "▶ Resume";
    btn.classList.add("btn-pause-active");
    btn.classList.remove("btn-ghost");
  } else {
    btn.textContent = "⏸ Pause";
    btn.classList.remove("btn-pause-active");
    btn.classList.add("btn-ghost");
  }
}

/* ============================================================
 * 10c. Comparison Logic
 * ========================================================== */

async function openComparison() {
  const ids = [...selectedRequests];
  const entryA = cachedNetworkLog.find((e) => e.id === ids[0]);
  const entryB = cachedNetworkLog.find((e) => e.id === ids[1]);
  if (!entryA || !entryB) return;

  // Populate Request tab immediately
  const reqContent = $("#compare-tab-request");
  const resContent = $("#compare-tab-response");

  reqContent.innerHTML = buildRequestComparisonHTML(entryA, entryB);
  resContent.innerHTML = `<div style="text-align:center;padding:40px;color:var(--text-muted)">Loading response bodies...</div>`;

  // Reset to Request tab
  $$(".compare-tab-btn").forEach((b) => b.classList.toggle("active", b.dataset.ctab === "request"));
  $$(".compare-tab-content").forEach((c) => {
    c.classList.toggle("active", c.id === "compare-tab-request");
    c.classList.toggle("hidden", c.id !== "compare-tab-request");
  });

  $("#compare-overlay").classList.remove("hidden");

  // Fetch missing response bodies in parallel
  const [bodyA, bodyB] = await Promise.all([
    getResponseBody(entryA),
    getResponseBody(entryB),
  ]);

  // Store fetched bodies on entries for future use
  entryA.responseBody = bodyA;
  entryB.responseBody = bodyB;

  // Render the response tab with actual data
  resContent.innerHTML = buildResponseComparisonHTML(
    { ...entryA, responseBody: bodyA },
    { ...entryB, responseBody: bodyB }
  );
}

/** Get response body — use cached if available, otherwise re-fetch from service worker. */
async function getResponseBody(entry) {
  if (entry.responseBody) return entry.responseBody;

  // Build request body text from the captured requestBody
  let requestBodyText = null;
  if (entry.requestBody) {
    if (entry.requestBody.type === "raw" && typeof entry.requestBody.data === "string") {
      requestBodyText = entry.requestBody.data;
    } else if (entry.requestBody.type === "formData" && entry.requestBody.data) {
      // Convert formData object to URL-encoded string
      const params = new URLSearchParams();
      for (const [key, vals] of Object.entries(entry.requestBody.data)) {
        const arr = Array.isArray(vals) ? vals : [vals];
        arr.forEach((v) => params.append(key, v));
      }
      requestBodyText = params.toString();
    }
  }

  // Try re-fetching from the service worker
  try {
    const resp = await sendMessage({
      action: "fetchResponseBody",
      url: entry.url,
      method: entry.method,
      requestHeaders: entry.requestHeaders,
      requestBodyText,
      entryId: entry.id,
    });
    if (resp && resp.success && resp.body) {
      return resp.body;
    }
    return resp && resp.error ? `(fetch failed: ${resp.error})` : "(not available)";
  } catch (err) {
    return `(fetch failed: ${err.message})`;
  }
}

function closeComparison() {
  $("#compare-overlay").classList.add("hidden");
  $("#compare-overlay .compare-header h2").textContent = "Request Comparison";
}

function buildRequestComparisonHTML(a, b) {
  let html = "";

  // General info
  html += `<div class="compare-section">
    <div class="compare-section-title">General</div>
    <div class="compare-grid">
      <div class="compare-column">
        <div class="compare-column-label">Request A</div>
        <div class="compare-info-row"><span class="compare-info-label">URL</span><span class="compare-info-value">${escapeHtml(a.url)}</span></div>
        <div class="compare-info-row"><span class="compare-info-label">Method</span><span class="compare-info-value">${escapeHtml(a.method)}</span></div>
        <div class="compare-info-row"><span class="compare-info-label">Time</span><span class="compare-info-value">${formatTime(a.timestamp)}</span></div>
      </div>
      <div class="compare-column">
        <div class="compare-column-label">Request B</div>
        <div class="compare-info-row"><span class="compare-info-label">URL</span><span class="compare-info-value">${escapeHtml(b.url)}</span></div>
        <div class="compare-info-row"><span class="compare-info-label">Method</span><span class="compare-info-value">${escapeHtml(b.method)}</span></div>
        <div class="compare-info-row"><span class="compare-info-label">Time</span><span class="compare-info-value">${formatTime(b.timestamp)}</span></div>
      </div>
    </div>
  </div>`;

  // Request headers diff
  html += `<div class="compare-section">
    <div class="compare-section-title">Request Headers</div>
    ${buildHeadersDiff(a.requestHeaders || [], b.requestHeaders || [])}
  </div>`;

  // Request body diff
  const bodyA = formatBody(a.requestBody);
  const bodyB = formatBody(b.requestBody);
  html += `<div class="compare-section">
    <div class="compare-section-title">Request Body</div>
    <div class="compare-grid">
      <div class="compare-column">
        <div class="compare-column-label">Request A</div>
        <pre style="font-size:12px;white-space:pre-wrap;word-break:break-all;margin:0;color:var(--text)">${escapeHtml(bodyA)}</pre>
      </div>
      <div class="compare-column">
        <div class="compare-column-label">Request B</div>
        <pre style="font-size:12px;white-space:pre-wrap;word-break:break-all;margin:0;color:var(--text)">${escapeHtml(bodyB)}</pre>
      </div>
    </div>
  </div>`;

  // If both request bodies are JSON, show structural diff
  try {
    const jsonA = JSON.parse(bodyA);
    const jsonB = JSON.parse(bodyB);
    if (typeof jsonA === "object" && typeof jsonB === "object" && jsonA !== null && jsonB !== null) {
      html += `<div class="compare-section">
        <div class="compare-section-title">Request Body Diff (JSON Keys)</div>
        ${buildJsonDiff(jsonA, jsonB)}
      </div>`;
    }
  } catch {
    // Not JSON — skip structural diff
  }

  return html;
}

function buildResponseComparisonHTML(a, b) {
  let html = "";

  // Status comparison
  const statusClassA = getStatusClass(a.statusCode);
  const statusClassB = getStatusClass(b.statusCode);
  html += `<div class="compare-section">
    <div class="compare-section-title">Status</div>
    <div class="compare-grid">
      <div class="compare-column">
        <div class="compare-column-label">Response A</div>
        <div class="compare-info-row">
          <span class="compare-info-label">Status</span>
          <span class="compare-info-value log-status ${statusClassA}">${a.statusCode || (a.error ? 'Error: ' + a.error : 'Pending')}</span>
        </div>
        <div class="compare-info-row"><span class="compare-info-label">Type</span><span class="compare-info-value">${escapeHtml(a.type || '—')}</span></div>
      </div>
      <div class="compare-column">
        <div class="compare-column-label">Response B</div>
        <div class="compare-info-row">
          <span class="compare-info-label">Status</span>
          <span class="compare-info-value log-status ${statusClassB}">${b.statusCode || (b.error ? 'Error: ' + b.error : 'Pending')}</span>
        </div>
        <div class="compare-info-row"><span class="compare-info-label">Type</span><span class="compare-info-value">${escapeHtml(b.type || '—')}</span></div>
      </div>
    </div>
  </div>`;

  // Response headers diff
  html += `<div class="compare-section">
    <div class="compare-section-title">Response Headers</div>
    ${buildHeadersDiff(a.responseHeaders || [], b.responseHeaders || [])}
  </div>`;

  // Response body comparison
  const respBodyA = a.responseBody || "(not captured)";
  const respBodyB = b.responseBody || "(not captured)";

  function prettyPrint(text) {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return text;
    }
  }

  html += `<div class="compare-section">
    <div class="compare-section-title">Response Body</div>
    <div class="compare-grid">
      <div class="compare-column">
        <div class="compare-column-label">Response A</div>
        <pre style="font-size:12px;white-space:pre-wrap;word-break:break-all;margin:0;color:var(--text);max-height:400px;overflow-y:auto">${escapeHtml(prettyPrint(respBodyA))}</pre>
      </div>
      <div class="compare-column">
        <div class="compare-column-label">Response B</div>
        <pre style="font-size:12px;white-space:pre-wrap;word-break:break-all;margin:0;color:var(--text);max-height:400px;overflow-y:auto">${escapeHtml(prettyPrint(respBodyB))}</pre>
      </div>
    </div>
  </div>`;

  // If both are JSON, show a key-by-key diff
  try {
    const jsonA = JSON.parse(respBodyA);
    const jsonB = JSON.parse(respBodyB);
    if (typeof jsonA === "object" && typeof jsonB === "object" && jsonA !== null && jsonB !== null) {
      html += `<div class="compare-section">
        <div class="compare-section-title">Response Body Diff (JSON Keys)</div>
        ${buildJsonDiff(jsonA, jsonB)}
      </div>`;
    }
  } catch {
    // Not JSON — skip structural diff
  }

  return html;
}

function buildJsonDiff(objA, objB, prefix) {
  prefix = prefix || "";
  const keysA = objA ? Object.keys(objA) : [];
  const keysB = objB ? Object.keys(objB) : [];
  const allKeys = new Set([...keysA, ...keysB]);
  const sorted = [...allKeys].sort();

  let rows = "";
  for (const key of sorted) {
    const fullKey = prefix ? prefix + "." + key : key;
    const valA = objA ? objA[key] : undefined;
    const valB = objB ? objB[key] : undefined;

    // If both are objects/arrays, recurse into them
    const bothObjects = valA && valB && typeof valA === "object" && typeof valB === "object"
      && !Array.isArray(valA) && !Array.isArray(valB);
    if (bothObjects) {
      rows += buildJsonDiffRows(valA, valB, fullKey);
      continue;
    }

    // If both are arrays, compare element-by-element
    if (Array.isArray(valA) && Array.isArray(valB)) {
      const maxLen = Math.max(valA.length, valB.length);
      for (let i = 0; i < maxLen; i++) {
        const itemKey = `${fullKey}[${i}]`;
        const a = valA[i];
        const b = valB[i];
        if (a && b && typeof a === "object" && typeof b === "object" && !Array.isArray(a) && !Array.isArray(b)) {
          rows += buildJsonDiffRows(a, b, itemKey);
        } else {
          rows += buildDiffRow(itemKey, a, b);
        }
      }
      continue;
    }

    rows += buildDiffRow(fullKey, valA, valB);
  }

  if (!rows) {
    return `<p style="color:var(--text-muted);font-size:12px;">Both responses are identical.</p>`;
  }

  return `<table class="compare-headers-table">
    <thead><tr><th></th><th>Key</th><th>Response A</th><th>Response B</th></tr></thead>
    <tbody>${rows}</tbody>
  </table>`;
}

/** Recursion helper — returns rows only (no wrapping table). */
function buildJsonDiffRows(objA, objB, prefix) {
  const keysA = objA ? Object.keys(objA) : [];
  const keysB = objB ? Object.keys(objB) : [];
  const allKeys = new Set([...keysA, ...keysB]);
  const sorted = [...allKeys].sort();

  let rows = "";
  for (const key of sorted) {
    const fullKey = prefix ? prefix + "." + key : key;
    const valA = objA ? objA[key] : undefined;
    const valB = objB ? objB[key] : undefined;

    const bothObjects = valA && valB && typeof valA === "object" && typeof valB === "object"
      && !Array.isArray(valA) && !Array.isArray(valB);
    if (bothObjects) {
      rows += buildJsonDiffRows(valA, valB, fullKey);
      continue;
    }

    if (Array.isArray(valA) && Array.isArray(valB)) {
      const maxLen = Math.max(valA.length, valB.length);
      for (let i = 0; i < maxLen; i++) {
        const itemKey = `${fullKey}[${i}]`;
        const a = valA[i];
        const b = valB[i];
        if (a && b && typeof a === "object" && typeof b === "object" && !Array.isArray(a) && !Array.isArray(b)) {
          rows += buildJsonDiffRows(a, b, itemKey);
        } else {
          rows += buildDiffRow(itemKey, a, b);
        }
      }
      continue;
    }

    rows += buildDiffRow(fullKey, valA, valB);
  }
  return rows;
}

function buildDiffRow(fullKey, valA, valB) {
  const strA = valA !== undefined ? JSON.stringify(valA) : undefined;
  const strB = valB !== undefined ? JSON.stringify(valB) : undefined;

  let rowClass = "";
  let indicator = "";

  if (valA !== undefined && valB === undefined) {
    rowClass = "diff-removed";
    indicator = `<span class="diff-indicator removed">−</span>`;
  } else if (valA === undefined && valB !== undefined) {
    rowClass = "diff-added";
    indicator = `<span class="diff-indicator added">+</span>`;
  } else if (strA !== strB) {
    rowClass = "diff-changed";
    indicator = `<span class="diff-indicator changed">≠</span>`;
  } else {
    indicator = `<span class="diff-indicator" style="opacity:0.3">=</span>`;
  }

  const dispA = valA !== undefined ? escapeHtml(typeof valA === "object" ? JSON.stringify(valA) : String(valA)) : '<span style="opacity:0.3">—</span>';
  const dispB = valB !== undefined ? escapeHtml(typeof valB === "object" ? JSON.stringify(valB) : String(valB)) : '<span style="opacity:0.3">—</span>';

  return `<tr class="${rowClass}">
    <td>${indicator}</td>
    <td class="header-name-cell">${escapeHtml(fullKey)}</td>
    <td>${dispA}</td>
    <td>${dispB}</td>
  </tr>`;
}

function formatBody(body) {
  if (!body) return "(no body)";
  if (body.type === "formData") {
    return Object.entries(body.data)
      .map(([k, v]) => `${k}=${Array.isArray(v) ? v.join(", ") : v}`)
      .join("\n");
  }
  if (body.type === "raw") {
    try {
      const parsed = JSON.parse(body.data);
      return JSON.stringify(parsed, null, 2);
    } catch {
      return body.data || "(empty)";
    }
  }
  return "(unknown format)";
}

function buildHeadersDiff(headersA, headersB) {
  // Normalize headers into maps (lowercase name → value)
  const mapA = new Map();
  const mapB = new Map();
  headersA.forEach((h) => mapA.set(h.name.toLowerCase(), { name: h.name, value: h.value }));
  headersB.forEach((h) => mapB.set(h.name.toLowerCase(), { name: h.name, value: h.value }));

  // Collect all unique header names
  const allNames = new Set([...mapA.keys(), ...mapB.keys()]);
  const sorted = [...allNames].sort();

  if (sorted.length === 0) {
    return `<p style="color:var(--text-muted); font-size:12px;">No headers captured.</p>`;
  }

  let rows = "";
  for (const key of sorted) {
    const a = mapA.get(key);
    const b = mapB.get(key);

    let rowClass = "";
    let indicator = "";

    if (a && !b) {
      rowClass = "diff-removed";
      indicator = `<span class="diff-indicator removed">−</span>`;
    } else if (!a && b) {
      rowClass = "diff-added";
      indicator = `<span class="diff-indicator added">+</span>`;
    } else if (a && b && a.value !== b.value) {
      rowClass = "diff-changed";
      indicator = `<span class="diff-indicator changed">≠</span>`;
    } else {
      indicator = `<span class="diff-indicator" style="opacity:0.3">=</span>`;
    }

    const nameDisplay = a ? a.name : b.name;
    const valA = a ? escapeHtml(a.value) : `<span style="opacity:0.3">—</span>`;
    const valB = b ? escapeHtml(b.value) : `<span style="opacity:0.3">—</span>`;

    rows += `<tr class="${rowClass}">
      <td>${indicator}</td>
      <td class="header-name-cell">${escapeHtml(nameDisplay)}</td>
      <td>${valA}</td>
      <td>${valB}</td>
    </tr>`;
  }

  return `<table class="compare-headers-table">
    <thead>
      <tr><th></th><th>Header</th><th>Request A</th><th>Request B</th></tr>
    </thead>
    <tbody>${rows}</tbody>
  </table>`;
}

/* ============================================================
 * 13. Statistics Tab
 * ========================================================== */

async function renderStats() {
  const [netResponse, rulesResponse] = await Promise.all([
    sendMessage({ action: "getNetworkLog" }),
    sendMessage({ action: "getRules" }),
  ]);

  const allEntries = netResponse.log || [];
  const rules = rulesResponse.rules || [];
  const patterns = rules.map((r) => ({ rule: r, regex: urlPatternToRegex(r.urlPattern) }));

  const statsContent = $("#stats-content");
  if (!statsContent) return;

  if (allEntries.length === 0) {
    statsContent.innerHTML = `<div class="empty-state"><p>No network data available.</p><p class="sub">Capture some requests or import a HAR file.</p></div>`;
    return;
  }

  let html = "";

  // ── Resource type filter ───────────────────────────────────
  const prevTypeFilter = ($("#stats-filter-type") || {}).value || "";
  html += `<div class="stats-filter-bar">
    <div class="filter-group">
      <label class="filter-label">Type:</label>
      <select id="stats-filter-type" class="filter-select">
        <option value="">All</option>
        <option value="fetch"${prevTypeFilter === 'fetch' ? ' selected' : ''}>Fetch/XHR</option>
        <option value="doc"${prevTypeFilter === 'doc' ? ' selected' : ''}>Doc</option>
        <option value="js"${prevTypeFilter === 'js' ? ' selected' : ''}>JS</option>
        <option value="css"${prevTypeFilter === 'css' ? ' selected' : ''}>CSS</option>
        <option value="img"${prevTypeFilter === 'img' ? ' selected' : ''}>Img</option>
        <option value="media"${prevTypeFilter === 'media' ? ' selected' : ''}>Media</option>
        <option value="font"${prevTypeFilter === 'font' ? ' selected' : ''}>Font</option>
        <option value="ws"${prevTypeFilter === 'ws' ? ' selected' : ''}>WS</option>
        <option value="other"${prevTypeFilter === 'other' ? ' selected' : ''}>Other</option>
      </select>
    </div>
  </div>`;

  // Apply type filter
  let filteredEntries = allEntries;
  if (prevTypeFilter) {
    filteredEntries = allEntries.filter((e) => classifyResourceType(e.type) === prevTypeFilter);
  }

  // Ensure cachedNetworkLog has filtered entries for openComparison()
  cachedNetworkLog = filteredEntries;

  // ── Summary cards ──────────────────────────────────────────
  const totalRequests = filteredEntries.length;
  const successCount = filteredEntries.filter((e) => e.statusCode >= 200 && e.statusCode < 400).length;
  const clientErrors = filteredEntries.filter((e) => e.statusCode >= 400 && e.statusCode < 500).length;
  const serverErrors = filteredEntries.filter((e) => e.statusCode >= 500).length;
  const failedCount = filteredEntries.filter((e) => !e.statusCode || e.error).length;
  const withRules = filteredEntries.filter((e) => patterns.some((p) => p.regex.test(e.url))).length;
  const withoutRules = totalRequests - withRules;

  const durations = filteredEntries.map((e) => e.duration).filter((d) => d != null && d > 0).sort((a, b) => a - b);
  const avgDuration = durations.length ? Math.round(durations.reduce((a, b) => a + b, 0) / durations.length) : 0;
  const p50 = percentile(durations, 50);
  const p90 = percentile(durations, 90);
  const p95 = percentile(durations, 95);
  const p99 = percentile(durations, 99);
  const maxDuration = durations.length ? durations[durations.length - 1] : 0;

  html += `<div class="stats-summary">
    <div class="stat-card"><div class="stat-value">${totalRequests}</div><div class="stat-label">Total Requests</div></div>
    <div class="stat-card"><div class="stat-value success">${successCount}</div><div class="stat-label">Success (2xx/3xx)</div></div>
    <div class="stat-card"><div class="stat-value warning">${clientErrors}</div><div class="stat-label">Client Errors (4xx)</div></div>
    <div class="stat-card"><div class="stat-value danger">${serverErrors + failedCount}</div><div class="stat-label">Server Errors / Failed</div></div>
    <div class="stat-card"><div class="stat-value">${withRules}</div><div class="stat-label">Matched by Rules</div></div>
    <div class="stat-card"><div class="stat-value warning">${withoutRules}</div><div class="stat-label">No Rule Match</div></div>
  </div>`;

  // ── Response Time Percentiles ──────────────────────────────
  if (durations.length > 0) {
    html += `<div class="stats-section">
      <div class="stats-section-title">Response Time Percentiles</div>
      ${buildPercentileBar("Avg", avgDuration, maxDuration)}
      ${buildPercentileBar("P50", p50, maxDuration)}
      ${buildPercentileBar("P90", p90, maxDuration)}
      ${buildPercentileBar("P95", p95, maxDuration)}
      ${buildPercentileBar("P99", p99, maxDuration)}
      ${buildPercentileBar("Max", maxDuration, maxDuration)}
    </div>`;
  }

  // ── HTTP Methods Breakdown ─────────────────────────────────
  const methodCounts = {};
  filteredEntries.forEach((e) => {
    methodCounts[e.method] = (methodCounts[e.method] || 0) + 1;
  });
  html += `<div class="stats-section">
    <div class="stats-section-title">HTTP Methods</div>
    <div class="stats-methods">
      ${Object.entries(methodCounts).sort((a, b) => b[1] - a[1]).map(
        ([method, count]) => `<span class="stats-method-chip">${escapeHtml(method)}<span class="method-count">${count}</span></span>`
      ).join("")}
    </div>
  </div>`;

  // ── Domain / Path Tree ─────────────────────────────────────
  const domainMap = new Map();
  filteredEntries.forEach((entry) => {
    try {
      const u = new URL(entry.url);
      const domain = u.origin;
      const path = u.pathname;
      if (!domainMap.has(domain)) domainMap.set(domain, new Map());
      const pathMap = domainMap.get(domain);
      if (!pathMap.has(path)) pathMap.set(path, []);
      pathMap.get(path).push(entry);
    } catch {}
  });

  html += `<div class="stats-section">
    <div class="stats-section-title">Requests by Domain & Path</div>
    <div style="display:flex;justify-content:flex-end;gap:8px;margin-bottom:8px;align-items:center;">
      <span id="stats-selection-count" class="selection-count hidden">0 selected</span>
      <button id="btn-stats-compare" class="btn btn-primary btn-sm hidden">Compare</button>
      <button class="btn btn-ghost btn-sm" id="btn-expand-all">Expand All</button>
    </div>`;

  const sortedDomains = [...domainMap.entries()].sort((a, b) => {
    const countA = [...a[1].values()].reduce((s, arr) => s + arr.length, 0);
    const countB = [...b[1].values()].reduce((s, arr) => s + arr.length, 0);
    return countB - countA;
  });

  for (const [domain, pathMap] of sortedDomains) {
    const domainEntries = [...pathMap.values()].flat();
    const domainOk = domainEntries.filter((e) => e.statusCode >= 200 && e.statusCode < 400).length;
    const domainFail = domainEntries.length - domainOk;
    const domainHasRule = patterns.some((p) => domainEntries.some((e) => p.regex.test(e.url)));
    const domainRuleBtn = domainHasRule
      ? ''
      : `<button class="btn-add-rule" data-url="${escapeAttr(domain + '/*')}" title="Add rule for this domain">+ Add Rule</button>`;

    const domainKey = domain;
    const domainExpanded = statsExpandedDomains.has(domainKey);

    html += `<div class="domain-group">
      <div class="domain-header" data-domain="${escapeAttr(domainKey)}">
        <span class="chevron">${domainExpanded ? '▼' : '▶'}</span>
        <span class="domain-name">${escapeHtml(domain)}</span>
        <div class="domain-stats">
          ${domainRuleBtn}
          <span class="domain-stat">${domainEntries.length} reqs</span>
          <span class="domain-stat ok">✓ ${domainOk}</span>
          ${domainFail > 0 ? `<span class="domain-stat fail">✗ ${domainFail}</span>` : ''}
        </div>
      </div>
      <div class="path-list${domainExpanded ? '' : ' hidden'}">`;

    const sortedPaths = [...pathMap.entries()].sort((a, b) => b[1].length - a[1].length);
    for (const [path, entries] of sortedPaths) {
      const sampleUrl = entries[0].url;
      const matchingRules = patterns.filter((p) => p.regex.test(sampleUrl));
      const hasRule = matchingRules.length > 0;
      const ruleBadges = hasRule
        ? matchingRules.map((p) => `<span class="path-rule-badge">${escapeHtml(p.rule.type)}</span>`).join("")
        : `<span class="path-no-rule">no rule</span>`;

      const addRuleBtn = !hasRule
        ? `<button class="btn-add-rule" data-url="${escapeAttr(domain + path + '*')}" title="Create a rule for this path">+ Add Rule</button>`
        : '';

      const pathDurations = entries.map((e) => e.duration).filter((d) => d != null);
      const pathAvg = pathDurations.length ? Math.round(pathDurations.reduce((a, b) => a + b, 0) / pathDurations.length) : null;

      const pathKey = domain + path;
      const pathExpanded = statsExpandedPaths.has(pathKey);

      html += `<div class="path-group">
        <div class="path-row" data-path-key="${escapeAttr(pathKey)}">
          <span class="chevron">${pathExpanded ? '▼' : '▶'}</span>
          <span class="path-name">${escapeHtml(path)}</span>
          <div class="path-badges">
            <span class="path-count">${entries.length}×</span>
            ${pathAvg != null ? '<span class="log-duration">' + pathAvg + 'ms</span>' : ''}
            ${ruleBadges}
            ${addRuleBtn}
          </div>
        </div>
        <div class="request-list${pathExpanded ? '' : ' hidden'}">
          ${entries.map((e) => renderStatsRequestRow(e)).join("")}
        </div>
      </div>`;
    }

    html += `</div></div>`;
  }

  html += `</div>`;

  statsContent.innerHTML = html;

  // Wire up stats type filter
  const statsTypeSelect = statsContent.querySelector("#stats-filter-type");
  if (statsTypeSelect) {
    statsTypeSelect.addEventListener("change", renderStats);
  }

  // Wire up "Add Rule" buttons — open modal pre-populated, don't save yet
  statsContent.querySelectorAll(".btn-add-rule").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const urlPattern = btn.dataset.url;
      openAddModalWithUrl(urlPattern);
    });
  });

  // Wire up Expand All / Collapse All toggle
  const btnExpandAll = statsContent.querySelector("#btn-expand-all");
  if (btnExpandAll) {
    btnExpandAll.addEventListener("click", () => {
      const expanding = btnExpandAll.textContent.includes("Expand");
      statsContent.querySelectorAll(".path-list").forEach((pl) => pl.classList.toggle("hidden", !expanding));
      statsContent.querySelectorAll(".request-list").forEach((rl) => rl.classList.toggle("hidden", !expanding));
      statsContent.querySelectorAll(".domain-header .chevron, .path-row .chevron").forEach((ch) => {
        ch.textContent = expanding ? "▼" : "▶";
      });
      // Sync persistent state
      if (expanding) {
        statsContent.querySelectorAll(".domain-header[data-domain]").forEach((h) => statsExpandedDomains.add(h.dataset.domain));
        statsContent.querySelectorAll(".path-row[data-path-key]").forEach((r) => statsExpandedPaths.add(r.dataset.pathKey));
      } else {
        statsExpandedDomains.clear();
        statsExpandedPaths.clear();
      }
      btnExpandAll.textContent = expanding ? "Collapse All" : "Expand All";
    });
  }

  // Wire up domain header collapse/expand with state persistence
  statsContent.querySelectorAll(".domain-header").forEach((header) => {
    header.addEventListener("click", (e) => {
      if (e.target.closest(".btn-add-rule")) return;
      const pathList = header.parentElement.querySelector(".path-list");
      const chevron = header.querySelector(".chevron");
      const domainKey = header.dataset.domain;
      if (pathList) {
        const isHidden = pathList.classList.toggle("hidden");
        if (chevron) chevron.textContent = isHidden ? "▶" : "▼";
        if (domainKey) {
          if (isHidden) statsExpandedDomains.delete(domainKey);
          else statsExpandedDomains.add(domainKey);
        }
      }
    });
  });

  // Wire up path row collapse/expand with state persistence
  statsContent.querySelectorAll(".path-row").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".btn-add-rule")) return;
      if (e.target.classList.contains("stats-checkbox")) return;
      const reqList = row.parentElement.querySelector(".request-list");
      const chevron = row.querySelector(".chevron");
      const pathKey = row.dataset.pathKey;
      if (reqList) {
        const isHidden = reqList.classList.toggle("hidden");
        if (chevron) chevron.textContent = isHidden ? "▶" : "▼";
        if (pathKey) {
          if (isHidden) statsExpandedPaths.delete(pathKey);
          else statsExpandedPaths.add(pathKey);
        }
      }
    });
  });

  // Wire up stats checkboxes for compare
  statsContent.querySelectorAll(".stats-checkbox").forEach((cb) => {
    cb.addEventListener("change", (e) => {
      e.stopPropagation();
      const id = cb.dataset.id;
      if (cb.checked) {
        if (selectedRequests.size >= 2) {
          cb.checked = false;
          return;
        }
        selectedRequests.add(id);
      } else {
        selectedRequests.delete(id);
      }
      updateStatsSelectionUI();
      // Sync visual state on rows
      statsContent.querySelectorAll(".stats-request-row").forEach((row) => {
        const rid = row.dataset.entryId;
        row.classList.toggle("selected", selectedRequests.has(rid));
      });
      // Disable unchecked when 2 selected
      if (selectedRequests.size >= 2) {
        statsContent.querySelectorAll(".stats-checkbox").forEach((c) => {
          if (!c.checked) c.disabled = true;
        });
      } else {
        statsContent.querySelectorAll(".stats-checkbox").forEach((c) => {
          c.disabled = false;
        });
      }
    });
    cb.addEventListener("click", (e) => e.stopPropagation());
  });

  // Allow clicking stats request row to toggle checkbox
  statsContent.querySelectorAll(".stats-request-row").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.classList.contains("stats-checkbox")) return;
      if (e.target.closest(".btn-view-detail")) return;
      const cb = row.querySelector(".stats-checkbox");
      if (cb && !cb.disabled) {
        cb.checked = !cb.checked;
        cb.dispatchEvent(new Event("change"));
      }
    });
  });

  // Wire up stats compare button — reuse openComparison()
  const btnStatsCompare = statsContent.querySelector("#btn-stats-compare");
  if (btnStatsCompare) {
    btnStatsCompare.addEventListener("click", openComparison);
  }

  // Disable unchecked checkboxes if 2 already selected (initial render)
  if (selectedRequests.size >= 2) {
    statsContent.querySelectorAll(".stats-checkbox").forEach((c) => {
      if (!c.checked) c.disabled = true;
    });
  }

  updateStatsSelectionUI();

  // Wire up View buttons in stats request rows
  statsContent.querySelectorAll(".btn-view-detail").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const entryId = btn.dataset.entryId;
      const entry = filteredEntries.find((e) => e.id === entryId);
      if (entry) showRequestDetail(entry);
    });
  });
}

function renderStatsRequestRow(entry) {
  const statusClass = getStatusClass(entry.statusCode);
  const statusText = entry.statusCode || (entry.error ? "ERR" : "…");
  const checked = selectedRequests.has(entry.id) ? "checked" : "";
  const selectedClass = selectedRequests.has(entry.id) ? " selected" : "";
  return `<div class="stats-request-row${selectedClass}" data-entry-id="${escapeAttr(entry.id)}">
    <input type="checkbox" class="stats-checkbox" ${checked} data-id="${escapeAttr(entry.id)}" />
    <span class="log-status ${statusClass}">${statusText}</span>
    <span class="log-method">${escapeHtml(entry.method)}</span>
    <span class="stats-req-url">${escapeHtml(entry.url)}</span>
    ${entry.duration != null ? '<span class="log-duration">' + entry.duration + 'ms</span>' : ''}
    <span class="log-time">${formatTime(entry.timestamp)}</span>
    <button class="btn-view-detail${viewedEntries.has(entry.id) ? ' viewed' : ''}" data-entry-id="${escapeAttr(entry.id)}" title="View details">${viewedEntries.has(entry.id) ? '✓ Viewed' : '👁 View'}</button>
  </div>`;
}

function updateStatsSelectionUI() {
  const countEl = $("#stats-selection-count");
  const compareBtn = $("#btn-stats-compare");
  if (!countEl || !compareBtn) return;

  if (selectedRequests.size > 0) {
    countEl.textContent = `${selectedRequests.size} selected`;
    countEl.classList.remove("hidden");
  } else {
    countEl.classList.add("hidden");
  }

  compareBtn.classList.toggle("hidden", selectedRequests.size !== 2);
}

async function showRequestDetail(entry) {
  viewedEntries.add(entry.id);
  // Update only the view buttons (not the row containers)
  $$(`.btn-view-detail[data-entry-id="${entry.id}"]`).forEach((btn) => {
    btn.classList.add("viewed");
    btn.textContent = "✓ Viewed";
  });
  $$(`.log-view-btn[data-entry-id="${entry.id}"]`).forEach((btn) => {
    btn.classList.add("viewed");
    btn.textContent = "✓ Viewed";
  });

  const respBody = await getResponseBody(entry);
  entry.responseBody = respBody;

  const reqContent = $("#compare-tab-request");
  const resContent = $("#compare-tab-response");

  reqContent.innerHTML = buildSingleRequestHTML(entry);
  resContent.innerHTML = buildSingleResponseHTML(entry);

  // Reset to Request tab
  $$(".compare-tab-btn").forEach((b) => b.classList.toggle("active", b.dataset.ctab === "request"));
  $$(".compare-tab-content").forEach((c) => {
    c.classList.toggle("active", c.id === "compare-tab-request");
    c.classList.toggle("hidden", c.id !== "compare-tab-request");
  });

  // Update overlay title
  $("#compare-overlay .compare-header h2").textContent = "Request Detail";
  $("#compare-overlay").classList.remove("hidden");
}

function buildSingleRequestHTML(entry) {
  let html = `<div class="compare-section">
    <div class="compare-section-title">General</div>
    <div class="compare-column" style="max-width:100%">
      <div class="compare-info-row"><span class="compare-info-label">URL</span><span class="compare-info-value">${escapeHtml(entry.url)}</span></div>
      <div class="compare-info-row"><span class="compare-info-label">Method</span><span class="compare-info-value">${escapeHtml(entry.method)}</span></div>
      <div class="compare-info-row"><span class="compare-info-label">Status</span><span class="compare-info-value log-status ${getStatusClass(entry.statusCode)}">${entry.statusCode || 'N/A'}</span></div>
      <div class="compare-info-row"><span class="compare-info-label">Duration</span><span class="compare-info-value">${entry.duration != null ? entry.duration + 'ms' : 'N/A'}</span></div>
      <div class="compare-info-row"><span class="compare-info-label">Time</span><span class="compare-info-value">${formatTime(entry.timestamp)}</span></div>
      <div class="compare-info-row"><span class="compare-info-label">Type</span><span class="compare-info-value">${escapeHtml(entry.type || 'N/A')}</span></div>
    </div>
  </div>`;

  html += `<div class="compare-section">
    <div class="compare-section-title">Request Headers (${(entry.requestHeaders || []).length})</div>
    ${buildSingleHeadersTable(entry.requestHeaders || [])}
  </div>`;

  const body = formatBody(entry.requestBody);
  if (body !== "(no body)") {
    html += `<div class="compare-section">
      <div class="compare-section-title">Request Body</div>
      <pre style="font-size:12px;white-space:pre-wrap;word-break:break-all;margin:0;color:var(--text);background:var(--bg-input);padding:12px;border-radius:var(--radius-sm);max-height:400px;overflow-y:auto">${escapeHtml(body)}</pre>
    </div>`;
  }

  return html;
}

function buildSingleResponseHTML(entry) {
  let html = `<div class="compare-section">
    <div class="compare-section-title">Response Headers (${(entry.responseHeaders || []).length})</div>
    ${buildSingleHeadersTable(entry.responseHeaders || [])}
  </div>`;

  const respBody = entry.responseBody || "(not captured)";
  function prettyPrint(text) {
    try { return JSON.stringify(JSON.parse(text), null, 2); } catch { return text; }
  }
  html += `<div class="compare-section">
    <div class="compare-section-title">Response Body</div>
    <pre style="font-size:12px;white-space:pre-wrap;word-break:break-all;margin:0;color:var(--text);background:var(--bg-input);padding:12px;border-radius:var(--radius-sm);max-height:400px;overflow-y:auto">${escapeHtml(prettyPrint(respBody))}</pre>
  </div>`;

  return html;
}

function buildSingleHeadersTable(headers) {
  if (headers.length === 0) return `<p style="color:var(--text-muted);font-size:12px;">No headers.</p>`;
  let rows = headers.map((h) =>
    `<tr><td class="header-name-cell">${escapeHtml(h.name)}</td><td>${escapeHtml(h.value)}</td></tr>`
  ).join("");
  return `<table class="compare-headers-table"><thead><tr><th>Header</th><th>Value</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function percentile(sortedArr, p) {
  if (sortedArr.length === 0) return 0;
  const idx = Math.ceil((p / 100) * sortedArr.length) - 1;
  return sortedArr[Math.max(0, idx)];
}

function buildPercentileBar(label, value, max) {
  const pct = max > 0 ? (value / max) * 100 : 0;
  const color = value < 200 ? "var(--success)" : value < 500 ? "var(--warning)" : "var(--danger)";
  return `<div class="percentile-bar">
    <span class="percentile-label">${label}</span>
    <div class="percentile-track">
      <div class="percentile-fill" style="width:${pct}%;background:${color}"></div>
    </div>
    <span class="percentile-value">${value}ms</span>
  </div>`;
}

/* ============================================================
 * 11. Theme Management
 * ========================================================== */

/** Load saved theme preference and apply it. */
async function applyTheme() {
  try {
    const data = await chrome.storage.local.get("requestPilotTheme");
    const theme = data.requestPilotTheme || "dark";
    document.body.setAttribute("data-theme", theme);
  } catch {
    document.body.setAttribute("data-theme", "dark");
  }
}

/** Toggle between light and dark themes. */
async function toggleTheme() {
  const current = document.body.getAttribute("data-theme") || "dark";
  const next = current === "dark" ? "light" : "dark";
  document.body.setAttribute("data-theme", next);
  await chrome.storage.local.set({ requestPilotTheme: next });
}

/* ============================================================
 * 12. Pop-out & Standalone Mode
 * ========================================================== */

/** Detect if running as a tab (not popup) and apply standalone class. */
function detectStandaloneMode() {
  // When opened as a tab, window.innerWidth is much larger than the popup width
  if (window.location.search.includes("standalone=1") || window.innerWidth > 650) {
    document.body.classList.add("standalone");
  }
}

/** Open the popup.html in a new browser tab so the user can resize freely. */
function popOutToTab() {
  const url = chrome.runtime.getURL("popup.html?standalone=1");
  chrome.tabs.create({ url });
  window.close(); // close the popup
}
