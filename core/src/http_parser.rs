use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ParsedRequest {
    pub name: Option<String>,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TestSuite {
    pub variables: Vec<(String, String)>,
    pub blocks: Vec<TestBlock>,
    /// File-level auto-run interval (e.g. "15m", "1h"). Parsed from `# @auto_run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_run: Option<String>,
    /// Variables marked with `@prompt VAR description` — the runtime UI asks
    /// the user to supply values for each before executing the suite. Values
    /// supplied by the user take precedence over any existing `@variables`
    /// entry of the same name. Order of declaration is preserved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<PromptVariable>,
    /// File-level `# @@request-id [header-name]` directive. When set, every
    /// block inherits an auto-injected request-id header (a fresh UUIDv4 per
    /// request, unless the user already sets that header manually). Blocks
    /// can override or disable via their own `# @@request-id` directive.
    /// Default header name when the directive has no argument: `X-Request-Id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id_header: Option<String>,
    /// File-level body redaction rules from `# @@redact body ...` directives.
    /// Applied to request and response bodies before recording. Block-level
    /// rules are appended to these at recording time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact_body_rules: Vec<BodyRedactRule>,
}

/// A single body-redaction rule parsed from a `# @@redact body ...` directive.
/// `JsonPath` matches a JSONPath expression (e.g. `$.password`); `Regex` matches
/// a regular expression delimited by slashes in the source (e.g. `/Bearer\s+\S+/`).
/// Application of these rules is handled at recording time, not in the parser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BodyRedactRule {
    JsonPath(String),
    Regex(String),
}

/// A `@prompt` runtime-input variable declared at the top of the file.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PromptVariable {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TestBlock {
    pub block_type: String,
    pub name: String,
    pub description: String,
    pub disabled: bool,
    pub mode: Option<String>,
    pub dev_auth: Option<String>,
    pub group: Option<String>,
    pub depends: Vec<String>,
    pub request: ParsedRequest,
    pub assertions: Vec<Assertion>,
    pub extracts: Vec<Extract>,
    /// True when block has `# @compare` directive — contains multi-step comparison.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compare: bool,
    /// Steps within a @compare block. Empty for normal blocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<CompareStep>,
    /// Diff directive specifying which two steps to compare. Kept as a
    /// transitional single-pair field so `.http` files and history
    /// records produced before multi-diff support keep deserializing.
    /// New code reads `effective_diffs()` (which falls back to wrapping
    /// `diff` in a single-element vec when `diffs` is empty) instead of
    /// reading either field directly. The parser populates both fields:
    /// `diffs[0]` is also mirrored into `diff` so old consumers (e.g.
    /// the history viewer running an older build) still see something
    /// meaningful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffDirective>,
    /// Multi-pair diff directives. A block may declare any number of
    /// `# @@diff <a> <b>` lines; each becomes one entry here in source
    /// order. Empty for blocks without a diff directive. The first pair
    /// is treated as "primary" — `$diff.*` assertions are evaluated
    /// against it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<DiffDirective>,
    /// Validation errors detected during parsing (duplicate step names, invalid diff refs, etc.)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: ParseErrors,
    /// Block-level override for the file-level `# @@request-id` directive.
    /// `Some(name)` uses `name` as the injected header name for this block.
    /// Combined with `request_id_disabled` below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id_header: Option<String>,
    /// When true, this block opts out of auto request-id injection even if a
    /// file-level directive is active. Set via `# @@request-id off`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub request_id_disabled: bool,
    /// Block-level body redaction rules from `# @@redact body ...` directives.
    /// Appended to the file-level `TestSuite.redact_body_rules` at recording
    /// time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact_body_rules: Vec<BodyRedactRule>,
    /// Optional `# @@for <iter_var> in <source_var>` directive turning this
    /// block into a repeater. When `Some`, the runner executes the block
    /// once per element of the JSON array in `source_var`, binding
    /// `iter_var` (plus `$index` / `$iteration`) into a child var-store.
    /// V1 supports scalar AND object elements (object access via dotted
    /// `{{iter_var.field}}` interpolation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_loop: Option<ForLoop>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Assertion {
    pub left: String,
    pub operator: String,
    pub right: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Extract {
    pub variable_name: String,
    pub source_path: String,
}

/// A single step within a @compare block.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CompareStep {
    pub name: String,
    pub request: ParsedRequest,
    pub assertions: Vec<Assertion>,
    pub extracts: Vec<Extract>,
}

/// Specifies which two steps to diff in a @compare block.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiffDirective {
    pub step_a: String,
    pub step_b: String,
}

/// `# @@for <iter_var> in <source_var>` directive — turns a normal block
/// into a repeater. The runner reads `source_var` from the var-store at
/// execution time, parses it as a JSON array, and runs the block once per
/// element with `iter_var` bound to the element's JSON-stringified form.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ForLoop {
    pub iter_var: String,
    pub source_var: String,
    /// Optional `# @@parallel <N>` directive — bounded concurrency for
    /// iteration execution. `None` (or `Some(1)`) runs iterations
    /// sequentially. `Some(N)` runs up to N iterations concurrently using
    /// a worker-pool pattern. Clamped to `[1, 32]` by the parser.
    /// V1.2: parser-validated; runner-supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<u32>,
}

impl TestBlock {
    /// Return the effective set of diff directives for this block.
    /// Prefers the multi-pair `diffs` vec; falls back to wrapping the
    /// transitional single `diff` field when `diffs` is empty (old
    /// serialized blocks). Callers should use this rather than reading
    /// either backing field directly so they see a consistent shape
    /// regardless of which writer produced the data.
    pub fn effective_diffs(&self) -> Vec<&DiffDirective> {
        if !self.diffs.is_empty() {
            self.diffs.iter().collect()
        } else if let Some(d) = self.diff.as_ref() {
            vec![d]
        } else {
            Vec::new()
        }
    }
}

/// Validation errors detected during parsing.
/// Stored on `TestBlock` so the runner can report them.
pub type ParseErrors = Vec<String>;

/// Backward-compatible parse function. Returns a flat list of requests.
pub fn parse(content: &str) -> Vec<ParsedRequest> {
    let suite = parse_test_suite(content);
    suite
        .blocks
        .into_iter()
        .map(|b| {
            let mut req = b.request;
            if req.name.is_none() && !b.name.is_empty() {
                req.name = Some(b.name);
            }
            req
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// Syntax helpers
//
// Pilot's `.http` syntax is a strict superset of VS Code REST Client's:
//
//   * Variable definitions  : `@name = value`             (REST Client native)
//   * Variable references   : `{{name}}` and `{{$builtin}}` (REST Client native)
//   * Request separators    : `###` or `---`              (REST Client native)
//   * Comments              : lines starting with `#` or `//`
//   * Bare directives       : `@name foo`, `@description foo`,
//                             `@note foo`, `@prompt VAR description`
//                             (REST Client native — disambiguated from
//                              variable defs by absence of `=`)
//   * Pilot extensions      : `# @@assert ...`, `# @@extract ...`,
//                             `# @@group ...`, `# @@depends ...`,
//                             `# @@disabled`, `# @@mode app|dev`,
//                             `# @@dev_auth <scope>`, `# @@auto_run <duration>`,
//                             `# @@compare`, `# @@step <name>`,
//                             `# @@diff <a> <b>`, `### @@setup|@@test|@@teardown Title`
//   * Legacy (still accepted): `# @assert ...`, `### @setup Title`,
//                              `@variables` block with bare `name = value`
//
// All directive matching goes through these helpers so legacy and new forms
// are equivalent at the AST level.
// ─────────────────────────────────────────────────────────────────────

/// Strip a leading line comment marker (`#` or `//`) and surrounding whitespace,
/// returning the inner content. Returns `None` if `line` is not a comment line.
fn strip_comment_prefix(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("//") {
        Some(rest.trim_start())
    } else {
        t.strip_prefix('#').map(|r| r.trim_start())
    }
}

/// Match a Pilot directive on a comment line (e.g. `# @@assert ...` or the
/// legacy `# @assert ...`). Accepts both `#` and `//` comment markers and both
/// `@@name` (new) and `@name` (legacy) prefixes.
///
/// Returns the trimmed value following the directive keyword (or `Some("")`
/// for valueless directives like `@@disabled`). Returns `None` if the line
/// is not a directive matching `name`.
fn match_directive<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let after_comment = strip_comment_prefix(line)?;
    // Accept @@name (new) or @name (legacy)
    let after_at = after_comment
        .strip_prefix("@@")
        .or_else(|| after_comment.strip_prefix('@'))?;
    let rest = after_at.strip_prefix(name)?;
    if rest.is_empty() {
        Some("")
    } else if rest.starts_with(|c: char| c.is_whitespace()) {
        Some(rest.trim())
    } else {
        // e.g. "extract" should not match "ext"
        None
    }
}

/// Match a bare REST Client directive like `@name foo` or `@description foo`
/// (no leading comment marker, no `=` sign). Returns the trimmed value, or
/// `None` if the line is not a bare directive matching `name`.
///
/// IMPORTANT: a line of form `@x = value` is a variable assignment, NOT a
/// directive — disambiguated by the presence of `=`.
fn match_bare_directive<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    let after_at = trimmed.strip_prefix('@')?;
    // `@@foo` is a Pilot directive form, not a bare REST Client directive
    if after_at.starts_with('@') {
        return None;
    }
    let rest = after_at.strip_prefix(name)?;
    if rest.is_empty() {
        Some("")
    } else if rest.starts_with(|c: char| c.is_whitespace()) {
        let value = rest.trim();
        // Reject if it's actually a var def like `@name = value`
        if value.starts_with('=') {
            return None;
        }
        Some(value)
    } else {
        None
    }
}

/// Try to parse a line as a REST Client–style top-level variable definition:
/// `@name = value`. Returns `Some((name, value))` on match.
///
/// The variable name must consist of `[A-Za-z0-9_.-]+`. Lines starting with
/// `@@` are Pilot directives, not variable defs.
fn parse_var_def(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let after_at = trimmed.strip_prefix('@')?;
    if after_at.starts_with('@') {
        return None;
    }
    let (name, value) = after_at.split_once('=')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

/// Returns true if the line is a request separator (`###` or `---`) at the
/// start of the line. The separator must be `###`/`---` followed by either
/// end-of-line or whitespace — `###Subheading` (no space) is body text, not
/// a separator.
fn is_separator_line(line: &str) -> bool {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("###") {
        return rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace());
    }
    if let Some(rest) = t.strip_prefix("---") {
        return rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace());
    }
    false
}

/// Strip the leading separator marker (`###` or `---`) from a line, returning
/// the remaining text (which may include the block-type directive and title).
fn strip_separator_prefix(line: &str) -> &str {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("###") {
        rest
    } else if let Some(rest) = t.strip_prefix("---") {
        rest
    } else {
        line
    }
}

