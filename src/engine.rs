use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::fs;
use tokio::process::Command as AsyncCommand;

use crate::model;
use crate::preset;
use crate::ui;

pub async fn verify_health(port: u16) -> bool {
    model::verify_health("127.0.0.1", port).await
}

fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn physical_cpu_count() -> u32 {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.perflevel0.physicalcpu"])
        .output();
    match output {
        Ok(ref out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(count) = digits.parse::<u32>() {
                return count;
            }
        }
        Ok(_) | Err(_) => {}
    }

    std::thread::available_parallelism()
        .map(|p| p.get() as u32)
        .unwrap_or(4)
}

fn clamp_threads(value: u32, max_threads: u32) -> u32 {
    if value > max_threads && value != 0 {
        eprintln!("warning: clamping threads {} -> {}", value, max_threads);
        max_threads
    } else {
        value
    }
}

async fn is_llama_server_pid(pid: u32) -> bool {
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
            if comm != "llama-server" {
                continue;
            }
            let args = parts[1];
            if args.contains("--model") || args.contains("--host") {
                return true;
            }
        }
    }
    false
}

pub async fn is_running() -> bool {
    let home = std::env::var("HOME");
    let pid_file = match home {
        Ok(ref h) => PathBuf::from(h).join(".cache/muthr/llama-server.pid"),
        Err(_) => return false,
    };
    if !pid_file.exists() {
        return false;
    }
    let pid_bytes = match fs::read_to_string(&pid_file).await {
        Ok(b) => b,
        Err(_) => return false,
    };
    let pid = match pid_bytes.trim().parse::<u32>() {
        Ok(p) => p,
        Err(_) => return false,
    };
    is_llama_server_pid(pid).await
}

