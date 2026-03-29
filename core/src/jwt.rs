//! Lightweight JWT decoder (no signature verification).
//!
//! Decodes JWT header and payload for display purposes.

use serde_json::Value;

/// Decoded JWT token with header, payload, and metadata.
#[derive(Debug, Clone)]
pub struct DecodedJwt {
    pub header: Value,
    pub payload: Value,
    pub token_type: String,
    pub expiry: Option<ExpiryInfo>,
    pub signature_preview: String,
}

/// Expiry information extracted from the `exp` claim.
#[derive(Debug, Clone)]
pub struct ExpiryInfo {
    pub exp_epoch: i64,
    pub is_expired: bool,
    /// Human-readable time remaining or "EXPIRED".
    pub display: String,
}

/// Detect whether a string looks like a JWT (three base64url segments).
pub fn is_jwt(value: &str) -> bool {
    let parts: Vec<&str> = value.splitn(4, '.').collect();
    if parts.len() != 3 {
        return false;
    }
    // Both header and payload must be valid base64url
    base64url_decode(parts[0]).is_some() && base64url_decode(parts[1]).is_some()
}

/// Decode a JWT token without verifying the signature.
/// Returns `None` if the token is not a valid JWT structure.
pub fn decode(token: &str) -> Option<DecodedJwt> {
    let token = token.strip_prefix("Bearer ").unwrap_or(token);
    let parts: Vec<&str> = token.splitn(4, '.').collect();
    if parts.len() != 3 {
        return None;
    }

    let header_bytes = base64url_decode(parts[0])?;
    let payload_bytes = base64url_decode(parts[1])?;

    let header_str = String::from_utf8(header_bytes).ok()?;
    let payload_str = String::from_utf8(payload_bytes).ok()?;

    let header: Value = serde_json::from_str(&header_str).ok()?;
    let payload: Value = serde_json::from_str(&payload_str).ok()?;

    let token_type = detect_token_type(&header, &payload);
    let expiry = extract_expiry(&payload);
    let sig = parts[2];
    let signature_preview = if sig.len() > 32 {
        format!("{}…", &sig[..32])
    } else {
        sig.to_string()
    };

    Some(DecodedJwt {
        header,
        payload,
        token_type,
        expiry,
        signature_preview,
    })
}

/// Classify the token type from header/payload hints.
fn detect_token_type(header: &Value, payload: &Value) -> String {
    let typ = header.get("typ").and_then(|v| v.as_str()).unwrap_or("");
    if typ == "at+jwt" || payload.get("aud").is_some() {
        if payload.get("nonce").is_some() {
            return "ID Token".to_string();
        }
        return "Access Token".to_string();
    }
    if payload.get("refresh_token").is_some() {
        return "Refresh Token".to_string();
    }
    "JWT".to_string()
}

/// Extract expiry info from the `exp` claim.
fn extract_expiry(payload: &Value) -> Option<ExpiryInfo> {
    let exp = payload.get("exp")?.as_i64()?;
    let now = chrono::Utc::now().timestamp();
    let is_expired = exp < now;
    let display = if is_expired {
        let ago = now - exp;
        format!("EXPIRED ({})", format_duration(ago))
    } else {
        let remaining = exp - now;
        format_duration(remaining)
    };
    Some(ExpiryInfo {
        exp_epoch: exp,
        is_expired,
        display,
    })
}

fn format_duration(secs: i64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

/// Extract key claims from the payload for summary display.
/// Returns a list of (label, value) pairs.
pub fn key_claims(payload: &Value) -> Vec<(&'static str, String)> {
    let mut claims = Vec::new();

    let str_field = |key: &str| -> Option<String> {
        payload.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
    };

    if let Some(v) = str_field("iss") { claims.push(("Issuer", v)); }
    if let Some(v) = str_field("sub") { claims.push(("Subject", v)); }
    if let Some(v) = payload.get("aud") {
        let display = match v {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            _ => v.to_string(),
        };
        claims.push(("Audience", display));
    }
    if let Some(v) = str_field("name") { claims.push(("Name", v)); }
    if let Some(v) = str_field("preferred_username").or_else(|| str_field("email")) {
        claims.push(("User", v));
    }
    if let Some(v) = str_field("scp").or_else(|| str_field("scope")) {
        claims.push(("Scope", v));
    }
    if let Some(Value::Array(roles)) = payload.get("roles") {
        let roles_str = roles.iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !roles_str.is_empty() {
            claims.push(("Roles", roles_str));
        }
    }
    if let Some(v) = str_field("appid").or_else(|| str_field("azp")) {
        claims.push(("App ID", v));
    }
    if let Some(v) = str_field("tid") { claims.push(("Tenant", v)); }

    claims
}

/// Detect the type of a variable value.
#[derive(Debug, Clone)]
pub enum ValueType {
    Empty,
    Jwt(DecodedJwt),
    BearerOpaque(String),
    Url,
    Uuid,
    Json,
    Plain,
}

/// Analyze a variable value and return its detected type.
pub fn detect_value_type(value: &str) -> ValueType {
    if value.is_empty() {
        return ValueType::Empty;
    }

    // Check Bearer prefix first
    if let Some(inner) = value.strip_prefix("Bearer ") {
        if let Some(decoded) = decode(inner) {
            return ValueType::Jwt(decoded);
        }
        return ValueType::BearerOpaque(inner.to_string());
    }

    // JWT
    if is_jwt(value) {
        if let Some(decoded) = decode(value) {
            return ValueType::Jwt(decoded);
        }
    }

    // URL
    if value.starts_with("http://") || value.starts_with("https://") {
        return ValueType::Url;
    }

    // UUID
    if value.len() == 36
        && value.chars().enumerate().all(|(i, c)| {
            if i == 8 || i == 13 || i == 18 || i == 23 { c == '-' }
            else { c.is_ascii_hexdigit() }
        })
    {
        return ValueType::Uuid;
    }

    // JSON object or array
    let trimmed = value.trim();
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if serde_json::from_str::<Value>(trimmed).is_ok() {
            return ValueType::Json;
        }
    }

    ValueType::Plain
}

