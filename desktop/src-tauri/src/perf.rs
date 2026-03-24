use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── Structs ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextSpan {
    pub class: String,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HighlightedLine {
    pub spans: Vec<TextSpan>,
    pub raw: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonTreeNode {
    pub key: Option<String>,
    pub node_type: String,
    pub value_preview: Option<String>,
    pub children: Vec<JsonTreeNode>,
    pub child_count: usize,
    pub depth: u32,
    pub is_last: bool,
    pub path: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CharSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum DiffOp {
    #[serde(rename = "same")]
    Same { line: String },
    #[serde(rename = "add")]
    Add { line: String },
    #[serde(rename = "remove")]
    Remove { line: String },
    #[serde(rename = "change")]
    Change {
        left: String,
        right: String,
        left_highlights: Vec<CharSpan>,
        right_highlights: Vec<CharSpan>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiffResult {
    pub ops: Vec<DiffOp>,
    pub similarity: f64,
    pub added_count: u32,
    pub removed_count: u32,
    pub changed_count: u32,
    pub is_json: bool,
}

// ─── Format detection ──────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Format {
    Json,
    Xml,
    Yaml,
    Csv,
    Plain,
}

fn detect_format(content_type: &str) -> Format {
    let ct = content_type.to_lowercase();
    if ct.contains("json") {
        Format::Json
    } else if ct.contains("xml") || ct.contains("html") {
        Format::Xml
    } else if ct.contains("yaml") || ct.contains("yml") {
        Format::Yaml
    } else if ct.contains("csv") {
        Format::Csv
    } else {
        Format::Plain
    }
}

// ─── JSON helpers ──────────────────────────────────────────────────────────────

fn sort_json(val: &Value) -> Value {
    match val {
        Value::Object(map) => {
            let sorted: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_json(v)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect();
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json).collect()),
        other => other.clone(),
    }
}

fn pretty_json(body: &str) -> Result<String, String> {
    let val: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let sorted = sort_json(&val);
    serde_json::to_string_pretty(&sorted).map_err(|e| e.to_string())
}

fn highlight_json_line(line: &str) -> Vec<TextSpan> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    let len = bytes.len();

    while i < len {
        match bytes[i] {
            b' ' | b'\t' | b'\r' => {
                let start = i;
                while i < len && matches!(bytes[i], b' ' | b'\t' | b'\r') {
                    i += 1;
                }
                spans.push(TextSpan {
                    class: String::new(),
                    text: line[start..i].to_string(),
                });
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 1;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                let text = line[start..i].to_string();
                let mut j = i;
                while j < len && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                let class = if j < len && bytes[j] == b':' {
                    "syntax-key"
                } else {
                    "syntax-string"
                };
                spans.push(TextSpan {
                    class: class.to_string(),
                    text,
                });
            }
            b':' => {
                let start = i;
                i += 1;
                while i < len && bytes[i] == b' ' {
                    i += 1;
                }
                spans.push(TextSpan {
                    class: String::new(),
                    text: line[start..i].to_string(),
                });
            }
            b',' => {
                spans.push(TextSpan {
                    class: String::new(),
                    text: ",".to_string(),
                });
                i += 1;
            }
            b'{' | b'}' | b'[' | b']' => {
                spans.push(TextSpan {
                    class: "syntax-bracket".to_string(),
                    text: (bytes[i] as char).to_string(),
                });
                i += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                while i < len
                    && matches!(
                        bytes[i],
                        b'0'..=b'9' | b'.' | b'-' | b'+' | b'e' | b'E'
                    )
                {
                    i += 1;
                }
                spans.push(TextSpan {
                    class: "syntax-number".to_string(),
                    text: line[start..i].to_string(),
                });
            }
            b't' if line.len() >= i + 4 && &line[i..i + 4] == "true" => {
                spans.push(TextSpan {
                    class: "syntax-boolean".to_string(),
                    text: "true".to_string(),
                });
                i += 4;
            }
            b'f' if line.len() >= i + 5 && &line[i..i + 5] == "false" => {
                spans.push(TextSpan {
                    class: "syntax-boolean".to_string(),
                    text: "false".to_string(),
                });
                i += 5;
            }
            b'n' if line.len() >= i + 4 && &line[i..i + 4] == "null" => {
                spans.push(TextSpan {
                    class: "syntax-null".to_string(),
                    text: "null".to_string(),
                });
                i += 4;
            }
            _ => {
                spans.push(TextSpan {
                    class: String::new(),
                    text: String::from(bytes[i] as char),
                });
                i += 1;
            }
        }
    }
    spans
}

// ─── XML helpers ───────────────────────────────────────────────────────────────

fn pretty_xml(body: &str) -> String {
    let mut result = String::with_capacity(body.len() * 2);
    let mut indent: usize = 0;
    let mut i = 0;
    let bytes = body.as_bytes();
    let len = bytes.len();

    while i < len {
        // skip whitespace between tags
        while i < len && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i >= len {
            break;
        }

        if bytes[i] == b'<' {
            let tag_start = i;
            // find end of tag, respecting quotes inside attributes
            let mut in_quote: Option<u8> = None;
            i += 1;
            while i < len {
                if let Some(q) = in_quote {
                    if bytes[i] == q {
                        in_quote = None;
                    }
                } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                    in_quote = Some(bytes[i]);
                } else if bytes[i] == b'>' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let tag = &body[tag_start..i];

            if tag.starts_with("</") {
                indent = indent.saturating_sub(1);
                push_indent(&mut result, indent);
                result.push_str(tag);
                result.push('\n');
            } else if tag.ends_with("/>")
                || tag.starts_with("<?")
                || tag.starts_with("<!")
            {
                push_indent(&mut result, indent);
                result.push_str(tag);
                result.push('\n');
            } else {
                push_indent(&mut result, indent);
                result.push_str(tag);
                result.push('\n');
                indent += 1;
            }
        } else {
            let start = i;
            while i < len && bytes[i] != b'<' {
                i += 1;
            }
            let text = body[start..i].trim();
            if !text.is_empty() {
                push_indent(&mut result, indent);
                result.push_str(text);
                result.push('\n');
            }
        }
    }
    result
}

fn push_indent(buf: &mut String, level: usize) {
    for _ in 0..level {
        buf.push_str("  ");
    }
}

fn highlight_xml_line(line: &str) -> Vec<TextSpan> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    let len = bytes.len();

    while i < len {
        match bytes[i] {
            b' ' | b'\t' => {
                let start = i;
                while i < len && matches!(bytes[i], b' ' | b'\t') {
                    i += 1;
                }
                spans.push(TextSpan {
                    class: String::new(),
                    text: line[start..i].to_string(),
                });
            }
            b'<' => {
                if i + 3 < len && &line[i..i + 4] == "<!--" {
                    let end = line[i..].find("-->").map(|p| i + p + 3).unwrap_or(len);
                    spans.push(TextSpan {
                        class: "syntax-comment".to_string(),
                        text: line[i..end].to_string(),
                    });
                    i = end;
                } else {
                    spans.push(TextSpan {
                        class: "syntax-bracket".to_string(),
                        text: "<".to_string(),
                    });
                    i += 1;
                    if i < len && bytes[i] == b'/' {
                        spans.push(TextSpan {
                            class: "syntax-bracket".to_string(),
                            text: "/".to_string(),
                        });
                        i += 1;
                    }
                    if i < len && bytes[i] == b'?' {
                        spans.push(TextSpan {
                            class: "syntax-bracket".to_string(),
                            text: "?".to_string(),
                        });
                        i += 1;
                    }
                    let start = i;
                    while i < len && !matches!(bytes[i], b' ' | b'\t' | b'>' | b'/') {
                        i += 1;
                    }
                    if i > start {
                        spans.push(TextSpan {
                            class: "syntax-tag".to_string(),
                            text: line[start..i].to_string(),
                        });
                    }
                    while i < len && bytes[i] != b'>' {
                        if matches!(bytes[i], b' ' | b'\t') {
                            let s = i;
                            while i < len && matches!(bytes[i], b' ' | b'\t') {
                                i += 1;
                            }
                            spans.push(TextSpan {
                                class: String::new(),
                                text: line[s..i].to_string(),
                            });
                        } else if bytes[i] == b'/' || bytes[i] == b'?' {
                            spans.push(TextSpan {
                                class: "syntax-bracket".to_string(),
                                text: String::from(bytes[i] as char),
                            });
                            i += 1;
                        } else if bytes[i] == b'=' {
                            spans.push(TextSpan {
                                class: String::new(),
                                text: "=".to_string(),
                            });
                            i += 1;
                        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                            let q = bytes[i];
                            let s = i;
                            i += 1;
                            while i < len && bytes[i] != q {
                                i += 1;
                            }
                            if i < len {
                                i += 1;
                            }
                            spans.push(TextSpan {
                                class: "syntax-string".to_string(),
                                text: line[s..i].to_string(),
                            });
                        } else {
                            let s = i;
                            while i < len
                                && !matches!(bytes[i], b' ' | b'\t' | b'=' | b'>' | b'/')
                            {
                                i += 1;
                            }
                            spans.push(TextSpan {
                                class: "syntax-attr".to_string(),
                                text: line[s..i].to_string(),
                            });
                        }
                    }
                    if i < len && bytes[i] == b'>' {
                        spans.push(TextSpan {
                            class: "syntax-bracket".to_string(),
                            text: ">".to_string(),
                        });
                        i += 1;
                    }
                }
            }
            _ => {
                let start = i;
                while i < len && bytes[i] != b'<' {
                    i += 1;
                }
                spans.push(TextSpan {
                    class: String::new(),
                    text: line[start..i].to_string(),
                });
            }
        }
    }
    spans
}

// ─── YAML helpers ──────────────────────────────────────────────────────────────

fn highlight_yaml_line(line: &str) -> Vec<TextSpan> {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let mut spans = Vec::new();

    if indent_len > 0 {
        spans.push(TextSpan {
            class: String::new(),
            text: line[..indent_len].to_string(),
        });
    }

    if trimmed.starts_with('#') {
        spans.push(TextSpan {
            class: "syntax-comment".to_string(),
            text: trimmed.to_string(),
        });
        return spans;
    }

    if trimmed.starts_with("- ") {
        spans.push(TextSpan {
            class: "syntax-bracket".to_string(),
            text: "- ".to_string(),
        });
        spans.extend(highlight_yaml_value(&trimmed[2..]));
        return spans;
    }

    if let Some(colon_pos) = trimmed.find(": ") {
        let key = &trimmed[..colon_pos];
        if !key.starts_with('"') && !key.starts_with('\'') {
            spans.push(TextSpan {
                class: "syntax-key".to_string(),
                text: key.to_string(),
            });
            spans.push(TextSpan {
                class: String::new(),
                text: ": ".to_string(),
            });
            let val = &trimmed[colon_pos + 2..];
            spans.extend(highlight_yaml_value(val));
            return spans;
        }
    } else if trimmed.ends_with(':') && !trimmed.starts_with('"') {
        spans.push(TextSpan {
            class: "syntax-key".to_string(),
            text: trimmed.to_string(),
        });
        return spans;
    }

    spans.extend(highlight_yaml_value(trimmed));
    spans
}

fn highlight_yaml_value(val: &str) -> Vec<TextSpan> {
    let trimmed = val.trim_end();
    if trimmed == "true" || trimmed == "false" {
        return vec![TextSpan {
            class: "syntax-boolean".to_string(),
            text: val.to_string(),
        }];
    }
    if trimmed == "null" || trimmed == "~" {
        return vec![TextSpan {
            class: "syntax-null".to_string(),
            text: val.to_string(),
        }];
    }
    if !trimmed.is_empty() && trimmed.parse::<f64>().is_ok() {
        return vec![TextSpan {
            class: "syntax-number".to_string(),
            text: val.to_string(),
        }];
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        return vec![TextSpan {
            class: "syntax-string".to_string(),
            text: val.to_string(),
        }];
    }
    vec![TextSpan {
        class: "syntax-string".to_string(),
        text: val.to_string(),
    }]
}

// ─── CSV helpers ───────────────────────────────────────────────────────────────

fn highlight_csv_line(line: &str, is_header: bool) -> Vec<TextSpan> {
    if is_header {
        vec![TextSpan {
            class: "syntax-key".to_string(),
            text: line.to_string(),
        }]
    } else {
        vec![TextSpan {
            class: String::new(),
            text: line.to_string(),
        }]
    }
}

// ─── format_body implementation ────────────────────────────────────────────────

pub fn format_body_inner(body: &str, content_type: &str) -> Result<Vec<HighlightedLine>, String> {
    let format = detect_format(content_type);

    let formatted = match format {
        Format::Json => pretty_json(body).unwrap_or_else(|_| body.to_string()),
        Format::Xml => pretty_xml(body),
        _ => body.to_string(),
    };

    let lines: Vec<&str> = formatted.lines().collect();
    let threshold = 1000;

    let highlighted: Vec<HighlightedLine> = if lines.len() > threshold {
        let num_threads = rayon::current_num_threads().max(1);
        let chunk_size = (lines.len() / num_threads).max(100);
        lines
            .par_chunks(chunk_size)
            .enumerate()
            .flat_map(|(chunk_idx, chunk)| {
                chunk
                    .iter()
                    .enumerate()
                    .map(|(i, &line)| {
                        let global_idx = chunk_idx * chunk_size + i;
                        let spans = match format {
                            Format::Json => highlight_json_line(line),
                            Format::Xml => highlight_xml_line(line),
                            Format::Yaml => highlight_yaml_line(line),
                            Format::Csv => highlight_csv_line(line, global_idx == 0),
                            Format::Plain => vec![TextSpan {
                                class: String::new(),
                                text: line.to_string(),
                            }],
                        };
                        HighlightedLine {
                            spans,
                            raw: line.to_string(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        lines
            .iter()
            .enumerate()
            .map(|(i, &line)| {
                let spans = match format {
                    Format::Json => highlight_json_line(line),
                    Format::Xml => highlight_xml_line(line),
                    Format::Yaml => highlight_yaml_line(line),
                    Format::Csv => highlight_csv_line(line, i == 0),
                    Format::Plain => vec![TextSpan {
                        class: String::new(),
                        text: line.to_string(),
                    }],
                };
                HighlightedLine {
                    spans,
                    raw: line.to_string(),
                }
            })
            .collect()
    };

    Ok(highlighted)
}

// ─── JSON tree builder ─────────────────────────────────────────────────────────

pub fn build_tree_node(
    val: &Value,
    key: Option<String>,
    depth: u32,
    max_depth: u32,
    max_children: u32,
    path: Vec<String>,
    is_last: bool,
    use_rayon: bool,
) -> JsonTreeNode {
    match val {
        Value::Object(map) => {
            let child_count = map.len();
            let children = if depth < max_depth {
                let entries: Vec<(&String, &Value)> = map.iter().collect();
                let take = (max_children as usize).min(entries.len());
                let child_rayon = use_rayon && depth <= 1 && take > 10;

                let mut children: Vec<JsonTreeNode> = if child_rayon {
                    entries[..take]
                        .par_iter()
                        .enumerate()
                        .map(|(i, (k, v))| {
                            let mut child_path = path.clone();
                            child_path.push((*k).clone());
                            build_tree_node(
                                v,
                                Some((*k).clone()),
                                depth + 1,
                                max_depth,
                                max_children,
                                child_path,
                                i == take - 1 && entries.len() <= max_children as usize,
                                false,
                            )
                        })
                        .collect()
                } else {
                    entries[..take]
                        .iter()
                        .enumerate()
                        .map(|(i, (k, v))| {
                            let mut child_path = path.clone();
                            child_path.push((*k).clone());
                            build_tree_node(
                                v,
                                Some((*k).clone()),
                                depth + 1,
                                max_depth,
                                max_children,
                                child_path,
                                i == take - 1 && entries.len() <= max_children as usize,
                                false,
                            )
                        })
                        .collect()
                };

                if entries.len() > max_children as usize {
                    children.push(JsonTreeNode {
                        key: None,
                        node_type: "truncated".to_string(),
                        value_preview: Some(format!(
                            "... {} more items",
                            entries.len() - take
                        )),
                        children: vec![],
                        child_count: 0,
                        depth: depth + 1,
                        is_last: true,
                        path: path.clone(),
                    });
                }
                children
            } else {
                vec![]
            };

            JsonTreeNode {
                key,
                node_type: "object".to_string(),
                value_preview: Some(format!("{{{} keys}}", child_count)),
                children,
                child_count,
                depth,
                is_last,
                path,
            }
        }
        Value::Array(arr) => {
            let child_count = arr.len();
            let children = if depth < max_depth {
                let take = (max_children as usize).min(arr.len());
                let child_rayon = use_rayon && depth <= 1 && take > 10;

                let mut children: Vec<JsonTreeNode> = if child_rayon {
                    arr[..take]
                        .par_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let mut child_path = path.clone();
                            child_path.push(i.to_string());
                            build_tree_node(
                                v,
                                Some(i.to_string()),
                                depth + 1,
                                max_depth,
                                max_children,
                                child_path,
                                i == take - 1 && arr.len() <= max_children as usize,
                                false,
                            )
                        })
                        .collect()
                } else {
                    arr[..take]
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let mut child_path = path.clone();
                            child_path.push(i.to_string());
                            build_tree_node(
                                v,
                                Some(i.to_string()),
                                depth + 1,
                                max_depth,
                                max_children,
                                child_path,
                                i == take - 1 && arr.len() <= max_children as usize,
                                false,
                            )
                        })
                        .collect()
                };

                if arr.len() > max_children as usize {
                    children.push(JsonTreeNode {
                        key: None,
                        node_type: "truncated".to_string(),
                        value_preview: Some(format!(
                            "... {} more items",
                            arr.len() - take
                        )),
                        children: vec![],
                        child_count: 0,
                        depth: depth + 1,
                        is_last: true,
                        path: path.clone(),
                    });
                }
                children
            } else {
                vec![]
            };

            JsonTreeNode {
                key,
                node_type: "array".to_string(),
                value_preview: Some(format!("[{} items]", child_count)),
                children,
                child_count,
                depth,
                is_last,
                path,
            }
        }
        Value::String(s) => {
            let preview = if s.len() > 100 {
                format!("\"{}...\"", &s[..s.floor_char_boundary(100)])
            } else {
                format!("\"{}\"", s)
            };
            JsonTreeNode {
                key,
                node_type: "string".to_string(),
                value_preview: Some(preview),
                children: vec![],
                child_count: 0,
                depth,
                is_last,
                path,
            }
        }
        Value::Number(n) => JsonTreeNode {
            key,
            node_type: "number".to_string(),
            value_preview: Some(n.to_string()),
            children: vec![],
            child_count: 0,
            depth,
            is_last,
            path,
        },
        Value::Bool(b) => JsonTreeNode {
            key,
            node_type: "boolean".to_string(),
            value_preview: Some(b.to_string()),
            children: vec![],
            child_count: 0,
            depth,
            is_last,
            path,
        },
        Value::Null => JsonTreeNode {
            key,
            node_type: "null".to_string(),
            value_preview: Some("null".to_string()),
            children: vec![],
            child_count: 0,
            depth,
            is_last,
            path,
        },
    }
}

pub fn navigate_json<'a>(val: &'a Value, path: &[String]) -> Result<&'a Value, String> {
    let mut current = val;
    for segment in path {
        match current {
            Value::Object(map) => {
                current = map
                    .get(segment)
                    .ok_or_else(|| format!("Key '{}' not found", segment))?;
            }
            Value::Array(arr) => {
                let idx: usize = segment
                    .parse()
                    .map_err(|_| format!("Invalid array index '{}'", segment))?;
                current = arr
                    .get(idx)
                    .ok_or_else(|| format!("Index {} out of bounds", idx))?;
            }
            _ => {
                return Err(format!("Cannot navigate into primitive at '{}'", segment));
            }
        }
    }
    Ok(current)
}

// ─── Myers diff (O(ND) algorithm) ──────────────────────────────────────────────

#[allow(dead_code)]
enum RawEdit {
    Same(usize, usize), // (index in a, index in b)
    Insert(usize),       // index in b
    Delete(usize),       // index in a
}

fn v_idx(k: isize, offset: usize) -> usize {
    (k + offset as isize) as usize
}

fn myers_diff(a_lines: &[&str], b_lines: &[&str]) -> Vec<RawEdit> {
    let n = a_lines.len();
    let m = b_lines.len();

    if n == 0 && m == 0 {
        return vec![];
    }
    if n == 0 {
        return (0..m).map(RawEdit::Insert).collect();
    }
    if m == 0 {
        return (0..n).map(RawEdit::Delete).collect();
    }

    let max = n + m;
    let offset = max;
    let v_size = 2 * max + 1;

    // Limit trace memory: if edit distance would be huge, fall back to simple diff
    let max_d = max.min(10_000);

    let mut v = vec![0usize; v_size];
    let mut trace: Vec<Vec<usize>> = Vec::with_capacity(max_d + 1);

    let mut found_d: Option<usize> = None;

    for d in 0..=max_d {
        trace.push(v.clone());
        let di = d as isize;

        let mut k = -di;
        while k <= di {
            let x = if k == -di
                || (k != di && v[v_idx(k - 1, offset)] < v[v_idx(k + 1, offset)])
            {
                v[v_idx(k + 1, offset)]
            } else {
                v[v_idx(k - 1, offset)] + 1
            };

            let mut x = x;
            let mut y = (x as isize - k) as usize;

            while x < n && y < m && a_lines[x] == b_lines[y] {
                x += 1;
                y += 1;
            }

            v[v_idx(k, offset)] = x;

            if x >= n && y >= m {
                found_d = Some(d);
                break;
            }

            k += 2;
        }

        if found_d.is_some() {
            break;
        }
    }

    let final_d = match found_d {
        Some(d) => d,
        None => {
            // Exceeded max_d — fall back to "delete all a, insert all b"
            let mut edits: Vec<RawEdit> = (0..n).map(RawEdit::Delete).collect();
            edits.extend((0..m).map(RawEdit::Insert));
            return edits;
        }
    };

    // Backtrack
    let mut x = n;
    let mut y = m;
    let mut edits: Vec<RawEdit> = Vec::with_capacity(n + m);

    for d_val in (1..=final_d).rev() {
        let v_prev = &trace[d_val];
        let k = x as isize - y as isize;
        let d_i = d_val as isize;

        let is_down = k == -d_i
            || (k != d_i && v_prev[v_idx(k - 1, offset)] < v_prev[v_idx(k + 1, offset)]);

        let prev_k = if is_down { k + 1 } else { k - 1 };
        let prev_x = v_prev[v_idx(prev_k, offset)];
        let prev_y = (prev_x as isize - prev_k) as usize;

        // Diagonal (snake) after the edit
        let (snake_x, snake_y) = if is_down {
            (prev_x, prev_y + 1)
        } else {
            (prev_x + 1, prev_y)
        };

        while x > snake_x && y > snake_y {
            x -= 1;
            y -= 1;
            edits.push(RawEdit::Same(x, y));
        }

        if is_down {
            edits.push(RawEdit::Insert(prev_y));
        } else {
            edits.push(RawEdit::Delete(prev_x));
        }

        x = prev_x;
        y = prev_y;
    }

    // d=0: remaining diagonals
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        edits.push(RawEdit::Same(x, y));
    }

    edits.reverse();
    edits
}

