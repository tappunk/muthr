use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::process::Command;

use crate::engine;
use crate::model;
use crate::preset;
use crate::ui;

pub fn resolve_workspace_context() -> Result<(String, PathBuf, PathBuf), color_eyre::Report> {
    let current_dir = std::env::current_dir()?;
    let canonical_current = std::fs::canonicalize(&current_dir)?;
    let home = std::env::var("HOME")?;

    let raw_workspace_root = std::env::var("OPENCODE_WORKSPACE_ROOT")
        .unwrap_or_else(|_| format!("{}/src/projects", home));
    let canonical_workspace = std::fs::canonicalize(Path::new(&raw_workspace_root))
        .unwrap_or_else(|_| PathBuf::from(&raw_workspace_root));

    let mut inside_muthr_config = false;
    if let Ok(muthr_config_dir) = std::fs::canonicalize(PathBuf::from(&home).join(".config/muthr"))
    {
        if canonical_current.starts_with(&muthr_config_dir) {
            inside_muthr_config = true;
        }
    }

    if inside_muthr_config {
        return Ok((
            "muthr-config-sandbox".to_string(),
            PathBuf::from(&home).join(".config/muthr"),
            current_dir,
        ));
    }

    if canonical_current.starts_with(&canonical_workspace) {
        if canonical_current == canonical_workspace {
            return Err(color_eyre::eyre::eyre!(
                "Navigate into a project directory first."
            ));
        }
        let relative = canonical_current.strip_prefix(&canonical_workspace)?;
        let project_folder = relative
            .components()
            .next()
            .ok_or_else(|| color_eyre::eyre::eyre!("Invalid workspace path"))?
            .as_os_str()
            .to_str()
            .ok_or_else(|| color_eyre::eyre::eyre!("Invalid project name"))?
            .to_string();

        let sanitized: String = project_folder
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect();

        if sanitized.is_empty() {
            return Err(color_eyre::eyre::eyre!("Sanitized project name is empty"));
        }

        let vm_name = format!("{}-sandbox", sanitized);
        let mount_point = PathBuf::from(&raw_workspace_root).join(&project_folder);
        Ok((vm_name, mount_point, current_dir))
    } else {
        Err(color_eyre::eyre::eyre!(
            "Sandbox tasks are restricted to project workspaces or the muthr configuration directory."
        ))
    }
}

pub async fn vm_exists(vm_name: &str) -> bool {
    let output = Command::new("limactl")
        .args(["ls", "-q"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success());

    if let Some(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if line == vm_name {
                return true;
            }
        }
    }
    false
}

pub async fn vm_is_running(vm_name: &str) -> bool {
    let output = Command::new("limactl")
        .args(["ls", "-f", "'{{.Status}}'", vm_name])
        .output()
        .await
        .ok();

    if let Some(out) = output {
        if out.status.success() {
            let status = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return status == "Running";
        }
    }
    false
}

pub async fn vm_stop(vm_name: &str) -> Result<(), color_eyre::Report> {
    println!("\n[PROC] Stopping sandbox VM ({})...", vm_name);
    let output = Command::new("limactl")
        .arg("stop")
        .arg(vm_name)
        .output()
        .await
        .ok();

    match output {
        Some(out) if out.status.success() => {
            println!("[ OK ] VM stopped cleanly. System memory reclaimed.");
        }
        _ => {
            eprintln!("[WARN] ACPI stop sequence sent.");
        }
    }
    Ok(())
}

async fn vm_create(
    vm_name: &str,
    workspace_root: &Path,
    mount_point: &Path,
) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let template_path = PathBuf::from(&home).join(".config/muthr/dev-sandbox.yaml");

    if !template_path.exists() {
        return Err(color_eyre::eyre::eyre!(
            "Template not found: {:?}",
            template_path
        ));
    }

    let content = fs::read_to_string(&template_path).await?;
    let expanded = content
        .replace(
            "__WORKSPACE_ROOT__",
            workspace_root.to_str().unwrap_or_default(),
        )
        .replace("__MOUNT_POINT__", mount_point.to_str().unwrap_or_default());

    println!(
        "[PROC] VM '{}' not found. Creating and starting...",
        vm_name
    );
    let mut tmp_yaml = NamedTempFile::new()?;
    tmp_yaml.write_all(expanded.as_bytes())?;

    let create_status = Command::new("limactl")
        .args(["create", "--name", vm_name])
        .arg(tmp_yaml.path())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;

    if !create_status.success() {
        return Err(color_eyre::eyre::eyre!("Failed to create VM: {}", vm_name));
    }

    let start_status = Command::new("limactl")
        .arg("start")
        .arg(vm_name)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;

    if !start_status.success() {
        return Err(color_eyre::eyre::eyre!("Failed to start VM: {}", vm_name));
    }
    Ok(())
}

