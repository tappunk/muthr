use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimaManifest {
    minimum_lima_version: String,
    vm_type: String,
    mount_type: String,
    images: Vec<LimaImage>,
    cpus: Option<u32>,
    memory: Option<String>,
    disk: Option<String>,
    mounts: Option<Vec<LimaMount>>,
    upgrade_packages: Option<bool>,
    containerd: Option<LimaContainerd>,
    ssh: Option<LimaSsh>,
    host_resolver: Option<LimaHostResolver>,
    provision: Option<Vec<LimaProvision>>,
    port_forwards: Option<Vec<LimaPortForward>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LimaImage {
    location: String,
    arch: String,
    digest: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimaMount {
    location: String,
    mount_point: String,
    writable: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LimaContainerd {
    user: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimaSsh {
    forward_agent: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimaHostResolver {
    enabled: Option<bool>,
    ipv6: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LimaProvision {
    mode: String,
    script: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimaPortForward {
    guest_port: u16,
    host_port: u16,
}

fn parse_manifest_yaml(content: &str) -> Result<LimaManifest, color_eyre::Report> {
    if content.len() > MAX_MANIFEST_BYTES {
        return Err(color_eyre::eyre::eyre!(
            "manifest exceeds max size: {} bytes",
            MAX_MANIFEST_BYTES
        ));
    }

    serde_yaml::from_str(content)
        .map_err(|e| color_eyre::eyre::eyre!("invalid manifest yaml: {}", e))
}

fn update_mount_placeholders(manifest: &mut LimaManifest, host_workspace: &str, guest_mount: &str) {
    if let Some(mounts) = manifest.mounts.as_mut() {
        for mount in mounts {
            if mount.location == "__WORKSPACE_ROOT__" {
                mount.location = host_workspace.to_string();
            }

            if mount.mount_point == "__MOUNT_POINT__" {
                mount.mount_point = guest_mount.to_string();
            }
        }
    }
}

fn serialize_manifest_yaml(manifest: &LimaManifest) -> Result<String, color_eyre::Report> {
    serde_yaml::to_string(manifest)
        .map_err(|e| color_eyre::eyre::eyre!("failed to serialize manifest yaml: {}", e))
}

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

fn project_name_suffix(seed: &str) -> String {
    let hash = seed
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |acc, b| {
            (acc ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
    format!("{:08x}", (hash & 0xffff_ffff) as u32)
}

fn validate_workspace_root(workspace: &Path, home: &Path) -> Result<(), color_eyre::Report> {
    if workspace == Path::new("/") {
        return Err(color_eyre::eyre::eyre!("workspace root cannot be '/'"));
    }
    if workspace == Path::new("/Users") {
        return Err(color_eyre::eyre::eyre!("workspace root cannot be '/Users'"));
    }
    if workspace == home {
        return Err(color_eyre::eyre::eyre!("workspace root cannot be '$HOME'"));
    }
    if !workspace.starts_with(home) {
        return Err(color_eyre::eyre::eyre!(
            "workspace root must be inside '$HOME'"
        ));
    }
    Ok(())
}

pub fn resolve_workspace_context() -> Result<(String, PathBuf, PathBuf), color_eyre::Report> {
    let current_dir = std::env::current_dir()?;
    let home = std::env::var("HOME")?;
    let canonical_current_dir = current_dir.canonicalize()?;

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
    if muthr_config_dir.exists()
        && let Ok(canonical_muthr_config_dir) = muthr_config_dir.canonicalize()
        && canonical_current_dir.starts_with(&canonical_muthr_config_dir)
    {
        return Ok(("muthr-config".to_string(), muthr_config_dir, current_dir));
    }

    let workspace_path = PathBuf::from(&workspace_root);
    if !workspace_path.exists() {
        eprintln!("error: workspace root '{}' does not exist", workspace_root);
        eprintln!(
            "info: create it with 'mkdir -p {}' or set MUTHR_WORKSPACE_ROOT",
            workspace_root
        );
        std::process::exit(66);
    }

    let canonical_workspace_path = workspace_path
        .canonicalize()
        .map_err(|e| color_eyre::eyre::eyre!("failed to canonicalize workspace root: {}", e))?;
    let canonical_home = PathBuf::from(&home)
        .canonicalize()
        .map_err(|e| color_eyre::eyre::eyre!("failed to canonicalize home directory: {}", e))?;

    validate_workspace_root(&canonical_workspace_path, &canonical_home)?;

    if canonical_current_dir == canonical_workspace_path {
        return Err(color_eyre::eyre::eyre!(
            "navigate into a project directory first"
        ));
    }

    let relative = canonical_current_dir
        .strip_prefix(&canonical_workspace_path)
        .map_err(|_| {
            color_eyre::eyre::eyre!("current directory is outside the configured workspace root")
        })?;
    let project_component = relative
        .components()
        .next()
        .ok_or_else(|| color_eyre::eyre::eyre!("invalid workspace path"))?
        .as_os_str();
    let project_name = project_component
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("invalid project name"))?;
    let sanitized_project_name = sanitize_project_name(project_name)
        .ok_or_else(|| color_eyre::eyre::eyre!("sanitized project name is empty"))?;
    let project_token = if sanitized_project_name == project_name {
        sanitized_project_name
    } else {
        format!(
            "{}-{}",
            sanitized_project_name,
            project_name_suffix(project_name)
        )
    };

    let project_folder = format!("muthr-{}", project_token);
    let mount_point = canonical_workspace_path.join(project_component);

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

pub async fn cleanup_untracked_vms(verbose: bool) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(home).join(".cache/muthr");

    let output = Command::new("limactl")
        .args(["ls", "-q"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success());

    let vms: Vec<String> = match output {
        Some(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|name| name.starts_with("muthr-") && *name != "muthr-services")
            .map(std::borrow::ToOwned::to_owned)
            .collect(),
        None => Vec::new(),
    };

    let now = std::time::SystemTime::now();

    for vm_name in vms {
        let marker = cache_dir.join(format!("{}-profiles", vm_name));
        if marker.exists() {
            continue;
        }

        let vm_store_dir = cache_dir
            .parent()
            .and_then(std::path::Path::parent)
            .map(|p| p.join(".lima").join(&vm_name));
        if let Some(store_dir) = vm_store_dir
            && let Ok(meta) = std::fs::metadata(&store_dir)
            && let Ok(modified) = meta.modified()
            && now.duration_since(modified).unwrap_or_default().as_secs() < 300
        {
            if verbose {
                eprintln!("info: skipping young sandbox vm {}", vm_name);
            }
            continue;
        }

        if vm_is_running(&vm_name).await {
            if verbose {
                eprintln!("info: skipping running untracked sandbox vm {}", vm_name);
            }
            continue;
        }

        if verbose {
            eprintln!("warning: removing untracked sandbox vm {}", vm_name);
        }
        delete_vm(&vm_name, true).await?;
    }

    Ok(())
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

pub async fn stop_vm_with_timeout(
    vm_name: &str,
    timeout_secs: u64,
) -> Result<bool, color_eyre::Report> {
    if !vm_is_running(vm_name).await {
        return Ok(false);
    }

    let status = Command::new("limactl")
        .args(["stop", vm_name])
        .status()
        .await;

    if !matches!(status, Ok(s) if s.success()) {
        eprintln!("warning: failed to stop vm {}, forcing stop", vm_name);
        let force_status = Command::new("limactl")
            .args(["stop", "--force", vm_name])
            .status()
            .await?;
        if !force_status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to force stop vm '{}'",
                vm_name
            ));
        }
    }

    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < timeout_secs {
        if !vm_is_running(vm_name).await {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    eprintln!(
        "warning: {} timed out after {}s, forcing stop",
        vm_name, timeout_secs
    );
    let force_status = Command::new("limactl")
        .args(["stop", "--force", vm_name])
        .status()
        .await?;
    if !force_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to force stop vm '{}'",
            vm_name
        ));
    }

    if vm_is_running(vm_name).await {
        return Err(color_eyre::eyre::eyre!("vm '{}' is still running", vm_name));
    }

    Ok(true)
}

pub async fn vm_stop(vm_name: &str) -> Result<(), color_eyre::Report> {
    let _ = stop_vm_with_timeout(vm_name, 30).await?;
    Ok(())
}

pub async fn protect_vm(vm_name: &str) -> Result<(), color_eyre::Report> {
    let status = Command::new("limactl")
        .args(["protect", vm_name])
        .output()
        .await?;

    if !status.status.success() {
        eprintln!("warning: failed to protect vm");
    }
    Ok(())
}

pub async fn unprotect_vm(vm_name: &str) -> Result<(), color_eyre::Report> {
    let status = Command::new("limactl")
        .args(["unprotect", vm_name])
        .output()
        .await?;

    if !status.status.success() {
        eprintln!("warning: failed to unprotect vm");
    }
    Ok(())
}

pub async fn delete_vm(vm_name: &str, force: bool) -> Result<(), color_eyre::Report> {
    if !force && !std::io::stdout().is_terminal() {
        eprintln!("error: terminal required for deletion, use --force to skip");
        std::process::exit(77);
    }

    eprintln!("info: deleting vm {}", vm_name);

    unprotect_vm(vm_name).await?;

    if vm_is_running(vm_name).await {
        vm_stop(vm_name).await?;
    }

    let status = Command::new("limactl")
        .args(["delete", vm_name])
        .output()
        .await?;

    if !status.status.success() {
        return Err(color_eyre::eyre::eyre!("failed to delete vm '{}'", vm_name));
    }

    let cache_dir = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".cache/muthr"))
        .unwrap_or_else(|| PathBuf::from("/tmp/muthr-cache"));

    let profile_cache = cache_dir.join(format!("{}-profiles", vm_name));
    if profile_cache.exists() {
        fs::remove_file(&profile_cache).await.ok();
    }

    eprintln!("info: deleted {}", vm_name);
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
            "provision script not found: {:?}",
            host_script
        ));
    }

    let mount_str = mount_point
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("workspace mount path contains invalid UTF-8"))?;

    fn validate_env_value(value: &str, field: &str) -> Result<(), color_eyre::Report> {
        if value.contains('\0') || value.contains('\n') || value.contains('\r') {
            return Err(color_eyre::eyre::eyre!(
                "invalid value for {}: contains control characters",
                field
            ));
        }
        Ok(())
    }

    fn validate_model_name(model_name: &str) -> Result<(), color_eyre::Report> {
        if model_name.is_empty() {
            return Err(color_eyre::eyre::eyre!("invalid model name: empty"));
        }
        if model_name
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/')))
        {
            return Err(color_eyre::eyre::eyre!(
                "invalid model name: unsupported characters"
            ));
        }
        Ok(())
    }

    eprintln!("info: provisioning: {}", script_name);
    let openai_url = format!("http://host.lima.internal:{}/v1", port);
    validate_env_value(&openai_url, "MUTHR_OPENAI_URL")?;
    validate_env_value(model_name, "MUTHR_MODEL_NAME")?;
    validate_model_name(model_name)?;
    validate_env_value(mount_str, "MUTHR_WORKSPACE_MOUNT")?;

    let host_lib_dir = host_script
        .parent()
        .ok_or_else(|| color_eyre::eyre::eyre!("invalid provision script path"))?
        .join("lib");
    if !host_lib_dir.is_dir() {
        return Err(color_eyre::eyre::eyre!(
            "provision library directory not found: {:?}",
            host_lib_dir
        ));
    }

    let guest_provision_dir = format!("/tmp/muthr-provision-{}", script_name);

    let mkdir_status = Command::new("limactl")
        .args([
            "shell",
            "--workdir",
            "/tmp",
            vm_name,
            "mkdir",
            "-p",
            &guest_provision_dir,
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !mkdir_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to prepare guest provision directory"
        ));
    }

    let guest_script_path = format!("{}/{}.sh", guest_provision_dir, script_name);
    let guest_lib_dir = format!("{}/lib", guest_provision_dir);

    let copy_script_status = Command::new("limactl")
        .args([
            "cp",
            host_script.to_str().ok_or_else(|| {
                color_eyre::eyre::eyre!("provision script path contains invalid UTF-8")
            })?,
            &format!("{}:{}", vm_name, guest_provision_dir),
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !copy_script_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to copy provision script into VM"
        ));
    }

    let copy_lib_status = Command::new("limactl")
        .args([
            "cp",
            "-r",
            host_lib_dir.to_str().ok_or_else(|| {
                color_eyre::eyre::eyre!("provision lib path contains invalid UTF-8")
            })?,
            &format!("{}:{}", vm_name, guest_lib_dir),
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !copy_lib_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to copy provision library into VM"
        ));
    }

    let mut child = Command::new("limactl")
        .args([
            "shell",
            "--workdir",
            "/tmp",
            vm_name,
            "bash",
            &guest_script_path,
        ])
        .env("MUTHR_OPENAI_URL", openai_url)
        .env("MUTHR_MODEL_NAME", model_name)
        .env("MUTHR_CTX_WINDOW", ctx_window.to_string())
        .env("MUTHR_WORKSPACE_MOUNT", mount_str)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;

    if let Some(ref mut stdin) = child.stdin {
        stdin.shutdown().await.ok();
    }
    std::mem::drop(child.stdin.take());

    let status = child.wait().await?;
    if !status.success() {
        return Err(color_eyre::eyre::eyre!("provision failed: {}", script_name));
    }

    eprintln!("info: provisioned: {}", script_name);
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
                "timed out waiting for VM '{}' to become ready",
                vm_name
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

