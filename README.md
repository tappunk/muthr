![muthr](https://raw.githubusercontent.com/tappunk/.github/refs/heads/main/assets/muthr.webp)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Crates.io Version](https://img.shields.io/crates/v/muthr?color=orange&cacheSeconds=3600)](https://crates.io/crates/muthr)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr?color=blue)](https://github.com/tappunk/muthr/releases)
[![X Follow](https://img.shields.io/twitter/follow/tappunk?style=social)](https://x.com/tappunk)

# muthr

**Zero-trust orchestrator for local AI agents.** Manages GPU accelerated inference, per-project sandbox VMs, and MCP service routing.

[Installation](#installation) • [Quick Start](#quick-start) • [Usage](#usage) • [Architecture](#architecture) • [MCP Compatibility](#mcp-compatibility) • [Configuration](#configuration)

## Features

- **GPU accelerated inference** — llama.cpp on the host with Metal support, automatic VRAM tuning, and preset profile management
- **Per-project sandbox VMs** — Lima VMs with workspace mounts and profile-based provisioning (base, opencode, hermes-agent)
- **MCP services** — persistent services VM with MCP server and SearXNG for agent tool access
- **Zero-trust isolation** — agents get full read-write workspace access but zero host OS, SSH key, or filesystem exposure
- **Full lifecycle** — `muthr run` boots the complete stack `muthr shutdown` stops everything with timeout management
- **Model management** — GGUF downloads from HuggingFace with progress bars, directory organization, and HF token auth
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
muthr init                   # Clone runtime profiles and VM definitions
muthr run                    # Boot inference engine + services VM
muthr download unsloth/Qwen3.6-35B-A3B-GGUF Qwen3.6-35B-A3B-UD-Q4_K_M.gguf   # Download a working model
muthr sandbox start          # Create a sandbox for the current project
```

That is all you need to have a zero-trust local AI agent running.

> **Lower memory or bandwidth?** Use the 9B model instead: `muthr download unsloth/Qwen3.5-9B-GGUF Qwen3.5-9B-Q4_K_M.gguf` — runs on 24GB MacBooks.

## Usage

### Manage the inference engine

```bash
muthr engine start           # Start llama-server with preset selection
muthr engine stop            # Graceful SIGTERM with SIGKILL fallback
muthr engine presets         # List available preset profiles
muthr engine status          # Check engine state
```

### Provision sandbox VMs

```bash
cd ~/src/myproject
muthr sandbox start          # Create sandbox with profile prompt
muthr sandbox start --profile opencode   # Create with a specific profile
muthr sandbox ls             # List all managed sandboxes
muthr sandbox stop           # Stop the active sandbox
muthr sandbox delete         # Delete the active sandbox
```

### Full stack lifecycle

```bash
muthr run                    # Boot inference engine + services VM
muthr shutdown               # Stop everything with timeout management
```

### Download models

```bash
muthr download org/model model.gguf
muthr download https://huggingface.co/org/model/resolve/main/model.gguf
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
│  │ llama-server │        │ muthr-services VM  │                │
│  │  (Metal GPU) │        │                    │                │
│  └──────┬───────┘        │  ┌──────────────┐  │                │
│         │                │  │ MCP Server   │  │                │
│         │                │  │ SearXNG      │  │                │
│         │                │  └──────────────┘  │                │
│  ┌──────┴───────┐        │  (runs contin.)    │                │
│  │  VMs         │        └────────────────────┘                │
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

muthr orchestrates three layers: the inference engine on the host, a persistent services VM for MCP tool access, and per-project sandbox VMs for agent execution. Agents connect to the host inference server and services VM over `host.lima.internal`. The workspace directory is mounted read-write into each sandbox VM so agents can read and modify project files. The host OS, SSH keys, and sensitive files are never mounted into any sandbox.

## MCP Compatibility

The `muthr-services` VM provides a persistent MCP server and SearXNG instance for agent tool access:

```
mcp://  →  MCP server for tool calling
searxng →  web search via SearXNG
```

The services VM is provisioned during `muthr run` and runs continuously until `muthr shutdown`. Agents connect to it from their sandbox VMs over the Lima network interface.

## Configuration

muthr stores configuration in `~/.config/muthr/` and runtime state in `~/.cache/muthr/`.

**Configuration files:**

- `~/.config/muthr/muthr.toml` — main config file (server port, workspace root, model directory, default profile)
- `~/.config/muthr/provider.d/llama-cpp/*.ini` — inference preset profiles (context size, threading, model paths, GPU layers)
- `~/.config/muthr/manifests/` — VM architecture and resource definitions
- `~/.config/muthr/provision.d/` — profile-specific boot scripts
- `~/.config/muthr/clients/` — client config templates (reference only)

**Environment variable overrides:**

```bash
MUTHR_SERVER_PORT          # Override server port (default: 8080)
MUTHR_WORKSPACE_ROOT       # Override workspace root directory
MUTHR_MODEL_DIR            # Override model storage directory
MUTHR_PROVISION_PROFILE    # Override default provision profile
```

**Profile system:**

Profiles define VM resources and boot scripts. Available profiles:

- `base` — Minimal Debian 13 VM with shell access only
- `opencode` — Full opencode AI setup with MCP service integration
- `hermes-agent` — Hermes agent installation with local engine config

Profile manifests are optional — create `<profile>.yaml` only if you need different VM resources. muthr falls back to `base-sandbox.yaml` for profiles without a specific manifest.

All profile configs, sandbox manifests, and provision scripts are managed via [muthr-specs](https://github.com/tappunk/muthr-specs). Run `muthr init` to pull the latest profiles.
