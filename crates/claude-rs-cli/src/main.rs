use anyhow::Result;
use clap::Parser;
use claude_rs_core::session;
use claude_rs_core::{Agent, PermissionMode};
use claude_rs_llm::{ChatOptions, openai::OpenAiProvider};
use serde::Deserialize;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

mod tui;

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
    no_tui: bool,

    #[arg(long)]
    ask: bool,

    #[arg(long)]
    plan: bool,

    #[arg(long)]
    resume: Option<String>,

    #[arg(long, default_value = "workspace-write")]
    permissions: String,
}

#[derive(Debug, Deserialize, Default)]
struct CliConfig {
    api_key: Option<String>,
}

fn load_api_key_from_config() -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".claude-rs").join("config.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let cfg: CliConfig = toml::from_str(&text).ok()?;
    cfg.api_key.and_then(|k| {
        let key = k.trim().to_string();
        if key.is_empty() { None } else { Some(key) }
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let resolved_api_key = args.api_key.clone().or_else(load_api_key_from_config);
    let permission_mode = PermissionMode::parse(&args.permissions).ok_or_else(|| {
        anyhow::anyhow!(
            "无效 permissions 模式：{}，可选 read-only / workspace-write / danger-full-access",
            args.permissions
        )
    })?;
    info!("启动 claude-rs，模型：{}", args.model);

    if !args.no_tui {
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
            permission_mode,
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

    let options = ChatOptions::new(args.model);
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

    println!("claude-rs 已就绪。请在下方输入消息，输入 /quit 退出。\n");

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        match input {
            "/quit" | "/exit" => {
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
            cmd if cmd.starts_with("/permissions ") => {
                let value = cmd.strip_prefix("/permissions ").unwrap_or("").trim();
                match PermissionMode::parse(value) {
                    Some(m) => {
                        agent.set_permission_mode(m);
                        println!("权限模式已更新为：{}", m);
                    }
                    None => eprintln!(
                        "无效模式：{}，可选 read-only / workspace-write / danger-full-access",
                        value
                    ),
                }
                continue;
            }
            cmd if cmd.starts_with("/model ") => {
                let model = cmd.strip_prefix("/model ").unwrap_or("").trim();
                if model.is_empty() {
                    eprintln!("用法：/model <模型名>");
                } else {
                    agent.set_model(model.to_string());
                    println!("模型已切换为：{}", agent.model());
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
                                println!(
                                    "{} - {} messages - {}",
                                    s.id,
                                    s.messages.len(),
                                    s.updated_at
                                );
                            }
                        }
                    }
                    Err(e) => eprintln!("列出会话失败：{}", e),
                }
                continue;
            }
            cmd if cmd.starts_with("/resume ") => {
                let id = cmd.strip_prefix("/resume ").unwrap_or("").trim();
                if id.is_empty() {
                    eprintln!("用法：/resume <会话ID>");
                    continue;
                }
                match agent.load_session(id).await {
                    Ok(()) => println!("已恢复会话 {}。", id),
                    Err(e) => eprintln!("恢复会话失败：{}", e),
                }
                continue;
            }
            _ => {}
        }

        match agent.run_turn(input).await {
            Ok(reply) => {
                println!("\n{}", reply);
            }
            Err(e) => {
                error!("代理错误：{}", e);
                eprintln!("错误：{}", e);
            }
        }
        println!();
    }

    Ok(())
}
