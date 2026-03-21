/* ============================================================
   Request Pilot — Guided Tours
   Overview tour + detailed hands-on walkthrough
   ============================================================ */

// Reuse $ and $$ from app.js (global scope)

// ---- Overview Tour Steps ----
const overviewSteps = [
  {
    target: '.toolbar',
    title: 'Welcome to Request Pilot! ✈',
    body: 'This is your API testing workbench. Let\'s take a quick tour of the key features. Re-open anytime via the ❓ button.',
    position: 'bottom'
  },
  {
    target: '#filesSection',
    title: '📁 Files Panel',
    body: 'Load .http test files here. Click 📂 to open a file, or 📄 to create a new one. Files appear as a tree with groups, setup, and teardown blocks.',
    position: 'right'
  },
  {
    target: '#envSection',
    title: '🔐 Variables & Environment',
    body: 'Variables from your .http files and .env files appear here. You can add, edit, or override variables on the fly. They\'re substituted into <code>{{variable}}</code> placeholders.',
    position: 'right'
  },
  {
    target: '#modeToggle',
    title: '🔧 View Modes',
    body: '<b>Builder</b> — Form-based request editor.<br><b>Code</b> — Raw .http editor.<br><b>History</b> — Past requests with stats, filtering, and comparison.',
    position: 'bottom'
  },
  {
    target: '#runAllBtn',
    title: '▶ Run All Tests',
    body: 'Runs all enabled test blocks in parallel. Setup → tests (respecting dependencies) → teardown. Shortcut: <kbd>Ctrl+Shift+Enter</kbd>.',
    position: 'bottom'
  },
  {
    target: '#requestPanel',
    title: '📤 Request Builder',
    body: 'Set HTTP method, URL, headers, and body. Click <b>Send</b> for a single request, or <b>Run All</b> for the full suite.',
    position: 'top'
  },
  {
    target: '#responsePanel',
    title: '📥 Response Viewer',
    body: 'Responses with status, headers, and JSON tree viewer. Syntax-highlighted with expand/collapse at every level.',
    position: 'top'
  },
  {
    target: '#zoomIndicator',
    title: '🔍 Zoom Controls',
    body: '<kbd>Ctrl+Scroll</kbd> or <kbd>Ctrl+/−</kbd> to zoom. <kbd>Ctrl+0</kbd> resets to 100%.',
    position: 'bottom'
  }
];

