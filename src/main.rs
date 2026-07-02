// Copyright 2026 tappunk
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod catalog;
pub mod config;
pub mod doctor;
pub mod engine;
pub mod init;
pub mod lifecycle;
pub mod model;
pub mod sandbox;
pub mod services;
pub mod shutdown;

use clap::{ArgAction, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use serde::Serialize;

use crate::config::ConfigCommands;

#[derive(Parser)]
#[command(
    name = "muthr",
    version,
    author,
    about = "Manage inference and sandbox containers for local AI development",
    long_about = "Zero-trust orchestrator for MLX inference, container-based sandboxes, and MCP services on Apple Silicon.",
    arg_required_else_help = false,
    propagate_version = true,
    trailing_var_arg = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(ValueEnum, Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Json,
    Ndjson,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Manage inference engine")]
    Engine {
        #[command(subcommand)]
        action: EngineCommands,
    },

    #[command(about = "Manage project sandbox containers")]
    Sandbox {
        #[command(subcommand)]
        action: SandboxCommands,
    },

    #[command(about = "Manage persistent muthr-services container")]
    Services {
        #[command(subcommand)]
        action: ServicesCommands,
    },

    #[command(about = "Start inference engine and muthr-services container")]
    Run {
        #[arg(long, help = "Show detailed progress output during boot")]
        verbose: bool,
        #[arg(long, help = "Model repo ID to use (defaults to config)")]
        profile: Option<String>,
        #[arg(long, help = "Inference engine runtime (supported: mlxcel, llama)")]
        runtime: Option<String>,
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },

    #[command(about = "Shutdown all managed components")]
    Shutdown {
        #[arg(long, help = "Show detailed progress output during shutdown")]
        verbose: bool,
        #[arg(
            long,
            value_name = "SECONDS",
            help = "Timeout per component in seconds (default: 30)"
        )]
        timeout: Option<u64>,
        #[arg(short, long, help = "Skip confirmation prompts")]
        yes: bool,
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },

    #[command(about = "Generate shell completion scripts")]
    Completion {
        #[arg(
            value_enum,
            help = "Target shell environment for completion generation"
        )]
        shell: Shell,
    },

    #[command(about = "Initialize muthr from upstream specs")]
    Init {
        #[arg(
            long,
            help = "Custom Git URL for muthr-specs repository source override"
        )]
        git_url: Option<String>,
        #[arg(
            long,
            help = "Force overwrite existing configurations inside ~/.config/muthr/"
        )]
        force: bool,
    },

    #[command(about = "Manage muthr config")]
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },

    #[command(about = "Run diagnostics and health checks")]
    Doctor,

    #[command(about = "Manage pre-baked sandbox golden images")]
    Image {
        #[command(subcommand)]
        action: ImageCommands,
    },
}

#[derive(Subcommand)]
pub enum ImageCommands {
    #[command(about = "Build a golden image from a provision profile")]
    Build {
        #[arg(long, help = "Profile to pre-bake into a local image")]
        profile: String,
    },
}

#[derive(Subcommand)]
pub enum ServicesCommands {
    #[command(about = "Start muthr-services container")]
    Start {
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "Stop muthr-services container")]
    Stop {
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "Show muthr-services container status")]
    Status {
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
    },
    #[command(about = "Restart muthr-services container")]
    Restart {
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "Delete muthr-services container")]
    Delete {
        #[arg(long, help = "Skip confirmation prompt")]
        force: bool,
        #[arg(short, long, help = "Skip confirmation prompts")]
        yes: bool,
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum EngineCommands {
    #[command(about = "Start inference engine runtime")]
    Start {
        #[arg(long, help = "Inference engine runtime (supported: mlxcel, llama)")]
        runtime: Option<String>,
        #[arg(long, help = "Model repo ID to load")]
        profile: Option<String>,
        #[arg(
            long,
            help = "Bind host for inference server (e.g. 127.0.0.1 or 0.0.0.0)"
        )]
        bind_host: Option<String>,
        #[arg(
            long,
            help = "Port to bind the inference engine server (default from muthr.toml or 8080)"
        )]
        engine_server_port: Option<u16>,
        #[arg(
            long,
            help = "Run in foreground (blocking mode) instead of as a background daemon"
        )]
        foreground: bool,
    },
    #[command(about = "Stop inference engine runtime")]
    Stop {
        #[arg(long, help = "Inference engine runtime (supported: mlxcel, llama)")]
        runtime: Option<String>,
        #[arg(long, help = "Stop all running engines")]
        all: bool,
    },
    #[command(about = "Show engine status")]
    Status {
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
    },
    #[command(about = "List configured model profiles")]
    Presets {
        #[arg(long, help = "Inference engine runtime (supported: mlxcel, llama)")]
        runtime: Option<String>,
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
    },
}

