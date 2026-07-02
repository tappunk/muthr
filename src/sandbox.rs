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

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::signal::unix::{SignalKind, signal};

const SAFE_ENV_ALLOWLIST: &[&str] = &["TERM", "COLORTERM", "COLUMNS", "LINES"];
const NATIVE_PLATFORM: &str = "linux/arm64";

#[derive(serde::Deserialize, Debug, Clone, Default)]
struct SandboxManifest {
    image: Option<String>,
    resources: Option<ResourceLimits>,
    mounts: Option<HashMap<String, String>>,
    security: Option<SecurityCaps>,
}

#[derive(serde::Deserialize, Debug, Clone, Default)]
struct ResourceLimits {
    cpus: Option<u32>,
    memory: Option<String>,
}

#[derive(serde::Deserialize, Debug, Clone, Default)]
struct SecurityCaps {
    network: Option<String>,
    workspace_mode: Option<String>,
}

#[derive(Debug, Clone)]
struct MountSpec {
    host: String,
    guest: String,
    read_only: bool,
}

#[derive(Debug, Clone)]
struct ContainerProfileSettings {
    image: String,
    workspace_guest_path: String,
    mounts: Vec<MountSpec>,
    network_none: bool,
    cpus: Option<u32>,
    memory: Option<String>,
    uses_golden_image: bool,
}

#[derive(Debug, Clone)]
struct AuditLogger {
    path: PathBuf,
}

struct TerminalStateGuard {
    fds: Vec<(i32, libc::termios)>,
}

#[derive(Clone, Copy)]
struct TerminalDimensions {
    rows: u16,
    cols: u16,
}

impl TerminalStateGuard {
    fn capture() -> Self {
        let mut fds = Vec::new();

        for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            // SAFETY: `fd` values are valid process file descriptor integers.
            let is_tty = unsafe { libc::isatty(fd) } == 1;
            if !is_tty {
                continue;
            }

            // SAFETY: zeroed `termios` is immediately initialized by `tcgetattr` on success.
            let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
            // SAFETY: `fd` is a tty and `termios` points to valid writable memory.
            let ok = unsafe { libc::tcgetattr(fd, &mut termios as *mut libc::termios) } == 0;
            if ok {
                fds.push((fd, termios));
            }
        }

        Self { fds }
    }
}

impl Drop for TerminalStateGuard {
    fn drop(&mut self) {
        for (fd, termios) in &self.fds {
            // SAFETY: `fd` and `termios` values were captured from successful `tcgetattr` calls.
            let _ = unsafe { libc::tcsetattr(*fd, libc::TCSANOW, termios as *const libc::termios) };
        }
    }
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

impl AuditLogger {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn write_event(&self, event: serde_json::Value) -> Result<(), color_eyre::Report> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let serialized = serde_json::to_string(&event)?;
        writeln!(file, "{}", serialized)?;
        Ok(())
    }
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn default_audit_log_path(container_id: &str) -> Result<PathBuf, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let audit_dir = PathBuf::from(home).join(".cache/muthr/audit");
    std::fs::create_dir_all(&audit_dir)?;
    let ts = now_unix_seconds();
    Ok(audit_dir.join(format!("{}-{}.ndjson", ts, container_id)))
}

fn resolve_audit_logger(
    audit_log: Option<String>,
    container_id: &str,
) -> Result<Option<AuditLogger>, color_eyre::Report> {
    let Some(path_str) = audit_log else {
        return Ok(None);
    };

    let path = if path_str.trim().is_empty() {
        default_audit_log_path(container_id)?
    } else {
        PathBuf::from(path_str)
    };

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    Ok(Some(AuditLogger::new(path)))
}

fn runtime_env_summary(envs: &[(String, String)]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (k, v) in envs {
        if matches!(
            k.as_str(),
            "MUTHR_INFERENCE_URL"
                | "MUTHR_MCP_BRIDGE_URL"
                | "MUTHR_SEARXNG_URL"
                | "MUTHR_MODEL_NAME"
                | "MUTHR_ENGINE_RUNTIME"
        ) {
            map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
    }
    serde_json::Value::Object(map)
}

fn run_container_session(
    container_id: &str,
    guest_workdir: &str,
    injected_envs: &[(String, String)],
    target_args: &[&str],
    requires_tty: bool,
    audit: Option<&AuditLogger>,
) -> Result<(), color_eyre::Report> {
    let use_tty = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal();

    if requires_tty && !use_tty {
        return Err(color_eyre::eyre::eyre!(
            "interactive TTY is required for this profile; run from a local terminal"
        ));
    }

    let started_at = Instant::now();
    if let Some(logger) = audit {
        logger.write_event(serde_json::json!({
            "event": "session_start",
            "ts": now_unix_seconds(),
            "container_id": container_id,
            "workdir": guest_workdir,
            "tty": use_tty,
            "requires_tty": requires_tty,
            "runtime_env": runtime_env_summary(injected_envs),
            "target_args": target_args,
        }))?;
    }

    let run_once = |tty: bool| -> Result<std::process::ExitStatus, color_eyre::Report> {
        let _terminal_state_guard = tty.then(TerminalStateGuard::capture);

        let mut args: Vec<String> = vec!["exec".to_string()];
        if tty {
            args.push("--interactive".to_string());
            args.push("--tty".to_string());
        }
        args.push("--workdir".to_string());
        args.push(guest_workdir.to_string());
        args.push("--user".to_string());
        args.push("muthr".to_string());
        for (key, value) in injected_envs {
            args.push("--env".to_string());
            args.push(format!("{}={}", key, value));
        }
        args.push(container_id.to_string());
        args.extend(target_args.iter().map(|s| s.to_string()));

        if let Some(logger) = audit {
            logger.write_event(serde_json::json!({
                "event": "exec_invocation",
                "ts": now_unix_seconds(),
                "container_id": container_id,
                "argv": args,
                "tty": tty,
            }))?;
        }

        let status = std::process::Command::new("container")
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        Ok(status)
    };

    let mut status = run_once(use_tty)?;
    if status.success() {
        if let Some(logger) = audit {
            logger.write_event(serde_json::json!({
                "event": "session_exit",
                "ts": now_unix_seconds(),
                "container_id": container_id,
                "exit_code": status.code(),
                "duration_ms": started_at.elapsed().as_millis(),
            }))?;
        }
        return Ok(());
    }

    if use_tty && !requires_tty {
        eprintln!("warning: interactive TTY launch failed; retrying without TTY");
        status = run_once(false)?;
        if status.success() {
            if let Some(logger) = audit {
                logger.write_event(serde_json::json!({
                    "event": "session_exit",
                    "ts": now_unix_seconds(),
                    "container_id": container_id,
                    "exit_code": status.code(),
                    "duration_ms": started_at.elapsed().as_millis(),
                }))?;
            }
            return Ok(());
        }
    }

    if let Some(logger) = audit {
        logger.write_event(serde_json::json!({
            "event": "session_exit",
            "ts": now_unix_seconds(),
            "container_id": container_id,
            "exit_code": status.code(),
            "duration_ms": started_at.elapsed().as_millis(),
        }))?;
    }

    Err(color_eyre::eyre::eyre!(
        "application session exited with error"
    ))
}

fn parse_explicit_env(input: &str) -> Result<(String, String), color_eyre::Report> {
    let Some((key, value)) = input.split_once('=') else {
        return Err(color_eyre::eyre::eyre!(
            "invalid --env value '{}': expected KEY=VALUE",
            input
        ));
    };

    if key.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "invalid --env value '{}': key cannot be empty",
            input
        ));
    }

    if key
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(color_eyre::eyre::eyre!(
            "invalid --env key '{}': use [A-Za-z0-9_] only",
            key
        ));
    }

    if value.contains('\0') || value.contains('\n') || value.contains('\r') {
        return Err(color_eyre::eyre::eyre!(
            "invalid --env value for '{}': contains control characters",
            key
        ));
    }

    Ok((key.to_string(), value.to_string()))
}

