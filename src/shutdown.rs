use tokio::process::Command as AsyncCommand;

use crate::engine;

const DEFAULT_TIMEOUT_SECS: u64 = 30;

async fn discover_sandbox_vms() -> Vec<String> {
    let output = AsyncCommand::new("limactl")
        .args(["ls", "-q"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success());

    match output {
        Some(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|v| v.starts_with("muthr-"))
            .map(|v| v.to_string())
            .collect(),
        None => Vec::new(),
    }
}

async fn is_vm_running(vm_name: &str) -> bool {
    let output = AsyncCommand::new("limactl")
        .args(["ls", "-f", "{{.Status}}", vm_name])
        .output()
        .await
        .ok();

    match output {
        Some(out) if out.status.success() => {
            let status = String::from_utf8_lossy(&out.stdout);
            status.contains("Running")
        }
        _ => false,
    }
}

async fn stop_vm(name: String, timeout_secs: u64, verbose: bool) {
    if verbose {
        println!("[PROC] Stopping VM '{}'...", name);
    }

    if !is_vm_running(&name).await {
        if verbose {
            println!("[OK] VM '{}' is not running.", name);
        }
        return;
    }

    let status = AsyncCommand::new("limactl")
        .arg("stop")
        .arg(&name)
        .status()
        .await;

    if !matches!(status, Ok(s) if s.success()) {
        eprintln!("[WARN] Failed to stop VM '{}' via limactl.", name);
    }

    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < timeout_secs {
        if !is_vm_running(&name).await {
            println!("[OK] VM '{}' stopped gracefully.", name);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    eprintln!(
        "[WARN] VM '{}' timed out after {}s. Escalating to force termination...",
        name, timeout_secs
    );
    let _ = AsyncCommand::new("limactl")
        .args(["stop", "--force", &name])
        .status()
        .await;

    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
}

pub async fn run(verbose: bool, timeout_secs: Option<u64>) {
    let default_timeout = DEFAULT_TIMEOUT_SECS;
    let timeout = timeout_secs.unwrap_or(default_timeout);

    if verbose {
        println!("[PROC] Scanning runtime hypervisor layers...");
    }

    let sandboxes = discover_sandbox_vms().await;

    for vm in sandboxes {
        stop_vm(vm.clone(), timeout, verbose).await;
    }

    stop_vm("muthr-services".to_string(), timeout, verbose).await;

    if verbose {
        println!("[PROC] Tearing down local hardware inference loops...");
    }
    let _ = engine::stop().await;

    if verbose {
        println!("[OK] Global context engine shutdown complete.");
    }
}
