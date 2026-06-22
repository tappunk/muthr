use std::path::{Path, PathBuf};
use std::process::Command;

pub struct InitCommands {
    pub git_url: Option<String>,
    pub force: bool,
}

const MANAGED_DIRS: &[&str] = &["clients", "manifests", "provider.d", "provision.d"];

pub fn run(cmd: InitCommands) -> Result<(), color_eyre::Report> {
    let config_dir = get_config_dir()?;

    if cmd.force {
        println!("overwriting existing configs");
    } else if config_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&config_dir)?
            .filter_map(|e| e.ok())
            .collect();

        if !entries.is_empty() {
            return Ok(());
        }
    }

    let repo_url = cmd
        .git_url
        .clone()
        .unwrap_or_else(|| "https://github.com/tappunk/muthr-specs.git".to_string());

    let tmp_dir = std::env::temp_dir().join(format!(
        "muthr-init-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    println!("cloning muthr-specs into {}", tmp_dir.display());

    let status = Command::new("git")
        .args(["clone", "--depth", "1", &repo_url])
        .arg(&tmp_dir)
        .status()?;

    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        eprintln!("err: failed to clone muthr-specs");
        std::process::exit(1);
    }

    if let Some(parent) = config_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if cmd.force && config_dir.exists() {
        sync_managed_dirs(&tmp_dir, &config_dir)?;
    } else {
        std::fs::rename(&tmp_dir, &config_dir)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to install configs: {}", e))?;
    }

    let home = std::env::var("HOME").ok();
    if let Some(home) = home {
        let muthr_toml = PathBuf::from(&home).join(".config/muthr/muthr.toml");
        if !muthr_toml.exists() {
            crate::config::init_config(false)?;
        }
    }

    println!("installed");

    Ok(())
}

fn sync_managed_dirs(src: &Path, dst: &Path) -> Result<(), color_eyre::Report> {
    for dir_name in MANAGED_DIRS {
        let src_path = src.join(dir_name);
        let dst_path = dst.join(dir_name);

        if src_path.exists() {
            if dst_path.exists() {
                remove_dir_all(&dst_path)?;
            }
            std::fs::rename(&src_path, &dst_path)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to sync {}: {}", dir_name, e))?;
        }
    }
    Ok(())
}

fn get_config_dir() -> Result<PathBuf, color_eyre::Report> {
    let home = dirs::home_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("Could not determine home directory"))?;
    Ok(home.join(".config/muthr"))
}

fn remove_dir_all(path: &PathBuf) -> Result<(), color_eyre::Report> {
    std::fs::remove_dir_all(path)?;
    Ok(())
}
