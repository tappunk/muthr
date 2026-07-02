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

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use std::{collections::HashMap, fs as stdfs};

use tokio::fs;
use tokio::process::Command as AsyncCommand;
use tokio::signal::unix::{SignalKind, signal};

use crate::config;
use crate::model;

#[derive(Clone, Copy)]
pub struct EngineRuntimeSpec {
    pub name: &'static str,
    pub executable: &'static str,
    pub pid_file_name: &'static str,
    pub active_preset_file: &'static str,
    pub log_stdout: &'static str,
    pub log_stderr: &'static str,
    pub default_model_id: &'static str,
    pub default_bind_host: &'static str,
}

#[derive(Debug, Clone)]
struct PresetSpec {
    name: String,
    runtime: Option<String>,
    model: String,
}

const MLXCEL_SPEC: EngineRuntimeSpec = EngineRuntimeSpec {
    name: "mlxcel",
    executable: "mlxcel-server",
    pid_file_name: "mlxcel-server.pid",
    active_preset_file: "active-preset-name-mlxcel",
    log_stdout: "mlxcel-server.log",
    log_stderr: "mlxcel-server-err.log",
    default_model_id: "mlx-community/Qwen3.5-9B-MLX-4bit",
    default_bind_host: "0.0.0.0",
};

const LLAMA_SPEC: EngineRuntimeSpec = EngineRuntimeSpec {
    name: "llama",
    executable: "llama-server",
    pid_file_name: "llama-server.pid",
    active_preset_file: "active-preset-name-llama",
    log_stdout: "llama-server.log",
    log_stderr: "llama-server-err.log",
    default_model_id: "unsloth/Qwen3.6-35B-A3B-GGUF/Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf",
    default_bind_host: "0.0.0.0",
};

const SUPPORTED_RUNTIMES: [EngineRuntimeSpec; 2] = [MLXCEL_SPEC, LLAMA_SPEC];

pub fn runtime_spec(runtime: &str) -> Option<EngineRuntimeSpec> {
    match runtime {
        "mlxcel" => Some(MLXCEL_SPEC),
        "llama" => Some(LLAMA_SPEC),
        _ => None,
    }
}

pub fn supports_runtime(runtime: &str) -> bool {
    runtime_spec(runtime).is_some()
}

pub fn supported_runtime_names() -> &'static [&'static str] {
    &["mlxcel", "llama"]
}

pub fn resolve_runtime_for_profile(
    runtime_flag: Option<String>,
    configured_runtime: Option<String>,
    profile: Option<&str>,
) -> Result<String, color_eyre::Report> {
    if let Some(runtime) = runtime_flag {
        if !supports_runtime(&runtime) {
            return Err(color_eyre::eyre::eyre!(
                "unsupported engine runtime '{}' (supported: {})",
                runtime,
                supported_runtime_names().join(", ")
            ));
        }
        return Ok(runtime);
    }

    if let Some(profile_name) = profile
        && let Ok(Some(preset)) = resolve_preset(profile_name)
        && let Some(runtime) = preset.runtime
    {
        if supports_runtime(&runtime) {
            return Ok(runtime);
        }

        return Err(color_eyre::eyre::eyre!(
            "preset '{}' declares unsupported runtime '{}' (supported: {})",
            profile_name,
            runtime,
            supported_runtime_names().join(", ")
        ));
    }

    let runtime = configured_runtime.unwrap_or_else(|| "mlxcel".to_string());
    if !supports_runtime(&runtime) {
        return Err(color_eyre::eyre::eyre!(
            "unsupported engine runtime '{}' (supported: {})",
            runtime,
            supported_runtime_names().join(", ")
        ));
    }

    Ok(runtime)
}

pub fn active_preset_file_for_runtime(runtime: &str) -> &'static str {
    runtime_spec(runtime)
        .unwrap_or(MLXCEL_SPEC)
        .active_preset_file
}

pub fn default_model_for_runtime(runtime: &str) -> &'static str {
    runtime_spec(runtime)
        .unwrap_or(MLXCEL_SPEC)
        .default_model_id
}

