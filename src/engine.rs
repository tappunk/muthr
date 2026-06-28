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

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::fs;
use tokio::process::Command as AsyncCommand;

use crate::model;
use crate::preset;
use crate::ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum EngineRuntime {
    #[value(name = "mlxcel")]
    Mlxcel,
}

impl EngineRuntime {
    fn as_str(self) -> &'static str {
        "mlxcel"
    }

    fn pid_file_name(self) -> &'static str {
        "mlxcel-server.pid"
    }

    fn stdout_log_name(self) -> &'static str {
        "mlxcel-server.log"
    }

    fn stderr_log_name(self) -> &'static str {
        "mlxcel-server-err.log"
    }

    fn executable(self) -> &'static str {
        "mlxcel-server"
    }

    fn active_preset_file_name(self) -> &'static str {
        "active-preset-name-mlxcel"
    }
}

impl std::fmt::Display for EngineRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

pub async fn verify_health(port: u16) -> bool {
    model::verify_health("127.0.0.1", port).await
}

fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
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

fn matches_runtime_process(comm: &str, args: &str) -> bool {
    comm == "mlxcel-server"
        || comm == "mlxcel"
        || args.contains(" mlxcel-server")
        || args.contains("/mlxcel-server")
        || (args.contains("mlxcel") && args.contains(" serve "))
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

pub async fn is_running(runtime: EngineRuntime) -> bool {
    let home = std::env::var("HOME");
    let pid_file = match home {
        Ok(ref h) => PathBuf::from(h).join(format!(".cache/muthr/{}", runtime.pid_file_name())),
        Err(_) => return false,
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
    runtime: EngineRuntime,
) -> Result<(), color_eyre::Report> {
    let target_profile = match profile {
        Some(p) => p,
        None => {
            let presets = preset::list_presets()?;
            if presets.is_empty() {
                eprintln!("none");
                eprintln!("info: no presets in ~/.config/muthr/provider.d/mlxcel");
                eprintln!("info: run 'muthr init' to install default presets");
                return Ok(());
            }

            let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
            match ui::select_list(&names) {
                Some(idx) => presets[idx].name.clone(),
                None => return Ok(()),
            }
        }
    };

    let preset_path = preset::resolve_preset(&target_profile).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "preset not found: {} (run 'muthr init' to install default presets)",
            target_profile
        )
    })?;

    apply_vram_limits(foreground).await;

    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir).await?;

    let tmp_preset = cache_dir.join("active-preset.ini");
    let raw_content = fs::read_to_string(&preset_path).await?;
    let mut expanded_lines: Vec<String> = Vec::new();
    for line in raw_content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            expanded_lines.push(line.to_string());
            continue;
        }

        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_ascii_lowercase();
            let is_path_like = key == "model" || key == "path" || key.ends_with("-path");
            if is_path_like {
                let value = line[eq_pos + 1..].trim();
                let expanded_value = if value == "~" {
                    Some(home.clone())
                } else {
                    value
                        .strip_prefix("~/")
                        .map(|stripped| format!("{}/{}", home, stripped))
                };

                if let Some(expanded_value) = expanded_value {
                    expanded_lines.push(format!(
                        "{} = {}",
                        line[..eq_pos].trim_end(),
                        expanded_value
                    ));
                    continue;
                }
            }
        }

        expanded_lines.push(line.to_string());
    }

    let expanded = expanded_lines.join("\n");
    fs::write(&tmp_preset, expanded).await?;
    fs::write(
        cache_dir.join(runtime.active_preset_file_name()),
        &target_profile,
    )
    .await?;

    let preset = preset::parse_preset(&tmp_preset)?;
    let bind_host = preset
        .global
        .host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let server_port = preset.global.port.unwrap_or(port as u32) as u16;

    let log_stdout = cache_dir.join(runtime.stdout_log_name());
    let log_stderr = cache_dir.join(runtime.stderr_log_name());
    let pid_file = cache_dir.join(runtime.pid_file_name());

    if !foreground && pid_file.exists() {
        let stale = || async { fs::remove_file(&pid_file).await.ok() };

        if let Ok(pid_bytes) = fs::read_to_string(&pid_file).await
            && let Ok(old_pid) = pid_bytes.trim().parse::<u32>()
        {
            if is_runtime_pid(old_pid).await {
                eprintln!(
                    "warning: server already running (pid {}), stopping first",
                    old_pid
                );
                let _ = stop(runtime).await;
                fs::remove_file(&pid_file).await.ok();
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            } else {
                stale().await;
            }
        }
    }

    let args = build_mlxcel_args(&preset, bind_host.clone(), server_port)?;

    if foreground {
        eprintln!(
            "info: {} starting on {}:{}",
            runtime, bind_host, server_port
        );

        let mut child = AsyncCommand::new(runtime.executable())
            .args(&args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        let status = child.wait().await?;
        if !status.success() {
            eprintln!("error: server exited with code {}", status);
        }
        Ok(())
    } else {
        eprintln!(
            "{} starting (background) on {}:{}",
            runtime, bind_host, server_port
        );

        let stdout_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stdout)?;
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stderr)?;

        let mut cmd = std::process::Command::new(runtime.executable());
        cmd.args(&args).stdout(stdout_file).stderr(stderr_file);

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
                eprintln!("info: started pid {}", pid);
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}