// ---- Detailed Tour Steps ----
// Built dynamically via buildDetailedSteps() so we can capture initial state
function buildDetailedSteps() {
  // Snapshot current state so waitFor checks for NEW actions, not existing state
  const initialFileCount = (typeof loadedFiles !== 'undefined') ? loadedFiles.length : 0;

  return [
    {
      target: '.toolbar',
      title: 'Welcome to the Hands-On Tour! 🎓',
      body: 'We\'ll walk through every feature step by step. First, let\'s load a sample file with real API tests.',
      position: 'bottom'
    },
    {
      target: '#filesSection .sidebar-section-header',
      title: '📂 Load the Sample File',
      body: 'Click the <b>📂 button</b> above, then select <code>sample-getting-started.http</code> from the file picker.<br><br>💡 The sample uses public Microsoft Azure and jsonplaceholder APIs — no setup needed!',
      position: 'right',
      interactive: true,
      action: () => {
        // Expand files section so buttons are visible
        const section = document.querySelector('#filesSection');
        if (section && !section.classList.contains('expanded')) {
          const header = section.querySelector('.sidebar-section-header');
          if (header) header.click();
        }
        // Force the open button visible
        const btn = document.querySelector('#openFileBtn');
        if (btn) btn.style.opacity = '1';
      },
      waitFor: () => (typeof loadedFiles !== 'undefined') && loadedFiles.length > initialFileCount,
      waitMessage: '⏳ Click 📂 to load a file…'
    },
    {
      target: '#filesSection',
      title: '📁 File Tree & Groups',
      body: 'Your file is loaded! Blocks are organized into <b>groups</b> — setup, azure-discovery, crud-operations, error-handling. Each group can be expanded/collapsed. Click a test block to view its details.',
      position: 'right',
      action: () => {
        // Expand the files section and first file so groups are visible
        const section = document.querySelector('#filesSection');
        if (section && !section.classList.contains('expanded')) {
          const header = section.querySelector('.sidebar-section-header');
          if (header) header.click();
        }
        // Expand first file node
        const firstFile = document.querySelector('.file-node:not(.expanded)');
        if (firstFile) {
          const header = firstFile.querySelector('.file-header');
          if (header) header.click();
        }
        // Select first test block so builder shows something
        if (typeof loadedFiles !== 'undefined' && loadedFiles.length > 0) {
          const file = loadedFiles[loadedFiles.length - 1];
          if (file && file.suite && file.suite.blocks.length > 0) {
            selectBlock(loadedFiles.length - 1, 0);
          }
        }
      }
    },
    {
      target: '#envSection',
      title: '🔐 Variables',
      body: 'Variables from the file\'s <code>@variables</code> block appear here: <b>azure_base</b>, <b>json_api</b>, etc. You can edit values or add new ones. They\'re substituted into <code>{{variable}}</code> in URLs, headers, and bodies.',
      position: 'right'
    },
    {
      target: '#requestPanel',
      title: '📤 Builder Mode — Request',
      body: 'This panel shows the selected request\'s <b>method</b>, <b>URL</b> (with variables resolved), <b>headers</b>, and <b>body</b>. We\'ve auto-selected the first block so you can see it in action.',
      position: 'top'
    },
    {
      target: '#responsePanel',
      title: '📥 Builder Mode — Response',
      body: 'After sending a request, the response appears here with:<br>• <b>Status</b> badge (colored by code)<br>• <b>Headers</b> table<br>• <b>JSON tree</b> with syntax highlighting and expand/collapse',
      position: 'top'
    },
    {
      target: '[data-mode="code"]',
      title: '📝 Code Mode',
      body: 'Click the <b>Code</b> tab to see and edit the raw .http file. You can edit requests, add assertions (<code># @assert</code>), extractions (<code># @extract</code>), and dependencies (<code># @depends</code>) directly.',
      position: 'bottom',
      action: () => {
        const codeBtn = document.querySelector('[data-mode="code"]');
        if (codeBtn) codeBtn.click();
      }
    },
    {
      target: '[data-mode="builder"]',
      title: '🔧 Back to Builder',
      body: 'Switch back to <b>Builder</b> mode. Changes made in Code mode are automatically synced.',
      position: 'bottom',
      action: () => {
        const builderBtn = document.querySelector('[data-mode="builder"]');
        if (builderBtn) builderBtn.click();
      }
    },
    {
      target: '#runAllBtn',
      title: '▶ Run All Tests',
      body: 'Now let\'s run the full suite! Click <b>Run All</b> (or press <kbd>Ctrl+Shift+Enter</kbd>). Watch the sidebar update with pass/fail status dots, and results stream in below.',
      position: 'bottom',
      interactive: true,
      waitFor: () => {
        const bar = document.querySelector('#testResultsBar');
        return bar && !bar.classList.contains('hidden');
      },
      waitMessage: '⏳ Click Run All — waiting for test run…'
    },
    {
      target: '#testResultsBar',
      title: '📊 Test Results',
      body: 'Results appear here as tests complete. Each row shows:<br>• <b>Status dot</b> (green = pass, red = fail)<br>• <b>Response time</b><br>• <b>Failure reason</b> (HTTP status or assertion)<br><br>Click a row to inspect that request\'s full response. The 👁 icon tracks what you\'ve viewed.',
      position: 'top'
    },
    {
      target: '#filesSection',
      title: '✅ Sidebar Status',
      body: 'The sidebar now shows status dots next to each test block — green for passed, red for failed. Group headers show aggregate status. Hover over any test or group to see <b>detailed tooltips</b> with run history and trends.',
      position: 'right'
    },
    {
      target: '[data-mode="history"]',
      title: '📊 History Mode',
      body: 'Click <b>History</b> to see all past requests. Let\'s explore!',
      position: 'bottom',
      action: () => {
        const histBtn = document.querySelector('[data-mode="history"]');
        if (histBtn) histBtn.click();
      }
    },
    {
      target: '.history-filters',
      title: '🔍 History Filters',
      body: 'Filter requests by <b>method</b>, <b>status code</b>, <b>source</b> (test vs manual), and <b>URL search</b>. The URL search has <b>autocomplete</b> — start typing and press Tab to accept suggestions.',
      position: 'bottom'
    },
    {
      target: '#historyStats',
      title: '📈 History Stats',
      body: 'Summary cards show total requests, success rate, average response time, and more. Below, requests are grouped by <b>domain → path</b>. Click a group to expand and see individual requests.',
      position: 'bottom',
      action: () => {
        // Expand domain groups so user can see the grouped structure
        const groups = document.querySelectorAll('.hist-domain-group:not(.expanded)');
        if (groups.length > 0) groups[0].classList.add('expanded');
        // Also scroll the history log into view
        const histLog = document.querySelector('#historyLog');
        if (histLog) histLog.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      }
    },
    {
      target: '#historyLog',
      title: '📋 Grouped Requests',
      body: 'Requests are organized by <b>domain → path</b>. Each group shows method badges, status breakdown, and average response times. Click a group row to expand and see individual request entries with timestamps.',
      position: 'top',
      action: () => {
        const histLog = document.querySelector('#historyLog');
        if (histLog) histLog.scrollIntoView({ behavior: 'smooth', block: 'start' });
      }
    },
    {
      target: '[data-mode="builder"]',
      title: '🎉 Tour Complete!',
      body: 'You\'ve seen all the key features! A few more tips:<br>• <b>Ctrl+Scroll</b> to zoom in/out<br>• <b>Hover</b> over sidebar items for rich tooltips<br>• <b>Select 2 history items</b> to compare them side by side<br>• <b>Enable/disable</b> individual test blocks via sidebar checkboxes<br><br>Happy testing! ✈',
      position: 'bottom',
      action: () => {
        const builderBtn = document.querySelector('[data-mode="builder"]');
        if (builderBtn) builderBtn.click();
      }
    }
  ];
}

