use std::collections::HashMap;

/// Parse a .env file content into key-value pairs.
/// Format: KEY=VALUE, one per line. Lines starting with # are comments. Empty lines ignored.
/// Values can be optionally quoted with " or '.
pub fn parse_env(content: &str) -> HashMap<String, String> {
    parse_env_named(content).1
}

/// Parse a .env file content, returning both any `# @@name <name>` directive
/// (falling back to legacy `# @name <name>`) and the variable map. Directive
/// must appear in a comment line — plain dotenv/docker readers ignore it.
pub fn parse_env_named(content: &str) -> (Option<String>, HashMap<String, String>) {
    let mut map = HashMap::new();
    let mut name: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            // Look for `@@name <value>` or legacy `@name <value>` inside the comment.
            if name.is_none() {
                if let Some(v) = extract_name_directive(rest) {
                    name = Some(v);
                }
            }
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim().to_string();
            let mut value = value.trim().to_string();
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
    (name, map)
}

fn extract_name_directive(comment_body: &str) -> Option<String> {
    let s = comment_body.trim_start();
    // Accept `@@name <value>`, `@@name=<value>`, and legacy `@name <value>` / `@name=<value>`.
    let rest = if let Some(r) = s.strip_prefix("@@name") {
        r
    } else if let Some(r) = s.strip_prefix("@name") {
        r
    } else {
        return None;
    };
    let rest = rest.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
    if rest.is_empty() {
        return None;
    }
    Some(rest.trim().to_string())
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

/// Read and parse a .env file, returning its embedded `# @@name` directive
/// (if any) alongside the variable map.
pub fn read_env_named_from_path(
    path: &str,
) -> Result<(Option<String>, HashMap<String, String>), String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path, e))?;
    Ok(parse_env_named(&content))
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

    #[test]
    fn name_directive_double_at() {
        let content = "# @@name staging\nBASE_URL=https://staging";
        let (name, vars) = parse_env_named(content);
        assert_eq!(name.as_deref(), Some("staging"));
        assert_eq!(vars.get("BASE_URL").unwrap(), "https://staging");
    }

    #[test]
    fn name_directive_legacy_single_at() {
        let content = "# @name prod\nBASE_URL=https://api";
        let (name, _) = parse_env_named(content);
        assert_eq!(name.as_deref(), Some("prod"));
    }

    #[test]
    fn name_directive_equals_form() {
        let content = "# @@name=dev\nK=V";
        let (name, _) = parse_env_named(content);
        assert_eq!(name.as_deref(), Some("dev"));
    }

    #[test]
    fn name_directive_extra_whitespace() {
        let content = "#    @@name    regional-prod-eu   \nK=V";
        let (name, _) = parse_env_named(content);
        assert_eq!(name.as_deref(), Some("regional-prod-eu"));
    }

    #[test]
    fn name_directive_absent() {
        let content = "# just a comment\nK=V";
        let (name, vars) = parse_env_named(content);
        assert!(name.is_none());
        assert_eq!(vars.get("K").unwrap(), "V");
    }

    #[test]
    fn name_directive_first_wins() {
        let content = "# @@name first\n# @@name second\nK=V";
        let (name, _) = parse_env_named(content);
        assert_eq!(name.as_deref(), Some("first"));
    }

    #[test]
    fn name_directive_only_before_vars() {
        // Comment after KV lines is fine; parser still picks up the directive
        // if it appears anywhere as a standalone comment.
        let content = "K=V\n# @@name afterwards";
        let (name, _) = parse_env_named(content);
        assert_eq!(name.as_deref(), Some("afterwards"));
    }

    #[test]
    fn plain_dotenv_compatibility() {
        // Directive must live in a comment line so it's silently ignored by
        // plain dotenv / docker --env-file consumers.
        let content = "# @@name staging\nFOO=bar";
        let plain_map = parse_env(content);
        assert_eq!(plain_map.len(), 1);
        assert_eq!(plain_map.get("FOO").unwrap(), "bar");
    }
}
