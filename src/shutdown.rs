use tokio::process::Command as AsyncCommand;

use crate::engine;
use crate::sandbox;

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

async fn stop_vm(name: String, timeout_secs: u64, verbose: bool) {
    if verbose {
        eprintln!("info: stopping vm {}", name);
    }

    match sandbox::stop_vm_with_timeout(&name, timeout_secs).await {
        Ok(true) => eprintln!("info: stopped {}", name),
        Ok(false) => eprintln!("info: already stopped {}", name),
        Err(e) => {
            eprintln!("warning: failed to stop vm {}", name);
            eprintln!("warning: {}", e);
        }
    }
}

pub async fn run(verbose: bool, timeout_secs: Option<u64>, _yes: bool, dry_run: bool) {
    if dry_run {
        eprintln!("info: dry run, skipping shutdown actions");
        return;
    }
    let default_timeout = DEFAULT_TIMEOUT_SECS;
    let timeout = timeout_secs.unwrap_or(default_timeout);

    if verbose {
        eprintln!("info: scanning vms");
    }

    let sandboxes = discover_sandbox_vms().await;

    for vm in sandboxes {
        stop_vm(vm.clone(), timeout, verbose).await;
    }

    stop_vm("muthr-services".to_string(), timeout, verbose).await;

    if verbose {
        eprintln!("info: stopping inference engine");
    }
    let _ = engine::stop().await;

    if verbose {
        eprintln!("info: shutdown complete");
    }
}