// ---- Tour Engine ----
let currentSteps = [];
let currentStep = 0;
let tourActive = false;
let waitCheckInterval = null;
let isInteractiveStep = false;
let backdropCloseGrace = 0; // timestamp until which backdrop clicks are ignored

const backdropEl = () => $('#tourBackdrop');

function showPicker() {
  $('#tourPickerOverlay').classList.remove('hidden');
}

function hidePicker() {
  $('#tourPickerOverlay').classList.add('hidden');
}

function startTour(steps) {
  hidePicker();
  currentSteps = steps;
  currentStep = 0;
  tourActive = true;
  isInteractiveStep = false;
  $('#tourOverlay').classList.remove('hidden');
  setBackdropInteractive(false);
  showStep();
}

function endTour() {
  tourActive = false;
  isInteractiveStep = false;
  $('#tourOverlay').classList.add('hidden');
  localStorage.setItem('rp-tour-seen', 'true');
  $$('.tour-highlight').forEach(el => el.classList.remove('tour-highlight', 'tour-interactive'));
  setBackdropInteractive(false);
  clearWaitCheck();
}

function clearWaitCheck() {
  if (waitCheckInterval) {
    clearInterval(waitCheckInterval);
    waitCheckInterval = null;
  }
}

// When interactive, backdrop becomes click-through so user can interact with UI
function setBackdropInteractive(interactive) {
  isInteractiveStep = interactive;
  const bd = backdropEl();
  if (!bd) return;
  bd.style.pointerEvents = interactive ? 'none' : 'auto';
  bd.style.background = interactive
    ? 'rgba(0, 0, 0, 0.3)'   // lighter so user sees the UI clearly
    : 'rgba(0, 0, 0, 0.55)';
}

function showStep() {
  clearWaitCheck();
  const step = currentSteps[currentStep];
  const badge = $('#tourStepBadge');
  const title = $('#tourTitle');
  const body = $('#tourBody');
  const prevBtn = $('#tourPrevBtn');
  const nextBtn = $('#tourNextBtn');
  const tooltip = $('#tourTooltip');

  badge.textContent = `${currentStep + 1}/${currentSteps.length}`;
  title.textContent = step.title;
  body.innerHTML = step.body;

  prevBtn.style.display = currentStep === 0 ? 'none' : '';
  const isLast = currentStep === currentSteps.length - 1;
  nextBtn.textContent = isLast ? '✓ Finish' : 'Next →';
  nextBtn.disabled = false; // Reset immediately; waitFor will re-disable if needed

  // Remove previous highlight
  $$('.tour-highlight').forEach(el => {
    el.classList.remove('tour-highlight', 'tour-interactive');
  });

  // Execute action if defined (e.g., switch mode) — catch errors so tour doesn't break
  if (step.action) {
    try { step.action(); } catch (e) { console.warn('[Tour] action error:', e); }
  }

  // Determine if this is an interactive step (user must do something)
  const needsWait = step.waitFor && !step.waitFor();
  setBackdropInteractive(needsWait && step.interactive);

  // Highlight target
  const targetEl = document.querySelector(step.target);
  if (targetEl) {
    targetEl.classList.add('tour-highlight');
    if (step.interactive) targetEl.classList.add('tour-interactive');
    targetEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  }

  positionTooltip(targetEl, step.position);

  // Handle waitFor steps — disable Next until condition is met
  if (needsWait) {
    nextBtn.disabled = true;
    nextBtn.textContent = step.waitMessage || '⏳ Waiting…';
    waitCheckInterval = setInterval(() => {
      if (step.waitFor()) {
        clearWaitCheck();
        setBackdropInteractive(false);
        // Grace period: ignore backdrop clicks for 1s after transitioning
        // from interactive → non-interactive (native dialogs can fire stray clicks)
        backdropCloseGrace = Date.now() + 1000;
        $$('.tour-interactive').forEach(el => el.classList.remove('tour-interactive'));
        nextBtn.disabled = false;
        nextBtn.textContent = isLast ? '✓ Finish' : 'Next →';
      }
    }, 500);
  }
}

