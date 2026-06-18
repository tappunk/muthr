pub mod config;
pub mod download;
pub mod engine;
pub mod init;
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
    #[command(about = "start the llama-server (background daemon or foreground)")]
    Serve {
        #[arg(long)]
        profile: Option<String>,
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
        #[arg(
            long,
            help = "run in foreground (blocking) instead of background daemon"
        )]
        foreground: bool,
    },

    #[command(about = "stop the running llama-server daemon")]
    Stop,

    #[command(about = "show active profile and server status")]
    Status,

    #[command(about = "list available preset profiles")]
    List,

    #[command(about = "provision and start a lima sandbox vm")]
    Up {
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },

    #[command(about = "stop the current sandbox vm")]
    Down,

    #[command(about = "list all sandbox vms")]
    Ls,

    #[command(about = "manage mcp services vm (start/stop/status/restart)")]
    Services {
        #[command(subcommand)]
        action: ServicesCommands,
    },

    #[command(about = "download a gguf model from huggingface")]
    Download {
        #[arg(help = "HuggingFace repo or URL")]
        source: String,
        #[arg(help = "GGUF filename (required for repo syntax)")]
        file: Option<String>,
    },

    #[command(about = "run darwin-rebuild and sync neovim configuration")]
    Rebase {
        #[arg(long, help = "skip dry-run preview and proceed directly")]
        yes: bool,
    },

    #[command(about = "remove .DS_Store files from the project")]
    Clean,

    #[command(about = "generate shell completion scripts")]
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },

    #[command(about = "browse and select ghostty themes")]
    Themes,

    #[command(about = "initialize muthr configs from muthr-configs repository")]
    Init {
        #[arg(
            long,
            help = "custom git URL for muthr-configs (default: tappunk/muthr-configs)"
        )]
        git_url: Option<String>,
        #[arg(long, help = "overwrite existing configs")]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum ServicesCommands {
    #[command(about = "start the MCP services VM")]
    Start,
    #[command(about = "stop the MCP services VM")]
    Stop,
    #[command(about = "show MCP services VM status")]
    Status,
    #[command(about = "restart the MCP services VM")]
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
        Commands::Init { git_url, force } => init::run(init::InitCommands { git_url, force })?,
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "muthr", &mut std::io::stdout());
        }
    }

    Ok(())
}
