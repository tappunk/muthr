use std::fs;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::NamedTempFile;

pub async fn run(action: crate::ServicesCommands) -> Result<(), color_eyre::Report> {
    match action {
        crate::ServicesCommands::Start => start().await?,
        crate::ServicesCommands::Stop => stop().await?,
        crate::ServicesCommands::Status => status().await?,
        crate::ServicesCommands::Restart => restart().await?,
        crate::ServicesCommands::Delete { force } => delete(force).await?,
    }
    Ok(())
}

pub async fn start() -> Result<(), color_eyre::Report> {
    let vm_name = "muthr-services";
    let home = std::env::var("HOME")?;
    let template_path = PathBuf::from(&home).join(".config/muthr/manifests/mcp-services.yaml");

    if !is_vm_running(vm_name) {
        if !is_vm_exists(vm_name) {
            println!("creating mcp-services VM");

            if !template_path.exists() {
                return Err(color_eyre::eyre::eyre!(
                    "template not found: {:?}",
                    template_path
                ));
            }

            let content = fs::read_to_string(&template_path)?;
            let mut tmp_yaml = NamedTempFile::new()?;
            tmp_yaml.write_all(content.as_bytes())?;

            let create_status = Command::new("limactl")
                .args(["create", "--name", vm_name])
                .arg(tmp_yaml.path())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()?;

            if !create_status.success() {
                return Err(color_eyre::eyre::eyre!("failed to create MCP VM"));
            }

            let start_status = Command::new("limactl")
                .args(["start", vm_name])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()?;

            if !start_status.success() {
                return Err(color_eyre::eyre::eyre!("failed to start MCP VM"));
            }
        } else {
            println!("starting mcp-services VM");
            let status = Command::new("limactl")
                .args(["start", vm_name])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()?;

            if !status.success() {
                return Err(color_eyre::eyre::eyre!("failed to start MCP VM"));
            }
        }
    } else {
        println!("mcp-services VM already running");
        return Ok(());
    }

    if is_vm_provisioned(vm_name) {
        // nothing - already provisioned
    } else {
        let cp_status = Command::new("limactl")
            .args([
                "cp",
                &format!("{}/.config/muthr/provision.d/mcp-services.sh", home),
                &format!("{}:/tmp/mcp-services.sh", vm_name),
            ])
            .status()?;

        if !cp_status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to copy provision script into services VM"
            ));
        }

        let provision_status = Command::new("limactl")
            .args(["shell", vm_name, "bash", "/tmp/mcp-services.sh", vm_name])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;

        if provision_status.success() {
            println!("mcp services provisioned");
        } else {
            return Err(color_eyre::eyre::eyre!("MCP services provisioning failed"));
        }
    }

    println!("searxng:  http://127.0.0.1:18766");
    println!("mcp:      http://127.0.0.1:18765/mcp");

    Ok(())
}

pub async fn stop() -> Result<(), color_eyre::Report> {
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
            println!("stopped {}", vm_name);
        }
        _ => {
            eprintln!("warn: ACPI stop sequence sent");
        }
    }

    Ok(())
}

pub async fn status() -> Result<(), color_eyre::Report> {
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
        .unwrap_or_else(|| "Unknown".to_string());

    println!("mcp-services: {}", status);

    Ok(())
}

pub async fn restart() -> Result<(), color_eyre::Report> {
    stop().await?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    start().await?;
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
        _ => false,
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

pub async fn delete(force: bool) -> Result<(), color_eyre::Report> {
    let vm_name = "muthr-services";

    if !is_vm_exists(vm_name) {
        return Ok(());
    }

    if !force && !std::io::stdout().is_terminal() {
        eprintln!("err: terminal required for deletion. Use --force.");
        std::process::exit(1);
    }

    println!("deleting mcp-services VM");

    let unprotect_status = Command::new("limactl")
        .args(["unprotect", vm_name])
        .output()?;
    if !unprotect_status.status.success() {
        eprintln!("warn: failed to unprotect VM");
    }

    if is_vm_running(vm_name) {
        let stop_output = Command::new("limactl").arg("stop").arg(vm_name).output()?;
        if !stop_output.status.success() {
            eprintln!("warn: failed to stop VM");
        }
    }

    let delete_status = Command::new("limactl").args(["delete", vm_name]).output()?;

    if !delete_status.status.success() {
        return Err(color_eyre::eyre::eyre!("failed to delete MCP services VM"));
    }

    let home = std::env::var("HOME")?;
    let cache_file = PathBuf::from(&home).join(".cache/muthr/services-profiles");
    if cache_file.exists() {
        fs::remove_file(&cache_file)?;
    }

    println!("deleted {}", vm_name);
    Ok(())
}