#[derive(Subcommand)]
pub enum SandboxCommands {
    #[command(about = "Start sandbox container for current project")]
    Start {
        #[arg(
            long,
            help = "Profile to apply (run without --profile to list available profiles)!"
        )]
        profile: Option<String>,
        #[arg(long, help = "Write session audit logs to this NDJSON file path")]
        audit_log: Option<String>,
    },
    #[command(
        about = "Execute an interactive shell or a custom command inside the project sandbox"
    )]
    Shell {
        #[arg(long, help = "Ensure this profile is applied before attaching")]
        profile: Option<String>,
        #[arg(
            short,
            long,
            help = "Execute a non-interactive command instead of opening a login shell"
        )]
        command: Option<String>,
        #[arg(long, help = "Bypass TTY requirements for non-interactive automation")]
        no_tty: bool,
        #[arg(
            short,
            long,
            action = ArgAction::Append,
            help = "Explicit environment additions in KEY=VALUE form"
        )]
        env: Vec<String>,
        #[arg(long, help = "Write session audit logs to this NDJSON file path")]
        audit_log: Option<String>,
    },
    #[command(about = "Stop active sandbox container, selected sandboxes, or all sandboxes")]
    Stop {
        #[arg(long, help = "Stop all managed project sandbox containers")]
        all: bool,
        #[arg(
            long,
            action = ArgAction::Append,
            help = "Stop a specific sandbox container by name (repeatable)"
        )]
        name: Vec<String>,
    },
    #[command(about = "Delete active sandbox container")]
    Delete {
        #[arg(long, help = "Skip confirmation prompt")]
        force: bool,
        #[arg(short, long, help = "Skip confirmation prompts")]
        yes: bool,
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "List sandbox containers")]
    Ls {
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
    },
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    run().await
}

async fn boot(
    verbose: bool,
    profile: Option<String>,
    runtime: Option<String>,
) -> Result<(), color_eyre::Report> {
    sandbox::cleanup_untracked_vms(verbose).await?;

    let cfg = config::load()?;
    let engine_name = engine::resolve_runtime_for_profile(
        runtime,
        cfg.default_engine_runtime.clone(),
        profile.as_deref(),
    )?;
    let server_port = cfg.server_port.unwrap_or(8080);

    if engine::is_running().await {
        eprintln!("info: engine already running");
    } else {
        engine::start(&engine_name, profile, server_port, None, false).await?;
    }

    if verbose {
        eprintln!("info: ensuring muthr-services is running");
    }
    services::start(false).await?;

    Ok(())
}

fn resolve_runtime(
    runtime_flag: Option<String>,
    default_engine_runtime: Option<String>,
    profile: Option<&str>,
) -> Result<String, color_eyre::Report> {
    engine::resolve_runtime_for_profile(runtime_flag, default_engine_runtime, profile)
}

