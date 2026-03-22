use std::collections::HashMap;

/// Parse a .env file content into key-value pairs.
/// Format: KEY=VALUE, one per line. Lines starting with # are comments. Empty lines ignored.
/// Values can be optionally quoted with " or '.
pub fn parse_env(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim().to_string();
            let mut value = value.trim().to_string();
            // Strip surrounding quotes
            if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                value = value[1..value.len() - 1].to_string();
            }
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

/// Generate .env file content from key-value pairs.
/// Outputs KEY=VALUE format, one per line, sorted alphabetically.
/// Values containing spaces or special chars are quoted.
pub fn generate_env(vars: &HashMap<String, String>) -> String {
    let mut sorted: Vec<_> = vars.iter().collect();
    sorted.sort_by_key(|(k, _)| k.to_lowercase());

    let mut output = String::new();
    output.push_str("# Request Pilot environment variables\n");
    output.push_str("# Edit values below and save\n\n");

    for (key, value) in sorted {
        if value.contains(' ') || value.contains('#') || value.contains('=') {
            output.push_str(&format!("{}=\"{}\"\n", key, value));
        } else {
            output.push_str(&format!("{}={}\n", key, value));
        }
    }
    output
}

/// Read and parse a .env file from disk.
pub fn read_env_from_path(path: &str) -> Result<HashMap<String, String>, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path, e))?;
    Ok(parse_env(&content))
}

/// Write env vars to a .env file on disk.
pub fn write_env_to_path(path: &str, vars: &HashMap<String, String>) -> Result<(), String> {
    let content = generate_env(vars);
    std::fs::write(path, content).map_err(|e| format!("Failed to write {}: {}", path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let content = "KEY=value\nANOTHER=123";
        let map = parse_env(content);
        assert_eq!(map.get("KEY").unwrap(), "value");
        assert_eq!(map.get("ANOTHER").unwrap(), "123");
    }

    #[test]
    fn parse_with_comments_and_empty_lines() {
        let content = "# comment\n\nKEY=value\n# another comment\n";
        let map = parse_env(content);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("KEY").unwrap(), "value");
    }

    #[test]
    fn parse_quoted_values() {
        let content = "KEY=\"hello world\"\nSINGLE='single quoted'";
        let map = parse_env(content);
        assert_eq!(map.get("KEY").unwrap(), "hello world");
        assert_eq!(map.get("SINGLE").unwrap(), "single quoted");
    }

    #[test]
    fn parse_value_with_equals() {
        let content = "URL=https://example.com?a=1&b=2";
        let map = parse_env(content);
        assert_eq!(
            map.get("URL").unwrap(),
            "https://example.com?a=1&b=2"
        );
    }

    #[test]
    fn parse_empty_value() {
        let content = "EMPTY=";
        let map = parse_env(content);
        assert_eq!(map.get("EMPTY").unwrap(), "");
    }

    #[test]
    fn parse_whitespace_around_key_value() {
        let content = "  KEY  =  value  ";
        let map = parse_env(content);
        assert_eq!(map.get("KEY").unwrap(), "value");
    }

    #[test]
    fn generate_sorts_alphabetically() {
        let mut vars = HashMap::new();
        vars.insert("ZEBRA".to_string(), "z".to_string());
        vars.insert("ALPHA".to_string(), "a".to_string());
        let output = generate_env(&vars);
        let lines: Vec<&str> = output
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();
        assert!(lines[0].starts_with("ALPHA="));
        assert!(lines[1].starts_with("ZEBRA="));
    }

    #[test]
    fn generate_quotes_spaces() {
        let mut vars = HashMap::new();
        vars.insert("KEY".to_string(), "hello world".to_string());
        let output = generate_env(&vars);
        assert!(output.contains("KEY=\"hello world\""));
    }

    #[test]
    fn roundtrip() {
        let mut vars = HashMap::new();
        vars.insert("HOST".to_string(), "localhost".to_string());
        vars.insert("PORT".to_string(), "8080".to_string());
        vars.insert("NAME".to_string(), "test user".to_string());
        let content = generate_env(&vars);
        let parsed = parse_env(&content);
        assert_eq!(parsed.get("HOST").unwrap(), "localhost");
        assert_eq!(parsed.get("PORT").unwrap(), "8080");
        assert_eq!(parsed.get("NAME").unwrap(), "test user");
    }
}
