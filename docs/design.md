# claude-rs 对标 Claude Code / Codex 功能差距与设计方案

## 1. 目标与范围

本文基于以下三类基线做差距分析，并给出 `claude-rs` 的实现路线：

1. Learn Claude Code 的 19 章能力路径（s01~s19）。
2. Claude Code 官方文档中的关键能力（slash commands、settings/permissions、hooks、MCP）。
3. Codex 官方文档中的关键能力（slash commands、sandbox/approval、config、multi-agent、MCP）。

本设计聚焦 CLI/TUI 产品能力，不涉及模型效果优化与商业化策略。

## 2. 当前实现盘点（代码现状）

当前仓库已具备的核心能力：

- ReAct 主循环与工具回流：`crates/claude-rs-core/src/lib.rs`
- 工具层：`bash/read/write/edit/grep/glob/todo_write`
- 单层子代理（`task`，不可递归）
- 基础会话存取（JSON 文件）与 `/save`、`/history`、`/resume`
- 基础上下文压缩（阈值触发、固定摘要占位）
- TUI 欢迎页与基本命令输入

明显短板：

- 无权限门控与沙箱（工具直接执行）
- 无 Hook 系统
- 无持久记忆系统（仅会话）
- 无任务运行时（后台任务/调度）
- 无 MCP 与插件控制面
- 无多 Agent 团队与 worktree 隔离
- `plan` 仅为占位行为（未形成真实可执行计划流程）

## 3. 对标能力矩阵（Learn Claude Code s01~s19）

| 阶段 | 能力 | 现状 | 结论 |
|---|---|---|---|
| s01 | Agent 循环 | 已有 | 完成 |
| s02 | 工具使用 | 已有 | 完成 |
| s03 | 待办写入 | 已有（内存 todo） | 基础完成 |
| s04 | 子代理 | 已有（单层） | 基础完成 |
| s05 | 技能系统 | 无 | 未实现 |
| s06 | 上下文压缩 | 有简化版 | 部分实现 |
| s07 | 权限系统 | 无 | 未实现 |
| s08 | Hook 系统 | 无 | 未实现 |
| s09 | 记忆系统 | 无 | 未实现 |
| s10 | 系统提示词流水线 | 有基础拼接（system + AGENTS.md） | 部分实现 |
| s11 | 错误恢复 | 无显式恢复状态机 | 未实现 |
| s12 | 任务系统 | 无持久任务图 | 未实现 |
| s13 | 后台任务 | 无 | 未实现 |
| s14 | 定时调度 | 无 | 未实现 |
| s15 | Agent 团队 | 无 | 未实现 |
| s16 | 团队协议 | 无 | 未实现 |
| s17 | 自主代理 | 无 | 未实现 |
| s18 | Worktree 隔离 | 无 | 未实现 |
| s19 | MCP 与插件 | 无 | 未实现 |

## 4. 与 Claude Code / Codex 的关键差距

## 4.1 P0（必须先补，决定“可安全日用”）

- 权限与沙箱双层控制。
- 真实 `plan` 模式与计划执行切换。
- 统一 slash 命令控制面（至少覆盖 `/permissions`、`/compact`、`/model`、`/status`、`/review`）。
- 上下文压缩从“占位提示”升级为“结构化摘要 + 关键事实保留”。

## 4.2 P1（形成“可扩展工程平台”）

- Hook 生命周期（`SessionStart/PreToolUse/PostToolUse/PostToolUseFailure/Stop/Notification`）。
- MCP 客户端接入与工具注册治理。
- 持久记忆系统（跨会话长期知识，含写入准则）。
- 持久任务运行时（任务图 + 槽位 + 重试 + 失败恢复）。

## 4.3 P2（形成“多 Agent 协作平台”）

- Agent 团队、协议与身份。
- worktree 隔离执行车道。
- 自主代理与定时调度。
- 插件市场/分发与签名信任。

## 5. 目标架构设计

## 5.1 模块拆分

1. `claude-rs-policy`（新 crate）
- 职责：权限规则解析、审批策略、命令风险分类、最终放行判定。
- 输入：工具调用意图（tool name + args + cwd + mode）。
- 输出：`allow | ask | deny` 与理由。