function positionTooltip(targetEl, position) {
  const tooltip = $('#tourTooltip');
  const TOOLTIP_W = 360;
  const GAP = 12;
  const MARGIN = 12;
  const vw = window.innerWidth;
  const vh = window.innerHeight;

  // Reset all positioning
  tooltip.style.top = '';
  tooltip.style.bottom = '';
  tooltip.style.left = '';
  tooltip.style.right = '';

  if (!targetEl) {
    tooltip.style.top = `${vh / 2 - 100}px`;
    tooltip.style.left = `${(vw - TOOLTIP_W) / 2}px`;
    return;
  }

  const rect = targetEl.getBoundingClientRect();
  let top, left;

  if (position === 'bottom') {
    top = rect.bottom + GAP;
    left = Math.max(MARGIN, Math.min(rect.left, vw - TOOLTIP_W - MARGIN));
    if (top + 200 > vh) {
      top = Math.max(MARGIN, rect.top - GAP - 200);
    }
  } else if (position === 'right') {
    top = Math.max(MARGIN, rect.top);
    left = rect.right + GAP;
    if (left + TOOLTIP_W > vw - MARGIN) {
      left = Math.max(MARGIN, rect.left - TOOLTIP_W - GAP);
    }
  } else if (position === 'top') {
    left = Math.max(MARGIN, Math.min(rect.left, vw - TOOLTIP_W - MARGIN));
    top = rect.top - GAP - 200;
    if (top < MARGIN) {
      top = rect.bottom + GAP;
    }
  }

  // Final clamp
  top = Math.max(MARGIN, Math.min(top, vh - 220));
  left = Math.max(MARGIN, Math.min(left, vw - TOOLTIP_W - MARGIN));

  tooltip.style.position = 'fixed';
  tooltip.style.top = `${top}px`;
  tooltip.style.left = `${left}px`;
}

// ---- Event Listeners ----
$('#tourNextBtn').addEventListener('click', () => {
  if (currentStep >= currentSteps.length - 1) {
    endTour();
  } else {
    currentStep++;
    showStep();
  }
});

$('#tourPrevBtn').addEventListener('click', () => {
  if (currentStep > 0) {
    currentStep--;
    showStep();
  }
});

$('#tourSkipBtn').addEventListener('click', endTour);
$('#tourBackdrop').addEventListener('click', () => {
  // Ignore clicks during grace period (native dialog can fire stray events)
  if (Date.now() < backdropCloseGrace) return;
  // Backdrop click only dismisses on non-interactive steps
  if (!isInteractiveStep) endTour();
});

// Picker events
$('#tourOverviewBtn').addEventListener('click', () => startTour(overviewSteps));
$('#tourDetailedBtn').addEventListener('click', () => startTour(buildDetailedSteps()));
$('#tourPickerDismiss').addEventListener('click', () => {
  hidePicker();
  localStorage.setItem('rp-tour-seen', 'true');
});
$('#tourPickerBackdrop').addEventListener('click', () => {
  hidePicker();
  localStorage.setItem('rp-tour-seen', 'true');
});

// Help button shows picker
$('#helpBtn').addEventListener('click', showPicker);

// First launch — show picker
if (!localStorage.getItem('rp-tour-seen')) {
  setTimeout(showPicker, 500);
}
