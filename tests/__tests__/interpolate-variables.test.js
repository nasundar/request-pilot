const fs = require('fs');
const path = require('path');
const vm = require('vm');

const APP_JS = path.resolve(__dirname, '../../desktop/ui/app.js');
const jsSource = fs.readFileSync(APP_JS, 'utf-8');

function extractFunction(name) {
  const re = new RegExp(
    `(?:async\\s+)?function\\s+${name}\\s*\\([^)]*\\)\\s*\\{`,
    'm'
  );
  const match = re.exec(jsSource);
  if (!match) throw new Error(`Function "${name}" not found in app.js`);

  let start = match.index;
  let i = match.index + match[0].length - 1;
  let depth = 0;
  let quote = null;
  let escaped = false;
  let inLineComment = false;
  let inBlockComment = false;
  let inRegex = false;
  let inRegexClass = false;
  for (; i < jsSource.length; i++) {
    const ch = jsSource[i];
    const next = jsSource[i + 1];
    if (inLineComment) {
      if (ch === '\n') inLineComment = false;
      continue;
    }
    if (inBlockComment) {
      if (ch === '*' && next === '/') {
        inBlockComment = false;
        i++;
      }
      continue;
    }
    if (quote) {
      if (escaped) escaped = false;
      else if (ch === '\\') escaped = true;
      else if (ch === quote) quote = null;
      continue;
    }
    if (inRegex) {
      if (escaped) escaped = false;
      else if (ch === '\\') escaped = true;
      else if (ch === '[') inRegexClass = true;
      else if (ch === ']') inRegexClass = false;
      else if (ch === '/' && !inRegexClass) inRegex = false;
      continue;
    }
    if (ch === '/' && next === '/') {
      inLineComment = true;
      i++;
      continue;
    }
    if (ch === '/' && next === '*') {
      inBlockComment = true;
      i++;
      continue;
    }
    if (ch === '"' || ch === "'" || ch === '`') {
      quote = ch;
      continue;
    }
    if (ch === '/' && ['(', '=', ':', ',', '[', '!', '?'].includes(previousNonWhitespace(jsSource, i))) {
      inRegex = true;
      continue;
    }
    if (ch === '{') depth++;
    else if (ch === '}') {
      depth--;
      if (depth === 0) break;
    }
  }
  return jsSource.slice(start, i + 1);
}

function previousNonWhitespace(source, index) {
  for (let i = index - 1; i >= 0; i--) {
    if (!/\s/.test(source[i])) return source[i];
  }
  return '';
}

function evalFunctions(code, exportName) {
  const wrapped = `(function() { ${code}; return ${exportName}; })()`;
  const script = new vm.Script(wrapped, { filename: 'interpolate-variables-eval.js' });
  return script.runInNewContext({
    Array,
    Date,
    JSON,
    Map,
    Math,
    Number,
    Object,
    RegExp,
    Set,
    Uint8Array,
    console,
    parseFloat,
    parseInt,
    crypto: {
      randomUUID: () => '11111111-2222-4333-8444-555555555555',
      getRandomValues: (a) => {
        for (let i = 0; i < a.length; i++) a[i] = i;
        return a;
      },
    },
  });
}

function evalWithMerged(mergedFn) {
  const uuidSource = extractFunction('_rpUuidV4');
  const strictIntSource = extractFunction('_rpParseStrictInt');
  const timestampOffsetSource = extractFunction('_rpApplyTimestampOffset');
  const innerSource = extractFunction('_rpInterpolateInner');
  const interpolateSource = extractFunction('interpolateVariables');
  return evalFunctions(
    `const buildMergedVarsObject = ${mergedFn.toString()};\n${uuidSource}\n${strictIntSource}\n${timestampOffsetSource}\n${innerSource}\n${interpolateSource}`,
    'interpolateVariables'
  );
}

function loadInterpolateVariables() {
  return evalWithMerged(
    () => ({
      name: 'Alice',
    })
  );
}

