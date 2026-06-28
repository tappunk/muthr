![muthr](https://raw.githubusercontent.com/tappunk/.github/refs/heads/main/assets/muthr.webp)

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Crates.io Version](https://img.shields.io/crates/v/muthr?color=orange&cacheSeconds=3600)](https://crates.io/crates/muthr)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr?color=blue)](https://github.com/tappunk/muthr/releases)
[![X Follow](https://img.shields.io/twitter/follow/tappunk?style=social)](https://x.com/tappunk)

# muthr

**Zero-trust orchestrator for local AI on Apple Silicon. Manages MLX GPU inference, per-project Linux containers, and MCP service routing.**

[Installation](#installation) • [Quick Start](#quick-start) • [Usage](#usage) • [Architecture](#architecture) • [MCP Compatibility](#mcp-compatibility) • [Configuration](#configuration)

## Features

- **GPU accelerated inference** — mlxcel-server on the host with Metal support, automatic VRAM tuning, and preset profile management
- **Per-project sandbox containers** — container-based sandboxes with workspace mounts and profile-based provisioning (base, opencode)
- **MCP services** — persistent services container with MCP server and SearXNG for agent tool access
- **Zero-trust isolation** — agents get full read-write workspace access but zero host OS, SSH key, or filesystem exposure
- **Full lifecycle** — `muthr run` boots the complete stack `muthr shutdown` stops everything with timeout management
- **Model management** — mlx downloads from HuggingFace with progress bars, directory organization, and HF token auth
- **Machine readable output** — JSON and NDJSON modes for all commands to support automation and agent pipelines
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
muthr init                   # Clone runtime profiles and container definitions
muthr download mlx-community/Qwen3.6-35B-A3B-4bit \
               config.json  # Download a model
muthr run                    # Boot inference engine + services container
muthr sandbox start          # Create a sandbox for the current project
```

That is all you need to have a zero-trust local AI agent running.

> **Lower memory or bandwidth?** Use the efficient preset after init: `muthr engine start --profile mlxcel/efficient-qwen3.5-9b-mlx-4bit.ini`.

## Usage

### Manage the inference engine

```bash
muthr engine start           # Start mlxcel-server with preset selection
muthr engine start --profile mlxcel/quality-qwen3.6-35b-a3b-4bit.ini
muthr engine stop            # Stop mlxcel runtime
muthr engine presets         # List available preset profiles
muthr engine status          # Check engine state
```

### Provision sandbox containers

```bash
cd ~/src/myproject
muthr sandbox start          # Create sandbox with profile prompt
muthr sandbox start --profile opencode   # Create with a specific profile
muthr sandbox ls             # List all managed sandboxes
muthr sandbox stop           # Stop the active sandbox
muthr sandbox delete         # Delete the active sandbox
```

### Manage the services container

```bash
muthr services start           # Create and provision the muthr-services container
muthr services stop            # Stop the services container
muthr services status          # Check services container state
muthr services restart         # Stop and restart the services container
muthr services delete          # Delete the services container (requires --yes or --force)
```

### Full stack lifecycle

```bash
muthr run                    # Boot inference engine + services container
muthr shutdown               # Stop everything with timeout management
```

### Download models

```bash
muthr download org/model config.json
muthr download https://huggingface.co/org/model/resolve/main/config.json
muthr download mlx-community/Qwen3.6-35B-A3B-4bit
muthr download https://huggingface.co/mlx-community/Qwen3.6-35B-A3B-4bit/tree/main
```

When no filename is provided, `muthr download` downloads the full repository file set at the requested revision.

### Manage configuration

```bash
muthr config init            # Create muthr.toml (use --force to overwrite)
muthr config show            # Print resolved configuration
```

### Shell completions

```bash
muthr completion zsh         # Add to ~/.zshrc
muthr completion bash        # Add to /etc/bash_completion.d/
muthr completion fish        # Add to fish completion directory
muthr completion powershell  # Add to PowerShell profile
```

## Architecture

```
┌────────────────────────── macOS Host ──────────────────────────┐
│                                                                │
│  ┌──────────────┐        ┌────────────────────┐                │
│  │ mlxcel-server│        │ muthr-services     │                │
│  │  (Metal GPU) │        │                    │                │
│  └──────┬───────┘        │  ┌──────────────┐  │                │
│         │                │  │ MCP Server   │  │                │
│         │                │  │ SearXNG      │  │                │
│         │                │  └──────────────┘  │                │
│  ┌──────┴───────┐        │  (runs contin.)    │                │
│  │ Containers   │        └────────────────────┘                │
│  │              │                                              │
│  │ ┌──────────┐ │                                              │
│  │ │ Agent    │ │                                              │
│  │ │ code     │ │                                              │
│  │ │          │ │                                              │
│  │ │ workspace│ │── read-write mount ────────┐                 │
│  │ │ inference│ │── read-only API call ──────┤                 │
│  │ │ MCP tools│ │── read-only RPC call ──────┤                 │
│  │ └──────────┘ │                            │                 │
│  └──────────────┘                            │                 │
│                                              │                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  ZERO-TRUST BOUNDARY                                     │  │
│  │                                                          │  │
│  │  Host OS / SSH keys / secrets  ────  NOT accessible      │  │
│  │                                          to agent        │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

muthr orchestrates three layers: the inference engine on the host, a persistent services container for MCP tool access, and per-project sandbox containers for agent execution. Agents connect to the host inference server and services container over the container network. The workspace directory is mounted read-write into each sandbox container so agents can read and modify project files. The host OS, SSH keys, and sensitive files are never mounted into any sandbox.

## MCP Compatibility

The `muthr-services` container provides a persistent MCP server and SearXNG instance for agent tool access:

```
mcp://  →  MCP server for tool calling
searxng →  web search via SearXNG
```

The services container is provisioned during `muthr run` and runs continuously until `muthr shutdown`. Agents connect to it from their sandbox containers over the container network.

## Configuration

muthr stores configuration in `~/.config/muthr/` and runtime state in `~/.cache/muthr/`.

**Configuration files:**

- `~/.config/muthr/muthr.toml` — main config file (server port, workspace root, model directory, default profile, default engine runtime)
- `~/.config/muthr/provider.d/mlxcel/*.ini` — mlxcel preset profiles (model paths and sampling defaults)
- `~/.config/muthr/sandbox.d/container/manifests/` — container profile metadata
- `~/.config/muthr/sandbox.d/container/provision.d/` — profile-specific boot scripts
- `~/.config/muthr/clients/` — client config templates (reference only)

For `mlxcel`, muthr currently maps these preset keys to CLI flags:

- global: `host`, `port`
- slot: `model`, `max-output-tokens`, `temp`, `top-p`, `top-k`, `min-p`, `repeat-penalty`

Known-good `mlxcel` preset for `mlx-community/Qwen3.6-35B-A3B-4bit` (`mlxcel/quality-qwen3.6-35b-a3b-4bit.ini`):

```ini
[*]
host = 0.0.0.0
port = 8080

[01-qwen3-6-35b-a3b-4bit]
model = /Users/user/opt/models/mlx-community/Qwen3.6-35B-A3B-4bit
max-output-tokens = 131072
```

Equivalent command:

```bash
mlxcel-server -m /Users/user/opt/models/mlx-community/Qwen3.6-35B-A3B-4bit \
  --port 8080 \
  --host 0.0.0.0 \
  --predict 131072
```

**Runtime selection precedence (highest to lowest):**

1. CLI flag (`--runtime`, currently only `mlxcel`)
2. Environment variable (`MUTHR_ENGINE_RUNTIME`, must be `mlxcel`)
3. Config value (`default_engine_runtime` in `muthr.toml`, must be `mlxcel`)
4. Built-in fallback (`mlxcel`)

Example:

```bash
MUTHR_ENGINE_RUNTIME=mlxcel muthr run
```

**Environment variable overrides:**

```bash
MUTHR_SERVER_PORT          # Override server port (default: 8080)
MUTHR_WORKSPACE_ROOT       # Override workspace root directory
MUTHR_MODEL_DIR            # Override model storage directory
MUTHR_PROVISION_PROFILE    # Override default provision profile
MUTHR_ENGINE_RUNTIME       # Override default engine runtime (mlxcel)
MUTHR_CONTAINER_HOST_GATEWAY  # Optional override for container host gateway
```

**Profile system:**

Profiles define container resources and boot scripts. Available profiles:

- `base` — Minimal Debian 13 container with shell access only
- `opencode` — Full opencode AI setup with MCP service integration

Profile manifests are optional — create `<profile>.yaml` only if you need different container resources. muthr falls back to `base-sandbox.yaml` for profiles without a specific manifest.

All profile configs, sandbox manifests, and provision scripts are managed via [muthr-specs](https://github.com/tappunk/muthr-specs). Run `muthr init` to pull the latest profiles.
