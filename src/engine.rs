use std::os::fd::{FromRawFd, IntoRawFd};
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

async fn is_llama_server_pid(pid: u32) -> bool {
    let output = AsyncCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .await;
    if let Ok(out) = output {
        let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
        comm.contains("llama-server")
    } else {
        false
    }
}

fn expand_path(path: &str) -> String {
    if path.starts_with('~') {
        if let Ok(home) = std::env::var("HOME") {
            return path.replacen('~', &home, 1);
        }
    }
    path.to_string()
}

pub async fn serve(
    profile: Option<String>,
    port: u16,
    foreground: bool,
) -> Result<(), color_eyre::Report> {
    let target_profile = match profile {
        Some(p) => p,
        None => {
            let presets = preset::list_presets()?;
            if presets.is_empty() {
                eprintln!("[ERR] No presets found in ~/.config/muthr/llama/presets/");
                return Ok(());
            }

            let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
            match ui::select_list(&names) {
                Some(idx) => presets[idx].name.clone(),
                None => {
                    eprintln!("[INFO] Cancelled.");
                    return Ok(());
                }
            }
        }
    };

    let preset_path = preset::resolve_preset(&target_profile)
        .ok_or_else(|| color_eyre::eyre::eyre!("Preset not found: {}", target_profile))?;

    let opencode_config_path = preset::resolve_opencode_config(&target_profile);
    if let Some(path) = &opencode_config_path {
        println!("[ OK ] Config found: {:?}", path);
    } else {
        eprintln!(
            "[WARN] No matching opencode config at ~/.config/opencode/opencode-{}.json",
            target_profile
        );
    }

    apply_vram_limits().await;

    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir).await?;

    let tmp_preset = cache_dir.join("active-preset.ini");
    let raw_content = fs::read_to_string(&preset_path).await?;
    let expanded = raw_content.replace('~', &home);
    fs::write(&tmp_preset, expanded).await?;

    let preset = preset::parse_preset(&preset_path)?;
    let bind_host = preset
        .global
        .host
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let server_port = preset.global.port.unwrap_or(port as u32) as u16;

    let log_stdout = cache_dir.join("llama-server.log");
    let log_stderr = cache_dir.join("llama-server-err.log");
    let pid_file = cache_dir.join("llama-server.pid");

    if !foreground && pid_file.exists() {
        if let Ok(pid_bytes) = fs::read_to_string(&pid_file).await {
            if let Ok(old_pid) = pid_bytes.trim().parse::<u32>() {
                if is_llama_server_pid(old_pid).await {
                    eprintln!(
                        "[WARN] Server already running (PID {}). Stopping first.",
                        old_pid
                    );
                    let _ = stop().await;
                    fs::remove_file(&pid_file).await.ok();
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                } else {
                    fs::remove_file(&pid_file).await.ok();
                }
            }
        }
    }

    // === Build direct mode arguments ===
    let mut args: Vec<String> = vec![
        "--host".to_string(),
        bind_host.clone(),
        "--port".to_string(),
        server_port.to_string(),
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
            let expanded_model = expand_path(&model_path.to_string_lossy());
            args.push("--model".to_string());
            args.push(expanded_model);
        }

        if let Some(ctx) = slot.ctx_size {
            args.push("--ctx-size".to_string());
            args.push(ctx.to_string());
        }
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
        args.push("--threads".to_string());
        args.push(t.to_string());
    }
    if let Some(tb) = preset.global.threads_batch {
        args.push("--threads-batch".to_string());
        args.push(tb.to_string());
    }

    let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    persist_profile(
        &target_profile,
        preset_path.to_str().unwrap(),
        &opencode_config_path,
    )
    .await?;

    if foreground {
        println!("[PROC] Starting llama.cpp Server (Direct Mode)...");
        println!("   Binding Address : http://{}:{}", bind_host, server_port);
        println!("   Profile Target  : {:?}", preset_path);
        println!("   Press Ctrl+C to stop the server.\n");

        let mut child = AsyncCommand::new("llama-server")
            .args(&args_str)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        let status = child.wait().await?;
        if !status.success() {
            eprintln!("[WARN] Server exited with code: {}", status);
        }
        Ok(())
    } else {
        println!("[PROC] Starting llama.cpp Server in background (Direct Mode)...");
        println!("   Binding Address : http://{}:{}", bind_host, server_port);
        println!("   Profile Target  : {:?}", preset_path);
        println!("   Log file        : {:?}", log_stdout);
        println!("   Error log       : {:?}", log_stderr);
        println!();

        let stdout_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stdout)?;
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stderr)?;

        let stdout_fd = stdout_file.into_raw_fd();
        let stderr_fd = stderr_file.into_raw_fd();

        let child = unsafe {
            std::process::Command::new("llama-server")
                .args(&args_str)
                .stdout(Stdio::from_raw_fd(stdout_fd))
                .stderr(Stdio::from_raw_fd(stderr_fd))
                .pre_exec(|| {
                    let _ = libc::setsid();
                    Ok(())
                })
                .spawn()
        };

        match child {
            Ok(c) => {
                let pid = c.id();
                fs::write(&pid_file, pid.to_string()).await?;
                println!("[ OK ] Server started (PID {})", pid);
                println!("   Run 'muthr stop' to stop the server.");
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
        println!("[WARN] No PID file found — server may not be running.");
        return Ok(());
    }

    let pid_bytes = fs::read_to_string(&pid_file).await?;
    let pid = pid_bytes.trim().parse::<u32>()?;

    if !is_llama_server_pid(pid).await {
        println!(
            "[WARN] PID {} is not llama-server (stale PID file). Cleaning up.",
            pid
        );
        fs::remove_file(&pid_file).await.ok();
        return Ok(());
    }

    println!(
        "[PROC] Initiating graceful shutdown (SIGTERM) for PID {}...",
        pid
    );
    let _ = AsyncCommand::new("kill")
        .args(["-15", &pid.to_string()])
        .output()
        .await;

    let mut died = false;
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if !is_llama_server_pid(pid).await {
            died = true;
            break;
        }
    }

    if died {
        println!("[ OK ] Server stopped gracefully. VRAM released.");
    } else {
        eprintln!("[WARN] Server did not respond to SIGTERM. Escalating to SIGKILL.");
        let _ = AsyncCommand::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .await;
        println!("[ OK ] Server force-killed.");
    }

    fs::remove_file(&pid_file).await.ok();
    Ok(())
}