fn resolve_bind_host(
    spec: EngineRuntimeSpec,
    bind_host: Option<String>,
) -> Result<String, color_eyre::Report> {
    if let Some(host) = bind_host
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return Ok(host);
    }

    if let Ok(cfg) = config::load()
        && let Some(host) = cfg
            .default_engine_bind_host
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    {
        return Ok(host);
    }

    Ok(spec.default_bind_host.to_string())
}

pub async fn verify_health(port: u16) -> bool {
    model::verify_health("127.0.0.1", port).await
}

fn parse_ini_file(path: &std::path::Path) -> Result<HashMap<String, String>, color_eyre::Report> {
    let mut map = HashMap::new();
    let content = stdfs::read_to_string(path)?;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with(';')
            || (line.starts_with('[') && line.ends_with(']'))
        {
            continue;
        }

        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let value = v.trim().trim_matches('"').trim_matches('\'').to_string();
        if !key.is_empty() && !value.is_empty() {
            map.insert(key, value);
        }
    }
    Ok(map)
}

fn preset_from_kv(name: &str, kv: &HashMap<String, String>) -> Option<PresetSpec> {
    let model = kv
        .get("model")
        .or_else(|| kv.get("model_id"))
        .or_else(|| kv.get("profile"))
        .cloned()?;
    let runtime = kv
        .get("runtime")
        .or_else(|| kv.get("engine_runtime"))
        .cloned();

    Some(PresetSpec {
        name: name.to_string(),
        runtime,
        model,
    })
}

fn discover_presets() -> Result<Vec<PresetSpec>, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let root = PathBuf::from(home).join(".config/muthr/provider.d");
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut presets = Vec::new();
    let mut stack = vec![root];

    while let Some(dir) = stack.pop() {
        for entry in stdfs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
                continue;
            }

            if path.extension().and_then(|s| s.to_str()) != Some("ini") {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if name.is_empty() {
                continue;
            }

            let kv = parse_ini_file(&path)?;
            if let Some(preset) = preset_from_kv(&name, &kv) {
                presets.push(preset);
            }
        }
    }

    presets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(presets)
}

fn resolve_preset(profile: &str) -> Result<Option<PresetSpec>, color_eyre::Report> {
    let trimmed = profile.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let presets = discover_presets()?;
    Ok(presets.into_iter().find(|p| p.name == trimmed))
}

fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn pid_file_path(spec: EngineRuntimeSpec) -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(format!(".cache/muthr/{}", spec.pid_file_name)))
}

fn kill_runtime_target(pid: u32, sig: i32) {
    let pgid = unsafe { libc::getpgid(pid as i32) };
    if pgid > 0 {
        unsafe {
            libc::kill(-pgid, sig);
        }
    } else {
        unsafe {
            libc::kill(pid as i32, sig);
        }
    }
}

fn last_errno() -> i32 {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        // SAFETY: libc exposes thread-local errno pointer for current thread.
        unsafe { *libc::__error() }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        // SAFETY: libc exposes thread-local errno pointer for current thread.
        unsafe { *libc::__errno_location() }
    }
}

fn matches_runtime_process(spec: EngineRuntimeSpec, comm: &str, args: &str) -> bool {
    let executable = spec.executable;
    comm == executable
        || args.contains(&format!(" {}", executable))
        || args.contains(&format!("/{}", executable))
}

async fn is_runtime_pid(spec: EngineRuntimeSpec, pid: u32) -> bool {
    if !is_process_alive(pid) {
        return false;
    }

    let output = AsyncCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm=", "-o", "args="])
        .output()
        .await;

    if let Ok(out) = output {
        let ps_output = String::from_utf8_lossy(&out.stdout);
        for line in ps_output.lines() {
            let trimmed = line.trim();
            let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
            if parts.len() < 2 {
                continue;
            }
            let comm = parts[0].trim();
            let args = parts[1];
            if matches_runtime_process(spec, comm, args) {
                return true;
            }
        }
    }

    false
}

