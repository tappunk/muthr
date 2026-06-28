// Copyright 2026 tappunk
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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

fn project_name_suffix(seed: &str) -> String {
    let hash = seed
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |acc, b| {
            (acc ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
    format!("{:08x}", (hash & 0xffff_ffff) as u32)
}

fn run_container_session(
    container_id: &str,
    guest_workdir: &str,
    target_args: &[&str],
    requires_tty: bool,
) -> Result<(), color_eyre::Report> {
    let use_tty = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal();

    if requires_tty && !use_tty {
        return Err(color_eyre::eyre::eyre!(
            "interactive TTY is required for this profile; run from a local terminal"
        ));
    }

    let run_once = |tty: bool| -> Result<std::process::ExitStatus, color_eyre::Report> {
        let mut args = vec!["exec"];
        if tty {
            args.push("--interactive");
            args.push("--tty");
        }
        args.push("--workdir");
        args.push(guest_workdir);
        args.push(container_id);
        args.extend_from_slice(target_args);

        let status = std::process::Command::new("container")
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        Ok(status)
    };

    let status = run_once(use_tty)?;
    if status.success() {
        return Ok(());
    }

    if use_tty && !requires_tty {
        eprintln!("warning: interactive TTY launch failed; retrying without TTY");
        let retry_status = run_once(false)?;
        if retry_status.success() {
            return Ok(());
        }
    }

    Err(color_eyre::eyre::eyre!(
        "application session exited with error"
    ))
}

fn validate_workspace_root(workspace: &Path, home: &Path) -> Result<(), color_eyre::Report> {
    if workspace == Path::new("/") {
        return Err(color_eyre::eyre::eyre!("workspace root cannot be '/'"));
    }
    if workspace == Path::new("/Users") {
        return Err(color_eyre::eyre::eyre!("workspace root cannot be '/Users'"));
    }
    if workspace == home {
        return Err(color_eyre::eyre::eyre!(
            "security violation: workspace root cannot be the home directory; use a dedicated subdirectory (for example, ~/src)"
        ));
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

async fn container_list_all_json() -> Option<Vec<serde_json::Value>> {
    let output = Command::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }

    serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout).ok()
}

fn container_id_from_item(item: &serde_json::Value) -> Option<String> {
    item.get("id")
        .and_then(|v| v.as_str())
        .or_else(|| {
            item.get("configuration")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
        })
        .map(std::borrow::ToOwned::to_owned)
}

fn container_status_from_item(item: &serde_json::Value) -> Option<String> {
    item.get("status")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("state").and_then(|v| v.as_str()))
        .or_else(|| {
            item.get("status")
                .and_then(|v| v.get("state"))
                .and_then(|v| v.as_str())
        })
        .map(std::borrow::ToOwned::to_owned)
}

async fn container_exists(container_id: &str) -> bool {
    let Some(items) = container_list_all_json().await else {
        return false;
    };

    items
        .iter()
        .filter_map(container_id_from_item)
        .any(|id| id == container_id)
}

async fn container_is_running(container_id: &str) -> bool {
    let Some(items) = container_list_all_json().await else {
        return false;
    };

    items.iter().any(|item| {
        container_id_from_item(item).is_some_and(|id| id == container_id)
            && container_status_from_item(item)
                .map(|s| s.eq_ignore_ascii_case("running"))
                .unwrap_or(false)
    })
}

pub async fn sandbox_exists(container_id: &str) -> bool {
    container_exists(container_id).await
}

pub async fn cleanup_untracked_vms(verbose: bool) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(home).join(".cache/muthr");

    let Some(items) = container_list_all_json().await else {
        if verbose {
            eprintln!("warning: failed to list containers for cleanup");
        }
        return Ok(());
    };

    for item in items {
        let Some(container_id) = container_id_from_item(&item) else {
            continue;
        };
        if !container_id.starts_with("muthr-")
            || container_id == "muthr-services"
            || container_id == "muthr-searxng"
        {
            continue;
        }

        let profile_cache = cache_dir.join(format!("{}-profiles", container_id));
        if profile_cache.exists() {
            continue;
        }

        let running = container_status_from_item(&item)
            .map(|s| s.eq_ignore_ascii_case("running"))
            .unwrap_or(false);
        if running {
            if verbose {
                eprintln!(
                    "info: skipping running untracked sandbox container {}",
                    container_id
                );
            }
            continue;
        }

        if verbose {
            eprintln!(
                "warning: removing untracked sandbox container {}",
                container_id
            );
        }
        delete_container(&container_id, true).await?;
    }

    Ok(())
}

