![muthr](https://raw.githubusercontent.com/tappunk/.github/refs/heads/main/assets/muthr.webp)

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Crates.io Version](https://img.shields.io/crates/v/muthr?color=orange&cacheSeconds=3600)](https://crates.io/crates/muthr)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr?color=blue)](https://github.com/tappunk/muthr/releases)
[![X Follow](https://img.shields.io/twitter/follow/tappunk?style=social)](https://x.com/tappunk)

# muthr
> \[!NOTE]
> Experimental not for production use

**Zero-trust orchestrator for MLX inference, container-based sandboxes, and MCP services on Apple Silicon.**

[Installation](#installation) · [Quick Start](#quick-start) · [Usage](#usage) · [Architecture](#architecture) · [MCP Compatibility](#mcp-compatibility) · [Configuration](#configuration)

## Features

- **MLX host inference** — manages `mlxcel-server` lifecycle on macOS with OpenAI-compatible API surface
- **Per-project sandbox containers** — container-based sandboxes with workspace mounts and profile-based provisioning (`base`, `opencode`)
- **MCP services** — persistent services containers with MCP bridge and SearXNG for agent tool access
- **Zero-trust isolation** — agents get workspace access without host OS, SSH keys, or home directory exposure
- **Full lifecycle** — `muthr run` boots engine + services, `muthr shutdown` tears down owned components
- **Machine-readable output** — JSON and NDJSON support on status/list commands for automation
- **Shell completions** — generated completions for bash, zsh, fish, and powershell

## Installation

### Homebrew

```bash
brew install tappunk/muthr/muthr
```

### Cargo

```bash
cargo install muthr
```

### Prebuilt binaries

Download from [GitHub Releases](https://github.com/tappunk/muthr/releases).

## Quick Start

```bash
muthr init
muthr run
cd ~/src/myproject
muthr sandbox start --profile opencode
```

## Usage

### Manage the inference engine

```bash
muthr engine start --profile mlx-community/Qwen3.5-9B-MLX-4bit
muthr engine status
muthr engine presets
muthr engine stop
```

### Provision sandbox containers

```bash
cd ~/src/myproject
muthr sandbox start --profile opencode
muthr sandbox ls
muthr sandbox stop
muthr sandbox delete --yes
```

### Manage the services container

```bash
muthr services start
muthr services status
muthr services restart
muthr services stop
muthr services delete --yes
```

### Full stack lifecycle

```bash
muthr run --verbose
muthr shutdown --yes
```

### Manage configuration

```bash
muthr config init
muthr config show
```

### Shell completions

```bash
muthr completion zsh
muthr completion bash
muthr completion fish
muthr completion powershell
```

## Architecture

```
┌────────────────────────── macOS Host ──────────────────────────┐
│                                                                │
│  ┌──────────────┐        ┌────────────────────┐                │
│  │ mlxcel-server│        │ muthr-services     │                │
│  │ (Metal GPU)  │        │                    │                │
│  └──────┬───────┘        │  ┌──────────────┐  │                │
│         │                │  │ MCP Bridge   │  │                │
│         │                │  │ SearXNG      │  │                │
│  ┌──────┴───────┐        │  └──────────────┘  │                │
│  │ Sandboxes    │        └────────────────────┘                │
│  │ (container)  │                                              │
│  └──────────────┘                                              │
│                                                                │
│  Agent access: workspace mount + OpenAI URL + MCP tools only   │
│  Host secrets/SSH keys/home remain outside sandbox boundary    │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

muthr orchestrates three layers: host inference (`mlxcel-server`), persistent services containers, and per-project sandbox containers for agent execution.

## Engine Runtime

muthr is **mlxcel-only**.

Runtime selection precedence (highest to lowest):

1. CLI flag (`--runtime` on `muthr run`, must be `mlxcel`)
2. Environment variable (`MUTHR_ENGINE_RUNTIME`, must be `mlxcel`)
3. Config value (`default_engine_runtime` in `muthr.toml`, must be `mlxcel`)
4. Built-in fallback (`mlxcel`)

`muthr engine start` does not accept a runtime flag.

## MCP Compatibility

`muthr-services` provides the persistent MCP + SearXNG integration layer used by sandboxed agents.

- SearXNG endpoint (host): `http://127.0.0.1:18766`
- MCP access: stdio bridge through `muthr-services` container

## Configuration

muthr stores config in `~/.config/muthr/` and runtime state in `~/.cache/muthr/`.

Configuration files:

- `~/.config/muthr/muthr.toml` — server port, workspace root, model dir, default provision profile, engine runtime, default engine profile
- `~/.config/muthr/sandbox.d/container/manifests/` — container profile metadata
- `~/.config/muthr/sandbox.d/container/provision.d/` — profile provisioning scripts
- `~/.config/muthr/clients/` — reference templates

Model identity is a Hugging Face repository ID end-to-end (for example `mlx-community/Qwen3.5-9B-MLX-4bit`).

Environment variable overrides:

```bash
MUTHR_SERVER_PORT
MUTHR_WORKSPACE_ROOT
MUTHR_MODEL_DIR
MUTHR_PROVISION_PROFILE
MUTHR_ENGINE_RUNTIME
MUTHR_ENGINE_PROFILE
MUTHR_CONTAINER_HOST_GATEWAY
```

Profile system:

- `base` — minimal Debian 13 container with shell access
- `opencode` — opencode setup with MCP integration

All profile assets are managed via [muthr-specs](https://github.com/tappunk/muthr-specs). Run `muthr init` to refresh local config.
