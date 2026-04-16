use crate::provider::LlmProvider;
use crate::types::{
    ChatOptions, ChatResponse, Message, StopReason, StreamChunk, TokenUsage, ToolCall,
    ToolDefinition,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::json;

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
        // MVP fallback: simulate a stream by calling chat() and yielding one text chunk.
        let response = self.chat(messages, tools, options).await?;
        let chunks = vec![
            Ok(StreamChunk::Text(response.text.clone())),
            Ok(StreamChunk::Stop(response.stop_reason.clone())),
        ];
        Ok(Box::new(tokio_stream::iter(chunks)))
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