pub async fn start(profile_name: String) -> Result<(), color_eyre::Report> {
    let (vm_name, project_root, workdir) = resolve_workspace_context()?;
    eprintln!("info: target: {}", vm_name);

    let home = std::env::var("HOME")?;
    let config_dir = PathBuf::from(&home).join(".config/muthr");
    let manifest_path = crate::catalog::resolve_manifest(&config_dir, &profile_name);

    if !manifest_path.exists() {
        eprintln!("error: manifest not found at {:?}", manifest_path);
        eprintln!("info: run 'muthr init'");
        std::process::exit(66);
    }

    let manifest_file = fs::File::open(&manifest_path).await?;
    let mut content = String::new();
    manifest_file
        .take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_string(&mut content)
        .await?;
    if content.len() > MAX_MANIFEST_BYTES {
        return Err(color_eyre::eyre::eyre!(
            "manifest exceeds max size: {} bytes",
            MAX_MANIFEST_BYTES
        ));
    }
    let host_workspace_mount = project_root
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("workspace path contains invalid UTF-8"))?;
    let guest_workspace_mount = "/workspace";
    let mut manifest_doc = parse_manifest_yaml(&content).map_err(|e| {
        color_eyre::eyre::eyre!(
            "failed to parse manifest '{}': {}",
            manifest_path.display(),
            e
        )
    })?;
    update_mount_placeholders(
        &mut manifest_doc,
        host_workspace_mount,
        guest_workspace_mount,
    );
    let expanded = serialize_manifest_yaml(&manifest_doc)?;

    if !vm_exists(&vm_name).await {
        eprintln!("info: creating vm {}", vm_name);
        let mut tmp_yaml = NamedTempFile::new()?;
        tmp_yaml.write_all(expanded.as_bytes())?;

        let mut create_cmd = Command::new("limactl");
        create_cmd
            .args(["create", "--name", &vm_name])
            .arg(tmp_yaml.path())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let create_status = create_cmd.status().await?;

        if !create_status.success() {
            return Err(color_eyre::eyre::eyre!("failed to create vm: {}", vm_name));
        }

        let mut start_cmd = Command::new("limactl");
        start_cmd
            .args(["start", &vm_name])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let start_status = start_cmd.status().await?;

        if !start_status.success() {
            return Err(color_eyre::eyre::eyre!("failed to start vm: {}", vm_name));
        }

        wait_for_vm_ready(&vm_name).await?;

        protect_vm(&vm_name).await?;
    } else if !vm_is_running(&vm_name).await {
        eprintln!("info: starting vm {}", vm_name);
        let mut start_cmd = Command::new("limactl");
        start_cmd
            .args(["start", &vm_name])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let status = start_cmd.status().await?;

        if !status.success() {
            return Err(color_eyre::eyre::eyre!("failed to start VM: {}", vm_name));
        }

        wait_for_vm_ready(&vm_name).await?;
    } else {
        eprintln!("info: vm already running");
    }

    let guest_workdir = match workdir.strip_prefix(&project_root) {
        Ok(relative_workdir) => PathBuf::from(guest_workspace_mount).join(relative_workdir),
        Err(_) => PathBuf::from(guest_workspace_mount),
    };
    let guest_workdir_str = guest_workdir
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("guest workdir contains invalid UTF-8"))?;

    let cfg = crate::config::load()?;
    let server_port = cfg.server_port.unwrap_or(8080);
    let resolve_default_model = || async {
        let preset_name_path = PathBuf::from(&home).join(".cache/muthr/active-preset-name");
        let preset_name = fs::read_to_string(&preset_name_path)
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let slot_name = preset_name
            .as_deref()
            .and_then(crate::preset::resolve_preset)
            .and_then(|path| crate::preset::parse_preset(&path).ok())
            .and_then(|preset| preset.slots.first().map(|slot| slot.name.clone()))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        slot_name.unwrap_or_else(|| "local-model".to_string())
    };

    let parsed_model =
        match crate::model::poll_loaded_model("127.0.0.1", server_port, 20, 1.0).await {
            Ok(model) => model,
            Err(err) => {
                eprintln!(
                    "warning: failed to poll loaded model from llama-server, using fallback: {}",
                    err
                );
                resolve_default_model().await
            }
        };
    let sanitized_model = std::path::Path::new(&parsed_model)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&parsed_model)
        .trim_end_matches(".gguf")
        .to_string();
    let active_model = if sanitized_model.trim().is_empty() {
        resolve_default_model().await
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

        eprintln!("info: applying profile: {}", profile_name);
        run_provision(
            &vm_name,
            &profile_name,
            &active_model,
            ctx_window,
            Path::new(guest_workspace_mount),
            server_port,
        )
        .await?;

        let existing_profiles = fs::read_to_string(&cache_dir).await.unwrap_or_default();
        if !existing_profiles.lines().any(|l| l.trim() == profile_name) {
            let mut existing = existing_profiles;
            if !existing.is_empty() && !existing.ends_with('\n') {
                existing.push('\n');
            }
            existing.push_str(&profile_name);
            existing.push('\n');
            let Some(cache_parent) = cache_dir.parent() else {
                return Err(color_eyre::eyre::eyre!("invalid profile cache path"));
            };
            fs::create_dir_all(cache_parent).await?;
            fs::write(&cache_dir, existing).await?;
        }

        eprintln!("info: launching workspace context");
        let target_args = match profile_name.as_str() {
            "opencode" => vec!["opencode"],
            "hermes-agent" => vec!["bash", "-l"],
            _ => vec![],
        };

        let mut args = vec!["--tty", "shell", "--workdir", guest_workdir_str, &vm_name];
        args.extend(target_args);

        let status = std::process::Command::new("limactl")
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;

        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "application session exited with error"
            ));
        }
    } else {
        let cache_dir = PathBuf::from(&home)
            .join(".cache/muthr")
            .join(format!("{}-profiles", vm_name));

        if !cache_dir.exists() {
            let Some(cache_parent) = cache_dir.parent() else {
                return Err(color_eyre::eyre::eyre!("invalid profile cache path"));
            };
            fs::create_dir_all(cache_parent).await?;
            fs::write(&cache_dir, "base\n").await?;
        }

        eprintln!("info: sandbox ready, launching shell");
        let status = std::process::Command::new("limactl")
            .args(["--tty", "shell", "--workdir", guest_workdir_str, &vm_name])
            .stdin(Stdio::inherit())
            .status()?;

        if !status.success() {
            return Err(color_eyre::eyre::eyre!("shell session exited with error"));
        }
    }

    Ok(())
}