pub async fn sandbox_is_running(container_id: &str) -> bool {
    container_is_running(container_id).await
}

pub async fn stop_container_with_timeout(
    container_id: &str,
    timeout_secs: u64,
) -> Result<bool, color_eyre::Report> {
    if !container_is_running(container_id).await {
        return Ok(false);
    }

    let status = Command::new("container")
        .args(["stop", "--time", &timeout_secs.to_string(), container_id])
        .status()
        .await?;
    if !status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to stop container '{}'",
            container_id
        ));
    }

    Ok(true)
}

pub async fn stop_container_by_name(container_id: &str) -> Result<(), color_eyre::Report> {
    let _ = stop_container_with_timeout(container_id, 30).await?;
    Ok(())
}

pub async fn protect_vm(vm_name: &str) -> Result<(), color_eyre::Report> {
    let _ = vm_name;
    Ok(())
}

pub async fn unprotect_vm(vm_name: &str) -> Result<(), color_eyre::Report> {
    let _ = vm_name;
    Ok(())
}

pub async fn delete_sandbox(container_id: &str, force: bool) -> Result<(), color_eyre::Report> {
    delete_container(container_id, force).await
}

pub async fn run_provision_script(
    container_id: &str,
    script_name: &str,
    model_name: &str,
    ctx_window: u32,
    mount_point: &std::path::Path,
    port: u16,
) -> Result<(), color_eyre::Report> {
    run_provision_container(
        container_id,
        script_name,
        model_name,
        ctx_window,
        mount_point,
        port,
    )
    .await
}

async fn compute_specs_revision_hash(
    script_path: &Path,
    lib_dir: &Path,
) -> Result<String, color_eyre::Report> {
    let mut files = vec![script_path.to_path_buf()];
    files.extend(collect_regular_files_recursive(lib_dir)?);
    files.sort();

    let mut shasum_cmd = Command::new("shasum");
    shasum_cmd.args(["-a", "256"]);
    for file in &files {
        shasum_cmd.arg(file);
    }

    let output = shasum_cmd.output().await?;
    if !output.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to compute provision hash for script and library"
        ));
    }

    let mut second_pass = Command::new("shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = second_pass.stdin.take() {
        stdin.write_all(&output.stdout).await?;
        stdin.shutdown().await?;
    }
    let second_output = second_pass.wait_with_output().await?;
    if !second_output.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to finalize provision hash digest"
        ));
    }

    let stdout = String::from_utf8_lossy(&second_output.stdout);
    let hash = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| color_eyre::eyre::eyre!("invalid shasum output"))?;
    Ok(hash.to_string())
}

fn collect_regular_files_recursive(root: &Path) -> Result<Vec<PathBuf>, color_eyre::Report> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];

    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }

    Ok(files)
}