fn expand_home_path(input: &str, home: &str) -> String {
    input
        .strip_prefix("~/")
        .map(|tail| format!("{}/{}", home, tail))
        .unwrap_or_else(|| input.to_string())
}

fn parse_guest_mount_target(value: &str) -> Result<(String, bool), color_eyre::Report> {
    let raw = value.trim();
    if raw.is_empty() {
        return Err(color_eyre::eyre::eyre!("invalid guest mount target: empty"));
    }

    let mut read_only = false;
    let mut guest = raw;
    if let Some(stripped) = raw.strip_suffix(":ro") {
        read_only = true;
        guest = stripped;
    }

    if !guest.starts_with('/') {
        return Err(color_eyre::eyre::eyre!(
            "invalid guest mount target '{}' (must be absolute)",
            value
        ));
    }

    Ok((guest.to_string(), read_only))
}

async fn load_profile_manifest(
    profile_name: &str,
) -> Result<Option<SandboxManifest>, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let config_dir = PathBuf::from(&home).join(".config/muthr");
    let manifest_path = crate::catalog::resolve_manifest(&config_dir, profile_name);
    if !manifest_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&manifest_path).await?;
    let manifest: SandboxManifest = serde_yaml::from_str(&content)?;
    Ok(Some(manifest))
}

async fn container_profile_settings(
    profile_name: &str,
    project_root: &Path,
) -> Result<ContainerProfileSettings, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let mount_src = project_root
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("workspace path contains invalid UTF-8"))?
        .to_string();

    let mut settings = ContainerProfileSettings {
        image: "debian:13-slim".to_string(),
        workspace_guest_path: "/workspace".to_string(),
        mounts: Vec::new(),
        network_none: false,
        cpus: None,
        memory: None,
        uses_golden_image: false,
    };

    if let Some(manifest) = load_profile_manifest(profile_name).await? {
        if let Some(image) = manifest.image
            && !image.trim().is_empty()
        {
            settings.image = image.trim().to_string();
        }

        if let Some(resources) = manifest.resources {
            settings.cpus = resources.cpus;
            settings.memory = resources.memory;
        }

        if let Some(security) = manifest.security {
            if let Some(network) = security.network {
                let mode = network.trim().to_ascii_lowercase();
                if mode == "none" || mode == "restricted" {
                    settings.network_none = true;
                }
            }

            if let Some(workspace_mode) = security.workspace_mode {
                let mode = workspace_mode.trim().to_ascii_lowercase();
                if mode == "overlay" {
                    eprintln!(
                        "warning: workspace_mode=overlay requested; overlay semantics are not implemented yet, using direct mount"
                    );
                }
            }
        }

        if let Some(mounts) = manifest.mounts {
            for (host_key, guest_value) in mounts {
                if host_key == "workspace" {
                    if guest_value.trim().starts_with('/') {
                        settings.workspace_guest_path = guest_value.trim().to_string();
                    }
                    continue;
                }

                let host = expand_home_path(host_key.trim(), &home);
                if host.is_empty() {
                    continue;
                }

                let (guest, read_only) = parse_guest_mount_target(&guest_value)?;
                settings.mounts.push(MountSpec {
                    host,
                    guest,
                    read_only,
                });
            }
        }
    }

    settings.mounts.insert(
        0,
        MountSpec {
            host: mount_src,
            guest: settings.workspace_guest_path.clone(),
            read_only: false,
        },
    );

    let golden_tag = golden_image_tag(profile_name);
    if image_exists(&golden_tag).await {
        settings.image = golden_tag;
        settings.uses_golden_image = true;
    }

    Ok(settings)
}

fn golden_image_tag(profile_name: &str) -> String {
    format!("muthr-profile-{}:latest", profile_name)
}

async fn image_exists(image: &str) -> bool {
    Command::new("container")
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

fn create_args_for_settings(
    container_id: &str,
    settings: &ContainerProfileSettings,
) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        "--name".to_string(),
        container_id.to_string(),
        "--platform".to_string(),
        NATIVE_PLATFORM.to_string(),
        "--detach".to_string(),
        "--label".to_string(),
        "muthr.managed=true".to_string(),
        "--label".to_string(),
        "muthr.owner=project".to_string(),
        "--label".to_string(),
        "muthr.profile.base=true".to_string(),
    ];

    if settings.network_none {
        args.push("--network".to_string());
        args.push("none".to_string());
    }

    if let Some(cpus) = settings.cpus {
        args.push(format!("--cpus={}", cpus));
    }

    if let Some(memory) = &settings.memory
        && !memory.trim().is_empty()
    {
        args.push(format!("--memory={}", memory.trim()));
    }

    for mount in &settings.mounts {
        args.push("--volume".to_string());
        if mount.read_only {
            args.push(format!("{}:{}:ro", mount.host, mount.guest));
        } else {
            args.push(format!("{}:{}", mount.host, mount.guest));
        }
    }

    args.push("--workdir".to_string());
    args.push(settings.workspace_guest_path.clone());
    args.push(settings.image.clone());
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push("while true; do sleep 3600; done".to_string());
    args
}