async fn vm_start(vm_name: &str) -> Result<(), color_eyre::Report> {
    println!("[PROC] Starting sandbox VM ({})...", vm_name);
    let status = Command::new("limactl")
        .arg("start")
        .arg(vm_name)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;

    if !status.success() {
        return Err(color_eyre::eyre::eyre!("Failed to start VM: {}", vm_name));
    }
    Ok(())
}

async fn is_vm_provisioned(vm_name: &str) -> bool {
    let output = Command::new("limactl")
        .args(["shell", "--workdir", "/tmp", vm_name])
        .arg("bash")
        .arg("-c")
        .arg("test -f /var/log/opencode_provision.lock")
        .output()
        .await
        .ok();

    match output {
        Some(out) => out.status.success(),
        None => false,
    }
}

async fn dpkg_lock_free(vm_name: &str) -> bool {
    let output = Command::new("limactl")
        .args(["shell", "--workdir", "/tmp", vm_name])
        .arg("bash")
        .arg("-c")
        .arg(
            "pgrep -x apt-get > /dev/null 2>&1 || \
             pgrep -x dpkg > /dev/null 2>&1 || \
             fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1; \
             exit $?",
        )
        .output()
        .await
        .ok();

    match output {
        Some(out) => !out.status.success(),
        None => true,
    }
}

