//! Minimal JSONC (JSON with comments and trailing commas) editing for
//! top-level object keys. Zed's settings.json is JSONC, so a plain
//! serde_json round-trip would destroy the user's comments; instead we
//! locate the exact byte span of a top-level value and splice.

use anyhow::{bail, Result};
use serde_json::Value;

/// Byte range of a top-level entry's value, e.g. the `false` in
/// `"zoomed_padding": false`.
struct ValueSpan {
    start: usize,
    end: usize,
}

pub fn get(content: &str, key: &str) -> Option<Value> {
    let span = find_value_span(content, key)?;
    parse_value(&content[span.start..span.end]).ok()
}

pub fn set(content: &str, key: &str, value: &Value) -> Result<String> {
    let rendered = render(value);
    if let Some(span) = find_value_span(content, key) {
        let mut updated = String::with_capacity(content.len() + rendered.len());
        updated.push_str(&content[..span.start]);
        updated.push_str(&rendered);
        updated.push_str(&content[span.end..]);
        return Ok(updated);
    }
    let entry = format!("  {}: {rendered},", Value::String(key.into()));
    let open = match root_open_brace(content) {
        Ok(open) => open,
        // A blank or comment-only file (Zed writes settings.json lazily)
        // gets a fresh object appended after whatever is there.
        Err(_) if !has_significant_token(content) => {
            let mut updated = content.trim_end().to_owned();
            if !updated.is_empty() {
                updated.push('\n');
            }
            updated.push_str(&format!("{{\n{entry}\n}}\n"));
            return Ok(updated);
        }
        Err(error) => return Err(error),
    };
    let mut updated = String::with_capacity(content.len() + entry.len() + 8);
    updated.push_str(&content[..=open]);
    updated.push('\n');
    updated.push_str(&entry);
    updated.push_str(&content[open + 1..]);
    Ok(updated)
}

/// Parse a whole JSONC document — Zed's keymap.json has an array at the
/// root, so this is the entry point when the top level is not an object.
pub fn parse_document(content: &str) -> Result<Value> {
    parse_value(content)
}

/// Append a pre-rendered element to the document's root array, before its
/// closing bracket, preserving comments. `rendered` must be one complete
/// JSON value; it is re-indented to sit two spaces deep. An empty document
/// becomes a one-element array.
pub fn push_root_array_item(content: &str, rendered: &str) -> Result<String> {
    let indented = indent_item(rendered);
    let mut depth = 0usize;
    let mut saw_open = false;
    let mut close: Option<usize> = None;
    let mut last_significant = '[';
    for token in tokens(content) {
        match token.kind {
            TokenKind::Comment => {}
            TokenKind::Str(_) => {
                if !saw_open {
                    bail!("keymap file has no top-level array");
                }
                last_significant = '"';
            }
            TokenKind::Text(c) if c.is_whitespace() => {}
            TokenKind::Text(c) => {
                if !saw_open {
                    if c == '[' {
                        saw_open = true;
                        depth = 1;
                        continue;
                    }
                    bail!("keymap file has no top-level array");
                }
                match c {
                    '[' | '{' => {
                        depth += 1;
                        last_significant = c;
                    }
                    ']' | '}' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            close = Some(token.start);
                            break;
                        }
                        last_significant = c;
                    }
                    _ => last_significant = c,
                }
            }
        }
    }
    if !saw_open {
        // Only comments and whitespace precede here (anything else bailed
        // above); keep them and start the array after them.
        let mut updated = content.trim_end().to_owned();
        if !updated.is_empty() {
            updated.push('\n');
        }
        updated.push_str(&format!("[\n  {indented}\n]\n"));
        return Ok(updated);
    }
    let close = close.unwrap_or(content.len());
    let needs_comma = !matches!(last_significant, '[' | ',');
    let mut updated = String::with_capacity(content.len() + indented.len() + 8);
    updated.push_str(content[..close].trim_end());
    if needs_comma {
        updated.push(',');
    }
    updated.push_str("\n  ");
    updated.push_str(&indented);
    updated.push('\n');
    updated.push_str(&content[close..]);
    Ok(updated)
}

/// Whether the document contains anything besides comments and whitespace.
fn has_significant_token(content: &str) -> bool {
    tokens(content).any(|token| match token.kind {
        TokenKind::Comment => false,
        TokenKind::Str(_) => true,
        TokenKind::Text(c) => !c.is_whitespace(),
    })
}