describe('interpolateVariables', () => {
  let interpolateVariables;
  const uuidRe = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

  beforeAll(() => {
    interpolateVariables = loadInterpolateVariables();
  });

  function expectTimestampSecondsClose(input, expectedSeconds, tolerance = 5) {
    const value = interpolateVariables(input);
    const numeric = Number(value);
    expect(value).toMatch(/^-?\d+$/);
    expect(Math.abs(numeric - expectedSeconds)).toBeLessThanOrEqual(tolerance);
  }

  test('resolves {{$timestamp}} as Unix epoch seconds', () => {
    const value = interpolateVariables('{{$timestamp}}');
    const numeric = Number(value);
    expect(value).toMatch(/^\d+$/);
    expect(Math.abs(numeric - Math.floor(Date.now() / 1000))).toBeLessThanOrEqual(5);
    expect(value).toHaveLength(10);
  });

  test('$timestamp with zero-second offset resolves near now', () => {
    expectTimestampSecondsClose('{{$timestamp 0 s}}', Math.floor(Date.now() / 1000));
  });

  test('$timestamp with negative hour offset subtracts seconds', () => {
    expectTimestampSecondsClose('{{$timestamp -1 h}}', Math.floor(Date.now() / 1000) - 3600);
  });

  test('$timestamp with positive minute offset adds seconds', () => {
    expectTimestampSecondsClose('{{$timestamp 30 m}}', Math.floor(Date.now() / 1000) + 1800);
  });

  test('$timestamp with positive day offset adds seconds', () => {
    expectTimestampSecondsClose('{{$timestamp 2 d}}', Math.floor(Date.now() / 1000) + 172800);
  });

  test('$timestamp with positive week offset adds seconds', () => {
    expectTimestampSecondsClose('{{$timestamp 1 w}}', Math.floor(Date.now() / 1000) + 604800);
  });

  test('$timestamp with month offset uses calendar arithmetic', () => {
    const value = Number(interpolateVariables('{{$timestamp 1 M}}'));
    const diff = value - Math.floor(Date.now() / 1000);
    expect(diff).toBeGreaterThanOrEqual(28 * 86400);
    expect(diff).toBeLessThanOrEqual(31 * 86400);
  });

  test('$timestamp with year offset uses calendar arithmetic', () => {
    const value = Number(interpolateVariables('{{$timestamp -1 y}}'));
    const diff = value - Math.floor(Date.now() / 1000);
    expect(diff).toBeGreaterThanOrEqual(-366 * 86400);
    expect(diff).toBeLessThanOrEqual(-365 * 86400);
  });

  test('$timestamp with millisecond offset truncates to seconds', () => {
    expectTimestampSecondsClose('{{$timestamp 5 ms}}', Math.floor(Date.now() / 1000));
  });

  test('$timestamp preserves unknown offset units', () => {
    expect(interpolateVariables('{{$timestamp -1 X}}')).toBe('{{$timestamp -1 X}}');
  });

  test('$timestamp preserves invalid offset values', () => {
    expect(interpolateVariables('{{$timestamp abc h}}')).toBe('{{$timestamp abc h}}');
  });

  test('$timestamp preserves offset without a unit', () => {
    expect(interpolateVariables('{{$timestamp -1}}')).toBe('{{$timestamp -1}}');
  });

  test('$timestamp offset interpolates when mixed in body text', () => {
    const result = interpolateVariables('before {{$timestamp -1 h}} after');
    expect(result).toMatch(/^before -?\d+ after$/);
    const numeric = Number(result.match(/^before (-?\d+) after$/)[1]);
    expect(Math.abs(numeric - (Math.floor(Date.now() / 1000) - 3600))).toBeLessThanOrEqual(5);
  });

  test('$datetime returns RFC 3339 with explicit +00:00 offset (parity with Rust to_rfc3339)', () => {
    const out = interpolateVariables('{{$datetime}}');
    expect(out).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?\+00:00$/);
  });

  test('resolves {{$uuid}} as UUID', () => {
    expect(interpolateVariables('{{$uuid}}')).toMatch(uuidRe);
  });

  test('resolves {{$guid}} as UUID', () => {
    expect(interpolateVariables('{{$guid}}')).toMatch(uuidRe);
  });

  test('resolves {{$randomInt}} in the default range', () => {
    const value = Number(interpolateVariables('{{$randomInt}}'));
    expect(Number.isInteger(value)).toBe(true);
    expect(value).toBeGreaterThanOrEqual(0);
    expect(value).toBeLessThan(10000);
  });

  test('resolves {{$randomInt 5 10}} in the requested range', () => {
    for (let i = 0; i < 50; i++) {
      const value = Number(interpolateVariables('{{$randomInt 5 10}}'));
      expect(Number.isInteger(value)).toBe(true);
      expect(value).toBeGreaterThanOrEqual(5);
      expect(value).toBeLessThan(10);
    }
  });

  test('returns min for {{$randomInt 7 7}}', () => {
    expect(interpolateVariables('{{$randomInt 7 7}}')).toBe('7');
  });

  test('$randomInt -5 5 supports negative min', () => {
    for (let i = 0; i < 50; i++) {
      const n = parseInt(interpolateVariables('{{$randomInt -5 5}}'), 10);
      expect(n).toBeGreaterThanOrEqual(-5);
      expect(n).toBeLessThan(5);
    }
  });

  test('$randomInt rejects non-integer args (parity with Rust)', () => {
    for (let i = 0; i < 30; i++) {
      const n = parseInt(interpolateVariables('{{$randomInt 5.5 10}}'), 10);
      expect(n).toBeGreaterThanOrEqual(0);
      expect(n).toBeLessThan(10);
    }
  });

  test('$randomInt with one invalid arg defaults that one only', () => {
    for (let i = 0; i < 30; i++) {
      const n = parseInt(interpolateVariables('{{$randomInt 5 x}}'), 10);
      expect(n).toBeGreaterThanOrEqual(5);
      expect(n).toBeLessThan(10000);
    }
  });

  test('resolves user variables from merged map', () => {
    expect(interpolateVariables('{{name}}')).toBe('Alice');
  });

  test('mixes literal text, built-ins, and user variables', () => {
    const result = interpolateVariables('prefix-{{$timestamp}}-{{name}}-suffix');
    expect(result).toMatch(/^prefix-\d{10}-Alice-suffix$/);
  });

  test('preserves missing variables', () => {
    expect(interpolateVariables('{{missing}}')).toBe('{{missing}}');
  });

  test('preserves {{$processEnv FOO}} for Rust-side resolution', () => {
    expect(interpolateVariables('{{$processEnv FOO}}')).toBe('{{$processEnv FOO}}');
  });

  test('preserves {{$localHostname}} for Rust-side resolution', () => {
    expect(interpolateVariables('{{$localHostname}}')).toBe('{{$localHostname}}');
  });

  test('returns empty string, null, and undefined as-is', () => {
    expect(interpolateVariables('')).toBe('');
    expect(interpolateVariables(null)).toBeNull();
    expect(interpolateVariables(undefined)).toBeUndefined();
  });

  test('tolerates spaces inside braces for user variables', () => {
    expect(interpolateVariables('{{ name }}')).toBe('Alice');
  });

  test('does not hang on malformed input with many spaces and no closing braces', () => {
    const malformed = '{{' + ' '.repeat(10000);
    const start = Date.now();
    const out = interpolateVariables(malformed);
    const elapsed = Date.now() - start;
    expect(elapsed).toBeLessThan(1000);
    expect(out).toBe(malformed);
  });

  test('does not hang on alternating braces', () => {
    const adversarial = '{{ '.repeat(2000) + ' }}'.repeat(2000);
    const start = Date.now();
    interpolateVariables(adversarial);
    const elapsed = Date.now() - start;
    expect(elapsed).toBeLessThan(1000);
  });
});

