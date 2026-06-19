use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

fn resolve_flake_dir() -> Result<PathBuf, color_eyre::Report> {
    let home = dirs::home_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("Could not determine home directory"))?;

    if let Ok(cwd) = std::env::current_dir() {
        let mut ancestor = cwd.clone();
        while ancestor.components().count() > 1 {
            if ancestor.join("nix").exists() || ancestor.join("flake.nix").exists() {
                return Ok(ancestor);
            }
            ancestor.pop();
        }
    }

    let standard_paths = [
        home.join(".config/dotfiles"),
        home.join("src/projects/dotfiles"),
        home.join(".config/muthr"),
    ];

    for path in &standard_paths {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    Err(color_eyre::eyre::eyre!(
        "Could not automatically locate your active Nix flake directory. Run this command from within your configuration repository."
    ))
}

pub async fn rebase(yes: bool) -> Result<(), color_eyre::Report> {
    println!("[PROC] Starting Master Host Update Chain...");

    let flake_root = resolve_flake_dir()?;
    let flake_target = flake_root.join("nix");

    if !flake_target.exists() && !flake_root.join("flake.nix").exists() {
        return Err(color_eyre::eyre::eyre!(
            "Could not locate Nix configuration flake files at target path: {:?}",
            flake_root
        ));
    }

    let flake_argument = format!("{}#system", flake_target.to_string_lossy());

    if !yes {
        println!(
            "[WARN] Previewing changes (--dry-run) using target: {}...",
            flake_argument
        );
        println!();

        let output = Command::new("sudo")
            .args([
                "darwin-rebuild",
                "switch",
                "--flake",
                &flake_argument,
                "--dry-run",
            ])
            .stdin(Stdio::inherit())
            .output()
            .await?;

        if !output.status.success() {
            eprintln!("Dry run failed. Proceeding anyway...");
        }

        println!();
        let confirm = crate::ui::confirm("[WARN] Proceed with rebuild?");
        if !confirm {
            println!("[WARN] Rebuild cancelled.");
            return Ok(());
        }
    }

    Command::new("sudo")
        .args(["darwin-rebuild", "switch", "--flake", &flake_argument])
        .stdin(Stdio::inherit())
        .status()
        .await?;

    println!();
    println!("[PROC] Synchronizing Editor Environment (Neovim)...");
    Command::new("nvim")
        .args(["--headless", "+Lazy! sync", "+qa"])
        .status()
        .await?;

    println!();
    println!("[PROC] Cleaning workspace metadata...");
    clean().await?;

    println!();
    println!("[ OK ] Update complete.");
    println!("   Run 'muthr services restart' to upgrade sandboxed VMs.");

    Ok(())
}

pub async fn clean() -> Result<(), color_eyre::Report> {
    let output = Command::new("which").arg("fd").output().await?;

    if !output.status.success() {
        eprintln!("[WARN] 'fd' not installed. Skipping .DS_Store cleanup.");
        return Ok(());
    }

    let home = dirs::home_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("Could not determine home directory"))?;

    let mut paths_to_clean = vec![
        home.join("src"),
        home.join("opt"),
        home.join(".config/muthr"),
    ];

    if let Ok(flake_dir) = resolve_flake_dir() {
        if !paths_to_clean.contains(&flake_dir) {
            paths_to_clean.push(flake_dir);
        }
    }

    let args = vec!["-H", "-I", "^[.]DS_Store$"];

    for dir in &paths_to_clean {
        if !dir.exists() {
            continue;
        }

        let mut child_args = args.clone();
        child_args.push(dir.to_str().unwrap());
        child_args.push("-t");
        child_args.push("f");
        child_args.push("-X");
        child_args.push("rm");
        child_args.push("-f");

        Command::new("fd").args(&child_args).status().await?;
    }

    println!("[ OK ] Cleanup complete.");
    Ok(())
}