fn host_uid_gid() -> (u32, u32) {
    // SAFETY: reading effective uid/gid for current process is thread-safe.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: reading effective uid/gid for current process is thread-safe.
    let gid = unsafe { libc::getegid() };
    (uid, gid)
}

fn get_current_tty_dimensions() -> Option<TerminalDimensions> {
    // SAFETY: zeroed winsize is initialized by ioctl on success.
    let mut ws = unsafe { std::mem::zeroed::<libc::winsize>() };
    // SAFETY: STDOUT fd is valid for current process; ws points to writable memory.
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
    if !ok || ws.ws_row == 0 || ws.ws_col == 0 {
        return None;
    }

    Some(TerminalDimensions {
        rows: ws.ws_row,
        cols: ws.ws_col,
    })
}

async fn resize_container_pty(container_id: &str, dims: TerminalDimensions) {
    let _ = Command::new("container")
        .args([
            "resize",
            container_id,
            &dims.rows.to_string(),
            &dims.cols.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

async fn sync_container_user_ids(container_id: &str) {
    let (uid, gid) = host_uid_gid();
    let script = format!(
        "if id -u muthr >/dev/null 2>&1; then \
CURRENT_UID=\"$(id -u muthr)\"; CURRENT_GID=\"$(id -g muthr)\"; \
if [ \"$CURRENT_GID\" != \"{gid}\" ]; then groupmod -o -g {gid} muthr >/dev/null 2>&1 || true; fi; \
if [ \"$CURRENT_UID\" != \"{uid}\" ]; then usermod -o -u {uid} -g {gid} muthr >/dev/null 2>&1 || true; fi; \
chown -h muthr:muthr /home/muthr >/dev/null 2>&1 || true; \
fi"
    );

    let status = Command::new("container")
        .args(["exec", container_id, "sh", "-lc", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    if let Ok(exit) = status
        && !exit.success()
    {
        eprintln!(
            "warning: failed to synchronize sandbox uid/gid mapping for '{}'",
            container_id
        );
    }
}

async fn ensure_container_infrastructure(
    container_id: &str,
    project_root: &Path,
) -> Result<ContainerProfileSettings, color_eyre::Report> {
    let settings = container_profile_settings("base", project_root).await?;

    if !container_exists(container_id).await {
        eprintln!("info: creating container {}", container_id);
        let args = create_args_for_settings(container_id, &settings);
        let status = Command::new("container").args(&args).status().await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to create container '{}' (run 'container system start' if the service is not running)",
                container_id
            ));
        }
    }

    if !container_is_running(container_id).await {
        let status = Command::new("container")
            .args(["start", container_id])
            .status()
            .await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to start container '{}'",
                container_id
            ));
        }
    }

    ensure_container_runtime_baseline(container_id).await?;
    sync_container_user_ids(container_id).await;
    Ok(settings)
}

async fn update_container_network_mode(
    container_id: &str,
    mode: &str,
) -> Result<(), color_eyre::Report> {
    let support_check = Command::new("container")
        .args(["help", "update"])
        .output()
        .await?;

    if !support_check.status.success() {
        let stderr = String::from_utf8_lossy(&support_check.stderr).to_ascii_lowercase();
        if stderr.contains("unknown command")
            || stderr.contains("not found")
            || stderr.contains("plugin")
        {
            eprintln!(
                "warning: container CLI does not support 'update'; continuing without deferred network isolation for '{}' (mode '{}')",
                container_id, mode
            );
            return Ok(());
        }

        return Err(color_eyre::eyre::eyre!(
            "failed to verify container 'update' command support for '{}'",
            container_id
        ));
    }

    let output = Command::new("container")
        .args(["update", container_id, "--network", mode])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("unknown command")
            || stderr.contains("not found")
            || stderr.contains("plugin")
        {
            eprintln!(
                "warning: container update plugin unavailable; continuing without deferred network isolation for '{}' (mode '{}')",
                container_id, mode
            );
            return Ok(());
        }

        return Err(color_eyre::eyre::eyre!(
            "failed to set network mode '{}' for container '{}'",
            mode,
            container_id
        ));
    }

    Ok(())
}

