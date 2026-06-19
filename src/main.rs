pub mod config;
pub mod download;
pub mod engine;
pub mod init;
pub mod model;
pub mod preset;
pub mod sandbox;
pub mod services;
pub mod theme;
pub mod ui;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

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
            default_value_t = 8080,
            help = "Port to bind the inference engine server"
        )]
        port: u16,
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
            default_value_t = 8080,
            help = "Port where the inference engine is reachable"
        )]
        port: u16,
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

async fn run() -> Result<(), color_eyre::Report> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            profile,
            port,
            foreground,
        } => engine::serve(profile, port, foreground).await?,
        Commands::Status => engine::status().await?,
        Commands::Stop => engine::stop().await?,
        Commands::List => engine::list()?,
        Commands::Up { port } => sandbox::up(port).await?,
        Commands::Down => sandbox::down().await?,
        Commands::Ls => sandbox::list().await?,
        Commands::Services { action } => services::run(action).await?,
        Commands::Download { source, file } => download::download(&source, file.as_deref()).await?,
        Commands::Themes => theme::run()?,
        Commands::Init { git_url, force } => init::run(init::InitCommands { git_url, force })?,
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "muthr", &mut std::io::stdout());
        }
    }

    Ok(())
}
