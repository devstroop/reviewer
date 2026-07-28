use crate::tools::Tool;
use async_trait::async_trait;
use serde_json::Value;

/// Signal that the review is complete.
/// No arguments. When called, the tool loop terminates.
pub struct TaskDone;

#[async_trait]
impl Tool for TaskDone {
    fn name(&self) -> &str {
        "task_done"
    }

    fn description(&self) -> &str {
        "Call this when you have completed the review and submitted all findings. This signals that you are done and no further analysis is needed."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: Value) -> Result<String, String> {
        Ok("Task completed.".to_string())
    }
}
