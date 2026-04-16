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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StopReason, ToolCall};

    struct DummyProvider;

    #[async_trait]
    impl LlmProvider for DummyProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &ChatOptions,
        ) -> anyhow::Result<ChatResponse> {
            Ok(ChatResponse {
                text: "ok".to_string(),
                tool_calls: vec![ToolCall {
                    id: "1".to_string(),
                    name: "read".to_string(),
                    arguments: serde_json::json!({"path":"a"}),
                }],
                stop_reason: StopReason::End,
                usage: None,
            })
        }

        async fn chat_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &ChatOptions,
        ) -> anyhow::Result<
            Box<dyn tokio_stream::Stream<Item = anyhow::Result<StreamChunk>> + Send + Unpin>,
        > {
            Ok(Box::new(tokio_stream::iter(vec![
                Ok(StreamChunk::Text("ok".to_string())),
                Ok(StreamChunk::Stop(StopReason::End)),
            ])))
        }
    }

    #[tokio::test]
    async fn test_provider_normal_chat_returns_text() {
        let p = DummyProvider;
        let r = p
            .chat(&[Message::user("hi")], &[], &ChatOptions::new("m"))
            .await
            .expect("chat should succeed");
        assert_eq!(r.text, "ok");
    }

    #[tokio::test]
    async fn test_provider_boundary_stream_has_stop() {
        let p = DummyProvider;
        let mut s = p
            .chat_stream(&[Message::user("hi")], &[], &ChatOptions::new("m"))
            .await
            .expect("stream should succeed");
        let mut seen_stop = false;
        while let Some(item) = tokio_stream::StreamExt::next(&mut s).await {
            if let Ok(StreamChunk::Stop(_)) = item {
                seen_stop = true;
            }
        }
        assert!(seen_stop);
    }

    #[tokio::test]
    async fn test_provider_error_case_not_applicable_dummy_still_ok() {
        let p = DummyProvider;
        let r = p.chat(&[], &[], &ChatOptions::new("m")).await;
        assert!(r.is_ok());
    }
}