/// Split a `.http` file into raw blocks using line-anchored separators.
/// Unlike `content.split("###")`, this only splits on lines whose trimmed
/// content starts with `###` (or equals `---`), so separators inside comments
/// or bodies are not treated as block boundaries.
///
/// Each returned string includes the post-separator content of the FIRST line
/// (so `### @@setup Title` becomes ` @@setup Title\n...`), matching the
/// existing parser's expectation.
fn split_into_raw_blocks(content: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in content.split('\n') {
        if is_separator_line(line) {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            current.push_str(strip_separator_prefix(line));
            current.push('\n');
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// Parse enhanced .http content into a TestSuite.
pub fn parse_test_suite(content: &str) -> TestSuite {
    let mut variables = Vec::new();
    let mut blocks = Vec::new();
    let mut auto_run: Option<String> = None;
    let mut request_id_header: Option<String> = None;
    let mut prompts: Vec<PromptVariable> = Vec::new();
    let mut redact_body_rules: Vec<BodyRedactRule> = Vec::new();
    let raw_blocks = split_into_raw_blocks(content);

    for raw_block in raw_blocks.iter() {
        let block = raw_block.trim();
        if block.is_empty() {
            continue;
        }

        // Legacy `@variables` block: a block whose first non-empty,
        // non-comment line is `@variables` (with no `=`). Inside the block,
        // `name = value` lines (with or without leading `@`) define variables.
        let has_legacy_variables_block = block.lines().any(|l| {
            let t = l.trim();
            t == "@variables" || t.starts_with("@variables ")
        });

        if has_legacy_variables_block {
            // Scan for file-level directives (e.g. `# @@auto_run`) within this block
            for line in block.lines() {
                pick_file_directives(line, &mut auto_run, &mut request_id_header, &mut redact_body_rules, true);
                pick_prompt_directive(line, &mut prompts);
            }
            parse_variables_block(block, &mut variables);
            continue;
        }

        // A "header-only" block has no request line — only comments,
        // top-level variable definitions, REST Client directives like
        // `@name foo`, and file-level directives like `# @@auto_run`.
        // We treat the block as header-only and harvest variables/directives
        // without producing a TestBlock.
        let is_true = blocks.is_empty()
            && block.lines().all(|l| {
                let t = l.trim();
                t.is_empty()
                    || strip_comment_prefix(t).is_some()
                    || parse_var_def(t).is_some()
                    || match_bare_directive(t, "name").is_some()
                    || match_bare_directive(t, "description").is_some()
                    || match_bare_directive(t, "note").is_some()
                    || match_bare_directive(t, "prompt").is_some()
            });

        if is_true {
            for line in block.lines() {
                pick_file_directives(line, &mut auto_run, &mut request_id_header, &mut redact_body_rules, true);
                pick_prompt_directive(line, &mut prompts);
                if let Some((name, value)) = parse_var_def(line) {
                    variables.push((name, value));
                }
            }
            continue;
        }

        // Real test block. Peel off any leading top-level `@var = value`
        // lines (REST Client style) before handing off to parse_test_block.
        let (peeled_vars, remainder) = peel_leading_var_defs(block);
        for (name, value) in peeled_vars {
            variables.push((name, value));
        }

        // Also scan for file-level directives anywhere in the block
        // (e.g. `# @@auto_run` placed in a header comment block alongside a
        // request — uncommon, but supported). Pass `false` so block-scoped
        // directives (like `# @@request-id`) don't leak to file level.
        for line in remainder.lines() {
            pick_file_directives(line, &mut auto_run, &mut request_id_header, &mut redact_body_rules, false);
            pick_prompt_directive(line, &mut prompts);
        }

        if let Some(test_block) = parse_test_block(&remainder) {
            blocks.push(test_block);
        }
    }

    TestSuite {
        variables,
        blocks,
        auto_run,
        prompts,
        request_id_header,
        redact_body_rules,
    }
}

/// Peel REST Client–style `@var = value` lines from the top of a block,
/// returning the harvested vars and the remainder of the block (with those
/// leading lines removed). Comment lines and blank lines are preserved in
/// the remainder.
fn peel_leading_var_defs(block: &str) -> (Vec<(String, String)>, String) {
    let mut vars = Vec::new();
    let mut remainder = String::new();
    let mut still_peeling = true;

    for line in block.lines() {
        if still_peeling {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                // Blank lines are kept in the remainder so request body
                // boundaries are preserved.
                remainder.push_str(line);
                remainder.push('\n');
                continue;
            }
            if let Some((name, value)) = parse_var_def(line) {
                vars.push((name, value));
                continue;
            }
            still_peeling = false;
        }
        remainder.push_str(line);
        remainder.push('\n');
    }

    (vars, remainder)
}

/// Default header name used when `# @@request-id` has no argument.
pub const DEFAULT_REQUEST_ID_HEADER: &str = "X-Request-Id";

/// Classify the value of a `# @@request-id` directive. Returns `Some(Disabled)`
/// for off/none/false, `Some(Header(name))` for a header name (defaults to
/// [`DEFAULT_REQUEST_ID_HEADER`] when the value is empty), or `None` if the
/// value is something invalid we should skip silently.
pub enum RequestIdDirective {
    Header(String),
    Disabled,
}

pub fn parse_request_id_value(raw: &str) -> RequestIdDirective {
    let t = raw.trim();
    if t.is_empty() {
        return RequestIdDirective::Header(DEFAULT_REQUEST_ID_HEADER.to_string());
    }
    let low = t.to_ascii_lowercase();
    if matches!(low.as_str(), "off" | "none" | "false" | "no" | "disabled") {
        return RequestIdDirective::Disabled;
    }
    // Take first whitespace-separated token as header name; ignore trailing.
    let header = t.split_whitespace().next().unwrap_or(DEFAULT_REQUEST_ID_HEADER);
    RequestIdDirective::Header(header.to_string())
}

/// Resolve the effective request-id header name for `block` given `suite`'s
/// file-level default. Returns `None` when auto-injection should be skipped.
pub fn effective_request_id_header(suite: &TestSuite, block: &TestBlock) -> Option<String> {
    if block.request_id_disabled {
        return None;
    }
    if let Some(h) = &block.request_id_header {
        return Some(h.clone());
    }
    suite.request_id_header.clone()
}

/// Parse the value following `@@redact body` into a list of redaction rules.
///
/// Accepts two forms:
///   * `body $.path1,$.path2` — comma-separated JSONPath expressions
///   * `body /regex-pattern/` — a single slash-delimited regex
///
/// Returns an empty `Vec` if the value does not start with the `body` keyword
/// or yields no rules. The leading `body` keyword is required (a future
/// extension may add other targets such as `headers`).
pub fn parse_redact_body_directive(value: &str) -> Vec<BodyRedactRule> {
    let v = value.trim();
    let rest = match v.strip_prefix("body") {
        Some(r) if r.is_empty() => "",
        Some(r) if r.starts_with(|c: char| c.is_whitespace()) => r.trim(),
        _ => return Vec::new(),
    };
    if rest.is_empty() {
        return Vec::new();
    }
    if let Some(inner) = rest.strip_prefix('/') {
        if let Some(pat) = inner.strip_suffix('/') {
            if !pat.is_empty() {
                return vec![BodyRedactRule::Regex(pat.to_string())];
            }
            return Vec::new();
        }
    }
    rest.split(',')
        .filter_map(parse_body_redact_pattern)
        .collect()
}

/// Parse a single body-redaction pattern string (one entry from a comma-
/// separated directive list, or one entry from the global
/// `SessionsConfig.body_redaction_paths` config). Recognized forms:
///   * `$.json.path`  → `BodyRedactRule::JsonPath`
///   * `/regex/`      → `BodyRedactRule::Regex`
///
/// Whitespace around the pattern is trimmed. Empty / unrecognized inputs
/// return `None`.
pub fn parse_body_redact_pattern(s: &str) -> Option<BodyRedactRule> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(inner) = t.strip_prefix('/') {
        if let Some(pat) = inner.strip_suffix('/') {
            if pat.is_empty() {
                return None;
            }
            return Some(BodyRedactRule::Regex(pat.to_string()));
        }
        return None;
    }
    Some(BodyRedactRule::JsonPath(t.to_string()))
}

/// Recognize file-level directives like `# @@auto_run 15m` (also legacy
/// `# @auto_run`). Updates the provided slots.
///
/// `in_true` should be `true` when scanning a header-only block (pure
/// comments / variable defs / REST Client bare directives at the top of the
/// file). When `false`, we're scanning inside/alongside a real request block
/// and we do NOT pick up directives that have block-scoped semantics
/// (e.g. `request-id`) — those are handled by `parse_test_block`.
fn pick_file_directives(
    line: &str,
    auto_run: &mut Option<String>,
    request_id_header: &mut Option<String>,
    redact_body_rules: &mut Vec<BodyRedactRule>,
    in_true: bool,
) {
    if let Some(rest) = match_directive(line, "auto_run") {
        let interval = rest.trim();
        if crate::duration::parse_duration_secs(interval).is_some() {
            *auto_run = Some(interval.to_string());
        }
    }
    if in_true {
        if let Some(rest) = match_directive(line, "redact") {
            redact_body_rules.extend(parse_redact_body_directive(rest));
        }
        if let Some(rest) = match_directive(line, "request-id")
            .or_else(|| match_directive(line, "request_id"))
        {
            match parse_request_id_value(rest) {
                RequestIdDirective::Header(name) => *request_id_header = Some(name),
                // File-level "off" means "do not enable". Leave as None.
                RequestIdDirective::Disabled => *request_id_header = None,
            }
        }
    }
}

/// Recognize a `@prompt VAR description` REST Client–style directive and
/// append it to `prompts`. Duplicates (same name) are de-duplicated; the
/// first-seen description wins. Skips blank var names.
fn pick_prompt_directive(line: &str, prompts: &mut Vec<PromptVariable>) {
    let Some(rest) = match_bare_directive(line, "prompt") else {
        return;
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = match parts.next() {
        Some(n) => n.trim().to_string(),
        None => return,
    };
    if name.is_empty() {
        return;
    }
    if prompts.iter().any(|p| p.name == name) {
        return;
    }
    let description = parts.next().unwrap_or("").trim().to_string();
    prompts.push(PromptVariable { name, description });
}

/// Returns the 0-based starting line number of each runnable block in the same
/// order and count as `parse_test_suite(content).blocks`. Blocks that the parser
/// skips (@variables block, file-level comment-only headers) are excluded, so
/// the returned indices map 1:1 to `TestSuite.blocks`.
///
/// A block "starts" on the line where its `###`/`---` separator sits (or line 0
/// if the file begins with a block without a leading separator).
pub fn block_start_lines(content: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut seen_any_runnable = false;

    let mut current_start: usize = 0;
    let mut current_lines: Vec<&str> = Vec::new();
    let lines: Vec<&str> = content.split('\n').collect();

    let flush = |current_start: usize,
                 current_lines: &[&str],
                 starts: &mut Vec<usize>,
                 seen_any_runnable: &mut bool| {
        let block_text: String = current_lines.join("\n");
        let block = block_text.trim();
        if block.is_empty() {
            return;
        }
        let has_variables = block.lines().any(|l| {
            let t = l.trim();
            t == "@variables" || t.starts_with("@variables ")
        });
        if has_variables {
            return;
        }
        // Header-only block: comments + bare directives + var defs only
        let is_header_block = !*seen_any_runnable
            && block.lines().all(|l| {
                let t = l.trim();
                t.is_empty()
                    || strip_comment_prefix(t).is_some()
                    || parse_var_def(t).is_some()
                    || match_bare_directive(t, "name").is_some()
                    || match_bare_directive(t, "description").is_some()
                    || match_bare_directive(t, "note").is_some()
                    || match_bare_directive(t, "prompt").is_some()
            });
        if is_header_block {
            return;
        }
        // Only record if parse_test_block would actually produce a block.
        let (_, remainder) = peel_leading_var_defs(block);
        if parse_test_block(&remainder).is_some() {
            starts.push(current_start);
            *seen_any_runnable = true;
        }
    };

    for (idx, line) in lines.iter().enumerate() {
        if is_separator_line(line) {
            flush(
                current_start,
                &current_lines,
                &mut starts,
                &mut seen_any_runnable,
            );
            current_lines.clear();
            current_start = idx;
            // Include everything after the separator on this line as the
            // first line of the new block.
            current_lines.push(strip_separator_prefix(line));
        } else {
            current_lines.push(line);
        }
    }
    flush(
        current_start,
        &current_lines,
        &mut starts,
        &mut seen_any_runnable,
    );

    starts
}

fn parse_variables_block(block: &str, variables: &mut Vec<(String, String)>) {
    let mut past_header = false;
    for line in block.lines() {
        let trimmed = line.trim();
        if !past_header {
            if trimmed == "@variables" || trimmed.starts_with("@variables ") {
                past_header = true;
            }
            continue;
        }
        if trimmed.is_empty() || strip_comment_prefix(trimmed).is_some() {
            continue;
        }
        // Support both "@name = value" and "name = value" formats. Also accept
        // arbitrary whitespace; do NOT match `@@name = value` (Pilot directive).
        let var_line = if let Some(rest) = trimmed.strip_prefix("@@") {
            // `@@x = ...` is not a variable
            let _ = rest;
            continue;
        } else if let Some(rest) = trimmed.strip_prefix('@') {
            rest
        } else {
            trimmed
        };
        if let Some((name, value)) = var_line.split_once('=') {
            let name = name.trim();
            let value = value.trim();
            if !name.is_empty() {
                variables.push((name.to_string(), value.to_string()));
            }
        }
    }
}

fn parse_test_block(block: &str) -> Option<TestBlock> {
    let mut block_type = "request".to_string();
    let mut block_name = String::new();
    let mut description = String::new();
    let mut disabled = false;
    let mut mode: Option<String> = None;
    let mut dev_auth: Option<String> = None;
    let mut group: Option<String> = None;
    let mut depends = Vec::new();
    let mut block_request_id_header: Option<String> = None;
    let mut block_request_id_disabled = false;
    let mut block_redact_body_rules: Vec<BodyRedactRule> = Vec::new();
    let mut is_compare = false;
    let mut diff_directives: Vec<DiffDirective> = Vec::new();
    let mut for_loop: Option<ForLoop> = None;
    // V1.2: `# @@parallel <N>` is order-insensitive relative to `# @@for`.
    // We collect it during the per-line directive sweep and only attach it
    // to `for_loop.parallel` after the whole block has been parsed (and we
    // know whether `for_loop` survived final validation).
    let mut parallel_n: Option<u32> = None;
    // Block-level assertions (used for $diff.* in compare blocks, or normal assertions)
    let mut assertions = Vec::new();
    let mut extracts = Vec::new();
    let mut request_name: Option<String> = None;
    let mut request_lines: Vec<&str> = Vec::new();
    let mut first_meaningful = true;

    // Compare step tracking
    let mut steps: Vec<CompareStep> = Vec::new();
    let mut current_step_name: Option<String> = None;
    let mut step_assertions: Vec<Assertion> = Vec::new();
    let mut seen_step_names: HashSet<String> = HashSet::new();
    let mut errors: ParseErrors = Vec::new();
    let mut step_extracts: Vec<Extract> = Vec::new();
    let mut step_request_lines: Vec<&str> = Vec::new();
    let mut step_request_name: Option<String> = None;

    for line in block.lines() {
        let trimmed = line.trim();

        // First non-empty line: check for block type annotation.
        // Accept both `@@setup`/`@@test`/`@@teardown` (new) and `@setup`/etc.
        // (legacy). The line may have a title after the type keyword.
        if first_meaningful && !trimmed.is_empty() {
            first_meaningful = false;

            // Strip leading `@@` (new) or `@` (legacy) before matching keyword.
            let after_at = trimmed
                .strip_prefix("@@")
                .or_else(|| trimmed.strip_prefix('@'));
            if let Some(rest) = after_at {
                let (kw, title) = match rest.find(|c: char| c.is_whitespace()) {
                    Some(idx) => (&rest[..idx], rest[idx..].trim()),
                    None => (rest, ""),
                };
                let matched = match kw {
                    "setup" => {
                        block_type = "setup".to_string();
                        true
                    }
                    "test" => {
                        block_type = "test".to_string();
                        true
                    }
                    "teardown" => {
                        block_type = "teardown".to_string();
                        true
                    }
                    _ => false,
                };
                if matched {
                    block_name = title.to_string();
                    continue;
                }
            }
        }

        // ── Pilot directives (accept both `# @@x` and legacy `# @x`) ──
        if let Some(rest) = match_directive(line, "description") {
            description = rest.to_string();
            continue;
        }
        if let Some(rest) = match_directive(line, "disabled") {
            // `@@disabled` is valueless — accept any (or empty) value
            let _ = rest;
            disabled = true;
            continue;
        }
        if let Some(rest) = match_directive(line, "compare") {
            let _ = rest;
            is_compare = true;
            continue;
        }
        if let Some(rest) = match_directive(line, "mode") {
            let m = rest.to_lowercase();
            if m == "app" || m == "dev" {
                mode = Some(m);
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "dev_auth") {
            let scope = rest.to_string();
            if !scope.is_empty() {
                dev_auth = Some(scope);
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "group") {
            let g = rest.to_string();
            if !g.is_empty() {
                group = Some(g);
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "depends") {
            let dep = rest.to_string();
            if !dep.is_empty() {
                depends.push(dep);
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "request-id")
            .or_else(|| match_directive(line, "request_id"))
        {
            match parse_request_id_value(rest) {
                RequestIdDirective::Header(name) => {
                    block_request_id_header = Some(name);
                    block_request_id_disabled = false;
                }
                RequestIdDirective::Disabled => {
                    block_request_id_header = None;
                    block_request_id_disabled = true;
                }
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "redact") {
            block_redact_body_rules.extend(parse_redact_body_directive(rest));
            continue;
        }
        if let Some(rest) = match_directive(line, "diff") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                // Multiple `# @@diff a b` lines accumulate; each becomes
                // a separate pair so a block can render v1↔v2 and v1↔v3
                // side-by-side. Pre-multi-diff `.http` files only had
                // one such line, so this is a strict superset.
                diff_directives.push(DiffDirective {
                    step_a: parts[0].to_string(),
                    step_b: parts[1].to_string(),
                });
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "for") {
            // `# @@for <iter_var> in <source_var>`
            // Tolerant of extra whitespace; rejects malformed lines silently
            // (parser-level non-fatal — block runs as non-loop).
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if toks.len() == 3 && toks[1].eq_ignore_ascii_case("in") {
                let iter_var = toks[0].to_string();
                let source_var = toks[2].to_string();
                if !iter_var.is_empty() && !source_var.is_empty() {
                    if for_loop.is_some() {
                        errors.push(format!(
                            "duplicate `# @@for` directive — keeping the first"
                        ));
                    } else {
                        for_loop = Some(ForLoop { iter_var, source_var, parallel: None });
                    }
                }
            } else {
                errors.push(format!(
                    "invalid `# @@for` directive: expected `<iter_var> in <source_var>`, got `{}`",
                    rest
                ));
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "parallel") {
            // `# @@parallel <N>` — bounded concurrency for `# @@for` blocks.
            // Order-insensitive relative to `# @@for` (collected here, attached
            // to ForLoop.parallel after the block finishes parsing). Bare
            // directive defaults to 4. Clamped to [1, 32]. Invalid values are
            // dropped with a warning so the rest of the block still parses.
            const PARALLEL_DEFAULT: u32 = 4;
            const PARALLEL_MAX: u32 = 32;
            let trimmed = rest.trim();
            if parallel_n.is_some() {
                errors.push(format!(
                    "duplicate `# @@parallel` directive — keeping the first"
                ));
                continue;
            }
            if trimmed.is_empty() {
                parallel_n = Some(PARALLEL_DEFAULT);
            } else {
                match trimmed.parse::<i64>() {
                    Ok(n) if n >= 1 && n <= PARALLEL_MAX as i64 => {
                        parallel_n = Some(n as u32);
                    }
                    Ok(n) if n > PARALLEL_MAX as i64 => {
                        errors.push(format!(
                            "`# @@parallel {}` exceeds maximum {} — clamped to {}",
                            n, PARALLEL_MAX, PARALLEL_MAX
                        ));
                        parallel_n = Some(PARALLEL_MAX);
                    }
                    Ok(n) if n <= 0 => {
                        errors.push(format!(
                            "`# @@parallel {}` must be >= 1 — using 1 (sequential)",
                            n
                        ));
                        parallel_n = Some(1);
                    }
                    Ok(_) => unreachable!(),
                    Err(_) => {
                        errors.push(format!(
                            "invalid `# @@parallel` value `{}` — directive dropped",
                            trimmed
                        ));
                    }
                }
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "step") {
            let step_name = rest.to_string();
            if !step_name.is_empty() {
                if !seen_step_names.insert(step_name.clone()) {
                    errors.push(format!("duplicate step name: '{}'", step_name));
                }
                // Finalize previous step if one was open
                if let Some(prev_name) = current_step_name.take() {
                    if let Some(req) = parse_request_from_lines(&step_request_lines, &step_request_name) {
                        steps.push(CompareStep {
                            name: prev_name,
                            request: req,
                            assertions: std::mem::take(&mut step_assertions),
                            extracts: std::mem::take(&mut step_extracts),
                        });
                    }
                    step_request_lines.clear();
                    step_request_name = None;
                }
                current_step_name = Some(step_name);
                is_compare = true; // auto-enable compare when steps are present
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "assert") {
            if let Some(assertion) = parse_assertion_directive(rest) {
                if current_step_name.is_some() && !assertion.left.starts_with("$diff.") {
                    step_assertions.push(assertion);
                } else {
                    assertions.push(assertion);
                }
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "extract") {
            if let Some(extract) = parse_extract_directive(rest) {
                if current_step_name.is_some() {
                    step_extracts.push(extract);
                } else {
                    extracts.push(extract);
                }
            }
            continue;
        }
        if let Some(rest) = match_directive(line, "name") {
            if current_step_name.is_some() {
                step_request_name = Some(rest.to_string());
            } else {
                request_name = Some(rest.to_string());
            }
            continue;
        }

        // ── REST Client–style bare directives ──
        // `@name foo`, `@description foo`, `@note foo`, `@prompt VAR description`
        if let Some(rest) = match_bare_directive(line, "name") {
            if !rest.is_empty() {
                if current_step_name.is_some() {
                    step_request_name = Some(rest.to_string());
                } else {
                    request_name = Some(rest.to_string());
                }
            }
            continue;
        }
        if let Some(rest) = match_bare_directive(line, "description") {
            if !rest.is_empty() && description.is_empty() {
                description = rest.to_string();
            }
            continue;
        }
        if let Some(rest) = match_bare_directive(line, "note") {
            if !rest.is_empty() && description.is_empty() {
                description = rest.to_string();
            }
            continue;
        }
        if let Some(_rest) = match_bare_directive(line, "prompt") {
            // `@prompt` is parsed and accepted (REST Client compat).
            // Runtime UI handling lives in the desktop/TUI layer (Phase B).
            // For now we silently consume it so it does not appear in body.
            continue;
        }

        // Request/body lines
        if current_step_name.is_some() {
            step_request_lines.push(line);
        } else {
            request_lines.push(line);
        }
    }

    // Finalize last open step
    if let Some(prev_name) = current_step_name.take() {
        if let Some(req) = parse_request_from_lines(&step_request_lines, &step_request_name) {
            steps.push(CompareStep {
                name: prev_name,
                request: req,
                assertions: step_assertions,
                extracts: step_extracts,
            });
        }
    }

    // Validate @diff references against actual step names. Each pair
    // is checked independently so multi-diff blocks surface bad
    // references one-pair-at-a-time rather than failing globally.
    for diff in diff_directives.iter() {
        if !seen_step_names.contains(&diff.step_a) {
            errors.push(format!(
                "@diff references unknown step '{}'",
                diff.step_a
            ));
        }
        if !seen_step_names.contains(&diff.step_b) {
            errors.push(format!(
                "@diff references unknown step '{}'",
                diff.step_b
            ));
        }
    }

    // For compare blocks with steps, we don't require a block-level request
    let request = if is_compare && !steps.is_empty() {
        // Use dummy request for compare blocks — steps hold the real requests
        if let Some(req) = parse_request_from_lines(&request_lines, &request_name) {
            req
        } else {
            ParsedRequest {
                name: request_name,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            }
        }
    } else {
        parse_request_from_lines(&request_lines, &request_name)?
    };

    if block_name.is_empty() {
        if let Some(ref n) = request.name {
            block_name = n.clone();
        }
    }

    // Mirror the first pair into the legacy `diff` field so older
    // consumers (history-store entries, downstream tools) reading
    // `block.diff` still see the primary pair instead of `null`.
    let primary_diff = diff_directives.first().cloned();

    // V1: reject `@@for` + `@@compare` on the same block. Both can be valid
    // someday (loop the entire compare block) but that's a separate design
    // exercise; for now we surface the conflict clearly so users notice.
    if for_loop.is_some() && is_compare {
        errors.push(format!(
            "`# @@for` and `# @@compare` cannot be used on the same block in V1"
        ));
        for_loop = None;
    }

    // V1.2: finalize `# @@parallel`. It only makes sense WITH `# @@for` —
    // if for_loop was dropped (compare conflict, or never set), warn and
    // discard `parallel_n` so the round-trip doesn't keep emitting an
    // orphaned directive.
    if let Some(n) = parallel_n {
        if let Some(ref mut fl) = for_loop {
            fl.parallel = Some(n);
        } else {
            errors.push(format!(
                "`# @@parallel` requires a `# @@for` directive on the same block — directive dropped"
            ));
        }
    }

    Some(TestBlock {
        block_type,
        name: block_name,
        description,
        disabled,
        mode,
        dev_auth,
        group,
        depends,
        request,
        assertions,
        extracts,
        compare: is_compare,
        steps,
        diff: primary_diff,
        diffs: diff_directives,
        errors,
        request_id_header: block_request_id_header,
        request_id_disabled: block_request_id_disabled,
        redact_body_rules: block_redact_body_rules,
        for_loop,
    })
}

fn parse_assertion_directive(text: &str) -> Option<Assertion> {
    let text = text.trim();
    let space_idx = text.find(' ')?;
    let left = text[..space_idx].to_string();
    let rest = text[space_idx..].trim();

    let operators = [">=", "<=", "==", "!=", ">", "<", "contains"];
    for op in &operators {
        if rest.starts_with(op) {
            let right = rest[op.len()..].trim().to_string();
            if !right.is_empty() {
                return Some(Assertion {
                    left,
                    operator: op.to_string(),
                    right,
                });
            }
        }
    }

    None
}

fn parse_extract_directive(text: &str) -> Option<Extract> {
    let text = text.trim();
    let (name, path) = text.split_once('=')?;
    Some(Extract {
        variable_name: name.trim().to_string(),
        source_path: path.trim().to_string(),
    })
}

/// Generate .http file content from a TestSuite.
///
/// Writes the canonical NEW syntax:
///   * Top-level `@name = value` for variables (REST Client native)
///   * `### @@type Title` for block separators
///   * `# @@directive value` for Pilot extensions
///
/// Legacy syntax (`@variables` block, `# @directive`, `### @type Title`)
/// is still accepted by the parser but is never emitted by the generator.
pub fn generate_http_content(suite: &TestSuite) -> String {
    let mut output = String::new();

    // File-level auto_run directive
    if let Some(ref interval) = suite.auto_run {
        output.push_str(&format!("# @@auto_run {}\n", interval));
    }

    // File-level request-id directive
    if let Some(ref header) = suite.request_id_header {
        if header == DEFAULT_REQUEST_ID_HEADER {
            output.push_str("# @@request-id\n");
        } else {
            output.push_str(&format!("# @@request-id {}\n", header));
        }
    }

    // Variables — REST Client style: top-level `@name = value`, no @variables block
    if !suite.variables.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        for (name, value) in &suite.variables {
            output.push_str(&format!("@{} = {}\n", name, value));
        }
    }

    for block in &suite.blocks {
        // Blank line separator before each block
        if !output.is_empty() {
            output.push('\n');
        }

        // Block header
        if block.block_type == "request" {
            output.push_str("###\n");
            if !block.name.is_empty() {
                output.push_str(&format!("# @@name {}\n", block.name));
            }
        } else if block.name.is_empty() {
            output.push_str(&format!("### @@{}\n", block.block_type));
        } else {
            output.push_str(&format!("### @@{} {}\n", block.block_type, block.name));
        }

        // Description directive
        if !block.description.is_empty() {
            output.push_str(&format!("# @@description {}\n", block.description));
        }

        // Disabled directive
        if block.disabled {
            output.push_str("# @@disabled\n");
        }

        // Mode directive
        if let Some(ref m) = block.mode {
            output.push_str(&format!("# @@mode {}\n", m));
        }

        // Dev auth scope directive
        if let Some(ref scope) = block.dev_auth {
            output.push_str(&format!("# @@dev_auth {}\n", scope));
        }

        // Group directive
        if let Some(ref g) = block.group {
            output.push_str(&format!("# @@group {}\n", g));
        }

        // Depends directives
        for dep in &block.depends {
            output.push_str(&format!("# @@depends {}\n", dep));
        }

        // Block-level request-id override
        if block.request_id_disabled {
            output.push_str("# @@request-id off\n");
        } else if let Some(ref h) = block.request_id_header {
            if h == DEFAULT_REQUEST_ID_HEADER {
                output.push_str("# @@request-id\n");
            } else {
                output.push_str(&format!("# @@request-id {}\n", h));
            }
        }

        // Compare directive
        if block.compare {
            output.push_str("# @@compare\n");
        }

        // For-loop directive (# @@for iter_var in source_var) — V1 rejects
        // simultaneous compare + for, but defensive code: emit only when
        // compare is false to keep round-trips clean even if data is malformed.
        if !block.compare {
            if let Some(ref fl) = block.for_loop {
                output.push_str(&format!("# @@for {} in {}\n", fl.iter_var, fl.source_var));
                // V1.2: emit `# @@parallel N` right after `# @@for` so the
                // two related directives stay visually grouped. Skip when
                // `parallel.is_none()` or `Some(1)` (sequential default).
                if let Some(n) = fl.parallel {
                    if n > 1 {
                        output.push_str(&format!("# @@parallel {}\n", n));
                    }
                }
            }
        }

        if block.compare && !block.steps.is_empty() {
            // Render each step
            for step in &block.steps {
                output.push_str(&format!("# @@step {}\n", step.name));
                output.push_str(&format!("{} {}\n", step.request.method, step.request.url));
                for (key, value) in &step.request.headers {
                    output.push_str(&format!("{}: {}\n", key, value));
                }
                if let Some(ref body) = step.request.body {
                    output.push('\n');
                    output.push_str(body);
                    output.push('\n');
                }
                if !step.extracts.is_empty() || !step.assertions.is_empty() {
                    output.push('\n');
                }
                for extract in &step.extracts {
                    output.push_str(&format!(
                        "# @@extract {} = {}\n",
                        extract.variable_name, extract.source_path
                    ));
                }
                for assertion in &step.assertions {
                    output.push_str(&format!(
                        "# @@assert {} {} {}\n",
                        assertion.left, assertion.operator, assertion.right
                    ));
                }
            }

            // Diff directives — emit one `# @@diff a b` per pair so
            // multi-diff blocks roundtrip through generate_http without
            // collapsing back to a single pair.
            for diff in block.effective_diffs() {
                output.push_str(&format!("# @@diff {} {}\n", diff.step_a, diff.step_b));
            }

            // Block-level assertions (comparison assertions)
            for assertion in &block.assertions {
                output.push_str(&format!(
                    "# @@assert {} {} {}\n",
                    assertion.left, assertion.operator, assertion.right
                ));
            }
        } else {
            // Normal (non-compare) block rendering
            // Request line
            output.push_str(&format!("{} {}\n", block.request.method, block.request.url));

            // Headers
            for (key, value) in &block.request.headers {
                output.push_str(&format!("{}: {}\n", key, value));
            }

            // Body (preceded by blank line)
            if let Some(ref body) = block.request.body {
                output.push('\n');
                output.push_str(body);
                output.push('\n');
            }

            // Blank line before directives
            if !block.extracts.is_empty() || !block.assertions.is_empty() {
                output.push('\n');
            }

            // Extract directives
            for extract in &block.extracts {
                output.push_str(&format!(
                    "# @@extract {} = {}\n",
                    extract.variable_name, extract.source_path
                ));
            }

            // Assertion directives
            for assertion in &block.assertions {
                output.push_str(&format!(
                    "# @@assert {} {} {}\n",
                    assertion.left, assertion.operator, assertion.right
                ));
            }
        }
    }

    output
}

fn parse_request_from_lines(
    lines: &[&str],
    directive_name: &Option<String>,
) -> Option<ParsedRequest> {
    let mut iter = lines.iter().peekable();
    let mut name = directive_name.clone();

    // Skip leading comments and empty lines; extract name from first comment
    while let Some(line) = iter.peek() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            let comment = trimmed
                .trim_start_matches('#')
                .trim_start_matches("//")
                .trim();
            if !comment.is_empty() && name.is_none() {
                name = Some(comment.to_string());
            }
            iter.next();
        } else if trimmed.is_empty() {
            iter.next();
        } else {
            break;
        }
    }

    // Parse request line: METHOD URL [HTTP/version]
    let request_line = iter.next()?.trim().to_string();
    // Extract method (first word)
    let method_end = request_line.find(' ')?;
    let method = request_line[..method_end].to_uppercase();
    let rest = request_line[method_end + 1..].trim();

    // Check if last part is HTTP version (e.g., "HTTP/1.1", "HTTP/2")
    let url = if let Some(last_space) = rest.rfind(' ') {
        let maybe_version = &rest[last_space + 1..];
        if maybe_version.starts_with("HTTP/") {
            rest[..last_space].trim().to_string()
        } else {
            rest.to_string()
        }
    } else {
        rest.to_string()
    };

    // Parse headers until blank line
    let mut headers = Vec::new();
    let mut found_blank = false;

    for line in iter.by_ref() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            found_blank = true;
            break;
        }
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            headers.push((key.trim().to_string(), value.trim().to_string()));
        }
    }

    // Everything remaining is the body — strip trailing comments and blanks
    let body_lines: Vec<&str> = iter.copied().collect();
    let body = if found_blank && !body_lines.is_empty() {
        // Trim trailing comment lines and empty lines from the end of the body
        let mut end = body_lines.len();
        while end > 0 {
            let t = body_lines[end - 1].trim();
            if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
                end -= 1;
            } else {
                break;
            }
        }
        let body_text = body_lines[..end].join("\n").trim().to_string();
        if body_text.is_empty() {
            None
        } else {
            Some(body_text)
        }
    } else {
        None
    };

    Some(ParsedRequest {
        name,
        method,
        url,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── block_start_lines: 1:1 mapping with parse_test_suite.blocks ──
    #[test]
    fn block_start_lines_skips_variables_and_header() {
        let input = "\
# File header comment
# @auto_run 30s

### @variables
baseUrl = https://example.com

### @test First
GET {{baseUrl}}/a

### @test Second
GET {{baseUrl}}/b
";
        let suite = parse_test_suite(input);
        let starts = block_start_lines(input);
        assert_eq!(starts.len(), suite.blocks.len());
        assert_eq!(suite.blocks.len(), 2);
        // Find expected line indices of each ### @test marker.
        let lines: Vec<&str> = input.split('\n').collect();
        let first = lines.iter().position(|l| l.starts_with("### @test First")).unwrap();
        let second = lines.iter().position(|l| l.starts_with("### @test Second")).unwrap();
        assert_eq!(starts[0], first);
        assert_eq!(starts[1], second);
    }

    #[test]
    fn block_start_lines_no_leading_separator() {
        let input = "GET https://example.com/api\n";
        let suite = parse_test_suite(input);
        let starts = block_start_lines(input);
        assert_eq!(starts.len(), suite.blocks.len());
    }
    // ── parse: simple GET ────────────────────────────────────────────
    #[test]
    fn parse_simple_get() {
        let input = "GET https://example.com/api";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].url, "https://example.com/api");
        assert!(reqs[0].headers.is_empty());
        assert!(reqs[0].body.is_none());
    }

    // ── parse: POST with headers and JSON body ──────────────────────
    #[test]
    fn parse_post_with_headers_and_body() {
        let input = "\
POST https://example.com/api
Content-Type: application/json
Authorization: Bearer tok123

{\"name\":\"test\"}";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(reqs[0].url, "https://example.com/api");
        assert_eq!(reqs[0].headers.len(), 2);
        assert_eq!(reqs[0].headers[0], ("Content-Type".into(), "application/json".into()));
        assert_eq!(reqs[0].headers[1], ("Authorization".into(), "Bearer tok123".into()));
        assert_eq!(reqs[0].body.as_deref(), Some("{\"name\":\"test\"}"));
    }

    // ── parse_test_suite: @variables block ───────────────────────────
    #[test]
    fn parse_variables_block_multiple() {
        let input = "\
### @variables
@host = https://api.example.com
@token = abc123
@count = 42";
        let suite = parse_test_suite(input);
        assert_eq!(suite.variables.len(), 3);
        assert_eq!(suite.variables[0], ("host".into(), "https://api.example.com".into()));
        assert_eq!(suite.variables[1], ("token".into(), "abc123".into()));
        assert_eq!(suite.variables[2], ("count".into(), "42".into()));
    }

    // ── @setup block with extract and assert directives ─────────────
    #[test]
    fn parse_setup_block_with_directives() {
        let input = "\
### @setup Create user
# @extract token = response.body.token
# @assert response.status == 201
POST https://example.com/users

{\"name\":\"test\"}";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let blk = &suite.blocks[0];
        assert_eq!(blk.block_type, "setup");
        assert_eq!(blk.name, "Create user");
        assert_eq!(blk.extracts.len(), 1);
        assert_eq!(blk.extracts[0].variable_name, "token");
        assert_eq!(blk.extracts[0].source_path, "response.body.token");
        assert_eq!(blk.assertions.len(), 1);
        assert_eq!(blk.assertions[0].left, "response.status");
        assert_eq!(blk.assertions[0].operator, "==");
        assert_eq!(blk.assertions[0].right, "201");
    }

    // ── @test block with assertions ─────────────────────────────────
    #[test]
    fn parse_test_block_with_assertions() {
        let input = "\
### @test Check status
# @assert response.status == 200
# @assert response.body.name == \"John\"
GET https://example.com/users/1";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let blk = &suite.blocks[0];
        assert_eq!(blk.block_type, "test");
        assert_eq!(blk.name, "Check status");
        assert_eq!(blk.assertions.len(), 2);
    }

    // ── @teardown block ─────────────────────────────────────────────
    #[test]
    fn parse_teardown_block() {
        let input = "\
### @teardown Cleanup
DELETE https://example.com/users/1";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        assert_eq!(suite.blocks[0].block_type, "teardown");
        assert_eq!(suite.blocks[0].name, "Cleanup");
    }

    // ── Mixed blocks ────────────────────────────────────────────────
    #[test]
    fn parse_mixed_blocks() {
        let input = "\
### @variables
@host = https://api.example.com

### @setup Init
POST {{host}}/init

### @test Verify
# @assert response.status == 200
GET {{host}}/verify

### @teardown Cleanup
DELETE {{host}}/cleanup";
        let suite = parse_test_suite(input);
        assert_eq!(suite.variables.len(), 1);
        assert_eq!(suite.blocks.len(), 3);
        assert_eq!(suite.blocks[0].block_type, "setup");
        assert_eq!(suite.blocks[1].block_type, "test");
        assert_eq!(suite.blocks[2].block_type, "teardown");
    }

    // ── Backwards-compatible .http file ─────────────────────────────
    #[test]
    fn parse_backward_compat_http_file() {
        let input = "\
GET https://example.com/api/one

###

POST https://example.com/api/two
Content-Type: application/json

{\"key\":\"value\"}";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[1].method, "POST");
        assert_eq!(reqs[1].body.as_deref(), Some("{\"key\":\"value\"}"));
    }

    // ── Empty input ─────────────────────────────────────────────────
    #[test]
    fn parse_empty_input() {
        let suite = parse_test_suite("");
        assert!(suite.variables.is_empty());
        assert!(suite.blocks.is_empty());
    }

    // ── Assertion directive operators ───────────────────────────────
    #[test]
    fn parse_assertion_operators() {
        let ops = ["==", "!=", ">", "<", ">=", "<=", "contains"];
        for op in &ops {
            let text = format!("response.status {} 200", op);
            let a = parse_assertion_directive(&text);
            assert!(a.is_some(), "failed to parse operator: {}", op);
            let a = a.unwrap();
            assert_eq!(a.left, "response.status");
            assert_eq!(a.operator, *op);
            assert_eq!(a.right, "200");
        }
    }

    // ── Extract directive ───────────────────────────────────────────
    #[test]
    fn parse_extract_directive_valid() {
        let ext = parse_extract_directive("token = response.body.token");
        assert!(ext.is_some());
        let ext = ext.unwrap();
        assert_eq!(ext.variable_name, "token");
        assert_eq!(ext.source_path, "response.body.token");
    }

    // ── # @name directive ───────────────────────────────────────────
    #[test]
    fn parse_name_directive() {
        let input = "\
### 
# @name My Request
GET https://example.com/api";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        assert_eq!(suite.blocks[0].request.name.as_deref(), Some("My Request"));
    }

    // ── Body after blank line ───────────────────────────────────────
    #[test]
    fn body_parsed_after_blank_line() {
        let input = "\
POST https://example.com/data
Content-Type: text/plain

Hello, World!";
        let reqs = parse(input);
        assert_eq!(reqs[0].body.as_deref(), Some("Hello, World!"));
    }

    // ── Headers stop at blank line ──────────────────────────────────
    #[test]
    fn headers_stop_at_blank_line() {
        let input = "\
GET https://example.com
X-Custom: value1
Accept: text/html

body content here";
        let reqs = parse(input);
        assert_eq!(reqs[0].headers.len(), 2);
        assert_eq!(reqs[0].body.as_deref(), Some("body content here"));
    }

    // ── No body when no blank line ──────────────────────────────────
    #[test]
    fn no_body_without_blank_line() {
        let input = "\
GET https://example.com
Accept: text/html";
        let reqs = parse(input);
        assert!(reqs[0].body.is_none());
    }

    // ── Block name falls back to request name ───────────────────────
    #[test]
    fn block_name_from_request_name() {
        let input = "\
###
# @name Login
POST https://example.com/login";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].name, "Login");
    }

    // ── @prompt directive parsing ───────────────────────────────────
    #[test]
    fn test_prompt_directive_basic() {
        let input = "\
@prompt api_key Your API key
@prompt region Azure region

### @@test List
GET https://example.com/\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.prompts.len(), 2);
        assert_eq!(suite.prompts[0].name, "api_key");
        assert_eq!(suite.prompts[0].description, "Your API key");
        assert_eq!(suite.prompts[1].name, "region");
        assert_eq!(suite.prompts[1].description, "Azure region");
    }

    #[test]
    fn test_prompt_directive_no_description() {
        let input = "\
@prompt token

### @@test X
GET https://example.com/\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.prompts.len(), 1);
        assert_eq!(suite.prompts[0].name, "token");
        assert_eq!(suite.prompts[0].description, "");
    }

    #[test]
    fn test_prompt_directive_dedup() {
        let input = "\
@prompt token first
@prompt token second

### @@test X
GET https://example.com/\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.prompts.len(), 1);
        assert_eq!(suite.prompts[0].description, "first");
    }

    #[test]
    fn test_prompt_in_legacy_variables_block() {
        let input = "\
@variables
base_url = https://example.com
@prompt token Auth token

### @@test X
GET {{base_url}}\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.prompts.len(), 1);
        assert_eq!(suite.prompts[0].name, "token");
        assert_eq!(suite.variables, vec![("base_url".to_string(), "https://example.com".to_string())]);
    }

    // ── generate_http_content: empty suite ──────────────────────────
    #[test]
    fn test_generate_empty_suite() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.is_empty());
    }

    // ── generate_http_content: variables only ───────────────────────
    #[test]
    fn test_generate_variables_only() {
        let suite = TestSuite {
            variables: vec![
                ("baseUrl".into(), "https://api.example.com".into()),
                ("token".into(), "abc123".into()),
            ],
            blocks: vec![],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("@baseUrl = https://api.example.com"));
        assert!(output.contains("@token = abc123"));
    }

    // ── generate_http_content: simple GET ───────────────────────────
    #[test]
    fn test_generate_simple_get() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![TestBlock {
                block_type: "request".into(),
                name: String::new(),
                description: String::new(),
                disabled: false,
                mode: None,
                dev_auth: None,
                group: None,
                depends: Vec::new(),
                request: ParsedRequest {
                    name: None,
                    method: "GET".into(),
                    url: "https://example.com/api".into(),
                    headers: vec![],
                    body: None,
                },
                assertions: vec![],
                extracts: vec![],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("###"));
        assert!(output.contains("GET https://example.com/api"));
        assert!(!output.contains("# @@name"));
    }

    // ── generate_http_content: POST with headers and body ───────────
    #[test]
    fn test_generate_post_with_body() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![TestBlock {
                block_type: "request".into(),
                name: "Create User".into(),
                description: String::new(),
                disabled: false,
                mode: None,
                dev_auth: None,
                group: None,
                depends: Vec::new(),
                request: ParsedRequest {
                    name: Some("Create User".into()),
                    method: "POST".into(),
                    url: "https://example.com/users".into(),
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: Some("{\"name\":\"test\"}".into()),
                },
                assertions: vec![],
                extracts: vec![],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("# @@name Create User"));
        assert!(output.contains("POST https://example.com/users"));
        assert!(output.contains("Content-Type: application/json"));
        assert!(output.contains("{\"name\":\"test\"}"));
    }

    // ── generate_http_content: with assertions and extracts ─────────
    #[test]
    fn test_generate_with_assertions() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![TestBlock {
                block_type: "test".into(),
                name: "Check Users".into(),
                description: String::new(),
                disabled: false,
                mode: None,
                dev_auth: None,
                group: None,
                depends: Vec::new(),
                request: ParsedRequest {
                    name: None,
                    method: "GET".into(),
                    url: "https://example.com/users".into(),
                    headers: vec![],
                    body: None,
                },
                assertions: vec![
                    Assertion {
                        left: "response.status".into(),
                        operator: "==".into(),
                        right: "200".into(),
                    },
                    Assertion {
                        left: "response.body.length".into(),
                        operator: ">".into(),
                        right: "0".into(),
                    },
                ],
                extracts: vec![Extract {
                    variable_name: "userId".into(),
                    source_path: "response.body.0.id".into(),
                }],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("### @@test Check Users"));
        assert!(output.contains("# @@extract userId = response.body.0.id"));
        assert!(output.contains("# @@assert response.status == 200"));
        assert!(output.contains("# @@assert response.body.length > 0"));
    }

    // ── generate_http_content: full suite ───────────────────────────
    #[test]
    fn test_generate_full_suite() {
        let suite = TestSuite {
            variables: vec![
                ("baseUrl".into(), "https://api.example.com".into()),
                ("token".into(), "abc123".into()),
            ],
            blocks: vec![
                TestBlock {
                    block_type: "setup".into(),
                    name: "Login".into(),
                    description: String::new(),
                    disabled: false,
                    mode: None,
                dev_auth: None,
                    group: None,
                    depends: Vec::new(),
                    request: ParsedRequest {
                        name: None,
                        method: "POST".into(),
                        url: "{{baseUrl}}/auth/login".into(),
                        headers: vec![("Content-Type".into(), "application/json".into())],
                        body: Some("{\"email\":\"admin@test.com\"}".into()),
                    },
                    assertions: vec![Assertion {
                        left: "response.status".into(),
                        operator: "==".into(),
                        right: "200".into(),
                    }],
                    extracts: vec![Extract {
                        variable_name: "token".into(),
                        source_path: "response.body.token".into(),
                    }],
                    compare: false,
                    steps: Vec::new(),
                    diff: None,
                    diffs: Vec::new(),
                errors: Vec::new(),
                request_id_header: None,
                request_id_disabled: false,
                redact_body_rules: Vec::new(),
                for_loop: None,
                },
                TestBlock {
                    block_type: "test".into(),
                    name: "List Users".into(),
                    description: String::new(),
                    disabled: false,
                    mode: None,
                dev_auth: None,
                    group: None,
                    depends: Vec::new(),
                    request: ParsedRequest {
                        name: None,
                        method: "GET".into(),
                        url: "{{baseUrl}}/api/users".into(),
                        headers: vec![("Authorization".into(), "Bearer {{token}}".into())],
                        body: None,
                    },
                    assertions: vec![
                        Assertion {
                            left: "response.status".into(),
                            operator: "==".into(),
                            right: "200".into(),
                        },
                        Assertion {
                            left: "response.body.length".into(),
                            operator: ">".into(),
                            right: "0".into(),
                        },
                    ],
                    extracts: vec![],
                    compare: false,
                    steps: Vec::new(),
                    diff: None,
                    diffs: Vec::new(),
                errors: Vec::new(),
                request_id_header: None,
                request_id_disabled: false,
                redact_body_rules: Vec::new(),
                for_loop: None,
                },
                TestBlock {
                    block_type: "teardown".into(),
                    name: "Cleanup".into(),
                    description: String::new(),
                    disabled: false,
                    mode: None,
                dev_auth: None,
                    group: None,
                    depends: Vec::new(),
                    request: ParsedRequest {
                        name: None,
                        method: "DELETE".into(),
                        url: "{{baseUrl}}/cleanup".into(),
                        headers: vec![],
                        body: None,
                    },
                    assertions: vec![],
                    extracts: vec![],
                    compare: false,
                    steps: Vec::new(),
                    diff: None,
                    diffs: Vec::new(),
                    errors: Vec::new(),
                    request_id_header: None,
                    request_id_disabled: false,
                    redact_body_rules: Vec::new(),
                    for_loop: None,
                },
            ],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("### @@setup Login"));
        assert!(output.contains("### @@test List Users"));
        assert!(output.contains("### @@teardown Cleanup"));
        assert!(output.contains("# @@extract token = response.body.token"));
        assert!(output.contains("# @@assert response.status == 200"));
        assert!(output.contains("DELETE {{baseUrl}}/cleanup"));
    }

    // ── roundtrip: generate → parse produces equivalent suite ───────
    #[test]
    fn test_roundtrip() {
        let original = TestSuite {
            variables: vec![
                ("baseUrl".into(), "https://api.example.com".into()),
                ("token".into(), "abc123".into()),
            ],
            blocks: vec![
                TestBlock {
                    block_type: "setup".into(),
                    name: "Login".into(),
                    description: String::new(),
                    disabled: false,
                    mode: None,
                dev_auth: None,
                    group: None,
                    depends: Vec::new(),
                    request: ParsedRequest {
                        name: None,
                        method: "POST".into(),
                        url: "{{baseUrl}}/auth/login".into(),
                        headers: vec![("Content-Type".into(), "application/json".into())],
                        body: Some("{\"email\":\"admin@test.com\"}".into()),
                    },
                    assertions: vec![Assertion {
                        left: "response.status".into(),
                        operator: "==".into(),
                        right: "200".into(),
                    }],
                    extracts: vec![Extract {
                        variable_name: "token".into(),
                        source_path: "response.body.token".into(),
                    }],
                    compare: false,
                    steps: Vec::new(),
                    diff: None,
                    diffs: Vec::new(),
                errors: Vec::new(),
                request_id_header: None,
                request_id_disabled: false,
                redact_body_rules: Vec::new(),
                for_loop: None,
                },
                TestBlock {
                    block_type: "test".into(),
                    name: "List Users".into(),
                    description: String::new(),
                    disabled: false,
                    mode: None,
                dev_auth: None,
                    group: None,
                    depends: Vec::new(),
                    request: ParsedRequest {
                        name: None,
                        method: "GET".into(),
                        url: "{{baseUrl}}/api/users".into(),
                        headers: vec![("Authorization".into(), "Bearer {{token}}".into())],
                        body: None,
                    },
                    assertions: vec![Assertion {
                        left: "response.status".into(),
                        operator: "==".into(),
                        right: "200".into(),
                    }],
                    extracts: vec![],
                    compare: false,
                    steps: Vec::new(),
                    diff: None,
                    diffs: Vec::new(),
                errors: Vec::new(),
                request_id_header: None,
                request_id_disabled: false,
                redact_body_rules: Vec::new(),
                for_loop: None,
                },
                TestBlock {
                    block_type: "teardown".into(),
                    name: "Cleanup".into(),
                    description: String::new(),
                    disabled: false,
                    mode: None,
                dev_auth: None,
                    group: None,
                    depends: Vec::new(),
                    request: ParsedRequest {
                        name: None,
                        method: "DELETE".into(),
                        url: "{{baseUrl}}/cleanup".into(),
                        headers: vec![],
                        body: None,
                    },
                    assertions: vec![],
                    extracts: vec![],
                    compare: false,
                    steps: Vec::new(),
                    diff: None,
                    diffs: Vec::new(),
                    errors: Vec::new(),
                    request_id_header: None,
                    request_id_disabled: false,
                    redact_body_rules: Vec::new(),
                    for_loop: None,
                },
            ],
            ..Default::default()
        };

        let generated = generate_http_content(&original);
        let reparsed = parse_test_suite(&generated);

        // Same number of variables
        assert_eq!(
            reparsed.variables.len(),
            original.variables.len(),
            "variable count mismatch"
        );
        for (orig, rep) in original.variables.iter().zip(reparsed.variables.iter()) {
            assert_eq!(orig.0, rep.0, "variable name mismatch");
            assert_eq!(orig.1, rep.1, "variable value mismatch");
        }

        // Same number of blocks
        assert_eq!(
            reparsed.blocks.len(),
            original.blocks.len(),
            "block count mismatch"
        );
        for (orig, rep) in original.blocks.iter().zip(reparsed.blocks.iter()) {
            assert_eq!(orig.block_type, rep.block_type, "block_type mismatch");
            assert_eq!(orig.name, rep.name, "block name mismatch");
            assert_eq!(
                orig.request.method, rep.request.method,
                "request method mismatch"
            );
            assert_eq!(orig.request.url, rep.request.url, "request url mismatch");
            assert_eq!(
                orig.request.headers.len(),
                rep.request.headers.len(),
                "headers count mismatch"
            );
            assert_eq!(orig.request.body, rep.request.body, "body mismatch");
            assert_eq!(
                orig.assertions.len(),
                rep.assertions.len(),
                "assertions count mismatch"
            );
            for (a_orig, a_rep) in orig.assertions.iter().zip(rep.assertions.iter()) {
                assert_eq!(a_orig.left, a_rep.left);
                assert_eq!(a_orig.operator, a_rep.operator);
                assert_eq!(a_orig.right, a_rep.right);
            }
            assert_eq!(
                orig.extracts.len(),
                rep.extracts.len(),
                "extracts count mismatch"
            );
            for (e_orig, e_rep) in orig.extracts.iter().zip(rep.extracts.iter()) {
                assert_eq!(e_orig.variable_name, e_rep.variable_name);
                assert_eq!(e_orig.source_path, e_rep.source_path);
            }
        }
    }

    #[test]
    fn test_parse_description_directive() {
        let content = "### @test My Test\n# @description Tests the API endpoint\nGET https://api.example.com\n\n# @assert status == 200\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks.len(), 1);
        assert_eq!(suite.blocks[0].description, "Tests the API endpoint");
    }

    #[test]
    fn test_parse_disabled_directive() {
        let content = "### @test Disabled Test\n# @disabled\nGET https://api.example.com\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks.len(), 1);
        assert!(suite.blocks[0].disabled);
    }

    #[test]
    fn test_parse_not_disabled_by_default() {
        let content = "### @test Normal Test\nGET https://api.example.com\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks.len(), 1);
        assert!(!suite.blocks[0].disabled);
    }

    #[test]
    fn test_generate_description_and_disabled() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![TestBlock {
                block_type: "test".to_string(),
                name: "My Test".to_string(),
                description: "Tests something".to_string(),
                disabled: true,
                mode: None,
                dev_auth: None,
                group: None,
                depends: Vec::new(),
                request: ParsedRequest {
                    name: None,
                    method: "GET".to_string(),
                    url: "https://example.com".to_string(),
                    headers: vec![],
                    body: None,
                },
                assertions: vec![],
                extracts: vec![],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @@description Tests something"));
        assert!(content.contains("# @@disabled"));
    }

    #[test]
    fn test_roundtrip_description_disabled() {
        let original = "### @setup My Setup\n# @description Fetches the auth token\n# @disabled\nPOST https://auth.example.com/token\nContent-Type: application/x-www-form-urlencoded\n\ngrant_type=client_credentials\n\n# @assert status == 200\n";
        let suite = parse_test_suite(original);
        assert_eq!(suite.blocks[0].description, "Fetches the auth token");
        assert!(suite.blocks[0].disabled);
        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @@description Fetches the auth token"));
        assert!(regenerated.contains("# @@disabled"));
    }

    #[test]
    fn test_parse_variables_with_comments_before() {
        let input = "\
# ============================================================
# This is a file header comment
# ============================================================

@variables
tenant_id = your-tenant-id
api_version = 2023-04-03
mgmt_api = https://management.azure.com

### @setup Fetch Token
POST https://login.example.com/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.variables.len(), 3, "Should parse 3 variables even with comments before @variables");
        assert_eq!(suite.variables[0], ("tenant_id".to_string(), "your-tenant-id".to_string()));
        assert_eq!(suite.variables[1], ("api_version".to_string(), "2023-04-03".to_string()));
        assert_eq!(suite.variables[2], ("mgmt_api".to_string(), "https://management.azure.com".to_string()));
        assert_eq!(suite.blocks.len(), 1);
    }

    #[test]
    fn test_generate_includes_variables() {
        let suite = TestSuite {
            variables: vec![
                ("base_url".to_string(), "https://api.example.com".to_string()),
                ("api_key".to_string(), "sk-12345".to_string()),
            ],
            blocks: vec![TestBlock {
                block_type: "test".to_string(),
                name: "Get Users".to_string(),
                description: String::new(),
                disabled: false,
                mode: None,
                dev_auth: None,
                group: None,
                depends: Vec::new(),
                request: ParsedRequest {
                    name: None,
                    method: "GET".to_string(),
                    url: "{{base_url}}/users".to_string(),
                    headers: vec![],
                    body: None,
                },
                assertions: vec![],
                extracts: vec![],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.starts_with("@base_url = https://api.example.com\n"), "Generated content should start with @var = value");
        assert!(content.contains("@base_url = https://api.example.com"));
        assert!(content.contains("@api_key = sk-12345"));
    }

    #[test]
    fn test_parse_depends_directive() {
        let input = "### @test Verify Update\n# @depends Update User\nGET https://api.example.com/users/1\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].depends, vec!["Update User"]);
    }

    #[test]
    fn test_parse_multiple_depends() {
        let input = "### @test Final Check\n# @depends Create Resource\n# @depends Update Resource\nGET https://api.example.com/resources/1\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].depends, vec!["Create Resource", "Update Resource"]);
    }

    #[test]
    fn test_generate_depends() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![TestBlock {
                block_type: "test".to_string(),
                name: "Verify Update".to_string(),
                description: String::new(),
                disabled: false,
                mode: None,
                dev_auth: None,
                group: None,
                depends: vec!["Update User".to_string()],
                request: ParsedRequest {
                    name: None,
                    method: "GET".to_string(),
                    url: "https://api.example.com/users/1".to_string(),
                    headers: vec![],
                    body: None,
                },
                assertions: vec![],
                extracts: vec![],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @@depends Update User"));
    }

    #[test]
    fn test_roundtrip_depends() {
        let input = "### @test Verify Update\n# @depends Update User\n# @depends Create User\nGET https://api.example.com/users/1\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].depends.len(), 2);
        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @@depends Update User"));
        assert!(regenerated.contains("# @@depends Create User"));
    }

    #[test]
    fn test_parse_group_directive() {
        let input = "### @test Validate Order\n# @group validation\nGET https://api.example.com/orders/1\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].group, Some("validation".to_string()));
    }

    #[test]
    fn test_parse_group_and_depends() {
        let input = "### @test Validate Invoice\n# @group validation\n# @depends setup-data\nGET https://api.example.com/invoices/1\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].group, Some("validation".to_string()));
        assert_eq!(suite.blocks[0].depends, vec!["setup-data"]);
    }

    #[test]
    fn test_generate_group() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![TestBlock {
                block_type: "test".to_string(),
                name: "Validate Order".to_string(),
                description: String::new(),
                disabled: false,
                mode: None,
                dev_auth: None,
                group: Some("validation".to_string()),
                depends: vec!["setup-data".to_string()],
                request: ParsedRequest {
                    name: None,
                    method: "GET".to_string(),
                    url: "https://api.example.com/orders/1".to_string(),
                    headers: vec![],
                    body: None,
                },
                assertions: vec![],
                extracts: vec![],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @@group validation"));
        assert!(content.contains("# @@depends setup-data"));
    }

    #[test]
    fn test_roundtrip_nested_groups() {
        let input = "\
### @test Create Order
# @group create
POST https://api.example.com/orders
Content-Type: application/json

{\"name\": \"test\"}

# @extract order_id = $.id
# @assert status == 201

### @test Validate Order
# @group validate
# @depends create
GET https://api.example.com/orders/1

# @assert status == 200

### @test Generate Report
# @group report
# @depends validate
GET https://api.example.com/report

# @assert status == 200
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 3);
        assert_eq!(suite.blocks[0].group, Some("create".to_string()));
        assert!(suite.blocks[0].depends.is_empty());
        assert_eq!(suite.blocks[1].group, Some("validate".to_string()));
        assert_eq!(suite.blocks[1].depends, vec!["create"]);
        assert_eq!(suite.blocks[2].group, Some("report".to_string()));
        assert_eq!(suite.blocks[2].depends, vec!["validate"]);

        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @@group create"));
        assert!(regenerated.contains("# @@group validate"));
        assert!(regenerated.contains("# @@group report"));
        assert!(regenerated.contains("# @@depends create"));
        assert!(regenerated.contains("# @@depends validate"));
    }

    #[test]
    fn parse_url_with_spaces_in_query() {
        let input = r#"GET https://prom.example.com/api/v1/query_range?query={"system.cpu.time"} * on() group_left up&start=2026-03-17T19:00:00Z&step=60"#;
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].url, r#"https://prom.example.com/api/v1/query_range?query={"system.cpu.time"} * on() group_left up&start=2026-03-17T19:00:00Z&step=60"#);
    }

    #[test]
    fn parse_url_with_spaces_and_http_version() {
        let input = r#"GET https://example.com/api?query=foo bar baz HTTP/1.1"#;
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].url, "https://example.com/api?query=foo bar baz");
    }

    #[test]
    fn parse_url_with_braces_and_special_chars() {
        let input = "GET https://example.com/api?q={\"cpu\"} * on()\nContent-Type: application/json";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].url, r#"https://example.com/api?q={"cpu"} * on()"#);
        assert_eq!(reqs[0].headers.len(), 1);
    }

    #[test]
    fn parse_url_no_spaces_still_works() {
        let input = "GET https://example.com/api?query=foo&bar=baz";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].url, "https://example.com/api?query=foo&bar=baz");
    }

    #[test]
    fn parse_url_no_spaces_with_http_version() {
        let input = "POST https://example.com/api HTTP/1.1\nContent-Type: application/json\n\n{\"name\":\"test\"}";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(reqs[0].url, "https://example.com/api");
        assert_eq!(reqs[0].body.as_deref(), Some("{\"name\":\"test\"}"));
    }

    #[test]
    fn body_strips_trailing_comments() {
        let input = "\
POST https://example.com/api
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&scope=https://api.example.com/.default

# This trailing comment should not be in the body
# ============================================================
#  Section decoration
# ============================================================";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(
            reqs[0].body.as_deref(),
            Some("grant_type=client_credentials&scope=https://api.example.com/.default")
        );
    }

    #[test]
    fn body_preserves_mid_body_hash_lines() {
        // YAML-like body where # appears in the middle — should be preserved
        let input = "\
POST https://example.com/api
Content-Type: text/yaml

# this is part of the body
key: value
another_key: another_value";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(
            reqs[0].body.as_deref(),
            Some("# this is part of the body\nkey: value\nanother_key: another_value")
        );
    }

    #[test]
    fn body_strips_trailing_comments_and_blanks() {
        let input = "\
POST https://example.com/api
Content-Type: application/json

{\"name\":\"test\"}

# trailing comment
# another comment

";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].body.as_deref(), Some("{\"name\":\"test\"}"));
    }

    #[test]
    fn body_all_comments_results_in_no_body() {
        // Body section is ONLY comments — should result in None
        let input = "\
GET https://example.com/api
Authorization: Bearer token

# This is just a comment
# Another comment";
        let reqs = parse(input);
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].body.is_none());
    }

    #[test]
    fn test_suite_section_comments_not_in_body() {
        let input = "\
### @setup Auth
POST https://example.com/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials

# @extract token = $.access_token
# @assert status == 200

# ============================================================
#  TESTS — section header decoration
# ============================================================

# --------------------------------------------------
# Group: my-tests
# --------------------------------------------------

### @test Check status
GET https://example.com/api
Authorization: Bearer {{token}}

# @assert status == 200";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 2);
        // The setup block's body should be ONLY the form data, not the section comments
        assert_eq!(
            suite.blocks[0].request.body.as_deref(),
            Some("grant_type=client_credentials")
        );
        // The test block should have no body
        assert!(suite.blocks[1].request.body.is_none());
    }

    #[test]
    fn test_parse_mode_directive_app() {
        let content = "### @setup Fetch Token\n# @mode app\nPOST https://login.example.com/token\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks[0].mode, Some("app".to_string()));
    }

    #[test]
    fn test_parse_mode_directive_dev() {
        let content = "### @setup Dev Auth\n# @mode dev\nGET https://example.com/me\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks[0].mode, Some("dev".to_string()));
    }

    #[test]
    fn test_parse_no_mode_is_none() {
        let content = "### @test Normal\nGET https://example.com\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks[0].mode, None);
    }

    #[test]
    fn test_generate_mode_directive() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![TestBlock {
                block_type: "setup".to_string(),
                name: "Fetch Token".to_string(),
                description: String::new(),
                disabled: false,
                mode: Some("app".to_string()),
                dev_auth: None,
                group: None,
                depends: Vec::new(),
                request: ParsedRequest {
                    name: None,
                    method: "POST".to_string(),
                    url: "https://login.example.com/token".to_string(),
                    headers: vec![],
                    body: None,
                },
                assertions: vec![],
                extracts: vec![],
                compare: false,
                steps: Vec::new(),
                diff: None,
                diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @@mode app"));
    }

    #[test]
    fn test_roundtrip_mode_directive() {
        let original = "### @setup Fetch Token\n# @mode app\nPOST https://login.example.com/token\n";
        let suite = parse_test_suite(original);
        assert_eq!(suite.blocks[0].mode, Some("app".to_string()));
        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @@mode app"));
        let reparsed = parse_test_suite(&regenerated);
        assert_eq!(reparsed.blocks[0].mode, Some("app".to_string()));
    }

    #[test]
    fn test_parse_dev_auth_directive() {
        let content = "### @setup Fetch ARM Token\n# @mode app\n# @dev_auth https://management.azure.com/.default\nPOST https://login.example.com/token\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks[0].mode, Some("app".to_string()));
        assert_eq!(suite.blocks[0].dev_auth, Some("https://management.azure.com/.default".to_string()));
    }

    #[test]
    fn test_roundtrip_dev_auth_directive() {
        let content = "### @setup Fetch Token\n# @mode app\n# @dev_auth https://prometheus.monitor.azure.com/.default\nPOST https://login.example.com/token\n# @extract access_token = $.access_token\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks[0].dev_auth, Some("https://prometheus.monitor.azure.com/.default".to_string()));
        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @@dev_auth https://prometheus.monitor.azure.com/.default"));
        let reparsed = parse_test_suite(&regenerated);
        assert_eq!(reparsed.blocks[0].dev_auth, Some("https://prometheus.monitor.azure.com/.default".to_string()));
    }

    #[test]
    fn test_no_dev_auth_is_none() {
        let content = "### @setup Verify Cluster\nGET https://management.azure.com/subscriptions/123\n";
        let suite = parse_test_suite(content);
        assert_eq!(suite.blocks[0].dev_auth, None);
    }

    #[test]
    fn test_telemetry_variables_parsed() {
        let input = "@variables\ntelemetry_traces_endpoint = https://example.com/v1/traces\ntelemetry_metrics_endpoint = https://example.com/v1/metrics\ntelemetry_logs_endpoint = https://example.com/v1/logs\ntelemetry_token = my-token\ntelemetry_service = my-e2e\n\n### @test Ping\nGET https://example.com\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        // Telemetry endpoints are regular variables
        assert!(suite.variables.iter().any(|(k, v)| k == "telemetry_traces_endpoint" && v == "https://example.com/v1/traces"));
        assert!(suite.variables.iter().any(|(k, v)| k == "telemetry_metrics_endpoint" && v == "https://example.com/v1/metrics"));
        assert!(suite.variables.iter().any(|(k, v)| k == "telemetry_logs_endpoint" && v == "https://example.com/v1/logs"));
        assert!(suite.variables.iter().any(|(k, v)| k == "telemetry_token" && v == "my-token"));
        assert!(suite.variables.iter().any(|(k, v)| k == "telemetry_service" && v == "my-e2e"));
    }

    // ── @compare block parsing ───────────────────────

    #[test]
    fn parse_compare_block_with_two_steps() {
        let input = r#"
### @test Compare APIs
# @compare
# @description Compares v1 and v2
# @step baseline
GET https://api.example.com/v1/users
Authorization: Bearer token123

# @assert status == 200

# @step candidate
GET https://api.example.com/v2/users
Authorization: Bearer token123

# @assert status == 200

# @diff baseline candidate
# @assert $diff.match == true
"#;
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let block = &suite.blocks[0];
        assert!(block.compare);
        assert_eq!(block.block_type, "test");
        assert_eq!(block.name, "Compare APIs");
        assert_eq!(block.steps.len(), 2);
        assert_eq!(block.steps[0].name, "baseline");
        assert_eq!(block.steps[0].request.method, "GET");
        assert_eq!(block.steps[0].request.url, "https://api.example.com/v1/users");
        assert_eq!(block.steps[0].assertions.len(), 1);
        assert_eq!(block.steps[1].name, "candidate");
        assert_eq!(block.steps[1].request.method, "GET");
        assert_eq!(block.steps[1].request.url, "https://api.example.com/v2/users");
        assert!(block.diff.is_some());
        let diff = block.diff.as_ref().unwrap();
        assert_eq!(diff.step_a, "baseline");
        assert_eq!(diff.step_b, "candidate");
        // Single-diff blocks also populate the multi-pair `diffs` vec
        // (the parser mirrors `diffs[0]` into `diff` for compat).
        assert_eq!(block.diffs.len(), 1);
        assert_eq!(block.diffs[0].step_a, "baseline");
        assert_eq!(block.diffs[0].step_b, "candidate");
        assert_eq!(block.effective_diffs().len(), 1);
        // Block-level assertions ($diff.*)
        assert_eq!(block.assertions.len(), 1);
        assert_eq!(block.assertions[0].left, "$diff.match");
    }

    #[test]
    fn parse_compare_step_with_extracts() {
        let input = r#"
### @test Compare with extracts
# @compare
# @step first
POST https://api.example.com/query
Content-Type: application/json

{"query": "test"}

# @extract result_count = $.count
# @assert status == 200

# @step second
POST https://api.example.com/query-v2
Content-Type: application/json

{"query": "test"}

# @extract result_count_v2 = $.count
# @assert status == 200

# @diff first second
# @assert $diff.changed_count == 0
"#;
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        assert!(block.compare);
        assert_eq!(block.steps.len(), 2);
        assert_eq!(block.steps[0].extracts.len(), 1);
        assert_eq!(block.steps[0].extracts[0].variable_name, "result_count");
        assert_eq!(block.steps[1].extracts.len(), 1);
        assert!(block.steps[0].request.body.is_some());
    }

    #[test]
    fn parse_compare_auto_enables_on_step() {
        // Even without explicit # @compare, # @step auto-enables compare mode
        let input = r#"
### @test Auto Compare
# @step a
GET https://api.com/a
# @step b
GET https://api.com/b
"#;
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        assert!(block.compare);
        assert_eq!(block.steps.len(), 2);
    }

    #[test]
    fn parse_compare_roundtrip() {
        let input = r#"
### @test Compare Roundtrip
# @description Tests generation roundtrip
# @compare
# @step baseline
GET https://api.example.com/v1
Authorization: Bearer {{token}}

# @extract v1_id = $.id
# @assert status == 200

# @step candidate
GET https://api.example.com/v2
Authorization: Bearer {{token}}

# @extract v2_id = $.id
# @assert status == 200

# @diff baseline candidate
# @assert $diff.match == true
"#;
        let suite = parse_test_suite(input);
        let generated = generate_http_content(&suite);
        // Verify key directives survive roundtrip
        assert!(generated.contains("# @@compare"));
        assert!(generated.contains("# @@step baseline"));
        assert!(generated.contains("# @@step candidate"));
        assert!(generated.contains("# @@diff baseline candidate"));
        assert!(generated.contains("# @@assert $diff.match == true"));
        assert!(generated.contains("# @@extract v1_id = $.id"));
    }

    // ── Multi-diff: multiple `# @@diff a b` lines per block ─────────
    #[test]
    fn parse_multiple_diff_directives_in_one_block() {
        let input = r#"
### @test Compare three versions
# @compare
# @step v1
GET https://api.example.com/v1/users

# @step v2
GET https://api.example.com/v2/users

# @step v3
GET https://api.example.com/v3/users

# @diff v1 v2
# @diff v1 v3
# @assert $diff.match == true
"#;
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let block = &suite.blocks[0];
        assert_eq!(block.steps.len(), 3);
        // Both `# @@diff` lines accumulate into the multi-pair vec.
        assert_eq!(block.diffs.len(), 2);
        assert_eq!(block.diffs[0].step_a, "v1");
        assert_eq!(block.diffs[0].step_b, "v2");
        assert_eq!(block.diffs[1].step_a, "v1");
        assert_eq!(block.diffs[1].step_b, "v3");
        // Primary (compat) field mirrors the first pair.
        let primary = block.diff.as_ref().unwrap();
        assert_eq!(primary.step_a, "v1");
        assert_eq!(primary.step_b, "v2");
        // effective_diffs() exposes all pairs.
        assert_eq!(block.effective_diffs().len(), 2);
    }

    #[test]
    fn multi_diff_roundtrips_through_generator() {
        let input = r#"
### @test Compare three versions
# @compare
# @step v1
GET https://api.example.com/v1/users

# @step v2
GET https://api.example.com/v2/users

# @step v3
GET https://api.example.com/v3/users

# @diff v1 v2
# @diff v1 v3
"#;
        let suite = parse_test_suite(input);
        let generated = generate_http_content(&suite);
        // Both pairs must be emitted — no collapsing back to a single pair.
        assert!(
            generated.contains("# @@diff v1 v2"),
            "expected v1↔v2 directive in:\n{generated}"
        );
        assert!(
            generated.contains("# @@diff v1 v3"),
            "expected v1↔v3 directive in:\n{generated}"
        );
        // Re-parse the generated text and verify the second roundtrip
        // still produces both pairs (idempotent).
        let suite2 = parse_test_suite(&generated);
        let block2 = &suite2.blocks[0];
        assert_eq!(block2.diffs.len(), 2);
        assert_eq!(block2.diffs[0].step_b, "v2");
        assert_eq!(block2.diffs[1].step_b, "v3");
    }

    #[test]
    fn multi_diff_validates_each_pair_independently() {
        // A bad reference in one pair should produce one error per bad ref,
        // not silently drop the other valid pair.
        let input = r#"
### @test Compare with bad ref
# @compare
# @step v1
GET https://api.example.com/v1

# @step v2
GET https://api.example.com/v2

# @diff v1 v2
# @diff v1 nonexistent
"#;
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        // Both pairs are parsed (validation reports the error but keeps
        // the pair in the model so the UI can surface it).
        assert_eq!(block.diffs.len(), 2);
        // Validation surfaces the unknown step.
        assert!(
            block.errors.iter().any(|e| e.contains("nonexistent")),
            "expected validation error for unknown step, got: {:?}",
            block.errors
        );
    }

    #[test]
    fn effective_diffs_prefers_multi_pair_vec_over_legacy_field() {
        // When `diffs` has entries, they win. The legacy `diff` field is
        // ignored even if populated (writers typically mirror diffs[0]
        // into diff for older readers, but `effective_diffs()` must not
        // double-count that mirror).
        let block = TestBlock {
            block_type: "test".to_string(),
            name: "n".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: None,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: true,
            steps: Vec::new(),
            diff: Some(DiffDirective {
                step_a: "v1".to_string(),
                step_b: "v2".to_string(),
            }),
            diffs: vec![
                DiffDirective {
                    step_a: "v1".to_string(),
                    step_b: "v2".to_string(),
                },
                DiffDirective {
                    step_a: "v1".to_string(),
                    step_b: "v3".to_string(),
                },
            ],
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };
        let eff = block.effective_diffs();
        assert_eq!(eff.len(), 2, "must use diffs vec, not double-count diff");
        assert_eq!(eff[0].step_b, "v2");
        assert_eq!(eff[1].step_b, "v3");
    }

    #[test]
    fn effective_diffs_falls_back_to_legacy_diff_field_when_diffs_empty() {
        // History records or older serialized blocks may have only the
        // legacy `diff` field. The helper wraps it in a single-element
        // vec so callers get a uniform shape.
        let block = TestBlock {
            block_type: "test".to_string(),
            name: "n".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: None,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: true,
            steps: Vec::new(),
            diff: Some(DiffDirective {
                step_a: "v1".to_string(),
                step_b: "v2".to_string(),
            }),
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };
        let eff = block.effective_diffs();
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].step_a, "v1");
        assert_eq!(eff[0].step_b, "v2");
    }

    #[test]
    fn effective_diffs_returns_empty_when_no_diff_at_all() {
        let block = TestBlock {
            block_type: "test".to_string(),
            name: "n".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: None,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: false,
            steps: Vec::new(),
            diff: None,
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };
        assert_eq!(block.effective_diffs().len(), 0);
    }

    // ── Duplicate step names produce parse error ────────────────────
    #[test]
    fn duplicate_step_names_produce_error() {
        let input = "\
### @test Duplicate steps
# @compare
# @step baseline
GET https://example.com/v1
# @step baseline
GET https://example.com/v2
# @diff baseline baseline
# @assert $diff.match == true";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let blk = &suite.blocks[0];
        assert!(
            blk.errors.iter().any(|e| e.contains("duplicate step name") && e.contains("baseline")),
            "expected duplicate step name error, got: {:?}",
            blk.errors
        );
    }

    // ── @diff referencing non-existent steps produces parse error ───
    #[test]
    fn diff_references_nonexistent_steps_produce_error() {
        let input = "\
### @test Bad diff ref
# @compare
# @step step_a
GET https://example.com/a
# @step step_b
GET https://example.com/b
# @diff step_a typo_step
# @assert $diff.match == true";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let blk = &suite.blocks[0];
        assert!(
            blk.errors.iter().any(|e| e.contains("unknown step") && e.contains("typo_step")),
            "expected unknown step error for typo_step, got: {:?}",
            blk.errors
        );
        // step_a is valid, so no error for it
        assert!(
            !blk.errors.iter().any(|e| e.contains("step_a")),
            "step_a is valid and should not appear in errors"
        );
    }

    // ── @diff where both references are invalid ─────────────────────
    #[test]
    fn diff_both_refs_invalid_produce_two_errors() {
        let input = "\
### @test Both bad
# @compare
# @step real_a
GET https://example.com/a
# @step real_b
GET https://example.com/b
# @diff ghost_a ghost_b";
        let suite = parse_test_suite(input);
        let blk = &suite.blocks[0];
        let diff_errors: Vec<_> = blk.errors.iter().filter(|e| e.contains("unknown step")).collect();
        assert_eq!(diff_errors.len(), 2, "expected two unknown step errors, got: {:?}", blk.errors);
    }

    // ── Valid compare block has no errors ────────────────────────────
    #[test]
    fn valid_compare_block_no_errors() {
        let input = "\
### @test Valid compare
# @compare
# @step baseline
GET https://example.com/v1
# @step candidate
GET https://example.com/v2
# @diff baseline candidate
# @assert $diff.match == true";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        assert!(suite.blocks[0].errors.is_empty(), "valid block should have no errors");
    }

    // ── Compare block edge cases ────────────────────────────────────

    #[test]
    fn compare_empty_steps() {
        // @compare with no @step directives — block still parses, steps is empty
        let input = "\
### @test Empty compare
# @compare
GET https://example.com/api
# @assert status == 200";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let blk = &suite.blocks[0];
        assert!(blk.compare);
        assert!(blk.steps.is_empty(), "no @step directives means empty steps vec");
        // The block-level request should still be parsed normally
        assert_eq!(blk.request.method, "GET");
    }

    #[test]
    fn compare_single_step() {
        let input = "\
### @test Single step compare
# @compare
# @step only
GET https://example.com/v1
# @assert status == 200";
        let suite = parse_test_suite(input);
        let blk = &suite.blocks[0];
        assert!(blk.compare);
        assert_eq!(blk.steps.len(), 1);
        assert_eq!(blk.steps[0].name, "only");
        assert!(blk.diff.is_none(), "no @diff directive for a single step");
    }

    #[test]
    fn compare_step_name_with_spaces() {
        let input = "\
### @test Spaced steps
# @compare
# @step production api
GET https://prod.example.com/users
# @step staging api
GET https://staging.example.com/users
# @diff production api staging api";
        let suite = parse_test_suite(input);
        let blk = &suite.blocks[0];
        assert_eq!(blk.steps.len(), 2);
        assert_eq!(blk.steps[0].name, "production api");
        assert_eq!(blk.steps[1].name, "staging api");
        let diff = blk.diff.as_ref().unwrap();
        assert_eq!(diff.step_a, "production");
        assert_eq!(diff.step_b, "api");
    }

    #[test]
    fn compare_diff_references_same_step() {
        let input = "\
### @test Self diff
# @compare
# @step alpha
GET https://example.com/v1
# @diff alpha alpha";
        let suite = parse_test_suite(input);
        let blk = &suite.blocks[0];
        assert!(blk.diff.is_some());
        let diff = blk.diff.as_ref().unwrap();
        assert_eq!(diff.step_a, "alpha");
        assert_eq!(diff.step_b, "alpha");
        assert!(blk.errors.is_empty(), "self-diff is syntactically valid");
    }

    #[test]
    fn compare_step_with_body() {
        let input = r#"
### @test POST steps
# @compare
# @step first
POST https://api.example.com/query
Content-Type: application/json

{"query": "test", "limit": 10}

# @assert status == 200

# @step second
POST https://api.example.com/query-v2
Content-Type: application/json

{"query": "test", "limit": 10}

# @assert status == 200

# @diff first second
"#;
        let suite = parse_test_suite(input);
        let blk = &suite.blocks[0];
        assert_eq!(blk.steps.len(), 2);
        assert_eq!(blk.steps[0].request.method, "POST");
        assert!(
            blk.steps[0].request.body.is_some(),
            "first step should have a body"
        );
        assert!(
            blk.steps[0].request.body.as_ref().unwrap().contains("\"query\""),
            "first step body should contain query field"
        );
        assert!(
            blk.steps[1].request.body.is_some(),
            "second step should have a body"
        );
    }

    #[test]
    fn compare_assertions_mixed_with_diff() {
        let input = "\
### @test Mixed assertions
# @compare
# @step baseline
GET https://api.example.com/v1
# @assert status == 200
# @step candidate
GET https://api.example.com/v2
# @assert status == 200
# @assert $.name != null
# @diff baseline candidate
# @assert $diff.match == true
# @assert $diff.changed_count == 0";
        let suite = parse_test_suite(input);
        let blk = &suite.blocks[0];
        // Step-level assertions (non $diff.*)
        assert_eq!(blk.steps[0].assertions.len(), 1, "baseline has 1 step assertion");
        assert_eq!(blk.steps[1].assertions.len(), 2, "candidate has 2 step assertions");
        // Block-level assertions ($diff.*)
        assert_eq!(blk.assertions.len(), 2, "two block-level diff assertions");
        assert!(blk.assertions[0].left.starts_with("$diff."));
        assert!(blk.assertions[1].left.starts_with("$diff."));
    }

    #[test]
    fn generate_roundtrip_compare_block() {
        let input = r#"
### @test Roundtrip Test
# @description Verifies parse-generate-parse identity
# @compare
# @step baseline
GET https://api.example.com/v1
Authorization: Bearer {{token}}

# @extract v1_id = $.id
# @assert status == 200

# @step candidate
POST https://api.example.com/v2
Content-Type: application/json

{"query": "test"}

# @extract v2_id = $.id
# @assert status == 201

# @diff baseline candidate
# @assert $diff.match == true
# @assert $diff.changed_count == 0
"#;
        let suite1 = parse_test_suite(input);
        let generated = generate_http_content(&suite1);
        let suite2 = parse_test_suite(&generated);

        assert_eq!(suite1.blocks.len(), suite2.blocks.len());
        let b1 = &suite1.blocks[0];
        let b2 = &suite2.blocks[0];
        assert_eq!(b1.compare, b2.compare);
        assert_eq!(b1.steps.len(), b2.steps.len());
        for (s1, s2) in b1.steps.iter().zip(b2.steps.iter()) {
            assert_eq!(s1.name, s2.name, "step names must match");
            assert_eq!(s1.request.method, s2.request.method, "step methods must match");
            assert_eq!(s1.request.url, s2.request.url, "step URLs must match");
            assert_eq!(s1.assertions.len(), s2.assertions.len(), "step assertion counts must match for {}", s1.name);
            assert_eq!(s1.extracts.len(), s2.extracts.len(), "step extract counts must match for {}", s1.name);
        }
        assert_eq!(b1.assertions.len(), b2.assertions.len(), "block-level assertion count must match");
        assert_eq!(b1.diff.is_some(), b2.diff.is_some(), "diff directive presence must match");
        if let (Some(d1), Some(d2)) = (&b1.diff, &b2.diff) {
            assert_eq!(d1.step_a, d2.step_a);
            assert_eq!(d1.step_b, d2.step_b);
        }
    }

    // ── Auto-run directive tests ─────────────────────────────────────

    #[test]
    fn parse_auto_run_in_variables_block() {
        let input = "\
# @auto_run 15m
@variables
base_url = https://example.com

### @test Health
GET {{base_url}}/health
# @assert status == 200";
        let suite = parse_test_suite(input);
        assert_eq!(suite.auto_run, Some("15m".to_string()));
        assert_eq!(suite.variables.len(), 1);
        assert_eq!(suite.blocks.len(), 1);
    }

    #[test]
    fn parse_auto_run_in_comment_header() {
        let input = "\
# ============================================================
# Health Check — E2E Tests
# @auto_run 1h
# ============================================================

### @test Health
GET https://example.com/health
# @assert status == 200";
        let suite = parse_test_suite(input);
        assert_eq!(suite.auto_run, Some("1h".to_string()));
    }

    #[test]
    fn parse_auto_run_with_all_units() {
        for (interval, _) in &[("30s", 30), ("5m", 300), ("2h", 7200), ("1d", 86400)] {
            let input = format!("# @auto_run {}\n@variables\nx = 1\n\n### @test T\nGET http://x", interval);
            let suite = parse_test_suite(&input);
            assert_eq!(suite.auto_run, Some(interval.to_string()));
        }
    }

    #[test]
    fn parse_auto_run_missing() {
        let input = "\
@variables
base_url = https://example.com

### @test Health
GET {{base_url}}/health";
        let suite = parse_test_suite(input);
        assert_eq!(suite.auto_run, None);
    }

    #[test]
    fn parse_auto_run_invalid_ignored() {
        let input = "\
# @auto_run invalid
@variables
x = 1

### @test T
GET http://x";
        let suite = parse_test_suite(input);
        assert_eq!(suite.auto_run, None);
    }

    #[test]
    fn generate_preserves_auto_run() {
        let input = "\
# @auto_run 15m
@variables
base_url = https://example.com

### @test Health
GET {{base_url}}/health
# @assert status == 200";
        let suite = parse_test_suite(input);
        assert_eq!(suite.auto_run, Some("15m".to_string()));

        let output = generate_http_content(&suite);
        assert!(output.starts_with("# @@auto_run 15m\n"));

        // Round-trip: re-parse should preserve auto_run
        let suite2 = parse_test_suite(&output);
        assert_eq!(suite2.auto_run, Some("15m".to_string()));
    }

    #[test]
    fn generate_without_auto_run() {
        let input = "\
@variables
x = 1

### @test T
GET http://x";
        let suite = parse_test_suite(input);
        let output = generate_http_content(&suite);
        assert!(!output.contains("@auto_run"));
    }

    // ─────────────────────────────────────────────────────────────────
    // `# @@request-id` directive
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_request_id_file_default_header() {
        let input = "\
# @@request-id

### @test T
GET http://x";
        let suite = parse_test_suite(input);
        assert_eq!(
            suite.request_id_header.as_deref(),
            Some(DEFAULT_REQUEST_ID_HEADER)
        );
    }

    #[test]
    fn parse_request_id_file_custom_header() {
        let input = "\
# @@request-id X-Correlation-ID

### @test T
GET http://x";
        let suite = parse_test_suite(input);
        assert_eq!(
            suite.request_id_header.as_deref(),
            Some("X-Correlation-ID")
        );
    }

    #[test]
    fn parse_request_id_legacy_single_at_and_underscore_alias() {
        // legacy single-`@` + underscore-separated name alias
        let input = "\
# @request_id X-My-Id

### @test T
GET http://x";
        let suite = parse_test_suite(input);
        assert_eq!(suite.request_id_header.as_deref(), Some("X-My-Id"));
    }

    #[test]
    fn parse_request_id_block_override() {
        let input = "\
# @@request-id X-Global

### @test A
GET http://x

### @test B
# @@request-id X-Local
GET http://x

### @test C
# @@request-id off
GET http://x";
        let suite = parse_test_suite(input);
        assert_eq!(suite.request_id_header.as_deref(), Some("X-Global"));
        assert_eq!(suite.blocks[0].request_id_header, None);
        assert!(!suite.blocks[0].request_id_disabled);
        assert_eq!(
            suite.blocks[1].request_id_header.as_deref(),
            Some("X-Local")
        );
        assert!(suite.blocks[2].request_id_disabled);
    }

    #[test]
    fn generate_request_id_round_trip() {
        let input = "\
# @@request-id X-Correlation-ID

### @test A
GET http://x

### @test B
# @@request-id off
GET http://x";
        let suite = parse_test_suite(input);
        let output = generate_http_content(&suite);
        assert!(output.contains("# @@request-id X-Correlation-ID\n"));
        assert!(output.contains("# @@request-id off\n"));

        let suite2 = parse_test_suite(&output);
        assert_eq!(
            suite2.request_id_header.as_deref(),
            Some("X-Correlation-ID")
        );
        assert!(suite2.blocks[1].request_id_disabled);
    }

    #[test]
    fn effective_request_id_header_resolution() {
        let input = "\
# @@request-id X-Global

### @test A
GET http://x

### @test B
# @@request-id X-Local
GET http://x

### @test C
# @@request-id off
GET http://x";
        let suite = parse_test_suite(input);
        assert_eq!(
            effective_request_id_header(&suite, &suite.blocks[0]).as_deref(),
            Some("X-Global")
        );
        assert_eq!(
            effective_request_id_header(&suite, &suite.blocks[1]).as_deref(),
            Some("X-Local")
        );
        assert_eq!(
            effective_request_id_header(&suite, &suite.blocks[2]),
            None
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // NEW SYNTAX (`# @@directive`, top-level `@var`, `### @@type`)
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_top_level_var_defs() {
        let input = "\
@base_url = https://api.example.com
@token = abc123

### @@test Get Users
GET {{base_url}}/users
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.variables.len(), 2);
        assert_eq!(suite.variables[0], ("base_url".into(), "https://api.example.com".into()));
        assert_eq!(suite.variables[1], ("token".into(), "abc123".into()));
        assert_eq!(suite.blocks.len(), 1);
        assert_eq!(suite.blocks[0].block_type, "test");
        assert_eq!(suite.blocks[0].name, "Get Users");
    }

    #[test]
    fn parse_double_at_directives() {
        let input = "\
### @@test Get Users
# @@description Fetch all users
# @@group users
# @@depends setup-token
GET https://api.example.com/users

# @@assert status == 200
# @@extract user_count = $.length
";
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        assert_eq!(block.description, "Fetch all users");
        assert_eq!(block.group.as_deref(), Some("users"));
        assert_eq!(block.depends, vec!["setup-token"]);
        assert_eq!(block.assertions.len(), 1);
        assert_eq!(block.extracts[0].variable_name, "user_count");
    }

    #[test]
    fn parse_mixed_legacy_and_new_directives() {
        // A file mixing `# @x` and `# @@x` should parse cleanly.
        let input = "\
@host = https://api.example.com

### @setup Login
# @description Old-style legacy
POST {{host}}/login

# @assert status == 200

### @@test Verify
# @@description New-style
GET {{host}}/me

# @@assert status == 200
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.variables[0].0, "host");
        assert_eq!(suite.blocks.len(), 2);
        assert_eq!(suite.blocks[0].description, "Old-style legacy");
        assert_eq!(suite.blocks[1].description, "New-style");
    }

    #[test]
    fn parse_bare_rest_client_directives() {
        // REST Client native: `@name`, `@description`, `@note` (bare, no `#`).
        let input = "\
### Get Users
@name fetchUsers
@description List all users from API
GET https://api.example.com/users
";
        let suite = parse_test_suite(input);
        let req = &suite.blocks[0].request;
        assert_eq!(req.name.as_deref(), Some("fetchUsers"));
        assert_eq!(suite.blocks[0].description, "List all users from API");
    }

    #[test]
    fn bare_directive_disambiguates_from_var_def() {
        // `@name = value` is a variable; `@name value` is a directive.
        let input = "\
@name = my-api-host

### @@test First
@name actuallyADirective
GET https://api.example.com/x
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.variables[0], ("name".into(), "my-api-host".into()));
        assert_eq!(
            suite.blocks[0].request.name.as_deref(),
            Some("actuallyADirective")
        );
    }

    #[test]
    fn parse_dash_separator() {
        // REST Client also accepts `---` as a request separator.
        let input = "\
GET https://api.example.com/a
---
GET https://api.example.com/b
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 2);
        assert_eq!(suite.blocks[0].request.url, "https://api.example.com/a");
        assert_eq!(suite.blocks[1].request.url, "https://api.example.com/b");
    }

    #[test]
    fn parse_double_slash_comments_for_directives() {
        let input = "\
### @@test x
// @@description Slash-style comment directive
// @@assert status == 200
GET https://api.example.com/x
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].description, "Slash-style comment directive");
        assert_eq!(suite.blocks[0].assertions.len(), 1);
    }

    #[test]
    fn line_anchored_separator_does_not_split_inside_body() {
        // A `###` inside a request body should NOT split the block.
        let input = "\
### @@test Posts a body containing hashes
POST https://api.example.com/notes
Content-Type: text/plain

# Title
###Subheading
Body text
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks.len(), 1);
        let body = suite.blocks[0].request.body.as_deref().unwrap_or("");
        assert!(body.contains("###Subheading"), "body should retain '###Subheading', got: {:?}", body);
    }

    #[test]
    fn generator_writes_new_syntax() {
        let input = "\
@host = https://api.example.com

### @setup Get Token
# @description Legacy directive
POST {{host}}/auth

# @extract token = $.access_token
# @assert status == 200
";
        let suite = parse_test_suite(input);
        let regenerated = generate_http_content(&suite);
        // New syntax: top-level @var, ### @@setup, # @@description, etc.
        assert!(regenerated.contains("@host = https://api.example.com"));
        assert!(regenerated.contains("### @@setup Get Token"));
        assert!(regenerated.contains("# @@description Legacy directive"));
        assert!(regenerated.contains("# @@extract token = $.access_token"));
        assert!(regenerated.contains("# @@assert status == 200"));
        // No legacy `@variables` block, no single-@ directives in output.
        assert!(!regenerated.contains("@variables\n"));
        assert!(!regenerated.contains("# @description"));
        assert!(!regenerated.contains("# @assert"));
    }

    #[test]
    fn full_roundtrip_new_syntax() {
        let input = "\
@host = https://api.example.com
@token =

### @@setup Get Token
# @@description Fetch OAuth token
POST {{host}}/auth
Content-Type: application/json

{\"client\":\"test\"}

# @@extract token = $.access_token
# @@assert status == 200

### @@test Authenticated Get
# @@group reads
GET {{host}}/me
Authorization: Bearer {{token}}

# @@assert status == 200
# @@assert $.id != null
";
        let s1 = parse_test_suite(input);
        let regen = generate_http_content(&s1);
        let s2 = parse_test_suite(&regen);
        // Idempotent: round-trip the regenerated content.
        assert_eq!(generate_http_content(&s2), regen);
        assert_eq!(s1.blocks.len(), 2);
        assert_eq!(s1.variables.len(), 2);
    }

    // ── @@redact body directive parsing ─────────────────────────────

    #[test]
    fn parses_file_level_redact_jsonpath_directive() {
        let input = "\
@host = https://api.example.com
# @@redact body $.password,$.token

### @@test Get
GET {{host}}/me
";
        let suite = parse_test_suite(input);
        assert_eq!(
            suite.redact_body_rules,
            vec![
                BodyRedactRule::JsonPath("$.password".to_string()),
                BodyRedactRule::JsonPath("$.token".to_string()),
            ]
        );
        // Block-level rules unaffected
        assert!(suite.blocks[0].redact_body_rules.is_empty());
    }

    #[test]
    fn parses_block_level_redact_directive() {
        let input = "\
### @@test Login
# @@redact body $.user.ssn,$.user.dob
POST https://api.example.com/login
";
        let suite = parse_test_suite(input);
        assert!(suite.redact_body_rules.is_empty());
        assert_eq!(
            suite.blocks[0].redact_body_rules,
            vec![
                BodyRedactRule::JsonPath("$.user.ssn".to_string()),
                BodyRedactRule::JsonPath("$.user.dob".to_string()),
            ]
        );
    }

    #[test]
    fn parses_redact_regex_directive() {
        let input = "\
# @@redact body /Bearer\\s+[A-Za-z0-9._-]+/

### @@test Call
GET https://api.example.com/x
# @@redact body /sk-[A-Za-z0-9]+/
";
        let suite = parse_test_suite(input);
        assert_eq!(
            suite.redact_body_rules,
            vec![BodyRedactRule::Regex("Bearer\\s+[A-Za-z0-9._-]+".to_string())]
        );
        assert_eq!(
            suite.blocks[0].redact_body_rules,
            vec![BodyRedactRule::Regex("sk-[A-Za-z0-9]+".to_string())]
        );
    }

    #[test]
    fn multiple_redact_directives_accumulate() {
        let input = "\
# @@redact body $.password
# @@redact body $.token,$.refresh_token
# @@redact body /api_key=\\S+/

### @@test Multi
GET https://api.example.com/x
# @@redact body $.local_secret
# @@redact body /Bearer\\s+\\S+/
";
        let suite = parse_test_suite(input);
        assert_eq!(
            suite.redact_body_rules,
            vec![
                BodyRedactRule::JsonPath("$.password".to_string()),
                BodyRedactRule::JsonPath("$.token".to_string()),
                BodyRedactRule::JsonPath("$.refresh_token".to_string()),
                BodyRedactRule::Regex("api_key=\\S+".to_string()),
            ]
        );
        assert_eq!(
            suite.blocks[0].redact_body_rules,
            vec![
                BodyRedactRule::JsonPath("$.local_secret".to_string()),
                BodyRedactRule::Regex("Bearer\\s+\\S+".to_string()),
            ]
        );
    }

    #[test]
    fn non_redact_at_directives_unchanged() {
        let input = "\
@host = https://api.example.com
# @@auto_run 15m
# @@request-id X-Correlation-ID

### @@test Things
# @@description Test description
# @@group reads
# @@depends seed-data
GET {{host}}/items
# @@assert status == 200
# @@extract first_id = $.items[0].id
";
        let suite = parse_test_suite(input);
        assert_eq!(suite.auto_run.as_deref(), Some("15m"));
        assert_eq!(suite.request_id_header.as_deref(), Some("X-Correlation-ID"));
        assert!(suite.redact_body_rules.is_empty());
        let b = &suite.blocks[0];
        assert_eq!(b.description, "Test description");
        assert_eq!(b.group.as_deref(), Some("reads"));
        assert_eq!(b.depends, vec!["seed-data".to_string()]);
        assert_eq!(b.assertions.len(), 1);
        assert_eq!(b.extracts.len(), 1);
        assert!(b.redact_body_rules.is_empty());
    }

    // ── Repeater (`# @@for var in src`) parser tests ──────────────────

    #[test]
    fn parse_for_directive_basic() {
        let input = "### @test Validate each user\n# @for user_id in user_ids\nGET https://api.example.com/users/{{user_id}}\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        assert!(block.for_loop.is_some(), "block should carry for_loop");
        let fl = block.for_loop.as_ref().unwrap();
        assert_eq!(fl.iter_var, "user_id");
        assert_eq!(fl.source_var, "user_ids");
        // Errors should be empty for a well-formed directive.
        assert!(block.errors.is_empty(), "no errors expected, got: {:?}", block.errors);
    }

    #[test]
    fn for_directive_double_at_form_also_works() {
        // The canonical NEW syntax uses `# @@for ...` (double-at).
        let input = "### @@test Validate\n# @@for x in xs\nGET https://api.example.com/{{x}}\n";
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop missing");
        assert_eq!(fl.iter_var, "x");
        assert_eq!(fl.source_var, "xs");
    }

    #[test]
    fn for_directive_roundtrips_through_generator() {
        let original = "### @@test Each user\n# @@for uid in user_ids\nGET https://api.example.com/users/{{uid}}\n\n# @@assert status == 200\n";
        let suite = parse_test_suite(original);
        let regenerated = generate_http_content(&suite);
        assert!(
            regenerated.contains("# @@for uid in user_ids"),
            "expected # @@for line in regenerated output, got:\n{}",
            regenerated
        );
        // And re-parsing the regenerated output preserves the loop.
        let suite2 = parse_test_suite(&regenerated);
        let fl = suite2.blocks[0].for_loop.as_ref().expect("for_loop lost on re-parse");
        assert_eq!(fl.iter_var, "uid");
        assert_eq!(fl.source_var, "user_ids");
    }

    #[test]
    fn for_with_compare_is_rejected_in_v1() {
        // Mixing `# @@for` and `# @@compare` on the same block is an error
        // in V1; the loop is dropped and a parse error is recorded so the
        // UI can surface it.
        let input = "### @test Conflict\n# @compare\n# @for x in xs\n# @step a\nGET https://api.example.com/a\n";
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        assert!(block.compare, "compare flag should still be set");
        assert!(block.for_loop.is_none(), "for_loop should be cleared on conflict");
        assert!(
            block.errors.iter().any(|e| e.contains("@@compare") && e.contains("@@for")),
            "expected validation error for compare+for conflict, got: {:?}",
            block.errors
        );
    }

    #[test]
    fn duplicate_for_directive_keeps_first_and_errors() {
        let input = "### @test Dup\n# @for a in xs\n# @for b in ys\nGET https://api.example.com/x\n";
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        let fl = block.for_loop.as_ref().expect("first for_loop should win");
        assert_eq!(fl.iter_var, "a");
        assert_eq!(fl.source_var, "xs");
        assert!(
            block.errors.iter().any(|e| e.contains("duplicate") && e.contains("@@for")),
            "expected duplicate-directive error, got: {:?}",
            block.errors
        );
    }

    #[test]
    fn malformed_for_directive_is_silently_dropped_with_error() {
        // `# @@for foo` (no `in src_var`) — invalid; we drop the directive
        // and record a parse error.
        let input = "### @test Bad\n# @for missing_in_keyword\nGET https://api.example.com/x\n";
        let suite = parse_test_suite(input);
        let block = &suite.blocks[0];
        assert!(block.for_loop.is_none(), "malformed for should be dropped");
        assert!(
            block.errors.iter().any(|e| e.contains("invalid") && e.contains("@@for")),
            "expected invalid-directive error, got: {:?}",
            block.errors
        );
    }

    #[test]
    fn block_without_for_directive_has_none() {
        // Sanity check: a normal block leaves for_loop as None.
        let input = "### @test Normal\nGET https://api.example.com/items\n";
        let suite = parse_test_suite(input);
        assert!(suite.blocks[0].for_loop.is_none());
    }

    // ──────────────────────────────────────────────────────────────────
    // V1.2: # @@parallel <N>
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn parallel_directive_basic_attaches_to_for_loop() {
        let input = r#"### @test Parallel basic
# @@for x in xs
# @@parallel 4
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert_eq!(fl.iter_var, "x");
        assert_eq!(fl.source_var, "xs");
        assert_eq!(fl.parallel, Some(4));
    }

    #[test]
    fn parallel_directive_bare_defaults_to_four() {
        let input = r#"### @test Bare parallel
# @@for x in xs
# @@parallel
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert_eq!(fl.parallel, Some(4));
    }

    #[test]
    fn parallel_directive_clamps_above_thirtytwo() {
        let input = r#"### @test Clamp high
# @@for x in xs
# @@parallel 100
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert_eq!(fl.parallel, Some(32), "values > 32 clamp to 32");
    }

    #[test]
    fn parallel_directive_zero_becomes_one_with_warning() {
        let input = r#"### @test Zero
# @@for x in xs
# @@parallel 0
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert_eq!(fl.parallel, Some(1));
        assert!(
            suite.blocks[0].errors.iter().any(|e| e.contains("parallel") && e.contains("0")),
            "should warn about value 0, got errors: {:?}",
            suite.blocks[0].errors
        );
    }

    #[test]
    fn parallel_directive_negative_is_dropped_with_warning() {
        let input = r#"### @test Negative
# @@for x in xs
# @@parallel -3
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        // Negative values clamp to 1 (sequential) — they're nonsensical as
        // concurrency counts, but treating them as "user wanted sequential"
        // is friendlier than silently dropping.
        assert_eq!(fl.parallel, Some(1));
        assert!(
            suite.blocks[0].errors.iter().any(|e| e.contains("parallel") && e.contains("-3")),
            "should warn about negative value"
        );
    }

    #[test]
    fn parallel_directive_non_numeric_is_dropped_with_warning() {
        let input = r#"### @test Garbage
# @@for x in xs
# @@parallel banana
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert!(fl.parallel.is_none(), "non-numeric drops directive");
        assert!(
            suite.blocks[0].errors.iter().any(|e| e.contains("parallel")),
            "should warn about non-numeric value"
        );
    }

    #[test]
    fn parallel_directive_before_for_is_order_insensitive() {
        let input = r#"### @test Order swap
# @@parallel 6
# @@for x in xs
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert_eq!(fl.parallel, Some(6));
    }

    #[test]
    fn parallel_without_for_warns_and_drops() {
        let input = r#"### @test Orphan parallel
# @@parallel 4
GET https://api.example.com/items
"#;
        let suite = parse_test_suite(input);
        assert!(suite.blocks[0].for_loop.is_none());
        assert!(
            suite.blocks[0].errors.iter().any(|e|
                e.contains("parallel") && e.contains("requires a `# @@for`")
            ),
            "expected warning about parallel-without-for, got errors: {:?}",
            suite.blocks[0].errors
        );
    }

    #[test]
    fn duplicate_parallel_keeps_first_with_warning() {
        let input = r#"### @test Dup
# @@for x in xs
# @@parallel 4
# @@parallel 8
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        let fl = suite.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert_eq!(fl.parallel, Some(4), "first @@parallel wins");
        assert!(
            suite.blocks[0].errors.iter().any(|e| e.contains("duplicate") && e.contains("parallel")),
            "expected duplicate warning"
        );
    }

    #[test]
    fn parallel_directive_roundtrips_through_generator() {
        let input = "### @test RT\n# @@for x in xs\n# @@parallel 8\nGET https://api.example.com/{{x}}\n";
        let suite = parse_test_suite(input);
        let regenerated = generate_http_content(&suite);
        assert!(
            regenerated.contains("# @@for x in xs"),
            "regen missing @@for: {}", regenerated
        );
        assert!(
            regenerated.contains("# @@parallel 8"),
            "regen missing @@parallel 8: {}", regenerated
        );
        // Round-trip again — second pass should match first.
        let suite2 = parse_test_suite(&regenerated);
        let fl = suite2.blocks[0].for_loop.as_ref().expect("for_loop set");
        assert_eq!(fl.parallel, Some(8));
    }

    #[test]
    fn parallel_one_does_not_emit_directive() {
        // `# @@parallel 1` is equivalent to no directive, so the regenerator
        // omits it to avoid noise in code-mode round-trips.
        let input = "### @test One\n# @@for x in xs\n# @@parallel 1\nGET https://api.example.com/{{x}}\n";
        let suite = parse_test_suite(input);
        let regenerated = generate_http_content(&suite);
        assert!(
            !regenerated.contains("# @@parallel"),
            "parallel 1 should not be emitted, got: {}", regenerated
        );
    }

    #[test]
    fn parallel_with_compare_conflict_drops_both() {
        // @@for + @@compare is rejected; @@parallel was attached to that
        // for_loop, so it should be dropped along with it.
        let input = r#"### @test Conflict
# @@compare
# @@for x in xs
# @@parallel 4
GET https://api.example.com/{{x}}
"#;
        let suite = parse_test_suite(input);
        assert!(suite.blocks[0].for_loop.is_none(), "for_loop dropped");
        // Sanity: error about for+compare conflict was raised.
        assert!(
            suite.blocks[0].errors.iter().any(|e| e.contains("@@for") && e.contains("@@compare")),
            "expected for+compare conflict error"
        );
    }
}


