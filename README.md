![muthr](https://raw.githubusercontent.com/tappunk/.github/refs/heads/main/assets/muthr-banner.webp)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Crates.io Version](https://img.shields.io/crates/v/muthr?color=orange&cacheSeconds=3600)](https://crates.io/crates/muthr)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr?color=blue)](https://github.com/tappunk/muthr/releases)

**muthr** is a zero-trust orchestrator that automates **llama.cpp** and **Lima** to run local AI agents. It controls inference via a host-based llama-server and spawns isolated Lima VMs for agent execution. Agents get full read-write access to your project workspace, but zero access to the host OS or SSH keys.

## Architecture

1. `llama-server` on macOS, accelerated via Metal
2. `limactl` VMs provisioned per-project
3. `opencode` inside guest VMs, connecting over `host.lima.internal`

## Prerequisites

macOS (Apple Silicon, ≥48GB RAM for 35B models), [Lima](https://github.com/lima-vm/lima), [llama.cpp](https://github.com/ggml-org/llama.cpp)

> [!NOTE]
> The ≥48GB RAM requirement applies to 35B models. Smaller models run on machines with less memory.

## Usage

```bash
muthr init               # Clone configs from tappunk/muthr-configs
muthr download <source>  # Fetch GGUF model from HuggingFace

muthr serve              # Start llama-server as a background daemon
muthr serve --foreground # Run in foreground
muthr status             # Check engine status and active profile
muthr stop               # Stop the engine

muthr up                 # Provision a Debian 13 VM for the current project
muthr ls                 # List all active sandboxes
muthr down               # Stop the current sandbox

muthr services start     # Launch MCP services VM
muthr services status
muthr services stop
```

## Configuration

Config in `~/.config/muthr/`:

- `llama/presets/*.ini` — context sizes, threading, model paths
- `lima/templates/*.yaml` — VM architecture, memory, container configs
- `lima/provision/*.sh` — boot scripts for OpenCode CLI and dependencies

Runtime state (PID files, logs, generated JSON) in `~/.cache/muthr/`.

## Installation

muthr is available on [crates.io](https://crates.io/crates/muthr) and [Homebrew](https://brew.sh/).

### Cargo

```bash
cargo install muthr
```

### Homebrew

```bash
brew install tappunk/muthr/muthr
```
