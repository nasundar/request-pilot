//! Migration from legacy Pilot `.http` syntax to REST Client–compatible `@@` syntax.
//!
//! Rules (content-preserving; stable when re-applied):
//!   - Lines matching `@variables` (legacy block header) are removed; subsequent
//!     `name = value` lines in that block are rewritten as top-level `@name = value`.
//!   - `### @<type> [title]` → `### @@<type> [title]` (for setup/test/teardown/variables).
//!   - `# @<directive>` / `// @<directive>` → `# @@<directive>` / `// @@<directive>`
//!     where `<directive>` is a known Pilot directive.
//!   - All other lines (bare REST Client directives `@name`/`@description`/`@note`/`@prompt`,
//!     top-level `@var = value`, requests, headers, bodies, separators) are left untouched.

use std::path::{Path, PathBuf};

const PILOT_DIRECTIVES: &[&str] = &[
    "assert", "extract", "group", "depends", "disabled",
    "mode", "dev_auth", "auto_run", "compare", "step", "diff",
    "telemetry", "telemetry_token", "telemetry_service", "description",
    "name", "note", "prompt",
];

const BLOCK_TYPES: &[&str] = &["setup", "test", "teardown", "variables"];

/// Apply the legacy → new syntax migration to a string, returning the new content.
///
/// The function is idempotent: running it twice produces the same result as once.
pub fn migrate_content(input: &str) -> String {
    let mut out: Vec<String> = Vec::with_capacity(input.lines().count() + 8);
    let mut in_legacy_vars_block = false;

    for raw_line in input.split_inclusive('\n') {
        // Preserve the line ending; split_inclusive keeps `\n` (and any preceding `\r`).
        let (body, ending) = match raw_line.find('\n') {
            Some(_) => {
                let n = raw_line.len();
                let body_end = if raw_line.ends_with("\r\n") { n - 2 } else { n - 1 };
                (&raw_line[..body_end], &raw_line[body_end..])
            }
            None => (raw_line, ""),
        };

        let trimmed = body.trim();

        // Exit legacy variables block on blank line, separator, or non-assignment
        if in_legacy_vars_block {
            let is_blank = trimmed.is_empty();
            let is_sep = is_separator(trimmed);
            let is_assignment = is_var_assignment_line(trimmed);
            let is_comment = trimmed.starts_with('#') || trimmed.starts_with("//");

            if is_sep || (!is_blank && !is_assignment && !is_comment) {
                in_legacy_vars_block = false;
            }
        }

        // Case 1: legacy `@variables` block header — drop this line entirely
        if trimmed == "@variables" {
            in_legacy_vars_block = true;
            continue;
        }

        // Case 2: inside legacy @variables block — rewrite `name = value` as `@name = value`
        if in_legacy_vars_block && is_var_assignment_line(trimmed) {
            let leading_len = body.len() - body.trim_start().len();
            let (indent, rest) = body.split_at(leading_len);
            out.push(format!("{}@{}{}", indent, rest, ending));
            continue;
        }

        // Case 3: separator with block-type `### @<type>` → `### @@<type>`
        if let Some(rewritten) = rewrite_separator_line(body) {
            out.push(format!("{}{}", rewritten, ending));
            continue;
        }

        // Case 4: pilot directive comment `# @x` / `// @x` → `# @@x` / `// @@x`
        if let Some(rewritten) = rewrite_directive_line(body) {
            out.push(format!("{}{}", rewritten, ending));
            continue;
        }

        out.push(format!("{}{}", body, ending));
    }

    out.concat()
}

fn is_separator(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with("###") && (t.len() == 3 || t[3..].starts_with(|c: char| c.is_whitespace())))
        || (t.starts_with("---") && (t.len() == 3 || t[3..].starts_with(|c: char| c.is_whitespace())))
}

fn is_var_assignment_line(trimmed: &str) -> bool {
    // `name = value`  — starts with an identifier, then optional spaces, then `=`
    let mut chars = trimmed.char_indices();
    let Some((_, first)) = chars.next() else { return false; };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    for (i, c) in chars {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            continue;
        }
        let rest = trimmed[i..].trim_start();
        return rest.starts_with('=');
    }
    false
}

fn rewrite_separator_line(line: &str) -> Option<String> {
    let trimmed_start = line.trim_start();
    let leading = &line[..line.len() - trimmed_start.len()];
    let marker_len = if trimmed_start.starts_with("###") {
        3
    } else if trimmed_start.starts_with("---") {
        3
    } else {
        return None;
    };
    let after_marker = &trimmed_start[marker_len..];
    // Must be followed by whitespace to be a real separator
    if !after_marker.is_empty() && !after_marker.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    // Look for `@<type>` (legacy, no `@@`) within the remainder
    let after_trim = after_marker.trim_start();
    let ws_between = &after_marker[..after_marker.len() - after_trim.len()];
    if after_trim.starts_with("@@") {
        return None; // already new syntax
    }
    if !after_trim.starts_with('@') {
        return None;
    }
    let rest = &after_trim[1..]; // after the single @
    let type_end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let type_name = &rest[..type_end];
    if !BLOCK_TYPES.contains(&type_name) {
        return None;
    }
    Some(format!(
        "{}{}{}@@{}{}",
        leading,
        &trimmed_start[..marker_len],
        ws_between,
        type_name,
        &rest[type_end..],
    ))
}

