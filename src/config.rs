use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Initialize a starter muthr.toml configuration file
    Init {
        #[arg(long, help = "Force overwrite existing muthr.toml")]
        force: bool,
    },
    /// Show the resolved configuration (TOML + env overrides + defaults)
    Show,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct MuthrConfig {
    pub server_port: Option<u16>,
    pub workspace_root: Option<String>,
    pub model_dir: Option<String>,
    pub default_provision_profile: Option<String>,
}

impl MuthrConfig {
    fn resolve(self) -> (u16, String, String, String) {
        let server_port = self.server_port.unwrap_or(8080);
        let workspace_root = match self.workspace_root {
            Some(v) => v,
            None => std::env::var("HOME")
                .ok()
                .map(|h| format!("{}/src", h))
                .unwrap_or_else(|| "~/src".to_string()),
        };
        let model_dir = match self.model_dir {
            Some(v) => v,
            None => std::env::var("HOME")
                .ok()
                .map(|h| format!("{}/opt/models", h))
                .unwrap_or_else(|| "~/opt/models".to_string()),
        };
        let provision_profile = self
            .default_provision_profile
            .unwrap_or_else(|| "base".to_string());
        (server_port, workspace_root, model_dir, provision_profile)
    }

    pub fn print_resolved(&self) {
        let (server_port, workspace_root, model_dir, provision_profile) = self.clone().resolve();
        println!("muthr configuration:");
        println!("  server_port:                   {}", server_port);
        println!("  workspace_root:                {}", workspace_root);
        println!("  model_dir:                     {}", model_dir);
        println!("  default_provision_profile:     {}", provision_profile);
    }
}

pub fn load() -> Result<MuthrConfig, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let config_path = PathBuf::from(&home).join(".config/muthr/muthr.toml");

    let mut config = if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        toml::from_str(&content)?
    } else {
        MuthrConfig::default()
    };

    if let Ok(v) = std::env::var("MUTHR_SERVER_PORT") {
        config.server_port = v.parse().ok();
    }
    if let Ok(v) = std::env::var("MUTHR_WORKSPACE_ROOT") {
        config.workspace_root = Some(v);
    }
    if let Ok(v) = std::env::var("MUTHR_MODEL_DIR") {
        config.model_dir = Some(v);
    }
    if let Ok(v) = std::env::var("MUTHR_PROVISION_PROFILE") {
        config.default_provision_profile = Some(v);
    }

    Ok(config)
}

pub fn init_config(force: bool) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let config_dir = PathBuf::from(&home).join(".config/muthr");
    let config_path = config_dir.join("muthr.toml");

    if config_path.exists() && !force {
        println!("[INFO] muthr.toml already exists at {:?}", config_path);
        println!("[INFO] Use --force to overwrite.");
        return Ok(());
    }

    fs::create_dir_all(&config_dir)?;

    let template = r##"# muthr configuration
server_port = 8080
workspace_root = "~/src"
model_dir = "~/opt/models"
default_provision_profile = "base"
"##;

    fs::write(&config_path, template)?;
    println!("[OK] muthr.toml created at {:?}", config_path);
    Ok(())
}