pub async fn shell(
    profile: Option<String>,
    command: Option<String>,
    no_tty: bool,
    explicit_envs: Vec<String>,
    audit_log: Option<String>,
) -> Result<(), color_eyre::Report> {
    let use_tty = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal();
    let requires_tty = !no_tty;

    if requires_tty && !use_tty {
        return Err(color_eyre::eyre::eyre!(
            "interactive TTY is required; use --no-tty for non-interactive commands"
        ));
    }

    let parsed_envs: Vec<(String, String)> = explicit_envs
        .iter()
        .map(|entry| parse_explicit_env(entry))
        .collect::<Result<Vec<_>, _>>()?;

    let _lock =
        crate::lifecycle::acquire("container-lifecycle", std::time::Duration::from_secs(20))
            .await?;

    let (container_id, project_root, workdir) = resolve_workspace_context()?;
    if container_id == "muthr-config" {
        return Err(color_eyre::eyre::eyre!(
            "sandbox shell is only available inside a project directory"
        ));
    }

    let settings = ensure_container_infrastructure(&container_id, &project_root).await?;
    let audit = resolve_audit_logger(audit_log, &container_id)?;
    let home = std::env::var("HOME")?;

    let cfg = crate::config::load()?;
    let server_port = cfg.server_port.unwrap_or(8080);
    let engine_name = cfg.default_engine_runtime.as_deref().unwrap_or("mlxcel");
    let (active_model, ctx_window) =
        resolve_active_model_and_ctx(&home, server_port, engine_name).await;
    let runtime_envs =
        runtime_env_contract(&container_id, server_port, engine_name, &active_model).await?;

    if let Some(profile_name) = profile.as_deref() {
        if profile_name != "base" {
            let profile_settings = container_profile_settings(profile_name, &project_root).await?;
            let deferred_network_isolation = profile_settings.network_none;

            let cache_dir = PathBuf::from(&home)
                .join(".cache/muthr")
                .join(format!("{}-profiles", container_id));

            eprintln!("info: applying profile: {}", profile_name);

            let provision_result = run_provision_container(
                &container_id,
                profile_name,
                engine_name,
                &active_model,
                ctx_window,
                Path::new(&settings.workspace_guest_path),
                server_port,
            )
            .await;

            if deferred_network_isolation {
                eprintln!("info: sealing sandbox boundary -> cutting off network access");
                update_container_network_mode(&container_id, "none").await?;
            }

            provision_result?;

            let existing_profiles = fs::read_to_string(&cache_dir).await.unwrap_or_default();
            if !existing_profiles.lines().any(|l| l.trim() == profile_name) {
                let mut existing = existing_profiles;
                if !existing.is_empty() && !existing.ends_with('\n') {
                    existing.push('\n');
                }
                existing.push_str(profile_name);
                existing.push('\n');
                let Some(cache_parent) = cache_dir.parent() else {
                    return Err(color_eyre::eyre::eyre!("invalid profile cache path"));
                };
                fs::create_dir_all(cache_parent).await?;
                fs::write(&cache_dir, existing).await?;
            }
        }

        mark_container_profile(&container_id, profile_name).await;
    } else {
        mark_container_profile(&container_id, "base").await;
    }

    let guest_workdir = match workdir.strip_prefix(&project_root) {
        Ok(relative_workdir) => {
            PathBuf::from(&settings.workspace_guest_path).join(relative_workdir)
        }
        Err(_) => PathBuf::from(&settings.workspace_guest_path),
    };
    let guest_workdir_str = guest_workdir
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("guest workdir contains invalid UTF-8"))?;

    let mut args: Vec<String> = vec!["exec".to_string()];
    if use_tty && requires_tty {
        args.push("--interactive".to_string());
        args.push("--tty".to_string());
    }
    args.push("--workdir".to_string());
    args.push(guest_workdir_str.to_string());
    args.push("--user".to_string());
    args.push("muthr".to_string());

    for key in SAFE_ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(key) {
            args.push("--env".to_string());
            args.push(format!("{}={}", key, value));
        }
    }

    for (key, value) in &runtime_envs {
        args.push("--env".to_string());
        args.push(format!("{}={}", key, value));
    }

    for (key, value) in parsed_envs {
        args.push("--env".to_string());
        args.push(format!("{}={}", key, value));
    }

    args.push(container_id.clone());

    match command {
        Some(cmd) => {
            args.push("bash".to_string());
            args.push("-lc".to_string());
            args.push(cmd);
        }
        None => {
            args.push("bash".to_string());
            args.push("-l".to_string());
        }
    }

    let _terminal_state_guard = (use_tty && requires_tty).then(TerminalStateGuard::capture);
    if let Some(logger) = &audit {
        logger.write_event(serde_json::json!({
            "event": "session_start",
            "ts": now_unix_seconds(),
            "container_id": container_id,
            "workdir": guest_workdir_str,
            "tty": use_tty && requires_tty,
            "requires_tty": requires_tty,
            "runtime_env": runtime_env_summary(&runtime_envs),
        }))?;
        logger.write_event(serde_json::json!({
            "event": "exec_invocation",
            "ts": now_unix_seconds(),
            "container_id": container_id,
            "argv": args,
            "tty": use_tty && requires_tty,
            "runtime_env": runtime_env_summary(&runtime_envs),
        }))?;
    }
    let mut child_cmd = Command::new("container");
    child_cmd.args(&args);
    if use_tty && requires_tty {
        child_cmd
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    } else {
        child_cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }

    let mut child = child_cmd.spawn()?;

    let resize_task = if use_tty && requires_tty {
        let resize_container_id = container_id.clone();

        if let Some(initial_dims) = get_current_tty_dimensions() {
            resize_container_pty(&resize_container_id, initial_dims).await;
        }

        Some(tokio::spawn(async move {
            let Ok(mut sigwinch) = signal(SignalKind::from_raw(libc::SIGWINCH)) else {
                return;
            };

            while sigwinch.recv().await.is_some() {
                if let Some(dims) = get_current_tty_dimensions() {
                    resize_container_pty(&resize_container_id, dims).await;
                }
            }
        }))
    } else {
        None
    };

    let status = if use_tty && requires_tty {
        child.wait().await?
    } else {
        let output = child.wait_with_output().await?;
        if !output.stdout.is_empty() {
            let mut stdout = tokio::io::stdout();
            stdout.write_all(&output.stdout).await?;
            stdout.flush().await?;
        }
        if !output.stderr.is_empty() {
            let mut stderr = tokio::io::stderr();
            stderr.write_all(&output.stderr).await?;
            stderr.flush().await?;
        }
        output.status
    };

    if let Some(logger) = &audit {
        logger.write_event(serde_json::json!({
            "event": "session_exit",
            "ts": now_unix_seconds(),
            "container_id": container_id,
            "exit_code": status.code(),
        }))?;
    }

    if let Some(task) = resize_task {
        task.abort();
    }

    if status.success() {
        return Ok(());
    }

    std::process::exit(status.code().unwrap_or(1));
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
    let canonical_workspace_path = match workspace_path.canonicalize() {
        Ok(path) => path,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("error: workspace root '{}' does not exist", workspace_root);
            eprintln!(
                "info: create it with 'mkdir -p {}' or set MUTHR_WORKSPACE_ROOT",
                workspace_root
            );
            std::process::exit(66);
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(color_eyre::eyre::eyre!(
                "permission denied resolving workspace root '{}': {}",
                workspace_root,
                err
            ));
        }
        Err(err) => {
            return Err(color_eyre::eyre::eyre!(
                "failed to canonicalize workspace root '{}': {}",
                workspace_root,
                err
            ));
        }
    };
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

