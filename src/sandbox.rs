use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

fn sanitize_project_name(name: &str) -> Option<String> {
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();

    if sanitized.is_empty() {
        return None;
    }

    Some(sanitized)
}

fn paths_are_prefix(current: &Path, potential_parent: &Path) -> bool {
    let canonical_pair = match (
        std::fs::canonicalize(current),
        std::fs::canonicalize(potential_parent),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return false,
    };

    let current_components: Vec<_> = canonical_pair.0.components().collect();
    let parent_components: Vec<_> = canonical_pair.1.components().collect();

    if parent_components.len() > current_components.len() {
        return false;
    }

    current_components
        .iter()
        .zip(parent_components.iter())
        .all(|(a, b)| a == b)
}

pub fn resolve_workspace_context() -> Result<(String, PathBuf, PathBuf), color_eyre::Report> {
    let current_dir = std::env::current_dir()?;
    let home = std::env::var("HOME")?;

    let raw_workspace_root = if let Ok(v) = std::env::var("MUTHR_WORKSPACE_ROOT") {
        v
    } else if let Ok(cfg) = crate::config::load() {
        cfg.workspace_root
            .unwrap_or_else(|| format!("{}/src", home))
    } else {
        format!("{}/src", home)
    };

    let workspace_root = raw_workspace_root
        .strip_prefix("~/")
        .map(|p| format!("{}/{}", home, p))
        .unwrap_or(raw_workspace_root);

    let muthr_config_dir = PathBuf::from(&home).join(".config/muthr");
    if paths_are_prefix(&current_dir, &muthr_config_dir) {
        return Ok((
            "muthr-config".to_string(),
            muthr_config_dir.clone(),
            current_dir,
        ));
    }

    let workspace_path = PathBuf::from(&workspace_root);
    if !workspace_path.exists() {
        eprintln!("Error: workspace root '{}' does not exist", workspace_root);
        eprintln!(
            "Hint: Create it with 'mkdir -p {}' or set MUTHR_WORKSPACE_ROOT.",
            workspace_root
        );
        std::process::exit(1);
    }

    let result = (|| -> Option<(String, PathBuf)> {
        let relative = current_dir.strip_prefix(&workspace_path).ok()?;
        let project_name = relative.components().next()?.as_os_str().to_str()?;
        sanitize_project_name(project_name)
            .map(|s| (format!("muthr-{}", s), workspace_path.join(&s)))
    })();

    let (project_folder, mount_point) = match result {
        Some(v) => v,
        None => {
            let can_current = std::fs::canonicalize(&current_dir).map_err(|e| {
                color_eyre::eyre::eyre!("Failed to canonicalize current directory: {}", e)
            })?;
            let can_workspace = workspace_path.canonicalize().map_err(|e| {
                color_eyre::eyre::eyre!("Failed to canonicalize workspace root: {}", e)
            })?;

            if can_current == can_workspace {
                return Err(color_eyre::eyre::eyre!(
                    "Navigate into a project directory first."
                ));
            }

            let relative = match can_current.strip_prefix(&can_workspace) {
                Ok(r) => r,
                Err(_) => {
                    return Err(color_eyre::eyre::eyre!(
                        "Current directory is not within the canonicalized workspace root"
                    ));
                }
            };
            let project_name = relative
                .components()
                .next()
                .ok_or_else(|| color_eyre::eyre::eyre!("Invalid workspace path"))?
                .as_os_str()
                .to_str()
                .ok_or_else(|| color_eyre::eyre::eyre!("Invalid project name"))?;

            let sanitized = sanitize_project_name(project_name)
                .ok_or_else(|| color_eyre::eyre::eyre!("Sanitized project name is empty"))?;

            (
                format!("muthr-{}", sanitized),
                can_workspace.join(&sanitized),
            )
        }
    };

    Ok((project_folder, mount_point, current_dir))
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
        .args(["ls", "-f", "{{.Status}}", vm_name])
        .output()
        .await
        .ok();

    if let Some(ref out) = output
        && out.status.success()
    {
        let status = String::from_utf8_lossy(&out.stdout);
        return status.contains("Running");
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

pub async fn protect_vm(vm_name: &str) -> Result<(), color_eyre::Report> {
    let status = Command::new("limactl")
        .args(["protect", vm_name])
        .output()
        .await?;

    if !status.status.success() {
        eprintln!("[WARN] Failed to protect VM '{}'.", vm_name);
    }
    Ok(())
}

pub async fn unprotect_vm(vm_name: &str) -> Result<(), color_eyre::Report> {
    let status = Command::new("limactl")
        .args(["unprotect", vm_name])
        .output()
        .await?;

    if !status.status.success() {
        eprintln!("[WARN] Failed to unprotect VM '{}'.", vm_name);
    }
    Ok(())
}

pub async fn delete_vm(vm_name: &str, force: bool) -> Result<(), color_eyre::Report> {
    if !force && !std::io::stdout().is_terminal() {
        eprintln!("Error: terminal required for deletion. Use --force to skip confirmation.");
        std::process::exit(1);
    }

    println!("[PROC] Deleting sandbox VM '{}'...", vm_name);

    unprotect_vm(vm_name).await?;

    if vm_is_running(vm_name).await {
        vm_stop(vm_name).await?;
    }

    let status = Command::new("limactl")
        .args(["delete", vm_name])
        .output()
        .await?;

    if !status.status.success() {
        return Err(color_eyre::eyre::eyre!("Failed to delete VM '{}'", vm_name));
    }

    let cache_dir = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".cache/muthr"))
        .unwrap_or_else(|| PathBuf::from("/tmp/muthr-cache"));

    let profile_cache = cache_dir.join(format!("{}-profiles", vm_name));
    if profile_cache.exists() {
        fs::remove_file(&profile_cache).await.ok();
    }

    println!("[ OK ] VM '{}' deleted and cache cleaned.", vm_name);
    Ok(())
}

