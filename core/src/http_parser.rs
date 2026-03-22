use serde::{Deserialize, Serialize};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_token: Option<String>,
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

/// Parse enhanced .http content into a TestSuite.
pub fn parse_test_suite(content: &str) -> TestSuite {
    let mut variables = Vec::new();
    let mut blocks = Vec::new();
    let mut telemetry_var: Option<String> = None;
    let mut telemetry_service: Option<String> = None;
    let mut telemetry_token: Option<String> = None;

    let raw_blocks: Vec<&str> = content.split("###").collect();

    for (chunk_idx, raw_block) in raw_blocks.iter().enumerate() {
        let block = raw_block.trim();
        if block.is_empty() {
            continue;
        }

        // The first chunk (before any ###) may contain file-level directives
        if chunk_idx == 0 {
            for line in block.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("# @telemetry_token ") {
                    telemetry_token = Some(rest.trim().to_string());
                } else if let Some(rest) = trimmed.strip_prefix("# @telemetry_service ") {
                    telemetry_service = Some(rest.trim().to_string());
                } else if let Some(rest) = trimmed.strip_prefix("# @telemetry ") {
                    telemetry_var = Some(rest.trim().to_string());
                }
            }
        }

        // Check if block contains @variables anywhere (not just first line)
        // This handles files with comment headers before the @variables block
        let has_variables = block
            .lines()
            .any(|l| l.trim().starts_with("@variables"));

        if has_variables {
            parse_variables_block(block, &mut variables);
            continue;
        }

        if let Some(test_block) = parse_test_block(block) {
            blocks.push(test_block);
        }
    }

    TestSuite {
        variables,
        blocks,
        telemetry_var,
        telemetry_service,
        telemetry_token,
    }
}

