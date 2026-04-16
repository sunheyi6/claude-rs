# claude-rs

A fast Rust-native AI coding agent that chats with an OpenAI-compatible LLM and executes tools to read, write, edit, search, and run shell commands on the local filesystem.

## Project Overview

`claude-rs` is a command-line application built as a Cargo workspace. It implements a classic ReAct-style agent loop:

1. The user sends a message.
2. The agent forwards the conversation to an LLM provider together with a list of available tools.
3. If the model returns text, the agent prints it and ends the turn.
4. If the model requests tool calls, the agent executes them, appends the results back to the conversation, and asks the LLM for the next step.
5. The loop repeats until the model produces a final answer.

The binary name is `claude-rs` and is produced by the `claude-rs-cli` crate.

## Workspace Layout

The project is organised into five crates under `crates/`:

| Crate | Responsibility |
|-------|----------------|
| `claude-rs-cli` | CLI entry point (`src/main.rs`). Parses arguments with `clap`, initialises tracing, wires up the provider and agent, and runs the interactive REPL. |
| `claude-rs-core` | Core agent loop (`Agent`). Manages conversation history, tool registry, orchestrates LLM calls, and contains the `task` subagent implementation. |
| `claude-rs-llm` | LLM abstractions and provider implementations. Currently contains an `OpenAiProvider` that talks to any OpenAI-compatible HTTP endpoint. |
| `claude-rs-tools` | Tool implementations: `bash`, `read`, `write`, `edit`, `grep`, `glob`, `todo_write`. |
| `claude-rs-sandbox` | **Placeholder crate.** Currently only exposes a trivial `add` function and a single unit test. It is not used by the rest of the workspace for sandboxing. |

## Technology Stack

- **Language:** Rust (edition 2024, resolver 3)
- **Async runtime:** Tokio (`full` feature set)
- **HTTP client:** reqwest (with `json`, `stream`, `rustls-tls`)
- **CLI parsing:** clap v4 (derive feature)
- **Serialization:** serde + serde_json
- **Error handling:** anyhow (application) / thiserror (listed for library use)
- **Logging:** tracing + tracing-subscriber (with env-filter)
- **TUI libraries:** ratatui and crossterm are listed in workspace dependencies but are **not currently used**.
- **Tool utilities:**
  - `similar` – text diffing (listed, not yet used)
  - `globset` + `ignore` – glob matching with `.gitignore` support
  - `pulldown-cmark` – Markdown parsing (listed, not yet used)

## Build and Run

```bash
# Build the entire workspace
cargo build --workspace

# Build the release binary
cargo build --release --bin claude-rs

# Run the CLI (requires an OpenAI API key)
cargo run --bin claude-rs -- --api-key $OPENAI_API_KEY --model gpt-4o-mini
```

### CLI Arguments

- `--api-key` (or `OPENAI_API_KEY` env var) – required API key
- `--model` – model name (default: `gpt-4o-mini`)
- `--base-url` – OpenAI-compatible base URL (default: `https://api.openai.com/v1`)
- `--system` – system prompt override

Interactive commands inside the REPL:
- `/quit` or `/exit` – exit
- `/clear` – clear the session (currently prints a message but does not reset state)

## Testing

The project currently has minimal test coverage.

```bash
# Run all workspace tests
cargo test --workspace
```

Only `claude-rs-sandbox` contains a unit test at the moment. The tool crates and core agent logic rely on manual/integration testing via the CLI.

## Architecture Details

### Agent Loop (`claude-rs-core`)

`Agent::new` creates an agent with the default tool set. `Agent::with_task` additionally registers the `task` tool, which allows the model to spawn a **subagent**. The subagent receives its own `Agent` instance but **cannot spawn further subagents** (it is created with `Agent::new` only).

`Agent::run_turn` is the main driver:
- Appends the user message.
- Calls `LlmProvider::chat` with the full message history and tool definitions.
- Handles `StopReason::End`, `StopReason::ToolUse`, `StopReason::Length`, and `StopReason::Other`.
- For tool calls, executes each tool sequentially and appends `Message::Tool` results before looping again.