fn rewrite_directive_line(line: &str) -> Option<String> {
    let trimmed_start = line.trim_start();
    let leading = &line[..line.len() - trimmed_start.len()];

    // Determine comment marker
    let (marker, after_marker) = if let Some(r) = trimmed_start.strip_prefix("//") {
        ("//", r)
    } else if let Some(r) = trimmed_start.strip_prefix('#') {
        ("#", r)
    } else {
        return None;
    };

    let after_ws_trim = after_marker.trim_start();
    let between_ws = &after_marker[..after_marker.len() - after_ws_trim.len()];

    if after_ws_trim.starts_with("@@") {
        return None; // already new syntax
    }
    let rest = after_ws_trim.strip_prefix('@')?;
    let name_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    if !PILOT_DIRECTIVES.contains(&name) {
        return None;
    }
    Some(format!(
        "{}{}{}@@{}{}",
        leading,
        marker,
        between_ws,
        name,
        &rest[name_end..],
    ))
}

/// Produce a minimal unified-style diff between two strings, line by line.
/// Fits stdlib only — no external diff crate needed.
pub fn simple_diff(before: &str, after: &str, path: &Path) -> String {
    if before == after {
        return String::new();
    }
    let b: Vec<&str> = before.lines().collect();
    let a: Vec<&str> = after.lines().collect();
    let mut out = String::new();
    out.push_str(&format!("--- {} (legacy)\n", path.display()));
    out.push_str(&format!("+++ {} (migrated)\n", path.display()));
    let max = b.len().max(a.len());
    let mut i = 0;
    while i < max {
        let bl = b.get(i).copied();
        let al = a.get(i).copied();
        if bl == al {
            i += 1;
            continue;
        }
        // Print a small change window
        let start = i.saturating_sub(0);
        let mut end = i;
        while end < max && b.get(end).copied() != a.get(end).copied() {
            end += 1;
        }
        out.push_str(&format!("@@ line {} @@\n", start + 1));
        for j in start..end {
            if let Some(line) = b.get(j) {
                out.push_str(&format!("- {}\n", line));
            }
        }
        for j in start..end {
            if let Some(line) = a.get(j) {
                out.push_str(&format!("+ {}\n", line));
            }
        }
        i = end;
    }
    out
}

/// Run the migrate subcommand on a set of files. Returns exit code semantics:
/// returns `Ok(())` on success, `Err` if any file couldn't be read/written.
pub fn run(files: Vec<PathBuf>, write: bool) -> color_eyre::Result<()> {
    use std::fs;
    let mut total_changed = 0;
    for path in &files {
        let content = fs::read_to_string(path)
            .map_err(|e| color_eyre::eyre::eyre!("{}: {}", path.display(), e))?;
        let migrated = migrate_content(&content);
        if migrated == content {
            println!("unchanged  {}", path.display());
            continue;
        }
        total_changed += 1;
        if write {
            fs::write(path, &migrated)
                .map_err(|e| color_eyre::eyre::eyre!("{}: {}", path.display(), e))?;
            println!("migrated   {}", path.display());
        } else {
            println!("{}", simple_diff(&content, &migrated, path));
        }
    }
    if !write && total_changed > 0 {
        println!();
        println!(
            "{} file(s) would be migrated. Re-run with --write to apply.",
            total_changed
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_block_type_marker() {
        let input = "### @setup Authenticate\nGET https://x/\n";
        let out = migrate_content(input);
        assert!(out.contains("### @@setup Authenticate"));
    }

    #[test]
    fn converts_pilot_directives() {
        let input = "# @assert status == 200\n# @extract tok = $.token\n";
        let out = migrate_content(input);
        assert_eq!(out, "# @@assert status == 200\n# @@extract tok = $.token\n");
    }

    #[test]
    fn converts_slash_slash_comments() {
        let input = "// @assert status == 200\n";
        let out = migrate_content(input);
        assert_eq!(out, "// @@assert status == 200\n");
    }

    #[test]
    fn preserves_bare_rest_client_directives() {
        let input = "@name login\n@description logs in\n@note careful\n";
        let out = migrate_content(input);
        assert_eq!(out, input);
    }

    #[test]
    fn preserves_top_level_var_assignments() {
        let input = "@baseUrl = https://x\n@token = abc\n";
        let out = migrate_content(input);
        assert_eq!(out, input);
    }

    #[test]
    fn flattens_variables_block() {
        let input = "@variables\nname = alice\ntoken = abc\n\nGET https://x/\n";
        let out = migrate_content(input);
        assert!(!out.contains("@variables"));
        assert!(out.contains("@name = alice"));
        assert!(out.contains("@token = abc"));
        assert!(out.contains("GET https://x/"));
    }

    #[test]
    fn variables_block_with_comments_and_blanks() {
        let input = "@variables\n# creds\nname = alice\n\n# env\nhost = x\n\n### @test Hi\nGET /\n";
        let out = migrate_content(input);
        assert!(out.contains("@name = alice"));
        assert!(out.contains("@host = x"));
        assert!(out.contains("# creds"));
        assert!(out.contains("### @@test Hi"));
    }

    #[test]
    fn idempotent() {
        let input = "@variables\nname = alice\n\n### @setup S\n# @assert status == 200\n";
        let once = migrate_content(input);
        let twice = migrate_content(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn leaves_new_syntax_alone() {
        let input = "@name = alice\n\n### @@test Hi\n# @@assert status == 200\n";
        let out = migrate_content(input);
        assert_eq!(out, input);
    }

    #[test]
    fn preserves_crlf() {
        let input = "# @assert status == 200\r\n";
        let out = migrate_content(input);
        assert_eq!(out, "# @@assert status == 200\r\n");
    }

    #[test]
    fn does_not_touch_body_lines() {
        let input = "POST /x\n\n{\n  \"@type\": \"person\"\n}\n";
        let out = migrate_content(input);
        assert_eq!(out, input);
    }
}
