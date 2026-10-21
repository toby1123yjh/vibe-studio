# Agent Runtime 底座重构

> 状态：规划已批准 · 等待实施激活  
> 关联 Trellis 任务：`.trellis/tasks/08-05-agent-runtime-refactor/`

## 目标

为 Vibe Kanban 建立一层异构 Agent Runtime 底座。真实 Agent CLI、SDK、ACP、App Server 和 HTTP sidecar 保留各自的原生能力，通过显式的 adapter 与 transport 接入；Vibe Kanban 负责运行生命周期、会话关联、取消、审批、事件记录、审计和统一业务输出。

本设计不要求各家 Agent 使用同一种协议，也不以“抹平所有差异”为目标。目标是把差异隔离在可版本化、可回放、可测试的边界内。

## 已确认的产品边界

- Agent 继续直接使用原生文件、命令和 MCP 工具，操作 Vibe Kanban 按协作模式准备的真实 workspace/worktree。
- 首期不把所有工具执行收回到 QM 式 Tool Gateway；VK 负责观察、记录和展示。
- 采用“先适配、再统一”的数据流。provider 原生输入/输出与平台统一事件分别管理、分别审计。
- Native Audit Store 独立存储，首期按原文保存，不脱敏，不增加周期清理机制。
- canonical 层保留公共事件主干，也保留 provider-specific extension。
- 原生输出采用 `NativeFrame → ProviderEvent → Canonical AgentEvent` 三段转换：先解码 Provider 语义，再映射平台语义。
- Native Audit 采用 write-ahead/fail-closed；canonical event/state 写入失败允许进入 `projection_degraded` 并从 Native Audit 重建。
- Canonical Event Store 使用 append-only event log；RunState 是 Reducer 生成的可重建快照，不是不可追溯的唯一事实。
- 目标运行关系采用 `Session → Managed AgentRun → Turn → RunAttempt`：一个 AgentRun 表示一个完整 Agent 任务并对应一个发起 Turn；崩溃、重试或恢复只增加 RunAttempt，后续新要求创建可复用原 Session 的新 AgentRun。
- 为了区分工具循环、模型调用、审批和协议阶段，在 `RunAttempt` 下保留细粒度 `ProviderStep/ModelCall` 记录；可观测性不足时明确标记未知，不伪造精确语义。
- AgentRun Runtime Core、Process Supervisor 和 Transport 分别拥有平台 identity/state、真实进程/句柄生命周期和协议 frame 通道；服务重启由 Supervisor attach/query/reconcile，不能默认清理仍存活的 Agent。
- canonical request、event 和 RunState 分别版本化；Rust 类型是契约唯一源码，TypeScript 通过 `ts-rs` 生成。
- 项目尚未正式发布，V1 直接建立 Session、AgentRun、Turn、RunAttempt、provider session reference、canonical event/state 和 orchestration 的新事实源；不保留旧运行数据、旧表回填、双写或兼容投影。
- Native Audit 固定使用 `<asset_dir>/runtime/native-audit/v1/.../manifest.json + frames.jsonl`，SQLite 只保存定位、版本和完整性索引；导出与 fixture 共用版本化 Audit Bundle。
- 产品按本地单用户可信环境设计，不引入用户管理、管理员角色、RBAC 或多租户授权。
- V1 不提供 native/canonical 对照审计 UI；普通 UI 和 WebSocket 只消费 canonical 数据。
- V1 不建设知识库/RAG/长期记忆子系统，不包含文档摄取、切片、Embedding、向量索引和检索编排；Agent 仍可通过自身 MCP 或原生能力访问已有外部知识源。
- V1 同时建设多 Agent/task orchestration；平台创建的跨 Runtime 子运行与 Provider native subagents 必须分开建模，并统一接入审计、canonical event、取消和终态机制。
- 业务拓扑采用 `Platform Product → {direct Managed AgentRun | OrchestrationRun → Managed AgentRun[]}`；每个 AgentRun 下仍可观察 Provider-native Agent Topology。代码通过 `OrchestrationService → AgentRunPort → AgentRunService` 单向组合，不把编排嵌入单 Agent Runtime Core。
- Workflow/Arena 保留流程图、工作区、Human Gate 和 winner 选择等产品语义；Orchestration Runtime 只提供通用父子运行、fan-out/join、取消、失败和恢复原语。
- 每个 OrchestrationRun 在启动时冻结不可变 `OrchestrationPlan Snapshot`；运行中的 Workflow/Arena 编辑只影响后续运行，当前运行和崩溃恢复始终使用原快照。
- 失败策略按节点或 fan-out group 固化：普通 Workflow 默认 `fail_fast`，Arena 默认 `allow_partial`；策略判定、兄弟取消和聚合终态都必须产生可回放的编排事件。
- Fan-in/join 同时支持 `all`、`any`、`each`，普通汇总默认 `all`；它与失败策略独立配置，触发判定和已消费的上游结果必须可持久化、可回放且不会重复启动下游。
- 下游只接收被消费上游 AgentRun 的首条 canonical input 和最终 canonical Agent output；不自动注入完整 transcript、diff、文件列表、workspace 路径或原生审计内容。`all` 传全部上游对，`any` 传实际触发者，`each` 每次只传当次触发者。
- `allow_partial` 中失败/取消上游只参与 join 终态计数，不生成 handoff，也不伪造最终输出或注入错误详情；存在成功结果时继续，全部失败时编排失败。`fail_fast` 则阻止下游启动并执行取消传播。
- `any` 首次成功后默认让其他上游继续运行，也可显式设置 `cancel_remaining`；取消策略独立于 join/failure policy，采用可审计的尽力取消，不回滚 workspace 修改，迟到成功不重复触发下游。
- 平台自动重试默认关闭，节点可显式配置最大 RunAttempt 数、退避、可重试终态和 `resume|restart`；每次平台重试在同一 AgentRun/Turn 下新增 RunAttempt且不回滚文件。Provider 内部 API 重试仍属于当前 RunAttempt。
- V1 不设置 AgentRun/OrchestrationRun 执行超时、空闲超时或截止时间；Agent 可以无限期运行或等待输入/审批。lease 过期和服务重启只触发 reconciliation，不结束或替换原 Agent process；人工/流程显式取消仍然有效。
- `each` 产生的多个下游默认并行，也可显式串行；串行按上游成功事件顺序持久排队。V1 不设置全局 Runtime 并发数量上限，共享 workspace 并行仍不加锁、不仲裁、不回滚。
- Agent 等待 input/approval 时只阻塞自身及依赖分支，无关分支继续；全部剩余工作都在等待时，OrchestrationRun 显示非终态 waiting 状态并可无限期恢复。回复继续原 AgentRun/RunAttempt，不修改冻结计划。
- Orchestration Runtime 使用独立的 append-only event log/reducer，并通过持久化 outbox/inbox、幂等 AgentRun 命令、消费游标和 lease/reconciliation 跨重启恢复；至少一次投递不得变成重复 Agent process。
- 显式取消 OrchestrationRun 时先进入 `cancelling`、停止新 dispatch 并取消活跃子运行；确认所有受管理子运行终态后才进入 `cancelled`。取消单个子运行默认不取消父运行，迟到事件保留但不能重新打开父运行或重复触发 join。
- `AgentRunPort` 是 Orchestration Runtime 调用新 AgentRun Runtime 的稳定窄接口，不承担旧 Executor 桥接或旧数据投影。
- Worktree 是编排可选的协作模式：`shared_workspace` 支持多个 Agent 在同一真实目录串行或并行协作，`isolated_worktree` 支持独立候选和后续比较/提升；Runtime 不强制隔离，也不把显式共享静默改成副本。
- 共享目录并行模式不引入 Runtime 文件锁或自动写入仲裁；Agent/Workflow 自行协调任务边界，并发冲突和部分修改作为真实 worktree 结果保留。
- capability 统一使用 `native`、`emulated`、`unsupported`、`unknown` 四种状态；请求执行前必须解析并固化本次 RunAttempt 的能力快照，禁止静默降级。
- `unknown` 在可安全探测时先探测，仍无法确认则显式拒绝依赖该能力的操作；`emulated` 必须由 profile/run policy 明确允许，并记录模拟策略与限制。
- 原生能力不能在运行时悄悄切换为模拟能力，例如 native resume 失败后不得自动改用 transcript backfill 或新会话。
- V1 的一等 Runtime 范围缩为 Gemini、Codex、Claude Code 和 Oh My Pi，不设置单一试点；Oh My Pi 明确指向 `can1357/oh-my-pi` 的 `omp` CLI，首期通过外部进程 stdio RPC 接入，不嵌入 Node SDK。
- 四个目标 Adapter 直接接入新契约、Native Audit 和 canonical event/state；四者都达到适用能力、fixture/replay、Reducer、重启恢复和产品消费者门槛后，V1 才完成验收。
- 原四家 V1 阶段曾移除 Amp、OpenCode、Cursor Agent、Qwen Code、Copilot 和 Droid 的旧接入。2026-10-05 已批准以现有底座重新接入 OpenCode，并新增 DeepSeek Harness；不是恢复旧 OpenCode Executor，也不改变其他已移除厂商的范围。接入方式及原生限制见 [OpenCode](../../agents/opencode.mdx) 和 [DeepSeek Harness](../../agents/deepseek-harness.mdx)。
- 实施顺序固定为：新契约与 schema、Native Audit、Supervisor/Transport/reconciliation、四个 Adapter、Platform Orchestration、canonical 消费者、旧与非目标代码清理及完整验证。
- 不建设 Legacy bridge、旧/新 normalizer 双写、shadow 状态机或运行时回退开关。开发失败通过 Git commit/tag 和重新部署回退；开发数据库可以重置并按新 schema 重建。
- 当前启动时清理 registered Agent process、把 running Workflow node 直接判失败的行为必须改为 reconciliation；这是长任务跨服务重启验收的阻断门槛。