/// Indent every line after the first by two spaces, so a multi-line value
/// sits correctly under a two-space array element.
fn indent_item(rendered: &str) -> String {
    rendered
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_owned()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse a JSONC value fragment by stripping comments and trailing commas.
fn parse_value(fragment: &str) -> Result<Value> {
    let mut stripped = String::with_capacity(fragment.len());
    let mut pending_comma = String::new();
    for token in tokens(fragment) {
        match token.kind {
            TokenKind::Comment => {}
            TokenKind::Text(c) => {
                if c == ',' {
                    pending_comma.push(c);
                    continue;
                }
                if !c.is_whitespace() {
                    if c != '}' && c != ']' {
                        stripped.push_str(&pending_comma);
                    }
                    pending_comma.clear();
                }
                stripped.push(c);
            }
            TokenKind::Str(s) => {
                stripped.push_str(&pending_comma);
                pending_comma.clear();
                stripped.push_str(s);
            }
        }
    }
    Ok(serde_json::from_str(stripped.trim())?)
}

fn render(value: &Value) -> String {
    // Indent nested lines to sit under a two-space top-level entry.
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| value.to_string())
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_owned()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn root_open_brace(content: &str) -> Result<usize> {
    for token in tokens(content) {
        match token.kind {
            TokenKind::Text('{') => return Ok(token.start),
            TokenKind::Text(c) if c.is_whitespace() => {}
            TokenKind::Comment => {}
            _ => break,
        }
    }
    bail!("settings file has no top-level object")
}

fn find_value_span(content: &str, key: &str) -> Option<ValueSpan> {
    let mut depth = 0usize;
    let mut last_string: Option<(usize, String)> = None;
    let mut matched_key = false;
    let mut value_start: Option<usize> = None;
    let mut value_depth = 0usize;
    let mut value_end = 0usize;

    for token in tokens(content) {
        match token.kind {
            TokenKind::Comment => {}
            TokenKind::Str(raw) => {
                if let Some(start) = value_start {
                    value_end = token.end;
                    let _ = start;
                } else if depth == 1 {
                    let unquoted: String = serde_json::from_str(raw).ok()?;
                    last_string = Some((token.start, unquoted));
                }
            }
            TokenKind::Text(c) => {
                if let Some(start) = value_start {
                    match c {
                        '{' | '[' => value_depth += 1,
                        '}' | ']' if value_depth > 0 => {
                            value_depth -= 1;
                            value_end = token.end;
                        }
                        ',' | '}' | ']' if value_depth == 0 => {
                            return Some(ValueSpan {
                                start,
                                end: value_end.max(start),
                            });
                        }
                        _ if !c.is_whitespace() => value_end = token.end,
                        _ => {}
                    }
                    continue;
                }
                match c {
                    ':' if depth == 1 => {
                        if let Some((_, name)) = &last_string {
                            matched_key = name == key;
                        }
                        last_string = None;
                    }
                    '{' | '[' => {
                        depth += 1;
                        if matched_key && depth == 2 {
                            value_start = Some(token.start);
                            value_depth = 1;
                            value_end = token.end;
                        }
                    }
                    '}' | ']' => depth = depth.saturating_sub(1),
                    _ if !c.is_whitespace() && matched_key && depth == 1 => {
                        value_start = Some(token.start);
                        value_depth = 0;
                        value_end = token.end;
                    }
                    _ => {}
                }
            }
        }
        if value_start.is_none() && matched_key {
            // A string value: the Str arm above skips depth-1 strings once
            // the key matched, so catch them here.
            if let TokenKind::Str(_) = token.kind {
                return Some(ValueSpan {
                    start: token.start,
                    end: token.end,
                });
            }
        }
    }
    None
}

enum TokenKind<'a> {
    /// One character of structural or scalar text.
    Text(char),
    /// A complete string literal, quotes included.
    Str(&'a str),
    /// A complete line or block comment.
    Comment,
}

struct Token<'a> {
    kind: TokenKind<'a>,
    start: usize,
    end: usize,
}

