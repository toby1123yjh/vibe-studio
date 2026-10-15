# Vibe Studio

A multi-agent studio for independent developers and one-person companies (OPCs).
Use your agents in a browser, organise tasks and files, and connect agents through
reusable workflows.

## Start

After the beta package is published, run:

```bash
npx vibe-studio@beta
```

Vibe Studio starts on your machine and opens in your browser. The current npm
release includes Windows x64 and Linux x64 binaries. Node.js 18.18 or newer is
required; Git is needed for Git repository features.

You can install Claude Code, Codex, Gemini CLI, Oh My Pi, OpenCode and DeepSeek
Harness from the Agent Center, or use agents already installed on the same host.
Configure credentials and models for the agents you use.

```bash
npx vibe-studio@beta --help
npx vibe-studio@beta --version
npx vibe-studio@beta review --help
npx vibe-studio@beta mcp --help
```

## Features

- Agent conversations with native configuration, skills, MCP and commands.
- Projects and tasks backed by local directories.
- Multi-agent workflows with file handoffs and deliverables.
- Shared-directory or isolated worktree execution.
- Review changes and continue work from the same conversation.
- API integration for external systems.

## Development and releases

Source, setup instructions and release status:
[toby1123yjh/vibe-studio](https://github.com/toby1123yjh/vibe-studio).

The package command is `vibe-studio`. Internal `vibe-kanban` executable names and
data directories are retained so existing runtime and storage paths keep working.
Based on [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) by Bloop AI.
