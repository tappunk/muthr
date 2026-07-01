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

use tokio::fs;
use tokio::process::Command as AsyncCommand;
use tokio::signal::unix::{SignalKind, signal};

use crate::config;
use crate::model;

pub const ENGINE_NAME: &str = "mlxcel";
pub const EXECUTABLE: &str = "mlxcel-server";
pub const PID_FILE_NAME: &str = "mlxcel-server.pid";
pub const ACTIVE_PRESET_FILE: &str = "active-preset-name-mlxcel";
pub const LOG_STDOUT: &str = "mlxcel-server.log";
pub const LOG_RE_ERR: &str = "mlxcel-server-err.log";

const DEFAULT_MODEL_ID: &str = "mlx-community/Qwen3.5-9B-MLX-4bit";
const BIND_HOST: &str = "127.0.0.1";

pub async fn verify_health(port: u16) -> bool {
    model::verify_health(BIND_HOST, port).await
}

fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn pid_file_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(format!(".cache/muthr/{}", PID_FILE_NAME)))
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

fn matches_runtime_process(comm: &str, args: &str) -> bool {
    comm == EXECUTABLE || args.contains(" mlxcel-server") || args.contains("/mlxcel-server")
}

async fn is_runtime_pid(pid: u32) -> bool {
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
            if matches_runtime_process(comm, args) {
                return true;
            }
        }
    }

    false
}