async fn resolve_active_model_and_ctx(home: &str, server_port: u16) -> (String, u32) {
    let resolve_preset_model_and_ctx = || async {
        let preset_name_path = PathBuf::from(home).join(".cache/muthr/active-preset-name-mlxcel");
        let preset_name = fs::read_to_string(&preset_name_path)
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let parsed = preset_name
            .as_deref()
            .and_then(crate::preset::resolve_preset)
            .and_then(|path| crate::preset::parse_preset(&path).ok())
            .and_then(|preset| {
                preset.slots.first().map(|slot| {
                    let ctx_hint = slot.max_output_tokens.unwrap_or(131072);
                    (slot.name.clone(), ctx_hint)
                })
            });

        parsed.unwrap_or_else(|| ("local-model".to_string(), 131072))
    };

    let (default_model, preset_ctx_hint) = resolve_preset_model_and_ctx().await;

    let parsed_model =
        match crate::model::poll_loaded_model("127.0.0.1", server_port, 20, 1.0).await {
            Ok(model) => model,
            Err(err) => {
                eprintln!(
                    "warning: failed to poll loaded model from mlxcel-server, using fallback: {}",
                    err
                );
                default_model.clone()
            }
        };
    let sanitized_model = if parsed_model.contains('/') || parsed_model.contains('\\') {
        std::path::Path::new(&parsed_model)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&parsed_model)
            .to_string()
    } else {
        parsed_model.clone()
    };
    let active_model = if sanitized_model.trim().is_empty() {
        default_model
    } else {
        sanitized_model
    };
    let model_ctx_window = crate::model::get_ctx_window("127.0.0.1", server_port)
        .await
        .unwrap_or(preset_ctx_hint);
    let ctx_window = std::cmp::max(model_ctx_window, preset_ctx_hint);

    (active_model, ctx_window)
}

async fn discover_container_gateway() -> Option<String> {
    let output = Command::new("container")
        .args(["network", "list", "--format", "json"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let entries = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout).ok()?;
    for entry in entries {
        let candidates = [
            entry.get("gateway"),
            entry.get("Gateway"),
            entry.get("status").and_then(|v| v.get("ipv4Gateway")),
            entry.get("status").and_then(|v| v.get("ipv6Gateway")),
            entry.get("status").and_then(|v| v.get("gateway")),
            entry.get("Status").and_then(|v| v.get("IPv4Gateway")),
            entry.get("Status").and_then(|v| v.get("IPv6Gateway")),
            entry.get("Status").and_then(|v| v.get("Gateway")),
            entry.get("ipam").and_then(|v| v.get("gateway")),
            entry.get("IPAM").and_then(|v| v.get("Gateway")),
            entry
                .get("subnets")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("gateway")),
            entry
                .get("Subnets")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("Gateway")),
        ];

        for candidate in candidates.into_iter().flatten() {
            if let Some(ip) = candidate.as_str()
                && !ip.trim().is_empty()
            {
                return Some(ip.trim().to_string());
            }
        }
    }

    None
}

async fn resolve_container_host_gateway() -> Result<String, color_eyre::Report> {
    let cfg = crate::config::load()?;
    let host = if let Some(configured) = cfg.container_host_gateway {
        configured.trim().to_string()
    } else if let Ok(env_host) = std::env::var("MUTHR_CONTAINER_HOST_GATEWAY") {
        env_host.trim().to_string()
    } else {
        discover_container_gateway()
            .await
            .ok_or_else(|| {
                color_eyre::eyre::eyre!(
                    "could not determine container host gateway; set MUTHR_CONTAINER_HOST_GATEWAY or container_host_gateway in config"
                )
            })?
    };

    if host.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "container host gateway resolved to an empty value"
        ));
    }

    Ok(host)
}

async fn backend_openai_url(port: u16) -> Result<String, color_eyre::Report> {
    let host = resolve_container_host_gateway().await?;

    Ok(format!("http://{}:{}/v1", host, port))
}

pub async fn start(profile_name: String) -> Result<(), color_eyre::Report> {
    start_container(profile_name).await
}

pub async fn stop() -> Result<(), color_eyre::Report> {
    stop_container().await
}

pub async fn ls(out_fmt: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    list_containers(out_fmt).await
}