async fn list_runtime_pids(spec: EngineRuntimeSpec) -> Vec<u32> {
    let output = AsyncCommand::new("ps")
        .args(["-axo", "pid=,comm=,args="])
        .output()
        .await;
    let mut pids = Vec::new();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut parts = trimmed.split_whitespace();
            let pid_str = match parts.next() {
                Some(v) => v,
                None => continue,
            };
            let comm = match parts.next() {
                Some(v) => v,
                None => continue,
            };
            let args = parts.collect::<Vec<_>>().join(" ");

            if let Ok(pid) = pid_str.parse::<u32>()
                && matches_runtime_process(spec, comm, &args)
            {
                pids.push(pid);
            }
        }
    }

    pids.sort_unstable();
    pids.dedup();
    pids
}

async fn list_container_sandboxes() -> Vec<(String, String)> {
    #[derive(serde::Deserialize)]
    struct ContainerConfiguration {
        #[serde(default)]
        id: Option<String>,
        #[serde(default, alias = "ID", alias = "Id")]
        id_alias: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct ContainerItem {
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

    let output = AsyncCommand::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .await;
    let Ok(out) = output else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }

    let Ok(items) = serde_json::from_slice::<Vec<ContainerItem>>(&out.stdout) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    for item in items {
        let id = item
            .id
            .as_deref()
            .or(item.id_alias.as_deref())
            .or_else(|| {
                item.configuration
                    .as_ref()
                    .or(item.configuration_alias.as_ref())
                    .and_then(|c| c.id.as_deref().or(c.id_alias.as_deref()))
            })
            .unwrap_or_default()
            .to_string();
        if !id.starts_with("muthr-") || id == "muthr-services" || id == "muthr-searxng" {
            continue;
        }

        let status = item
            .status
            .as_ref()
            .or(item.status_alias.as_ref())
            .and_then(|v| {
                if let Some(s) = v.as_str() {
                    return Some(s);
                }
                v.get("state")
                    .or_else(|| v.get("State"))
                    .and_then(|s| s.as_str())
            })
            .or(item.state.as_deref())
            .or(item.state_alias.as_deref())
            .unwrap_or("unknown")
            .to_string();

        let token = id.strip_prefix("muthr-").unwrap_or(&id).to_string();
        rows.push((token, status));
    }

    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn resolve_model_id_for_runtime(spec: EngineRuntimeSpec, profile: Option<String>) -> String {
    profile
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(|p| {
            if let Ok(Some(preset)) = resolve_preset(&p) {
                preset.model
            } else {
                p
            }
        })
        .or_else(|| {
            config::load()
                .ok()
                .and_then(|cfg| cfg.default_engine_profile)
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
        })
        .unwrap_or_else(|| spec.default_model_id.to_string())
}

fn build_command(
    spec: EngineRuntimeSpec,
    model_id: &str,
    host: &str,
    port: u16,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(spec.executable);
    cmd.arg("--model").arg(model_id);
    cmd.arg("--host").arg(host);
    cmd.arg("--port").arg(port.to_string());
    cmd
}

pub async fn is_running() -> bool {
    for spec in SUPPORTED_RUNTIMES {
        if is_running_for_runtime(spec).await {
            return true;
        }
    }

    false
}

async fn is_running_for_runtime(spec: EngineRuntimeSpec) -> bool {
    let pid_file = match pid_file_path(spec) {
        Some(path) => path,
        None => return !list_runtime_pids(spec).await.is_empty(),
    };

    if pid_file.exists() {
        let pid_bytes = match fs::read_to_string(&pid_file).await {
            Ok(b) => b,
            Err(_) => return !list_runtime_pids(spec).await.is_empty(),
        };

        let pid = match pid_bytes.trim().parse::<u32>() {
            Ok(p) => p,
            Err(_) => {
                fs::remove_file(&pid_file).await.ok();
                return !list_runtime_pids(spec).await.is_empty();
            }
        };

        if is_runtime_pid(spec, pid).await {
            return true;
        }

        fs::remove_file(&pid_file).await.ok();
    }

    !list_runtime_pids(spec).await.is_empty()
}

pub async fn start(
    runtime: &str,
    profile: Option<String>,
    port: u16,
    bind_host: Option<String>,
    foreground: bool,
) -> Result<(), color_eyre::Report> {
    let spec = runtime_spec(runtime).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "unsupported engine runtime '{}' (supported: {})",
            runtime,
            supported_runtime_names().join(", ")
        )
    })?;

    let _engine_lock = crate::lifecycle::acquire("engine", Duration::from_secs(20)).await?;
    let model_id = resolve_model_id_for_runtime(spec, profile);
    let bind_host = resolve_bind_host(spec, bind_host)?;

    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir).await?;

    let log_stdout = cache_dir.join(spec.log_stdout);
    let log_stderr = cache_dir.join(spec.log_stderr);
    let pid_file = cache_dir.join(spec.pid_file_name);

    let mut existing_pids: Vec<(EngineRuntimeSpec, u32)> = Vec::new();
    for runtime_spec in SUPPORTED_RUNTIMES {
        for pid in list_runtime_pids(runtime_spec).await {
            existing_pids.push((runtime_spec, pid));
        }
    }

    if !existing_pids.is_empty() {
        eprintln!("warning: found running engine process(es), stopping before start");
        for (running_spec, pid) in existing_pids {
            stop_pid(running_spec, pid).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    fs::remove_file(&pid_file).await.ok();

    if foreground {
        eprintln!("info: {} starting on {}:{}", spec.name, bind_host, port);

        let mut child = AsyncCommand::new(spec.executable);
        child
            .arg("--model")
            .arg(&model_id)
            .arg("--host")
            .arg(&bind_host)
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = child.spawn()?;
        let child_pid = child.id().unwrap_or_default();

        let active_model_path = cache_dir.join(spec.active_preset_file);
        fs::write(&active_model_path, &model_id).await?;

        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut shutdown_requested = false;

        loop {
            tokio::select! {
                maybe_status = child.wait() => {
                    let status = maybe_status?;
                    if !status.success() {
                        eprintln!("error: server exited with code {}", status);
                    }
                    return Ok(());
                }
                _ = sigterm.recv() => {
                    if child_pid != 0 {
                        if !shutdown_requested {
                            eprintln!("info: forwarding SIGTERM to {} pid {}", spec.name, child_pid);
                            kill_runtime_target(child_pid, libc::SIGTERM);
                            shutdown_requested = true;
                        } else {
                            eprintln!("warning: second signal received, forwarding SIGKILL to {} pid {}", spec.name, child_pid);
                            kill_runtime_target(child_pid, libc::SIGKILL);
                        }
                    }
                }
                _ = sigint.recv() => {
                    if child_pid != 0 {
                        if !shutdown_requested {
                            eprintln!("info: forwarding SIGINT to {} pid {}", spec.name, child_pid);
                            kill_runtime_target(child_pid, libc::SIGTERM);
                            shutdown_requested = true;
                        } else {
                            eprintln!("warning: second signal received, forwarding SIGKILL to {} pid {}", spec.name, child_pid);
                            kill_runtime_target(child_pid, libc::SIGKILL);
                        }
                    }
                }
            }
        }
    }

    eprintln!(
        "{} starting (background) on {}:{}",
        spec.name, bind_host, port
    );

    let stdout_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_stdout)?;
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_stderr)?;

    let mut cmd = build_command(spec, &model_id, &bind_host, port);
    cmd.stdout(stdout_file).stderr(stderr_file);

    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::from_raw_os_error(last_errno()));
            }
            Ok(())
        });
    }

    match cmd.spawn() {
        Ok(c) => {
            let pid = c.id();
            fs::write(&pid_file, pid.to_string()).await?;
            let active_model_path = cache_dir.join(spec.active_preset_file);
            fs::write(&active_model_path, &model_id).await?;
            eprintln!("info: started pid {}", pid);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn stop(runtime: &str) -> Result<(), color_eyre::Report> {
    let spec = runtime_spec(runtime).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "unsupported engine runtime '{}' (supported: {})",
            runtime,
            supported_runtime_names().join(", ")
        )
    })?;

    let _engine_lock = crate::lifecycle::acquire("engine", Duration::from_secs(20)).await?;
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    let pid_file = cache_dir.join(spec.pid_file_name);

    let mut target_pids = Vec::new();
    if pid_file.exists()
        && let Ok(pid_bytes) = fs::read_to_string(&pid_file).await
        && let Ok(pid) = pid_bytes.trim().parse::<u32>()
    {
        if is_runtime_pid(spec, pid).await {
            target_pids.push(pid);
        } else {
            eprintln!(
                "warning: stale pid file for non-{} process {}, removing",
                spec.name, pid
            );
        }
    }

    for pid in list_runtime_pids(spec).await {
        if !target_pids.contains(&pid) {
            target_pids.push(pid);
        }
    }

    if target_pids.is_empty() {
        fs::remove_file(&pid_file).await.ok();
        return Ok(());
    }

    for pid in target_pids {
        stop_pid(spec, pid).await;
    }

    fs::remove_file(&pid_file).await.ok();
    Ok(())
}