async fn list_runtime_pids() -> Vec<u32> {
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
                && matches_runtime_process(comm, &args)
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

    let Ok(items) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    for item in items {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .or_else(|| {
                item.get("configuration")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or_default()
            .to_string();
        if !id.starts_with("muthr-") || id == "muthr-services" || id == "muthr-searxng" {
            continue;
        }

        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("state").and_then(|v| v.as_str()))
            .or_else(|| {
                item.get("status")
                    .and_then(|v| v.get("state"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("unknown")
            .to_string();

        let token = id.strip_prefix("muthr-").unwrap_or(&id).to_string();
        rows.push((token, status));
    }

    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn resolve_model_id(profile: Option<String>) -> String {
    profile
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .or_else(|| {
            config::load()
                .ok()
                .and_then(|cfg| cfg.default_engine_profile)
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string())
}

fn build_command(model_id: &str, host: &str, port: u16) -> std::process::Command {
    let mut cmd = std::process::Command::new(EXECUTABLE);
    cmd.arg("--model").arg(model_id);
    cmd.arg("--host").arg(host);
    cmd.arg("--port").arg(port.to_string());
    cmd
}

pub async fn is_running() -> bool {
    let pid_file = match pid_file_path() {
        Some(path) => path,
        None => return !list_runtime_pids().await.is_empty(),
    };

    if pid_file.exists() {
        let pid_bytes = match fs::read_to_string(&pid_file).await {
            Ok(b) => b,
            Err(_) => return !list_runtime_pids().await.is_empty(),
        };

        let pid = match pid_bytes.trim().parse::<u32>() {
            Ok(p) => p,
            Err(_) => {
                fs::remove_file(&pid_file).await.ok();
                return !list_runtime_pids().await.is_empty();
            }
        };

        if is_runtime_pid(pid).await {
            return true;
        }

        fs::remove_file(&pid_file).await.ok();
    }

    !list_runtime_pids().await.is_empty()
}

pub async fn start(
    profile: Option<String>,
    port: u16,
    foreground: bool,
) -> Result<(), color_eyre::Report> {
    let _engine_lock = crate::lifecycle::acquire("engine", Duration::from_secs(20)).await?;
    let model_id = resolve_model_id(profile);

    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir).await?;

    let log_stdout = cache_dir.join(LOG_STDOUT);
    let log_stderr = cache_dir.join(LOG_RE_ERR);
    let pid_file = cache_dir.join(PID_FILE_NAME);

    let existing_pids = list_runtime_pids().await;
    if !existing_pids.is_empty() {
        eprintln!(
            "warning: found running {} process(es), stopping before start",
            ENGINE_NAME
        );
        for pid in existing_pids {
            stop_pid(pid).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    fs::remove_file(&pid_file).await.ok();

    if foreground {
        eprintln!("info: {} starting on {}:{}", ENGINE_NAME, BIND_HOST, port);

        let mut child = AsyncCommand::new(EXECUTABLE);
        child
            .arg("--model")
            .arg(&model_id)
            .arg("--host")
            .arg(BIND_HOST)
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = child.spawn()?;
        let child_pid = child.id().unwrap_or_default();

        let active_model_path = cache_dir.join(ACTIVE_PRESET_FILE);
        fs::write(&active_model_path, &model_id).await?;

        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;

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
                        eprintln!("info: forwarding SIGTERM to {} pid {}", ENGINE_NAME, child_pid);
                        kill_runtime_target(child_pid, libc::SIGTERM);
                    }
                }
                _ = sigint.recv() => {
                    if child_pid != 0 {
                        eprintln!("info: forwarding SIGINT to {} pid {}", ENGINE_NAME, child_pid);
                        kill_runtime_target(child_pid, libc::SIGTERM);
                    }
                }
            }
        }
    }

    eprintln!(
        "{} starting (background) on {}:{}",
        ENGINE_NAME, BIND_HOST, port
    );

    let stdout_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_stdout)?;
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_stderr)?;

    let mut cmd = build_command(&model_id, BIND_HOST, port);
    cmd.stdout(stdout_file).stderr(stderr_file);

    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    match cmd.spawn() {
        Ok(c) => {
            let pid = c.id();
            fs::write(&pid_file, pid.to_string()).await?;
            let active_model_path = cache_dir.join(ACTIVE_PRESET_FILE);
            fs::write(&active_model_path, &model_id).await?;
            eprintln!("info: started pid {}", pid);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn stop() -> Result<(), color_eyre::Report> {
    let _engine_lock = crate::lifecycle::acquire("engine", Duration::from_secs(20)).await?;
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    let pid_file = cache_dir.join(PID_FILE_NAME);

    let mut target_pids = Vec::new();
    if pid_file.exists()
        && let Ok(pid_bytes) = fs::read_to_string(&pid_file).await
        && let Ok(pid) = pid_bytes.trim().parse::<u32>()
    {
        if is_runtime_pid(pid).await {
            target_pids.push(pid);
        } else {
            eprintln!(
                "warning: stale pid file for non-{} process {}, removing",
                ENGINE_NAME, pid
            );
        }
    }

    for pid in list_runtime_pids().await {
        if !target_pids.contains(&pid) {
            target_pids.push(pid);
        }
    }

    if target_pids.is_empty() {
        fs::remove_file(&pid_file).await.ok();
        return Ok(());
    }

    for pid in target_pids {
        stop_pid(pid).await;
    }

    fs::remove_file(&pid_file).await.ok();
    Ok(())
}

pub async fn stop_all() -> Result<(), color_eyre::Report> {
    stop().await
}

async fn stop_pid(pid: u32) {
    if !is_runtime_pid(pid).await {
        return;
    }

    eprintln!("info: stopping {} pid {}", ENGINE_NAME, pid);
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
        eprintln!("info: stopped {} pid {}", ENGINE_NAME, pid);
    } else {
        eprintln!(
            "warning: sigterm failed for {} pid {}, escalating to sigkill",
            ENGINE_NAME, pid
        );
        kill_runtime_target(pid, libc::SIGKILL);
        eprintln!("info: killed {} pid {}", ENGINE_NAME, pid);
    }
}

pub async fn status(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");

    let active_model = fs::read_to_string(cache_dir.join(ACTIVE_PRESET_FILE))
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let running = is_running().await;
    let any_model = !active_model.is_empty();

    let overall_state = if !any_model {
        "not_configured"
    } else if running {
        "running"
    } else {
        "configured_stopped"
    };

    if output == crate::OutputFormat::Json || output == crate::OutputFormat::Ndjson {
        let payload = serde_json::json!({
            "state": overall_state,
            ENGINE_NAME: {
                "model": if active_model.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(active_model.clone()) },
                "server_running": running,
            }
        });
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if !any_model {
        eprintln!("muthr: not configured");
    } else if running {
        eprintln!("muthr: running");
    } else {
        eprintln!("muthr: configured, stopped");
    }

    print_runtime_status(&active_model, running);

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

fn print_runtime_status(model_id: &str, is_running: bool) {
    if model_id.is_empty() {
        eprintln!("  engine {:<10} (none)", ENGINE_NAME);
    } else {
        eprintln!("  engine {:<10} active      {}", ENGINE_NAME, model_id);
    }

    if is_running {
        eprintln!("  server {:<10} running", ENGINE_NAME);
    } else {
        eprintln!("  server {:<10} stopped", ENGINE_NAME);
    }
}

pub fn presets(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let default_model = config::load()?
        .default_engine_profile
        .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string());

    if output == crate::OutputFormat::Json {
        let payload = vec![serde_json::json!({
            "id": default_model,
            "runtime": ENGINE_NAME,
        })];
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if output == crate::OutputFormat::Ndjson {
        let payload = serde_json::json!({
            "id": default_model,
            "runtime": ENGINE_NAME,
        });
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    eprintln!("{}", default_model);
    Ok(())
}
