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

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

type ResolvedConfig = (
    u16,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Subcommand)]
pub enum ConfigCommands {
    Init {
        #[arg(long, help = "Force overwrite existing muthr.toml")]
        force: bool,
    },
    Show,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct MuthrConfig {
    pub server_port: Option<u16>,
    pub workspace_root: Option<String>,
    pub model_dir: Option<String>,
    pub default_provision_profile: Option<String>,
    pub default_engine_runtime: Option<String>,
    pub default_engine_profile: Option<String>,
    pub container_host_gateway: Option<String>,
}

impl MuthrConfig {
    fn resolve(self) -> Result<ResolvedConfig, color_eyre::Report> {
        let server_port = self.server_port.unwrap_or(8080);
        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .ok_or_else(|| color_eyre::eyre::eyre!("could not resolve home directory"))?;
        let workspace_root = match self.workspace_root {
            Some(v) => v,
            None => format!("{}/src", home),
        };
        let model_dir = match self.model_dir {
            Some(v) => v,
            None => format!("{}/opt/models", home),
        };
        let provision_profile = self
            .default_provision_profile
            .unwrap_or_else(|| "opencode".to_string());
        let engine_runtime = self.default_engine_runtime.clone();
        let engine_profile = self.default_engine_profile.clone();
        let container_host_gateway = self.container_host_gateway.clone();
        Ok((
            server_port,
            workspace_root,
            model_dir,
            provision_profile,
            engine_runtime,
            engine_profile,
            container_host_gateway,
        ))
    }

    pub fn print_resolved(&self) {
        let (
            server_port,
            workspace_root,
            model_dir,
            provision_profile,
            engine_runtime,
            engine_profile,
            container_host_gateway,
        ) = match self.clone().resolve() {
            Ok(v) => v,
            Err(err) => {
                eprintln!("error: {}", err);
                return;
            }
        };
        eprintln!("info: server_port        {}", server_port);
        eprintln!("info: workspace_root    {}", workspace_root);
        eprintln!("info: model_dir         {}", model_dir);
        eprintln!("info: provision_profile {}", provision_profile);
        eprintln!(
            "info: engine_runtime    {}",
            engine_runtime.as_deref().unwrap_or("mlxcel")
        );
        eprintln!(
            "info: engine_profile    {}",
            engine_profile
                .as_deref()
                .unwrap_or("mlx-community/Qwen3.5-9B-MLX-4bit")
        );
        eprintln!(
            "info: container_gateway {}",
            container_host_gateway.unwrap_or_else(|| "<auto>".to_string())
        );
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
    if let Ok(v) = std::env::var("MUTHR_ENGINE_RUNTIME") {
        config.default_engine_runtime = Some(v);
    }
    if let Ok(v) = std::env::var("MUTHR_ENGINE_PROFILE") {
        config.default_engine_profile = Some(v);
    }
    if let Ok(v) = std::env::var("MUTHR_CONTAINER_HOST_GATEWAY") {
        config.container_host_gateway = Some(v);
    }

    Ok(config)
}

pub fn init_config(force: bool) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let config_dir = PathBuf::from(&home).join(".config/muthr");
    let config_path = config_dir.join("muthr.toml");

    if config_path.exists() && !force {
        return Ok(());
    }

    fs::create_dir_all(&config_dir)?;
    fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o700))?;

    let template = r##"# muthr configuration
server_port = 8080
workspace_root = "~/src"
model_dir = "~/opt/models"
default_provision_profile = "opencode"
default_engine_runtime = "mlxcel"
default_engine_profile = "mlx-community/Qwen3.5-9B-MLX-4bit"
"##;

    fs::write(&config_path, template)?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    eprintln!("info: created {}", config_path.display());
    Ok(())
}