async fn wait_for_dpkg(vm_name: &str, timeout_secs: u64) -> Result<(), color_eyre::Report> {
    let start = std::time::Instant::now();
    loop {
        if dpkg_lock_free(vm_name).await {
            return Ok(());
        }
        if start.elapsed() > std::time::Duration::from_secs(timeout_secs) {
            return Err(color_eyre::eyre::eyre!(
                "Timed out waiting for dpkg/apt lock to be released"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

async fn run_provision(vm_name: &str, script_name: &str) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let host_script =
        PathBuf::from(&home).join(format!(".config/muthr/provision/{}.sh", script_name));

    if !host_script.exists() {
        return Err(color_eyre::eyre::eyre!(
            "Provision script not found: {:?}",
            host_script
        ));
    }

    println!("[PROC] Running provision: {}...", script_name);
    let script_str = host_script
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid UTF-8 in provision script path"))?;

    println!("[PROC] Waiting for dpkg/apt lock to be free...");
    wait_for_dpkg(vm_name, 120).await?;

    let status = Command::new("bash")
        .arg(script_str)
        .arg(vm_name)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;

    if !status.success() {
        return Err(color_eyre::eyre::eyre!("Provision failed: {}", script_name));
    }

    println!("[ OK ] Provision complete: {}", script_name);
    Ok(())
}

async fn handle_provisioning(vm_name: &str) -> Result<(), color_eyre::Report> {
    if is_vm_provisioned(vm_name).await {
        return Ok(());
    }

    let options = vec![
        "Base only -- shell access, no extra installs",
        "Base + opencode -- MCP servers + opencode-ai CLI",
    ];

    let is_tty = std::io::stdout().is_terminal();
    let idx = if is_tty {
        match ui::select_list(&options) {
            Some(i) => i,
            None => {
                println!("[INFO] Skipping provision. Base VM only.");
                return Ok(());
            }
        }
    } else {
        println!("[INFO] No TTY detected. Defaulting to: {}", options[1]);
        1
    };

    if idx == 1 {
        run_provision(vm_name, "opencode").await?;
    }
    Ok(())
}

pub async fn up(port: u16) -> Result<(), color_eyre::Report> {
    let (vm_name, mount_point, workdir) = resolve_workspace_context()?;
    println!("[INFO] Target Virtual Environment Context: {}", vm_name);

    if !engine::verify_health(port).await {
        return Err(color_eyre::eyre::eyre!(
            "Inference pipeline unreachable at 127.0.0.1:{}. Run 'muthr serve' first.",
            port
        ));
    }

    let home = std::env::var("HOME")?;
    let presets = preset::list_presets()?;

    if !vm_exists(&vm_name).await {
        vm_create(&vm_name, &mount_point, &mount_point).await?;
    } else if !vm_is_running(&vm_name).await {
        vm_start(&vm_name).await?;
    } else {
        println!("[ OK ] VM already running");
    }

    handle_provisioning(&vm_name).await?;

    let loaded_model = model::poll_loaded_model("127.0.0.1", port, 20, 1.5).await?;
    println!("[INFO] Model detected: {}", loaded_model);

    let ctx_window = model::get_ctx_window("127.0.0.1", port).await?;
    println!("[INFO] Context window: {}", ctx_window);

    let runtime_config = {
        let active_profile_path = PathBuf::from(&home).join(".cache/muthr/opencode-profile");
        let preset_to_use = if active_profile_path.exists() {
            let content = fs::read_to_string(&active_profile_path).await?;
            let mut preset_name = String::new();
            for line in content.lines() {
                if line.starts_with("export LLAMA_ARG_MODELS_PRESET=") {
                    if let Some(start) = line.find('"') {
                        if let Some(end) = line[start + 1..].find('"') {
                            preset_name = line[start + 1..start + 1 + end].to_string();
                        }
                    }
                }
            }
            if !preset_name.is_empty() {
                presets
                    .iter()
                    .find(|p| p.path.to_string_lossy() == preset_name)
            } else {
                None
            }
        } else {
            None
        };

        let selected_preset = preset_to_use.or(presets.first());
        match selected_preset {
            Some(p) => crate::config::generate_runtime_config(p, port, &mount_point)?,
            None => {
                return Err(color_eyre::eyre::eyre!(
                    "No presets available for config generation"
                ))
            }
        }
    };

    println!("[PROC] Injecting runtime configuration mapping...");
    let cp_status = Command::new("limactl")
        .args([
            "cp",
            runtime_config.to_str().unwrap(),
            &format!("{}:/tmp/opencode-config.json", vm_name),
        ])
        .status()
        .await?;

    if !cp_status.success() {
        return Err(color_eyre::eyre::eyre!("Failed to copy config into VM."));
    }

    println!("[PROC] Launching opencode session...");
    let status = Command::new("limactl")
        .args([
            "shell",
            "--workdir",
            workdir.to_str().unwrap_or("/tmp"),
            &vm_name,
            "--",
            "env",
            "PATH=/home/user.guest/.opencode/bin:/home/user.guest/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "OPENCODE_CONFIG=/tmp/opencode-config.json",
            "opencode",
        ])
        .stdin(Stdio::inherit())
        .status()
        .await?;

    vm_stop(&vm_name).await?;

    if !status.success() {
        return Err(color_eyre::eyre::eyre!(
            "opencode session exited with error"
        ));
    }
    Ok(())
}

pub async fn down() -> Result<(), color_eyre::Report> {
    let (vm_name, _, _) = resolve_workspace_context()?;
    if !vm_exists(&vm_name).await {
        println!("[WARN] VM '{}' does not exist", vm_name);
        return Ok(());
    }
    vm_stop(&vm_name).await?;
    Ok(())
}

pub async fn list() -> Result<(), color_eyre::Report> {
    let sandbox_suffix = "-sandbox";
    println!("[INFO] Sandbox VMs:");
    println!("===============================================================================");

    let output = Command::new("limactl")
        .args(["ls", "-q"])
        .output()
        .await
        .ok();

    let vms: Vec<String> = match output {
        Some(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|v| v.ends_with(sandbox_suffix))
            .map(|v| v.to_string())
            .collect(),
        _ => Vec::new(),
    };

    if vms.is_empty() {
        println!("[WARN] No sandbox VMs found");
        return Ok(());
    }

    let is_tty = std::io::stdout().is_terminal();
    if !is_tty {
        for vm in &vms {
            let status = Command::new("limactl")
                .args(["ls", "-f", "'{{.Status}}'", vm])
                .output()
                .await
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

            let project = vm.strip_suffix(sandbox_suffix).unwrap_or(vm);
            let mount_point = format!("/sandbox-{}", project);
            println!("  {:<30} {}  Mount: {}", vm, status, mount_point);
        }
    } else {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for vm in &vms {
            let status = Command::new("limactl")
                .args(["ls", "-f", "'{{.Status}}'", vm])
                .output()
                .await
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

            let project = vm.strip_suffix(sandbox_suffix).unwrap_or(vm);
            let mount_point = format!("/sandbox-{}", project);
            rows.push(vec![vm.clone(), status, mount_point.to_string()]);
        }

        let headers = vec!["VM Name", "Status", "Mount Point"];
        ui::select_table(&headers, rows);
    }

    println!("===============================================================================");
    Ok(())
}