// ── Base64url decoding ──

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let padded = match input.len() % 4 {
        2 => format!("{}==", input),
        3 => format!("{}=", input),
        _ => input.to_string(),
    };
    let b64 = padded.replace('-', "+").replace('_', "/");
    base64_decode(&b64)
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 4 {
            return None;
        }
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let c = val(chunk[2])?;
        let d = val(chunk[3])?;
        let n = (a as u32) << 18 | (b as u32) << 12 | (c as u32) << 6 | d as u32;
        out.push((n >> 16) as u8);
        if chunk[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if chunk[3] != b'=' {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_empty() {
        assert!(matches!(detect_value_type(""), ValueType::Empty));
    }

    #[test]
    fn detect_url() {
        assert!(matches!(detect_value_type("https://api.example.com/v1"), ValueType::Url));
        assert!(matches!(detect_value_type("http://localhost:3000"), ValueType::Url));
    }

    #[test]
    fn detect_uuid() {
        assert!(matches!(
            detect_value_type("550e8400-e29b-41d4-a716-446655440000"),
            ValueType::Uuid
        ));
    }

    #[test]
    fn detect_json() {
        assert!(matches!(detect_value_type(r#"{"key": "value"}"#), ValueType::Json));
        assert!(matches!(detect_value_type(r#"[1, 2, 3]"#), ValueType::Json));
    }

    #[test]
    fn detect_plain() {
        assert!(matches!(detect_value_type("just a string"), ValueType::Plain));
        assert!(matches!(detect_value_type("12345"), ValueType::Plain));
    }

    #[test]
    fn is_jwt_valid() {
        // Minimal valid JWT: {"alg":"none"}.{"sub":"test"}.signature
        let header = "eyJhbGciOiJub25lIn0"; // {"alg":"none"}
        let payload = "eyJzdWIiOiJ0ZXN0In0"; // {"sub":"test"}
        let token = format!("{}.{}.sig", header, payload);
        assert!(is_jwt(&token));
    }

    #[test]
    fn is_jwt_invalid() {
        assert!(!is_jwt("not-a-jwt"));
        assert!(!is_jwt("one.two"));
        assert!(!is_jwt(""));
    }

    #[test]
    fn decode_jwt_basic() {
        // {"alg":"RS256","typ":"JWT"}.{"sub":"1234567890","name":"Test User","iat":1516239022}
        let token = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IlRlc3QgVXNlciIsImlhdCI6MTUxNjIzOTAyMn0.fakesig";
        let decoded = decode(token).unwrap();
        assert_eq!(decoded.header["alg"], "RS256");
        assert_eq!(decoded.payload["sub"], "1234567890");
        assert_eq!(decoded.payload["name"], "Test User");
    }

    #[test]
    fn decode_jwt_with_bearer_prefix() {
        let token = "Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IlRlc3QgVXNlciIsImlhdCI6MTUxNjIzOTAyMn0.fakesig";
        let decoded = decode(token).unwrap();
        assert_eq!(decoded.payload["name"], "Test User");
    }

    #[test]
    fn key_claims_extraction() {
        let payload: Value = serde_json::json!({
            "iss": "https://login.microsoftonline.com",
            "sub": "abc123",
            "aud": "https://api.example.com",
            "name": "Test User",
            "preferred_username": "test@example.com",
            "roles": ["admin", "reader"],
            "tid": "tenant-id-123"
        });
        let claims = key_claims(&payload);
        assert!(claims.iter().any(|(k, _)| *k == "Issuer"));
        assert!(claims.iter().any(|(k, _)| *k == "User"));
        assert!(claims.iter().any(|(k, v)| *k == "Roles" && v.contains("admin")));
    }

    #[test]
    fn detect_bearer_jwt() {
        let token = "Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.fakesig";
        assert!(matches!(detect_value_type(token), ValueType::Jwt(_)));
    }

    #[test]
    fn detect_bearer_opaque() {
        assert!(matches!(
            detect_value_type("Bearer some-opaque-token-value"),
            ValueType::BearerOpaque(_)
        ));
    }
}
