//! LLM tool definitions and registry for the interactive review loop.
//!
//! Tools allow the AI to gather context during review: reading files,
//! searching the codebase, submitting findings, and signalling completion.

use async_trait::async_trait;
use serde_json::Value;

/// A tool that the AI can call during review.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool name (used by the AI to invoke it).
    fn name(&self) -> &str;

    /// A description of what the tool does (sent to the AI).
    fn description(&self) -> &str;

    /// JSON schema for the tool's arguments.
    fn schema(&self) -> Value;

    /// Execute the tool with the given arguments and return a result string.
    async fn execute(&self, args: Value) -> Result<String, String>;
}

/// Registry of tools available to the AI during review.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Get all registered tools as ToolDefs for the AI API.
    pub fn tool_defs(&self) -> Vec<crate::ai::ToolDef> {
        self.tools
            .iter()
            .map(|t| crate::ai::ToolDef {
                tool_type: "function".into(),
                function: crate::ai::ToolFunction {
                    name: t.name().into(),
                    description: Some(t.description().into()),
                    parameters: t.schema(),
                },
            })
            .collect()
    }

    /// Execute a tool by name with the given JSON arguments.
    pub async fn execute(&self, name: &str, args: Value) -> Result<String, String> {
        for tool in &self.tools {
            if tool.name() == name {
                return tool.execute(args).await;
            }
        }
        Err(format!("Unknown tool: {}", name))
    }

    /// Check if a tool with the given name exists.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t.name() == name)
    }

    /// Build a registry with the default set of tools for the code domain.
    pub fn code_domain() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(super::file_read::FileRead));
        reg.register(Box::new(super::code_search::CodeSearch));
        reg.register(Box::new(super::file_find::FileFind));
        reg.register(Box::new(super::submit_finding::SubmitFinding));
        reg.register(Box::new(super::task_done::TaskDone));
        reg
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
