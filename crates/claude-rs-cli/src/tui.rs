use claude_rs_core::{Agent, PermissionMode, session};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use std::io;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

use claude_rs_llm::{ChatOptions, openai::OpenAiProvider};
use std::path::PathBuf;

const ACCENT: Color = Color::Rgb(255, 107, 107);
const DIM: Color = Color::Rgb(120, 120, 120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ask,
    Code,
    Plan,
}

impl Mode {
    fn label(&self) -> &'static str {
        match self {
            Mode::Ask => "ASK",
            Mode::Code => "CODE",
            Mode::Plan => "PLAN",
        }
    }

    fn color(&self) -> Color {
        match self {
            Mode::Ask => Color::Cyan,
            Mode::Code => Color::Green,
            Mode::Plan => Color::Yellow,
        }
    }
}

pub struct TuiConfig {
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: String,
    pub system: String,
    pub mode: Mode,
    pub resume: Option<String>,
    pub permission_mode: PermissionMode,
}

#[derive(Debug, Clone)]
enum Sender {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
struct UiMessage {
    sender: Sender,
    text: String,
}

enum AppEvent {
    Key(KeyEvent),
    Paste(String),
    AgentResponse(Result<String, String>),
}

enum Screen {
    ApiKeyInput,
    Welcome,
    Chat,
}

struct App {
    screen: Screen,
    input: String,
    api_key_input: String,
    pending_message: Option<String>,
    messages: Vec<UiMessage>,
    scroll: usize,
    mode: Mode,
    waiting: bool,
}

impl App {
    fn new(mode: Mode) -> Self {
        Self {
            screen: Screen::Welcome,
            input: String::new(),
            api_key_input: String::new(),
            pending_message: None,
            messages: Vec::new(),
            scroll: 0,
            mode,
            waiting: false,
        }
    }

    fn push_message(&mut self, sender: Sender, text: impl Into<String>) {
        self.screen = Screen::Chat;
        self.messages.push(UiMessage {
            sender,
            text: text.into(),
        });
        self.scroll = self.messages.len().saturating_sub(1);
    }
}

pub async fn run(config: TuiConfig) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        event::EnableMouseCapture,
        event::EnableBracketedPaste
    )?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = app_loop(&mut terminal, config).await;

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        event::DisableMouseCapture,
        event::DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    result
}

