use anyhow::Result;
use claude_rs_core::session;
use claude_rs_core::{Agent, PermissionMode};
use crossterm::cursor::{self, MoveTo};
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::execute;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::error;

pub const MAX_COMPLETION_ROWS: usize = 6;

pub struct CommandItem {
    pub name: &'static str,
    pub desc: &'static str,
}

pub const COMMANDS: &[CommandItem] = &[
    CommandItem { name: "/quit", desc: "退出程序" },
    CommandItem { name: "/clear", desc: "清空会话" },
    CommandItem { name: "/status", desc: "查看状态" },
    CommandItem { name: "/compact", desc: "压缩会话" },
    CommandItem { name: "/save", desc: "保存会话" },
    CommandItem { name: "/history", desc: "历史会话" },
    CommandItem { name: "/permissions", desc: "切换权限模式" },
    CommandItem { name: "/model", desc: "切换模型" },
    CommandItem { name: "/fast", desc: "快速模式开关" },
    CommandItem { name: "/resume", desc: "恢复会话" },
];

pub struct CompletionState {
    pub active: bool,
    pub filtered: Vec<&'static CommandItem>,
    pub selected: usize,
}

impl CompletionState {
    pub fn new() -> Self {
        Self {
            active: false,
            filtered: Vec::new(),
            selected: 0,
        }
    }

    pub fn refresh(&mut self, input: &str) {
        if input.is_empty() || !input.starts_with('/') {
            self.active = false;
            self.filtered.clear();
            self.selected = 0;
            return;
        }
        let query = input.to_lowercase();
        let query_body = query.trim_start_matches('/');
        let mut scored: Vec<_> = COMMANDS
            .iter()
            .filter(|c| {
                let name = c.name.to_lowercase();
                name.contains(&query) || name.contains(query_body)
            })
            .collect();
        scored.sort_by_key(|c| {
            if c.name.to_lowercase().starts_with(&query) {
                0u8
            } else {
                1u8
            }
        });
        self.filtered = scored;
        self.selected = 0;
        self.active = !self.filtered.is_empty();
    }

    pub fn next(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.filtered.len().min(MAX_COMPLETION_ROWS);
    }

    pub fn prev(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len().min(MAX_COMPLETION_ROWS);
        self.selected = if self.selected == 0 { max - 1 } else { self.selected - 1 };
    }

    pub fn select_by_mouse(&mut self, row: u16, list_start_row: u16) {
        if !self.active || self.filtered.is_empty() {
            return;
        }
        let idx = (row.saturating_sub(list_start_row)) as usize;
        let max = self.filtered.len().min(MAX_COMPLETION_ROWS);
        if idx < max {
            self.selected = idx;
        }
    }

    pub fn accept(&mut self) -> Option<&'static str> {
        if self.active && self.selected < self.filtered.len() {
            let name = self.filtered[self.selected].name;
            self.active = false;
            self.filtered.clear();
            self.selected = 0;
            Some(name)
        } else {
            None
        }
    }

    pub fn dismiss(&mut self) {
        self.active = false;
        self.filtered.clear();
        self.selected = 0;
    }
}

enum InlineEvent {
    Key(event::KeyEvent),
    Mouse(event::MouseEvent),
    Output(String),
    Resize,
    Interrupt,
}

pub struct InlineReplState {
    pub input: String,
    pub queue: Vec<String>,
    pub running: bool,
    pub output_pos: (u16, u16),
    pub completion: CompletionState,
}

