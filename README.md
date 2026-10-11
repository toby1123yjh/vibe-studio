<p align="center">
  <picture>
    <source srcset="packages/public/vibe-kanban-logo-dark.svg" media="(prefers-color-scheme: dark)">
    <source srcset="packages/public/vibe-kanban-logo.svg" media="(prefers-color-scheme: light)">
    <img src="packages/public/vibe-kanban-logo.svg" alt="easy-vibe-kanban logo">
  </picture>
</p>

<p align="center"><strong>Your AI agents, together in one web interface.</strong></p>
<p align="center">Install and use agent CLIs, manage their settings and tools, then organize bigger work with projects, Tasks, and multi-agent workflows.</p>

<p align="center">
  <a href="https://www.npmjs.com/package/easy-vibe-kanban"><img alt="npm beta" src="https://img.shields.io/npm/v/easy-vibe-kanban/beta?style=flat-square" /></a>
  <a href="LICENSE"><img alt="license" src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square" /></a>
</p>

<p align="center">English | <a href="README.zh-CN.md">中文</a></p>

## Install and start

Install [Node.js](https://nodejs.org/) (22 recommended), then run:

```bash
npx --yes --registry https://registry.npmjs.org/ easy-vibe-kanban@beta
```

The current 2.0 version line is published under the npm `beta` tag; omitting `@beta` selects the older `latest` channel. Published builds currently cover **Linux x64 and Windows x64**. Rust is not needed to run the npm package.

Open the address printed at startup. Vibe Kanban manages the machine running its service: agent CLIs, configuration, project files, and execution all live on that host, even when you access the web UI from elsewhere.

### Connect an agent

Open **Agents → Agent Center**:

1. Already installed an agent? **Rescan host** to discover its current installation and configuration.
2. Otherwise, use the preset installer in the UI when available and inspect its installation output. For npm-based installers, an optional registry setting applies to that installation only.
3. Complete the agent's own login or API-key setup. Installation is not authentication, and scanning cannot verify every provider's login state.
4. Manage the settings the agent supports—such as API key, API base URL, model, and reasoning effort—alongside its MCP servers, skills, and commands.

Manual installation remains supported. If the preset installer fails, install the CLI on the same host yourself and rescan; there is no need to recreate your Vibe Kanban setup.

| Agent | Native runtime | Setup |
| --- | --- | --- |
| Claude Code | `claude` CLI | [Guide](docs/agents/claude-code.mdx) |
| OpenAI Codex | `codex` CLI | [Guide](docs/agents/openai-codex.mdx) |
| Gemini CLI | `gemini` CLI | [Guide](docs/agents/gemini-cli.mdx) |
| Oh My Pi | `omp` runtime | [Guide](docs/agents/oh-my-pi.mdx) |
| OpenCode | `opencode` over ACP | [Guide](docs/agents/opencode.mdx) |
| DeepSeek Harness | `dsh` over ACP · developer preview | [Guide](docs/agents/deepseek-harness.mdx) |

Agent capabilities and authentication methods differ. Vibe Kanban uses your installed CLIs rather than bundling a fixed version of each agent.

## Start your first conversation

Click **New session**, choose an agent, and send a message. You do not have to create a project, create a Task, or choose a directory first.

The built-in default project collects these sessions. Each new session gets its own subdirectory automatically. You can change the default directory in Settings later; that changes the location of future sessions, not existing ones.

For work you want to organize on a board:

```text
Create a project and choose its directory
  → create a Task
  → choose an agent execution
  → chat, review files, and continue the work
```

A Task describes the work; its executions and agent sessions carry it out. This can be code, documents, research, or other file-based work.

## Everyday features

- **Agent sessions** — use different agents across sessions, continue conversations, inspect tool activity, and stop running work. A started session stays bound to its agent and working directory.
- **Agent settings and tools** — discover existing configuration, manage MCP servers and skills, enable or disable supported tools, and copy portable definitions between agents where supported.
- **Projects and Tasks** — keep work in real directories, organize Tasks on a kanban board, and track their executions.
- **Files and review** — browse current project files and inspect Git diffs where a repository is available.
- **Optional Git workflow** — use branches, isolated worktrees, pull requests, and merges for coding work, or work directly in an ordinary directory without Git.
- **Terminal and preview** — inspect command output and preview a project's development server from the workspace.

![Task and workspace view](packages/public/vibe-kanban-screenshot-workspace.png)

## Multi-agent workflows

Use the visual canvas when a Task benefits from several agents working in defined roles. Configure agent Nodes and control Nodes, connect them, and let the runtime move the work through the graph.

For example, a document workflow can combine independent research before producing one deliverable:

```text
                  ┌→ Research A ─┐
Start → Plan ─────┤              ├→ Review → Produce report → End
                  └→ Research B ─┘
```

- Agent Nodes use real CLI sessions. Start, End, routing, and human-decision Nodes provide flow control rather than pretending to be conversations.
- Different agent Nodes can use different providers. The runtime schedules dependencies, parallel branches, and joins, and records waiting, failure, and cancellation states.
- Work happens in the configured workspace: a shared directory or a Git worktree. Files provide the shared materials and deliverables.
- A workflow-mode Task has one workflow instance. Once its first run is accepted, that instance's graph and configuration are fixed; subsequent template edits affect new instances.
- A configured main agent can help manage the workflow, check its state, and explain results when you ask. The runtime—not the main agent—controls scheduling.

![Workflow canvas](packages/public/workflow-canvas.png)

## Connect another system

The external workflow API lets another application create or reuse a project, create a Task, upload materials, and submit an asynchronous run of a workflow prepared in Vibe Kanban. It can query status, handle pending human decisions, cancel work, and browse or download project files.

Configure an integration key, grant project access, and enable external calls on the workflow. Requests use project IDs and project-relative paths, not arbitrary server paths. A project runs at most one active workflow execution at a time; additional accepted requests queue.

The API also reports file changes that can be attributed to the run's agents. This list can be incomplete: it is separate from Git diffs and from the full current directory contents. Use directory browsing and downloads to obtain the files you need.

See the [external workflow API guide](docs/integrations/external-workflows.mdx) for the complete request flow.

> The service listens on localhost by default. Before exposing it over a network, protect the web UI and management API separately and use HTTPS. An integration key protects its scoped API, not the whole application.

## Data and runtime boundaries

- Project Task definitions, statuses, tags, and relationships live in `.vibe-kanban/project.json`. The board and external API use this same project-owned data.
- The host database keeps local conversation, execution, and runtime records. Moving a project directory does not move those records, credentials, workflow templates, or running processes.
- Provider-native sessions and configuration stay in the provider's own locations. They are not automatically imported into Vibe Kanban's history.
- Removing a project from Vibe Kanban does not delete its working directory.

## Development

### Prerequisites

- [Rust via rustup](https://rustup.rs/), using the pinned `nightly-2025-12-04` toolchain in `rust-toolchain.toml`
- [Node.js](https://nodejs.org/) 22 recommended
- [pnpm](https://pnpm.io/) `10.13.1`, as pinned in `package.json`

```bash
pnpm i
pnpm run dev
```

The development launcher starts the backend and web app. Read the addresses it prints instead of assuming fixed ports.

> During the current testing phase, every debug backend startup resets the development SQLite database from `dev_assets_seed`. Local conversation, execution, and database-stored credential changes are discarded on restart. Project files, including `.vibe-kanban/project.json`, and existing workspace directories are not deleted. npm/release installations use a persistent user database and do not load this fixture.

| Command | Description |
| --- | --- |
| `pnpm run check` | Type-check the frontend and Rust workspaces |
| `pnpm run lint` | Run ESLint and Clippy |
| `pnpm run format` | Run Prettier and rustfmt |
| `cargo test --workspace` | Run Rust tests |
| `pnpm run generate-types` | Regenerate TypeScript types from Rust with ts-rs |
| `pnpm run dev:fixture` | Rebuild the committed development database fixture |
| `pnpm run dev:fixture:check` | Validate the fixture without changing it |

The frontend is in `packages/local-web` and `packages/web-core`; the backend is a Rust workspace under `crates/`. Shared TypeScript types are generated from Rust—do not edit `shared/types.ts` by hand.

## Acknowledgements and license

easy-vibe-kanban is an independently maintained fork of [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) by Bloop AI. Thanks to the original team for building and open-sourcing it.

Licensed under the [Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for attribution details.