fn tokens(content: &str) -> impl Iterator<Item = Token<'_>> {
    let bytes = content.as_bytes();
    let mut index = 0usize;
    std::iter::from_fn(move || {
        if index >= content.len() {
            return None;
        }
        let start = index;
        let rest = &content[index..];
        if rest.starts_with("//") {
            let len = rest.find('\n').unwrap_or(rest.len());
            index += len;
            return Some(Token {
                kind: TokenKind::Comment,
                start,
                end: index,
            });
        }
        if rest.starts_with("/*") {
            let len = rest.find("*/").map_or(rest.len(), |at| at + 2);
            index += len;
            return Some(Token {
                kind: TokenKind::Comment,
                start,
                end: index,
            });
        }
        if bytes[index] == b'"' {
            let mut escaped = false;
            let mut len = 1;
            for c in rest[1..].chars() {
                len += c.len_utf8();
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    break;
                }
            }
            index += len;
            return Some(Token {
                kind: TokenKind::Str(&content[start..index]),
                start,
                end: index,
            });
        }
        let c = rest.chars().next()?;
        index += c.len_utf8();
        Some(Token {
            kind: TokenKind::Text(c),
            start,
            end: index,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    const ZED: &str = r#"// Zed settings
{
  "tab_bar": {
    "show": true, // trailing comment
  },
  "zoomed_padding": true,
  "ui_font_size": 15.0,
  "theme": "One { Dark }",
}
"#;

    #[test]
    fn get_reads_values_despite_comments_and_trailing_commas() {
        assert_eq!(get(ZED, "zoomed_padding"), Some(json!(true)));
        assert_eq!(get(ZED, "tab_bar"), Some(json!({"show": true})));
        assert_eq!(get(ZED, "theme"), Some(json!("One { Dark }")));
        assert_eq!(get(ZED, "missing"), None);
    }

    #[test]
    fn set_replaces_an_existing_scalar_in_place() {
        let updated = set(ZED, "zoomed_padding", &json!(false)).unwrap();
        assert!(updated.contains("\"zoomed_padding\": false,"));
        assert!(updated.contains("// Zed settings"));
        assert!(updated.contains("// trailing comment"));
        assert_eq!(get(&updated, "zoomed_padding"), Some(json!(false)));
    }

    #[test]
    fn set_replaces_an_existing_object_in_place() {
        let updated = set(ZED, "tab_bar", &json!({"show": false})).unwrap();
        assert_eq!(get(&updated, "tab_bar"), Some(json!({"show": false})));
        assert_eq!(get(&updated, "ui_font_size"), Some(json!(15.0)));
    }

    #[test]
    fn set_inserts_a_missing_key_after_the_opening_brace() {
        let updated = set(
            ZED,
            "centered_layout",
            &json!({"left_padding": 0, "right_padding": 0}),
        )
        .unwrap();
        assert_eq!(
            get(&updated, "centered_layout"),
            Some(json!({"left_padding": 0, "right_padding": 0}))
        );
        assert_eq!(get(&updated, "zoomed_padding"), Some(json!(true)));
        assert!(updated.starts_with("// Zed settings"));
    }

    #[test]
    fn set_works_on_an_empty_document() {
        let updated = set("{}\n", "zoomed_padding", &json!(false)).unwrap();
        assert_eq!(get(&updated, "zoomed_padding"), Some(json!(false)));
    }

    #[test]
    fn set_builds_an_object_in_a_blank_or_comment_only_file() {
        let updated = set("", "zoomed_padding", &json!(false)).unwrap();
        assert_eq!(get(&updated, "zoomed_padding"), Some(json!(false)));
        let updated = set("// zed settings\n", "zoomed_padding", &json!(false)).unwrap();
        assert!(updated.starts_with("// zed settings"));
        assert_eq!(get(&updated, "zoomed_padding"), Some(json!(false)));
    }

    #[test]
    fn set_still_rejects_a_non_object_document() {
        assert!(set("[]\n", "zoomed_padding", &json!(false)).is_err());
    }

    const KEYMAP: &str = r#"// Zed keymap
[
  {
    "context": "Editor",
    "bindings": {
      "cmd-left": "pane::GoBack", // comment inside
    }
  }
]
"#;

    #[test]
    fn push_root_array_item_appends_before_the_closing_bracket() {
        let updated = push_root_array_item(
            KEYMAP,
            "{\n  \"context\": \"Terminal\",\n  \"bindings\": {}\n}",
        )
        .unwrap();
        assert!(updated.starts_with("// Zed keymap"));
        assert!(updated.contains("// comment inside"));
        let document = parse_document(&updated).unwrap();
        let blocks = document.as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["context"], json!("Terminal"));
    }

    #[test]
    fn push_root_array_item_separates_from_a_comma_free_last_element() {
        let updated = push_root_array_item("[\n  { \"context\": \"Editor\" }\n]\n", "{}").unwrap();
        assert_eq!(
            parse_document(&updated).unwrap().as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn push_root_array_item_builds_a_document_from_nothing() {
        let updated = push_root_array_item("", "{ \"context\": \"Terminal\" }").unwrap();
        let document = parse_document(&updated).unwrap();
        assert_eq!(document.as_array().unwrap().len(), 1);
    }

    #[test]
    fn push_root_array_item_keeps_comments_of_a_comment_only_file() {
        let updated =
            push_root_array_item("// zed keymap\n", "{ \"context\": \"Terminal\" }").unwrap();
        assert!(updated.starts_with("// zed keymap"));
        assert_eq!(
            parse_document(&updated).unwrap().as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn push_root_array_item_rejects_an_object_document() {
        assert!(push_root_array_item("{}\n", "{}").is_err());
    }
}
