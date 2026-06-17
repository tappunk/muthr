# muthr

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr)](https://github.com/tappunk/muthr/releases)
[![Crates.io](https://img.shields.io/crates/v/muthr?color=orange)](https://crates.io/crates/muthr)

**muthr** runs autonomous AI agents inside isolated VMs on Apple Silicon. Running coding agents on your host exposes SSH keys and system root to unpredictable tool calls.

muthr runs inference on the host and execution inside sandboxes. Agents get full read-write access to the target project and strict read-only access to the host. Inference routes to a local `llama-server`. Sandboxes disable SSH agent forwarding.

## Usage

```bash
muthr serve              # Start llama-server as a background daemon
muthr serve --foreground # Run in foreground
muthr status             # Check engine status and active profile
muthr stop               # Stop the engine

muthr up    # Provision a Debian 13 VM for the current project
muthr ls    # List all active sandboxes
muthr down  # Stop the current sandbox

muthr services start     # Launch MCP services VM
muthr services status
muthr services stop
```

## Architecture

1. `llama-server` on macOS, accelerated via Metal
2. `limactl` VMs provisioned per-project
3. [OpenCode](https://opencode.ai) inside guest VMs, connecting over `host.lima.internal`

## Prerequisites

macOS (Apple Silicon, ≥48GB RAM for 35B models), [Lima](https://github.com/lima-vm/lima), [llama.cpp](https://github.com/ggml-org/llama.cpp)

## Configuration

Config in `~/.config/muthr/`:

- `llama/presets/*.ini` — context sizes, threading, model paths
- `lima/templates/*.yaml` — VM architecture, memory, container configs
- `lima/provision/*.sh` — boot scripts for OpenCode CLI and dependencies

Runtime state (PID files, logs, generated JSON) in `~/.cache/muthr/`.

## Installation

muthr is part of [tappunk/dotfiles](https://github.com/tappunk/dotfiles). Install it by following the [dotfiles instructions](https://github.com/tappunk/dotfiles).
