<p align="center">
  <picture>
    <source srcset="packages/public/vibe-kanban-logo-dark.svg" media="(prefers-color-scheme: dark)">
    <source srcset="packages/public/vibe-kanban-logo.svg" media="(prefers-color-scheme: light)">
    <img src="packages/public/vibe-kanban-logo.svg" alt="Vibe Studio logo">
  </picture>
</p>

<p align="center"><strong>面向独立开发者和 OPC（一人公司）的多智能体工作室。</strong></p>
<p align="center">在一个 Web 界面安装和使用 AI 智能体，管理配置和工具，再让它们协作开发产品、研究想法、完成交付。</p>

<p align="center">
  <a href="https://www.npmjs.com/package/vibe-studio"><img alt="npm beta" src="https://img.shields.io/npm/v/vibe-studio/beta?style=flat-square" /></a>
  <a href="LICENSE"><img alt="license" src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square" /></a>
</p>

<p align="center"><a href="README.md">English</a> | 中文</p>

## 安装并启动

安装 [Node.js](https://nodejs.org/)（推荐 22），然后运行：

```bash
npx --yes --registry https://registry.npmjs.org/ vibe-studio@beta
```

2.0 版本线使用 npm 的 `beta` 标签。更名后的 `vibe-studio` 包在首次成功发布后可用，发布状态见 [GitHub Actions](https://github.com/toby1123yjh/vibe-studio/actions/workflows/publish-easy-npx.yml)。发布包面向 **Linux x64 和 Windows x64**。使用 npm 安装包不需要 Rust。

打开启动时输出的访问地址。Vibe Studio 管理的是运行服务的那台机器：智能体 CLI、配置、项目文件和执行都在该主机上，即使你从别处打开 Web 界面也一样。

### 接入智能体

打开 **智能体 → 智能体中心**：

1. 已经安装过智能体？点击 **重新扫描本机**，读取当前安装和配置。
2. 尚未安装？若界面提供预设安装程序，可直接安装并查看安装输出。使用 npm 的安装程序可以选填 registry，仅对本次安装生效。
3. 完成智能体自己的登录或 API Key 配置。安装不等于认证成功，扫描也不能验证所有智能体的登录状态。
4. 管理智能体支持的设置，例如 API Key、API 地址、模型和思考等级，以及 MCP 服务、技能和命令。

也支持手动安装。预设安装失败时，可以直接在同一台主机上安装 CLI，再回到这里重新扫描，不必重新配置 Vibe Studio。

| 智能体 | 原生运行时 | 配置说明 |
| --- | --- | --- |
| Claude Code | `claude` CLI | [文档](docs/agents/claude-code.mdx) |
| OpenAI Codex | `codex` CLI | [文档](docs/agents/openai-codex.mdx) |
| Gemini CLI | `gemini` CLI | [文档](docs/agents/gemini-cli.mdx) |
| Oh My Pi | `omp` 运行时 | [文档](docs/agents/oh-my-pi.mdx) |
| OpenCode | 通过 ACP 使用 `opencode` | [文档](docs/agents/opencode.mdx) |
| DeepSeek Harness | 通过 ACP 使用 `dsh` · 开发预览 | [文档](docs/agents/deepseek-harness.mdx) |

各智能体的能力和认证方式不同。Vibe Studio 使用你安装的 CLI，而不是为每个智能体内置固定版本。

## 开始第一次对话

点击 **新建会话**，选择智能体，发送消息即可。不需要先创建项目、创建任务或选择目录。

内置的默认项目会收纳这些会话，每个新会话自动分配一个独立子目录。之后可以在设置中修改默认目录；它只影响新建会话的位置，不会改变已有会话。

需要用看板组织工作时：

```text
创建项目并选择工作目录
  → 创建任务
  → 选择智能体执行
  → 对话、查看文件并继续工作
```

任务描述要完成的工作，执行记录和智能体会话承载具体执行。它可以是代码，也可以是文档、调研或其他基于文件的工作。

## 日常功能

- **智能体会话** —— 在不同会话中使用不同智能体，继续对话、查看工具活动并停止运行中的工作。会话一旦启动，就固定其智能体和工作目录。
- **智能体配置和工具** —— 读取已有配置，管理 MCP 服务和技能，启用或停用支持的工具，并在支持时向其他智能体复制可移植配置。
- **项目和任务** —— 使用真实目录，在看板上组织任务并跟踪它们的执行。
- **文件和审查** —— 浏览项目当前文件；项目是 Git 仓库时，也可以查看 Git 差异。
- **按需使用 Git** —— 编码时使用分支、独立 worktree、拉取请求和合并，也可以不依赖 Git，直接在普通目录中工作。
- **终端和预览** —— 在工作区查看命令输出，预览项目的开发服务器。

![任务和工作区界面](packages/public/vibe-kanban-screenshot-workspace.png)

## 多智能体工作流

当一个任务需要多个智能体分工时，可以使用可视化画布。配置智能体 Node 和控制 Node，通过连线组成流程，由运行时推进执行。

例如，文档工作流可以汇总两路独立调研，形成一份交付物：

```text
                  ┌→ 调研 A ─┐
开始 → 规划 ──────┤          ├→ 评审 → 生成报告 → 结束
                  └→ 调研 B ─┘
```

- 智能体 Node 使用真实的 CLI 会话。开始、结束、路由和人工决策 Node 负责控制流程，并不是对话。
- 不同智能体 Node 可以使用不同智能体。运行时负责调度依赖、并行分支和汇合，并记录等待、失败和取消状态。
- 工作在配置好的工作区中进行：可以是共享目录，也可以是 Git worktree。文件承载共享材料和交付物。
- 工作流模式的任务对应一个工作流实例。首次执行被受理后，实例的图和配置固定；后续模板修改只影响新实例。
- 配置的主智能体可以在你提出请求时帮助管理工作流、查看状态和解释结果。流程调度由运行时负责，而不是主智能体。

![工作流画布](packages/public/workflow-canvas.png)

## 接入其他系统

外部工作流 API 允许其他应用创建或复用项目、创建任务、上传材料，并异步运行已在 Vibe Studio 中准备好的工作流。调用方可以查询状态、处理待人工决定的事项、取消工作，以及浏览或下载项目文件。

先配置接入凭据、授权项目，并开启工作流的外部调用。请求使用项目 ID 和项目相对路径，不传任意服务器路径。同一项目同时最多运行一个活跃工作流执行，后续已受理请求进入队列。

API 还会返回能够确认属于本次智能体执行的文件变更。这个清单可能不完整，它与 Git 差异、目录当前的全部文件是不同的信息；获取所需文件时，应使用目录浏览和下载能力。

完整调用流程见[外部工作流 API 文档](docs/integrations/external-workflows.mdx)。

> 服务默认只监听本机地址。对外开放前，应单独保护 Web 界面和管理 API，并使用 HTTPS。接入凭据只保护对应范围的 API，不保护整个应用。

## 数据和运行边界

- 项目的任务定义、状态、标签和关联保存在 `.vibe-kanban/project.json` 中，看板和外部 API 使用同一份项目数据。
- 本机数据库保存会话、执行和运行时记录。移动项目目录不会带走这些记录、凭据、工作流模板或正在运行的进程。
- 智能体原生会话和配置仍保存在各自位置，不会自动导入 Vibe Studio 的历史记录。
- 从 Vibe Studio 中移除项目不会删除项目工作目录。

## 本地开发

### 环境要求

- 通过 [rustup](https://rustup.rs/) 安装 Rust，使用 `rust-toolchain.toml` 固定的 `nightly-2025-12-04`
- [Node.js](https://nodejs.org/) 推荐 22
- [pnpm](https://pnpm.io/) `10.13.1`，与 `package.json` 一致

```bash
pnpm i
pnpm run dev
```

开发启动器会启动后端和 Web 应用。请使用启动时输出的地址，不要假设端口固定。

> 当前测试阶段，每次调试后端启动都会从 `dev_assets_seed` 重置开发 SQLite 数据库。重启会丢弃本机会话、执行记录以及保存在数据库中的凭据变更，但不会删除项目文件（包括 `.vibe-kanban/project.json`）和已有工作目录。npm/正式安装使用持久化用户数据库，不加载这份开发快照。

| 命令 | 说明 |
| --- | --- |
| `pnpm run check` | 检查前端和 Rust workspace 类型 |
| `pnpm run lint` | 运行 ESLint 和 Clippy |
| `pnpm run format` | 运行 Prettier 和 rustfmt |
| `cargo test --workspace` | 运行 Rust 测试 |
| `pnpm run generate-types` | 使用 ts-rs 从 Rust 重新生成 TypeScript 类型 |
| `pnpm run dev:fixture` | 重建提交到仓库的开发数据库快照 |
| `pnpm run dev:fixture:check` | 检查快照，不修改它 |

前端位于 `packages/local-web` 和 `packages/web-core`，后端是 `crates/` 下的 Rust workspace。共享 TypeScript 类型从 Rust 生成，不要手动编辑 `shared/types.ts`。

## 致谢与许可

Vibe Studio 是 Bloop AI 的 [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) 的独立维护分叉版本，感谢原团队的开发与开源。

基于 [Apache License 2.0](LICENSE) 开源，署名信息见 [NOTICE](NOTICE)。
