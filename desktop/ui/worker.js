/* ============================================================
   Request Pilot - Web Worker for Heavy Text Processing
   Handles escapeHtml, JSON parse/stringify, body formatting,
   and text search off the main thread.
   ============================================================ */

// --- escapeHtml ---
function escapeHtmlWorker(str) {
  if (typeof str !== 'string') return '';
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}

// --- JSON stringify with sorted keys ---
function sortKeys(obj) {
  if (obj === null || typeof obj !== 'object') return obj;
  if (Array.isArray(obj)) return obj.map(sortKeys);
  const sorted = {};
  const keys = Object.keys(obj).sort();
  for (let i = 0; i < keys.length; i++) {
    sorted[keys[i]] = sortKeys(obj[keys[i]]);
  }
  return sorted;
}

function jsonStringifySorted(data, indent) {
  return JSON.stringify(sortKeys(data), null, indent);
}

// --- Format body into lines ---
function formatBodyWorker(body, contentType) {
  if (!body || typeof body !== 'string') return [];
  const ct = (contentType || '').toLowerCase();
  if (ct.includes('json') || ct.includes('javascript')) {
    try {
      const parsed = JSON.parse(body);
      const pretty = jsonStringifySorted(parsed, 2);
      return pretty.split('\n');
    } catch {
      return body.split('\n');
    }
  }
  return body.split('\n');
}

// --- Search text ---
function searchTextWorker(text, query, maxResults) {
  if (!text || !query) return [];
  const results = [];
  const lines = text.split('\n');
  const lowerQuery = query.toLowerCase();
  for (let i = 0; i < lines.length && results.length < maxResults; i++) {
    const lowerLine = lines[i].toLowerCase();
    const idx = lowerLine.indexOf(lowerQuery);
    if (idx !== -1) {
      results.push({
        lineNumber: i + 1,
        lineText: lines[i],
        matchStart: idx,
        matchEnd: idx + query.length,
      });
    }
  }
  return results;
}

// --- Message handler ---
self.onmessage = function (e) {
  const { type, id, payload } = e.data;
  try {
    let result;
    switch (type) {
      case 'escapeHtml':
        result = escapeHtmlWorker(payload.text);
        break;
      case 'jsonParse':
        result = JSON.parse(payload.text);
        break;
      case 'jsonStringify':
        result = jsonStringifySorted(payload.data, payload.indent || 2);
        break;
      case 'formatBody':
        result = formatBodyWorker(payload.body, payload.contentType);
        break;
      case 'searchText':
        result = searchTextWorker(payload.text, payload.query, payload.maxResults || 100);
        break;
      default:
        throw new Error('Unknown worker message type: ' + type);
    }
    self.postMessage({ type, id, result });
  } catch (err) {
    self.postMessage({ type, id, error: err.message });
  }
};
