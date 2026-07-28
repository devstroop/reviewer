use crate::tools::Tool;
use async_trait::async_trait;
use serde_json::Value;

/// Find files by name pattern.
/// Args: `name` (required), `case_sensitive` (optional)
pub struct FileFind;

#[async_trait]
impl Tool for FileFind {
    fn name(&self) -> &str {
        "file_find"
    }

    fn description(&self) -> &str {
        "Find files by name pattern. Supports glob patterns like '*.rs' or '**/test*'. Max 50 results."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "File name or glob pattern to search for" },
                "case_sensitive": { "type": "boolean", "description": "Whether to match case", "default": false }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, String> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'name' argument".to_string())?;

        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("ls-files")
            .arg("--cached")
            .arg("--others")
            .arg("--exclude-standard");

        if name.contains('*') || name.contains('?') {
            cmd.arg("--glob").arg(name);
        } else {
            cmd.arg("--").arg(name);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to run git ls-files: {}", e))?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let files: Vec<&str> = stdout.lines().take(50).collect();
            if files.is_empty() {
                Ok("No matching files found.".to_string())
            } else {
                Ok(files.join("\n"))
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("git ls-files failed: {}", stderr))
        }
    }
}