async fn run() -> Result<(), color_eyre::Report> {
    let cli = Cli::parse();

    match cli.command {
        None => {
            let cfg = config::load()?;
            let _ = resolve_runtime(None, cfg.default_engine_runtime.clone(), None)?;
            engine::status(OutputFormat::Text).await?
        }
        Some(Commands::Engine { action }) => match action {
            EngineCommands::Start {
                runtime,
                profile,
                bind_host,
                engine_server_port,
                foreground,
            } => {
                let cfg = config::load()?;
                let engine_name = resolve_runtime(
                    runtime,
                    cfg.default_engine_runtime.clone(),
                    profile.as_deref(),
                )?;
                let server_port = engine_server_port.unwrap_or(cfg.server_port.unwrap_or(8080));
                engine::start(&engine_name, profile, server_port, bind_host, foreground).await?
            }
            EngineCommands::Status { output } => {
                let cfg = config::load()?;
                let _ = resolve_runtime(None, cfg.default_engine_runtime.clone(), None)?;
                engine::status(output).await?
            }
            EngineCommands::Stop { runtime, all } => {
                if all {
                    engine::stop_all().await?;
                } else {
                    let cfg = config::load()?;
                    let engine_name =
                        resolve_runtime(runtime, cfg.default_engine_runtime.clone(), None)?;
                    engine::stop(&engine_name).await?;
                }
            }
            EngineCommands::Presets { runtime, output } => {
                let cfg = config::load()?;
                let engine_name =
                    resolve_runtime(runtime, cfg.default_engine_runtime.clone(), None)?;
                engine::presets_for_runtime(&engine_name, output)?;
            }
        },
        Some(Commands::Sandbox { action }) => match action {
            SandboxCommands::Start { profile, audit_log } => {
                let home = std::env::var("HOME")?;
                let config_dir = std::path::PathBuf::from(&home).join(".config/muthr");
                let cfg = config::load()?;
                let default_profile = cfg
                    .default_provision_profile
                    .unwrap_or_else(|| "opencode".to_string());

                let profiles = catalog::list_profiles(&config_dir)?;
                let all_profiles: Vec<String> = std::iter::once("base".to_string())
                    .chain(profiles.iter().map(|p| p.name.clone()))
                    .collect();

                let (container_id, _, _) = sandbox::resolve_workspace_context()?;
                let sandbox_exists =
                    !container_id.is_empty() && sandbox::sandbox_exists(&container_id).await;

                let profile_name = match profile {
                    Some(p) => p,
                    None if sandbox_exists
                        && !all_profiles.iter().any(|name| name == &default_profile) =>
                    {
                        "base".to_string()
                    }
                    None if all_profiles.iter().any(|name| name == &default_profile) => {
                        default_profile
                    }
                    None if sandbox_exists => "base".to_string(),
                    None => {
                        let mut items: Vec<String> = Vec::new();
                        for name in &all_profiles {
                            items.push(format!("  {}) {}", items.len() + 1, name));
                        }
                        for item in &items {
                            eprintln!("{}", item);
                        }
                        eprint!("choice ({}-{}) or q: ", 1, items.len());
                        std::io::Write::flush(&mut std::io::stderr()).ok();

                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input).ok();
                        let trimmed = input.trim();

                        if trimmed == "q" || trimmed.is_empty() {
                            return Ok(());
                        }

                        match trimmed.parse::<usize>() {
                            Ok(n) if n > 0 && n <= items.len() => {
                                let idx = n - 1;
                                all_profiles[idx].clone()
                            }
                            _ => {
                                return Ok(());
                            }
                        }
                    }
                };

                sandbox::start(profile_name, audit_log).await?
            }
            SandboxCommands::Shell {
                profile,
                command,
                no_tty,
                env,
                audit_log,
            } => sandbox::shell(profile, command, no_tty, env, audit_log).await?,
            SandboxCommands::Stop { all, name } => {
                if all && !name.is_empty() {
                    return Err(color_eyre::eyre::eyre!(
                        "--all cannot be combined with --name"
                    ));
                }
                sandbox::stop(name, all).await?
            }
            SandboxCommands::Delete {
                force,
                yes,
                dry_run,
            } => {
                if dry_run {
                    eprintln!("info: dry run, skipping sandbox deletion");
                    return Ok(());
                }
                let (container_id, _, _) = sandbox::resolve_workspace_context()?;
                if container_id.is_empty() || container_id == "muthr-config" {
                    eprintln!("error: must be inside a project directory");
                    std::process::exit(64);
                }
                sandbox::delete_sandbox(&container_id, force || yes).await?
            }
            SandboxCommands::Ls { output } => sandbox::ls(output).await?,
        },
        Some(Commands::Services { action }) => services::run(action).await?,
        Some(Commands::Run {
            verbose,
            profile,
            runtime,
            dry_run,
        }) => {
            if dry_run {
                eprintln!("info: dry run, skipping run actions");
                return Ok(());
            }
            boot(verbose, profile, runtime).await?
        }
        Some(Commands::Shutdown {
            verbose,
            timeout,
            yes,
            dry_run,
        }) => {
            shutdown::run(verbose, timeout, yes, dry_run).await?;
        }
        Some(Commands::Init { git_url, force }) => {
            tokio::task::spawn_blocking(move || init::run(init::InitCommands { git_url, force }))
                .await
                .map_err(|e| color_eyre::eyre::eyre!("init task failed: {}", e))??
        }
        Some(Commands::Config { action }) => match action {
            ConfigCommands::Init { force } => config::init_config(force)?,
            ConfigCommands::Show => {
                let cfg = config::load()?;
                cfg.print_resolved();
            }
        },
        Some(Commands::Doctor) => {
            doctor::run().await?;
        }
        Some(Commands::Image { action }) => match action {
            ImageCommands::Build { profile } => sandbox::build_golden_image(profile).await?,
        },
        Some(Commands::Completion { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "muthr", &mut std::io::stdout());
        }
    }

    Ok(())
}