pub async fn start(
    profile: Option<String>,
    port: u16,
    foreground: bool,
) -> Result<(), color_eyre::Report> {
    let target_profile = match profile {
        Some(p) => p,
        None => {
            let presets = preset::list_presets()?;
            if presets.is_empty() {
                eprintln!("error: no presets in ~/.config/muthr/provider.d/");
                return Ok(());
            }

            let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
            match ui::select_list(&names) {
                Some(idx) => presets[idx].name.clone(),
                None => return Ok(()),
            }
        }
    };

    let preset_path = preset::resolve_preset(&target_profile)
        .ok_or_else(|| color_eyre::eyre::eyre!("preset not found: {}", target_profile))?;

    apply_vram_limits(foreground).await;

    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir).await?;

    let tmp_preset = cache_dir.join("active-preset.ini");
    let raw_content = fs::read_to_string(&preset_path).await?;
    let expanded = raw_content.replace("~", &home);
    fs::write(&tmp_preset, expanded).await?;
    fs::write(cache_dir.join("active-preset-name"), &target_profile).await?;

    let preset = preset::parse_preset(&tmp_preset)?;
    let bind_host = preset
        .global
        .host
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let server_port = preset.global.port.unwrap_or(port as u32) as u16;

    let max_threads = physical_cpu_count();

    let log_stdout = cache_dir.join("llama-server.log");
    let log_stderr = cache_dir.join("llama-server-err.log");
    let pid_file = cache_dir.join("llama-server.pid");

    if !foreground && pid_file.exists() {
        let stale = || async { fs::remove_file(&pid_file).await.ok() };

        if let Ok(pid_bytes) = fs::read_to_string(&pid_file).await
            && let Ok(old_pid) = pid_bytes.trim().parse::<u32>()
        {
            if is_llama_server_pid(old_pid).await {
                eprintln!(
                    "warning: server already running (pid {}), stopping first",
                    old_pid
                );
                let _ = stop().await;
                fs::remove_file(&pid_file).await.ok();
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            } else {
                stale().await;
            }
        }
    }

    let mut args: Vec<String> = vec![
        "--host".to_string(),
        bind_host.clone(),
        "--port".to_string(),
        server_port.to_string(),
        "--reuse-port".to_string(),
        "--prio".to_string(),
        "2".to_string(),
        "--n-gpu-layers".to_string(),
        preset.global.n_gpu_layers.to_string(),
    ];

    if preset.global.flash_attn {
        args.push("--flash-attn".to_string());
        args.push("on".to_string());
    }
    if preset.global.mlock {
        args.push("--mlock".to_string());
    }

    if let Some(slot) = preset.slots.first() {
        if let Some(model_path) = &slot.model_path {
            let expanded_model = preset::expand_home(model_path);
            args.push("--model".to_string());
            args.push(expanded_model.to_string_lossy().to_string());
        }

        if let Some(ctx) = slot.ctx_size {
            args.push("--ctx-size".to_string());
            args.push(ctx.to_string());
        }
        args.push("--ctx-checkpoints".to_string());
        args.push("0".to_string());
        if let Some(k) = &slot.cache_type_k {
            args.push("--cache-type-k".to_string());
            args.push(k.clone());
        }
        if let Some(v) = &slot.cache_type_v {
            args.push("--cache-type-v".to_string());
            args.push(v.clone());
        }
        if let Some(cache_ram) = slot.cache_ram {
            args.push("--cache-ram".to_string());
            args.push(cache_ram.to_string());
        }
        if let Some(temp) = slot.temp {
            args.push("--temperature".to_string());
            args.push(temp.to_string());
        }
        if let Some(top_p) = slot.top_p {
            args.push("--top-p".to_string());
            args.push(top_p.to_string());
        }
        if let Some(min_p) = slot.min_p {
            args.push("--min-p".to_string());
            args.push(min_p.to_string());
        }
        if let Some(top_k) = slot.top_k {
            args.push("--top-k".to_string());
            args.push(top_k.to_string());
        }
        if let Some(rp) = slot.repeat_penalty {
            args.push("--repeat-penalty".to_string());
            args.push(rp.to_string());
        }
        if slot.jinja == Some(true) {
            args.push("--jinja".to_string());
        }
        if let Some(parallel) = slot.parallel {
            args.push("--parallel".to_string());
            args.push(parallel.to_string());
        }
    }

    if let Some(b) = preset.global.batch_size {
        args.push("--batch-size".to_string());
        args.push(b.to_string());
    }
    if let Some(ub) = preset.global.ubatch_size {
        args.push("--ubatch-size".to_string());
        args.push(ub.to_string());
    }
    if let Some(t) = preset.global.threads {
        let clamped = clamp_threads(t, max_threads);
        args.push("--threads".to_string());
        args.push(clamped.to_string());
    }
    if let Some(tb) = preset.global.threads_batch {
        let clamped = clamp_threads(tb, max_threads);
        args.push("--threads-batch".to_string());
        args.push(clamped.to_string());
    }

    if foreground {
        eprintln!(
            "info: llama-server starting on {}:{}",
            bind_host, server_port
        );

        let mut child = AsyncCommand::new("llama-server")
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
            "llama-server starting (background) on {}:{}",
            bind_host, server_port
        );

        let stdout_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stdout)?;
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stderr)?;

        let mut cmd = std::process::Command::new("llama-server");
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

pub async fn stop() -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    let pid_file = cache_dir.join("llama-server.pid");

    if !pid_file.exists() {
        return Ok(());
    }

    let pid_bytes = fs::read_to_string(&pid_file).await?;
    let pid = pid_bytes.trim().parse::<u32>()?;

    if !is_llama_server_pid(pid).await {
        eprintln!(
            "warning: stale pid file for non-llama process {}, removing",
            pid
        );
        fs::remove_file(&pid_file).await.ok();
        return Ok(());
    }

    if !is_process_alive(pid) {
        fs::remove_file(&pid_file).await.ok();
        return Ok(());
    }

    eprintln!("info: stopping pid {}", pid);
    let _ = AsyncCommand::new("kill")
        .args(["-15", &pid.to_string()])
        .output()
        .await;

    let mut died = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if !is_process_alive(pid) {
            died = true;
            break;
        }
    }

    if died {
        eprintln!("info: stopped");
    } else {
        eprintln!("warning: sigterm failed, escalating to sigkill");
        let _ = AsyncCommand::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .await;
        eprintln!("info: killed");
    }

    fs::remove_file(&pid_file).await.ok();
    Ok(())
}