#[derive(serde::Deserialize, Debug, Clone)]
struct ContainerListItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, alias = "ID", alias = "Id")]
    id_alias: Option<String>,
    #[serde(default)]
    status: Option<serde_json::Value>,
    #[serde(default, alias = "Status")]
    status_alias: Option<serde_json::Value>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default, alias = "State")]
    state_alias: Option<String>,
    #[serde(default)]
    configuration: Option<ContainerConfiguration>,
    #[serde(default, alias = "Configuration", alias = "config", alias = "Config")]
    configuration_alias: Option<ContainerConfiguration>,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct ContainerConfiguration {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, alias = "ID", alias = "Id")]
    id_alias: Option<String>,
    #[serde(default)]
    labels: Option<HashMap<String, serde_json::Value>>,
    #[serde(default, alias = "Labels")]
    labels_alias: Option<HashMap<String, serde_json::Value>>,
}

impl ContainerListItem {
    fn id(&self) -> Option<&str> {
        self.id.as_deref().or(self.id_alias.as_deref()).or_else(|| {
            self.configuration_ref()
                .and_then(|c| c.id.as_deref().or(c.id_alias.as_deref()))
        })
    }

    fn status_state(&self) -> Option<&str> {
        self.status
            .as_ref()
            .or(self.status_alias.as_ref())
            .and_then(|v| {
                if let Some(s) = v.as_str() {
                    return Some(s);
                }
                v.get("state")
                    .or_else(|| v.get("State"))
                    .and_then(|s| s.as_str())
            })
            .or(self.state.as_deref())
            .or(self.state_alias.as_deref())
    }

    fn label(&self, key: &str) -> Option<&str> {
        self.labels_ref()
            .and_then(|labels| labels.get(key))
            .and_then(|v| v.as_str())
    }

    fn has_profile_label(&self) -> bool {
        self.labels_ref()
            .is_some_and(|labels| labels.keys().any(|k| k.starts_with("muthr.profile.")))
    }

    fn configuration_ref(&self) -> Option<&ContainerConfiguration> {
        self.configuration
            .as_ref()
            .or(self.configuration_alias.as_ref())
    }

    fn labels_ref(&self) -> Option<&HashMap<String, serde_json::Value>> {
        self.configuration_ref()
            .and_then(|c| c.labels.as_ref().or(c.labels_alias.as_ref()))
    }
}

async fn container_list_all() -> Option<Vec<ContainerListItem>> {
    let output = Command::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }

    serde_json::from_slice::<Vec<ContainerListItem>>(&output.stdout).ok()
}

fn container_id_from_item(item: &ContainerListItem) -> Option<String> {
    item.id().map(std::borrow::ToOwned::to_owned)
}

fn container_status_from_item(item: &ContainerListItem) -> Option<String> {
    item.status_state().map(std::borrow::ToOwned::to_owned)
}

fn container_has_profile_label(item: &ContainerListItem) -> bool {
    item.has_profile_label()
}

async fn mark_container_profile(container_id: &str, profile_name: &str) {
    let label = format!("muthr.profile.{}=true", profile_name);
    let result = Command::new("container")
        .args(["update", container_id, "--label", &label])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Plugin 'container-update' not found") {
                return;
            }
            eprintln!(
                "warning: failed to persist profile label '{}' for container '{}'",
                profile_name, container_id
            );
        }
        Err(_) => {
            eprintln!(
                "warning: failed to persist profile label '{}' for container '{}'",
                profile_name, container_id
            );
        }
    }
}

async fn container_exists(container_id: &str) -> bool {
    let Some(items) = container_list_all().await else {
        return false;
    };

    items
        .iter()
        .filter_map(container_id_from_item)
        .any(|id| id == container_id)
}