// ─── Char-level diff ───────────────────────────────────────────────────────────

fn char_level_diff(left: &str, right: &str) -> (Vec<CharSpan>, Vec<CharSpan>) {
    if left == right {
        return (vec![], vec![]);
    }

    let l_bytes = left.as_bytes();
    let r_bytes = right.as_bytes();

    // Common prefix (byte level, safe for UTF-8 boundary: we verify below)
    let prefix_len = l_bytes
        .iter()
        .zip(r_bytes.iter())
        .take_while(|(a, b)| a == b)
        .count();
    // Adjust to char boundary
    let prefix_len = safe_char_boundary(left, prefix_len);

    // Common suffix (avoiding overlap with prefix)
    let l_rem = l_bytes.len() - prefix_len;
    let r_rem = r_bytes.len() - prefix_len;
    let suffix_len = l_bytes[prefix_len..]
        .iter()
        .rev()
        .zip(r_bytes[prefix_len..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(l_rem)
        .min(r_rem);
    let suffix_len_l = safe_suffix_boundary(left, suffix_len);
    let suffix_len_r = safe_suffix_boundary(right, suffix_len);

    let l_start = prefix_len;
    let l_end = left.len() - suffix_len_l;
    let r_start = prefix_len;
    let r_end = right.len() - suffix_len_r;

    let left_hl = if l_start < l_end {
        vec![CharSpan {
            start: l_start,
            end: l_end,
        }]
    } else {
        vec![]
    };
    let right_hl = if r_start < r_end {
        vec![CharSpan {
            start: r_start,
            end: r_end,
        }]
    } else {
        vec![]
    };
    (left_hl, right_hl)
}

fn safe_char_boundary(s: &str, byte_pos: usize) -> usize {
    let mut pos = byte_pos;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn safe_suffix_boundary(s: &str, suffix_bytes: usize) -> usize {
    let mut pos = suffix_bytes;
    let start = s.len().saturating_sub(pos);
    let mut start = start;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
        if pos > 0 {
            pos -= 1;
        }
    }
    pos
}

// ─── Diff grouping ─────────────────────────────────────────────────────────────

fn build_diff_result(
    a_lines: &[&str],
    b_lines: &[&str],
    edits: Vec<RawEdit>,
    is_json: bool,
) -> DiffResult {
    // First pass: group into ops, collect change pairs for parallel char diff
    let mut ops: Vec<DiffOp> = Vec::new();
    let mut change_indices: Vec<usize> = Vec::new();
    let mut change_pairs: Vec<(String, String)> = Vec::new();

    let mut i = 0;
    while i < edits.len() {
        match &edits[i] {
            RawEdit::Same(ai, _) => {
                ops.push(DiffOp::Same {
                    line: a_lines[*ai].to_string(),
                });
                i += 1;
            }
            RawEdit::Delete(_) => {
                let mut deletes = Vec::new();
                while i < edits.len() {
                    if let RawEdit::Delete(ai) = &edits[i] {
                        deletes.push(*ai);
                        i += 1;
                    } else {
                        break;
                    }
                }
                let mut inserts = Vec::new();
                while i < edits.len() {
                    if let RawEdit::Insert(bi) = &edits[i] {
                        inserts.push(*bi);
                        i += 1;
                    } else {
                        break;
                    }
                }
                let pairs = deletes.len().min(inserts.len());
                for j in 0..pairs {
                    let left = a_lines[deletes[j]].to_string();
                    let right = b_lines[inserts[j]].to_string();
                    change_indices.push(ops.len());
                    change_pairs.push((left.clone(), right.clone()));
                    ops.push(DiffOp::Change {
                        left,
                        right,
                        left_highlights: vec![],
                        right_highlights: vec![],
                    });
                }
                for j in pairs..deletes.len() {
                    ops.push(DiffOp::Remove {
                        line: a_lines[deletes[j]].to_string(),
                    });
                }
                for j in pairs..inserts.len() {
                    ops.push(DiffOp::Add {
                        line: b_lines[inserts[j]].to_string(),
                    });
                }
            }
            RawEdit::Insert(bi) => {
                ops.push(DiffOp::Add {
                    line: b_lines[*bi].to_string(),
                });
                i += 1;
            }
        }
    }

    // Compute char-level diffs (parallel for large sets)
    let char_diffs: Vec<(Vec<CharSpan>, Vec<CharSpan>)> = if change_pairs.len() > 50 {
        change_pairs
            .par_iter()
            .map(|(l, r)| char_level_diff(l, r))
            .collect()
    } else {
        change_pairs
            .iter()
            .map(|(l, r)| char_level_diff(l, r))
            .collect()
    };

    for (idx, (lh, rh)) in change_indices.into_iter().zip(char_diffs) {
        if let DiffOp::Change {
            ref mut left_highlights,
            ref mut right_highlights,
            ..
        } = ops[idx]
        {
            *left_highlights = lh;
            *right_highlights = rh;
        }
    }

    // Stats
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut changed = 0u32;
    let mut same_count = 0u32;
    for op in &ops {
        match op {
            DiffOp::Same { .. } => same_count += 1,
            DiffOp::Add { .. } => added += 1,
            DiffOp::Remove { .. } => removed += 1,
            DiffOp::Change { .. } => changed += 1,
        }
    }
    let total = (same_count + added + removed + changed) as f64;
    let similarity = if total > 0.0 {
        same_count as f64 / total
    } else {
        1.0
    };

    DiffResult {
        ops,
        similarity,
        added_count: added,
        removed_count: removed,
        changed_count: changed,
        is_json,
    }
}

// ─── XML normalization (sort attributes) ───────────────────────────────────────

fn sort_xml_attributes(xml: &str) -> String {
    let mut result = String::with_capacity(xml.len());
    for line in xml.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('<')
            && !trimmed.starts_with("</")
            && !trimmed.starts_with("<!--")
            && !trimmed.starts_with("<?")
            && !trimmed.starts_with("<!")
        {
            if let Some(sorted) = try_sort_tag_attrs(line) {
                result.push_str(&sorted);
            } else {
                result.push_str(line);
            }
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    result
}

fn try_sort_tag_attrs(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    let rest = trimmed.strip_prefix('<')?;
    let name_end = rest.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
    let tag_name = &rest[..name_end];
    let mut remaining = rest[name_end..].trim_start();

    let mut attrs: Vec<(String, String)> = Vec::new();

    while !remaining.is_empty()
        && !remaining.starts_with('>')
        && !remaining.starts_with("/>")
    {
        let eq = remaining.find('=')?;
        let attr_name = remaining[..eq].trim().to_string();
        remaining = remaining[eq + 1..].trim_start();

        let quote = remaining.as_bytes().first()?;
        if *quote != b'"' && *quote != b'\'' {
            return None;
        }
        let q = *quote as char;
        let end = remaining[1..].find(q)? + 2;
        let attr_val = remaining[..end].to_string();
        remaining = remaining[end..].trim_start();
        attrs.push((attr_name, attr_val));
    }

    attrs.sort_by(|a, b| a.0.cmp(&b.0));

    let closing = remaining;
    let mut out = format!("{}<{}", indent, tag_name);
    for (name, val) in &attrs {
        out.push(' ');
        out.push_str(name);
        out.push('=');
        out.push_str(val);
    }
    if !closing.is_empty() {
        out.push_str(closing);
    }
    Some(out)
}

// ─── Sort and normalize ────────────────────────────────────────────────────────

pub fn sort_and_normalize_inner(body: &str, content_type: &str) -> Result<String, String> {
    match detect_format(content_type) {
        Format::Json => pretty_json(body),
        Format::Xml => Ok(sort_xml_attributes(&pretty_xml(body))),
        _ => Ok(body.to_string()),
    }
}

// ─── Diff inner ────────────────────────────────────────────────────────────────

pub fn compute_diff_inner(text_a: &str, text_b: &str, content_type: &str) -> Result<DiffResult, String> {
    let format = detect_format(content_type);
    let is_json = matches!(format, Format::Json);

    let norm_a = match format {
        Format::Json => pretty_json(text_a).unwrap_or_else(|_| text_a.to_string()),
        Format::Xml => sort_xml_attributes(&pretty_xml(text_a)),
        _ => text_a.to_string(),
    };
    let norm_b = match format {
        Format::Json => pretty_json(text_b).unwrap_or_else(|_| text_b.to_string()),
        Format::Xml => sort_xml_attributes(&pretty_xml(text_b)),
        _ => text_b.to_string(),
    };

    let a_lines: Vec<&str> = norm_a.lines().collect();
    let b_lines: Vec<&str> = norm_b.lines().collect();

    let edits = myers_diff(&a_lines, &b_lines);
    Ok(build_diff_result(&a_lines, &b_lines, edits, is_json))
}

// ─── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn format_body(
    body: String,
    content_type: String,
) -> Result<Vec<HighlightedLine>, String> {
    tokio::task::spawn_blocking(move || format_body_inner(&body, &content_type))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn build_json_tree(
    json: String,
    max_depth: Option<u32>,
    max_children: Option<u32>,
) -> Result<JsonTreeNode, String> {
    tokio::task::spawn_blocking(move || {
        let val: Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let md = max_depth.unwrap_or(3);
        let mc = max_children.unwrap_or(100);
        Ok(build_tree_node(&val, None, 0, md, mc, vec![], true, true))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn expand_json_node(
    json: String,
    path: Vec<String>,
    max_depth: Option<u32>,
    max_children: Option<u32>,
) -> Result<JsonTreeNode, String> {
    tokio::task::spawn_blocking(move || {
        let val: Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let md = max_depth.unwrap_or(3);
        let mc = max_children.unwrap_or(100);
        let target = navigate_json(&val, &path)?;
        let key = path.last().cloned();
        Ok(build_tree_node(
            target,
            key,
            0,
            md,
            mc,
            path,
            true,
            true,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn compute_diff(
    text_a: String,
    text_b: String,
    content_type: String,
) -> Result<DiffResult, String> {
    tokio::task::spawn_blocking(move || compute_diff_inner(&text_a, &text_b, &content_type))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn sort_and_normalize(
    body: String,
    content_type: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || sort_and_normalize_inner(&body, &content_type))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Helper ────────────────────────────────────────────────────────────────

    fn spans_text(lines: &[HighlightedLine]) -> String {
        lines.iter().map(|l| l.raw.as_str()).collect::<Vec<_>>().join("\n")
    }

    fn has_span_class(lines: &[HighlightedLine], class: &str) -> bool {
        lines.iter().any(|l| l.spans.iter().any(|s| s.class == class))
    }

    // ─── format_body tests ─────────────────────────────────────────────────────

    #[test]
    fn test_format_json_basic() {
        let body = r#"{"name":"Alice","age":30}"#;
        let result = format_body_inner(body, "application/json").unwrap();
        assert!(has_span_class(&result, "syntax-key"));
        assert!(has_span_class(&result, "syntax-string"));
        assert!(has_span_class(&result, "syntax-number"));
    }

    #[test]
    fn test_format_json_nested() {
        let body = r#"{"user":{"name":"Bob","tags":["a","b"]}}"#;
        let result = format_body_inner(body, "application/json").unwrap();
        assert!(has_span_class(&result, "syntax-bracket"));
        assert!(has_span_class(&result, "syntax-key"));
        assert!(result.len() > 3); // pretty-printed produces multiple lines
    }

    #[test]
    fn test_format_json_sorts_keys() {
        let body = r#"{"z":1,"a":2,"m":3}"#;
        let result = format_body_inner(body, "application/json").unwrap();
        let text = spans_text(&result);
        let pos_a = text.find("\"a\"").unwrap();
        let pos_m = text.find("\"m\"").unwrap();
        let pos_z = text.find("\"z\"").unwrap();
        assert!(pos_a < pos_m);
        assert!(pos_m < pos_z);
    }

    #[test]
    fn test_format_json_empty_object() {
        let result = format_body_inner("{}", "application/json").unwrap();
        assert!(!result.is_empty());
        let text = spans_text(&result);
        assert!(text.contains('{'));
        assert!(text.contains('}'));
    }

    #[test]
    fn test_format_json_empty_array() {
        let result = format_body_inner("[]", "application/json").unwrap();
        let text = spans_text(&result);
        assert!(text.contains('['));
        assert!(text.contains(']'));
    }

    #[test]
    fn test_format_json_null() {
        let result = format_body_inner("null", "application/json").unwrap();
        assert!(has_span_class(&result, "syntax-null"));
    }

    #[test]
    fn test_format_json_large() {
        let mut obj = serde_json::Map::new();
        for i in 0..1500 {
            obj.insert(format!("key_{:04}", i), Value::Number(i.into()));
        }
        let json = serde_json::to_string(&Value::Object(obj)).unwrap();
        let result = format_body_inner(&json, "application/json").unwrap();
        // 1500 keys → well above the 1000-line Rayon threshold after pretty-printing
        assert!(result.len() > 1000);
    }

    #[test]
    fn test_format_xml_basic() {
        let xml = r#"<root><item id="1">Hello</item></root>"#;
        let result = format_body_inner(xml, "application/xml").unwrap();
        assert!(has_span_class(&result, "syntax-tag"));
        assert!(has_span_class(&result, "syntax-bracket"));
    }

    #[test]
    fn test_format_yaml_basic() {
        let yaml = "name: Alice\nage: 30\n";
        let result = format_body_inner(yaml, "application/yaml").unwrap();
        assert!(has_span_class(&result, "syntax-key"));
        assert!(has_span_class(&result, "syntax-number"));
    }

    #[test]
    fn test_format_csv_basic() {
        let csv = "name,age\nAlice,30\nBob,25\n";
        let result = format_body_inner(csv, "text/csv").unwrap();
        // First line should be header with syntax-key class
        assert!(result[0].spans.iter().any(|s| s.class == "syntax-key"));
    }

    #[test]
    fn test_format_plain_text() {
        let text = "Hello, world!\nLine two.";
        let result = format_body_inner(text, "text/plain").unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].raw, "Hello, world!");
        // Plain text spans have empty class
        assert!(result[0].spans.iter().all(|s| s.class.is_empty()));
    }

    #[test]
    fn test_format_empty_body() {
        let result = format_body_inner("", "application/json").unwrap();
        // Empty string has no lines
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_invalid_json() {
        let result = format_body_inner("{not valid json}", "application/json");
        // Should not panic; falls back to raw text
        assert!(result.is_ok());
        let lines = result.unwrap();
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_format_body_unicode() {
        let body = r#"{"emoji":"🚀","cjk":"日本語","accent":"café"}"#;
        let result = format_body_inner(body, "application/json").unwrap();
        let text = spans_text(&result);
        assert!(text.contains("🚀"));
        assert!(text.contains("日本語"));
        assert!(text.contains("café"));
    }

    #[test]
    fn test_format_body_very_large() {
        // ~2MB JSON: array of 10000 objects, each with 20 keys with longer values
        let mut items = Vec::new();
        for i in 0..10_000 {
            let mut obj = serde_json::Map::new();
            for j in 0..20 {
                obj.insert(
                    format!("field_{:04}", j),
                    Value::String(format!("value_{:06}_{:04}_padding_data_here", i, j)),
                );
            }
            items.push(Value::Object(obj));
        }
        let json = serde_json::to_string(&Value::Array(items)).unwrap();
        assert!(json.len() > 1_500_000, "JSON size was {}", json.len());
        let result = format_body_inner(&json, "application/json").unwrap();
        assert!(result.len() > 10_000);
    }

    // ─── build_json_tree tests ─────────────────────────────────────────────────

    fn build_tree(json: &str, max_depth: u32, max_children: u32) -> Result<JsonTreeNode, String> {
        let val: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        Ok(build_tree_node(&val, None, 0, max_depth, max_children, vec![], true, false))
    }

    #[test]
    fn test_tree_primitive_string() {
        let node = build_tree(r#""hello""#, 3, 100).unwrap();
        assert_eq!(node.node_type, "string");
        assert_eq!(node.value_preview.unwrap(), "\"hello\"");
    }

    #[test]
    fn test_tree_primitive_number() {
        let node = build_tree("42", 3, 100).unwrap();
        assert_eq!(node.node_type, "number");
        assert_eq!(node.value_preview.unwrap(), "42");
    }

    #[test]
    fn test_tree_primitive_boolean() {
        let node = build_tree("true", 3, 100).unwrap();
        assert_eq!(node.node_type, "boolean");
        assert_eq!(node.value_preview.unwrap(), "true");
    }

    #[test]
    fn test_tree_primitive_null() {
        let node = build_tree("null", 3, 100).unwrap();
        assert_eq!(node.node_type, "null");
        assert_eq!(node.value_preview.unwrap(), "null");
    }

    #[test]
    fn test_tree_simple_object() {
        let node = build_tree(r#"{"a":1,"b":"x"}"#, 3, 100).unwrap();
        assert_eq!(node.node_type, "object");
        assert_eq!(node.child_count, 2);
        assert_eq!(node.children.len(), 2);
        // Children should have keys
        let keys: Vec<&str> = node.children.iter().map(|c| c.key.as_deref().unwrap()).collect();
        assert!(keys.contains(&"a"));
        assert!(keys.contains(&"b"));
    }

    #[test]
    fn test_tree_simple_array() {
        let node = build_tree("[1,2,3]", 3, 100).unwrap();
        assert_eq!(node.node_type, "array");
        assert_eq!(node.child_count, 3);
        assert_eq!(node.children.len(), 3);
    }

    #[test]
    fn test_tree_nested() {
        let json = r#"{"a":{"b":{"c":1}}}"#;
        let node = build_tree(json, 10, 100).unwrap();
        assert_eq!(node.depth, 0);
        let a = &node.children[0];
        assert_eq!(a.depth, 1);
        let b = &a.children[0];
        assert_eq!(b.depth, 2);
        let c = &b.children[0];
        assert_eq!(c.depth, 3);
        assert_eq!(c.node_type, "number");
    }

    #[test]
    fn test_tree_max_depth() {
        // 10-level deep nesting, but max_depth=3
        let json = r#"{"l1":{"l2":{"l3":{"l4":{"l5":{"l6":{"l7":{"l8":{"l9":{"l10":42}}}}}}}}}}"#;
        let node = build_tree(json, 3, 100).unwrap();
        // At depth 3, children should be empty but child_count > 0
        let l1 = &node.children[0];
        let l2 = &l1.children[0];
        let l3 = &l2.children[0];
        assert_eq!(l3.depth, 3);
        // l3 is at max_depth, so its children vec is empty
        assert!(l3.children.is_empty());
        assert!(l3.child_count > 0);
    }

    #[test]
    fn test_tree_max_children() {
        let mut obj = serde_json::Map::new();
        for i in 0..200 {
            obj.insert(format!("key_{:03}", i), Value::Number(i.into()));
        }
        let json = serde_json::to_string(&Value::Object(obj)).unwrap();
        let node = build_tree(&json, 3, 50).unwrap();
        assert_eq!(node.child_count, 200);
        // 50 real children + 1 truncated node
        assert_eq!(node.children.len(), 51);
        let last = node.children.last().unwrap();
        assert_eq!(last.node_type, "truncated");
    }

    #[test]
    fn test_tree_path_tracking() {
        let json = r#"{"data":{"items":[10,20]}}"#;
        let node = build_tree(json, 10, 100).unwrap();
        assert!(node.path.is_empty());
        let data = &node.children[0];
        assert_eq!(data.path, vec!["data"]);
        let items = &data.children[0];
        assert_eq!(items.path, vec!["data", "items"]);
        let first_item = &items.children[0];
        assert_eq!(first_item.path, vec!["data", "items", "0"]);
    }

    #[test]
    fn test_tree_is_last_flag() {
        let json = r#"{"a":1,"b":2,"c":3}"#;
        let node = build_tree(json, 3, 100).unwrap();
        let len = node.children.len();
        for (i, child) in node.children.iter().enumerate() {
            if i == len - 1 {
                assert!(child.is_last, "last child should have is_last=true");
            } else {
                assert!(!child.is_last, "non-last child should have is_last=false");
            }
        }
    }

    #[test]
    fn test_tree_empty_json() {
        let result = build_tree("", 3, 100);
        assert!(result.is_err());
    }

    // ─── expand_json_node tests ────────────────────────────────────────────────

    fn expand_node(json: &str, path: Vec<String>, max_depth: u32, max_children: u32) -> Result<JsonTreeNode, String> {
        let val: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let target = navigate_json(&val, &path)?;
        let key = path.last().cloned();
        Ok(build_tree_node(target, key, 0, max_depth, max_children, path, true, false))
    }

    #[test]
    fn test_expand_root() {
        let json = r#"{"a":1,"b":2}"#;
        let node = expand_node(json, vec![], 3, 100).unwrap();
        assert_eq!(node.node_type, "object");
        assert_eq!(node.child_count, 2);
    }

    #[test]
    fn test_expand_nested_key() {
        let json = r#"{"data":{"x":1,"y":2}}"#;
        let node = expand_node(json, vec!["data".into()], 3, 100).unwrap();
        assert_eq!(node.node_type, "object");
        assert_eq!(node.child_count, 2);
        assert_eq!(node.key.as_deref(), Some("data"));
    }

    #[test]
    fn test_expand_array_index() {
        let json = r#"{"items":[{"a":1},{"b":2}]}"#;
        let node = expand_node(json, vec!["items".into(), "0".into()], 3, 100).unwrap();
        assert_eq!(node.node_type, "object");
        assert_eq!(node.child_count, 1); // {"a":1}
    }

    #[test]
    fn test_expand_deep_path() {
        let json = r#"{"a":{"b":{"c":{"d":{"e":42}}}}}"#;
        let path: Vec<String> = vec!["a", "b", "c", "d", "e"].iter().map(|s| s.to_string()).collect();
        let node = expand_node(json, path, 3, 100).unwrap();
        assert_eq!(node.node_type, "number");
        assert_eq!(node.value_preview.as_deref(), Some("42"));
    }

    #[test]
    fn test_expand_invalid_path() {
        let json = r#"{"a":1}"#;
        let result = expand_node(json, vec!["nonexistent".into()], 3, 100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_expand_invalid_array_index() {
        let json = r#"[1,2,3]"#;
        let result = expand_node(json, vec!["999".into()], 3, 100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of bounds"));
    }

    #[test]
    fn test_expand_max_depth() {
        let json = r#"{"a":{"b":{"c":{"d":1}}}}"#;
        let node = expand_node(json, vec!["a".into()], 1, 100).unwrap();
        // max_depth=1: we see "b" at depth 1 but its children are cut
        assert_eq!(node.node_type, "object");
        let b = &node.children[0];
        assert_eq!(b.depth, 1);
        assert!(b.children.is_empty());
        assert!(b.child_count > 0);
    }

    #[test]
    fn test_expand_max_children() {
        let mut obj = serde_json::Map::new();
        for i in 0..100 {
            obj.insert(format!("k{:03}", i), Value::Number(i.into()));
        }
        let json = serde_json::to_string(&serde_json::json!({"data": Value::Object(obj)})).unwrap();
        let node = expand_node(&json, vec!["data".into()], 3, 10).unwrap();
        // 10 real + 1 truncated
        assert_eq!(node.children.len(), 11);
        assert_eq!(node.child_count, 100);
    }

    // ─── compute_diff tests ────────────────────────────────────────────────────

    #[test]
    fn test_diff_identical() {
        let text = "line1\nline2\nline3";
        let result = compute_diff_inner(text, text, "text/plain").unwrap();
        assert!((result.similarity - 1.0).abs() < f64::EPSILON);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.changed_count, 0);
        assert!(result.ops.iter().all(|op| matches!(op, DiffOp::Same { .. })));
    }

    #[test]
    fn test_diff_completely_different() {
        let a = "alpha\nbeta";
        let b = "gamma\ndelta";
        let result = compute_diff_inner(a, b, "text/plain").unwrap();
        assert!(result.similarity < 0.5);
        // No Same ops
        assert!(!result.ops.iter().any(|op| matches!(op, DiffOp::Same { .. })));
    }

    #[test]
    fn test_diff_single_addition() {
        let a = "line1\nline2";
        let b = "line1\nline2\nline3";
        let result = compute_diff_inner(a, b, "text/plain").unwrap();
        assert_eq!(result.added_count, 1);
        assert_eq!(result.removed_count, 0);
    }

    #[test]
    fn test_diff_single_removal() {
        let a = "line1\nline2\nline3";
        let b = "line1\nline2";
        let result = compute_diff_inner(a, b, "text/plain").unwrap();
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.added_count, 0);
    }

    #[test]
    fn test_diff_single_change() {
        let a = "line1\nold_line\nline3";
        let b = "line1\nnew_line\nline3";
        let result = compute_diff_inner(a, b, "text/plain").unwrap();
        assert_eq!(result.changed_count, 1);
        // Find the Change op
        let change = result.ops.iter().find(|op| matches!(op, DiffOp::Change { .. }));
        assert!(change.is_some());
        if let DiffOp::Change { left, right, left_highlights, right_highlights } = change.unwrap() {
            assert_eq!(left, "old_line");
            assert_eq!(right, "new_line");
            assert!(!left_highlights.is_empty());
            assert!(!right_highlights.is_empty());
        }
    }

    #[test]
    fn test_diff_json_sorts_before_diff() {
        let a = r#"{"b":2,"a":1}"#;
        let b = r#"{"a":1,"b":2}"#;
        let result = compute_diff_inner(a, b, "application/json").unwrap();
        assert!((result.similarity - 1.0).abs() < f64::EPSILON);
        assert!(result.is_json);
    }

    #[test]
    fn test_diff_json_value_change() {
        let a = r#"{"name":"Alice","age":30}"#;
        let b = r#"{"name":"Alice","age":31}"#;
        let result = compute_diff_inner(a, b, "application/json").unwrap();
        assert!(result.changed_count > 0 || result.added_count > 0 || result.removed_count > 0);
        assert!(result.similarity < 1.0);
    }

    #[test]
    fn test_diff_multiline() {
        let a = "header\nline1\nline2\nline3\nfooter";
        let b = "header\nline1\nmodified\nline3\nnew_line\nfooter";
        let result = compute_diff_inner(a, b, "text/plain").unwrap();
        // Should have a mix of Same, Change, and Add ops
        let same = result.ops.iter().filter(|op| matches!(op, DiffOp::Same { .. })).count();
        assert!(same >= 3); // header, line1, line3, footer (at least some)
        assert!(result.changed_count > 0 || result.added_count > 0);
    }

    #[test]
    fn test_diff_char_highlights() {
        let a = "the quick brown fox";
        let b = "the quick red fox";
        let result = compute_diff_inner(a, b, "text/plain").unwrap();
        // The entire content is one line each, so we should get a Change
        let change = result.ops.iter().find(|op| matches!(op, DiffOp::Change { .. }));
        assert!(change.is_some());
        if let DiffOp::Change { left_highlights, right_highlights, .. } = change.unwrap() {
            assert!(!left_highlights.is_empty());
            assert!(!right_highlights.is_empty());
            // The differing region should be within the string bounds
            let lh = &left_highlights[0];
            assert!(lh.start < lh.end);
            assert!(lh.end <= a.len());
        }
    }

    #[test]
    fn test_diff_empty_left() {
        let result = compute_diff_inner("", "line1\nline2", "text/plain").unwrap();
        assert_eq!(result.added_count, 2);
        assert_eq!(result.removed_count, 0);
    }

    #[test]
    fn test_diff_empty_right() {
        let result = compute_diff_inner("line1\nline2", "", "text/plain").unwrap();
        assert_eq!(result.removed_count, 2);
        assert_eq!(result.added_count, 0);
    }

    #[test]
    fn test_diff_empty_both() {
        let result = compute_diff_inner("", "", "text/plain").unwrap();
        assert!(result.ops.is_empty());
        assert!((result.similarity - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_diff_large_input() {
        let a: String = (0..5000).map(|i| format!("line_{}", i)).collect::<Vec<_>>().join("\n");
        let mut b_lines: Vec<String> = (0..5000).map(|i| format!("line_{}", i)).collect();
        // Modify a few lines
        b_lines[100] = "modified_100".into();
        b_lines[2000] = "modified_2000".into();
        b_lines[4999] = "modified_4999".into();
        let b = b_lines.join("\n");
        let result = compute_diff_inner(&a, &b, "text/plain").unwrap();
        assert!(result.similarity > 0.9);
        assert!(result.changed_count >= 3);
    }

    #[test]
    fn test_diff_counts() {
        let a = "same\nremoved\nchanged_old";
        let b = "same\nchanged_new\nadded";
        let result = compute_diff_inner(a, b, "text/plain").unwrap();
        // "same" → Same, "removed"/"changed_old" vs "changed_new"/"added" will be paired
        let total = result.added_count + result.removed_count + result.changed_count;
        assert!(total > 0);
    }

    #[test]
    fn test_diff_xml_normalizes() {
        let a = r#"<div b="1" a="2">text</div>"#;
        let b = r#"<div a="2" b="1">text</div>"#;
        let result = compute_diff_inner(a, b, "application/xml").unwrap();
        // After attribute normalization these should be identical
        assert!(
            result.similarity > 0.9,
            "XML with reordered attributes should have high similarity, got {}",
            result.similarity
        );
    }

    // ─── sort_and_normalize tests ──────────────────────────────────────────────

    #[test]
    fn test_normalize_json_sorts_keys() {
        let result = sort_and_normalize_inner(r#"{"b":1,"a":2}"#, "application/json").unwrap();
        let pos_a = result.find("\"a\"").unwrap();
        let pos_b = result.find("\"b\"").unwrap();
        assert!(pos_a < pos_b);
    }

    #[test]
    fn test_normalize_json_nested() {
        let input = r#"{"outer":{"z":3,"a":1},"inner":{"b":2,"a":1}}"#;
        let result = sort_and_normalize_inner(input, "application/json").unwrap();
        // "inner" should come before "outer" (alphabetical)
        let pos_inner = result.find("\"inner\"").unwrap();
        let pos_outer = result.find("\"outer\"").unwrap();
        assert!(pos_inner < pos_outer);
        // Inside "inner", "a" should come before "b"
        // Find "inner" block and check within it
        let inner_block = &result[pos_inner..];
        let a_in_inner = inner_block.find("\"a\"").unwrap();
        let b_in_inner = inner_block.find("\"b\"").unwrap();
        assert!(a_in_inner < b_in_inner);
    }

    #[test]
    fn test_normalize_json_array_preserved() {
        let input = r#"[3,1,2]"#;
        let result = sort_and_normalize_inner(input, "application/json").unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed, serde_json::json!([3, 1, 2]));
    }

    #[test]
    fn test_normalize_json_pretty_printed() {
        let input = r#"{"a":1}"#;
        let result = sort_and_normalize_inner(input, "application/json").unwrap();
        // serde_json::to_string_pretty uses 2-space indent
        assert!(result.contains('\n'));
        assert!(result.contains("  "));
    }

    #[test]
    fn test_normalize_invalid_json() {
        let result = sort_and_normalize_inner("{invalid}", "application/json");
        // Returns error for invalid JSON
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize_xml_sorts_attrs() {
        let input = r#"<div b="1" a="2">text</div>"#;
        let result = sort_and_normalize_inner(input, "application/xml").unwrap();
        let pos_a = result.find("a=").unwrap();
        let pos_b = result.find("b=").unwrap();
        assert!(pos_a < pos_b, "attribute 'a' should come before 'b' after sorting");
    }

    #[test]
    fn test_normalize_plain_text() {
        let input = "hello world";
        let result = sort_and_normalize_inner(input, "text/plain").unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_normalize_empty() {
        let result = sort_and_normalize_inner("", "text/plain").unwrap();
        assert_eq!(result, "");
    }

    // ─── Rayon parallelism tests ───────────────────────────────────────────────

    #[test]
    fn test_rayon_format_large_parallel() {
        // Generate 10,000+ key JSON to trigger Rayon path (>1000 lines when pretty-printed)
        let mut obj = serde_json::Map::new();
        for i in 0..10_000 {
            obj.insert(format!("key_{:05}", i), Value::String(format!("val_{}", i)));
        }
        let json = serde_json::to_string(&Value::Object(obj)).unwrap();
        let result = format_body_inner(&json, "application/json").unwrap();
        assert!(result.len() > 10_000);
        // Verify correctness: first and last keys should be present
        let text = spans_text(&result);
        assert!(text.contains("key_00000"));
        assert!(text.contains("key_09999"));
    }

    #[test]
    fn test_rayon_diff_parallel_char_highlights() {
        // Create diff with many Change ops to trigger parallel char-diff (>50 pairs)
        let a: String = (0..200).map(|i| format!("line_old_{:04}", i)).collect::<Vec<_>>().join("\n");
        let b: String = (0..200).map(|i| format!("line_new_{:04}", i)).collect::<Vec<_>>().join("\n");
        let result = compute_diff_inner(&a, &b, "text/plain").unwrap();
        // All 200 lines changed
        assert_eq!(result.changed_count, 200);
        // Verify every Change op has char highlights
        for op in &result.ops {
            if let DiffOp::Change { left_highlights, right_highlights, .. } = op {
                assert!(!left_highlights.is_empty(), "Change should have left highlights");
                assert!(!right_highlights.is_empty(), "Change should have right highlights");
            }
        }
    }

    #[test]
    fn test_rayon_tree_parallel() {
        // Large JSON with 500+ top-level keys to trigger Rayon in tree builder
        let mut obj = serde_json::Map::new();
        for i in 0..500 {
            let mut inner = serde_json::Map::new();
            inner.insert("value".into(), Value::Number(i.into()));
            obj.insert(format!("item_{:03}", i), Value::Object(inner));
        }
        let json = serde_json::to_string(&Value::Object(obj)).unwrap();
        let val: Value = serde_json::from_str(&json).unwrap();
        // use_rayon=true
        let node = build_tree_node(&val, None, 0, 3, 500, vec![], true, true);
        assert_eq!(node.node_type, "object");
        assert_eq!(node.child_count, 500);
        assert_eq!(node.children.len(), 500);
    }

    #[test]
    fn test_rayon_thread_safety() {
        use std::thread;

        let handles: Vec<_> = (0..4)
            .map(|i| {
                thread::spawn(move || match i % 4 {
                    0 => {
                        let body = r#"{"a":1,"b":"hello"}"#;
                        format_body_inner(body, "application/json").unwrap();
                    }
                    1 => {
                        let json = r#"{"x":{"y":1}}"#;
                        let val: Value = serde_json::from_str(json).unwrap();
                        build_tree_node(&val, None, 0, 3, 100, vec![], true, false);
                    }
                    2 => {
                        compute_diff_inner("hello\nworld", "hello\nearth", "text/plain").unwrap();
                    }
                    3 => {
                        sort_and_normalize_inner(r#"{"b":1,"a":2}"#, "application/json").unwrap();
                    }
                    _ => unreachable!(),
                })
            })
            .collect();

        for h in handles {
            h.join().expect("Thread should not panic");
        }
    }
}
