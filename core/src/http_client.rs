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

fn form_encode_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        form_encode_byte(b, &mut out);
    }
    out
}

/// Heuristic: should this `application/x-www-form-urlencoded` body be
/// re-encoded? A correctly-encoded form body contains only printable ASCII
/// with no raw whitespace (space → `+`, newlines → `%0A`, etc.). Any of
/// those signals indicates a human-authored / hand-typed body that the
/// runtime needs to encode for the wire.
///
/// Bodies that are already encoded (no raw whitespace, all non-ASCII
/// percent-encoded) pass through unchanged — i.e. this function is
/// idempotent on already-encoded input.
fn body_needs_form_encoding(body: &str) -> bool {
    body.bytes().any(|b| {
        // Raw whitespace (LF, CR, TAB, SPACE) is never valid in a wire
        // form-urlencoded body — it must be encoded.
        b == b'\n' || b == b'\r' || b == b'\t' || b == b' '
            // Other control chars (< 0x20 except those above) — not valid raw.
            || b < 0x20
            // Non-ASCII — must be percent-encoded as UTF-8 bytes.
            || b > 0x7E
    })
}

/// Re-encode a form-urlencoded body. Treats `&` as pair delimiter and the
/// FIRST `=` in each pair as key/value delimiter; everything else within
/// a value is treated as raw content and form-encoded.
///
/// This means literal `+` characters in the input (e.g. PromQL operators)
/// are encoded as `%2B`, raw newlines as `%0A`, and spaces as `+`.
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