### LLM Provider (`claude-rs-llm`)

- **`LlmProvider`** trait defines `chat` (non-streaming) and `chat_stream` (streaming).
- **`OpenAiProvider`** implements the trait for any OpenAI-compatible API.
  - `chat_stream` currently falls back to calling `chat()` and yielding a single text chunk followed by a stop chunk.
  - Supports `temperature`, `max_tokens`, `top_p`, and arbitrary extra body fields via `ChatOptions::extra`.

### Message and Tool Types (`claude-rs-llm`)

- `Message` is an enum with `System`, `User`, `Assistant`, and `Tool` variants.
- `ToolDefinition` describes a tool (name, description, JSON Schema parameters).
- `ToolCall` represents a call from the model (id, name, arguments as JSON).
- `StopReason` indicates why generation ended.

### Tools (`claude-rs-tools`)

All tools implement the `Tool` trait:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value; // JSON Schema
    async fn execute(&self, arguments: Value) -> anyhow::Result<String>;
}
```

| Tool | Description |
|------|-------------|
| `bash` | Runs a shell command. Uses PowerShell on Windows and `bash` on Unix. Default timeout is 60 seconds; configurable via `timeout_ms`. Returns exit code, stdout, and stderr. |
| `read` | Reads a file with optional `offset` (1-based line number) and `limit`. Default limit is 2000 lines. Output is prefixed with line numbers. |
| `write` | Writes content to a file, creating parent directories automatically. |
| `edit` | Replaces an exact text fragment in a file. Rejects the operation if the old text appears more than once (to avoid ambiguous edits). |
| `grep` | Searches file contents. Prefers `ripgrep` (`rg`) when available. Falls back to `findstr` on Windows or `grep` on Unix. Supports case-insensitive search, context lines, and a max results cap (default 250). |
| `glob` | Finds files matching a glob pattern (e.g. `src/**/*.rs`). Respects `.gitignore` via the `ignore` crate. |
| `todo_write` | Manages an in-memory todo list (`add`, `update`, `delete`, `clear`). State is held in an `Arc<Mutex<Vec<TodoItem>>>` shared by the agent instance. |

The `task` tool is defined inside `claude-rs-core` rather than `claude-rs-tools` because it needs access to the `Agent` constructor.

## Code Style and Conventions

- **Formatting:** Standard `rustfmt`. Run `cargo fmt --all` before committing.
- **Linting:** Run `cargo clippy --workspace` to catch issues.
- **Error style:** Application code uses `anyhow`. Tool implementations generally return `anyhow::Result<String>` and propagate errors back to the model as text.
- **Logging:** Use `tracing` levels (`info!` for high-level flow, `debug!` for verbose data). The CLI initialises `tracing_subscriber::fmt` with an `EnvFilter` so verbosity is controlled via the `RUST_LOG` environment variable.
- **Async:** All I/O-bound tools are `async` and run under Tokio.
- **Windows awareness:** The `bash` and `grep` tools contain `cfg!(target_os = "windows")` branches or runtime shell detection to work on Windows.

## Security Considerations

- **No actual sandboxing:** Despite the presence of `claude-rs-sandbox`, the agent runs shell commands directly on the host operating system with the same privileges as the user running the binary.
- **Filesystem access:** Tools can read from and write to any path the host process has permission to access. There are no allow-lists or path restrictions.
- **API key exposure:** The CLI accepts the API key via `--api-key` or the `OPENAI_API_KEY` environment variable. Be careful not to log or commit keys.
- **Network:** The `OpenAiProvider` makes outbound HTTPS requests. No proxy or certificate pinning is configured by default.

## Deployment

There is no containerisation, CI configuration, or release automation in the repository yet. Deployment is currently manual: build the binary with `cargo build --release` and distribute `target/release/claude-rs`.