impl InlineReplState {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            queue: Vec::new(),
            running: false,
            output_pos: (0, 0),
            completion: CompletionState::new(),
        }
    }

    /// 计算整个底部 UI 区域（补全列表 + 排队预览 + 横线 + 输入框）的起始行。
    pub fn ui_start_row(&self) -> u16 {
        let (_, h) = terminal::size().unwrap_or((80, 24));
        let line_row = h.saturating_sub(2);
        let completion_height = if self.completion.active {
            self.completion.filtered.len().min(MAX_COMPLETION_ROWS) as u16
        } else {
            0
        };
        // 排队预览始终只占用单行，避免输出滚动时多行残留
        let queue_height = if self.running && !self.queue.is_empty() {
            1u16
        } else {
            0
        };
        line_row.saturating_sub(completion_height + queue_height)
    }

    /// 绘制整个底部 UI：补全列表 + 排队预览 + 分隔横线 + 输入框。
    pub fn draw_ui(&self) -> Result<()> {
        let mut stdout = io::stdout();
        let (w, h) = terminal::size()?;
        let line_row = h.saturating_sub(2);
        let input_row = h.saturating_sub(1);
        let width = w as usize;

        let completion_height = if self.completion.active {
            self.completion.filtered.len().min(MAX_COMPLETION_ROWS) as u16
        } else {
            0
        };
        let queue_height = if self.running && !self.queue.is_empty() {
            self.queue.len().min(MAX_COMPLETION_ROWS) as u16
        } else {
            0
        };
        let list_start = line_row.saturating_sub(completion_height + queue_height);

        // 清除底部整个区域
        for row in list_start..=input_row {
            execute!(stdout, MoveTo(0, row))?;
            execute!(stdout, Clear(ClearType::CurrentLine))?;
        }

        // 绘制补全列表
        if self.completion.active {
            for (i, cmd) in self.completion.filtered.iter().take(MAX_COMPLETION_ROWS).enumerate() {
                let row = list_start + i as u16;
                let is_selected = i == self.completion.selected;

                let indicator = if is_selected { "> " } else { "  " };
                let indicator_color = if is_selected { Color::Cyan } else { Color::DarkGrey };

                let query = self.input.to_lowercase();
                let name_lower = cmd.name.to_lowercase();
                let match_len = if let Some(pos) = name_lower.find(&query) {
                    pos + query.len()
                } else {
                    0
                };
                let matched = &cmd.name[..match_len.min(cmd.name.len())];
                let unmatched = &cmd.name[match_len.min(cmd.name.len())..];

                execute!(
                    stdout,
                    MoveTo(0, row),
                    SetForegroundColor(indicator_color),
                    Print(indicator),
                    ResetColor,
                    SetForegroundColor(Color::White),
                    Print(matched),
                    ResetColor,
                    SetForegroundColor(Color::DarkGrey),
                    Print(unmatched),
                    ResetColor,
                )?;

                let name_vis = visible_width(cmd.name);
                let pad = 12usize.saturating_sub(name_vis);
                execute!(
                    stdout,
                    Print(" ".repeat(pad)),
                    SetForegroundColor(Color::Grey),
                    Print(cmd.desc),
                    ResetColor,
                )?;
            }
        }

        // 绘制排队预览（单行，紧邻横线上方）
        if self.running && !self.queue.is_empty() {
            let row = line_row.saturating_sub(1);
            let preview = if self.queue.len() == 1 {
                self.queue[0].clone()
            } else {
                format!("{} (+{})", self.queue[0], self.queue.len() - 1)
            };
            let truncated: String = preview.chars().take(width.saturating_sub(6)).collect();
            execute!(
                stdout,
                MoveTo(0, row),
                SetForegroundColor(Color::DarkGrey),
                Print("▸ [1] "),
                ResetColor,
                SetForegroundColor(Color::White),
                Print(&truncated),
                ResetColor,
            )?;
        }

        // 绘制分隔横线
        let line_chars = "─".repeat(width);
        execute!(
            stdout,
            MoveTo(0, line_row),
            SetForegroundColor(Color::DarkGrey),
            Print(line_chars),
            ResetColor,
        )?;

        // 绘制输入框
        let status_char = if self.running { "●" } else { "○" };
        let status_color = if self.running { Color::Yellow } else { Color::Green };

        execute!(
            stdout,
            MoveTo(0, input_row),
            SetForegroundColor(status_color),
            Print(status_char),
            ResetColor,
            Print("> "),
            Print(&self.input),
            Clear(ClearType::UntilNewLine),
        )?;

        // 光标定位到输入末尾
        let cursor_x = (visible_width(status_char)
            + 2 // "> "
            + visible_width(&self.input))
            .min(width) as u16;
        execute!(stdout, MoveTo(cursor_x, input_row))?;
        stdout.flush()?;
        Ok(())
    }

    pub fn clear_ui(&self) -> Result<()> {
        let mut stdout = io::stdout();
        let (_, h) = terminal::size()?;
        let list_start = self.ui_start_row();
        for row in list_start..h {
            execute!(stdout, MoveTo(0, row))?;
            execute!(stdout, Clear(ClearType::CurrentLine))?;
        }
        stdout.flush()?;
        Ok(())
    }

    pub fn print_output(&mut self, text: &str) -> Result<()> {
        let mut stdout = io::stdout();
        let (w, h) = terminal::size()?;
        let line_row = h.saturating_sub(2);

        // 移动到输出位置
        execute!(stdout, MoveTo(self.output_pos.0, self.output_pos.1))?;

        // 打印文本
        print!("{}", text);
        stdout.flush()?;

        // 获取打印后的光标位置
        self.output_pos = cursor::position().unwrap_or((0, line_row));

        // 如果输出跨行导致光标进入 UI 区（横线及以上），滚动恢复隔离
        if self.output_pos.1 >= line_row {
            let scroll = self.output_pos.1 - line_row + 1;
            execute!(stdout, terminal::ScrollUp(scroll))?;
            self.output_pos.1 = self.output_pos.1.saturating_sub(scroll);
            self.output_pos.0 = self.output_pos.0.min(w.saturating_sub(1));
        }

        // 重绘 UI（draw_ui 会清除 list_start-1 到 h，覆盖 ScrollUp 产生的脏残留）
        self.draw_ui()?;
        Ok(())
    }
}