describe('renderVariableChips', () => {
  let renderVariableChips;

  beforeAll(() => {
    const helperBody = extractFunction('escapeOverlayHtml');
    const fnBody = extractFunction('renderVariableChips');
    renderVariableChips = evalFunctions(
      `${helperBody}\nconst buildMergedVarsObject = () => ({ name: 'value' });\n${fnBody}`,
      'renderVariableChips'
    );
  });

  test('escapes < and > in surrounding text', () => {
    const { html } = renderVariableChips('<script>alert(1)</script>{{name}}');
    expect(html).not.toContain('<script>');
    expect(html).toContain('&lt;script&gt;');
  });

  test('escapes the chip text itself when token contains HTML special chars', () => {
    const { html } = renderVariableChips('{{a"b}}');
    expect(html).not.toContain('a"b');
    expect(html).toContain('&quot;');
  });
});

describe('interpolateVariables transitive resolution', () => {
  test('expands user variable containing {{$timestamp}}', () => {
    const interpolateVariables = evalWithMerged(() => ({ a: '{{$timestamp}}' }));
    const value = interpolateVariables('{{a}}');
    const numeric = Number(value);
    expect(value).toMatch(/^\d+$/);
    expect(Math.abs(numeric - Math.floor(Date.now() / 1000))).toBeLessThanOrEqual(5);
  });

  test('expands timestamp range variables transitively', () => {
    const interpolateVariables = evalWithMerged(() => ({
      x: '{{$timestamp -2 h}}',
      y: '{{$timestamp}}',
    }));
    const result = interpolateVariables('start={{x}}&end={{y}}');
    const match = result.match(/^start=(\d+)&end=(\d+)$/);
    expect(match).not.toBeNull();
    const start = Number(match[1]);
    const end = Number(match[2]);
    expect(end - start).toBeGreaterThanOrEqual(7200);
    expect(end - start).toBeLessThanOrEqual(7205);
  });

  test('expands chained user variables', () => {
    const interpolateVariables = evalWithMerged(() => ({
      a: '{{b}}',
      b: '{{c}}',
      c: 'deep',
    }));
    expect(interpolateVariables('{{a}}')).toBe('deep');
  });

  test('terminates on cycles and leaves a visible raw placeholder', () => {
    const interpolateVariables = evalWithMerged(() => ({
      a: '{{b}}',
      b: '{{a}}',
    }));
    const result = interpolateVariables('{{a}}');
    expect(result).toMatch(/\{\{[ab]\}\}/);
  });

  test('terminates on self-reference', () => {
    const interpolateVariables = evalWithMerged(() => ({ a: '{{a}}' }));
    expect(interpolateVariables('{{a}}')).toBe('{{a}}');
  });

  test('expands repeated top-level references independently', () => {
    const interpolateVariables = evalWithMerged(() => ({ a: '{{$timestamp}}' }));
    const parts = interpolateVariables('{{a}} {{a}}').split(' ');
    expect(parts).toHaveLength(2);
    expect(parts[0]).toMatch(/^\d+$/);
    expect(parts[1]).toMatch(/^\d+$/);
  });
});