async fn container_is_running(container_id: &str) -> bool {
    let Some(items) = container_list_all().await else {
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
    let _lock =
        crate::lifecycle::acquire("container-lifecycle", std::time::Duration::from_secs(20))
            .await?;
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(home).join(".cache/muthr");

    if !cache_dir.exists() {
        if verbose {
            eprintln!(
                "warning: sandbox cache directory is missing; skipping untracked cleanup to avoid accidental deletion"
            );
        }
        return Ok(());
    }

    let Some(items) = container_list_all().await else {
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

        let is_managed_project = item.label("muthr.managed").is_some_and(|v| v == "true")
            && item.label("muthr.owner").is_some_and(|v| v == "project");
        if !is_managed_project {
            if verbose {
                eprintln!(
                    "info: skipping unlabeled sandbox container {}",
                    container_id
                );
            }
            continue;
        }

        let profile_cache = cache_dir.join(format!("{}-profiles", container_id));
        if profile_cache.exists() || container_has_profile_label(&item) {
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
        delete_container_impl(&container_id, true).await?;
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

async fn discover_managed_project_sandboxes() -> Vec<String> {
    let Some(items) = container_list_all().await else {
        return Vec::new();
    };

    let mut ids: Vec<String> = items
        .iter()
        .filter_map(|item| {
            let id = container_id_from_item(item)?;
            if !id.starts_with("muthr-") || id == "muthr-services" || id == "muthr-searxng" {
                return None;
            }

            let managed = item.label("muthr.managed").is_some_and(|v| v == "true");
            let owner_project = item.label("muthr.owner").is_some_and(|v| v == "project");

            if managed && owner_project {
                Some(id)
            } else {
                None
            }
        })
        .collect();

    ids.sort();
    ids.dedup();
    ids
}

fn validate_named_sandbox(container_id: &str) -> Result<(), color_eyre::Report> {
    if !container_id.starts_with("muthr-") {
        return Err(color_eyre::eyre::eyre!(
            "invalid sandbox name '{}': must start with 'muthr-'",
            container_id
        ));
    }
    if container_id == "muthr-services" || container_id == "muthr-searxng" {
        return Err(color_eyre::eyre::eyre!(
            "'{}' is a services container, not a project sandbox",
            container_id
        ));
    }
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
    engine_runtime: &str,
    model_name: &str,
    ctx_window: u32,
    mount_point: &std::path::Path,
    port: u16,
) -> Result<(), color_eyre::Report> {
    run_provision_container(
        container_id,
        script_name,
        engine_runtime,
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

async fn resolve_active_model_and_ctx(
    home: &str,
    server_port: u16,
    engine_name: &str,
) -> (String, u32) {
    let default_model = crate::config::load()
        .ok()
        .and_then(|cfg| cfg.default_engine_profile)
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| crate::engine::default_model_for_runtime(engine_name).to_string());
    let preset_ctx_hint = 131072;

    let active_model_file = PathBuf::from(home).join(format!(
        ".cache/muthr/{}",
        crate::engine::active_preset_file_for_runtime(engine_name)
    ));
    let active_model_from_file = fs::read_to_string(&active_model_file)
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let fallback_model = active_model_from_file.unwrap_or_else(|| default_model.clone());

    let parsed_model = match crate::model::poll_loaded_model("127.0.0.1", server_port, 20, 1.0)
        .await
    {
        Ok(model) => model,
        Err(err) => {
            eprintln!(
                "warning: failed to poll loaded model from inference server, using fallback: {}",
                err
            );
            fallback_model.clone()
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
        fallback_model
    } else {
        sanitized_model
    };
    let model_ctx_window = crate::model::get_ctx_window("127.0.0.1", server_port)
        .await
        .unwrap_or(preset_ctx_hint);
    let ctx_window = std::cmp::max(model_ctx_window, preset_ctx_hint);

    (active_model, ctx_window)
}

fn parse_gateway_from_route_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let tokens: Vec<&str> = trimmed.split_whitespace().collect();

        if let Some(idx) = tokens.iter().position(|t| *t == "via")
            && let Some(candidate) = tokens.get(idx + 1)
            && !candidate.trim().is_empty()
        {
            return Some(candidate.trim().to_string());
        }

        if let Some(idx) = tokens.iter().position(|t| *t == "gateway:")
            && let Some(candidate) = tokens.get(idx + 1)
            && !candidate.trim().is_empty()
        {
            return Some(candidate.trim().to_string());
        }
    }

    None
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

async fn discover_gateway_from_container(container_id: &str) -> Option<String> {
    let output = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            "ip route show default 2>/dev/null || route -n get default 2>/dev/null || true",
        ])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_gateway_from_route_output(&stdout)
}

async fn resolve_container_host_gateway(container_id: &str) -> Result<String, color_eyre::Report> {
    let cfg = crate::config::load()?;
    let host = if let Some(configured) = cfg.container_host_gateway {
        configured.trim().to_string()
    } else if let Ok(env_host) = std::env::var("MUTHR_CONTAINER_HOST_GATEWAY") {
        env_host.trim().to_string()
    } else if let Some(discovered) = discover_gateway_from_container(container_id).await {
        discovered
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

async fn backend_openai_url(container_id: &str, port: u16) -> Result<String, color_eyre::Report> {
    let host = resolve_container_host_gateway(container_id).await?;

    Ok(format!("http://{}:{}/v1", host, port))
}

async fn runtime_env_contract(
    container_id: &str,
    port: u16,
    engine_runtime: &str,
    model_name: &str,
) -> Result<Vec<(String, String)>, color_eyre::Report> {
    let host_gateway = resolve_container_host_gateway(container_id).await?;
    let inference_url = backend_openai_url(container_id, port).await?;
    let mcp_bridge_url = format!("http://{}:18765", host_gateway);
    let searxng_url = format!("http://{}:18766", host_gateway);

    Ok(vec![
        ("MUTHR_INFERENCE_URL".to_string(), inference_url.clone()),
        ("MUTHR_OPENAI_URL".to_string(), inference_url),
        ("MUTHR_MCP_BRIDGE_URL".to_string(), mcp_bridge_url),
        ("MUTHR_SEARXNG_URL".to_string(), searxng_url),
        ("MUTHR_MODEL_NAME".to_string(), model_name.to_string()),
        (
            "MUTHR_ENGINE_RUNTIME".to_string(),
            engine_runtime.to_string(),
        ),
    ])
}

pub async fn start(
    profile_name: String,
    audit_log: Option<String>,
) -> Result<(), color_eyre::Report> {
    start_container(profile_name, audit_log).await
}

pub async fn build_golden_image(profile_name: String) -> Result<(), color_eyre::Report> {
    if profile_name.trim().is_empty() {
        return Err(color_eyre::eyre::eyre!("profile cannot be empty"));
    }

    let _lock =
        crate::lifecycle::acquire("container-lifecycle", std::time::Duration::from_secs(20))
            .await?;

    let sanitized = sanitize_project_name(&profile_name)
        .ok_or_else(|| color_eyre::eyre::eyre!("invalid profile name"))?;
    let builder_id = format!("muthr-builder-{}", sanitized);
    let image_tag = golden_image_tag(&profile_name);

    let cfg = crate::config::load()?;
    let server_port = cfg.server_port.unwrap_or(8080);
    let engine_name = cfg.default_engine_runtime.as_deref().unwrap_or("mlxcel");
    let model_name = cfg
        .default_engine_profile
        .unwrap_or_else(|| crate::engine::default_model_for_runtime(engine_name).to_string());
    let ctx_window = 131072_u32;

    let mut settings = container_profile_settings(&profile_name, Path::new("/tmp")).await?;
    settings.mounts.clear();
    settings.workspace_guest_path = "/workspace".to_string();
    settings.network_none = false;
    settings.uses_golden_image = false;

    let create_args = create_args_for_settings(&builder_id, &settings);

    if container_exists(&builder_id).await {
        let _ = Command::new("container")
            .args(["delete", "--force", &builder_id])
            .status()
            .await;
    }

    let mut cleanup_needed = false;
    let temp_dir = std::env::temp_dir().join(format!("muthr-image-build-{}", sanitized));
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
    std::fs::create_dir_all(&temp_dir)?;

    let result: Result<(), color_eyre::Report> = async {
        eprintln!("info: creating builder container: {}", builder_id);
        let status = Command::new("container").args(&create_args).status().await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to create builder container '{}'",
                builder_id
            ));
        }
        cleanup_needed = true;

        let status = Command::new("container")
            .args(["start", &builder_id])
            .status()
            .await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to start builder container '{}'",
                builder_id
            ));
        }

        ensure_container_runtime_baseline(&builder_id).await?;
        sync_container_user_ids(&builder_id).await;

        run_provision_container(
            &builder_id,
            &profile_name,
            engine_name,
            &model_name,
            ctx_window,
            Path::new("/workspace"),
            server_port,
        )
        .await?;

        let status = Command::new("container")
            .args(["stop", &builder_id])
            .status()
            .await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to stop builder container before export"
            ));
        }

        let rootfs_tar = temp_dir.join("rootfs.tar");
        let status = Command::new("container")
            .args([
                "export",
                "--output",
                rootfs_tar.to_str().ok_or_else(|| {
                    color_eyre::eyre::eyre!("invalid temporary export path")
                })?,
                &builder_id,
            ])
            .status()
            .await?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to export builder container filesystem"
            ));
        }

        let containerfile = temp_dir.join("Containerfile");
        std::fs::write(&containerfile, "FROM scratch\nADD rootfs.tar /\n")?;

        let output = Command::new("container")
            .args([
                "build",
                "--platform",
                NATIVE_PLATFORM,
                "--file",
                containerfile
                    .to_str()
                    .ok_or_else(|| color_eyre::eyre::eyre!("invalid Containerfile path"))?,
                "--tag",
                &image_tag,
                temp_dir
                    .to_str()
                    .ok_or_else(|| color_eyre::eyre::eyre!("invalid build context path"))?,
            ])
            .output()
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            let stdout = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
            if stderr.contains("rosetta") || stdout.contains("rosetta") {
                return Err(color_eyre::eyre::eyre!(
                    "golden image build failed because the container backend buildkit requires Rosetta on this host, even with '{}'. This is a backend limitation; use prebuilt arm64 images or install Rosetta for buildkit",
                    NATIVE_PLATFORM
                ));
            }
            return Err(color_eyre::eyre::eyre!(
                "failed to build golden image '{}'",
                image_tag
            ));
        }

        eprintln!("info: built golden image {}", image_tag);
        Ok(())
    }
    .await;

    if cleanup_needed {
        let _ = Command::new("container")
            .args(["delete", "--force", &builder_id])
            .status()
            .await;
    }
    let _ = std::fs::remove_dir_all(&temp_dir);

    result
}