async fn start_container(profile_name: String) -> Result<(), color_eyre::Report> {
    let (container_id, project_root, workdir) = resolve_workspace_context()?;
    eprintln!("info: target: {}", container_id);

    let mount_src = project_root
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("workspace path contains invalid UTF-8"))?;

    let exists = container_exists(&container_id).await;
    if !exists {
        eprintln!("info: creating container {}", container_id);
        let volume = format!("{}:/workspace", mount_src);
        let status = Command::new("container")
            .args([
                "create",
                "--name",
                &container_id,
                "--detach",
                "--volume",
                &volume,
                "--workdir",
                "/workspace",
                "debian:13-slim",
                "sh",
                "-lc",
                "while true; do sleep 3600; done",
            ])
            .status()
            .await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to create container '{}' (run 'container system start' if the service is not running)",
                container_id
            ));
        }
    }

    if !container_is_running(&container_id).await {
        let status = Command::new("container")
            .args(["start", &container_id])
            .status()
            .await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to start container '{}'",
                container_id
            ));
        }
    }

    ensure_container_runtime_baseline(&container_id).await?;

    let guest_workdir = match workdir.strip_prefix(&project_root) {
        Ok(relative_workdir) => PathBuf::from("/workspace").join(relative_workdir),
        Err(_) => PathBuf::from("/workspace"),
    };
    let guest_workdir_str = guest_workdir
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("guest workdir contains invalid UTF-8"))?;

    let home = std::env::var("HOME")?;
    let cfg = crate::config::load()?;
    let server_port = cfg.server_port.unwrap_or(8080);
    let (active_model, ctx_window) = resolve_active_model_and_ctx(&home, server_port).await;

    if profile_name != "base" {
        let cache_dir = PathBuf::from(&home)
            .join(".cache/muthr")
            .join(format!("{}-profiles", container_id));

        eprintln!("info: applying profile: {}", profile_name);
        run_provision_container(
            &container_id,
            &profile_name,
            &active_model,
            ctx_window,
            Path::new("/workspace"),
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
            _ => vec![],
        };

        let requires_tty = profile_name == "opencode";
        run_container_session(&container_id, guest_workdir_str, &target_args, requires_tty)?;

        return Ok(());
    }

    let cache_dir = PathBuf::from(&home)
        .join(".cache/muthr")
        .join(format!("{}-profiles", container_id));
    if !cache_dir.exists() {
        let Some(cache_parent) = cache_dir.parent() else {
            return Err(color_eyre::eyre::eyre!("invalid profile cache path"));
        };
        fs::create_dir_all(cache_parent).await?;
        fs::write(&cache_dir, "base\n").await?;
    }

    eprintln!("info: container ready, launching shell");
    run_container_session(&container_id, guest_workdir_str, &["sh"], false)
        .map_err(|_| color_eyre::eyre::eyre!("shell session exited with error"))?;

    Ok(())
}

async fn run_provision_container(
    container_id: &str,
    script_name: &str,
    model_name: &str,
    ctx_window: u32,
    mount_point: &std::path::Path,
    port: u16,
) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let host_script = PathBuf::from(&home).join(format!(
        ".config/muthr/sandbox.d/container/provision.d/{}.sh",
        script_name
    ));

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
    let host_gateway = resolve_container_host_gateway().await?;
    let openai_url = backend_openai_url(port).await?;
    let searxng_url = format!("http://{}:18766", host_gateway);
    validate_env_value(&openai_url, "MUTHR_OPENAI_URL")?;
    validate_env_value(&host_gateway, "MUTHR_CONTAINER_HOST_GATEWAY")?;
    validate_env_value(&searxng_url, "MUTHR_SEARXNG_URL")?;
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

    let specs_rev = compute_specs_revision_hash(&host_script, &host_lib_dir).await?;
    let guest_provision_dir = format!("/tmp/muthr-provision-{}", script_name);
    let guest_script_path = format!("{}/{}.sh", guest_provision_dir, script_name);

    let mkdir_status = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            &format!("mkdir -p '{}'", guest_provision_dir),
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

    let copy_script_status = Command::new("container")
        .args([
            "copy",
            host_script.to_str().ok_or_else(|| {
                color_eyre::eyre::eyre!("provision script path contains invalid UTF-8")
            })?,
            &format!("{}:{}", container_id, guest_script_path),
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !copy_script_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to copy provision script into container"
        ));
    }

    let copy_lib_status = Command::new("container")
        .args([
            "copy",
            host_lib_dir.to_str().ok_or_else(|| {
                color_eyre::eyre::eyre!("provision lib path contains invalid UTF-8")
            })?,
            &format!("{}:{}", container_id, guest_provision_dir),
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !copy_lib_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to copy provision library into container"
        ));
    }

    let mut child = Command::new("container")
        .args(["exec", "--workdir", "/tmp"])
        .arg("--env")
        .arg(format!("MUTHR_OPENAI_URL={}", openai_url))
        .arg("--env")
        .arg(format!("MUTHR_MODEL_NAME={}", model_name))
        .arg("--env")
        .arg(format!("MUTHR_CTX_WINDOW={}", ctx_window))
        .arg("--env")
        .arg(format!("MUTHR_WORKSPACE_MOUNT={}", mount_str))
        .arg("--env")
        .arg(format!("MUTHR_SPECS_REV={}", specs_rev))
        .arg("--env")
        .arg(format!("MUTHR_CONTAINER_HOST_GATEWAY={}", host_gateway))
        .arg("--env")
        .arg(format!("MUTHR_SEARXNG_URL={}", searxng_url))
        .arg(container_id)
        .arg("bash")
        .arg(&guest_script_path)
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

