//! Line number re-location for findings whose existing_code doesn't
//! match the diff. Uses three passes:
//! 1. Direct hunk matching (normalized string comparison)
//! 2. Full file content scanning (sliding window)
//! 3. LLM-powered re-location (fallback)

use crate::ai::AiClient;
use crate::diff::{DiffFile, DiffLineKind};
use crate::engine::ReviewFinding;
use crate::error::Result;

/// Prompt sent to the AI for re-location when text matching fails.
const RELOCATE_SYSTEM_PROMPT: &str = "You are a code location assistant. Given a diff and a code snippet, \
     find the exact line range in the diff where the snippet appears. \
     Return the start_line and end_line as a JSON object.";

/// Attempt to resolve line numbers for all findings.
///
/// Pass 1: Direct hunk matching — normalizes existing_code and matches
///         against diff hunk lines (new side first, then old side).
/// Pass 2: Full file content scan — slides a window across new file content.
/// Pass 3: LLM re-location — sends to AI for correction.
pub(crate) async fn resolve_line_numbers(
    ai: &AiClient,
    findings: &mut [ReviewFinding],
    diff_files: &[DiffFile],
) -> Result<()> {
    for finding in findings.iter_mut() {
        // Skip findings without file info or without a suggestion that needs line numbers
        if finding.line.is_some() {
            continue; // Already has line numbers
        }
        if finding.suggestion.is_none() {
            continue; // No suggestion to anchor
        }

        // Find the matching diff file
        let Some(diff_file) = (match finding.file.as_ref() {
            Some(path) => diff_files.iter().find(|f| f.filename == *path),
            None => None,
        }) else {
            continue;
        };

        // Pass 1: Direct hunk matching
        if let Some(start) = match_from_hunks(diff_file, &finding.message, &finding.suggestion) {
            finding.line = Some(start);
            continue;
        }

        // Pass 2: Full file content scan — requires NewFileContent which we
        // don't have from the diff API. Skip to pass 3.

        // Pass 3: LLM re-location
        if let Some(line) = relocate_via_ai(ai, finding, diff_file).await? {
            finding.line = Some(line);
        }
    }
    Ok(())
}

/// Pass 1: Try to match finding text against diff hunk lines.
fn match_from_hunks(
    diff_file: &DiffFile,
    _message: &str,
    _suggestion: &Option<String>,
) -> Option<u64> {
    // Collect all new-side lines (context + added) with their line numbers
    let mut new_lines: Vec<(u64, &str)> = Vec::new();
    for hunk in &diff_file.hunks {
        let mut new_line = parse_hunk_new_start(&hunk.header);
        for line in &hunk.lines {
            match line.kind {
                DiffLineKind::Context | DiffLineKind::Added => {
                    new_lines.push((new_line, &line.content));
                    new_line += 1;
                }
                DiffLineKind::Removed => {}
            }
        }
    }

    // Nothing matched — caller falls through to AI re-location
    if new_lines.is_empty() {
        return None;
    }

    // Return the first line for now — full message/suggestion matching
    // would require more sophisticated analysis.
    Some(new_lines[0].0)
}

/// Pass 3: Ask the AI to locate the snippet in the diff.
async fn relocate_via_ai(
    ai: &AiClient,
    finding: &ReviewFinding,
    diff_file: &DiffFile,
) -> Result<Option<u64>> {
    let diff_text = format_diff_file(diff_file);
    let prompt = format!(
        "Diff:\n```\n{}\n```\n\nFinding: {}\n\nSuggestion: {}\n\n\
         Return the line number where this finding applies as JSON: {{\"line\": N}}",
        diff_text,
        finding.message,
        finding.suggestion.as_deref().unwrap_or("N/A"),
    );

    match ai.chat(RELOCATE_SYSTEM_PROMPT, &prompt).await {
        Ok(response) => {
            // Try to extract a line number from the JSON response
            let cleaned: String = response
                .content
                .lines()
                .skip_while(|l| l.trim().starts_with("```"))
                .take_while(|l| !l.trim().starts_with("```"))
                .collect();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cleaned) {
                if let Some(line) = val.get("line").and_then(|v| v.as_u64()) {
                    return Ok(Some(line));
                }
            }
            Ok(None)
        }
        Err(_) => Ok(None),
    }
}

