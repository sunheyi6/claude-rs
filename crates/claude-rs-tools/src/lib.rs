pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod todo;
pub mod write;

use async_trait::async_trait;
use claude_rs_llm::ToolDefinition;
use serde_json::Value;

/// A tool that the agent can execute.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name of the tool.
    fn name(&self) -> &'static str;

    /// Human-readable description.
    fn description(&self) -> &'static str;

    /// JSON Schema for the tool's parameters.
    fn parameters(&self) -> Value;

    /// Execute the tool with the given arguments.
    async fn execute(&self, arguments: Value) -> anyhow::Result<String>;
}

/// Build a [`ToolDefinition`] from a [`Tool`] implementation.
pub fn definition(tool: &dyn Tool) -> ToolDefinition {
    ToolDefinition::new(tool.name(), tool.description(), tool.parameters())
}