pub async fn status(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let name_path = PathBuf::from(&home).join(".cache/muthr/active-preset-name");

    let preset_name = fs::read_to_string(&name_path)
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let is_server_running = is_running().await;

    let overall_state = if preset_name.is_empty() {
        "not_configured"
    } else if is_server_running {
        "running"
    } else {
        "configured_stopped"
    };

    if output == crate::OutputFormat::Json || output == crate::OutputFormat::Ndjson {
        let payload = serde_json::json!({
            "state": overall_state,
            "preset": if preset_name.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(preset_name.clone()) },
            "server_running": is_server_running,
        });
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if preset_name.is_empty() {
        eprintln!("muthr: not configured");
    } else if is_server_running {
        eprintln!("muthr: running");
    } else {
        eprintln!("muthr: configured, stopped");
    }

    if preset_name.is_empty() {
        eprintln!("  engine          (none)");
    } else {
        let preset =
            preset::resolve_preset(&preset_name).and_then(|p| preset::parse_preset(&p).ok());
        let profile_label = preset
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or_else(|| preset_name.as_str());
        eprintln!("  engine          active      {}", profile_label);
    }

    if is_server_running {
        eprintln!("  server          running");
    } else {
        eprintln!("  server          stopped");
    }

    let services_vm = "muthr-services";
    let services_output = AsyncCommand::new("limactl")
        .args(["ls", "-f", "{{.Status}}", services_vm])
        .output()
        .await
        .ok();

    let services_status = match services_output {
        Some(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            if raw.contains("Running") {
                Some("running")
            } else {
                Some("stopped")
            }
        }
        _ => None,
    };

    if let Some(status) = services_status {
        eprintln!("  services vm      {}     {}", services_vm, status);

        let provision_output = AsyncCommand::new("limactl")
            .args([
                "shell",
                services_vm,
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

    let sandbox_output = AsyncCommand::new("limactl")
        .args(["ls", "-f", "{{.Name}} {{.Status}}"])
        .output()
        .await;

    if let Ok(ref out) = sandbox_output {
        let stdout_str = String::from_utf8_lossy(&out.stdout);
        let mut active_sandboxes: Vec<(String, String)> = Vec::new();

        for line in stdout_str.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let vm_name = parts[0];
                let vm_status = parts[1];

                if vm_name.starts_with("muthr-") && vm_name != "muthr-services" {
                    let project_token = vm_name.strip_prefix("muthr-").unwrap_or(vm_name);
                    active_sandboxes.push((project_token.to_string(), vm_status.to_string()));
                }
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
    }

    Ok(())
}

pub fn presets(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let presets = preset::list_presets()?;
    if presets.is_empty() {
        eprintln!("error: no presets in ~/.config/muthr/provider.d/");
        return Ok(());
    }

    if output == crate::OutputFormat::Json {
        let payload: Vec<serde_json::Value> = presets
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "path": p.path.to_string_lossy().to_string(),
                    "slots": p.slots.len(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if output == crate::OutputFormat::Ndjson {
        for p in &presets {
            let payload = serde_json::json!({
                "name": p.name,
                "path": p.path.to_string_lossy().to_string(),
                "slots": p.slots.len(),
            });
            println!("{}", serde_json::to_string(&payload)?);
        }
        return Ok(());
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    for p in &presets {
        rows.push(vec![
            p.name.clone(),
            p.path.to_string_lossy().to_string(),
            p.slots.len().to_string(),
        ]);
    }

    let headers = vec!["Preset", "Path", "Slots"];

    match ui::select_table(&headers, &rows) {
        Some(idx) => eprintln!("{}", presets[idx].name),
        None => {
            for p in &presets {
                eprintln!(
                    "  {:<30} {} [{}]",
                    p.name,
                    p.path.to_string_lossy(),
                    p.slots.len()
                );
            }
        }
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

        let status = AsyncCommand::new("sudo")
            .args([
                "sysctl",
                "-w",
                &format!("iogpu.wired_limit_mb={}", wired_mb),
            ])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await;

        match status {
            Ok(s) if s.success() => eprintln!("info: iogpu limits applied"),
            _ => {
                eprintln!("warning: iogpu tuning declined or timed out");
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