2. `claude-rs-sandbox`（由占位改为真实实现）
- 职责：文件系统与网络能力收敛。
- Windows 优先方案：受限令牌 + Job Object + 可写根白名单 + 可选代理层网络控制。
- Unix 方案：`bubblewrap`/`firejail` 适配层（先抽象接口，后按平台落地）。

3. `claude-rs-runtime`（新 crate）
- 职责：任务图、后台运行槽位、调度触发器、重试策略、状态机。
- 数据模型：`task`、`run`、`trigger`、`lease`、`checkpoint`。

4. `claude-rs-memory`（新 crate）
- 职责：跨会话记忆写入/检索/衰减。
- 策略：仅写入“无法从当前工作区推导”的长期事实。

5. `claude-rs-ext`（新 crate）
- 职责：MCP 客户端、插件加载、外部工具目录。
- 功能：服务注册、健康检查、超时、权限标注、工具命名空间。

6. `claude-rs-cli` 增强
- slash 命令注册中心。
- `/permissions`、`/compact`、`/model`、`/status`、`/review`、`/mcp`。
- `~/.claude-rs/config.toml` + 项目级 `.claude-rs/config.toml` 分层合并。

## 5.2 核心数据流

1. 用户输入进入 `Agent::run_turn`。
2. 模型返回 `tool_use`。
3. `policy` 先判定（allow/ask/deny）。
4. allow 时进入 `sandbox executor` 执行工具。
5. `hooks` 在 pre/post/failure/stop 生命周期触发。
6. 结果写回消息流，必要时写入 `memory` 与 `runtime`。
7. 超长上下文触发结构化压缩，并保留可恢复锚点。

## 6. 里程碑计划

## M1（2~3 周）安全可用基线

- 权限规则 DSL（`allow/ask/deny`）
- `/permissions` 命令
- `read-only / workspace-write / danger-full-access` 三档
- `plan` 真正进入“只规划不执行”路径
- `/status` 展示当前模式、策略、上下文占用

验收：默认模式下，危险命令必须被 ask/deny；无显式授权不得越界写目录。

## M2（3~4 周）可观测与可扩展

- Hook 系统首版（命令型 hook）
- `/compact` 结构化压缩
- `/review` 基于当前工作树输出审查结论
- MCP 客户端首版（stdio server）与 `/mcp`

验收：可通过 hook 阻断不安全工具调用；可挂载至少 1 个 MCP server 并稳定调用。

## M3（4~6 周）任务运行时

- 持久任务图（SQLite）
- 后台运行槽位 + `/ps` + `/stop`
- 定时触发器（cron 子集）
- 失败恢复状态机

验收：进程重启后任务状态可恢复；失败重试符合策略；可追踪 run 历史。

## M4（6+ 周）多 Agent 平台

- Agent 团队与消息协议
- worktree 隔离执行
- 自主任务认领与安全边界
- 插件机制与信任模型

验收：并行 agent 不互相污染工作目录；协议消息可审计；可配置最大并行与深度。

## 7. 风险与应对

- 平台差异风险（Windows/Unix 沙箱能力不一致）。
- 应对：先定义统一能力接口与能力矩阵，按平台降级实现并显式告警。

- 模型兼容性风险（不同 OpenAI-compatible 供应商行为差异）。
- 应对：Provider 适配层增加 capability 探测与回退策略。

- 复杂度膨胀风险（功能线并行推进）。
- 应对：严格按 M1→M4 递进，先安全闭环后扩展。

## 8. 参考来源

- Learn Claude Code（中文首页与版本对比）：
  - https://learn.shareai.run/zh/
  - https://learn.shareai.run/zh/compare/
- Claude Code 文档：
  - Slash commands: https://code.claude.com/docs/en/slash-commands
  - Settings & permissions: https://code.claude.com/docs/en/settings
  - Hooks: https://code.claude.com/docs/en/hooks
  - MCP: https://code.claude.com/docs/en/mcp
- OpenAI Codex 文档：
  - Slash commands: https://developers.openai.com/codex/cli/slash-commands
  - Config reference: https://developers.openai.com/codex/config-reference
  - Sandbox & approvals: https://developers.openai.com/codex/agent-approvals-security
