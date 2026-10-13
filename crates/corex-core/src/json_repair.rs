use serde_json::Value;

/// Parses JSON from raw LLM output with maximum resilience.
///
/// Handles common LLM quirks:
/// - Markdown code blocks (```json ... ``` or ``` ... ```)
/// - Surrounding whitespace or leading/trailing commentary
/// - String-encoded JSON (where the entire payload is a quoted JSON string)
/// - Trailing commas before `}` or `]`
/// - Truncated JSON (unclosed strings, braces, or brackets due to token limits)
/// - Single quotes instead of double quotes for keys and strings
pub fn parse_lenient_json(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }

    // 1. Direct parse attempt (fast path for valid JSON)
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return normalize_parsed_value(v);
    }

    // 2. Strip Markdown code fences if present
    let stripped = strip_markdown_fences(trimmed);
    if let Ok(v) = serde_json::from_str::<Value>(&stripped) {
        return normalize_parsed_value(v);
    }

    // 3. Extract JSON substring between outermost braces '{' ... '}' or brackets '[' ... ']'
    if let Some(extracted) = extract_json_bounds(&stripped) {
        if let Ok(v) = serde_json::from_str::<Value>(&extracted) {
            return normalize_parsed_value(v);
        }
        // Try repairing the extracted section
        if let Ok(repaired) = repair_json_syntax(&extracted) {
            if let Ok(v) = serde_json::from_str::<Value>(&repaired) {
                return normalize_parsed_value(v);
            }
        }
    }

    // 4. Try repairing the stripped text directly
    if let Ok(repaired) = repair_json_syntax(&stripped) {
        if let Ok(v) = serde_json::from_str::<Value>(&repaired) {
            return normalize_parsed_value(v);
        }
    }

    Err(format!(
        "Failed to parse or repair JSON arguments. Raw input: '{}'",
        if raw.len() > 120 { format!("{}...", &raw[..120]) } else { raw.to_string() }
    ))
}

/// Normalizes parsed values: if the value is a string that itself contains a valid JSON object/array,
/// unpack it.
fn normalize_parsed_value(v: Value) -> Result<Value, String> {
    match v {
        Value::String(ref s) => {
            let s_trim = s.trim();
            if (s_trim.starts_with('{') && s_trim.ends_with('}'))
                || (s_trim.starts_with('[') && s_trim.ends_with(']'))
            {
                if let Ok(inner) = serde_json::from_str::<Value>(s_trim) {
                    return Ok(inner);
                }
            }
            Ok(v)
        }
        other => Ok(other),
    }
}

/// Strips markdown fences such as ```json ... ``` or ``` ... ```
fn strip_markdown_fences(s: &str) -> String {
    let mut text = s.trim();
    if text.starts_with("```") {
        if let Some(first_newline) = text.find('\n') {
            text = text[first_newline + 1..].trim();
        } else {
            text = text.trim_start_matches('`').trim();
        }
    }
    if text.ends_with("```") {
        text = text[..text.len() - 3].trim();
    }
    text.to_string()
}

/// Extracts content between the first opening brace/bracket and last closing brace/bracket.
fn extract_json_bounds(s: &str) -> Option<String> {
    let first_brace = s.find('{');
    let first_bracket = s.find('[');

    let start_idx = match (first_brace, first_bracket) {
        (Some(b), Some(k)) => Some(b.min(k)),
        (Some(b), None) => Some(b),
        (None, Some(k)) => Some(k),
        (None, None) => None,
    }?;

    let is_brace = s.as_bytes()[start_idx] == b'{';
    let end_char = if is_brace { '}' } else { ']' };

    let end_idx = s.rfind(end_char)?;
    if end_idx >= start_idx {
        Some(s[start_idx..=end_idx].to_string())
    } else {
        None
    }
}

