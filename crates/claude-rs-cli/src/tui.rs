use claude_rs_core::session;
use claude_rs_core::{Agent, PermissionMode};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::style::{Color as CrosstermColor, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, execute};
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use claude_rs_llm::{openai::OpenAiProvider, ChatOptions};
use std::path::PathBuf;

const ACCENT: CrosstermColor = CrosstermColor::Rgb { r: 255, g: 107, b: 107 };

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

    fn color(&self) -> CrosstermColor {
        match self {
            Mode::Ask => CrosstermColor::Cyan,
            Mode::Code => CrosstermColor::Green,
            Mode::Plan => CrosstermColor::Yellow,
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
}

#[derive(Debug, Clone)]
enum Sender {
    User,
    Assistant,
    System,
}

enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
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
    mode: Mode,
    waiting: bool,
    welcome_shown: bool,
    command_suggestions: Vec<&'static str>,
    command_selected: usize,
}

impl App {
    fn new(mode: Mode) -> Self {
        Self {
            screen: Screen::Welcome,
            input: String::new(),
            api_key_input: String::new(),
            pending_message: None,
            mode,
            waiting: false,
            welcome_shown: false,
            command_suggestions: Vec::new(),
            command_selected: 0,
        }
    }

    fn refresh_command_suggestions(&mut self) {
        let query = command_query(&self.input);
        self.command_suggestions = query
            .map(filter_commands)
            .unwrap_or_default()
            .into_iter()
            .take(6)
            .collect();
        if self.command_suggestions.is_empty() {
            self.command_selected = 0;
        } else if self.command_selected >= self.command_suggestions.len() {
            self.command_selected = self.command_suggestions.len() - 1;
        }
    }

    fn selected_command_for_enter(&self) -> Option<String> {
        if self.command_suggestions.is_empty() {
            None
        } else {
            Some(self.command_suggestions[self.command_selected].to_string())
        }
    }
}

fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

fn is_exit_input(input: &str) -> bool {
    input.trim() == "quit"
}

const COMMAND_LIST: [&str; 14] = [
    "/status",
    "/clear",
    "/compact",
    "/save",
    "/history",
    "/quit",
    "/exit",
    "/permissions read-only",
    "/permissions workspace-write",
    "/permissions danger-full-access",
    "/model kimi-for-coding",
    "/fast on",
    "/fast off",
    "/resume <会话ID>",
];

fn command_query(input: &str) -> Option<&str> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }
    Some(trimmed.trim_start_matches('/'))
}

fn fuzzy_match(command: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut chars = query.chars().map(|c| c.to_ascii_lowercase());
    let mut next = chars.next();
    for c in command.chars().map(|c| c.to_ascii_lowercase()) {
        if let Some(n) = next {
            if c == n {
                next = chars.next();
            }
        } else {
            return true;
        }
    }
    next.is_none()
}

fn filter_commands(query: &str) -> Vec<&'static str> {
    let query = query.to_ascii_lowercase();
    let mut items: Vec<&'static str> = COMMAND_LIST
        .iter()
        .copied()
        .filter(|cmd| fuzzy_match(cmd, &query))
        .collect();
    items.sort_by_key(|cmd| cmd.len());
    items
}

fn fuzzy_match_positions(command: &str, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut positions = Vec::new();
    let mut q = query.chars().map(|c| c.to_ascii_lowercase());
    let mut next = q.next();
    for (idx, c) in command.chars().enumerate() {
        if let Some(n) = next {
            if c.to_ascii_lowercase() == n {
                positions.push(idx);
                next = q.next();
                if next.is_none() {
                    break;
                }
            }
        } else {
            break;
        }
    }
    if next.is_none() { positions } else { Vec::new() }
}

pub async fn run(config: TuiConfig) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        cursor::Show,
        event::EnableBracketedPaste,
        event::EnableMouseCapture
    )?;

    let result = app_loop(&mut stdout, config).await;

    crossterm::terminal::disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        cursor::Show,
        event::DisableBracketedPaste,
        event::DisableMouseCapture
    )?;

    result
}

fn terminal_height() -> u16 {
    crossterm::terminal::size().unwrap_or((80, 24)).1
}

