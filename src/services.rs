use std::fs;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::NamedTempFile;

pub async fn run(action: crate::ServicesCommands) -> Result<(), color_eyre::Report> {
    match action {
        crate::ServicesCommands::Start { dry_run } => start(dry_run).await?,
        crate::ServicesCommands::Stop { dry_run } => stop(dry_run).await?,
        crate::ServicesCommands::Status { output } => status(output).await?,
        crate::ServicesCommands::Restart { dry_run } => restart(dry_run).await?,
        crate::ServicesCommands::Delete {
            force,
            yes,
            dry_run,
        } => delete(force || yes, dry_run).await?,
    }
    Ok(())
}

pub async fn start(dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services start");
        return Ok(());
    }
    let vm_name = "muthr-services";
    let home = std::env::var("HOME")?;
    let template_path = PathBuf::from(&home).join(".config/muthr/manifests/muthr-services.yaml");

    if !is_vm_running(vm_name) {
        if !is_vm_exists(vm_name) {
            eprintln!("info: creating muthr-services vm");

            if !template_path.exists() {
                return Err(color_eyre::eyre::eyre!(
                    "template not found: {:?}",
                    template_path
                ));
            }

            let content = fs::read_to_string(&template_path)?;
            let mut tmp_yaml = NamedTempFile::new()?;
            tmp_yaml.write_all(content.as_bytes())?;

            let mut create_cmd = Command::new("limactl");
            create_cmd
                .args(["create", "--name", vm_name])
                .arg(tmp_yaml.path())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            let create_status = create_cmd.status()?;

            if !create_status.success() {
                return Err(color_eyre::eyre::eyre!(
                    "failed to create muthr-services vm"
                ));
            }

            let mut start_cmd = Command::new("limactl");
            start_cmd
                .args(["start", vm_name])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            let start_status = start_cmd.status()?;

            if !start_status.success() {
                return Err(color_eyre::eyre::eyre!("failed to start muthr-services vm"));
            }
        } else {
            eprintln!("info: starting muthr-services vm");
            let mut start_cmd = Command::new("limactl");
            start_cmd
                .args(["start", vm_name])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            let status = start_cmd.status()?;

            if !status.success() {
                return Err(color_eyre::eyre::eyre!("failed to start muthr-services vm"));
            }
        }
    } else {
        eprintln!("info: muthr-services vm already running");
        return Ok(());
    }

    if !is_vm_provisioned(vm_name) {
        let mut cp_cmd = Command::new("limactl");
        cp_cmd.args([
            "cp",
            &format!("{}/.config/muthr/provision.d/muthr-services.sh", home),
            &format!("{}:/tmp/muthr-services.sh", vm_name),
        ]);
        let cp_status = cp_cmd.status()?;

        if !cp_status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to copy provision script into services VM"
            ));
        }

        let mut provision_cmd = Command::new("limactl");
        provision_cmd
            .args(["shell", vm_name, "bash", "/tmp/muthr-services.sh", vm_name])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let provision_status = provision_cmd.status()?;

        if provision_status.success() {
            eprintln!("info: muthr-services vm provisioned");
        } else {
            return Err(color_eyre::eyre::eyre!(
                "muthr-services vm provisioning failed"
            ));
        }
    }

    eprintln!("info: searxng:  http://127.0.0.1:18766");
    eprintln!("info: mcp:      http://127.0.0.1:18765/mcp");

    Ok(())
}

pub async fn stop(dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services stop");
        return Ok(());
    }
    let vm_name = "muthr-services";

    if !is_vm_exists(vm_name) {
        return Ok(());
    }

    let output = Command::new("limactl")
        .arg("stop")
        .arg(vm_name)
        .output()
        .ok();

    match output {
        Some(out) if out.status.success() => {
            eprintln!("info: stopped {}", vm_name);
        }
        Some(_) | None => {
            eprintln!("warning: acpi stop sequence sent");
        }
    }

    Ok(())
}

pub async fn status(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let vm_name = "muthr-services";

    if !is_vm_exists(vm_name) {
        return Ok(());
    }

    let status = Command::new("limactl")
        .args(["ls", "-f", "{{.Status}}", vm_name])
        .output()
        .ok()
        .and_then(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .to_string()
                .split_whitespace()
                .next()
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    match output {
        crate::OutputFormat::Text => eprintln!("muthr-services vm: {}", status.to_lowercase()),
        crate::OutputFormat::Json => {
            let payload = serde_json::json!({"name": vm_name, "status": status.to_lowercase()});
            println!("{}", serde_json::to_string(&payload)?);
        }
        crate::OutputFormat::Ndjson => {
            let payload = serde_json::json!({"name": vm_name, "status": status.to_lowercase()});
            println!("{}", serde_json::to_string(&payload)?);
        }
    }

    Ok(())
}

pub async fn restart(dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services restart");
        return Ok(());
    }
    stop(false).await?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    start(false).await?;
    Ok(())
}

fn is_vm_exists(vm_name: &str) -> bool {
    let output = Command::new("limactl")
        .args(["ls", "-q"])
        .output()
        .ok()
        .filter(|o| o.status.success());

    match output {
        Some(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                if line == vm_name {
                    return true;
                }
            }
            false
        }
        None => false,
    }
}

fn is_vm_running(vm_name: &str) -> bool {
    let output = Command::new("limactl")
        .args(["ls", "-f", "{{.Status}}", vm_name])
        .output()
        .ok();

    match output {
        Some(out) if out.status.success() => {
            let status = String::from_utf8_lossy(&out.stdout);
            status.contains("Running")
        }
        Some(_) | None => false,
    }
}

fn is_vm_provisioned(vm_name: &str) -> bool {
    let output = Command::new("limactl")
        .arg("shell")
        .arg(vm_name)
        .arg("bash")
        .arg("-c")
        .arg("test -f $HOME/mcp-stdio.sh && test -f $HOME/.local/lib/node_modules/mcp-searxng/dist/cli.js")
        .output()
        .ok();

    match output {
        Some(out) => out.status.success(),
        None => false,
    }
}

pub async fn delete(force: bool, dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services delete");
        return Ok(());
    }
    let vm_name = "muthr-services";

    if !is_vm_exists(vm_name) {
        return Ok(());
    }

    if !force && !std::io::stdout().is_terminal() {
        eprintln!("error: terminal required for deletion, use --yes");
        std::process::exit(77);
    }

    eprintln!("info: deleting muthr-services vm");

    let unprotect_status = Command::new("limactl")
        .args(["unprotect", vm_name])
        .output()?;
    if !unprotect_status.status.success() {
        eprintln!("warning: failed to unprotect vm");
    }

    if is_vm_running(vm_name) {
        let stop_output = Command::new("limactl").arg("stop").arg(vm_name).output()?;
        if !stop_output.status.success() {
            eprintln!("warning: failed to stop vm");
        }
    }

    let delete_status = Command::new("limactl").args(["delete", vm_name]).output()?;

    if !delete_status.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to delete muthr-services vm"
        ));
    }

    eprintln!("info: deleted {}", vm_name);
    Ok(())
}
