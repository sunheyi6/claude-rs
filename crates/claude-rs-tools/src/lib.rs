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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &'static str {
            "mock_tool"
        }

        fn description(&self) -> &'static str {
            "mock desc"
        }

        fn parameters(&self) -> Value {
            json!({"type":"object"})
        }

        async fn execute(&self, _arguments: Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    #[test]
    fn test_definition_normal_builds_tool_definition() {
        let tool = MockTool;
        let def = definition(&tool);
        assert_eq!(def.name, "mock_tool");
        assert_eq!(def.description, "mock desc");
        assert_eq!(def.parameters["type"], "object");
    }

    #[test]
    fn test_definition_boundary_non_empty_name_and_description() {
        let tool = MockTool;
        let def = definition(&tool);
        assert!(!def.name.is_empty());
        assert!(!def.description.is_empty());
    }

    #[tokio::test]
    async fn test_definition_error_path_not_applicable_execute_still_returns_ok() {
        let tool = MockTool;
        let result = tool.execute(json!({})).await;
        assert!(result.is_ok());
    }
}
