use claude_rs_core::Agent;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Color as CrosstermColor, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, execute};
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use claude_rs_llm::{openai::OpenAiProvider, ChatOptions};
use std::path::PathBuf;

const ACCENT: CrosstermColor = CrosstermColor::Rgb { r: 255, g: 107, b: 107 };
const DIM: CrosstermColor = CrosstermColor::Rgb { r: 120, g: 120, b: 120 };

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
        }
    }
}

fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

pub async fn run(config: TuiConfig) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        cursor::Show,
        event::EnableBracketedPaste
    )?;

    let result = app_loop(&mut stdout, config).await;

    crossterm::terminal::disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        cursor::Show,
        event::DisableBracketedPaste
    )?;

    result
}

fn terminal_height() -> u16 {
    crossterm::terminal::size().unwrap_or((80, 24)).1
}

fn clear_prompt(stdout: &mut io::Stdout) -> anyhow::Result<()> {
    let h = terminal_height();
    execute!(stdout, cursor::MoveTo(0, h - 1), Clear(ClearType::CurrentLine))?;
    Ok(())
}

fn draw_prompt(stdout: &mut io::Stdout, app: &App) -> anyhow::Result<()> {
    clear_prompt(stdout)?;
    let h = terminal_height();
    let (w, _) = crossterm::terminal::size()?;
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
        let evt = tokio::select! {
            biased;
            Some(e) = rx.recv() => e,
            else => break,
        };

        match evt {
            AppEvent::Paste(text) => {
                match app.screen {
                    Screen::ApiKeyInput => app.api_key_input.push_str(&text),
                    _ => app.input.push_str(&text),
                }
                draw_prompt(stdout, &app)?;
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
                        KeyCode::Enter if !app.waiting => {
                            let text = app.input.trim().to_string();
                            if text.is_empty() {
                                continue;
                            }
                            app.input.clear();

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
                            draw_prompt(stdout, &app)?;
                        }
                        KeyCode::Backspace => {
                            app.input.pop();
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
}
