use crate::types::{ChatOptions, ChatResponse, Message, StreamChunk, ToolDefinition};
use async_trait::async_trait;

/// Abstraction over any LLM provider.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat request and return the complete response.
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &ChatOptions,
    ) -> anyhow::Result<ChatResponse>;

    /// Send a chat request and stream chunks back.
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &ChatOptions,
    ) -> anyhow::Result<
        Box<dyn tokio_stream::Stream<Item = anyhow::Result<StreamChunk>> + Send + Unpin>,
    >;
}
