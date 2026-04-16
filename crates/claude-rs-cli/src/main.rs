use anyhow::Result;
use clap::Parser;
use claude_rs_cli::inline_repl::{CompletionState, MAX_COMPLETION_ROWS};
use claude_rs_core::session;
use claude_rs_core::{Agent, PermissionMode};
use claude_rs_llm::{ChatOptions, openai::OpenAiProvider};
use crossterm::cursor::{self, MoveTo};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use crossterm::execute;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use serde::Deserialize;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use tracing::{error, info};

use claude_rs_cli::{inline_repl, tui};

#[derive(Parser, Debug)]
#[command(name = "claude-rs", about = "一个快速的 Rust 原生 AI 编程助手")]
struct Args {
    #[arg(long, env = "MOONSHOT_API_KEY")]
    api_key: Option<String>,

    #[arg(long, default_value = "kimi-for-coding")]
    model: String,

    #[arg(long, default_value = "https://api.kimi.com/coding/v1")]
    base_url: String,

    #[arg(
        long,
        default_value = "你是一个有用的编程助手。你可以使用 bash、read、write、edit、grep、glob、todo_write 和 task 工具。请始终逐步思考。"
    )]
    system: String,

    #[arg(long)]
    tui: bool,

    #[arg(long)]
    tui_stream: bool,

    #[arg(long, hide = true)]
    no_tui: bool,

    #[arg(long)]
    no_alt_screen: bool,

    #[arg(long)]
    ask: bool,

    #[arg(long)]
    plan: bool,

    #[arg(long)]
    resume: Option<String>,

    #[arg(long, default_value = "workspace-write")]
    permissions: String,

    #[arg(long)]
    fast: bool,

    #[arg(long)]
    inline: bool,
}

#[derive(Debug, Deserialize, Default)]
struct CliConfig {
    api_key: Option<String>,
}

fn load_cli_config() -> CliConfig {
    let home = match dirs::home_dir() {
        Some(v) => v,
        None => return CliConfig::default(),
    };
    let path = home.join(".claude-rs").join("config.toml");
    let text = match std::fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return CliConfig::default(),
    };
    toml::from_str(&text).unwrap_or_default()
}

fn normalize_api_key(value: Option<String>) -> Option<String> {
    value.and_then(|k| {
        let key = k.trim().to_string();
        if key.is_empty() { None } else { Some(key) }
    })
}