pub async fn stop_all() -> Result<(), color_eyre::Report> {
    for spec in SUPPORTED_RUNTIMES {
        stop(spec.name).await?;
    }
    Ok(())
}

async fn stop_pid(spec: EngineRuntimeSpec, pid: u32) {
    if !is_runtime_pid(spec, pid).await {
        return;
    }

    eprintln!("info: stopping {} pid {}", spec.name, pid);
    kill_runtime_target(pid, libc::SIGTERM);

    let mut died = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if !is_process_alive(pid) {
            died = true;
            break;
        }
    }

    if died {
        eprintln!("info: stopped {} pid {}", spec.name, pid);
    } else {
        eprintln!(
            "warning: sigterm failed for {} pid {}, escalating to sigkill",
            spec.name, pid
        );
        kill_runtime_target(pid, libc::SIGKILL);
        eprintln!("info: killed {} pid {}", spec.name, pid);
    }
}

pub async fn status(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");

    let mut runtimes = Vec::new();
    for spec in SUPPORTED_RUNTIMES {
        let active_model = fs::read_to_string(cache_dir.join(spec.active_preset_file))
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let running = is_running_for_runtime(spec).await;
        let configured = !active_model.is_empty();
        runtimes.push((spec, active_model, running, configured));
    }

    let any_running = runtimes.iter().any(|(_, _, running, _)| *running);
    let any_model = runtimes.iter().any(|(_, _, _, configured)| *configured);

    let overall_state = if !any_model {
        "not_configured"
    } else if any_running {
        "running"
    } else {
        "configured_stopped"
    };

    if output == crate::OutputFormat::Json || output == crate::OutputFormat::Ndjson {
        let mut runtimes_payload = serde_json::Map::new();
        for (spec, active_model, running, _) in &runtimes {
            runtimes_payload.insert(
                spec.name.to_string(),
                serde_json::json!({
                    "model": if active_model.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(active_model.clone()) },
                    "server_running": *running,
                }),
            );
        }

        let payload = serde_json::json!({
            "state": overall_state,
            "runtimes": runtimes_payload,
        });
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if !any_model {
        eprintln!("muthr: not configured");
    } else if any_running {
        eprintln!("muthr: running");
    } else {
        eprintln!("muthr: configured, stopped");
    }

    for (spec, active_model, running, configured) in &runtimes {
        print_runtime_status(spec.name, active_model, *running, *configured);
    }

    let services_container = "muthr-services";
    let searxng_container = "muthr-searxng";
    let container_items = AsyncCommand::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .await
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout).ok())
        .unwrap_or_default();

    let mut services_status: Option<&str> = None;
    for item in &container_items {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .or_else(|| {
                item.get("configuration")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or_default();
        if id != services_container {
            continue;
        }
        let state = item
            .get("status")
            .and_then(|v| v.get("state"))
            .and_then(|v| v.as_str())
            .or_else(|| item.get("state").and_then(|v| v.as_str()))
            .unwrap_or("unknown");
        services_status = Some(if state.eq_ignore_ascii_case("running") {
            "running"
        } else {
            "stopped"
        });
        break;
    }

    if let Some(status) = services_status {
        eprintln!("  services mcp     {}     {}", services_container, status);

        let provision_output = AsyncCommand::new("container")
            .args([
                "exec",
                services_container,
                "bash",
                "-c",
                "test -f $HOME/mcp-stdio.sh && test -f $HOME/.local/lib/node_modules/mcp-searxng/dist/cli.js",
            ])
            .output()
            .await
            .ok();

        match provision_output {
            Some(out) if out.status.success() => {
                eprintln!("  mcp provisioned  yes");
            }
            _ => {
                eprintln!("  mcp provisioned  no");
            }
        }
    }

    let mut searxng_status: Option<&str> = None;
    for item in &container_items {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .or_else(|| {
                item.get("configuration")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or_default();
        if id != searxng_container {
            continue;
        }
        let state = item
            .get("status")
            .and_then(|v| v.get("state"))
            .and_then(|v| v.as_str())
            .or_else(|| item.get("state").and_then(|v| v.as_str()))
            .unwrap_or("unknown");
        searxng_status = Some(if state.eq_ignore_ascii_case("running") {
            "running"
        } else {
            "stopped"
        });
        break;
    }

    if let Some(status) = searxng_status {
        eprintln!("  services searxng {}     {}", searxng_container, status);
    }

    let mut active_sandboxes: Vec<(String, String)> = Vec::new();

    for row in list_container_sandboxes().await {
        if !active_sandboxes.iter().any(|(t, _)| *t == row.0) {
            active_sandboxes.push(row);
        }
    }

    if !active_sandboxes.is_empty() {
        eprintln!("  sandboxes");
        for (i, (token, status)) in active_sandboxes.iter().enumerate() {
            let connector = if i + 1 == active_sandboxes.len() {
                "    └─"
            } else {
                "    ├─"
            };
            eprintln!("{} {:<20} {}", connector, token, status);
        }
    }

    Ok(())
}

fn print_runtime_status(runtime: &str, model_id: &str, is_running: bool, configured: bool) {
    if !configured {
        eprintln!("  engine {:<10} (none)", runtime);
    } else {
        eprintln!("  engine {:<10} active      {}", runtime, model_id);
    }

    if is_running {
        eprintln!("  server {:<10} running", runtime);
    } else {
        eprintln!("  server {:<10} stopped", runtime);
    }
}

pub fn presets(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let config = config::load()?;
    let runtime = config
        .default_engine_runtime
        .unwrap_or_else(|| MLXCEL_SPEC.name.to_string());
    presets_for_runtime(&runtime, output)
}

pub fn presets_for_runtime(
    runtime: &str,
    output: crate::OutputFormat,
) -> Result<(), color_eyre::Report> {
    let spec = runtime_spec(runtime).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "unsupported engine runtime '{}' (supported: {})",
            runtime,
            supported_runtime_names().join(", ")
        )
    })?;
    let default_model = config::load()?
        .default_engine_profile
        .unwrap_or_else(|| spec.default_model_id.to_string());

    let mut all_models: Vec<(String, String)> =
        vec![(default_model.clone(), spec.name.to_string())];
    for preset in discover_presets()? {
        let preset_runtime = preset.runtime.unwrap_or_else(|| spec.name.to_string());
        if preset_runtime == spec.name {
            all_models.push((preset.model, preset_runtime));
        }
    }

    all_models.sort_by(|a, b| a.0.cmp(&b.0));
    all_models.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);

    if output == crate::OutputFormat::Json {
        let payload: Vec<serde_json::Value> = all_models
            .iter()
            .map(|(model, runtime_name)| {
                serde_json::json!({
                    "id": model,
                    "runtime": runtime_name,
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if output == crate::OutputFormat::Ndjson {
        for (model, runtime_name) in &all_models {
            let payload = serde_json::json!({
                "id": model,
                "runtime": runtime_name,
            });
            println!("{}", serde_json::to_string(&payload)?);
        }
        return Ok(());
    }

    for (model, _) in &all_models {
        eprintln!("{}", model);
    }
    Ok(())
}
