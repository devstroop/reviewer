use serde::{Deserialize, Serialize};

/// Request body sent to the OpenAI-compatible chat completions endpoint.
#[derive(Debug, Serialize, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Tool definitions for function calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
}

/// A message in the chat conversation.
#[derive(Debug, Serialize, Clone)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls made by the assistant (only in assistant messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Tool call ID (only in tool result messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// A tool definition sent to the AI.
#[derive(Debug, Serialize, Clone)]
pub struct ToolDef {
    /// The type of tool (always "function").
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Serialize, Clone)]
pub struct ToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

/// A tool call in the AI response.
#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// Result returned by `AiClient::chat`, combining the response text with
/// optional token-usage, finish-reason, and tool call information.
#[derive(Debug, Clone)]
pub struct ChatOutput {
    /// The text content of the model's response.
    pub content: String,
    /// Token usage reported by the API, if available.
    pub usage: Option<Usage>,
    /// Why the model stopped generating (e.g. "stop", "length", "tool_calls").
    pub finish_reason: Option<String>,
    /// Tool calls requested by the model (when finish_reason is "tool_calls").
    pub tool_calls: Vec<ToolCall>,
}

/// Response from the OpenAI-compatible chat completions endpoint.
#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub id: Option<String>,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: ChoiceMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChoiceMessage {
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}
