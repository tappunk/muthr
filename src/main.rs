pub mod config;
pub mod download;
pub mod engine;
pub mod init;
pub mod model;
pub mod preset;
pub mod runtime_config;
pub mod sandbox;
pub mod services;
pub mod shutdown;
pub mod theme;
pub mod ui;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use tokio::process::Command as AsyncCommand;

use crate::config::ConfigCommands;
use crate::sandbox::ProvisionProfile;

#[derive(Parser)]
#[command(
    name = "muthr",
    version,
    author,
    about = "Manage llama.cpp inference and Lima sandbox VMs for local AI development.",
    long_about = "muthr automates llama.cpp on macOS for local inference and manages isolated Lima sandbox VMs for running AI agents with safe access to your workspace.\n\nPrerequisites: macOS (Apple Silicon), Lima VM, and llama.cpp",
    arg_required_else_help = true,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Start the llama-server inference engine")]
    Serve {
        #[arg(long, help = "Name of the target preset profile to load")]
        profile: Option<String>,
        #[arg(
            short,
            long,
            help = "Port to bind the inference engine server (default from muthr.toml or 8080)"
        )]
        port: Option<u16>,
        #[arg(
            long,
            help = "Run in foreground (blocking mode) instead of as a background daemon"
        )]
        foreground: bool,
    },

    #[command(about = "Stop the background llama-server daemon")]
    Stop,

    #[command(about = "Show the active profile and server status")]
    Status,

    #[command(about = "List available preset profiles")]
    List,

    #[command(about = "Provision and start a Lima sandbox VM for the current project")]
    Up {
        #[arg(
            short,
            long,
            help = "Port where the inference engine is reachable (default from muthr.toml or 8080)"
        )]
        port: Option<u16>,
        #[arg(
            long,
            value_enum,
            help = "Provisioning profile to apply (base = minimal, opencode = full toolchain; default from muthr.toml or base)"
        )]
        provision_profile: Option<ProvisionProfile>,
    },

    #[command(about = "Stop the active project sandbox VM")]
    Down,

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

    #[command(about = "Browse and select Ghostty terminal themes")]
    Themes,

    #[command(about = "Initialize muthr configurations from the upstream repository")]
    Init {
        #[arg(
            long,
            help = "Custom Git URL for muthr-configs repository source override"
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
        let server_port = cfg.port.unwrap_or(8080);
        engine::serve(None, server_port, false).await?;
    }

    let vm_name = "mcp-services-vm";
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
        Commands::Serve {
            profile,
            port,
            foreground,
        } => {
            let cfg = config::load()?;
            let server_port = port.unwrap_or(cfg.port.unwrap_or(8080));
            engine::serve(profile, server_port, foreground).await?
        }
        Commands::Status => engine::status().await?,
        Commands::Stop => engine::stop().await?,
        Commands::List => engine::list()?,
        Commands::Up {
            port,
            provision_profile,
        } => {
            let cfg = config::load()?;
            let up_port = port.unwrap_or(cfg.port.unwrap_or(8080));
            let up_profile = match (provision_profile, cfg.default_provision_profile.as_deref()) {
                (Some(p), _) => p,
                (None, Some("opencode")) => ProvisionProfile::Opencode,
                (None, None) => {
                    let options = vec![
                        crate::ui::ProvisionOption {
                            label: "base",
                            description: "Minimal VM provision — no extra tools installed",
                        },
                        crate::ui::ProvisionOption {
                            label: "opencode",
                            description: "Full toolchain with opencode binary and MCP support",
                        },
                    ];
                    match ui::select_provision_profile(&options) {
                        Some(0) => ProvisionProfile::Base,
                        Some(1) => ProvisionProfile::Opencode,
                        _ => {
                            println!("[INFO] Cancelled.");
                            return Ok(());
                        }
                    }
                }
                _ => ProvisionProfile::Base,
            };
            sandbox::up(up_port, up_profile).await?
        }
        Commands::Down => sandbox::down().await?,
        Commands::Ls => sandbox::list().await?,
        Commands::Services { action } => services::run(action).await?,
        Commands::Boot { verbose } => boot(verbose).await?,
        Commands::Shutdown { verbose, timeout } => {
            shutdown::run(verbose, timeout).await;
        }
        Commands::Download { source, file } => download::download(&source, file.as_deref()).await?,
        Commands::Themes => theme::run()?,
        Commands::Init { git_url, force } => init::run(init::InitCommands { git_url, force })?,
        Commands::Config { action } => match action {
            ConfigCommands::Init { force } => config::init_config(force)?,
            ConfigCommands::Show => {
                let cfg = config::load()?;
                cfg.print_resolved();
            }
        },
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "muthr", &mut std::io::stdout());
        }
    }

    Ok(())
}