pub async fn run_provision(
    vm_name: &str,
    script_name: &str,
    model_name: &str,
    ctx_window: u32,
    mount_point: &std::path::Path,
    port: u16,
) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let host_script =
        PathBuf::from(&home).join(format!(".config/muthr/provision.d/{}.sh", script_name));

    if !host_script.exists() {
        return Err(color_eyre::eyre::eyre!(
            "Provision script not found: {:?}",
            host_script
        ));
    }

    println!("[PROC] Running provision: {}...", script_name);
    let script_content = fs::read_to_string(&host_script).await?;
    let openai_url = format!("http://host.lima.internal:{}/v1", port);

    let mut child = Command::new("limactl")
        .args(["shell", "--workdir", "/tmp", vm_name, "bash"])
        .env("MUTHR_OPENAI_URL", &openai_url)
        .env("MUTHR_MODEL_NAME", model_name)
        .env("MUTHR_CTX_WINDOW", ctx_window.to_string())
        .env(
            "MUTHR_WORKSPACE_MOUNT",
            mount_point.to_str().unwrap_or("/workspace"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(script_content.as_bytes()).await?;
    }
    std::mem::drop(child.stdin.take());

    let status = child.wait().await?;
    if !status.success() {
        return Err(color_eyre::eyre::eyre!("Provision failed: {}", script_name));
    }

    println!("[ OK ] Provision complete: {}", script_name);
    Ok(())
}