pub async fn stop() -> Result<(), color_eyre::Report> {
    let (vm_name, _, _) = resolve_workspace_context()?;
    if !vm_exists(&vm_name).await {
        return Ok(());
    }
    let was_running = stop_vm_with_timeout(&vm_name, 30).await?;
    if !was_running {
        eprintln!("info: already stopped {}", vm_name);
        return Ok(());
    }
    eprintln!("info: stopped {}", vm_name);
    Ok(())
}

pub async fn ls(out_fmt: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let muthr_prefix = "muthr-";

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
        if out_fmt == crate::OutputFormat::Text {
            eprintln!("no managed vms");
        } else if out_fmt == crate::OutputFormat::Json {
            println!("[]");
        }
        return Ok(());
    }

    let mut entries: Vec<(String, String, String)> = Vec::new();
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
            .unwrap_or_else(|| "unknown".to_string());

        let project = vm.strip_prefix(muthr_prefix).unwrap_or(vm);
        let mount_point = format!("/muthr-{}", project);
        entries.push((vm.clone(), status, mount_point));
    }

    if out_fmt == crate::OutputFormat::Json {
        let payload: Vec<serde_json::Value> = entries
            .iter()
            .map(|(name, status, mount)| {
                serde_json::json!({"name": name, "status": status, "mount": mount})
            })
            .collect();
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if out_fmt == crate::OutputFormat::Ndjson {
        for (name, status, mount) in &entries {
            let payload = serde_json::json!({"name": name, "status": status, "mount": mount});
            println!("{}", serde_json::to_string(&payload)?);
        }
        return Ok(());
    }

    let is_tty = std::io::stdout().is_terminal();
    if !is_tty {
        for (vm, status, mount_point) in &entries {
            eprintln!("  {:<30} {}  mount: {}", vm, status, mount_point);
        }
    } else {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (vm, status, mount_point) in &entries {
            rows.push(vec![vm.clone(), status.clone(), mount_point.clone()]);
        }

        let headers = vec!["vm", "status", "mount"];
        crate::ui::select_table(&headers, &rows);
    }

    Ok(())
}
