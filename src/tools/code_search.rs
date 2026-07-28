use crate::tools::Tool;
use async_trait::async_trait;
use serde_json::Value;

/// Search the codebase for a pattern (git grep equivalent).
/// Args: `pattern` (required), `case_sensitive` (optional), `file_patterns` (optional)
pub struct CodeSearch;

#[async_trait]
impl Tool for CodeSearch {
    fn name(&self) -> &str {
        "code_search"
    }

    fn description(&self) -> &str {
        "Search the codebase for a text pattern using git grep. Returns matching file paths with line numbers and content snippets. Max 50 results."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Search pattern (regex supported)" },
                "case_sensitive": { "type": "boolean", "description": "Whether to match case", "default": false },
                "file_patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Only search files matching these globs (e.g. ['*.rs', '*.py'])"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, String> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'pattern' argument".to_string())?;

        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("grep")
            .arg("--line-number")
            .arg("--max-count")
            .arg("50")
            .arg("-I");

        if !args
            .get("case_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            cmd.arg("-i");
        }

        if let Some(patterns) = args.get("file_patterns").and_then(|v| v.as_array()) {
            for fp in patterns {
                if let Some(p) = fp.as_str() {
                    cmd.arg("--include").arg(p);
                }
            }
        }

        cmd.arg("--").arg(pattern);

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to run git grep: {}", e))?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if stdout.is_empty() {
                Ok("No matches found.".to_string())
            } else {
                Ok(stdout)
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("fatal: not a git repository") {
                Err("Not a git repository — code_search requires a git repo".to_string())
            } else if output.status.code() == Some(1) {
                Ok("No matches found.".to_string())
            } else {
                Err(format!("git grep failed: {}", stderr))
            }
        }
    }
}