async fn app_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    config: TuiConfig,
) -> anyhow::Result<()> {
    let mut app = App::new(config.mode);
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let agent: Arc<Mutex<Option<Agent>>> = Arc::new(Mutex::new(None));

    // Pre-build agent if API key is available
    if let Some(key) = config.api_key {
        let provider: Arc<dyn claude_rs_llm::LlmProvider> =
            Arc::new(OpenAiProvider::new(key).with_base_url(&config.base_url));
        let options = ChatOptions::new(&config.model);
        let mut new_agent = Agent::new(provider, options).with_task();
        new_agent.set_system_prompt(&config.system);
        new_agent.set_permission_mode(config.permission_mode);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let _ = new_agent.load_agents_md(&cwd).await;
        if let Some(ref session_id) = config.resume {
            let _ = new_agent.load_session(session_id).await;
        }
        *agent.lock().await = Some(new_agent);
    }

    let event_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            match event::read() {
                Ok(Event::Key(key)) => {
                    if event_tx.send(AppEvent::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(Event::Paste(text)) => {
                    if event_tx.send(AppEvent::Paste(text)).is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }
    });

    loop {
        terminal.draw(|f| draw(f, &app))?;

        let evt = tokio::select! {
            biased;
            Some(e) = rx.recv() => e,
            else => break,
        };

        match evt {
            AppEvent::Paste(text) => match app.screen {
                Screen::ApiKeyInput => app.api_key_input.push_str(&text),
                _ => app.input.push_str(&text),
            },
            AppEvent::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match app.screen {
                    Screen::ApiKeyInput => match key.code {
                        KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break;
                        }
                        KeyCode::Enter => {
                            let key = app.api_key_input.trim().to_string();
                            if key.is_empty() {
                                continue;
                            }
                            let provider: Arc<dyn claude_rs_llm::LlmProvider> =
                                Arc::new(OpenAiProvider::new(key).with_base_url(&config.base_url));
                            let options = ChatOptions::new(&config.model);
                            let mut new_agent = Agent::new(provider, options).with_task();
                            new_agent.set_system_prompt(&config.system);
                            new_agent.set_permission_mode(config.permission_mode);
                            let cwd =
                                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                            let _ = new_agent.load_agents_md(&cwd).await;
                            if let Some(ref session_id) = config.resume {
                                let _ = new_agent.load_session(session_id).await;
                            }
                            *agent.lock().await = Some(new_agent);
                            app.screen = Screen::Chat;
                            app.api_key_input.clear();

                            // If there was a pending message, send it now
                            if let Some(text) = app.pending_message.take() {
                                app.push_message(Sender::User, text.clone());
                                app.waiting = true;
                                let response_tx = tx.clone();
                                let mode = app.mode;
                                let agent = agent.clone();
                                tokio::spawn(async move {
                                    let result = if mode == Mode::Plan {
                                        Ok(format!(
                                            "[Plan mode] I would analyze and plan the following: {}\n\nSwitch to CODE mode to execute.",
                                            text
                                        ))
                                    } else {
                                        match agent.lock().await.as_mut() {
                                            Some(a) => match a.run_turn(text).await {
                                                Ok(reply) => Ok(reply),
                                                Err(e) => Err(e.to_string()),
                                            },
                                            None => Err("代理尚未初始化".to_string()),
                                        }
                                    };
                                    let _ = response_tx.send(AppEvent::AgentResponse(result));
                                });
                            }
                        }
                        KeyCode::Char(c) => app.api_key_input.push(c),
                        KeyCode::Backspace => {
                            app.api_key_input.pop();
                        }
                        _ => {}
                    },
                    _ => match key.code {
                        KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break;
                        }
                        KeyCode::Tab => {
                            app.mode = match app.mode {
                                Mode::Ask => Mode::Code,
                                Mode::Code => Mode::Plan,
                                Mode::Plan => Mode::Ask,
                            };
                        }
                        KeyCode::Enter if !app.waiting => {
                            let text = app.input.trim().to_string();
                            if text.is_empty() {
                                continue;
                            }
                            app.input.clear();

                            if handle_local_command(&text, &mut app, &agent).await? {
                                continue;
                            }

                            // Check if we need API key first
                            let has_agent = agent.lock().await.is_some();
                            if !has_agent {
                                app.pending_message = Some(text);
                                app.screen = Screen::ApiKeyInput;
                                continue;
                            }

                            app.push_message(Sender::User, text.clone());
                            app.waiting = true;

                            let response_tx = tx.clone();
                            let mode = app.mode;
                            let agent = agent.clone();
                            tokio::spawn(async move {
                                let result = if mode == Mode::Plan {
                                    Ok(format!(
                                        "[Plan mode] I would analyze and plan the following: {}\n\nSwitch to CODE mode to execute.",
                                        text
                                    ))
                                } else {
                                    match agent.lock().await.as_mut() {
                                        Some(a) => match a.run_turn(text).await {
                                            Ok(reply) => Ok(reply),
                                            Err(e) => Err(e.to_string()),
                                        },
                                        None => Err("代理尚未初始化".to_string()),
                                    }
                                };
                                let _ = response_tx.send(AppEvent::AgentResponse(result));
                            });
                        }
                        KeyCode::Char(c) => app.input.push(c),
                        KeyCode::Backspace => {
                            app.input.pop();
                        }
                        KeyCode::Up => {
                            app.scroll = app.scroll.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            app.scroll = app.scroll.saturating_add(1);
                        }
                        KeyCode::PageUp => {
                            app.scroll = app.scroll.saturating_sub(5);
                        }
                        KeyCode::PageDown => {
                            app.scroll = app.scroll.saturating_add(5);
                        }
                        _ => {}
                    },
                }
            }
            AppEvent::AgentResponse(result) => {
                app.waiting = false;
                match result {
                    Ok(text) => app.push_message(Sender::Assistant, text),
                    Err(e) => app.push_message(Sender::System, format!("错误：{}", e)),
                }
            }
        }
    }

    Ok(())
}