async fn ensure_container_runtime_baseline(container_id: &str) -> Result<(), color_eyre::Report> {
    let marker = "/var/lib/muthr/container-baseline-ready";
    let has_marker = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            &format!("test -f {}", marker),
        ])
        .status()
        .await?;
    if has_marker.success() {
        return Ok(());
    }

    let install_status = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq bash curl ca-certificates sudo git",
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !install_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to install container baseline dependencies"
        ));
    }

    let marker_status = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            "mkdir -p /var/lib/muthr && touch /var/lib/muthr/container-baseline-ready",
        ])
        .status()
        .await?;
    if !marker_status.success() {
        eprintln!("warning: failed to persist container baseline marker");
    }

    Ok(())
}

async fn stop_container() -> Result<(), color_eyre::Report> {
    let (container_id, _, _) = resolve_workspace_context()?;
    if !container_exists(&container_id).await {
        return Ok(());
    }

    if !container_is_running(&container_id).await {
        eprintln!("info: already stopped {}", container_id);
        return Ok(());
    }

    let status = Command::new("container")
        .args(["stop", &container_id])
        .status()
        .await?;
    if !status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to stop container '{}'",
            container_id
        ));
    }

    eprintln!("info: stopped {}", container_id);
    Ok(())
}

async fn delete_container(container_id: &str, force: bool) -> Result<(), color_eyre::Report> {
    if !force && !std::io::stdout().is_terminal() {
        eprintln!("error: terminal required for deletion, use --force to skip");
        std::process::exit(77);
    }

    if !container_exists(container_id).await {
        return Ok(());
    }

    if container_is_running(container_id).await && !force {
        let stop_status = Command::new("container")
            .args(["stop", container_id])
            .status()
            .await?;
        if !stop_status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to stop container '{}' before deletion",
                container_id
            ));
        }
    }

    let mut args: Vec<&str> = vec!["delete"];
    if force {
        args.push("--force");
    }
    args.push(container_id);

    let status = Command::new("container").args(args).status().await?;
    if !status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to delete container '{}'",
            container_id
        ));
    }

    eprintln!("info: deleted {}", container_id);
    Ok(())
}

async fn list_containers(out_fmt: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let Some(items) = container_list_all_json().await else {
        return Err(color_eyre::eyre::eyre!(
            "failed to list containers (run 'container system start' if the service is not running)"
        ));
    };

    let mut entries: Vec<(String, String, String)> = items
        .iter()
        .filter_map(|item| {
            let id = container_id_from_item(item)?;
            if !id.starts_with("muthr-") || id == "muthr-services" || id == "muthr-searxng" {
                return None;
            }
            let status = container_status_from_item(item).unwrap_or_else(|| "unknown".to_string());
            Some((id.clone(), status, "/workspace".to_string()))
        })
        .collect();

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    if entries.is_empty() {
        if out_fmt == crate::OutputFormat::Text {
            eprintln!("no managed sandbox containers");
        } else if out_fmt == crate::OutputFormat::Json {
            println!("[]");
        }
        return Ok(());
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

    for (name, status, mount) in &entries {
        eprintln!("  {:<30} {}  mount: {}", name, status, mount);
    }

    Ok(())
}