fn apply_speed_profile(options: &mut ChatOptions, fast: bool) {
    if fast {
        // Lower output length for faster completion time.
        options.max_tokens = Some(1024);
        // Kimi thinking models support disabling thinking for lower latency.
        options.extra.insert(
            "thinking".to_string(),
            serde_json::json!({
                "type": "disabled"
            }),
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let cli_cfg = load_cli_config();
    let test_mode = std::env::var("CLAUDE_RS_TEST_MODE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let resolved_api_key = normalize_api_key(args.api_key.clone())
        .or_else(|| normalize_api_key(cli_cfg.api_key.clone()));
    let permission_mode = PermissionMode::parse(&args.permissions).ok_or_else(|| {
        anyhow::anyhow!(
            "无效 permissions 模式：{}，可选 read-only / workspace-write / danger-full-access",
            args.permissions
        )
    })?;
    info!("启动 claude-rs，模型：{}", args.model);

    let use_tui = args.tui && !args.no_tui;

    if use_tui {
        let mode = if args.plan {
            tui::Mode::Plan
        } else if args.ask {
            tui::Mode::Ask
        } else {
            tui::Mode::Code
        };
        return tui::run(tui::TuiConfig {
            api_key: resolved_api_key.clone(),
            model: args.model,
            base_url: args.base_url,
            system: args.system,
            mode,
            resume: args.resume,
        })
        .await;
    }

    // Non-TUI: prompt for API key immediately if missing
    let api_key = match resolved_api_key {
        Some(key) => key,
        None => {
            print!("请输入 Kimi (Moonshot) API 密钥：");
            io::stdout().flush()?;
            let mut key = String::new();
            io::stdin().read_line(&mut key)?;
            let key = key.trim().to_string();
            if key.is_empty() {
                anyhow::bail!("需要 API 密钥才能继续。");
            }
            key
        }
    };

    let provider: Arc<dyn claude_rs_llm::LlmProvider> =
        Arc::new(OpenAiProvider::new(api_key).with_base_url(args.base_url));

    let mut options = ChatOptions::new(args.model.clone());
    apply_speed_profile(&mut options, args.fast);
    let mut agent = Agent::new(provider, options).with_task();
    agent.set_system_prompt(args.system);
    agent.set_permission_mode(permission_mode);

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Err(e) = agent.load_agents_md(&cwd).await {
        tracing::warn!("加载 AGENTS.md 失败：{}", e);
    }

    if let Some(session_id) = args.resume {
        match agent.load_session(&session_id).await {
            Ok(()) => println!("已恢复会话 {}。", session_id),
            Err(e) => eprintln!("恢复会话失败：{}", e),
        }
    }

    // 默认交互模式：轻量级增强 REPL（有实时补全、底部输入框）
    if !test_mode {
        let agent = Arc::new(tokio::sync::Mutex::new(agent));
        return inline_repl::run(agent).await;
    }

    // 以下仅为测试/管道 stdin 保留的兼容回退（line-buffered REPL）
    print_cli_welcome();
    if args.fast {
        println!("Fast mode: ON（低延迟配置已启用）\n");
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || loop {
        match read_input_line_with_fallback(test_mode) {
            Ok(Some(v)) => {
                if tx.send(v).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    });

    let mut queued: VecDeque<String> = VecDeque::new();

    loop {
        let next = if let Some(v) = queued.pop_front() {
            v
        } else {
            print!("> ");
            io::stdout().flush()?;

            match rx.recv().await {
                Some(v) => v,
                None => {
                    println!("\n输入流已关闭，结束会话。");
                    break;
                }
            }
        };

        let input = next.trim().to_string();
        if input.is_empty() {
            continue;
        }

        let is_command = input.starts_with('/');
        if !is_command {
            print_user_bar(&input);
        }

        match input.as_str() {
            cmd if is_exit_command(cmd) => {
                println!("再见！");
                break;
            }
            "/clear" => {
                agent.clear_session();
                println!("会话已清空。");
                continue;
            }
            "/status" => {
                println!("{}", agent.status_summary());
                continue;
            }
            "/compact" => {
                let dropped = agent.compact_now();
                println!("会话已压缩，移除了 {} 条消息。", dropped);
                continue;
            }
            "/save" => {
                match agent.save_session().await {
                    Ok(id) => println!("会话已保存：{}", id),
                    Err(e) => eprintln!("保存会话失败：{}", e),
                }
                continue;
            }
            "/history" => {
                match session::list_sessions().await {
                    Ok(sessions) => {
                        if sessions.is_empty() {
                            println!("没有保存的会话。");
                        } else {
                            for s in sessions {
                                println!("{} - {} messages - {}", s.id, s.messages.len(), s.updated_at);
                            }
                        }
                    }
                    Err(e) => eprintln!("列出会话失败：{}", e),
                }
                continue;
            }
            _ => {}
        }

        if let Some(value) = input.strip_prefix("/permissions ") {
            let value = value.trim();
            match PermissionMode::parse(value) {
                Some(m) => {
                    agent.set_permission_mode(m);
                    println!("权限模式已更新为：{}", m);
                }
                None => eprintln!("无效模式：{}，可选 read-only / workspace-write / danger-full-access", value),
            }
            continue;
        }
        if let Some(model) = input.strip_prefix("/model ") {
            let model = model.trim();
            if model.is_empty() {
                eprintln!("用法：/model <模型名>");
            } else {
                agent.set_model(model.to_string());
                println!("模型已切换为：{}", agent.model());
            }
            continue;
        }
        if let Some(value) = input.strip_prefix("/fast ") {
            let enabled = matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true");
            agent.set_fast_mode(enabled);
            if enabled {
                println!("Fast mode 已开启（max_tokens=1024, thinking=disabled）。");
            } else {
                println!("Fast mode 已关闭。");
            }
            continue;
        }
        if let Some(id) = input.strip_prefix("/resume ") {
            let id = id.trim();
            if id.is_empty() {
                eprintln!("用法：/resume <会话ID>");
            } else {
                match agent.load_session(id).await {
                    Ok(()) => println!("已恢复会话 {}。", id),
                    Err(e) => eprintln!("恢复会话失败：{}", e),
                }
            }
            continue;
        }

        if test_mode {
            if input.starts_with('/') {
                println!("错误：未知命令（test mode）");
            } else {
                println!("\n• TEST_ECHO: {}", input);
            }
            println!();
            continue;
        }

        print!("\n");
        io::stdout().flush()?;
        let mut run_fut = Box::pin(agent.run_turn_stream(input.clone(), |chunk| {
            print!("{}", chunk);
            let _ = io::stdout().flush();
        }));

        loop {
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    queued.clear();
                    println!("\n\n已手动中断当前任务，已清空排队命令。");
                    break;
                }
                Some(line) = rx.recv() => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    queued.push_back(line.clone());
                    print_queued_preview(&line, queued.len());
                }
                res = &mut run_fut => {
                    match res {
                        Ok(_) => {
                            println!();
                        }
                        Err(e) => {
                            error!("代理错误：{}", e);
                            eprintln!("\n错误：{}", e);
                        }
                    }
                    break;
                }
            }
        }
        println!();
    }

    Ok(())
}

fn read_input_line_with_fallback(test_mode: bool) -> anyhow::Result<Option<String>> {
    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(0) => Ok(None),
        Ok(_) => {
            let line = input.trim_end_matches(['\r', '\n']).to_string();
            if !line.starts_with('/') || test_mode {
                return Ok(Some(line));
            }
            pick_command_for_standard_mode(&line)
        }
        Err(e) => {
            #[cfg(target_os = "windows")]
            {
                // In some Windows sessions stdin can be unavailable/pipe-closed.
                // Always fallback to key-event reading for robust interactive input.
                let _ = e;
                let line = read_line_from_key_events()?;
                if !line.starts_with('/') || test_mode {
                    return Ok(Some(line));
                }
                return pick_command_for_standard_mode(&line);
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(e.into())
            }
        }
    }
}

fn read_line_from_key_events() -> anyhow::Result<String> {
    terminal::enable_raw_mode()?;
    let mut buffer = String::new();

    loop {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Enter => {
                    println!();
                    terminal::disable_raw_mode()?;
                    return Ok(buffer);
                }
                KeyCode::Backspace => {
                    if buffer.pop().is_some() {
                        print!("\u{8} \u{8}");
                        io::stdout().flush()?;
                    }
                }
                KeyCode::Char(c) => {
                    buffer.push(c);
                    print!("{c}");
                    io::stdout().flush()?;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn pick_command_for_standard_mode(initial: &str) -> anyhow::Result<Option<String>> {
    if !initial.starts_with('/') {
        return Ok(Some(initial.to_string()));
    }

    let mut query = initial.to_string();
    let mut completion = CompletionState::new();
    completion.refresh(&query);

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, event::EnableMouseCapture)?;
    let (_, base_row) = cursor::position().unwrap_or((0, 0));

    loop {
        draw_command_picker(&mut stdout, base_row, &query, &completion)?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => {
                    execute!(
                        stdout,
                        MoveTo(0, base_row),
                        Clear(ClearType::FromCursorDown),
                        event::DisableMouseCapture
                    )?;
                    terminal::disable_raw_mode()?;
                    println!();
                    return Ok(Some(String::new()));
                }
                KeyCode::Enter => {
                    let selected = if completion.active {
                        completion
                            .accept()
                            .map(str::to_string)
                            .unwrap_or_else(|| query.trim().to_string())
                    } else {
                        query.trim().to_string()
                    };
                    execute!(
                        stdout,
                        MoveTo(0, base_row),
                        Clear(ClearType::FromCursorDown),
                        event::DisableMouseCapture
                    )?;
                    terminal::disable_raw_mode()?;
                    println!();
                    return Ok(Some(selected));
                }
                KeyCode::Up => {
                    completion.prev();
                }
                KeyCode::Down => {
                    completion.next();
                }
                KeyCode::Backspace => {
                    query.pop();
                    completion.refresh(&query);
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    completion.refresh(&query);
                }
                _ => {}
            },
            Event::Mouse(mouse) => {
                if !completion.active {
                    continue;
                }
                let list_start = base_row.saturating_add(2);
                let list_len = completion.filtered.len().min(MAX_COMPLETION_ROWS) as u16;
                let in_list = mouse.row >= list_start && mouse.row < list_start + list_len;
                match mouse.kind {
                    MouseEventKind::Moved if in_list => {
                        completion.select_by_mouse(mouse.row, list_start);
                    }
                    MouseEventKind::Down(MouseButton::Left) if in_list => {
                        completion.select_by_mouse(mouse.row, list_start);
                        let selected = completion
                            .accept()
                            .map(str::to_string)
                            .unwrap_or_else(|| query.trim().to_string());
                        execute!(
                            stdout,
                            MoveTo(0, base_row),
                            Clear(ClearType::FromCursorDown),
                            event::DisableMouseCapture
                        )?;
                        terminal::disable_raw_mode()?;
                        println!();
                        return Ok(Some(selected));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn draw_command_picker(
    stdout: &mut io::Stdout,
    base_row: u16,
    query: &str,
    completion: &CompletionState,
) -> anyhow::Result<()> {
    execute!(stdout, MoveTo(0, base_row), Clear(ClearType::FromCursorDown))?;
    execute!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print("命令输入（↑↓/鼠标/Enter，Esc取消）\n"),
        ResetColor,
        Print(format!("> {}\n", query))
    )?;

    if completion.active {
        for (i, cmd) in completion
            .filtered
            .iter()
            .take(MAX_COMPLETION_ROWS)
            .enumerate()
        {
            let selected = i == completion.selected;
            execute!(
                stdout,
                SetForegroundColor(if selected { Color::Cyan } else { Color::DarkGrey }),
                Print(if selected { "▶ " } else { "  " }),
                ResetColor,
                SetForegroundColor(Color::White),
                Print(cmd.name),
                ResetColor,
                Print(" "),
                SetForegroundColor(Color::Grey),
                Print(cmd.desc),
                ResetColor,
                Print("\n")
            )?;
        }
    } else {
        execute!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print("无匹配命令，Enter 将按原输入执行。\n"),
            ResetColor
        )?;
    }
    stdout.flush()?;
    Ok(())
}

fn is_exit_command(input: &str) -> bool {
    matches!(input.trim(), "/quit" | "/exit" | "quit")
}

fn print_cli_welcome() {
    const ORANGE: &str = "\x1b[38;5;209m";
    const DIM: &str = "\x1b[90m";
    const WHITE: &str = "\x1b[97m";
    const RESET: &str = "\x1b[0m";

    let w = render_width();
    let inner = w.saturating_sub(2);
    let left_w = inner * 36 / 100;
    let right_w = inner.saturating_sub(left_w + 1);

    println!("{ORANGE}─ claude-rs v{} ─{RESET}", env!("CARGO_PKG_VERSION"));
    println!("{ORANGE}┌{}┐{RESET}", "─".repeat(inner));

    let left = vec![
        format!("{WHITE}欢迎回来！{RESET}"),
        String::new(),
        "    _ _".to_string(),
        "  _(o o)_".to_string(),
        " /  \\_/  \\".to_string(),
        " \\__/ \\__/".to_string(),
        String::new(),
        format!(
            "{DIM}{} · {}{RESET}",
            whoami::username(),
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .display()
        ),
    ];
    let right = vec![
        format!("{ORANGE}快速开始{RESET}"),
        "运行 /init 创建 AGENTS.md 项目指令文件。".to_string(),
        "使用 /status 查看模型和权限模式。".to_string(),
        "使用 /permissions <mode> 切换安全级别。".to_string(),
        "按需使用 /compact、/save、/history。".to_string(),
        String::new(),
        format!("{ORANGE}最近活动{RESET}"),
        format!("{DIM}暂无最近活动{RESET}"),
    ];

    let rows = left.len().max(right.len()).max(9);
    for i in 0..rows {
        let l = left.get(i).cloned().unwrap_or_default();
        let r = right.get(i).cloned().unwrap_or_default();
        println!(
            "{ORANGE}│{RESET}{}{ORANGE}│{RESET}{}{ORANGE}│{RESET}",
            pad_ansi(&l, left_w),
            pad_ansi(&r, right_w)
        );
    }

    println!("{ORANGE}└{}┘{RESET}", "─".repeat(inner));
    println!();
}

fn render_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| (w as usize).clamp(80, 160))
        .unwrap_or(120)
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                for ch in chars.by_ref() {
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn pad_ansi(s: &str, width: usize) -> String {
    let visible = strip_ansi(s).chars().count();
    if visible >= width {
        s.chars().take(width).collect()
    } else {
        format!("{s}{}", " ".repeat(width - visible))
    }
}

fn print_user_bar(input: &str) {
    let width = render_width().saturating_sub(2).max(24);
    let inner = width.saturating_sub(2);
    let text = input.trim();
    let shown: String = text.chars().take(inner).collect();
    let visible = shown.chars().count();
    let padded = if visible < inner {
        format!("{shown}{}", " ".repeat(inner - visible))
    } else {
        shown
    };

    const BORDER: &str = "\x1b[38;5;209m";
    const RESET: &str = "\x1b[0m";
    println!("{BORDER}│{RESET}{padded}{BORDER}│{RESET}");
}

fn print_queued_preview(input: &str, queued_len: usize) {
    let width = render_width().saturating_sub(2).max(24);
    let inner = width.saturating_sub(2);
    let text = format!("排队中({queued_len}) {input}");
    let shown: String = text.chars().take(inner).collect();
    let visible = shown.chars().count();
    let padded = if visible < inner {
        format!("{shown}{}", " ".repeat(inner - visible))
    } else {
        shown
    };

    const BORDER: &str = "\x1b[38;5;209m";
    const RESET: &str = "\x1b[0m";
    println!("{BORDER}│{RESET}{padded}{BORDER}│{RESET}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_api_key_normal_trimmed_value() {
        let value = normalize_api_key(Some("  abc  ".to_string()));
        assert_eq!(value, Some("abc".to_string()));
    }

    #[test]
    fn test_normalize_api_key_boundary_empty_value() {
        let value = normalize_api_key(Some("   ".to_string()));
        assert_eq!(value, None);
    }

    #[test]
    fn test_apply_speed_profile_normal_sets_fast_fields() {
        let mut options = ChatOptions::new("model");
        apply_speed_profile(&mut options, true);
        assert_eq!(options.max_tokens, Some(1024));
        assert!(options.extra.contains_key("thinking"));
    }

    #[test]
    fn test_apply_speed_profile_boundary_disabled_keeps_defaults() {
        let mut options = ChatOptions::new("model");
        apply_speed_profile(&mut options, false);
        assert_eq!(options.max_tokens, None);
        assert!(!options.extra.contains_key("thinking"));
    }

    #[test]
    fn test_strip_ansi_normal_removes_escape_sequences() {
        let s = "\u{1b}[31mhello\u{1b}[0m";
        let out = strip_ansi(s);
        assert_eq!(out, "hello");
    }

    #[test]
    fn test_pad_ansi_boundary_truncates_to_width() {
        let out = pad_ansi("abcdef", 3);
        assert_eq!(out, "abc");
    }

    #[test]
    fn test_args_parse_error_invalid_permissions_rejected_later() {
        let parsed = Args::try_parse_from(["claude-rs", "--permissions", "invalid"]);
        assert!(parsed.is_ok());
        let mode = PermissionMode::parse("invalid");
        assert!(mode.is_none());
    }

    #[test]
    fn test_print_queued_preview_boundary_empty_text_no_panic() {
        print_queued_preview("", 0);
    }

    #[test]
    fn test_is_exit_command_normal_quit_word() {
        assert!(is_exit_command("quit"));
    }

    #[test]
    fn test_is_exit_command_boundary_trimmed_quit_word() {
        assert!(is_exit_command("  quit  "));
    }

    #[test]
    fn test_is_exit_command_error_case_uppercase_not_allowed() {
        assert!(!is_exit_command("QUIT"));
    }
}
