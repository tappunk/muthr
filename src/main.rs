pub mod config;
pub mod download;
pub mod engine;
pub mod model;
pub mod preset;
pub mod sandbox;
pub mod services;
pub mod system;
pub mod theme;
pub mod ui;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

#[derive(Parser)]
#[command(name = "muthr")]
#[command(about = "Local AI Workspace")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start llama-server inference engine
    Serve {
        #[arg(short, long)]
        profile: Option<String>,
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
        #[arg(
            long,
            help = "Run in foreground (blocking) instead of background daemon"
        )]
        foreground: bool,
    },

    /// Stop llama-server inference engine
    Stop,

    /// Show active engine status and profile info
    Status,

    /// List available preset profiles with config status
    List,

    /// Launch sandboxed opencode session
    Up {
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },

    /// Stop current sandbox VM
    Down,

    /// List all sandbox VMs
    Ls,

    /// Manage MCP services VM
    Services {
        #[command(subcommand)]
        action: ServicesCommands,
    },

    /// Download GGUF models from HuggingFace
    Download {
        #[arg(help = "HuggingFace repo or URL")]
        source: String,
        #[arg(help = "GGUF filename (required for repo syntax)")]
        file: Option<String>,
    },

    /// Run darwin-rebuild + Neovim sync + system clean
    Rebase {
        #[arg(long, help = "Skip dry-run preview and proceed directly")]
        yes: bool,
    },

    /// Remove .DS_Store files from workspace directories
    Clean,

    /// Generate shell completion file
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Browse and select a Ghostty theme
    Themes,
}

#[derive(Subcommand)]
pub enum ServicesCommands {
    /// Start MCP services VM
    Start,
    /// Stop MCP services VM
    Stop,
    /// Show MCP services VM status
    Status,
    /// Restart MCP services VM
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
        Commands::Status => engine::status()?,
        Commands::Stop => engine::stop()?,
        Commands::List => engine::list()?,
        Commands::Up { port } => sandbox::up(port).await?,
        Commands::Down => sandbox::down().await?,
        Commands::Ls => sandbox::list().await?,
        Commands::Services { action } => services::run(action).await?,
        Commands::Download { source, file } => download::download(&source, file.as_deref())?,
        Commands::Rebase { yes } => system::rebase(yes)?,
        Commands::Clean => system::clean()?,
        Commands::Themes => theme::run()?,
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "muthr", &mut std::io::stdout());
        }
    }

    Ok(())
}