/// Repairs common syntax issues in LLM-generated JSON:
/// - Trailing commas before `}` or `]`
/// - Unclosed quotes, braces, brackets (truncated stream)
/// - Single-quoted string replacements
fn repair_json_syntax(input: &str) -> Result<String, String> {
    let mut out = String::with_capacity(input.len() + 16);
    let mut in_string = false;
    let mut escape_next = false;
    let mut quote_char = '"';
    let mut stack = Vec::new();

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if in_string {
            if escape_next {
                escape_next = false;
                out.push(ch);
            } else if ch == '\\' {
                escape_next = true;
                out.push(ch);
            } else if ch == quote_char {
                in_string = false;
                out.push('"'); // Always normalize closing quote to double-quote
            } else {
                if quote_char == '\'' && ch == '"' {
                    // Escape double quotes that were inside single-quoted strings
                    out.push('\\');
                }
                out.push(ch);
            }
        } else {
            match ch {
                '"' | '\'' => {
                    in_string = true;
                    quote_char = ch;
                    out.push('"'); // Always normalize opening quote to double-quote
                }
                '{' => {
                    stack.push('}');
                    out.push('{');
                }
                '[' => {
                    stack.push(']');
                    out.push('[');
                }
                '}' | ']' => {
                    // Strip any trailing comma immediately preceding this closing bracket
                    while let Some(last_c) = out.chars().last() {
                        if last_c.is_whitespace() || last_c == ',' {
                            out.pop();
                        } else {
                            break;
                        }
                    }
                    if let Some(pos) = stack.iter().rposition(|&expected| expected == ch) {
                        stack.remove(pos);
                    }
                    out.push(ch);
                }
                _ => {
                    out.push(ch);
                }
            }
        }
        i += 1;
    }

    // If the stream ended while inside a string, close the string
    if in_string {
        out.push('"');
    }

    // Strip any trailing comma before closing unclosed containers
    while let Some(last_c) = out.chars().last() {
        if last_c.is_whitespace() || last_c == ',' {
            out.pop();
        } else {
            break;
        }
    }

    // Close any unclosed braces or brackets in reverse order (LIFO)
    while let Some(expected_closing) = stack.pop() {
        out.push(expected_closing);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_json_passes() {
        let raw = r#"{"command": "cargo test", "timeout": 30}"#;
        let v = parse_lenient_json(raw).unwrap();
        assert_eq!(v["command"], "cargo test");
        assert_eq!(v["timeout"], 30);
    }

    #[test]
    fn test_markdown_code_block_stripping() {
        let raw = "```json\n{\n  \"path\": \"crates/corex-core/src/lib.rs\"\n}\n```";
        let v = parse_lenient_json(raw).unwrap();
        assert_eq!(v["path"], "crates/corex-core/src/lib.rs");
    }

    #[test]
    fn test_trailing_commas_repaired() {
        let raw = r#"{"files": ["a.txt", "b.txt",], "force": true,}"#;
        let v = parse_lenient_json(raw).unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 2);
        assert_eq!(v["force"], true);
    }

    #[test]
    fn test_truncated_json_repaired() {
        // Stream cut off mid-string or mid-object
        let raw = r#"{"action": "run_shell_command", "params": {"command": "git log"#;
        let v = parse_lenient_json(raw).unwrap();
        assert_eq!(v["action"], "run_shell_command");
        assert_eq!(v["params"]["command"], "git log");
    }

    #[test]
    fn test_single_quotes_repaired() {
        let raw = "{'model': 'deepseek-flash', 'temperature': 0.7}";
        let v = parse_lenient_json(raw).unwrap();
        assert_eq!(v["model"], "deepseek-flash");
        assert_eq!(v["temperature"], 0.7);
    }

    #[test]
    fn test_nested_stringified_json_unpacked() {
        let raw = r#""{\"query\": \"rust async tokio\", \"limit\": 5}""#;
        let v = parse_lenient_json(raw).unwrap();
        assert_eq!(v["query"], "rust async tokio");
        assert_eq!(v["limit"], 5);
    }

    #[test]
    fn test_surrounding_commentary_extracted() {
        let raw = "Here are the tool arguments:\n{\"path\": \"src/main.rs\", \"content\": \"hello\"}\nHope this helps!";
        let v = parse_lenient_json(raw).unwrap();
        assert_eq!(v["path"], "src/main.rs");
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn test_empty_string_gives_empty_object() {
        let v = parse_lenient_json("").unwrap();
        assert!(v.is_object());
    }
}
