use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub time_ms: u64,
    pub size_bytes: usize,
}

/// Percent-encode a single byte for use inside a form-urlencoded value.
///
/// Rules (from `application/x-www-form-urlencoded` spec):
/// - Unreserved per RFC 3986 (`A-Z a-z 0-9 - _ . ~`) → pass through.
/// - Space → `+`.
/// - Everything else → `%XX` (uppercase hex).
///
/// Note `*` is also passed through to match the de-facto behavior of
/// JS's `encodeURIComponent`, which is what most authors hand-encode with.
fn form_encode_byte(b: u8, out: &mut String) {
    match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'*' => {
            out.push(b as char);
        }
        b' ' => out.push('+'),
        _ => {
            out.push('%');
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
}

/// Form-encode a string, preserving any valid `%XX` escape sequences
/// already present in the input. This is what makes the runtime
/// auto-encoder idempotent on mixed-encoding source bodies — e.g.
/// authors can write `query=sum(a) %2B sum(b)` (literal parens and
/// spaces, but `%2B` for the literal `+` operator) and we won't
/// double-encode the `%2B` to `%252B`.
///
/// Hex digits inside preserved escapes are uppercased to match the
/// canonical wire form.
fn form_encode_str(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len() * 2);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            out.push('%');
            out.push(bytes[i + 1].to_ascii_uppercase() as char);
            out.push(bytes[i + 2].to_ascii_uppercase() as char);
            i += 3;
            continue;
        }
        form_encode_byte(b, &mut out);
        i += 1;
    }
    out
}

/// Heuristic: should this `application/x-www-form-urlencoded` body be
/// re-encoded? A correctly-encoded body contains ONLY chars from the
/// form-urlencoded "safe set":
///
/// - Unreserved (`A-Z a-z 0-9 - _ . ~ *`)
/// - Structural delimiters `&` and `=`
/// - `+` (RFC 1866 convention: encoded space)
/// - `%` followed by two hex digits (escape sequence)
///
/// Any byte outside this set indicates a human-authored / partially-
/// decoded body that needs encoding before going on the wire.
/// `%` not followed by two hex digits is a stray escape char and must
/// also be encoded (as `%25`) to produce a valid body.
///
/// Bodies that are already fully-encoded (only safe-set chars) pass
/// through unchanged — i.e. this function returns `false` for any body
/// that is already wire-format compliant.
fn body_needs_form_encoding(body: &str) -> bool {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit()
            {
                i += 3;
                continue;
            }
            // Stray `%` (not a valid escape) — must be encoded as `%25`.
            return true;
        }
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'*'
            | b'&'
            | b'='
            | b'+' => {
                i += 1;
            }
            _ => return true,
        }
    }
    false
}

/// Re-encode a form-urlencoded body. Treats `&` as pair delimiter and the
/// FIRST `=` in each pair as key/value delimiter; everything else within
/// a value is treated as raw content and form-encoded.
///
/// `form_encode_str` preserves any already-encoded `%XX` sequences in the
/// input, so literal `+` characters (e.g. PromQL operators) authored as
/// `%2B` in the source stay `%2B` rather than being double-encoded.
/// Raw newlines become `%0A`, raw spaces become `+`, parens become
/// `%28`/`%29`, and so on.
fn reencode_form_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len() * 2);
    let mut first = true;
    for pair in body.split('&') {
        if !first {
            out.push('&');
        }
        first = false;
        match pair.split_once('=') {
            Some((key, value)) => {
                out.push_str(&form_encode_str(key));
                out.push('=');
                out.push_str(&form_encode_str(value));
            }
            None => {
                out.push_str(&form_encode_str(pair));
            }
        }
    }
    out
}

/// Returns the request body to actually send on the wire. If the
/// Content-Type is `application/x-www-form-urlencoded` and the body
/// contains characters that aren't valid in form-urlencoded wire format
/// (raw whitespace, control chars, non-ASCII), the body is parsed as
/// `key=value&key=value` pairs and each value is form-encoded.
///
/// Otherwise the body is returned unchanged.
fn prepare_request_body(headers: &[(String, String)], body: &str) -> String {
    let is_form = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && v.to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
    });
    if is_form && body_needs_form_encoding(body) {
        reencode_form_body(body)
    } else {
        body.to_string()
    }
}