fn clear_prompt(stdout: &mut io::Stdout) -> anyhow::Result<()> {
    let h = terminal_height();
    for row in h.saturating_sub(7)..h.saturating_sub(1) {
        execute!(stdout, cursor::MoveTo(0, row), Clear(ClearType::CurrentLine))?;
    }
    execute!(stdout, cursor::MoveTo(0, h - 1), Clear(ClearType::CurrentLine))?;
    Ok(())
}

fn draw_prompt(stdout: &mut io::Stdout, app: &App) -> anyhow::Result<()> {
    clear_prompt(stdout)?;
    let h = terminal_height();
    let (w, _) = crossterm::terminal::size()?;
    if !app.command_suggestions.is_empty() {
        let count = app.command_suggestions.len() as u16;
        let start_row = h.saturating_sub(1 + count);
        let query = command_query(&app.input).unwrap_or("");
        for (i, cmd) in app.command_suggestions.iter().enumerate() {
            let row = start_row + i as u16;
            execute!(stdout, cursor::MoveTo(0, row), Clear(ClearType::CurrentLine))?;
            let selected = i == app.command_selected;
            let base = if selected {
                CrosstermColor::Yellow
            } else {
                CrosstermColor::DarkGrey
            };
            execute!(
                stdout,
                cursor::MoveTo(0, row),
                SetForegroundColor(base),
                Print(if selected { "▶ " } else { "  " }),
            )?;
            let positions = fuzzy_match_positions(cmd, query);
            if positions.is_empty() {
                execute!(stdout, Print(*cmd), ResetColor)?;
            } else {
                for (idx, ch) in cmd.chars().enumerate() {
                    if positions.contains(&idx) {
                        execute!(stdout, SetForegroundColor(CrosstermColor::Cyan), Print(ch))?;
                    } else {
                        execute!(stdout, SetForegroundColor(base), Print(ch))?;
                    }
                }
                execute!(stdout, ResetColor)?;
            }
        }
    }

    let prompt = if app.waiting { "⏳ " } else { "> " };
    let input_text = match app.screen {
        Screen::ApiKeyInput => app.api_key_input.as_str(),
        _ => app.input.as_str(),
    };

    execute!(
        stdout,
        cursor::MoveTo(0, h - 1),
        SetForegroundColor(if app.waiting {
            CrosstermColor::DarkGrey
        } else {
            app.mode.color()
        }),
        Print(prompt),
        ResetColor,
        Print(input_text),
    )?;

    let status = format!("● {}", app.mode.label().to_lowercase());
    let status_width = display_width(&status);
    let status_x = w.saturating_sub(status_width as u16 + 1);
    execute!(
        stdout,
        cursor::MoveTo(status_x, h - 1),
        SetForegroundColor(app.mode.color()),
        Print(&status),
        ResetColor,
    )?;

    let cursor_x = display_width(prompt) + display_width(input_text);
    execute!(stdout, cursor::MoveTo(cursor_x as u16, h - 1))?;
    stdout.flush()?;
    Ok(())
}

fn print_box_top(stdout: &mut io::Stdout, width: u16) -> anyhow::Result<()> {
    execute!(
        stdout,
        SetForegroundColor(ACCENT),
        Print(format!("┌{}┐", "─".repeat(width.saturating_sub(2) as usize))),
        ResetColor,
    )?;
    println!();
    Ok(())
}

fn print_box_bottom(stdout: &mut io::Stdout, width: u16) -> anyhow::Result<()> {
    execute!(
        stdout,
        SetForegroundColor(ACCENT),
        Print(format!("└{}┘", "─".repeat(width.saturating_sub(2) as usize))),
        ResetColor,
    )?;
    println!();
    Ok(())
}

fn print_box_empty(stdout: &mut io::Stdout, width: u16) -> anyhow::Result<()> {
    let inner = " ".repeat(width.saturating_sub(2) as usize);
    execute!(
        stdout,
        SetForegroundColor(ACCENT),
        Print(format!("│{}│", inner)),
        ResetColor,
    )?;
    println!();
    Ok(())
}