pub async fn stop(names: Vec<String>, all: bool) -> Result<(), color_eyre::Report> {
    if all {
        let sandboxes = discover_managed_project_sandboxes().await;
        if sandboxes.is_empty() {
            eprintln!("info: no managed sandbox containers found");
            return Ok(());
        }

        for id in sandboxes {
            if !container_is_running(&id).await {
                eprintln!("info: already stopped {}", id);
                continue;
            }

            let status = Command::new("container")
                .args(["stop", &id])
                .status()
                .await?;
            if !status.success() {
                return Err(color_eyre::eyre::eyre!("failed to stop container '{}'", id));
            }
            eprintln!("info: stopped {}", id);
        }
        return Ok(());
    }

    if !names.is_empty() {
        let mut unique = names;
        unique.sort();
        unique.dedup();

        for id in unique {
            validate_named_sandbox(&id)?;
            if !container_exists(&id).await {
                eprintln!("warning: sandbox '{}' does not exist", id);
                continue;
            }
            if !container_is_running(&id).await {
                eprintln!("info: already stopped {}", id);
                continue;
            }

            let status = Command::new("container")
                .args(["stop", &id])
                .status()
                .await?;
            if !status.success() {
                return Err(color_eyre::eyre::eyre!("failed to stop container '{}'", id));
            }
            eprintln!("info: stopped {}", id);
        }
        return Ok(());
    }

    stop_container().await
}

pub async fn ls(out_fmt: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    list_containers(out_fmt).await
}

async fn start_container(
    profile_name: String,
    audit_log: Option<String>,
) -> Result<(), color_eyre::Report> {
    let _lock =
        crate::lifecycle::acquire("container-lifecycle", std::time::Duration::from_secs(20))
            .await?;
    let (container_id, project_root, workdir) = resolve_workspace_context()?;
    eprintln!("info: target: {}", container_id);
    let audit = resolve_audit_logger(audit_log, &container_id)?;

    let mut settings = container_profile_settings(&profile_name, &project_root).await?;
    let needs_profile_provision = profile_name != "base" && !settings.uses_golden_image;
    let deferred_network_isolation = settings.network_none && needs_profile_provision;
    if deferred_network_isolation {
        settings.network_none = false;
    }

    let exists = container_exists(&container_id).await;
    if !exists {
        eprintln!("info: creating container {}", container_id);
        let args = create_args_for_settings(&container_id, &settings);
        let status = Command::new("container").args(&args).status().await?;
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
        Ok(relative_workdir) => {
            PathBuf::from(&settings.workspace_guest_path).join(relative_workdir)
        }
        Err(_) => PathBuf::from(&settings.workspace_guest_path),
    };
    let guest_workdir_str = guest_workdir
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("guest workdir contains invalid UTF-8"))?;

    let home = std::env::var("HOME")?;
    let cfg = crate::config::load()?;
    let server_port = cfg.server_port.unwrap_or(8080);
    let engine_name = cfg.default_engine_runtime.as_deref().unwrap_or("mlxcel");
    let (active_model, ctx_window) =
        resolve_active_model_and_ctx(&home, server_port, engine_name).await;
    let runtime_envs =
        runtime_env_contract(&container_id, server_port, engine_name, &active_model).await?;

    if profile_name != "base" {
        let cache_dir = PathBuf::from(&home)
            .join(".cache/muthr")
            .join(format!("{}-profiles", container_id));

        eprintln!("info: applying profile: {}", profile_name);

        let provision_result = if needs_profile_provision {
            run_provision_container(
                &container_id,
                &profile_name,
                engine_name,
                &active_model,
                ctx_window,
                Path::new(&settings.workspace_guest_path),
                server_port,
            )
            .await
        } else {
            eprintln!(
                "info: using pre-baked image {} for profile {}",
                settings.image, profile_name
            );
            Ok(())
        };

        if deferred_network_isolation {
            eprintln!("info: sealing sandbox boundary -> cutting off network access");
            update_container_network_mode(&container_id, "none").await?;
        }

        provision_result?;

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

        mark_container_profile(&container_id, &profile_name).await;

        eprintln!("info: launching workspace context");
        let target_args = match profile_name.as_str() {
            "opencode" => vec![
                "bash",
                "-lc",
                "export PATH=\"$HOME/.opencode/bin:$HOME/.local/bin:$PATH\"; exec opencode",
            ],
            _ => vec!["sh"],
        };

        let requires_tty = profile_name == "opencode";
        run_container_session(
            &container_id,
            guest_workdir_str,
            &runtime_envs,
            &target_args,
            requires_tty,
            audit.as_ref(),
        )?;

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

    mark_container_profile(&container_id, "base").await;

    eprintln!("info: container ready, launching shell");
    run_container_session(
        &container_id,
        guest_workdir_str,
        &runtime_envs,
        &["sh"],
        false,
        audit.as_ref(),
    )
    .map_err(|_| color_eyre::eyre::eyre!("shell session exited with error"))?;

    Ok(())
}

