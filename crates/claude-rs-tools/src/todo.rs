use crate::Tool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Clone, Default)]
pub struct TodoState {
    pub items: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoState {
    pub fn list(&self) -> Vec<TodoItem> {
        self.items.lock().unwrap().clone()
    }

    pub fn add(&self, id: String, content: String) {
        self.items.lock().unwrap().push(TodoItem {
            id,
            content,
            status: TodoStatus::Pending,
        });
    }

    pub fn update(&self, id: &str, status: TodoStatus) -> anyhow::Result<()> {
        let mut items = self.items.lock().unwrap();
        let item = items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| anyhow::anyhow!("todo item {} not found", id))?;
        item.status = status;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let mut items = self.items.lock().unwrap();
        let pos = items
            .iter()
            .position(|i| i.id == id)
            .ok_or_else(|| anyhow::anyhow!("todo item {} not found", id))?;
        items.remove(pos);
        Ok(())
    }

    pub fn clear(&self) {
        self.items.lock().unwrap().clear();
    }
}

pub struct TodoWriteTool {
    pub state: TodoState,
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "todo_write"
    }

    fn description(&self) -> &'static str {
        "Manage a task list. Supports add, update, delete, and clear operations."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "update", "delete", "clear"],
                    "description": "Action to perform"
                },
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "content": { "type": "string" },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "done"]
                            }
                        },
                        "required": ["id"]
                    },
                    "description": "List of todo items for add or update"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let action = arguments["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing action"))?;

        match action {
            "add" => {
                let todos = arguments["todos"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("missing todos"))?;
                for todo in todos {
                    let id = todo["id"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing todo id"))?;
                    let content = todo["content"].as_str().unwrap_or("");
                    self.state.add(id.to_string(), content.to_string());
                }
                Ok(format!("Added {} todo(s)", todos.len()))
            }
            "update" => {
                let todos = arguments["todos"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("missing todos"))?;
                for todo in todos {
                    let id = todo["id"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing todo id"))?;
                    let status_str = todo["status"].as_str().unwrap_or("pending");
                    let status = match status_str {
                        "pending" => TodoStatus::Pending,
                        "in_progress" => TodoStatus::InProgress,
                        "done" => TodoStatus::Done,
                        _ => anyhow::bail!("invalid status: {}", status_str),
                    };
                    self.state.update(id, status)?;
                }
                Ok(format!("Updated {} todo(s)", todos.len()))
            }
            "delete" => {
                let todos = arguments["todos"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("missing todos"))?;
                for todo in todos {
                    let id = todo["id"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing todo id"))?;
                    self.state.delete(id)?;
                }
                Ok(format!("Deleted {} todo(s)", todos.len()))
            }
            "clear" => {
                self.state.clear();
                Ok("Cleared all todos".to_string())
            }
            _ => anyhow::bail!("unknown action: {}", action),
        }
    }
}
