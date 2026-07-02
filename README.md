<div align="center">
  <img src="https://raw.githubusercontent.com/tappunk/.github/refs/heads/main/assets/muthr.webp" alt="muthr" width="280"/>

# muthr

**Zero-trust orchestrator for local AI on Apple Silicon.**

Run inference engines on the host, isolate agent runtimes inside per-project containers, and expose stable MCP + search endpoints — without giving AI tools access to your host OS, SSH keys, or home directory.

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/crates/v/muthr?color=orange)](https://crates.io/crates/muthr)
[![GitHub Release](https://img.shields.io/github/v/release/tappunk/muthr)](https://github.com/tappunk/muthr/releases)
[![X Follow](https://img.shields.io/twitter/follow/tappunk?style=social)](https://x.com/tappunk)

[Quick Start](#quick-start) · [Features](#features) · [Architecture](#architecture) · [Commands](#commands) · [Configuration](#configuration) · [Full Docs](https://tappunk.com/muthr/)
</div>

---

## Quick Start

Three commands to a safe agent sandbox:

```bash
brew install tappunk/muthr/muthr    # or: cargo install muthr
muthr init                          # populate config from muthr-specs
muthr run                           # boot engine + services
```

Then enter a project sandbox:

```bash
cd ~/src/myproject
muthr sandbox shell --profile opencode
```

That's it. Inference runs on the host. Your agent runs isolated inside a container with only the project workspace mounted. Host secrets stay on the host.

## Features

- **Host inference management** — runs `mlxcel-server` or `llama-server` with an OpenAI-compatible API surface
- **Per-project sandbox containers** — deterministic lifecycle, profile-aware provisioning, interactive shell with TTY resize forwarding
- **Persistent MCP services** — MCP bridge + SearXNG search in a long-lived services container
- **Zero-trust isolation** — agents get workspace access only. No host OS, SSH keys, or home directory
- **Session audit trail** — optional NDJSON logs (`session_start`, `exec_invocation`, `session_exit`) for forensics
- **Golden images** — pre-bake profile images for restricted or air-gapped environments
- **Doctor diagnostics** — proactive backend checks including arm64 buildkit/Rosetta probing
- **Full lifecycle** — `muthr run` boots engine + services, `muthr shutdown` tears down everything
- **Machine-readable output** — `--output json|ndjson` on status/list commands
- **Shell completions** — bash, zsh, fish, powershell

## Architecture

```
┌──────────────────── macOS Host ──────────────────────┐
│                                                      │
│  ┌──────────────┐          ┌──────────────────┐      │
│  │ mlxcel-server│          │ muthr-services   │      │
│  │  or          │          │  MCP Bridge      │      │
│  │ llama-server │          │  SearXNG         │      │
│  └──────┬───────┘          └──────────────────┘      │
│         │                                            │
│  ┌──────┴───────┐                                    │
│  │ Sandboxes    │                                    │
│  │ (container)  │                                    │
│  └──────────────┘                                    │
│                                                      │
│  Agent access: workspace mount + OpenAI URL          │
│  Host secrets/SSH/home remain outside sandbox        │
│                                                      │
└──────────────────────────────────────────────────────┘
```

muthr orchestrates three layers:

1. **Host inference runtime** — `mlxcel-server` or `llama-server` managed by `muthr engine`
2. **Persistent services plane** — `muthr-services` + `muthr-searxng` managed by `muthr services`
3. **Per-project sandbox** — one container per project directory, managed by `muthr sandbox`

The default `muthr run` path boots engine + services, then you enter project sandboxes as needed.

## Usage

### Engine

```bash
muthr engine start --profile mlx-community/Qwen3.5-9B-MLX-4bit
muthr engine start --runtime llama --profile ~/opt/models/my-model.gguf
muthr engine status
muthr engine presets
muthr engine stop
```

### Sandbox

```bash
cd ~/src/myproject
muthr sandbox start --profile opencode
muthr sandbox shell                          # interactive shell
muthr sandbox shell --no-tty --command "cargo test"
muthr sandbox shell --audit-log ~/audit.ndjson
muthr sandbox ls
muthr sandbox stop --all
muthr sandbox delete --yes

# Golden image for air-gapped profiles
muthr image build --profile hermes-agent
```

### Services

```bash
muthr services start
muthr services status
muthr services restart
muthr services stop
```

### Full lifecycle

```bash
muthr run --verbose
muthr shutdown --yes
```

## Commands

| Command | Purpose |
|---|---|
| `muthr engine start/stop/status/presets` | Manage inference runtime |
| `muthr sandbox start/shell/stop/ls/delete` | Per-project sandbox lifecycle |
| `muthr services start/status/restart/stop` | Persistent MCP + SearXNG services |
| `muthr run` | Boot engine + services together |
| `muthr shutdown` | Tear down all managed components |
| `muthr init` | Populate `~/.config/muthr/` from specs |
| `muthr config init/show` | Create or inspect config |
| `muthr doctor` | Prerequisite diagnostics |
| `muthr image build` | Pre-bake golden profile images |
| `muthr completion <shell>` | Generate shell completions |

## Configuration

Config lives in `~/.config/muthr/muthr.toml`:

```toml
server_port = 8080
workspace_root = "~/src"
model_dir = "~/opt/models"
default_provision_profile = "opencode"
default_engine_runtime = "mlxcel"
default_engine_bind_host = "0.0.0.0"
```

Key environment overrides: `MUTHR_SERVER_PORT`, `MUTHR_WORKSPACE_ROOT`, `MUTHR_MODEL_DIR`, `MUTHR_ENGINE_RUNTIME`, `MUTHR_ENGINE_BIND_HOST`, `MUTHR_CONTAINER_HOST_GATEWAY`.

Profiles (`base`, `opencode`, `hermes-agent`) define container provisioning. Profile assets are managed via [muthr-specs](https://github.com/tappunk/muthr-specs).

## Security

muthr's design premise: running AI agents on your host is high-risk. Agents execute package installers, shell commands, and network clients with broad filesystem access.

muthr mitigates this by:

- Running agents inside sandbox containers, never on the host shell
- Mounting only the project workspace (`/workspace`) into containers
- Exposing inference endpoints via explicit env vars, not ambient host access
- Using [Apple container](https://github.com/apple/container) for native virtualization-backed isolation

See [Security](https://tappunk.com/muthr/security) for the full threat model.

## Why this exists

Running AI agents directly on your host is high-risk. A single compromised dependency, malicious prompt chain, or unsafe tool invocation can leak credentials or compromise the system.

muthr's goal is practical risk reduction with low operational friction: isolate agent execution into containers, preserve host-only assets, and keep everything observable and auditable.

If you like Unix design principles — clear subcommands, scriptable output, explicit defaults — muthr should feel familiar.

## Contributing

Issues and PRs welcome. See the full docs for development setup and architecture details.

## Acknowledgements

- [llama.cpp](https://github.com/ggml-org/llama.cpp)
- [mlxcel](https://github.com/lablup/mlxcel)
- [Apple container](https://github.com/apple/container)

---

**Full documentation:** https://tappunk.com/muthr/