async fn run_provision_container(
    container_id: &str,
    script_name: &str,
    engine_runtime: &str,
    model_name: &str,
    ctx_window: u32,
    mount_point: &std::path::Path,
    port: u16,
) -> Result<(), color_eyre::Report> {
    fn validate_script_name(script_name: &str) -> Result<(), color_eyre::Report> {
        if script_name.is_empty() {
            return Err(color_eyre::eyre::eyre!("invalid profile name: empty"));
        }
        if script_name
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        {
            return Err(color_eyre::eyre::eyre!(
                "invalid profile name: unsupported characters"
            ));
        }
        Ok(())
    }

    fn validate_engine_runtime(engine_runtime: &str) -> Result<(), color_eyre::Report> {
        if engine_runtime.is_empty() {
            return Err(color_eyre::eyre::eyre!("invalid runtime: empty"));
        }
        if engine_runtime
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        {
            return Err(color_eyre::eyre::eyre!(
                "invalid runtime: unsupported characters"
            ));
        }
        Ok(())
    }

    validate_script_name(script_name)?;
    validate_engine_runtime(engine_runtime)?;

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
    let runtime_envs = runtime_env_contract(container_id, port, engine_runtime, model_name).await?;
    let mut env_map = std::collections::HashMap::new();
    for (k, v) in &runtime_envs {
        env_map.insert(k.as_str(), v.as_str());
    }

    let openai_url = env_map
        .get("MUTHR_OPENAI_URL")
        .copied()
        .ok_or_else(|| color_eyre::eyre::eyre!("missing MUTHR_OPENAI_URL env value"))?;
    let host_gateway = resolve_container_host_gateway(container_id).await?;
    let searxng_url = env_map
        .get("MUTHR_SEARXNG_URL")
        .copied()
        .ok_or_else(|| color_eyre::eyre::eyre!("missing MUTHR_SEARXNG_URL env value"))?;
    let inference_url = env_map
        .get("MUTHR_INFERENCE_URL")
        .copied()
        .ok_or_else(|| color_eyre::eyre::eyre!("missing MUTHR_INFERENCE_URL env value"))?;
    let mcp_bridge_url = env_map
        .get("MUTHR_MCP_BRIDGE_URL")
        .copied()
        .ok_or_else(|| color_eyre::eyre::eyre!("missing MUTHR_MCP_BRIDGE_URL env value"))?;

    validate_env_value(openai_url, "MUTHR_OPENAI_URL")?;
    validate_env_value(inference_url, "MUTHR_INFERENCE_URL")?;
    validate_env_value(mcp_bridge_url, "MUTHR_MCP_BRIDGE_URL")?;
    validate_env_value(&host_gateway, "MUTHR_CONTAINER_HOST_GATEWAY")?;
    validate_env_value(searxng_url, "MUTHR_SEARXNG_URL")?;
    validate_env_value(model_name, "MUTHR_MODEL_NAME")?;
    validate_model_name(model_name)?;
    validate_env_value(mount_str, "MUTHR_WORKSPACE_MOUNT")?;
    validate_env_value(engine_runtime, "MUTHR_ENGINE_RUNTIME")?;

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
        .args(["exec", container_id, "mkdir", "-p", &guest_provision_dir])
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
        .arg("--user")
        .arg("muthr")
        .arg("--env")
        .arg(format!("MUTHR_OPENAI_URL={}", openai_url))
        .arg("--env")
        .arg(format!("MUTHR_INFERENCE_URL={}", inference_url))
        .arg("--env")
        .arg(format!("MUTHR_MCP_BRIDGE_URL={}", mcp_bridge_url))
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
        .arg("--env")
        .arg(format!("MUTHR_ENGINE_RUNTIME={}", engine_runtime))
        .arg(container_id)
        .arg("bash")
        .arg(&guest_script_path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;

    let status = child.wait().await?;
    if !status.success() {
        return Err(color_eyre::eyre::eyre!("provision failed: {}", script_name));
    }

    eprintln!("info: provisioned: {}", script_name);
    Ok(())
}

async fn ensure_container_runtime_baseline(container_id: &str) -> Result<(), color_eyre::Report> {
    let marker = "/var/lib/muthr/container-baseline-v2";
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
            "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq bash curl ca-certificates sudo git nodejs npm && if ! id -u muthr >/dev/null 2>&1; then useradd -m -s /bin/bash muthr; fi && usermod -aG sudo muthr && install -d -m 755 /etc/sudoers.d && printf 'muthr ALL=(ALL) NOPASSWD:ALL\\n' >/etc/sudoers.d/muthr && chmod 0440 /etc/sudoers.d/muthr",
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
            "mkdir -p /var/lib/muthr && touch /var/lib/muthr/container-baseline-v2",
        ])
        .status()
        .await?;
    if !marker_status.success() {
        eprintln!("warning: failed to persist container baseline marker");
    }

    Ok(())
}

async fn stop_container() -> Result<(), color_eyre::Report> {
    let _lock =
        crate::lifecycle::acquire("container-lifecycle", std::time::Duration::from_secs(20))
            .await?;
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
    let _lock =
        crate::lifecycle::acquire("container-lifecycle", std::time::Duration::from_secs(20))
            .await?;
    delete_container_impl(container_id, force).await
}

async fn delete_container_impl(container_id: &str, force: bool) -> Result<(), color_eyre::Report> {
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
    let Some(items) = container_list_all().await else {
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
