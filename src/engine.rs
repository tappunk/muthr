use std::fs;
use std::io::IsTerminal;
use std::os::fd::{FromRawFd, IntoRawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::model;
use crate::preset;
use crate::ui;

pub async fn verify_health(port: u16) -> bool {
    model::verify_health("127.0.0.1", port).await
}

fn is_llama_server_pid(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output();
    if let Ok(out) = output {
        let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
        comm.contains("llama-server")
    } else {
        false
    }
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

    apply_vram_limits();

    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir)?;

    let tmp_preset = cache_dir.join("active-preset.ini");
    let raw_content = fs::read_to_string(&preset_path)?;
    let expanded = raw_content.replace("~", &home);
    fs::write(&tmp_preset, expanded)?;

    let preset = preset::parse_preset(&preset_path)?;
    let bind_host = preset.global.host.unwrap_or_else(|| "0.0.0.0".to_string());
    let server_port = preset.global.port.unwrap_or(port as u32) as u16;
    let use_jinja = preset.slots.first().is_some_and(|s| s.jinja == Some(true));

    let log_stdout = cache_dir.join("llama-server.log");
    let log_stderr = cache_dir.join("llama-server-err.log");
    let pid_file = cache_dir.join("llama-server.pid");

    if !foreground && pid_file.exists() {
        if let Ok(pid_bytes) = fs::read_to_string(&pid_file) {
            if let Ok(old_pid) = pid_bytes.trim().parse::<u32>() {
                if is_llama_server_pid(old_pid) {
                    eprintln!(
                        "[WARN] Server already running (PID {}). Stopping first.",
                        old_pid
                    );
                    let _ = stop();
                    fs::remove_file(&pid_file).ok();
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                } else {
                    fs::remove_file(&pid_file).ok();
                }
            }
        }
    }

    if foreground {
        println!("[PROC] Starting llama.cpp Server via muthr engine...");
        println!("   Binding Address : http://{}:{}", bind_host, server_port);
        println!("   Profile Target  : {:?}", preset_path);
        println!("   Press Ctrl+C to stop the server.");
        println!();

        let preset_str = tmp_preset.to_string_lossy();
        let port_str = server_port.to_string();
        let mut args: Vec<&str> = vec![
            "--models-preset",
            &preset_str,
            "--port",
            &port_str,
            "--host",
            &bind_host,
            "--prio",
            "2",
        ];
        if use_jinja {
            args.push("--jinja");
        }

        let mut child = Command::new("llama-server")
            .args(&args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        persist_profile(
            &target_profile,
            preset_path.to_str().unwrap(),
            &opencode_config_path,
        )?;

        let status = child.wait()?;
        if !status.success() {
            eprintln!("[WARN] Server exited with code: {}", status);
        }

        Ok(())
    } else {
        println!("[PROC] Starting llama.cpp Server in background...");
        println!("   Binding Address : http://{}:{}", bind_host, server_port);
        println!("   Profile Target  : {:?}", preset_path);
        println!("   Log file        : {:?}", log_stdout);
        println!("   Error log       : {:?}", log_stderr);
        println!();

        let stdout_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stdout)?;
        let stderr_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_stderr)?;

        let stdout_fd = stdout_file.into_raw_fd();
        let stderr_fd = stderr_file.into_raw_fd();

        let preset_str = tmp_preset.to_string_lossy();
        let port_str = server_port.to_string();
        let mut args: Vec<&str> = vec![
            "--models-preset",
            &preset_str,
            "--port",
            &port_str,
            "--host",
            &bind_host,
            "--prio",
            "2",
        ];
        if use_jinja {
            args.push("--jinja");
        }

        let child = unsafe {
            Command::new("llama-server")
                .args(&args)
                .stdout(Stdio::from_raw_fd(stdout_fd))
                .stderr(Stdio::from_raw_fd(stderr_fd))
                .pre_exec(|| {
                    let _ = libc::setsid();
                    Ok(())
                })
                .spawn()?
        };

        let pid = child.id();
        fs::write(&pid_file, pid.to_string())?;
        drop(child);

        persist_profile(
            &target_profile,
            preset_path.to_str().unwrap(),
            &opencode_config_path,
        )?;

        println!("[ OK ] Server started (PID {})", pid);
        println!("   Run 'muthr stop' to stop the server.");

        Ok(())
    }
}

