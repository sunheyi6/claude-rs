use crate::provider::LlmProvider;
use crate::types::{
    ChatOptions, ChatResponse, Message, StopReason, StreamChunk, TokenUsage, ToolCall,
    ToolDefinition,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.api_key)).unwrap(),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_str(&self.user_agent()).unwrap(),
        );
        headers
    }

    fn user_agent(&self) -> String {
        if let Ok(custom) = std::env::var("CLAUDE_RS_USER_AGENT") {
            let trimmed = custom.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }

        // Kimi Coding API currently validates known coding-agent user agents.
        if self.base_url.contains("api.kimi.com/coding") {
            return "claude-code/2.1.107".to_string();
        }

        format!("claude-rs/{}", env!("CARGO_PKG_VERSION"))
    }

    fn build_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &ChatOptions,
        stream: bool,
    ) -> serde_json::Value {
        let msgs: Vec<serde_json::Value> = messages.iter().map(message_to_openai).collect();
        let mut body = json!({
            "model": options.model,
            "messages": msgs,
            "stream": stream,
        });

        if let Some(t) = options.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = options.max_tokens {
            body["max_tokens"] = json!(m);
        }
        if let Some(p) = options.top_p {
            body["top_p"] = json!(p);
        }

        if !tools.is_empty() {
            let tool_specs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = json!(tool_specs);
            body["tool_choice"] = json!("auto");
        }

        for (k, v) in &options.extra {
            body[k] = v.clone();
        }

        body
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &ChatOptions,
    ) -> Result<ChatResponse> {
        let body = self.build_body(messages, tools, options, false);
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .context("failed to send request to OpenAI")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error {}: {}", status, text);
        }

        let data: OpenAiChatResponse = resp
            .json()
            .await
            .context("failed to parse OpenAI response")?;

        let choice = data
            .choices
            .into_iter()
            .next()
            .context("no choices in OpenAI response")?;

        let message = choice.message;
        let text = message.content.unwrap_or_default();
        let tool_calls: Vec<ToolCall> = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: serde_json::from_str(&tc.function.arguments).unwrap_or_default(),
            })
            .collect();

        let stop_reason = match choice.finish_reason.as_deref() {
            Some("stop") => StopReason::End,
            Some("tool_calls") => StopReason::ToolUse(tool_calls.clone()),
            Some("length") => StopReason::Length,
            Some(other) => StopReason::Other(other.to_string()),
            None => StopReason::End,
        };

        Ok(ChatResponse {
            text,
            tool_calls,
            stop_reason,
            usage: data.usage.map(|u| TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &ChatOptions,
    ) -> Result<Box<dyn tokio_stream::Stream<Item = Result<StreamChunk>> + Send + Unpin>> {
        let body = self.build_body(messages, tools, options, true);
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .context("failed to send streaming request to OpenAI")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error {}: {}", status, text);
        }

        let mut bytes_stream = resp.bytes_stream();
        let (tx, rx) = mpsc::unbounded_channel::<Result<StreamChunk>>();

        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut sent_stop = false;

            while let Some(next) = bytes_stream.next().await {
                let bytes = match next {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(Err(anyhow::anyhow!("stream read error: {}", e)));
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(idx) = buffer.find("\n\n") {
                    let event = buffer[..idx].to_string();
                    buffer.drain(..idx + 2);

                    for raw_line in event.lines() {
                        let line = raw_line.trim();
                        if !line.starts_with("data:") {
                            continue;
                        }
                        let data = line.trim_start_matches("data:").trim();
                        if data == "[DONE]" {
                            if !sent_stop {
                                let _ = tx.send(Ok(StreamChunk::Stop(StopReason::End)));
                                sent_stop = true;
                            }
                            continue;
                        }

                        let parsed: OpenAiStreamResponse = match serde_json::from_str(data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        for choice in parsed.choices {
                            if let Some(content) = choice.delta.content {
                                if !content.is_empty() {
                                    let _ = tx.send(Ok(StreamChunk::Text(content)));
                                }
                            }
                            if let Some(reason) = choice.finish_reason {
                                let mapped = match reason.as_str() {
                                    "stop" => StopReason::End,
                                    "tool_calls" => StopReason::ToolUse(Vec::new()),
                                    "length" => StopReason::Length,
                                    other => StopReason::Other(other.to_string()),
                                };
                                let _ = tx.send(Ok(StreamChunk::Stop(mapped)));
                                sent_stop = true;
                            }
                        }
                    }
                }
            }

            if !sent_stop {
                let _ = tx.send(Ok(StreamChunk::Stop(StopReason::End)));
            }
        });

        Ok(Box::new(UnboundedReceiverStream::new(rx)))
    }
}

fn message_to_openai(msg: &Message) -> serde_json::Value {
    match msg {
        Message::System { content } => json!({"role": "system", "content": content}),
        Message::User { content } => json!({"role": "user", "content": content}),
        Message::Assistant { content } => json!({"role": "assistant", "content": content}),
        Message::Tool {
            tool_call_id,
            content,
        } => json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content,
        }),
    }
}

// ------------------------------------------------------------------
// OpenAI response types
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamResponse {
    choices: Vec<OpenAiStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiStreamDelta {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_to_openai_normal_user_message() {
        let value = message_to_openai(&Message::user("hello"));
        assert_eq!(value["role"], "user");
        assert_eq!(value["content"], "hello");
    }

    #[test]
    fn test_message_to_openai_boundary_tool_message() {
        let value = message_to_openai(&Message::tool("tc1", ""));
        assert_eq!(value["role"], "tool");
        assert_eq!(value["tool_call_id"], "tc1");
        assert_eq!(value["content"], "");
    }

    #[test]
    fn test_user_agent_normal_kimi_base_url_returns_fixed_agent() {
        let provider = OpenAiProvider::new("key").with_base_url("https://api.kimi.com/coding/v1");
        assert_eq!(provider.user_agent(), "claude-code/2.1.107");
    }

    #[test]
    fn test_build_body_boundary_includes_optional_fields_and_tools() {
        let provider = OpenAiProvider::new("key");
        let mut options = ChatOptions::new("gpt-test");
        options.temperature = Some(0.5);
        options.max_tokens = Some(128);
        options.top_p = Some(0.9);
        options
            .extra
            .insert("thinking".to_string(), json!({"type":"disabled"}));
        let messages = vec![Message::system("s"), Message::user("u")];
        let tools = vec![ToolDefinition::new(
            "read",
            "Read file",
            json!({"type":"object","properties":{"path":{"type":"string"}}}),
        )];

        let body = provider.build_body(&messages, &tools, &options, true);
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["max_tokens"], 128);
        let top_p = body["top_p"].as_f64().expect("top_p should be f64");
        assert!((top_p - 0.9).abs() < 0.0001);
        assert_eq!(body["tool_choice"], "auto");
        assert!(body["tools"].is_array());
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn test_stream_response_error_case_invalid_json() {
        let parsed = serde_json::from_str::<OpenAiStreamResponse>("not-json");
        assert!(parsed.is_err());
    }
}
