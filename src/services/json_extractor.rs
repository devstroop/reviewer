use crate::engine::ReviewFinding;
use serde_json::Value;
use tracing::warn;

/// Valid severity levels for a structured finding.
const VALID_SEVERITIES: &[&str] = &["high", "medium", "low", "info"];

/// Valid category values for a structured finding.
const VALID_CATEGORIES: &[&str] = &[
    "logic_error",
    "security",
    "api_misuse",
    "performance",
    "missing_edge_case",
    "best_practice",
    "style",
    "documentation",
    "other",
];

/// Result of extracting structured findings from AI response text.
pub(crate) struct ExtractedFindings {
    /// Valid findings parsed from the JSON block.
    pub findings: Vec<ReviewFinding>,
    /// The remaining markdown review text (with JSON block removed).
    pub review_text: String,
    /// Number of malformed entries that were dropped during validation.
    pub dropped_count: usize,
}

/// Extracts structured JSON findings from AI response text.
pub(crate) struct JsonExtractor;

impl JsonExtractor {
    /// Extract findings from AI response text.
    ///
    /// The AI is instructed to place a JSON findings block before the markdown
    /// review.  This method extracts that block, validates each finding, drops
    /// malformed entries, and returns the valid findings + clean markdown.
    pub(crate) fn extract(text: &str) -> ExtractedFindings {
        let (json_value, review_text) = extract_json_block(text);

        match json_value {
            Some(Value::Array(items)) => {
                let total_items = items.len();
                let findings: Vec<ReviewFinding> = items
                    .into_iter()
                    .filter_map(|item| parse_single_finding(item))
                    .collect();
                let dropped_count = total_items - findings.len();
                ExtractedFindings {
                    findings,
                    review_text,
                    dropped_count,
                }
            }
            Some(other) => {
                warn!(
                    "Expected JSON array for findings, got {}",
                    json_type_name(&other)
                );
                ExtractedFindings {
                    findings: vec![],
                    review_text,
                    dropped_count: 0,
                }
            }
            None => ExtractedFindings {
                findings: vec![],
                review_text,
                dropped_count: 0,
            },
        }
    }
}

/// Try to extract a JSON block from the response text.
///
/// Returns `(Option<parsed_json>, remaining_text)` where remaining_text
/// is the original text with the JSON block removed.
fn extract_json_block(text: &str) -> (Option<Value>, String) {
    // Try code-fenced JSON blocks first (most common case).
    if let Some((json, before, after)) = extract_fenced_json(text, "json") {
        let mut remaining = String::with_capacity(before.len() + after.len());
        remaining.push_str(before);
        remaining.push_str(after);
        return (json, remaining);
    }
    if let Some((json, before, after)) = extract_fenced_json(text, "") {
        let mut remaining = String::with_capacity(before.len() + after.len());
        remaining.push_str(before);
        remaining.push_str(after);
        return (json, remaining);
    }

    // Try bare JSON array `[...]` anywhere in the text.
    if let Some((parsed, range)) = try_parse_bare_array(text) {
        let mut remaining = String::with_capacity(text.len() - (range.end - range.start));
        remaining.push_str(&text[..range.start]);
        remaining.push_str(&text[range.end..]);
        return (Some(parsed), remaining);
    }

    (None, text.to_string())
}

/// Extract JSON from a ```<fence_lang> ... ``` block.
///
/// Returns `(parsed_json, text_before_block, text_after_block)`.
fn extract_fenced_json<'a>(
    text: &'a str,
    fence_lang: &str,
) -> Option<(Option<Value>, &'a str, &'a str)> {
    let opener = if fence_lang.is_empty() {
        "```"
    } else {
        &format!("```{}", fence_lang)
    };

    let opener_pos = text.find(opener)?;
    let before = &text[..opener_pos];

    let after_opener = &text[opener_pos + opener.len()..];

    let content_start = after_opener.strip_prefix('\n').unwrap_or(after_opener);

    let closer_pos = content_start.find("\n```")?;
    let json_str = &content_start[..closer_pos];

    let after_closer = &content_start[closer_pos + 4..];
    let after = after_closer.strip_prefix('\n').unwrap_or(after_closer);

    match serde_json::from_str(json_str.trim()) {
        Ok(parsed) => Some((Some(parsed), before, after)),
        Err(e) => {
            warn!("Found JSON code fence but failed to parse: {}", e);
            Some((None, before, after))
        }
    }
}

/// Try to parse a bare `[...]` JSON array from the text.
///
/// Returns `(parsed_value, byte_range_in_original_text)`.
fn try_parse_bare_array(text: &str) -> Option<(Value, std::ops::Range<usize>)> {
    let start = text.find('[')?;
    let candidate = &text[start..];

    // Try the entire substring as JSON.
    if let Ok(val @ Value::Array(_)) = serde_json::from_str::<Value>(candidate) {
        let end = start + candidate.len();
        return Some((val, start..end));
    }

    // Try truncating at various points to handle trailing text.
    try_truncated_parse(text, start)
}

/// Try to parse a JSON array by truncating at successive ']' positions.
fn try_truncated_parse(text: &str, start: usize) -> Option<(Value, std::ops::Range<usize>)> {
    let candidate = &text[start..];
    let bytes = candidate.as_bytes();
    let mut last_pos = 0;
    while let Some(pos) = bytes[last_pos + 1..].iter().position(|&b| b == b']') {
        let end = last_pos + 1 + pos + 1;
        match serde_json::from_slice::<Value>(&bytes[..end]) {
            Ok(val @ Value::Array(_)) => {
                return Some((val, start..start + end));
            }
            _ => last_pos = end,
        }
    }
    None
}

