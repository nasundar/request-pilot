use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub time_ms: u64,
    pub size_bytes: usize,
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
        request = request.body(body_str.to_string());
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
}
