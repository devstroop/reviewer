use crate::tools::Tool;
use async_trait::async_trait;
use serde_json::Value;

/// Submit a review finding mid-review.
/// Args: `severity` (required), `category` (required), `message` (required),
///       `file` (optional), `line` (optional), `suggestion` (optional)
pub struct SubmitFinding;

#[async_trait]
impl Tool for SubmitFinding {
    fn name(&self) -> &str {
        "submit_finding"
    }

    fn description(&self) -> &str {
        "Submit a review finding for a specific issue found in the code. Use this when you identify a bug, security concern, or other issue. You will still need to call task_done when finished."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "severity": {
                    "type": "string",
                    "enum": ["high", "medium", "low", "info"],
                    "description": "Severity level of the finding"
                },
                "category": {
                    "type": "string",
                    "enum": ["logic_error", "security", "api_misuse", "performance",
                             "missing_edge_case", "best_practice", "style", "documentation", "other"],
                    "description": "Category of the finding"
                },
                "message": { "type": "string", "description": "Description of the issue" },
                "file": { "type": "string", "description": "File path relative to repo root (optional)" },
                "line": { "type": "integer", "description": "Line number (1-based, optional)" },
                "suggestion": { "type": "string", "description": "Suggested fix or improvement (optional)" }
            },
            "required": ["severity", "category", "message"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, String> {
        let severity = args
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("other");
        let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");

        if message.is_empty() {
            return Err("'message' is required for submit_finding".to_string());
        }

        let valid_severities = ["high", "medium", "low", "info"];
        if !valid_severities.contains(&severity) {
            return Err(format!(
                "Invalid severity '{}'. Must be one of: high, medium, low, info",
                severity
            ));
        }

        Ok(format!(
            "✅ Finding submitted: [{}] {} — {}",
            severity, category, message
        ))
    }
}
