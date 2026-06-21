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

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use tokio::process::Command as AsyncCommand;

use crate::config::ConfigCommands;

#[derive(Parser)]
#[command(
    name = "muthr",
    version,
    author,
    about = "Manage llama.cpp inference and Lima sandbox VMs for local AI development.",
    long_about = "muthr automates llama.cpp on macOS for local inference and manages isolated Lima sandbox VMs for running AI agents with safe access to your workspace.\n\nPrerequisites: macOS (Apple Silicon), Lima VM, and llama.cpp",
    arg_required_else_help = false,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Start the llama-server inference engine")]
    Serve {
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

    #[command(about = "Stop the background llama-server daemon")]
    Stop,

    #[command(about = "Show system status (default)")]
    Status,

    #[command(about = "List available preset profiles")]
    List,

    #[command(about = "Provision and start a Lima sandbox VM for the current project")]
    Up {
        #[arg(
            long,
            help = "Profile to apply (base, opencode, hermes-agent; listed from provision.d/)"
        )]
        profile: Option<String>,
    },

    #[command(about = "Stop the active project sandbox VM")]
    Down,

    #[command(about = "Delete the active project sandbox VM")]
    Delete {
        #[arg(long, help = "Skip confirmation prompt")]
        force: bool,
    },

    #[command(about = "List all managed sandbox VMs")]
    Ls,

    #[command(about = "Manage the persistent MCP services VM")]
    Services {
        #[command(subcommand)]
        action: ServicesCommands,
    },

    #[command(about = "Full stack startup: inference engine + MCP services VM")]
    Boot {
        #[arg(long, help = "Show detailed progress output during boot")]
        verbose: bool,
    },

    #[command(
        about = "Graceful shutdown of all owned components: sandboxes, MCP services VM, and inference engine"
    )]
    Shutdown {
        #[arg(long, help = "Show detailed progress output during shutdown")]
        verbose: bool,
        #[arg(
            long,
            value_name = "SECONDS",
            help = "Timeout per component in seconds (default: 30)"
        )]
        timeout: Option<u64>,
    },

    #[command(about = "Download a GGUF model from Hugging Face")]
    Download {
        #[arg(help = "Hugging Face repository (repo/name) or explicit resolve URL")]
        source: String,
        #[arg(help = "Target GGUF filename (required when using repository syntax)")]
        file: Option<String>,
    },

    #[command(about = "Generate shell completion scripts")]
    Completion {
        #[arg(
            value_enum,
            help = "Target shell environment for completion generation"
        )]
        shell: Shell,
    },

    #[command(about = "Initialize muthr configurations from the upstream repository")]
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

    #[command(about = "Manage muthr configuration")]
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },
}

#[derive(Subcommand)]
pub enum ServicesCommands {
    #[command(about = "Start the MCP services VM")]
    Start,
    #[command(about = "Stop the MCP services VM")]
    Stop,
    #[command(about = "Show the MCP services VM execution status")]
    Status,
    #[command(about = "Restart the MCP services VM execution context")]
    Restart,
    #[command(about = "Delete the MCP services VM")]
    Delete {
        #[arg(long, help = "Skip confirmation prompt")]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    run().await
}

async fn boot(verbose: bool) -> Result<(), color_eyre::Report> {
    if engine::is_running().await {
        println!("[OK] Engine already running.");
    } else {
        if verbose {
            println!("[PROC] Starting inference engine...");
        }
        let cfg = config::load()?;
        let server_port = cfg.server_port.unwrap_or(8080);
        engine::serve(None, server_port, false).await?;
    }

    let vm_name = "muthr-services";
    let output = AsyncCommand::new("limactl")
        .args(["ls", "-f", "'{{.Status}}'", vm_name])
        .output()
        .await
        .ok();
    let running = match output {
        Some(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim() == "Running"
        }
        _ => false,
    };

    if running {
        println!("[OK] MCP services already running.");
    } else {
        if verbose {
            println!("[PROC] Starting MCP services VM...");
        }
        services::start().await?;
    }

    println!("[OK] Boot complete.");
    Ok(())
}

async fn run() -> Result<(), color_eyre::Report> {
    let cli = Cli::parse();

    match cli.command {
        None => engine::status().await?,
        Some(Commands::Serve {
            profile,
            engine_server_port,
            foreground,
        }) => {
            let cfg = config::load()?;
            let server_port = engine_server_port.unwrap_or(cfg.server_port.unwrap_or(8080));
            engine::serve(profile, server_port, foreground).await?
        }
        Some(Commands::Status) => engine::status().await?,
        Some(Commands::Stop) => engine::stop().await?,
        Some(Commands::List) => engine::list()?,
        Some(Commands::Up { profile }) => {
            let home = std::env::var("HOME")?;
            let config_dir = std::path::PathBuf::from(&home).join(".config/muthr");

            let profiles = catalog::list_profiles(&config_dir)?;
            let all_profiles: Vec<&str> = std::iter::once("base")
                .chain(profiles.iter().map(|p| p.name.as_str()))
                .collect();

            let (vm_name, _, _) = sandbox::resolve_workspace_context()?;
            let vm_exists = !vm_name.is_empty() && sandbox::vm_exists(&vm_name).await;

            let profile_name = match profile {
                Some(p) => p,
                None if vm_exists => {
                    println!(
                        "[INFO] Sandbox '{}' already exists — launching session.",
                        vm_name
                    );
                    "base".to_string()
                }
                None => {
                    println!("Select a provisioning profile:");
                    let mut items: Vec<String> = Vec::new();
                    for name in &all_profiles {
                        let desc = match *name {
                            "base" => "Minimal VM — drops into shell",
                            "opencode" => "Opencode AI — fully configured with MCP services",
                            "hermes-agent" => "Hermes Agent — drops into shell after install",
                            _ => "",
                        };
                        items.push(format!("  {}) {:<15} {}", items.len() + 1, name, desc));
                    }
                    println!();
                    for item in &items {
                        println!("{}", item);
                    }
                    print!("\nEnter choice ({}-{}) or q to quit: ", 1, items.len());
                    std::io::Write::flush(&mut std::io::stdout()).ok();

                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).ok();
                    let trimmed = input.trim();

                    if trimmed == "q" || trimmed.is_empty() {
                        println!("[INFO] Cancelled.");
                        return Ok(());
                    }

                    match trimmed.parse::<usize>() {
                        Ok(n) if n > 0 && n <= items.len() => {
                            let idx = n - 1;
                            all_profiles[idx].to_string()
                        }
                        _ => {
                            println!("[INFO] Invalid selection.");
                            return Ok(());
                        }
                    }
                }
            };

            sandbox::up(profile_name).await?
        }
        Some(Commands::Down) => sandbox::down().await?,
        Some(Commands::Delete { force }) => {
            let (vm_name, _, _) = sandbox::resolve_workspace_context()?;
            if vm_name.is_empty() || vm_name == "muthr-config" {
                eprintln!("Error: must be inside a project directory.");
                std::process::exit(1);
            }
            sandbox::delete_vm(&vm_name, force).await?
        }
        Some(Commands::Ls) => sandbox::list().await?,
        Some(Commands::Services { action }) => services::run(action).await?,
        Some(Commands::Boot { verbose }) => boot(verbose).await?,
        Some(Commands::Shutdown { verbose, timeout }) => {
            shutdown::run(verbose, timeout).await;
        }
        Some(Commands::Download { source, file }) => {
            download::download(&source, file.as_deref()).await?
        }
        Some(Commands::Init { git_url, force }) => {
            init::run(init::InitCommands { git_url, force })?
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