fn print_box_line(stdout: &mut io::Stdout, width: u16, content: &str) -> anyhow::Result<()> {
    let total_inner = width.saturating_sub(2) as usize;
    let text_width = display_width(content);
    let pad_left = 4usize;
    let pad_right = total_inner.saturating_sub(pad_left + text_width);
    execute!(
        stdout,
        SetForegroundColor(ACCENT),
        Print("│"),
        ResetColor,
        Print(" ".repeat(pad_left)),
        Print(content),
        Print(" ".repeat(pad_right)),
        SetForegroundColor(ACCENT),
        Print("│"),
        ResetColor,
    )?;
    println!();
    Ok(())
}

fn print_box_center(stdout: &mut io::Stdout, width: u16, content: &str) -> anyhow::Result<()> {
    let total_inner = width.saturating_sub(2) as usize;
    let text_width = display_width(content);
    let pad = total_inner.saturating_sub(text_width) / 2;
    let extra = total_inner.saturating_sub(text_width) % 2;
    execute!(
        stdout,
        SetForegroundColor(ACCENT),
        Print("│"),
        ResetColor,
        Print(" ".repeat(pad)),
        Print(content),
        Print(" ".repeat(pad + extra)),
        SetForegroundColor(ACCENT),
        Print("│"),
        ResetColor,
    )?;
    println!();
    Ok(())
}

fn show_welcome(stdout: &mut io::Stdout, app: &mut App) -> anyhow::Result<()> {
    if app.welcome_shown {
        return Ok(());
    }
    app.welcome_shown = true;

    clear_prompt(stdout)?;
    execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

    let (w, h) = crossterm::terminal::size()?;
    let box_width = if w >= 74 { 70 } else { w.saturating_sub(4) };
    let box_height = 18u16;
    let top_margin = h.saturating_sub(box_height) / 2;

    for _ in 0..top_margin {
        println!();
    }

    print_box_top(stdout, box_width)?;
    print_box_empty(stdout, box_width)?;
    print_box_line(stdout, box_width, "")?;
    print_box_center(stdout, box_width, "    ┌─────┐    ")?;
    print_box_center(stdout, box_width, "    │ ◠ ◠ │    ")?;
    print_box_center(stdout, box_width, "    │  ▽  │    ")?;
    print_box_center(stdout, box_width, "    └─┬─┬─┘    ")?;
    print_box_center(stdout, box_width, "      │ │      ")?;
    print_box_center(stdout, box_width, "    ──┘ └──    ")?;
    print_box_empty(stdout, box_width)?;
    print_box_center(stdout, box_width, "欢迎回来！")?;
    print_box_center(stdout, box_width, &format!(
        "{} · {}",
        whoami::username(),
        std::env::current_dir().unwrap_or_default().display()
    ))?;
    print_box_empty(stdout, box_width)?;
    print_box_line(stdout, box_width, &format!("{}", "─".repeat(box_width.saturating_sub(10) as usize)))?;
    print_box_empty(stdout, box_width)?;
    print_box_line(stdout, box_width, "开始使用")?;
    print_box_empty(stdout, box_width)?;
    print_box_line(stdout, box_width, "运行 /init 创建 AGENTS.md 文件来添加自定义指令。")?;
    print_box_line(stdout, box_width, "按 Tab 键在 ASK / CODE / PLAN 模式之间切换。")?;
    print_box_line(stdout, box_width, "输入 /save 保存会话，/history 查看历史会话。")?;
    print_box_empty(stdout, box_width)?;
    print_box_bottom(stdout, box_width)?;

    draw_prompt(stdout, app)?;
    Ok(())
}

fn show_api_key_prompt(stdout: &mut io::Stdout, app: &App) -> anyhow::Result<()> {
    clear_prompt(stdout)?;
    println!();
    execute!(
        stdout,
        SetForegroundColor(ACCENT),
        Print("需要 API 密钥"),
        ResetColor
    )?;
    println!();
    println!("请输入 Kimi (Moonshot) API 密钥以继续。");
    println!("按 Enter 确认，按 Esc 退出。");
    println!();
    draw_prompt(stdout, app)?;
    Ok(())
}