fn build_mlxcel_args(
    preset: &preset::Preset,
    bind_host: String,
    server_port: u16,
) -> Result<Vec<String>, color_eyre::Report> {
    let mut args: Vec<String> = Vec::new();

    if let Some(slot) = preset.slots.first()
        && let Some(model_path) = &slot.model_path
    {
        let expanded_model = preset::expand_home(model_path);
        args.push("-m".to_string());
        args.push(expanded_model.to_string_lossy().to_string());
    } else {
        return Err(color_eyre::eyre::eyre!(
            "preset '{}' has no model configured; define 'model = ...' in the first slot",
            preset.name
        ));
    }

    args.push("--host".to_string());
    args.push(bind_host);
    args.push("--port".to_string());
    args.push(server_port.to_string());

    if let Some(slot) = preset.slots.first() {
        if let Some(max_out) = slot.max_output_tokens {
            args.push("--predict".to_string());
            args.push(max_out.to_string());
        }
        if let Some(temp) = slot.temp {
            args.push("--temp".to_string());
            args.push(temp.to_string());
        }
        if let Some(top_p) = slot.top_p {
            args.push("--top-p".to_string());
            args.push(top_p.to_string());
        }
        if let Some(top_k) = slot.top_k {
            args.push("--top-k".to_string());
            args.push(top_k.to_string());
        }
        if let Some(min_p) = slot.min_p {
            args.push("--min-p".to_string());
            args.push(min_p.to_string());
        }
        if let Some(rp) = slot.repeat_penalty {
            args.push("--repeat-penalty".to_string());
            args.push(rp.to_string());
        }
    }

    Ok(args)
}

pub async fn stop(runtime: EngineRuntime) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    let pid_file = cache_dir.join(runtime.pid_file_name());

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
                runtime, pid,
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
        stop_pid(pid, runtime).await;
    }

    fs::remove_file(&pid_file).await.ok();
    Ok(())
}

pub async fn stop_all() -> Result<(), color_eyre::Report> {
    stop(EngineRuntime::Mlxcel).await?;
    Ok(())
}

async fn stop_pid(pid: u32, runtime: EngineRuntime) {
    if !is_process_alive(pid) {
        return;
    }

    eprintln!("info: stopping {} pid {}", runtime, pid);
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }

    let mut died = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if !is_process_alive(pid) {
            died = true;
            break;
        }
    }

    if died {
        eprintln!("info: stopped {} pid {}", runtime, pid);
    } else {
        eprintln!(
            "warning: sigterm failed for {} pid {}, escalating to sigkill",
            runtime, pid
        );
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        eprintln!("info: killed {} pid {}", runtime, pid);
    }
}