pub fn stop() -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    let pid_file = cache_dir.join("llama-server.pid");

    if !pid_file.exists() {
        println!("[WARN] No PID file found — server may not be running.");
        return Ok(());
    }

    let pid_bytes = fs::read_to_string(&pid_file)?;
    let pid = pid_bytes.trim().parse::<u32>()?;

    if !is_llama_server_pid(pid) {
        println!(
            "[WARN] PID {} is not llama-server (stale PID file). Cleaning up.",
            pid
        );
        fs::remove_file(&pid_file).ok();
        return Ok(());
    }

    let result = Command::new("kill").args(["-9", &pid.to_string()]).output();

    match result {
        Ok(out) if out.status.success() => {
            println!("[ OK ] Server stopped (PID {})", pid);
        }
        Ok(_) => {
            eprintln!(
                "[WARN] Could not kill process {} (may have exited already).",
                pid
            );
        }
        Err(e) => {
            eprintln!("[WARN] Failed to kill process {}: {}", pid, e);
        }
    }

    fs::remove_file(&pid_file).ok();
    Ok(())
}

pub fn status() -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let profile_path = PathBuf::from(&home).join(".cache/muthr/opencode-profile");

    if !profile_path.exists() {
        println!("[STATUS] No active profile configured.");
        return Ok(());
    }

    let content = fs::read_to_string(&profile_path)?;
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

    let is_running = Command::new("pgrep")
        .arg("-x")
        .arg("llama-server")
        .output()?
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

    let is_tty = std::io::stdout().is_terminal();

    if !is_tty {
        println!("Available presets:");
        println!("===============================================================================");
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
        println!("===============================================================================");
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
        Some(idx) => {
            println!("{}", presets[idx].name);
        }
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

fn apply_vram_limits() {
    let mem_bytes = sysctl_memsize();
    let threshold: u64 = 32 * 1024 * 1024 * 1024;

    if mem_bytes >= threshold {
        let gb = mem_bytes / 1024 / 1024 / 1024;
        println!(
            "[INFO] High-memory host detected ({}GB). Adjusting wired Metal VRAM limits...",
            gb
        );
        let status = Command::new("sudo")
            .args(["sysctl", "iogpu.wired_limit_mb=43000"])
            .stdin(Stdio::inherit())
            .output();

        match status {
            Ok(out) if out.status.success() => {
                println!("[ OK ] VRAM limits adjusted.");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("[WARN] Failed to adjust VRAM limits: {}", stderr.trim());
            }
            Err(e) => {
                eprintln!("[WARN] Failed to execute sysctl: {}", e);
            }
        }
    } else {
        let gb = mem_bytes / 1024 / 1024 / 1024;
        println!(
            "[INFO] Standard host configuration detected ({}GB). Skipping VRAM limit override.",
            gb
        );
    }
}

fn sysctl_memsize() -> u64 {
    let output = Command::new("sysctl").arg("-n").arg("hw.memsize").output();

    match output {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        }
        _ => 0,
    }
}

fn persist_profile(
    _preset: &str,
    preset_path: &str,
    config_path: &Option<PathBuf>,
) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let cache_dir = PathBuf::from(&home).join(".cache/muthr");
    fs::create_dir_all(&cache_dir)?;
    let config_file = cache_dir.join("opencode-profile");

    let config_value = config_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "not set".to_string());

    let content = format!(
        "export LLAMA_ARG_MODELS_PRESET=\"{}\"\nexport OPENCODE_CONFIG=\"{}\"",
        preset_path, config_value
    );
    fs::write(&config_file, &content).map_err(|e| {
        color_eyre::eyre::eyre!(
            "Failed to persist profile to {}: {}",
            config_file.display(),
            e
        )
    })?;

    Ok(())
}
