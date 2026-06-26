pub mod catalog;
pub mod config;
pub mod download;
pub mod engine;
pub mod init;
pub mod model;
pub mod preset;
pub mod sandbox;
pub mod services;
pub mod shutdown;
pub mod ui;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use serde::Serialize;
use tokio::process::Command as AsyncCommand;

use crate::config::ConfigCommands;

#[derive(Parser)]
#[command(
    name = "muthr",
    version,
    author,
    about = "Manage llama.cpp inference and Lima sandbox vms for local ai development",
    long_about = "Zero-trust orchestrator for llama.cpp inference and Lima sandbox vms.\nPrerequisites: macOS arm64, Lima, llama.cpp",
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
    #[command(about = "Manage llama.cpp inference engine")]
    Engine {
        #[command(subcommand)]
        action: EngineCommands,
    },

    #[command(about = "Manage project sandbox vms")]
    Sandbox {
        #[command(subcommand)]
        action: SandboxCommands,
    },

    #[command(about = "Manage persistent muthr-services vm")]
    Services {
        #[command(subcommand)]
        action: ServicesCommands,
    },

    #[command(about = "Start inference engine and muthr-services vm")]
    Run {
        #[arg(long, help = "Show detailed progress output during boot")]
        verbose: bool,
        #[arg(short, long, help = "Skip confirmation prompts")]
        yes: bool,
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

    #[command(about = "Download gguf model from huggingface")]
    Download {
        #[arg(help = "Hugging Face repository (repo/name) or explicit resolve URL")]
        source: String,
        #[arg(help = "Target GGUF filename (required when using repository syntax)")]
        file: Option<String>,
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
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
}

#[derive(Subcommand)]
pub enum ServicesCommands {
    #[command(about = "Start muthr-services vm")]
    Start {
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "Stop muthr-services vm")]
    Stop {
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "Show muthr-services vm status")]
    Status {
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
    },
    #[command(about = "Restart muthr-services vm")]
    Restart {
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "Delete muthr-services vm")]
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
    #[command(about = "Start llama.cpp inference engine")]
    Start {
        #[arg(long, help = "Name of the target preset profile to load")]
        profile: Option<String>,
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
    #[command(about = "Stop inference engine")]
    Stop,
    #[command(about = "Show engine status")]
    Status {
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
    },
    #[command(about = "List preset profiles")]
    Presets {
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        output: OutputFormat,
    },
}

#[derive(Subcommand)]
pub enum SandboxCommands {
    #[command(about = "Start sandbox vm for current project")]
    Start {
        #[arg(
            long,
            help = "Profile to apply (base, opencode, hermes-agent; listed from provision.d/)"
        )]
        profile: Option<String>,
    },
    #[command(about = "Stop active sandbox vm")]
    Stop,
    #[command(about = "Delete active sandbox vm")]
    Delete {
        #[arg(long, help = "Skip confirmation prompt")]
        force: bool,
        #[arg(short, long, help = "Skip confirmation prompts")]
        yes: bool,
        #[arg(short = 'n', long, help = "Preview actions without side effects")]
        dry_run: bool,
    },
    #[command(about = "List sandbox vms")]
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

async fn boot(verbose: bool) -> Result<(), color_eyre::Report> {
    sandbox::cleanup_untracked_vms(verbose).await?;

    if engine::is_running().await {
        eprintln!("info: engine already running");
    } else {
        if verbose {
            eprintln!("info: starting inference engine");
        }
        let cfg = config::load()?;
        let server_port = cfg.server_port.unwrap_or(8080);
        engine::start(None, server_port, false).await?;
    }

    let vm_name = "muthr-services";
    let output = AsyncCommand::new("limactl")
        .args(["ls", "-f", "{{.Status}}", vm_name])
        .output()
        .await
        .ok();
    let running = match output {
        Some(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim() == "Running"
        }
        _ => false,
    };

    if !running {
        if verbose {
            eprintln!("info: starting muthr-services vm");
        }
        services::start(false).await?;
    }

    Ok(())
}

async fn run() -> Result<(), color_eyre::Report> {
    let cli = Cli::parse();

    match cli.command {
        None => engine::status(OutputFormat::Text).await?,
        Some(Commands::Engine { action }) => match action {
            EngineCommands::Start {
                profile,
                engine_server_port,
                foreground,
            } => {
                let cfg = config::load()?;
                let server_port = engine_server_port.unwrap_or(cfg.server_port.unwrap_or(8080));
                engine::start(profile, server_port, foreground).await?
            }
            EngineCommands::Status { output } => engine::status(output).await?,
            EngineCommands::Stop => engine::stop().await?,
            EngineCommands::Presets { output } => engine::presets(output)?,
        },
        Some(Commands::Sandbox { action }) => match action {
            SandboxCommands::Start { profile } => {
                let home = std::env::var("HOME")?;
                let config_dir = std::path::PathBuf::from(&home).join(".config/muthr");

                let profiles = catalog::list_profiles(&config_dir)?;
                let all_profiles: Vec<String> = std::iter::once("base".to_string())
                    .chain(profiles.iter().map(|p| p.name.clone()))
                    .collect();

                let (vm_name, _, _) = sandbox::resolve_workspace_context()?;
                let vm_exists = !vm_name.is_empty() && sandbox::vm_exists(&vm_name).await;

                let profile_name = match profile {
                    Some(p) => p,
                    None if vm_exists => "base".to_string(),
                    None => {
                        let mut items: Vec<String> = Vec::new();
                        for name in &all_profiles {
                            let desc = match name.as_str() {
                                "base" => "Minimal VM — drops into shell",
                                "opencode" => "Opencode AI — fully configured with muthr-services",
                                "hermes-agent" => "Hermes Agent — drops into shell after install",
                                _ => "",
                            };
                            items.push(format!("  {}) {:<15} {}", items.len() + 1, name, desc));
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

                sandbox::start(profile_name).await?
            }
            SandboxCommands::Stop => sandbox::stop().await?,
            SandboxCommands::Delete {
                force,
                yes,
                dry_run,
            } => {
                if dry_run {
                    eprintln!("info: dry run, skipping sandbox deletion");
                    return Ok(());
                }
                let (vm_name, _, _) = sandbox::resolve_workspace_context()?;
                if vm_name.is_empty() || vm_name == "muthr-config" {
                    eprintln!("error: must be inside a project directory");
                    std::process::exit(64);
                }
                sandbox::delete_vm(&vm_name, force || yes).await?
            }
            SandboxCommands::Ls { output } => sandbox::ls(output).await?,
        },
        Some(Commands::Services { action }) => services::run(action).await?,
        Some(Commands::Run {
            verbose,
            yes: _,
            dry_run,
        }) => {
            if dry_run {
                eprintln!("info: dry run, skipping run actions");
                return Ok(());
            }
            boot(verbose).await?
        }
        Some(Commands::Shutdown {
            verbose,
            timeout,
            yes,
            dry_run,
        }) => {
            shutdown::run(verbose, timeout, yes, dry_run).await;
        }
        Some(Commands::Download {
            source,
            file,
            output,
        }) => download::download(&source, file.as_deref(), output).await?,
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
        Some(Commands::Completion { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "muthr", &mut std::io::stdout());
        }
    }

    Ok(())
}