/// 轻量级增强型 REPL。
pub async fn run(agent: Arc<tokio::sync::Mutex<Agent>>) -> Result<()> {
    let state = Arc::new(Mutex::new(InlineReplState::new()));

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<InlineEvent>();
    let key_tx = event_tx.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            match event::read() {
                Ok(Event::Key(key)) => {
                    if key_tx.send(InlineEvent::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(Event::Mouse(mouse)) => {
                    if key_tx.send(InlineEvent::Mouse(mouse)).is_err() {
                        break;
                    }
                }
                Ok(Event::Resize(_, _)) => {
                    let _ = key_tx.send(InlineEvent::Resize);
                }
                _ => {}
            }
        }
    });

    let ctrlc_tx = event_tx.clone();
    tokio::spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_ok() {
                if ctrlc_tx.send(InlineEvent::Interrupt).is_err() {
                    break;
                }
            }
        }
    });

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, event::EnableMouseCapture)?;

    {
        print_inline_welcome(&agent.lock().await.model());
        let mut s = state.lock().unwrap();
        s.output_pos = cursor::position().unwrap_or((0, 0));
        let _ = s.draw_ui();
    }

    loop {
        // 如果当前空闲且队列中有命令，自动消费并执行，无需等待用户按 Enter。
        let auto_text = {
            let mut s = state.lock().unwrap();
            if !s.running && !s.queue.is_empty() && s.input.is_empty() {
                Some(s.queue.remove(0))
            } else {
                None
            }
        };

        let text = if let Some(line) = auto_text {
            line
        } else {
            // 等待用户提交一条输入
            let mut received: Option<String> = None;
            while received.is_none() {
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    let s = state.lock().unwrap();
                    let _ = s.clear_ui();
                    println!("再见！");
                    let _ = execute!(io::stdout(), event::DisableMouseCapture);
                    terminal::disable_raw_mode()?;
                    return Ok(());
                }
                Some(ev) = event_rx.recv() => {
                    match ev {
                        InlineEvent::Key(key) => {
                            if key.kind != KeyEventKind::Press {
                                continue;
                            }
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let s = state.lock().unwrap();
                                    let _ = s.clear_ui();
                                    println!("再见！");
                                    let _ = execute!(io::stdout(), event::DisableMouseCapture);
                                    terminal::disable_raw_mode()?;
                                    return Ok(());
                                }
                                KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let mut stdout = io::stdout();
                                    let _ = execute!(stdout, Clear(ClearType::All), MoveTo(0, 0));
                                    let mut s = state.lock().unwrap();
                                    s.output_pos = (0, 0);
                                    let _ = s.draw_ui();
                                }
                                KeyCode::Up => {
                                    let mut s = state.lock().unwrap();
                                    if s.completion.active {
                                        s.completion.prev();
                                        let _ = s.draw_ui();
                                    }
                                }
                                KeyCode::Down => {
                                    let mut s = state.lock().unwrap();
                                    if s.completion.active {
                                        s.completion.next();
                                        let _ = s.draw_ui();
                                    }
                                }
                                KeyCode::Tab => {
                                    let mut s = state.lock().unwrap();
                                    if s.completion.active {
                                        if let Some(name) = s.completion.accept() {
                                            s.input = name.to_string();
                                            let _ = s.draw_ui();
                                        }
                                    }
                                }
                                KeyCode::Char(c) => {
                                    let mut s = state.lock().unwrap();
                                    s.input.push(c);
                                    let input_clone = s.input.clone();
                                    s.completion.refresh(&input_clone);
                                    let _ = s.draw_ui();
                                }
                                KeyCode::Backspace => {
                                    let mut s = state.lock().unwrap();
                                    s.input.pop();
                                    if s.input.is_empty() || !s.input.starts_with('/') {
                                        s.completion.dismiss();
                                    } else {
                                        let input_clone = s.input.clone();
                                        s.completion.refresh(&input_clone);
                                    }
                                    let _ = s.draw_ui();
                                }
                                KeyCode::Enter => {
                                    let mut s = state.lock().unwrap();
                                    if s.completion.active {
                                        if let Some(name) = s.completion.accept() {
                                            s.input = name.to_string();
                                            s.completion.dismiss();
                                            let _ = s.draw_ui();
                                            // 如果是纯命令（无参数需要），直接提交执行
                                            received = Some(s.input.clone());
                                            s.input.clear();
                                            continue;
                                        }
                                    }
                                    let line = s.input.trim().to_string();
                                    if !line.is_empty() {
                                        s.input.clear();
                                        s.completion.dismiss();
                                        received = Some(line);
                                    }
                                }
                                KeyCode::Esc => {
                                    let mut s = state.lock().unwrap();
                                    if s.completion.active {
                                        s.completion.dismiss();
                                        let _ = s.draw_ui();
                                    } else {
                                        let _ = s.clear_ui();
                                        println!("再见！");
                                        let _ = execute!(io::stdout(), event::DisableMouseCapture);
                                        terminal::disable_raw_mode()?;
                                        return Ok(());
                                    }
                                }
                                _ => {}
                            }
                        }
                        InlineEvent::Mouse(mouse) => {
                            let mut s = state.lock().unwrap();
                            if !s.completion.active {
                                continue;
                            }
                            let list_start = s.ui_start_row();
                            let line_row = terminal::size().unwrap_or((80, 24)).1.saturating_sub(2);
                            let in_list = mouse.row >= list_start && mouse.row < line_row;

                            match mouse.kind {
                                MouseEventKind::Moved if in_list => {
                                    s.completion.select_by_mouse(mouse.row, list_start);
                                    let _ = s.draw_ui();
                                }
                                MouseEventKind::Down(MouseButton::Left) if in_list => {
                                    s.completion.select_by_mouse(mouse.row, list_start);
                                    if let Some(name) = s.completion.accept() {
                                        s.input = name.to_string();
                                        let _ = s.draw_ui();
                                        received = Some(s.input.clone());
                                        s.input.clear();
                                    }
                                }
                                _ => {}
                            }
                        }
                        InlineEvent::Resize => {
                            let _ = state.lock().unwrap().draw_ui();
                        }
                        InlineEvent::Output(_) => {}
                        InlineEvent::Interrupt => {
                            let s = state.lock().unwrap();
                            let _ = s.clear_ui();
                            println!("再见！");
                            let _ = execute!(io::stdout(), event::DisableMouseCapture);
                            terminal::disable_raw_mode()?;
                            return Ok(());
                        }
                    }
                }
            }
        }

            received.unwrap()
        };

        if is_exit_input(&text) {
            let s = state.lock().unwrap();
            let _ = s.clear_ui();
            println!("再见！");
            let _ = execute!(io::stdout(), event::DisableMouseCapture);
            terminal::disable_raw_mode()?;
            return Ok(());
        }

        // slash 命令：直接处理
        if text.starts_with('/') {
            let mut s = state.lock().unwrap();
            s.running = false;
            let _ = s.print_output(&format!("\n> {}\n", text));
            drop(s);

            let mut a = agent.lock().await;
            if handle_slash_command(text, &mut a, &state).await? {
                let _ = execute!(io::stdout(), event::DisableMouseCapture);
                terminal::disable_raw_mode()?;
                return Ok(());
            }
            continue;
        }

        // 显示用户消息并启动流式请求
        {
            let mut s = state.lock().unwrap();
            s.running = true;
            let _ = s.print_output(&format!("\n> {}\n", text));
            let _ = s.draw_ui();
        }

        // 内层循环：等待流式响应完成，同时接受预输入和排队
        let mut a = agent.lock().await;
        let tx = event_tx.clone();
        let mut run_fut = Box::pin(a.run_turn_stream(text, move |chunk| {
            let _ = tx.send(InlineEvent::Output(chunk.to_string()));
        }));

        loop {
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    let mut s = state.lock().unwrap();
                    s.running = false;
                    s.queue.clear();
                    let _ = s.print_output("\n[已手动中断当前任务]\n");
                    let _ = s.draw_ui();
                    break;
                }
                Some(ev) = event_rx.recv() => {
                    match ev {
                        InlineEvent::Key(key) => {
                            if key.kind != KeyEventKind::Press {
                                continue;
                            }
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let mut s = state.lock().unwrap();
                                    s.running = false;
                                    s.queue.clear();
                                    let _ = s.print_output("\n[已手动中断当前任务]\n");
                                    let _ = s.draw_ui();
                                    break;
                                }
                                KeyCode::Char(c) => {
                                    let mut s = state.lock().unwrap();
                                    s.input.push(c);
                                    let input_clone = s.input.clone();
                                    s.completion.refresh(&input_clone);
                                    let _ = s.draw_ui();
                                }
                                KeyCode::Backspace => {
                                    let mut s = state.lock().unwrap();
                                    s.input.pop();
                                    if s.input.is_empty() || !s.input.starts_with('/') {
                                        s.completion.dismiss();
                                    } else {
                                        let input_clone = s.input.clone();
                                        s.completion.refresh(&input_clone);
                                    }
                                    let _ = s.draw_ui();
                                }
                                KeyCode::Enter => {
                                    let mut s = state.lock().unwrap();
                                    if s.completion.active {
                                        if let Some(name) = s.completion.accept() {
                                            s.input = name.to_string();
                                            s.completion.dismiss();
                                            let _ = s.draw_ui();
                                            // 补全后直接加入队列
                                            let line = s.input.trim().to_string();
                                            if !line.is_empty() {
                                                s.input.clear();
                                                s.queue.push(line);
                                                let _ = s.draw_ui();
                                            }
                                            continue;
                                        }
                                    }
                                    let line = s.input.trim().to_string();
                                    if !line.is_empty() {
                                        s.input.clear();
                                        s.completion.dismiss();
                                        s.queue.push(line);
                                        let _ = s.draw_ui();
                                    }
                                }
                                _ => {}
                            }
                        }
                        InlineEvent::Mouse(mouse) => {
                            let mut s = state.lock().unwrap();
                            if !s.completion.active {
                                continue;
                            }
                            let list_start = s.ui_start_row();
                            let line_row = terminal::size().unwrap_or((80, 24)).1.saturating_sub(2);
                            let in_list = mouse.row >= list_start && mouse.row < line_row;
                            match mouse.kind {
                                MouseEventKind::Moved if in_list => {
                                    s.completion.select_by_mouse(mouse.row, list_start);
                                    let _ = s.draw_ui();
                                }
                                MouseEventKind::Down(MouseButton::Left) if in_list => {
                                    s.completion.select_by_mouse(mouse.row, list_start);
                                    if let Some(name) = s.completion.accept() {
                                        s.input = name.to_string();
                                        let _ = s.draw_ui();
                                        let line = s.input.trim().to_string();
                                        if !line.is_empty() {
                                            s.input.clear();
                                            s.queue.push(line);
                                            let _ = s.draw_ui();
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        InlineEvent::Output(chunk) => {
                            let mut s = state.lock().unwrap();
                            let _ = s.print_output(&chunk);
                        }
                        InlineEvent::Resize => {
                            let _ = state.lock().unwrap().draw_ui();
                        }
                        InlineEvent::Interrupt => {
                            let mut s = state.lock().unwrap();
                            s.running = false;
                            s.queue.clear();
                            let _ = s.print_output("\n[已手动中断当前任务]\n");
                            let _ = s.draw_ui();
                            break;
                        }
                    }
                }
                res = &mut run_fut => {
                    let mut s = state.lock().unwrap();
                    s.running = false;
                    match res {
                        Ok(_) => {}
                        Err(e) => {
                            error!("代理错误：{}", e);
                            let _ = s.print_output(&format!("\n错误：{}\n", e));
                        }
                    }
                    let _ = s.draw_ui();
                    break;
                }
            }
        }
    }
}

