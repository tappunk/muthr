use std::path::PathBuf;
use std::process::Command;

pub struct InitCommands {
    pub git_url: Option<String>,
    pub force: bool,
}

pub fn run(cmd: InitCommands) -> Result<(), color_eyre::Report> {
    let config_dir = get_config_dir()?;

    if cmd.force {
        println!("[INIT] Overwriting existing muthr configs...");
    } else if config_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&config_dir)?
            .filter_map(|e| e.ok())
            .collect();

        if !entries.is_empty() {
            println!(
                "[INIT] muthr configs already exist at {}",
                config_dir.display()
            );
            println!(
                "[INIT] Use --force to overwrite, or skip if you want to keep your customizations."
            );
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

    println!("[INIT] Cloning muthr-specs into {}...", tmp_dir.display());
    println!("[INIT] Source: {}", repo_url);

    let status = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            &repo_url,
            tmp_dir.to_str().unwrap(),
        ])
        .status()?;

    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        eprintln!("[ERR] Failed to clone muthr-specs repository.");
        std::process::exit(1);
    }

    if cmd.force && config_dir.exists() {
        println!("[INIT] Removing existing config directory...");
        remove_dir_all(&config_dir)?;
    }

    std::fs::rename(&tmp_dir, &config_dir)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to install configs: {}", e))?;

    println!("[OK] muthr configs installed successfully.");
    println!();
    println!("Next steps:");
    println!("  muthr list  — view available preset profiles");
    println!("  muthr serve — start llama-server engine context daemon");
    println!("  muthr up    — full isolated development VM workflow container spin-up");

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