## 文档使用方式

设计文档优先帮助实现者理解语义、职责边界、参考来源和取舍，不预先锁死具体 trait、表结构、目录名和完整事件枚举。

- 架构图和对象关系用于提供共同语言。
- 模块职责和参考项目用于减少重复探索。
- 示例接口、状态、字段和存储布局都可以按现有代码调整。
- 实现者可以合并或拆分模块，只要整体仍然能够区分 Provider 差异、运行状态和审计来源。

## 文档索引

- [竞品模型对比](./competitor-analysis.md)：AionUI、MultiCA、Happy、QM 的 runtime/session/turn/event/adapter 分析。
- [VK 目标架构](./architecture.md)：组件边界、数据模型、审计双轨、直接替换和验证策略。
- [智能体对话重构设计](../agent-chat-redesign/design.md)：六家原生协议、MindFS / Paseo / Multica / QM 的取舍，以及共享对话投影、工具交互、历史恢复和渲染性能的设计提案。

## 参考范围

结论基于 `docs/daily-radar/.cache` 的本地项目快照，而不是对上游当前分支的实时保证：

```text
docs/daily-radar/.cache/iOfficeAI__AionUi
docs/daily-radar/.cache/multica-ai__multica
docs/daily-radar/.cache/slopus__happy
docs/daily-radar/.cache/yc-software__qm
```

落地时，provider 协议字段和 CLI 参数仍需通过 native fixture 与对应版本的适配器测试确认。