fn print_message(stdout: &mut io::Stdout, app: &App, sender: Sender, text: &str) -> anyhow::Result<()> {
    clear_prompt(stdout)?;
    let (label, color) = match sender {
        Sender::User => ("你", CrosstermColor::Blue),
        Sender::Assistant => ("助手", CrosstermColor::Green),
        Sender::System => ("系统", CrosstermColor::Grey),
    };
    println!();
    execute!(
        stdout,
        SetForegroundColor(color),
        Print(format!("[{}] ", label)),
        ResetColor
    )?;
    for line in text.lines() {
        println!("{}", line);
    }
    draw_prompt(stdout, app)?;
    Ok(())
}

async fn app_loop(
    stdout: &mut io::Stdout,
    config: TuiConfig,
) -> anyhow::Result<()> {
    let mut app = App::new(config.mode);
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let agent: Arc<Mutex<Option<Agent>>> = Arc::new(Mutex::new(None));

    if let Some(key) = config.api_key {
        let provider: Arc<dyn claude_rs_llm::LlmProvider> =
            Arc::new(OpenAiProvider::new(key).with_base_url(&config.base_url));
        let options = ChatOptions::new(&config.model);
        let mut new_agent = Agent::new(provider, options).with_task();
        new_agent.set_system_prompt(&config.system);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let _ = new_agent.load_agents_md(&cwd).await;
        if let Some(ref session_id) = config.resume {
            let _ = new_agent.load_session(session_id).await;
        }
        *agent.lock().await = Some(new_agent);
    }

    show_welcome(stdout, &mut app)?;
    app.refresh_command_suggestions();

    let event_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            match event::read() {
                Ok(Event::Key(key)) => {
                    if event_tx.send(AppEvent::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(Event::Mouse(mouse)) => {
                    if event_tx.send(AppEvent::Mouse(mouse)).is_err() {
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
        let evt = tokio::select! {
            biased;
            Some(e) = rx.recv() => e,
            else => break,
        };

        match evt {
            AppEvent::Paste(text) => {
                match app.screen {
                    Screen::ApiKeyInput => app.api_key_input.push_str(&text),
                    _ => {
                        app.input.push_str(&text);
                        app.refresh_command_suggestions();
                    }
                }
                draw_prompt(stdout, &app)?;
            }
            AppEvent::Mouse(mouse) => {
                if app.command_suggestions.is_empty() {
                    continue;
                }
                if matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Down(_)) {
                    let h = terminal_height();
                    let count = app.command_suggestions.len() as u16;
                    let start_row = h.saturating_sub(1 + count);
                    if mouse.row >= start_row && mouse.row < start_row + count {
                        app.command_selected = (mouse.row - start_row) as usize;
                        draw_prompt(stdout, &app)?;
                    }
                }
            }
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
                            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                            let _ = new_agent.load_agents_md(&cwd).await;
                            if let Some(ref session_id) = config.resume {
                                let _ = new_agent.load_session(session_id).await;
                            }
                            *agent.lock().await = Some(new_agent);
                            app.screen = Screen::Chat;
                            app.api_key_input.clear();

                            if let Some(text) = app.pending_message.take() {
                                print_message(stdout, &app, Sender::User, &text)?;
                                app.waiting = true;
                                draw_prompt(stdout, &app)?;
                                let response_tx = tx.clone();
                                let mode = app.mode;
                                let agent = agent.clone();
                                tokio::spawn(async move {
                                    let result = if mode == Mode::Plan {
                                        Ok(format!(
                                            "[计划模式] 我会对以下内容进行分析和规划：{}\n\n切换到 CODE 模式以执行。",
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
                            } else {
                                draw_prompt(stdout, &app)?;
                            }
                        }
                        KeyCode::Char(c) => {
                            app.api_key_input.push(c);
                            draw_prompt(stdout, &app)?;
                        }
                        KeyCode::Backspace => {
                            app.api_key_input.pop();
                            draw_prompt(stdout, &app)?;
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
                            draw_prompt(stdout, &app)?;
                        }
                        KeyCode::Up if !app.command_suggestions.is_empty() => {
                            if app.command_selected == 0 {
                                app.command_selected = app.command_suggestions.len() - 1;
                            } else {
                                app.command_selected -= 1;
                            }
                            draw_prompt(stdout, &app)?;
                        }
                        KeyCode::Down if !app.command_suggestions.is_empty() => {
                            app.command_selected =
                                (app.command_selected + 1) % app.command_suggestions.len();
                            draw_prompt(stdout, &app)?;
                        }
                        KeyCode::Enter if !app.waiting => {
                            let mut text = app.input.trim().to_string();
                            if text.is_empty() {
                                continue;
                            }
                            if text.starts_with('/') && !app.command_suggestions.is_empty() {
                                if let Some(selected) = app.selected_command_for_enter() {
                                    text = selected;
                                }
                            }
                            if is_exit_input(&text) || matches!(text.as_str(), "/quit" | "/exit")
                            {
                                println!("再见！");
                                break;
                            }
                            app.input.clear();
                            app.refresh_command_suggestions();

                            if handle_local_command(stdout, &text, &agent, &app).await? {
                                continue;
                            }

                            let has_agent = agent.lock().await.is_some();
                            if !has_agent {
                                app.pending_message = Some(text);
                                app.screen = Screen::ApiKeyInput;
                                show_api_key_prompt(stdout, &app)?;
                                continue;
                            }

                            print_message(stdout, &app, Sender::User, &text)?;
                            app.waiting = true;

                            let response_tx = tx.clone();
                            let mode = app.mode;
                            let agent = agent.clone();
                            tokio::spawn(async move {
                                let result = if mode == Mode::Plan {
                                    Ok(format!(
                                        "[计划模式] 我会对以下内容进行分析和规划：{}\n\n切换到 CODE 模式以执行。",
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
                        KeyCode::Char(c) => {
                            app.input.push(c);
                            app.refresh_command_suggestions();
                            draw_prompt(stdout, &app)?;
                        }
                        KeyCode::Backspace => {
                            app.input.pop();
                            app.refresh_command_suggestions();
                            draw_prompt(stdout, &app)?;
                        }
                        _ => {}
                    },
                }
            }
            AppEvent::AgentResponse(result) => {
                app.waiting = false;
                match result {
                    Ok(text) => print_message(stdout, &app, Sender::Assistant, &text)?,
                    Err(e) => print_message(stdout, &app, Sender::System, &format!("错误：{}", e))?,
                }
            }
        }
    }

    Ok(())
}

async fn handle_local_command(
    stdout: &mut io::Stdout,
    text: &str,
    agent: &Arc<Mutex<Option<Agent>>>,
    app: &App,
) -> anyhow::Result<bool> {
    if text == "/clear" {
        if let Some(a) = agent.lock().await.as_mut() {
            a.clear_session();
        }
        print_message(stdout, app, Sender::System, "会话已清空。")?;
        return Ok(true);
    }
    if text == "/status" {
        let msg = if let Some(a) = agent.lock().await.as_ref() {
            a.status_summary()
        } else {
            "代理尚未初始化。".to_string()
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if text == "/compact" {
        let msg = if let Some(a) = agent.lock().await.as_mut() {
            format!("会话已压缩，移除了 {} 条消息。", a.compact_now())
        } else {
            "代理尚未初始化。".to_string()
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if text == "/save" {
        let msg = if let Some(a) = agent.lock().await.as_mut() {
            match a.save_session().await {
                Ok(id) => format!("会话已保存：{}", id),
                Err(e) => format!("保存会话失败：{}", e),
            }
        } else {
            "代理尚未初始化。".to_string()
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if text == "/history" {
        let msg = match session::list_sessions().await {
            Ok(sessions) if sessions.is_empty() => "没有保存的会话。".to_string(),
            Ok(sessions) => sessions
                .into_iter()
                .map(|s| format!("{} - {} messages - {}", s.id, s.messages.len(), s.updated_at))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("列出会话失败：{}", e),
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if let Some(value) = text.strip_prefix("/permissions ") {
        let value = value.trim();
        let msg = match PermissionMode::parse(value) {
            Some(mode) => {
                if let Some(a) = agent.lock().await.as_mut() {
                    a.set_permission_mode(mode);
                    format!("权限模式已更新为：{}", mode)
                } else {
                    "代理尚未初始化。".to_string()
                }
            }
            None => "无效模式，可选 read-only / workspace-write / danger-full-access".to_string(),
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if let Some(model) = text.strip_prefix("/model ") {
        let model = model.trim();
        let msg = if model.is_empty() {
            "用法：/model <模型名>".to_string()
        } else if let Some(a) = agent.lock().await.as_mut() {
            a.set_model(model.to_string());
            format!("模型已切换为：{}", a.model())
        } else {
            "代理尚未初始化。".to_string()
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if let Some(value) = text.strip_prefix("/fast ") {
        let enabled = matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true");
        let msg = if let Some(a) = agent.lock().await.as_mut() {
            a.set_fast_mode(enabled);
            if enabled {
                "Fast mode 已开启（max_tokens=1024, thinking=disabled）。".to_string()
            } else {
                "Fast mode 已关闭。".to_string()
            }
        } else {
            "代理尚未初始化。".to_string()
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if let Some(id) = text.strip_prefix("/resume ") {
        let id = id.trim();
        let msg = if id.is_empty() || id.contains('<') {
            "用法：/resume <会话ID>".to_string()
        } else if let Some(a) = agent.lock().await.as_mut() {
            match a.load_session(id).await {
                Ok(()) => format!("已恢复会话 {}。", id),
                Err(e) => format!("恢复会话失败：{}", e),
            }
        } else {
            "代理尚未初始化。".to_string()
        };
        print_message(stdout, app, Sender::System, &msg)?;
        return Ok(true);
    }
    if text.starts_with('/') {
        print_message(stdout, app, Sender::System, "未知命令。")?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_label_normal_values() {
        assert_eq!(Mode::Ask.label(), "ASK");
        assert_eq!(Mode::Code.label(), "CODE");
        assert_eq!(Mode::Plan.label(), "PLAN");
    }

    #[test]
    fn test_mode_color_boundary_values() {
        assert_eq!(Mode::Ask.color(), CrosstermColor::Cyan);
        assert_eq!(Mode::Code.color(), CrosstermColor::Green);
        assert_eq!(Mode::Plan.color(), CrosstermColor::Yellow);
    }

    #[test]
    fn test_display_width_normal_ascii_and_cjk() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("中a"), 3);
    }

    #[test]
    fn test_app_new_boundary_defaults() {
        let app = App::new(Mode::Code);
        assert!(matches!(app.screen, Screen::Welcome));
        assert_eq!(app.input, "");
        assert_eq!(app.api_key_input, "");
        assert!(!app.waiting);
    }

    #[test]
    fn test_terminal_height_error_case_not_applicable_returns_positive() {
        let h = terminal_height();
        assert!(h > 0);
    }

    #[test]
    fn test_filter_commands_normal_status_query() {
        let result = filter_commands("sta");
        assert!(result.iter().any(|c| *c == "/status"));
    }

    #[test]
    fn test_filter_commands_boundary_empty_query_returns_all() {
        let result = filter_commands("");
        assert_eq!(result.len(), COMMAND_LIST.len());
    }

    #[test]
    fn test_filter_commands_error_case_no_match_returns_empty() {
        let result = filter_commands("zzzz-no-match");
        assert!(result.is_empty());
    }

    #[test]
    fn test_fuzzy_match_positions_normal_marks_chars() {
        let positions = fuzzy_match_positions("/status", "sts");
        assert_eq!(positions, vec![1, 2, 6]);
    }

    #[test]
    fn test_fuzzy_match_boundary_subsequence() {
        assert!(fuzzy_match("/permissions workspace-write", "pw"));
    }

    #[test]
    fn test_fuzzy_match_error_case_non_subsequence() {
        assert!(!fuzzy_match("/status", "zx"));
    }

    #[test]
    fn test_fuzzy_match_positions_error_case_non_subsequence_empty_positions() {
        let positions = fuzzy_match_positions("/status", "zx");
        assert!(positions.is_empty());
    }

    #[test]
    fn test_is_exit_input_normal_quit_word() {
        assert!(is_exit_input("quit"));
    }

    #[test]
    fn test_is_exit_input_boundary_trimmed_quit_word() {
        assert!(is_exit_input("  quit "));
    }

    #[test]
    fn test_is_exit_input_error_case_uppercase_not_allowed() {
        assert!(!is_exit_input("QUIT"));
    }
}