async fn handle_slash_command(
    input: String,
    agent: &mut Agent,
    state: &Arc<Mutex<InlineReplState>>,
) -> Result<bool> {
    match input.as_str() {
        "/quit" | "/exit" => {
            let mut s = state.lock().unwrap();
            let _ = s.print_output("再见！\n");
            return Ok(true);
        }
        "/clear" => {
            agent.clear_session();
            let mut s = state.lock().unwrap();
            let _ = s.print_output("会话已清空。\n");
            let _ = s.draw_ui();
        }
        "/status" => {
            let mut s = state.lock().unwrap();
            let _ = s.print_output(&format!("{}\n", agent.status_summary()));
            let _ = s.draw_ui();
        }
        "/compact" => {
            let dropped = agent.compact_now();
            let mut s = state.lock().unwrap();
            let _ = s.print_output(&format!("会话已压缩，移除了 {} 条消息。\n", dropped));
            let _ = s.draw_ui();
        }
        "/save" => {
            let msg = match agent.save_session().await {
                Ok(id) => format!("会话已保存：{}\n", id),
                Err(e) => format!("保存会话失败：{}\n", e),
            };
            let mut s = state.lock().unwrap();
            let _ = s.print_output(&msg);
            let _ = s.draw_ui();
        }
        "/history" => {
            let msg = match session::list_sessions().await {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        "没有保存的会话。\n".to_string()
                    } else {
                        sessions
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
                            .join("\n")
                            + "\n"
                    }
                }
                Err(e) => format!("列出会话失败：{}\n", e),
            };
            let mut s = state.lock().unwrap();
            let _ = s.print_output(&msg);
            let _ = s.draw_ui();
        }
        _ => {}
    }

    if let Some(value) = input.strip_prefix("/permissions ") {
        let value = value.trim();
        match PermissionMode::parse(value) {
            Some(m) => {
                agent.set_permission_mode(m);
                let mut s = state.lock().unwrap();
                let _ = s.print_output(&format!("权限模式已更新为：{}\n", m));
                let _ = s.draw_ui();
            }
            None => {
                let mut s = state.lock().unwrap();
                let _ = s.print_output(&format!(
                    "无效模式：{}，可选 read-only / workspace-write / danger-full-access\n",
                    value
                ));
                let _ = s.draw_ui();
            }
        }
        return Ok(false);
    }

    if let Some(model) = input.strip_prefix("/model ") {
        let model = model.trim();
        if model.is_empty() {
            let mut s = state.lock().unwrap();
            let _ = s.print_output("用法：/model <模型名>\n");
            let _ = s.draw_ui();
        } else {
            agent.set_model(model.to_string());
            let mut s = state.lock().unwrap();
            let _ = s.print_output(&format!("模型已切换为：{}\n", agent.model()));
            let _ = s.draw_ui();
        }
        return Ok(false);
    }

    if let Some(value) = input.strip_prefix("/fast ") {
        let enabled = matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true");
        agent.set_fast_mode(enabled);
        let mut s = state.lock().unwrap();
        let msg = if enabled {
            "Fast mode 已开启（max_tokens=1024, thinking=disabled）。\n"
        } else {
            "Fast mode 已关闭。\n"
        };
        let _ = s.print_output(msg);
        let _ = s.draw_ui();
        return Ok(false);
    }

    if let Some(id) = input.strip_prefix("/resume ") {
        let id = id.trim();
        if id.is_empty() {
            let mut s = state.lock().unwrap();
            let _ = s.print_output("用法：/resume <会话ID>\n");
            let _ = s.draw_ui();
        } else {
            let msg = match agent.load_session(id).await {
                Ok(()) => format!("已恢复会话 {}。\n", id),
                Err(e) => format!("恢复会话失败：{}\n", e),
            };
            let mut s = state.lock().unwrap();
            let _ = s.print_output(&msg);
            let _ = s.draw_ui();
        }
        return Ok(false);
    }

    // 未知命令
    let mut s = state.lock().unwrap();
    let _ = s.print_output(&format!(
        "错误：未知命令 {}\n",
        input.split_whitespace().next().unwrap_or(&input)
    ));
    let _ = s.draw_ui();

    Ok(false)
}

pub fn visible_width(s: &str) -> usize {
    s.chars()
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum()
}

pub fn is_exit_input(input: &str) -> bool {
    input.trim() == "quit"
}

fn print_inline_welcome(model: &str) {
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .display()
        .to_string();
    let session = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );

    const BLUE: &str = "\x1b[94m";
    const DIM: &str = "\x1b[90m";
    const RESET: &str = "\x1b[0m";

    println!("{BLUE}┌──────────────────────────────────────────────────────────────────────────────┐{RESET}");
    println!("{BLUE}│{RESET}  欢迎使用 claude-rs Inline REPL！                                           {BLUE}│{RESET}");
    println!("{BLUE}│{RESET}  可用命令：/status /permissions /model /compact /quit                  {BLUE}│{RESET}");
    println!("{BLUE}│{RESET}                                                                              {BLUE}│{RESET}");
    println!("{BLUE}│{RESET}  {DIM}目录：{RESET} {cwd}");
    println!("{BLUE}│{RESET}  {DIM}会话：{RESET} {session}");
    println!("{BLUE}│{RESET}  {DIM}模型：{RESET} {model}");
    println!("{BLUE}└──────────────────────────────────────────────────────────────────────────────┘{RESET}");
    println!();
}
