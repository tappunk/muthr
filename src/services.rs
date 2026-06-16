use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::NamedTempFile;

pub fn run(action: crate::ServicesCommands) -> Result<(), color_eyre::Report> {
    match action {
        crate::ServicesCommands::Start => start()?,
        crate::ServicesCommands::Stop => stop()?,
        crate::ServicesCommands::Status => status()?,
        crate::ServicesCommands::Restart => restart()?,
    }
    Ok(())
}

pub fn start() -> Result<(), color_eyre::Report> {
    println!("[PROC] Starting MCP VM...");

    let vm_name = "mcp-services-vm";
    let home = std::env::var("HOME")?;
    let template_path =
        PathBuf::from(&home).join(".config/muthr/lima/templates/mcp-services-vm.yaml");

    if !is_vm_running(vm_name) {
        if !is_vm_exists(vm_name) {
            println!("[PROC] Creating new MCP VM...");

            if !template_path.exists() {
                return Err(color_eyre::eyre::eyre!(
                    "Template not found: {:?}",
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
                return Err(color_eyre::eyre::eyre!("Failed to create MCP VM"));
            }

            let start_status = Command::new("limactl")
                .arg("start")
                .arg(vm_name)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()?;

            if !start_status.success() {
                return Err(color_eyre::eyre::eyre!("Failed to start MCP VM"));
            }

            println!("[ OK ] VM created and started.");
        } else {
            println!("[PROC] Starting existing VM...");
            let status = Command::new("limactl")
                .arg("start")
                .arg(vm_name)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()?;

            if !status.success() {
                return Err(color_eyre::eyre::eyre!("Failed to start MCP VM"));
            }

            println!("[ OK ] VM started.");
        }
    } else {
        println!("[ OK ] MCP VM already running");
        return Ok(());
    }

    if is_vm_provisioned(vm_name) {
        println!("[ OK ] MCP VM already provisioned.");
    } else {
        println!("[PROC] Provisioning MCP services...");
        let provision_status = Command::new("limactl")
            .args(["shell", vm_name])
            .arg("bash")
            .arg("/tmp/mcp-services.sh")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;

        if provision_status.success() {
            println!("[ OK ] MCP services provisioned.");
        } else {
            return Err(color_eyre::eyre::eyre!("MCP services provisioning failed"));
        }
    }

    println!();
    println!("   SearXNG web UI:  http://127.0.0.1:18766");
    println!("   mcp-searxng API: http://127.0.0.1:18765/mcp");

    Ok(())
}

pub fn stop() -> Result<(), color_eyre::Report> {
    let vm_name = "mcp-services-vm";

    if !is_vm_exists(vm_name) {
        println!("[WARN] MCP VM '{}' does not exist", vm_name);
        return Ok(());
    }

    println!("[PROC] Stopping MCP VM...");
    let output = Command::new("limactl")
        .arg("stop")
        .arg(vm_name)
        .output()
        .ok();

    match output {
        Some(out) if out.status.success() => {
            println!("[ OK ] MCP VM stopped.");
        }
        _ => {
            eprintln!("[WARN] ACPI stop sequence sent.");
        }
    }

    Ok(())
}

pub fn status() -> Result<(), color_eyre::Report> {
    let vm_name = "mcp-services-vm";

    println!("[INFO] MCP VM Status:");
    println!("===============================================================================");

    if !is_vm_exists(vm_name) {
        println!("[WARN] VM '{}' does not exist", vm_name);
        return Ok(());
    }

    let status = Command::new("limactl")
        .args(["ls", "-f", "'{{.Status}}'", vm_name])
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

    println!("   VM:             ● {}", status);
    println!("===============================================================================");

    Ok(())
}

pub fn restart() -> Result<(), color_eyre::Report> {
    stop()?;
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?
        .block_on(async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        });
    start()?;
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
        .args(["ls", "-f", "'{{.Status}}'", vm_name])
        .output()
        .ok();

    match output {
        Some(out) if out.status.success() => {
            let status = String::from_utf8_lossy(&out.stdout).trim().to_string();
            status == "Running"
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