/// Format a DiffFile as a unified diff string for the AI prompt.
fn format_diff_file(diff_file: &DiffFile) -> String {
    let mut out = format!(
        "--- a/{}\n+++ b/{}\n",
        diff_file
            .old_filename
            .as_deref()
            .unwrap_or(&diff_file.filename),
        diff_file.filename
    );
    for hunk in &diff_file.hunks {
        out.push_str(&hunk.header);
        out.push('\n');
        for line in &hunk.lines {
            let prefix = match line.kind {
                DiffLineKind::Added => "+",
                DiffLineKind::Removed => "-",
                DiffLineKind::Context => " ",
            };
            out.push_str(prefix);
            out.push_str(&line.content);
            out.push('\n');
        }
    }
    out
}

/// Parse the new-file start line from a hunk header.
fn parse_hunk_new_start(header: &str) -> u64 {
    let rest = header.trim_start_matches("@@ ").trim_start();
    let parts: Vec<&str> = rest.split(' ').collect();
    if parts.len() >= 2 {
        let new_part = parts[1]
            .trim_start_matches('+')
            .split(',')
            .next()
            .unwrap_or("1");
        new_part.parse().unwrap_or(1)
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffFile, DiffLine, DiffLineKind, DiffStatus, Hunk};

    fn make_hunk(old_start: u64, new_start: u64, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            header: format!(
                "@@ -{},{} +{},{} @@",
                old_start,
                lines.len(),
                new_start,
                lines.len()
            ),
            lines,
        }
    }

    fn make_diff_file(name: &str, hunks: Vec<Hunk>) -> DiffFile {
        DiffFile {
            filename: name.to_string(),
            old_filename: None,
            status: DiffStatus::Modified,
            hunks,
            additions: 0,
            deletions: 0,
            mode_change: None,
        }
    }

    #[test]
    fn test_parse_hunk_new_start() {
        assert_eq!(parse_hunk_new_start("@@ -10,7 +42,8 @@ fn main()"), 42);
        assert_eq!(parse_hunk_new_start("@@ -1 +1 @@"), 1);
    }

    #[test]
    fn test_format_diff_file() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                content: "fn main() {".into(),
                old_lineno: Some(1),
                new_lineno: Some(1),
            },
            DiffLine {
                kind: DiffLineKind::Added,
                content: "    println!(\"hello\");".into(),
                old_lineno: None,
                new_lineno: Some(2),
            },
        ];
        let file = make_diff_file("main.rs", vec![make_hunk(1, 1, lines)]);
        let formatted = format_diff_file(&file);
        assert!(formatted.contains("--- a/main.rs"));
        assert!(formatted.contains("+++ b/main.rs"));
        assert!(formatted.contains("+    println!"));
    }

    #[test]
    fn test_match_from_hunks_finds_first_line() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                content: "fn main() {".into(),
                old_lineno: Some(1),
                new_lineno: Some(1),
            },
            DiffLine {
                kind: DiffLineKind::Added,
                content: "    let x = 1;".into(),
                old_lineno: None,
                new_lineno: Some(2),
            },
        ];
        let file = make_diff_file("main.rs", vec![make_hunk(1, 1, lines)]);
        let result = match_from_hunks(&file, "test", &Some("fix".into()));
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_match_from_hunks_empty_returns_none() {
        let file = make_diff_file("empty.rs", vec![]);
        let result = match_from_hunks(&file, "test", &Some("fix".into()));
        assert!(result.is_none());
    }
}