async fn wait_for_vm_ready(vm_name: &str) -> Result<(), color_eyre::Report> {
    let mut retries = 0;
    let max_retries = 60;

    loop {
        if vm_is_running(vm_name).await {
            return Ok(());
        }
        retries += 1;
        if retries >= max_retries {
            return Err(color_eyre::eyre::eyre!(
                "Timed out waiting for VM '{}' to become ready",
                vm_name
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

pub async fn up(profile_name: String) -> Result<(), color_eyre::Report> {
    let (vm_name, mount_point, workdir) = resolve_workspace_context()?;
    println!("[INFO] Target Virtual Environment Context: {}", vm_name);

    let home = std::env::var("HOME")?;
    let config_dir = PathBuf::from(&home).join(".config/muthr");
    let manifest_path = crate::catalog::resolve_manifest(&config_dir, &profile_name);

    if !manifest_path.exists() {
        eprintln!("Error: manifest not found at {:?}", manifest_path);
        eprintln!("Hint: Run 'muthr init' to install profiles, or create the manifest manually.");
        std::process::exit(1);
    }

    let content = fs::read_to_string(&manifest_path).await?;
    let expanded = content
        .replace(
            "__WORKSPACE_ROOT__",
            mount_point.to_str().unwrap_or_default(),
        )
        .replace("__MOUNT_POINT__", mount_point.to_str().unwrap_or_default());

    if !vm_exists(&vm_name).await {
        println!(
            "[PROC] VM '{}' not found. Creating and starting...",
            vm_name
        );
        let mut tmp_yaml = NamedTempFile::new()?;
        tmp_yaml.write_all(expanded.as_bytes())?;

        let create_status = Command::new("limactl")
            .args(["create", "--name", &vm_name])
            .arg(tmp_yaml.path())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await?;

        if !create_status.success() {
            return Err(color_eyre::eyre::eyre!("Failed to create VM: {}", vm_name));
        }

        let start_status = Command::new("limactl")
            .args(["start", &vm_name])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await?;

        if !start_status.success() {
            return Err(color_eyre::eyre::eyre!("Failed to start VM: {}", vm_name));
        }

        wait_for_vm_ready(&vm_name).await?;

        protect_vm(&vm_name).await?;
    } else if !vm_is_running(&vm_name).await {
        println!("[PROC] Starting sandbox VM ({})...", vm_name);
        let status = Command::new("limactl")
            .args(["start", &vm_name])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await?;

        if !status.success() {
            return Err(color_eyre::eyre::eyre!("Failed to start VM: {}", vm_name));
        }

        wait_for_vm_ready(&vm_name).await?;
    } else {
        println!("[ OK ] VM already running");
    }

    // Poll inference engine for runtime context (needed by provision scripts)
    let cfg = crate::config::load()?;
    let server_port = cfg.server_port.unwrap_or(8080);
    let parsed_model = crate::model::poll_loaded_model("127.0.0.1", server_port, 20, 1.0).await?;
    let sanitized_model = std::path::Path::new(&parsed_model)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&parsed_model)
        .trim_end_matches(".gguf")
        .to_string();
    let active_model = if sanitized_model.trim().is_empty() {
        "01-qwen3-6-35b-a3b".to_string()
    } else {
        sanitized_model
    };
    let ctx_window = crate::model::get_ctx_window("127.0.0.1", server_port)
        .await
        .unwrap_or(16000);

    if profile_name != "base" {
        let cache_dir = PathBuf::from(&home)
            .join(".cache/muthr")
            .join(format!("{}-profiles", vm_name));

        let existing_profiles = fs::read_to_string(&cache_dir).await.unwrap_or_default();
        if !existing_profiles.lines().any(|l| l.trim() == profile_name) {
            println!("[PROC] Applying profile: {}...", profile_name);
            run_provision(
                &vm_name,
                &profile_name,
                &active_model,
                ctx_window,
                &mount_point,
                server_port,
            )
            .await?;

            let mut existing = existing_profiles.clone();
            if !existing.is_empty() && !existing.ends_with('\n') {
                existing.push('\n');
            }
            existing.push_str(&profile_name);
            existing.push('\n');
            fs::create_dir_all(cache_dir.parent().unwrap()).await?;
            fs::write(&cache_dir, existing).await?;
        } else {
            println!("[ OK ] Profile '{}' already applied.", profile_name);
        }

        println!("[ OK ] Launching application workspace context...");
        let target_args = match profile_name.as_str() {
            "opencode" => vec!["opencode"],
            "hermes-agent" => vec!["bash", "-l"],
            _ => vec![],
        };

        let mut args = vec![
            "--tty",
            "shell",
            "--workdir",
            workdir.to_str().unwrap_or("/tmp"),
            &vm_name,
        ];
        args.extend(target_args);

        let status = Command::new("limactl")
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await?;

        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "Application session exited with error"
            ));
        }
    } else {
        let cache_dir = PathBuf::from(&home)
            .join(".cache/muthr")
            .join(format!("{}-profiles", vm_name));

        if !cache_dir.exists() {
            fs::create_dir_all(cache_dir.parent().unwrap()).await?;
            fs::write(&cache_dir, "base\n").await?;
        }

        println!("[ OK ] Sandbox ready. Launching clean hypervisor shell session...");
        let status = Command::new("limactl")
            .args([
                "--tty",
                "shell",
                "--workdir",
                workdir.to_str().unwrap_or("/tmp"),
                &vm_name,
            ])
            .stdin(Stdio::inherit())
            .status()
            .await?;

        if !status.success() {
            return Err(color_eyre::eyre::eyre!("Shell session exited with error"));
        }
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
    let muthr_prefix = "muthr-";
    println!("[INFO] Managed VMs:");
    println!("===============================================================================");

    let output = Command::new("limactl")
        .args(["ls", "-q"])
        .output()
        .await
        .ok();

    let vms: Vec<String> = match output {
        Some(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|v| v.starts_with(muthr_prefix) && *v != "muthr-services")
            .map(|v| v.to_string())
            .collect(),
        _ => Vec::new(),
    };

    if vms.is_empty() {
        println!("[WARN] No managed VMs found");
        return Ok(());
    }

    let is_tty = std::io::stdout().is_terminal();
    if !is_tty {
        for vm in &vms {
            let status = Command::new("limactl")
                .args(["ls", "-f", "{{.Status}}", vm])
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

            let project = vm.strip_prefix(muthr_prefix).unwrap_or(vm);
            let mount_point = format!("/muthr-{}", project);
            println!("  {:<30} {}  Mount: {}", vm, status, mount_point);
        }
    } else {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for vm in &vms {
            let status = Command::new("limactl")
                .args(["ls", "-f", "{{.Status}}", vm])
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

            let project = vm.strip_prefix(muthr_prefix).unwrap_or(vm);
            let mount_point = format!("/muthr-{}", project);
            rows.push(vec![vm.clone(), status, mount_point.to_string()]);
        }

        let headers = vec!["VM Name", "Status", "Mount Point"];
        crate::ui::select_table(&headers, rows);
    }

    println!("===============================================================================");
    Ok(())
}
