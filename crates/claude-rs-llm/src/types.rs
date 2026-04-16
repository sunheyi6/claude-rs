use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Tool {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
        }
    }
}

/// Definition of a tool available to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Why the model stopped generating.
#[derive(Debug, Clone)]
pub enum StopReason {
    /// The model produced a final assistant message.
    End,
    /// The model requested one or more tool calls.
    ToolUse(Vec<ToolCall>),
    /// The model hit a limit (e.g. max_tokens).
    Length,
    /// Unknown or provider-specific stop reason.
    Other(String),
}

/// A streamed chunk from the LLM.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// A piece of the assistant's text response.
    Text(String),
    /// A tool call is starting or being updated.
    ToolCallDelta { index: usize, delta: ToolCallDelta },
    /// The stream ended with this reason.
    Stop(StopReason),
}

/// Incremental update to a tool call during streaming.
#[derive(Debug, Clone, Default)]
pub struct ToolCallDelta {
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Complete response from the LLM for a single turn.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Option<TokenUsage>,
}

/// Token usage information returned by the provider.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Options for a chat completion request.
#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub extra: HashMap<String, serde_json::Value>,
}

impl ChatOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            temperature: None,
            max_tokens: None,
            top_p: None,
            extra: HashMap::new(),
        }
    }
}
