![muthr](https://raw.githubusercontent.com/tappunk/.github/refs/heads/main/assets/muthr.webp)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Crates.io Version](https://img.shields.io/crates/v/muthr?color=orange&cacheSeconds=3600)](https://crates.io/crates/muthr)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr?color=blue)](https://github.com/tappunk/muthr/releases)
[![X Follow](https://img.shields.io/twitter/follow/tappunk?style=social)](https://x.com/tappunk)

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
muthr                    # Show system status dashboard (default)
muthr --help             # List all subcommands
muthr init               # Clone specs from tappunk/muthr-specs
muthr download <source>  # Fetch GGUF model from HuggingFace

muthr serve              # Start llama-server as a background daemon
muthr serve --foreground # Run in foreground
muthr stop               # Stop the engine
muthr list               # List available preset profiles

muthr up                 # Provision a Debian 13 VM for the current project
muthr ls                 # List all managed sandbox VMs
muthr down               # Stop the current sandbox
muthr delete             # Delete the active sandbox VM

muthr services start     # Launch muthr-services VM
muthr services status
muthr services stop
muthr services restart   # Restart the muthr-services VM
muthr services delete    # Delete the muthr-services VM

muthr boot               # Full stack startup: inference engine + muthr-services VM
muthr shutdown           # Graceful shutdown of all owned components

muthr config init        # Create muthr.toml config file
muthr config show        # Show resolved configuration
```

## Configuration

Config in `~/.config/muthr/` (see [muthr-specs](https://github.com/tappunk/muthr-specs) for the full directory structure and examples):

- `provider.d/llama-cpp/*.ini` — presets (context sizes, threading, model paths)
- `clients/opencode-config.json` — template for OpenCode runtime config generation
- `manifests/*.yaml` — VM architecture, memory, container configs
- `provision.d/*.sh` — boot scripts for OpenCode CLI and dependencies

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