/// Validate and convert a single JSON value into a `ReviewFinding`.
fn parse_single_finding(value: Value) -> Option<ReviewFinding> {
    let obj = value.as_object()?;

    let severity = obj.get("severity")?.as_str()?.to_string();
    if !VALID_SEVERITIES.contains(&severity.as_str()) {
        warn!(
            "Invalid severity '{}' — accepted: {:?}",
            severity, VALID_SEVERITIES
        );
        return None;
    }

    let category = obj.get("category")?.as_str()?.to_string();
    if !VALID_CATEGORIES.contains(&category.as_str()) {
        warn!(
            "Invalid category '{}' — accepted: {:?}",
            category, VALID_CATEGORIES
        );
        return None;
    }

    let message = obj.get("message")?.as_str()?.to_string();
    if message.trim().is_empty() {
        warn!("Finding has empty message — dropping");
        return None;
    }

    let file = obj
        .get("file")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let line = obj.get("line").and_then(|v| v.as_u64());
    let suggestion = obj
        .get("suggestion")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());

    Some(ReviewFinding {
        severity,
        file,
        line,
        category,
        message,
        suggestion,
    })
}

fn json_type_name(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The minimum token output below which a length-truncated response won't be retried.
pub(crate) const MIN_TOKENS_FOR_RETRY: u32 = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_text() {
        let result = JsonExtractor::extract("");
        assert!(result.findings.is_empty());
        assert_eq!(result.review_text, "");
    }

    #[test]
    fn test_no_json() {
        let result = JsonExtractor::extract("## Review\n\nLooks good.");
        assert!(result.findings.is_empty());
        assert_eq!(result.review_text, "## Review\n\nLooks good.");
    }

    #[test]
    fn test_json_code_fence() {
        let input = r#"```json
[
  {"severity": "high", "category": "logic_error", "message": "Off by one"}
]
```
## Review
Stuff
"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].severity, "high");
        assert_eq!(result.findings[0].category, "logic_error");
        assert!(result.review_text.contains("## Review"));
        assert!(result.review_text.contains("Stuff"));
    }

    #[test]
    fn test_fence_without_lang() {
        let input = r#"```
[{"severity":"medium","category":"performance","message":"N+1 query"}]
```
"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].severity, "medium");
    }

    #[test]
    fn test_bare_json_array() {
        let input = r#"## Review
[{"severity":"low","category":"best_practice","message":"Use const"}]
Looking good.
"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].severity, "low");
    }

    #[test]
    fn test_multiple_findings() {
        let input = r#"```json
[
  {"severity":"high","category":"security","message":"SQL injection"},
  {"severity":"medium","category":"performance","message":"N+1"},
  {"severity":"low","category":"best_practice","message":"Use .env"}
]
```"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 3);
    }

    #[test]
    fn test_invalid_severity_dropped() {
        let input = r#"```json
[
  {"severity":"critical","category":"security","message":"Bad"},
  {"severity":"high","category":"logic_error","message":"Good"}
]
```"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].severity, "high");
    }

    #[test]
    fn test_invalid_category_dropped() {
        let input = r#"```json
[
  {"severity":"high","category":"unknown_cat","message":"Bad"},
  {"severity":"info","category":"documentation","message":"Good"}
]
```"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].category, "documentation");
    }

    #[test]
    fn test_empty_message_dropped() {
        let input = r#"```json
[
  {"severity":"high","category":"security","message":""},
  {"severity":"high","category":"security","message":"  "},
  {"severity":"high","category":"security","message":"Valid"}
]
```"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].message, "Valid");
    }

    #[test]
    fn test_missing_fields_dropped() {
        let input = r#"```json
[
  {"severity":"high","message":"No category"},
  {"message":"No severity"},
  {"severity":"high","category":"security","message":"Valid"}
]
```"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
    }

    #[test]
    fn test_finding_with_optional_fields() {
        let input = r#"```json
[
  {"severity":"high","category":"security","message":"XSS","file":"src/app.ts","line":42,"suggestion":"Use escape"}
]
```"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        let f = &result.findings[0];
        assert_eq!(f.file.as_deref(), Some("src/app.ts"));
        assert_eq!(f.line, Some(42));
        assert_eq!(f.suggestion.as_deref(), Some("Use escape"));
    }

    #[test]
    fn test_truncated_json_handled() {
        let input = r#"[{"severity":"high","category":"security","message":"XSS"}"#;
        let result = JsonExtractor::extract(input);
        // The bare array parser might not handle this well; should not crash.
        assert!(result.findings.is_empty());
    }

    #[test]
    fn test_not_json_at_start() {
        let input = r#"## Summary
Some text.
```json
[{"severity":"low","category":"other","message":"Note"}]
```"#;
        let result = JsonExtractor::extract(input);
        assert_eq!(result.findings.len(), 1);
        assert!(result.review_text.contains("## Summary"));
        assert!(result.review_text.contains("Some text."));
    }

    #[test]
    fn test_valid_severity_set() {
        for sev in VALID_SEVERITIES {
            let input = format!(
                r#"```json
[{{"severity":"{}","category":"security","message":"Test"}}]
```"#,
                sev
            );
            let result = JsonExtractor::extract(&input);
            assert_eq!(
                result.findings.len(),
                1,
                "Severity '{}' should be valid",
                sev
            );
            assert_eq!(result.findings[0].severity, *sev);
        }
    }

    #[test]
    fn test_valid_category_set() {
        for cat in VALID_CATEGORIES {
            let input = format!(
                r#"```json
[{{"severity":"low","category":"{}","message":"Test"}}]
```"#,
                cat
            );
            let result = JsonExtractor::extract(&input);
            assert_eq!(
                result.findings.len(),
                1,
                "Category '{}' should be valid",
                cat
            );
            assert_eq!(result.findings[0].category, *cat);
        }
    }
}
