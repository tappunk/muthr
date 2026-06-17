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
#[command(
    name = "muthr",
    version,
    author,
    about,
    arg_required_else_help = true,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Serve {
        #[arg(long)]
        profile: Option<String>,
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
        #[arg(
            long,
            help = "Run in foreground (blocking) instead of background daemon"
        )]
        foreground: bool,
    },

    Stop,

    Status,

    List,

    Up {
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },

    Down,

    Ls,

    Services {
        #[command(subcommand)]
        action: ServicesCommands,
    },

    Download {
        #[arg(help = "HuggingFace repo or URL")]
        source: String,
        #[arg(help = "GGUF filename (required for repo syntax)")]
        file: Option<String>,
    },

    Rebase {
        #[arg(long, help = "Skip dry-run preview and proceed directly")]
        yes: bool,
    },

    Clean,

    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },

    Themes,
}

#[derive(Subcommand)]
pub enum ServicesCommands {
    Start,
    Stop,
    Status,
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
        Commands::Rebase { yes } => system::rebase(yes).await?,
        Commands::Clean => system::clean().await?,
        Commands::Themes => theme::run()?,
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "muthr", &mut std::io::stdout());
        }
    }

    Ok(())
}