pub async fn status(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");

    let mlxcel_preset_name =
        fs::read_to_string(cache_dir.join(EngineRuntime::Mlxcel.active_preset_file_name()))
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

    let mlxcel_running = is_running(EngineRuntime::Mlxcel).await;
    let any_running = mlxcel_running;
    let any_preset = !mlxcel_preset_name.is_empty();

    let overall_state = if !any_preset {
        "not_configured"
    } else if any_running {
        "running"
    } else {
        "configured_stopped"
    };

    if output == crate::OutputFormat::Json || output == crate::OutputFormat::Ndjson {
        let payload = serde_json::json!({
            "state": overall_state,
            "mlxcel": {
                "preset": if mlxcel_preset_name.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(mlxcel_preset_name.clone()) },
                "server_running": mlxcel_running,
            }
        });
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if !any_preset {
        eprintln!("muthr: not configured");
    } else if any_running {
        eprintln!("muthr: running");
    } else {
        eprintln!("muthr: configured, stopped");
    }

    print_runtime_status(EngineRuntime::Mlxcel, &mlxcel_preset_name, mlxcel_running);

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

fn print_runtime_status(runtime: EngineRuntime, preset_name: &str, is_running: bool) {
    if preset_name.is_empty() {
        eprintln!("  engine {:<10} (none)", runtime);
    } else {
        let preset =
            preset::resolve_preset(preset_name).and_then(|p| preset::parse_preset(&p).ok());
        let profile_label = preset
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or(preset_name);
        eprintln!("  engine {:<10} active      {}", runtime, profile_label);
    }

    if is_running {
        eprintln!("  server {:<10} running", runtime);
    } else {
        eprintln!("  server {:<10} stopped", runtime);
    }
}

pub fn presets(
    output: crate::OutputFormat,
    _runtime: EngineRuntime,
) -> Result<(), color_eyre::Report> {
    let runtime = EngineRuntime::Mlxcel;
    let presets = preset::list_presets()?;
    if presets.is_empty() {
        if output == crate::OutputFormat::Json {
            println!("[]");
        } else if output == crate::OutputFormat::Ndjson {
        } else {
            eprintln!("none");
            eprintln!("info: no presets in ~/.config/muthr/provider.d/{}", runtime);
            eprintln!("info: run 'muthr init' to install default presets");
        }
        return Ok(());
    }

    if output == crate::OutputFormat::Json {
        let payload: Vec<serde_json::Value> = presets
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": format!("{}/{}.ini", runtime.as_str(), p.name),
                    "runtime": runtime.as_str(),
                    "file": format!("{}.ini", p.name),
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if output == crate::OutputFormat::Ndjson {
        for p in &presets {
            let payload = serde_json::json!({
                "id": format!("{}/{}.ini", runtime.as_str(), p.name),
                "runtime": runtime.as_str(),
                "file": format!("{}.ini", p.name),
            });
            println!("{}", serde_json::to_string(&payload)?);
        }
        return Ok(());
    }

    for p in &presets {
        eprintln!("{}/{}.ini", runtime.as_str(), p.name);
    }

    Ok(())
}

async fn apply_vram_limits(_foreground: bool) {
    let mem_bytes = sysctl_memsize().await;
    let threshold: u64 = 32 * 1024 * 1024 * 1024;

    if mem_bytes >= threshold {
        let gb = mem_bytes / 1024 / 1024 / 1024;

        let wired_mb = (mem_bytes / 1024 / 1024) * 85 / 100;

        if !std::io::stdin().is_terminal() {
            eprintln!(
                "info: skipping iogpu tuning in non-interactive session, run 'sudo sysctl -w iogpu.wired_limit_mb={}' manually",
                wired_mb
            );
            return;
        }

        eprintln!(
            "info: tuning iogpu.wired_limit_mb={} ({}gb host)",
            wired_mb, gb
        );

        let status = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            AsyncCommand::new("sudo")
                .args([
                    "-n",
                    "sysctl",
                    "-w",
                    &format!("iogpu.wired_limit_mb={}", wired_mb),
                ])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status(),
        )
        .await;

        match status {
            Ok(Ok(s)) if s.success() => eprintln!("info: iogpu limits applied"),
            _ => {
                eprintln!("warning: iogpu tuning skipped (sudo non-interactive unavailable)");
                eprintln!(
                    "info: run 'sudo sysctl -w iogpu.wired_limit_mb={}' manually",
                    wired_mb
                );
            }
        }
    }
}

async fn sysctl_memsize() -> u64 {
    let output = AsyncCommand::new("sysctl")
        .arg("-n")
        .arg("hw.memsize")
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        }
        _ => 0,
    }
}
