use std::process::Stdio;
use tokio::process::Command;

pub async fn rebase(yes: bool) -> Result<(), color_eyre::Report> {
    println!("[PROC] Starting Master Host Update Chain...");
    let flake_target = format!("{}/dotfiles/nix", std::env::var("HOME")?);

    if !yes {
        println!("[WARN] Previewing changes (--dry-run):");
        println!();

        let output = Command::new("sudo")
            .args([
                "darwin-rebuild",
                "switch",
                "--flake",
                &format!("{}#system", flake_target),
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
        .args([
            "darwin-rebuild",
            "switch",
            "--flake",
            &format!("{}#system", flake_target),
        ])
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
    println!("[ OK ] Master Host Update Chain Complete.");
    println!("   Run muthr services restart to upgrade sandboxed VMs.");

    Ok(())
}

pub async fn clean() -> Result<(), color_eyre::Report> {
    let output = Command::new("which").arg("fd").output().await?;

    if !output.status.success() {
        eprintln!("[WARN] 'fd' not installed. Skipping .DS_Store cleanup.");
        return Ok(());
    }

    let home = std::env::var("HOME")?;
    let dirs = [
        format!("{}/src", home),
        format!("{}/opt", home),
        format!("{}/.config", home),
        format!("{}/dotfiles", home),
    ];

    let args = vec!["-H", "-I", "^[.]DS_Store$"];

    for dir in &dirs {
        let mut child_args = args.clone();
        child_args.push(dir);
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
