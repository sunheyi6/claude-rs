pub mod agents_md;
pub mod compaction;
pub mod permissions;
pub mod session;
pub use permissions::PermissionMode;

use crate::permissions::enforce_tool_permission;
use claude_rs_llm::{ChatOptions, LlmProvider, Message, StopReason, ToolCall, ToolDefinition};
use claude_rs_tools::{
    Tool, bash::BashTool, definition, edit::EditTool, glob::GlobTool, grep::GrepTool,
    read::ReadTool, todo::TodoState, todo::TodoWriteTool, write::WriteTool,
};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

const MAX_MESSAGES_BEFORE_COMPACT: usize = 20;
const KEEP_RECENT_MESSAGES: usize = 6;

pub struct Agent {
    provider: Arc<dyn LlmProvider>,
    tools: Vec<Box<dyn Tool>>,
    messages: Vec<Message>,
    options: ChatOptions,
    agents_md: String,
    permission_mode: PermissionMode,
    workspace_root: PathBuf,
}

impl Agent {
    pub fn new(provider: Arc<dyn LlmProvider>, options: ChatOptions) -> Self {
        let todo_state = TodoState::default();
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(BashTool::default()),
            Box::new(ReadTool),
            Box::new(WriteTool),
            Box::new(EditTool),
            Box::new(GrepTool),
            Box::new(GlobTool),
            Box::new(TodoWriteTool { state: todo_state }),
        ];
        Self {
            provider,
            tools,
            messages: Vec::new(),
            options,
            agents_md: String::new(),
            permission_mode: PermissionMode::WorkspaceWrite,
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    pub fn with_task(mut self) -> Self {
        self.tools.push(Box::new(TaskTool {
            provider: self.provider.clone(),
            options: self.options.clone(),
        }));
        self
    }

    pub async fn load_agents_md(&mut self, cwd: &Path) -> anyhow::Result<()> {
        self.agents_md = agents_md::load_agents_md(cwd, None).await?;
        self.rebuild_system_prompt();
        Ok(())
    }

    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.messages
            .retain(|m| !matches!(m, Message::System { .. }));
        let base = prompt.into();
        let combined = if self.agents_md.is_empty() {
            base
        } else {
            format!("{}\n\n---\n\n{}", base, self.agents_md)
        };
        self.messages.insert(0, Message::system(combined));
    }

    fn rebuild_system_prompt(&mut self) {
        let base = self
            .messages
            .iter()
            .find_map(|m| match m {
                Message::System { content } => Some(content.clone()),
                _ => None,
            })
            .unwrap_or_default();

        self.messages
            .retain(|m| !matches!(m, Message::System { .. }));
        let combined = if self.agents_md.is_empty() {
            base
        } else {
            format!("{}\n\n---\n\n{}", base, self.agents_md)
        };
        if !combined.trim().is_empty() {
            self.messages.insert(0, Message::system(combined));
        }
    }

    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
    }

    pub fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.options.model = model.into();
    }

    pub fn model(&self) -> &str {
        &self.options.model
    }

    pub fn compact_now(&mut self) -> usize {
        let before = self.messages.len();
        compaction::compact_messages(&mut self.messages, KEEP_RECENT_MESSAGES);
        before.saturating_sub(self.messages.len())
    }

    pub fn status_summary(&self) -> String {
        format!(
            "model: {}\npermission_mode: {}\nmessages: {}\nest_tokens: {}\nworkspace_root: {}",
            self.options.model,
            self.permission_mode,
            self.messages.len(),
            compaction::total_tokens(&self.messages),
            self.workspace_root.display()
        )
    }

    pub fn clear_session(&mut self) {
        let system = self.messages.iter().find_map(|m| match m {
            Message::System { content } => Some(Message::system(content.clone())),
            _ => None,
        });
        self.messages.clear();
        if let Some(s) = system {
            self.messages.push(s);
        }
    }

    pub async fn save_session(&self) -> anyhow::Result<String> {
        let id = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let session = session::Session {
            id: id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            messages: self.messages.clone(),
        };
        session::save_session(&session).await?;
        Ok(id)
    }

    pub async fn load_session(&mut self, id: &str) -> anyhow::Result<()> {
        let session = session::load_session(id).await?;
        self.messages = session.messages;
        Ok(())
    }

    pub async fn run_turn(&mut self, user_input: impl Into<String>) -> anyhow::Result<String> {
        self.messages.push(Message::user(user_input));

        if self.messages.len() > MAX_MESSAGES_BEFORE_COMPACT {
            compaction::compact_messages(&mut self.messages, KEEP_RECENT_MESSAGES);
            info!("compacted conversation to {} messages", self.messages.len());
        }

        loop {
            let tool_defs: Vec<ToolDefinition> =
                self.tools.iter().map(|t| definition(t.as_ref())).collect();

            info!(
                "calling llm with {} messages (~{} est. tokens)",
                self.messages.len(),
                compaction::total_tokens(&self.messages)
            );
            debug!(
                "tools: {:?}",
                tool_defs.iter().map(|d| &d.name).collect::<Vec<_>>()
            );

            let response = self
                .provider
                .chat(&self.messages, &tool_defs, &self.options)
                .await?;

            match response.stop_reason {
                StopReason::End => {
                    self.messages
                        .push(Message::assistant(response.text.clone()));
                    return Ok(response.text);
                }
                StopReason::ToolUse(calls) => {
                    self.messages
                        .push(Message::assistant(response.text.clone()));

                    for call in calls {
                        let result = self.execute_tool(&call).await;
                        let content = match result {
                            Ok(output) => output,
                            Err(e) => format!("Error: {}", e),
                        };
                        self.messages.push(Message::tool(call.id.clone(), content));
                    }
                }
                StopReason::Length => {
                    return Ok("[Response was truncated due to length limit]".to_string());
                }
                StopReason::Other(reason) => {
                    return Ok(format!("[Stopped: {}]", reason));
                }
            }
        }
    }

    async fn execute_tool(&self, call: &ToolCall) -> anyhow::Result<String> {
        enforce_tool_permission(self.permission_mode, &self.workspace_root, call)?;

        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == call.name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", call.name))?;

        info!(
            "executing tool {} with args {:?}",
            call.name, call.arguments
        );
        tool.execute(call.arguments.clone()).await
    }
}

use async_trait::async_trait;
use serde_json::{Value, json};

struct TaskTool {
    provider: Arc<dyn LlmProvider>,
    options: ChatOptions,
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &'static str {
        "task"
    }

    fn description(&self) -> &'static str {
        "Spawn a subagent to handle a specific task. The subagent has its own context and cannot spawn further subagents."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Detailed description of the task for the subagent"
                }
            },
            "required": ["description"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let description = arguments["description"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing description"))?;

        let mut subagent = Agent::new(self.provider.clone(), self.options.clone());
        subagent.set_permission_mode(PermissionMode::WorkspaceWrite);
        subagent.set_system_prompt(
            "You are a focused subagent. You have access to bash, read, write, edit, grep, glob, and todo_write. \
             You CANNOT use the task tool. Work efficiently and return a concise summary of what you accomplished."
        );

        let result = subagent.run_turn(description).await?;
        Ok(format!("Subagent completed task. Summary: {}", result))
    }
}