fn parse_variables_block(block: &str, variables: &mut Vec<(String, String)>) {
    let mut past_header = false;
    for line in block.lines() {
        let trimmed = line.trim();
        if !past_header {
            if trimmed.starts_with("@variables") {
                past_header = true;
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Support both "@name = value" and "name = value" formats
        let var_line = trimmed.strip_prefix('@').unwrap_or(trimmed);
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
    let mut assertions = Vec::new();
    let mut extracts = Vec::new();
    let mut request_name: Option<String> = None;
    let mut request_lines: Vec<&str> = Vec::new();
    let mut first_meaningful = true;

    for line in block.lines() {
        let trimmed = line.trim();

        // First non-empty line: check for block type annotation
        if first_meaningful && !trimmed.is_empty() {
            first_meaningful = false;

            if let Some(rest) = trimmed.strip_prefix("@setup") {
                block_type = "setup".to_string();
                block_name = rest.trim().to_string();
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("@test") {
                block_type = "test".to_string();
                block_name = rest.trim().to_string();
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("@teardown") {
                block_type = "teardown".to_string();
                block_name = rest.trim().to_string();
                continue;
            }
        }

        // Directive comments
        if let Some(rest) = trimmed.strip_prefix("# @description ") {
            description = rest.trim().to_string();
            continue;
        }
        if trimmed == "# @disabled" {
            disabled = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# @mode ") {
            let m = rest.trim().to_lowercase();
            if m == "app" || m == "dev" {
                mode = Some(m);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# @dev_auth ") {
            let scope = rest.trim().to_string();
            if !scope.is_empty() {
                dev_auth = Some(scope);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# @group ") {
            let g = rest.trim().to_string();
            if !g.is_empty() {
                group = Some(g);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# @depends ") {
            let dep = rest.trim().to_string();
            if !dep.is_empty() {
                depends.push(dep);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# @assert ") {
            if let Some(assertion) = parse_assertion_directive(rest.trim()) {
                assertions.push(assertion);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# @extract ") {
            if let Some(extract) = parse_extract_directive(rest.trim()) {
                extracts.push(extract);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# @name ") {
            request_name = Some(rest.trim().to_string());
            continue;
        }

        request_lines.push(line);
    }

    let request = parse_request_from_lines(&request_lines, &request_name)?;

    if block_name.is_empty() {
        if let Some(ref n) = request.name {
            block_name = n.clone();
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
pub fn generate_http_content(suite: &TestSuite) -> String {
    let mut output = String::new();

    // File-level telemetry directives (before @variables)
    if let Some(ref tv) = suite.telemetry_var {
        output.push_str(&format!("# @telemetry {}\n", tv));
    }
    if let Some(ref ts) = suite.telemetry_service {
        output.push_str(&format!("# @telemetry_service {}\n", ts));
    }
    if let Some(ref tt) = suite.telemetry_token {
        output.push_str(&format!("# @telemetry_token {}\n", tt));
    }
    if suite.telemetry_var.is_some() || suite.telemetry_service.is_some() || suite.telemetry_token.is_some() {
        output.push('\n');
    }

    // Variables block
    if !suite.variables.is_empty() {
        output.push_str("@variables\n");
        for (name, value) in &suite.variables {
            output.push_str(&format!("{} = {}\n", name, value));
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
                output.push_str(&format!("# @name {}\n", block.name));
            }
        } else if block.name.is_empty() {
            output.push_str(&format!("### @{}\n", block.block_type));
        } else {
            output.push_str(&format!("### @{} {}\n", block.block_type, block.name));
        }

        // Description directive
        if !block.description.is_empty() {
            output.push_str(&format!("# @description {}\n", block.description));
        }

        // Disabled directive
        if block.disabled {
            output.push_str("# @disabled\n");
        }

        // Mode directive
        if let Some(ref m) = block.mode {
            output.push_str(&format!("# @mode {}\n", m));
        }

        // Dev auth scope directive
        if let Some(ref scope) = block.dev_auth {
            output.push_str(&format!("# @dev_auth {}\n", scope));
        }

        // Group directive
        if let Some(ref g) = block.group {
            output.push_str(&format!("# @group {}\n", g));
        }

        // Depends directives
        for dep in &block.depends {
            output.push_str(&format!("# @depends {}\n", dep));
        }

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
                "# @extract {} = {}\n",
                extract.variable_name, extract.source_path
            ));
        }

        // Assertion directives
        for assertion in &block.assertions {
            output.push_str(&format!(
                "# @assert {} {} {}\n",
                assertion.left, assertion.operator, assertion.right
            ));
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
        assert!(output.contains("@variables"));
        assert!(output.contains("baseUrl = https://api.example.com"));
        assert!(output.contains("token = abc123"));
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
            }],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("###"));
        assert!(output.contains("GET https://example.com/api"));
        assert!(!output.contains("# @name"));
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
            }],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("# @name Create User"));
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
            }],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("### @test Check Users"));
        assert!(output.contains("# @extract userId = response.body.0.id"));
        assert!(output.contains("# @assert response.status == 200"));
        assert!(output.contains("# @assert response.body.length > 0"));
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
                },
            ],
            ..Default::default()
        };
        let output = generate_http_content(&suite);
        assert!(output.contains("@variables"));
        assert!(output.contains("### @setup Login"));
        assert!(output.contains("### @test List Users"));
        assert!(output.contains("### @teardown Cleanup"));
        assert!(output.contains("# @extract token = response.body.token"));
        assert!(output.contains("# @assert response.status == 200"));
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
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @description Tests something"));
        assert!(content.contains("# @disabled"));
    }

    #[test]
    fn test_roundtrip_description_disabled() {
        let original = "### @setup My Setup\n# @description Fetches the auth token\n# @disabled\nPOST https://auth.example.com/token\nContent-Type: application/x-www-form-urlencoded\n\ngrant_type=client_credentials\n\n# @assert status == 200\n";
        let suite = parse_test_suite(original);
        assert_eq!(suite.blocks[0].description, "Fetches the auth token");
        assert!(suite.blocks[0].disabled);
        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @description Fetches the auth token"));
        assert!(regenerated.contains("# @disabled"));
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
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.starts_with("@variables\n"), "Generated content should start with @variables");
        assert!(content.contains("base_url = https://api.example.com"));
        assert!(content.contains("api_key = sk-12345"));
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
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @depends Update User"));
    }

    #[test]
    fn test_roundtrip_depends() {
        let input = "### @test Verify Update\n# @depends Update User\n# @depends Create User\nGET https://api.example.com/users/1\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.blocks[0].depends.len(), 2);
        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @depends Update User"));
        assert!(regenerated.contains("# @depends Create User"));
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
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @group validation"));
        assert!(content.contains("# @depends setup-data"));
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
        assert!(regenerated.contains("# @group create"));
        assert!(regenerated.contains("# @group validate"));
        assert!(regenerated.contains("# @group report"));
        assert!(regenerated.contains("# @depends create"));
        assert!(regenerated.contains("# @depends validate"));
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
            }],
            ..Default::default()
        };
        let content = generate_http_content(&suite);
        assert!(content.contains("# @mode app"));
    }

    #[test]
    fn test_roundtrip_mode_directive() {
        let original = "### @setup Fetch Token\n# @mode app\nPOST https://login.example.com/token\n";
        let suite = parse_test_suite(original);
        assert_eq!(suite.blocks[0].mode, Some("app".to_string()));
        let regenerated = generate_http_content(&suite);
        assert!(regenerated.contains("# @mode app"));
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
        assert!(regenerated.contains("# @dev_auth https://prometheus.monitor.azure.com/.default"));
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
    fn test_telemetry_directives_roundtrip() {
        let input = "# @telemetry conn_str_var\n# @telemetry_service my-e2e\n# @telemetry_token arm_token\n\n@variables\nconn_str_var = test\narm_token = bearer123\n\n### @test Ping\nGET https://example.com\n\n# @assert status == 200\n";
        let suite = parse_test_suite(input);
        assert_eq!(suite.telemetry_var.as_deref(), Some("conn_str_var"));
        assert_eq!(suite.telemetry_service.as_deref(), Some("my-e2e"));
        assert_eq!(suite.telemetry_token.as_deref(), Some("arm_token"));

        let output = generate_http_content(&suite);
        assert!(output.contains("# @telemetry conn_str_var"));
        assert!(output.contains("# @telemetry_service my-e2e"));
        assert!(output.contains("# @telemetry_token arm_token"));
    }
}