/// Percent-encode characters in the query string that are not valid in URIs.
/// Preserves URL structure (scheme, host, path) and query delimiters (&, =).
fn encode_url(raw: &str) -> String {
    // If no query string, encode only truly illegal chars in the whole URL
    let (base, query) = match raw.find('?') {
        Some(idx) => (&raw[..idx], Some(&raw[idx + 1..])),
        None => (raw, None),
    };

    // Encode spaces in base URL (rare but possible)
    let encoded_base: String = base.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        _ => c.to_string(),
    }).collect();

    match query {
        None => encoded_base,
        Some(q) => {
            let encoded_query: String = q.chars().map(|c| match c {
                ' ' => "%20".to_string(),
                '{' => "%7B".to_string(),
                '}' => "%7D".to_string(),
                '"' => "%22".to_string(),
                '<' => "%3C".to_string(),
                '>' => "%3E".to_string(),
                '|' => "%7C".to_string(),
                '\\' => "%5C".to_string(),
                '^' => "%5E".to_string(),
                '`' => "%60".to_string(),
                '[' => "%5B".to_string(),
                ']' => "%5D".to_string(),
                _ => c.to_string(),
            }).collect();
            format!("{}?{}", encoded_base, encoded_query)
        }
    }
}

pub async fn execute_request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
) -> Result<HttpResponse, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let method = method
        .parse::<reqwest::Method>()
        .map_err(|e| format!("Invalid HTTP method: {}", e))?;

    let mut header_map = HeaderMap::new();
    for (key, value) in headers {
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|e| format!("Invalid header name '{}': {}", key, e))?;
        let val = HeaderValue::from_str(value)
            .map_err(|e| format!("Invalid header value for '{}': {}", key, e))?;
        header_map.insert(name, val);
    }

    let encoded = encode_url(url);
    let mut request = client.request(method, &encoded).headers(header_map);
    if let Some(body_str) = body {
        let prepared = prepare_request_body(headers, body_str);
        request = request.body(prepared);
    }

    let start = Instant::now();
    let response = request
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let elapsed = start.elapsed().as_millis() as u64;

    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or("Unknown")
        .to_string();

    let resp_headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                v.to_str().unwrap_or("<binary>").to_string(),
            )
        })
        .collect();

    let body_bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;
    let size = body_bytes.len();
    let body_text = String::from_utf8_lossy(&body_bytes).to_string();

    Ok(HttpResponse {
        status,
        status_text,
        headers: resp_headers,
        body: body_text,
        time_ms: elapsed,
        size_bytes: size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_url_no_query() {
        assert_eq!(encode_url("https://example.com/api"), "https://example.com/api");
    }

    #[test]
    fn encode_url_normal_query() {
        assert_eq!(encode_url("https://example.com/api?foo=bar&baz=42"), "https://example.com/api?foo=bar&baz=42");
    }

    #[test]
    fn encode_url_spaces_in_query() {
        assert_eq!(
            encode_url("https://example.com/api?query=foo bar baz"),
            "https://example.com/api?query=foo%20bar%20baz"
        );
    }

    #[test]
    fn encode_url_braces_in_query() {
        assert_eq!(
            encode_url(r#"https://example.com/api?q={"cpu"} * on()"#),
            "https://example.com/api?q=%7B%22cpu%22%7D%20*%20on()"
        );
    }

    #[test]
    fn encode_url_complex_promql() {
        let input = r#"https://prom.example.com/api/v1/query_range?query={"system.cpu.time"} * on() group_left up&start=2026-03-17T19:00:00Z&step=60"#;
        let encoded = encode_url(input);
        assert!(encoded.contains("query=%7B%22system.cpu.time%22%7D%20*%20on()%20group_left%20up"));
        assert!(encoded.contains("&start=2026-03-17T19:00:00Z&step=60"));
        assert!(!encoded.contains(' '));
    }

    #[test]
    fn encode_url_space_in_base() {
        assert_eq!(encode_url("https://example.com/my path/api"), "https://example.com/my%20path/api");
    }

    #[test]
    fn encode_url_already_encoded() {
        // Should not double-encode %20
        assert_eq!(encode_url("https://example.com/api?q=foo%20bar"), "https://example.com/api?q=foo%20bar");
    }

    // ---------- form-urlencoded body re-encoding ----------

    fn ct_form() -> Vec<(String, String)> {
        vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())]
    }

    fn ct_form_charset() -> Vec<(String, String)> {
        vec![("Content-Type".to_string(), "application/x-www-form-urlencoded;charset=UTF-8".to_string())]
    }

    fn ct_json() -> Vec<(String, String)> {
        vec![("Content-Type".to_string(), "application/json".to_string())]
    }

    #[test]
    fn body_needs_encoding_detects_raw_space() {
        assert!(body_needs_form_encoding("query=foo bar"));
    }

    #[test]
    fn body_needs_encoding_detects_newline() {
        assert!(body_needs_form_encoding("query=foo\nbar"));
        assert!(body_needs_form_encoding("query=foo\r\nbar"));
    }

    #[test]
    fn body_needs_encoding_detects_tab() {
        assert!(body_needs_form_encoding("query=foo\tbar"));
    }

    #[test]
    fn body_needs_encoding_detects_non_ascii() {
        assert!(body_needs_form_encoding("query=café"));
    }

    #[test]
    fn body_needs_encoding_passes_clean_body() {
        assert!(!body_needs_form_encoding("query=foo&time=1700000000"));
        assert!(!body_needs_form_encoding("query=a%2Bb&time=1700000000"));
        assert!(!body_needs_form_encoding("query=foo+bar"));
        assert!(!body_needs_form_encoding(""));
    }

    #[test]
    fn body_needs_encoding_detects_parens() {
        // After broadening: parens (and other non-safe-set chars) trigger
        // re-encode even without raw whitespace, because the safe set is
        // strictly form-urlencoded chars + `+ & = %XX`.
        assert!(body_needs_form_encoding("query=sum(a)"));
        assert!(body_needs_form_encoding("query=sum(rate(foo[5m]))"));
        assert!(body_needs_form_encoding("data={\"a\":1}"));
    }

    #[test]
    fn body_needs_encoding_detects_stray_percent() {
        // `%` not followed by two hex digits is a stray escape char.
        assert!(body_needs_form_encoding("key=100%complete"));
        assert!(body_needs_form_encoding("key=foo%"));
        assert!(body_needs_form_encoding("key=foo%G1bar"));
        // But a valid `%XX` is fine.
        assert!(!body_needs_form_encoding("key=foo%2Fbar"));
    }

    #[test]
    fn body_needs_encoding_accepts_valid_escapes() {
        // A body composed entirely of safe-set chars + valid %XX escapes
        // does NOT need re-encoding.
        assert!(!body_needs_form_encoding(
            "query=sum%28foo%29+by+%28pod%29+%2B+1&time=1700000000"
        ));
    }

    #[test]
    fn prepare_body_passthrough_when_already_encoded() {
        let body = "query=sum+by+%28pod%29+up&time=1700000000";
        assert_eq!(prepare_request_body(&ct_form(), body), body);
    }

    #[test]
    fn prepare_body_passthrough_when_not_form_content_type() {
        let body = "{ \"name\": \"foo bar\", \"x\": \"a+b\" }";
        assert_eq!(prepare_request_body(&ct_json(), body), body);
    }

    #[test]
    fn prepare_body_passthrough_when_no_content_type() {
        let body = "query=foo bar"; // raw space — but no CT, so no encoding
        assert_eq!(prepare_request_body(&[], body), body);
    }

    #[test]
    fn prepare_body_encodes_promql_with_plus_operator() {
        // The exact bug from GetContainerStatus: literal `+` is a PromQL
        // operator and must be encoded as %2B (not sent as raw `+`, which
        // the server decodes to space).
        let body = "query=({\"a\"} * 1)\n            +\n            ({\"b\"} * 2)&time=123";
        let out = prepare_request_body(&ct_form(), body);
        assert!(out.contains("%2B"), "literal + must be encoded as %2B, got: {}", out);
        assert!(!out.contains('\n'), "raw newlines must be encoded");
        assert!(!out.contains(' '), "raw spaces must be encoded");
        // Pair delimiters and first `=` per pair stay as-is
        assert!(out.contains("&time=123"), "pair delimiter & and key=val must be preserved: {}", out);
        assert!(out.starts_with("query="), "key=val first delimiter preserved: {}", out);
    }

    #[test]
    fn prepare_body_encodes_spaces_as_plus() {
        let body = "query=hello world";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "query=hello+world");
    }

    #[test]
    fn prepare_body_encodes_newlines_as_percent_0a() {
        let body = "query=foo\nbar";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "query=foo%0Abar");
    }

    #[test]
    fn prepare_body_preserves_pair_and_kv_delimiters() {
        // & between pairs and the first = within a pair are structural and
        // must remain unencoded.
        let body = "q=a b&time=123&start=100";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "q=a+b&time=123&start=100");
    }

    #[test]
    fn prepare_body_handles_charset_in_content_type() {
        let body = "query=hello world";
        let out = prepare_request_body(&ct_form_charset(), body);
        assert_eq!(out, "query=hello+world");
    }

    #[test]
    fn prepare_body_handles_empty_value() {
        let body = "key=&other=val";
        // No whitespace → not re-encoded
        assert_eq!(prepare_request_body(&ct_form(), body), body);
    }

    #[test]
    fn prepare_body_handles_value_without_key() {
        // Defensive: a token with no `=` is encoded as a single key
        let body = "loose value";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "loose+value");
    }

    #[test]
    fn prepare_body_idempotent_on_pre_encoded_input() {
        // Already-encoded body has no whitespace → not re-encoded
        let body = "query=sum+by+%28pod%29+%28%7Bup%7D+%2B+1%29&time=1700000000";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, body);
    }

    #[test]
    fn prepare_body_encodes_special_chars() {
        let body = "q={\"x\"} != \"y\"";
        let out = prepare_request_body(&ct_form(), body);
        // braces, quotes, !, =, space inside value all encoded
        assert!(out.contains("%7B"));
        assert!(out.contains("%7D"));
        assert!(out.contains("%22"));
        assert!(out.contains("%21%3D"));
        // First `=` after `q` stays unencoded as kv separator
        assert!(out.starts_with("q="));
        assert!(!out.contains(' '));
    }

    #[test]
    fn prepare_body_encodes_non_ascii() {
        let body = "name=café";
        let out = prepare_request_body(&ct_form(), body);
        // UTF-8 for é is 0xC3 0xA9 → %C3%A9
        assert_eq!(out, "name=caf%C3%A9");
    }

    #[test]
    fn prepare_body_preserves_existing_escapes_when_re_encoding() {
        // The headline new behavior: a body authored in "minimal-encoded"
        // form (literal parens, spaces; `%2B` for the literal `+` operator)
        // must NOT have its `%2B` double-encoded to `%252B` when the
        // runtime re-encodes the rest of the value.
        let body = "query=sum(a) %2B sum(b)";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "query=sum%28a%29+%2B+sum%28b%29");
    }

    #[test]
    fn prepare_body_encodes_parens_without_whitespace() {
        // Pre-fix this would have passed through (no whitespace) and the
        // server would have received raw parens. Now we encode them.
        let body = "query=sum(a)";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "query=sum%28a%29");
    }

    #[test]
    fn prepare_body_encodes_json_form_value_without_whitespace() {
        // From `tryJsonToFormString` in the desktop UI — JSON-shaped
        // form values with `{"a":1}` and no whitespace must still be
        // encoded.
        let body = "payload={\"a\":1}";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "payload=%7B%22a%22%3A1%7D");
    }

    #[test]
    fn prepare_body_encodes_stray_percent() {
        // Stray `%` (not a valid escape) gets encoded as `%25`.
        let body = "key=100%complete";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "key=100%25complete");
    }

    #[test]
    fn prepare_body_uppercases_hex_in_preserved_escapes() {
        // Any `%xx` lowercased hex digits get normalized to uppercase
        // when the body is re-encoded.
        let body = "query=a %2b b";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "query=a+%2B+b");
    }

    #[test]
    fn prepare_body_decoded_promql_roundtrips_correctly() {
        // The end-to-end story: a fully-decoded readable PromQL body
        // (literal newlines, parens, commas, spaces; `%2B` for `+`)
        // becomes a valid wire-format body after auto-encoding.
        let body =
            "query=quantile_over_time(0.90, (\n  sum(rate(foo[5m]))\n)) %2B 1&time=1700000000";
        let out = prepare_request_body(&ct_form(), body);
        // No raw whitespace in the output
        assert!(!out.contains(' '), "spaces must be encoded: {}", out);
        assert!(!out.contains('\n'), "newlines must be encoded: {}", out);
        // Pre-existing `%2B` preserved (not double-encoded)
        assert!(out.contains("%2B"), "literal + preserved as %2B: {}", out);
        assert!(!out.contains("%252B"), "no double-encoding: {}", out);
        // Pair delimiter and first `=` per pair preserved
        assert!(out.contains("&time=1700000000"), "pair separator preserved: {}", out);
        assert!(out.starts_with("query="), "kv separator preserved: {}", out);
    }

    #[test]
    fn prepare_body_preserves_template_var_lookalike() {
        // `{{var}}` patterns in form bodies are interpolated by the JS
        // layer BEFORE the body reaches the runtime, so they should
        // never appear here in practice. But if they ever do, the runtime
        // encodes the braces correctly (it doesn't know about templates).
        let body = "query={{var}}";
        let out = prepare_request_body(&ct_form(), body);
        assert_eq!(out, "query=%7B%7Bvar%7D%7D");
    }

    #[test]
    fn form_encode_byte_unreserved_passthrough() {
        for b in b'A'..=b'Z' {
            let mut s = String::new();
            form_encode_byte(b, &mut s);
            assert_eq!(s, (b as char).to_string());
        }
        let mut s = String::new();
        form_encode_byte(b'-', &mut s);
        form_encode_byte(b'_', &mut s);
        form_encode_byte(b'.', &mut s);
        form_encode_byte(b'~', &mut s);
        form_encode_byte(b'*', &mut s);
        assert_eq!(s, "-_.~*");
    }
}
