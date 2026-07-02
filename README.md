![muthr](https://raw.githubusercontent.com/tappunk/.github/refs/heads/main/assets/muthr.webp)

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Crates.io Version](https://img.shields.io/crates/v/muthr?color=orange&cacheSeconds=3600)](https://crates.io/crates/muthr)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr?color=blue)](https://github.com/tappunk/muthr/releases)
[![X Follow](https://img.shields.io/twitter/follow/tappunk?style=social)](https://x.com/tappunk)

# muthr
> \[!NOTE]
> Stable core with active feature development. Review release notes before production rollout.

**Zero-trust orchestrator for local inference, container-based sandboxes, and MCP services on Apple Silicon.**

Canonical documentation: **https://tappunk.com/muthr/**

[Installation](#installation) · [Quick Start](#quick-start) · [Usage](#usage) · [Architecture](#architecture) · [MCP Compatibility](#mcp-compatibility) · [Configuration](#configuration)

## Features

- **Multi-runtime host inference** — manages `mlxcel-server` and `llama-server` lifecycle with OpenAI-compatible API surface
- **Advanced Apple container orchestration** — per-project sandbox containers with deterministic lifecycle control, profile-aware provisioning, and interactive shell execution
- **MCP services** — persistent services containers with MCP bridge and SearXNG for agent tool access
- **Zero-trust isolation** — agents get workspace access without host OS, SSH keys, or home directory exposure
- **Session audit trail** — optional NDJSON logs (`session_start`, `exec_invocation`, `session_exit`) for command-level forensics
- **Golden image workflow** — pre-bake profile images for restricted/air-gapped startup paths
- **Doctor diagnostics** — proactive backend checks, including arm64 buildkit/Rosetta limitation probing
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
muthr sandbox shell --profile opencode
```

## Usage

### Manage the inference engine

```bash
muthr engine start --profile mlx-community/Qwen3.5-9B-MLX-4bit
muthr engine start --runtime llama --profile ~/opt/models/unsloth/Qwen3.6-35B-A3B-GGUF/Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf
muthr engine status
muthr engine presets
muthr engine stop
```

### Provision sandbox containers

```bash
cd ~/src/myproject
muthr sandbox start --profile opencode
muthr sandbox shell
muthr sandbox shell --profile opencode
muthr sandbox shell --no-tty --command "cd /workspace && cargo test"
muthr sandbox shell --audit-log ~/.cache/muthr/audit/my-session.ndjson
muthr sandbox ls
muthr sandbox stop
muthr sandbox stop --name muthr-myproject
muthr sandbox stop --all
muthr sandbox delete --yes

# Golden image build for air-gapped restricted profiles
muthr image build --profile hermes-agent
```

### Sandbox control model

- `muthr sandbox start` is lifecycle-oriented and idempotent for provisioning/bootstrapping a project sandbox.
- `muthr sandbox shell` is the interactive entry point with TTY detection, terminal-state restore, and clean exit-code propagation.
- Window resize events (`SIGWINCH`) are forwarded so full-screen tools (`vim`, pagers, REPLs) keep correct PTY geometry.
- Environment propagation is allowlist-based (`TERM`, `COLORTERM`, `COLUMNS`, `LINES`) to avoid locale drift and noisy guest warnings.
- Managed sandbox stop modes support current project (`stop`), named containers (`stop --name ...`), and fleet stop (`stop --all`).

### Sandbox shell troubleshooting

- Terminal resize: `muthr sandbox shell` forwards host `SIGWINCH` events to the container PTY. If wrapping still looks wrong after a terminal multiplexer change, run `reset` inside the shell.
- File ownership mapping: muthr attempts best-effort UID/GID synchronization for the `muthr` user in containers. If host permission drift appears after manual container mutations, restart the sandbox with `muthr sandbox stop` then `muthr sandbox shell`.
- Non-interactive automation: use `muthr sandbox shell --no-tty --command "<cmd>"` for CI/script execution without a TTY.
- Audit logs: use `--audit-log <path>` on `sandbox shell` or `sandbox start` to write NDJSON session events (`session_start`, `exec_invocation`, `session_exit`).
- Golden images: run `muthr image build --profile <name>` to pre-bake provisioned profile images and allow create-time `--network none` startup without a WAN bootstrap window.

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
muthr doctor
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
│  │ or llama-server       │                    │                │
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

muthr orchestrates three layers: host inference (`mlxcel-server` or `llama-server`), persistent services containers, and per-project sandbox containers for agent execution.

## Engine Runtime

muthr supports `mlxcel` and `llama` runtimes.

Runtime selection precedence (highest to lowest):

1. CLI flag (`--runtime` on `muthr run`, `mlxcel` or `llama`)
2. Environment variable (`MUTHR_ENGINE_RUNTIME`, `mlxcel` or `llama`)
3. Config value (`default_engine_runtime` in `muthr.toml`, `mlxcel` or `llama`)
4. Built-in fallback (`mlxcel`)

`muthr engine start` and `muthr engine stop` accept `--runtime`.

Engine bind host precedence (highest to lowest):

1. CLI flag (`muthr engine start --bind-host <host>`)
2. Environment variable (`MUTHR_ENGINE_BIND_HOST`)
3. Config value (`default_engine_bind_host` in `muthr.toml`)
4. Built-in fallback (`0.0.0.0`)

Use `127.0.0.1` for host-only inference or `0.0.0.0` when sandbox profiles (for example `opencode` and `hermes-agent`) must reach the engine through the host gateway.

## Provider Presets

`muthr` can resolve engine model profiles from INI files under `~/.config/muthr/provider.d/`.

- file extension: `.ini` (searched recursively)
- preset name: filename without `.ini`
- keys:
  - `model` (or `model_id` / `profile`) — required
  - `runtime` (or `engine_runtime`) — optional (`mlxcel` or `llama`)

Runtime selection with presets:

1. explicit `--runtime` flag
2. preset-declared runtime (when `--profile` matches a provider preset file)
3. config/env runtime (`default_engine_runtime` / `MUTHR_ENGINE_RUNTIME`)
4. fallback `mlxcel`

Example preset:

```ini
runtime = llama
model = /Users/user/opt/models/unsloth/Qwen3.6-35B-A3B-GGUF/Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf
```

## MCP Compatibility

`muthr-services` provides the persistent MCP + SearXNG integration layer used by sandboxed agents.

- SearXNG endpoint (host): `http://127.0.0.1:18766`
- MCP access: stdio bridge through `muthr-services` container

## Configuration

muthr stores config in `~/.config/muthr/` and runtime state in `~/.cache/muthr/`.

Configuration files:

- `~/.config/muthr/muthr.toml` — server port, workspace root, model dir, default provision profile, engine runtime, default engine profile, default engine bind host
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
MUTHR_ENGINE_BIND_HOST
MUTHR_CONTAINER_HOST_GATEWAY
```

Workspace safety:

- Set `MUTHR_WORKSPACE_ROOT` to a dedicated subdirectory (for example, `~/src`), never to your home directory.
- If workspace root resolves to `$HOME`, `muthr` exits with a security error to prevent mounting your full home into sandbox containers.

Profile system:

- `base` — minimal Debian 13 container with shell access
- `opencode` — opencode setup with MCP integration
- `hermes-agent` — isolated Hermes-Agent runtime (uv + Python environment)

All profile assets are managed via [muthr-specs](https://github.com/tappunk/muthr-specs). Run `muthr init` to refresh local config.

## Acknowledgements

- [llama.cpp](https://github.com/ggml-org/llama.cpp)
- [mlxcel](https://github.com/lablup/mlxcel)
- [Apple container](https://github.com/apple/container)