pub async fn status() -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let profile_path = PathBuf::from(&home).join(".cache/muthr/opencode-profile");

    if !profile_path.exists() {
        println!("[STATUS] No active profile configured.");
        return Ok(());
    }

    let content = fs::read_to_string(&profile_path).await?;
    let mut preset_path = String::new();
    let mut config_path = String::new();

    for line in content.lines() {
        if line.starts_with("export LLAMA_ARG_MODELS_PRESET=") {
            preset_path = parse_export_value(line);
        } else if line.starts_with("export OPENCODE_CONFIG=") {
            config_path = parse_export_value(line);
        }
    }

    let preset_basename: String = PathBuf::from(&preset_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(String::from)
        .unwrap_or(preset_path.clone());

    println!("[STATUS] Active profile: {}", preset_basename);
    println!("   Preset:     {}", preset_path);
    println!("   OpenCode:   {}", config_path);

    let is_running = AsyncCommand::new("pgrep")
        .arg("-x")
        .arg("llama-server")
        .output()
        .await?
        .status
        .success();

    if is_running {
        println!("   Server:     running");
    } else {
        println!("   Server:     stopped");
    }

    Ok(())
}

pub fn list() -> Result<(), color_eyre::Report> {
    let presets = preset::list_presets()?;
    if presets.is_empty() {
        println!("[WARN] No presets found in ~/.config/muthr/llama/presets/");
        return Ok(());
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    for p in &presets {
        let config_status = if preset::resolve_opencode_config(&p.name).is_some() {
            "✓".to_string()
        } else {
            "✗".to_string()
        };
        rows.push(vec![
            p.name.clone(),
            p.path.to_string_lossy().to_string(),
            p.slots.len().to_string(),
            config_status,
        ]);
    }

    let headers = vec!["Preset", "Path", "Slots", "Config"];

    match ui::select_table(&headers, rows) {
        Some(idx) => println!("{}", presets[idx].name),
        None => {
            println!("Available presets:");
            println!(
                "==============================================================================="
            );
            for p in &presets {
                let config_status = if preset::resolve_opencode_config(&p.name).is_some() {
                    "✓"
                } else {
                    "✗"
                };
                println!(
                    "  {:<30} {} [{}] {}",
                    p.name,
                    p.path.to_string_lossy(),
                    p.slots.len(),
                    config_status
                );
            }
            println!(
                "==============================================================================="
            );
        }
    }

    Ok(())
}

fn parse_export_value(line: &str) -> String {
    if let Some(start) = line.find('"') {
        if let Some(end) = line[start + 1..].find('"') {
            return line[start + 1..start + 1 + end].to_string();
        }
    }
    String::new()
}

async fn apply_vram_limits() {
    let mem_bytes = sysctl_memsize().await;
    let threshold: u64 = 32 * 1024 * 1024 * 1024;

    if mem_bytes >= threshold {
        let gb = mem_bytes / 1024 / 1024 / 1024;
        println!(
            "[INFO] High-memory host detected ({}GB). Adjusting wired Metal VRAM limits...",
            gb
        );
        let _ = AsyncCommand::new("sudo")
            .args(["sysctl", "-w", "iogpu.wired_limit_mb=43000"])
            .output()
            .await;
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

async fn persist_profile(
    _preset: &str,
    preset_path: &str,
    config_path: &Option<PathBuf>,
) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir).await?;
    let config_file = cache_dir.join("opencode-profile");

    let config_value = config_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "not set".to_string());

    let content = format!(
        "export LLAMA_ARG_MODELS_PRESET=\"{}\"\nexport OPENCODE_CONFIG=\"{}\"",
        preset_path, config_value
    );
    fs::write(&config_file, content).await.map_err(|e| {
        color_eyre::eyre::eyre!(
            "Failed to persist profile to {}: {}",
            config_file.display(),
            e
        )
    })?;

    Ok(())
}