async fn handle_local_command(
    input: &str,
    app: &mut App,
    agent: &Arc<Mutex<Option<Agent>>>,
) -> anyhow::Result<bool> {
    if !input.starts_with('/') {
        return Ok(false);
    }

    match input {
        "/clear" => {
            if let Some(a) = agent.lock().await.as_mut() {
                a.clear_session();
            }
            app.messages.clear();
            app.push_message(Sender::System, "会话已清空。");
            Ok(true)
        }
        "/status" => {
            let result = match agent.lock().await.as_ref() {
                Some(a) => a.status_summary(),
                None => "代理尚未初始化。".to_string(),
            };
            app.push_message(Sender::System, result);
            Ok(true)
        }
        "/compact" => {
            let result = match agent.lock().await.as_mut() {
                Some(a) => {
                    let dropped = a.compact_now();
                    format!("会话已压缩，移除了 {} 条消息。", dropped)
                }
                None => "代理尚未初始化。".to_string(),
            };
            app.push_message(Sender::System, result);
            Ok(true)
        }
        "/save" => {
            let result = match agent.lock().await.as_mut() {
                Some(a) => a
                    .save_session()
                    .await
                    .map(|id| format!("会话已保存：{}", id))
                    .unwrap_or_else(|e| format!("保存会话失败：{}", e)),
                None => "代理尚未初始化，无法保存会话。".to_string(),
            };
            app.push_message(Sender::System, result);
            Ok(true)
        }
        "/history" => {
            let result = match session::list_sessions().await {
                Ok(sessions) if sessions.is_empty() => "没有保存的会话。".to_string(),
                Ok(sessions) => sessions
                    .into_iter()
                    .map(|s| {
                        format!(
                            "{} - {} messages - {}",
                            s.id,
                            s.messages.len(),
                            s.updated_at
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(e) => format!("列出会话失败：{}", e),
            };
            app.push_message(Sender::System, result);
            Ok(true)
        }
        "/init" => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let path = cwd.join("AGENTS.md");
            let msg = if tokio::fs::metadata(&path).await.is_ok() {
                format!("已存在：{}", path.display())
            } else {
                let content = default_agents_md_template();
                match tokio::fs::write(&path, content).await {
                    Ok(_) => {
                        if let Some(a) = agent.lock().await.as_mut() {
                            let _ = a.load_agents_md(&cwd).await;
                        }
                        format!("已创建：{}（并已尝试加载）", path.display())
                    }
                    Err(e) => format!("创建 AGENTS.md 失败：{}", e),
                }
            };
            app.push_message(Sender::System, msg);
            Ok(true)
        }
        cmd if cmd.starts_with("/resume ") => {
            let id = cmd.strip_prefix("/resume ").unwrap_or("").trim();
            if id.is_empty() {
                app.push_message(Sender::System, "用法：/resume <会话ID>");
                return Ok(true);
            }

            let result = match agent.lock().await.as_mut() {
                Some(a) => a
                    .load_session(id)
                    .await
                    .map(|_| format!("已恢复会话 {}。", id))
                    .unwrap_or_else(|e| format!("恢复会话失败：{}", e)),
                None => "代理尚未初始化，无法恢复会话。".to_string(),
            };
            app.push_message(Sender::System, result);
            Ok(true)
        }
        cmd if cmd.starts_with("/permissions ") => {
            let value = cmd.strip_prefix("/permissions ").unwrap_or("").trim();
            let result = match PermissionMode::parse(value) {
                Some(m) => match agent.lock().await.as_mut() {
                    Some(a) => {
                        a.set_permission_mode(m);
                        format!("权限模式已更新为：{}", m)
                    }
                    None => "代理尚未初始化。".to_string(),
                },
                None => {
                    "无效模式，可选 read-only / workspace-write / danger-full-access".to_string()
                }
            };
            app.push_message(Sender::System, result);
            Ok(true)
        }
        cmd if cmd.starts_with("/model ") => {
            let model = cmd.strip_prefix("/model ").unwrap_or("").trim();
            let result = if model.is_empty() {
                "用法：/model <模型名>".to_string()
            } else {
                match agent.lock().await.as_mut() {
                    Some(a) => {
                        a.set_model(model.to_string());
                        format!("模型已切换为：{}", a.model())
                    }
                    None => "代理尚未初始化。".to_string(),
                }
            };
            app.push_message(Sender::System, result);
            Ok(true)
        }
        "/quit" | "/exit" => {
            app.push_message(Sender::System, "请按 Esc 或 Ctrl+C 退出。");
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn default_agents_md_template() -> &'static str {
    r#"# AGENTS.md

## Project Instructions

- 在这个项目中优先使用 `rg` 做搜索。
- 修改代码后运行相关检查（如 `cargo check` / `cargo test`）。
- 回答时给出关键文件路径与原因说明。
"#
}

fn draw(frame: &mut Frame, app: &App) {
    let full_area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(3)])
        .split(full_area);

    match app.screen {
        Screen::ApiKeyInput => draw_api_key_dialog(frame, app, chunks[0]),
        Screen::Welcome => draw_welcome(frame, app, chunks[0]),
        Screen::Chat => draw_messages(frame, app, chunks[0], full_area),
    }

    draw_input_bar(frame, app, chunks[1]);
}

fn draw_api_key_dialog(frame: &mut Frame, app: &App, area: Rect) {
    // Render directly in the main content area, no popup window border
    let inner = area.inner(Margin {
        horizontal: 4,
        vertical: 4,
    });

    let text = Text::from(vec![
        Line::from(Span::styled(
            "需要 API 密钥",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "请输入 Kimi (Moonshot) API 密钥以继续：",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.api_key_input.as_str(),
            Style::default().fg(Color::Green),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "按 Enter 确认，按 Esc 退出",
            Style::default().fg(DIM),
        )),
    ]);
    frame.render_widget(Paragraph::new(text), inner);
}

fn draw_welcome(frame: &mut Frame, _app: &App, area: Rect) {
    let title = format!(" claude-rs v{} ", env!("CARGO_PKG_VERSION"));
    let panel = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));
    let inner = panel.inner(area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    }));
    frame.render_widget(
        panel,
        area.inner(Margin {
            horizontal: 2,
            vertical: 1,
        }),
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(inner);

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cwd_str = cwd.display().to_string();
    let home_dir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let in_home = !home_dir.is_empty() && cwd_str.eq_ignore_ascii_case(&home_dir);
    let location_hint = if in_home {
        "Note: 你当前在 home 目录启动，建议进入具体项目目录后再使用。".to_string()
    } else {
        format!("Project: {}", cwd_str)
    };

    let left_text = Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Welcome back!",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("    _ _"),
        Line::from("  _(o o)_"),
        Line::from(" /  \\_/  \\"),
        Line::from(" \\__/ \\__/"),
        Line::from(""),
        Line::from(Span::styled(
            format!("{} · {}", whoami::username(), cwd_str),
            Style::default().fg(DIM),
        )),
    ]);
    frame.render_widget(
        Paragraph::new(left_text).alignment(Alignment::Center),
        cols[0],
    );

    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .split(cols[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Tips for getting started",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))),
        right_rows[0],
    );
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from("Run /init to create AGENTS.md with project instructions."),
            Line::from("Use Tab to switch ASK / CODE / PLAN mode."),
            Line::from("Use /save and /history to manage sessions."),
            Line::from(location_hint),
            Line::from(""),
        ]))
        .style(Style::default().fg(Color::Gray)),
        right_rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(
            "--------------------------------------------------------",
        ))
        .style(Style::default().fg(DIM)),
        right_rows[2],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Recent activity",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))),
        right_rows[3],
    );
    frame.render_widget(
        Paragraph::new("No recent activity").style(Style::default().fg(Color::Gray)),
        right_rows[4],
    );
}

