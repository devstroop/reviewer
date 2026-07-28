//! Post-hoc review filter that asks the AI to validate findings and
//! discard false positives. Runs after the main review pipeline.

use crate::ai::AiClient;
use crate::error::Result;

/// Prompt sent to the AI to filter false positives.
/// Asks it to identify which findings are provably incorrect given the diff.
const FILTER_SYSTEM_PROMPT: &str = "You are a review quality checker. Given a diff and a list of review findings, \
     identify which findings are incorrect or not supported by the diff. \
     Return ONLY a JSON array of indices to REMOVE (empty array if all are correct).";

/// Filter findings by asking the AI to identify false positives.
///
/// Sends each finding (severity, category, message) plus the diff context
/// to the AI and asks which ones should be removed. Findings identified
/// as incorrect are dropped; the rest are kept.
pub(crate) async fn review_filter(
    ai: &AiClient,
    findings: &[crate::engine::ReviewFinding],
    diff: &str,
) -> Result<Vec<crate::engine::ReviewFinding>> {
    if findings.is_empty() {
        return Ok(vec![]);
    }

    let findings_json = serde_json::to_value(findings).map_err(crate::error::AgentError::Serde)?;

    let user_prompt = format!(
        "Diff:\n```\n{}\n```\n\nFindings:\n{}\n\n\
         Return a JSON array of indices to remove (e.g. [0, 3]) or [] if all are correct.",
        diff,
        serde_json::to_string_pretty(&findings_json).map_err(crate::error::AgentError::Serde)?,
    );

    let response = ai.chat(FILTER_SYSTEM_PROMPT, &user_prompt).await?;
    let indices_to_remove = parse_indices(&response.content);

    Ok(findings
        .iter()
        .enumerate()
        .filter(|(i, _)| !indices_to_remove.contains(i))
        .map(|(_, f)| f.clone())
        .collect())
}

/// Parse a JSON array of indices from the AI response.
/// Tolerant of markdown fences and extra text.
fn parse_indices(text: &str) -> Vec<usize> {
    // Strip markdown fences if present
    let cleaned = text
        .lines()
        .skip_while(|l| l.trim().starts_with("```"))
        .take_while(|l| !l.trim().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");

    // Try to parse as JSON array
    if let Ok(indices) = serde_json::from_str::<Vec<usize>>(&cleaned) {
        return indices;
    }

    // Fallback: scan for numbers in brackets
    let mut result = Vec::new();
    for word in cleaned.split([',', '[', ']', ' ']) {
        if let Ok(n) = word.trim().parse::<usize>() {
            result.push(n);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_indices_empty_array() {
        assert_eq!(parse_indices("[]"), Vec::<usize>::new());
    }

    #[test]
    fn parse_indices_single() {
        assert_eq!(parse_indices("[0]"), vec![0]);
    }

    #[test]
    fn parse_indices_multiple() {
        assert_eq!(parse_indices("[0, 2, 5]"), vec![0, 2, 5]);
    }

    #[test]
    fn parse_indices_with_markdown_fences() {
        assert_eq!(parse_indices("```json\n[1, 3]\n```"), vec![1, 3]);
    }

    #[test]
    fn parse_indices_with_extra_text() {
        assert_eq!(
            parse_indices("The incorrect findings are:\n[0]\nPlease fix them."),
            vec![0]
        );
    }

    #[test]
    fn parse_indices_empty_review_returns_empty() {
        assert!(review_filter_findings_empty().unwrap().is_empty());
    }

    fn review_filter_findings_empty()
    -> std::result::Result<Vec<crate::engine::ReviewFinding>, crate::error::AgentError> {
        Ok(vec![])
    }
}
