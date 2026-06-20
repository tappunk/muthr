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
    if let Ok(out) = output {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(count) = digits.parse::<u32>() {
                return count;
            }
        }
    }

    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.physicalcpu"])
        .output();
    if let Ok(out) = output {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(count) = digits.parse::<u32>() {
                return count;
            }
        }
    }

    std::thread::available_parallelism()
        .map(|p| p.get() as u32)
        .unwrap_or(4)
}

fn clamp_threads(value: u32, max_threads: u32) -> u32 {
    if value > max_threads && value != 0 {
        eprintln!(
            "[WARN] Thread count {} exceeds physical CPU count ({}). Clamping to {}.",
            value, max_threads, max_threads
        );
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
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .await;
    if let Ok(out) = output {
        let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
        comm.ends_with("llama-server")
    } else {
        false
    }
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
                eprintln!("[ERR] No providers found in ~/.config/muthr/provider.d/");
                return Ok(());
            }

            let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
            match ui::select_list(&names) {
                Some(idx) => presets[idx].name.clone(),
                None => {
                    println!("[INFO] Cancelled.");
                    return Ok(());
                }
            }
        }
    };

    let preset_path = preset::resolve_preset(&target_profile)
        .ok_or_else(|| color_eyre::eyre::eyre!("Preset not found: {}", target_profile))?;

    apply_vram_limits(foreground).await;

    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir).await?;

    let tmp_preset = cache_dir.join("active-preset.ini");
    let raw_content = fs::read_to_string(&preset_path).await?;
    let expanded = raw_content.replace("~", &home);
    fs::write(&tmp_preset, expanded).await?;
    fs::write(cache_dir.join("active-preset-name"), &target_profile).await?;

    let preset = preset::parse_preset(&preset_path)?;
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
            let expanded_model = preset::expand_home(model_path);
            args.push("--model".to_string());
            args.push(expanded_model.to_string_lossy().to_string());
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
        println!("[PROC] Starting llama.cpp Server (Direct Mode)...");
        println!("   Binding Address : http://{}:{}", bind_host, server_port);
        println!("   Profile Target  : {:?}", preset_path);
        println!("   Press Ctrl+C to stop the server.\n");

        let mut child = AsyncCommand::new("llama-server")
            .args(&args)
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

    if !is_process_alive(pid) {
        println!(
            "[WARN] PID {} not found (stale PID file). Cleaning up.",
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
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if !is_process_alive(pid) {
            died = true;
            break;
        }
    }

    if died {
        println!("[ OK ] Server stopped gracefully. VRAM released.");
    } else {
        eprintln!("[WARN] Server did not respond to SIGTERM after 15s. Escalating to SIGKILL.");
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
    let active_path = PathBuf::from(&home).join(".cache/muthr/active-preset.ini");
    let name_path = PathBuf::from(&home).join(".cache/muthr/active-preset-name");

    if !active_path.exists() {
        println!("[STATUS] No active profile configured.");
        return Ok(());
    }

    let preset_name = fs::read_to_string(&name_path)
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let preset = if !preset_name.is_empty() {
        preset::resolve_preset(&preset_name).and_then(|p| preset::parse_preset(&p).ok())
    } else {
        None
    };

    let preset_basename = preset
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "unknown".to_string());

    println!("[STATUS] Active profile: {}", preset_basename);
    if let Some(p) = &preset {
        println!("   Preset:     {}", p.path.display());
    } else {
        println!("   Preset:     (not found on disk)");
    }

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
        println!("[WARN] No providers found in ~/.config/muthr/provider.d/");
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

    match ui::select_table(&headers, rows) {
        Some(idx) => println!("{}", presets[idx].name),
        None => {
            println!("Available presets:");
            println!(
                "==============================================================================="
            );
            for p in &presets {
                println!(
                    "  {:<30} {} [{}]",
                    p.name,
                    p.path.to_string_lossy(),
                    p.slots.len()
                );
            }
            println!(
                "==============================================================================="
            );
        }
    }

    Ok(())
}

async fn apply_vram_limits(foreground: bool) {
    let mem_bytes = sysctl_memsize().await;
    let threshold: u64 = 32 * 1024 * 1024 * 1024;

    if mem_bytes >= threshold {
        let gb = mem_bytes / 1024 / 1024 / 1024;
        let wired_mb = (mem_bytes / 1024 / 1024) * 85 / 100;
        let wired_mb = if wired_mb > 24576 { 24576 } else { wired_mb };

        if !foreground {
            println!(
                "[INFO] High-memory host detected ({}GB). Wired Metal VRAM: {}MB (background mode — apply with --foreground to set now)",
                gb, wired_mb
            );
            return;
        }

        println!(
            "[INFO] High-memory host detected ({}GB). Adjusting wired Metal VRAM limits...",
            gb
        );
        println!("[PROC] Adjusting Metal VRAM limits (requires sudo access)...");
        let _ = AsyncCommand::new("sudo")
            .args([
                "sysctl",
                "-w",
                &format!("iogpu.wired_limit_mb={}", wired_mb),
            ])
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