fn draw_messages(frame: &mut Frame, app: &App, area: Rect, full_area: Rect) {
    let outer = Block::default()
        .title(Span::styled(
            " Conversation ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let message_text: Vec<Line> = app
        .messages
        .iter()
        .flat_map(|msg| {
            let (title, color) = match msg.sender {
                Sender::User => ("你（已发送）", Color::Blue),
                Sender::Assistant => ("助手回复", Color::Green),
                Sender::System => ("系统", Color::Gray),
            };

            let mut lines = vec![Line::from(Span::styled(
                format!("┌─ {} ", title),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))];

            for line in msg.text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(color)),
                    Span::raw(line.to_string()),
                ]));
            }

            if msg.text.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(color)),
                    Span::raw(""),
                ]));
            }

            lines.push(Line::from(Span::styled(
                "└────────────────────────────────",
                Style::default().fg(color),
            )));
            lines.push(Line::from(""));
            lines
        })
        .collect();

    let total_lines = message_text.len();
    let visible_lines = inner.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_lines);
    let scroll = app.scroll.min(max_scroll);

    let message_paragraph = Paragraph::new(Text::from(message_text))
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(message_paragraph, inner);

    let mut scrollbar_state = ScrollbarState::new(total_lines.max(1)).position(scroll);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        full_area,
        &mut scrollbar_state,
    );
}

fn draw_input_bar(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(if app.waiting {
            Color::DarkGray
        } else {
            app.mode.color()
        }));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(18)])
        .split(inner);

    let prompt = Span::styled(
        if app.waiting { "⏳ " } else { "> " },
        Style::default().fg(if app.waiting {
            Color::DarkGray
        } else {
            app.mode.color()
        }),
    );
    let input_line = Line::from(vec![prompt, Span::raw(app.input.as_str())]);
    let input_para = Paragraph::new(Text::from(input_line));
    frame.render_widget(input_para, chunks[0]);

    let status = Line::from(vec![
        Span::styled("● ", Style::default().fg(app.mode.color())),
        Span::styled(
            app.mode.label().to_lowercase(),
            Style::default().fg(Color::White),
        ),
    ]);
    let status_para = Paragraph::new(Text::from(status)).alignment(Alignment::Right);
    frame.render_widget(status_para, chunks[1]);
}
