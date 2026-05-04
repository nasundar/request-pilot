/**
 * Desktop UI validation tests.
 *
 * Ensures the desktop app's JS and HTML are syntactically valid and
 * structurally consistent — every DOM element referenced in JS must
 * exist in the HTML.
 */

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const UI_DIR = path.resolve(__dirname, '../../desktop/ui');
const APP_JS = path.join(UI_DIR, 'app.js');
const INDEX_HTML = path.join(UI_DIR, 'index.html');

describe('desktop/ui/app.js', () => {
  let jsSource;

  beforeAll(() => {
    jsSource = fs.readFileSync(APP_JS, 'utf-8');
  });

  test('is valid JavaScript (no syntax errors)', () => {
    // vm.Script throws SyntaxError if the source is invalid
    expect(() => {
      new vm.Script(jsSource, { filename: 'app.js' });
    }).not.toThrow();
  });
});

describe('desktop/ui DOM element references', () => {
  let jsSource;
  let htmlSource;

  beforeAll(() => {
    jsSource = fs.readFileSync(APP_JS, 'utf-8');
    htmlSource = fs.readFileSync(INDEX_HTML, 'utf-8');
  });

  /**
   * Collect all IDs present in the static HTML.
   */
  function getHtmlIds() {
    const ids = new Set();
    const re = /\bid=["']([a-zA-Z0-9_-]+)["']/g;
    let m;
    while ((m = re.exec(htmlSource)) !== null) ids.add(m[1]);
    return ids;
  }

  /**
   * Collect all IDs that are dynamically created via innerHTML/template
   * strings in JS (e.g. id="telemetryToggle" inside a string literal).
   */
  function getDynamicIds() {
    const ids = new Set();
    // Match `id="foo"` or id='foo' as HTML strings or `.id = 'foo'`
    // assignments. The latter pattern is used for elements built via
    // `document.createElement(...)` then assigned an id.
    const re = /id=(?:\\?["'])([a-zA-Z0-9_-]+)(?:\\?["'])/g;
    const reAssign = /\.id\s*=\s*['"]([a-zA-Z0-9_-]+)['"]/g;
    let m;
    while ((m = re.exec(jsSource)) !== null) ids.add(m[1]);
    while ((m = reAssign.exec(jsSource)) !== null) ids.add(m[1]);
    return ids;
  }

  test('every $(\"#id\") in JS has a matching id in HTML or is dynamically created', () => {
    const idRefs = new Set();
    const re = /\$\(\s*['"]#([a-zA-Z0-9_-]+)['"]\s*\)/g;
    let m;
    while ((m = re.exec(jsSource)) !== null) idRefs.add(m[1]);

    const htmlIds = getHtmlIds();
    const dynamicIds = getDynamicIds();
    const allIds = new Set([...htmlIds, ...dynamicIds]);

    const missing = [...idRefs].filter(id => !allIds.has(id));
    if (missing.length > 0) {
      throw new Error(
        `JS references DOM elements by ID that don't exist in HTML or dynamic creation:\n` +
        missing.map(id => `  - #${id}`).join('\n')
      );
    }
    expect(missing).toEqual([]);
  });

  test('every getElementById in JS has a matching id in HTML or is dynamically created', () => {
    const idRefs = new Set();
    const re = /getElementById\(\s*['"]([a-zA-Z0-9_-]+)['"]\s*\)/g;
    let m;
    while ((m = re.exec(jsSource)) !== null) idRefs.add(m[1]);

    const htmlIds = getHtmlIds();
    const dynamicIds = getDynamicIds();
    const allIds = new Set([...htmlIds, ...dynamicIds]);

    const missing = [...idRefs].filter(id => !allIds.has(id));
    if (missing.length > 0) {
      throw new Error(
        `JS getElementById references that don't exist in HTML or dynamic creation:\n` +
        missing.map(id => `  - #${id}`).join('\n')
      );
    }
    expect(missing).toEqual([]);
  });
});
