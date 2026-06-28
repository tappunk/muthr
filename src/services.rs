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

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SEARXNG_CONFIG_REV: &str = "v3";

fn discover_container_gateway() -> Option<String> {
    let output = Command::new("container")
        .args(["network", "list", "--format", "json"])
        .output()
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

fn resolve_host_gateway() -> Result<String, color_eyre::Report> {
    if let Ok(cfg) = crate::config::load()
        && let Some(configured) = cfg.container_host_gateway
    {
        let host = configured.trim().to_string();
        if host.is_empty() {
            return Err(color_eyre::eyre::eyre!(
                "container_host_gateway is empty in config"
            ));
        }
        return Ok(host);
    }
    if let Ok(env_host) = std::env::var("MUTHR_CONTAINER_HOST_GATEWAY")
        && !env_host.trim().is_empty()
    {
        return Ok(env_host.trim().to_string());
    }
    discover_container_gateway().ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "could not determine container host gateway; set MUTHR_CONTAINER_HOST_GATEWAY or container_host_gateway in config"
        )
    })
}

pub async fn run(action: crate::ServicesCommands) -> Result<(), color_eyre::Report> {
    match action {
        crate::ServicesCommands::Start { dry_run } => start(dry_run).await?,
        crate::ServicesCommands::Stop { dry_run } => stop(dry_run).await?,
        crate::ServicesCommands::Status { output } => status(output).await?,
        crate::ServicesCommands::Restart { dry_run } => restart(dry_run).await?,
        crate::ServicesCommands::Delete {
            force,
            yes,
            dry_run,
        } => delete(force || yes, dry_run).await?,
    }
    Ok(())
}

pub async fn start(dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services start");
        return Ok(());
    }

    let container_id = "muthr-services";
    let searxng_container_id = "muthr-searxng";
    let home = std::env::var("HOME")?;
    let searxng_settings_path = ensure_searxng_settings(&home)?;

    if is_container_exists(searxng_container_id)
        && !container_has_label(searxng_container_id, "muthr.config-rev", SEARXNG_CONFIG_REV)
    {
        eprintln!("info: recreating muthr-searxng container for updated config");
        let delete_status = Command::new("container")
            .args(["delete", "--force", searxng_container_id])
            .status()?;
        if !delete_status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to recreate muthr-searxng container"
            ));
        }
    }

    if !is_container_exists(searxng_container_id) {
        eprintln!("info: creating muthr-searxng container");
        let settings_mount = format!(
            "{}:/etc/searxng/settings.yml",
            searxng_settings_path.to_string_lossy()
        );
        let status = Command::new("container")
            .args([
                "create",
                "--name",
                searxng_container_id,
                "--detach",
                "--label",
                "muthr.config-rev=v3",
                "--publish",
                "18766:8080",
                "--volume",
                &settings_mount,
                "--env",
                "SEARXNG_SECRET=change-this-in-local-config",
                "docker.io/searxng/searxng:latest",
            ])
            .status()?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to create muthr-searxng container"
            ));
        }
    }

    if !is_container_running(searxng_container_id) {
        eprintln!("info: starting muthr-searxng container");
        let status = Command::new("container")
            .args(["start", searxng_container_id])
            .status()?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to start muthr-searxng container"
            ));
        }
    }

    if !is_container_exists(container_id) {
        eprintln!("info: creating muthr-services container");
        let status = Command::new("container")
            .args([
                "create",
                "--name",
                container_id,
                "--detach",
                "--publish",
                "127.0.0.1:18765:18765",
                "--workdir",
                "/tmp",
                "debian:13-slim",
                "sh",
                "-lc",
                "while true; do sleep 3600; done",
            ])
            .status()?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to create muthr-services container"
            ));
        }
    }

    if !is_container_running(container_id) {
        eprintln!("info: starting muthr-services container");
        let status = Command::new("container")
            .args(["start", container_id])
            .status()?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to start muthr-services container"
            ));
        }
    } else {
        eprintln!("info: muthr-services container already running");
        return Ok(());
    }

    ensure_services_runtime_baseline(container_id)?;

    if !is_container_provisioned(container_id) {
        let host_gateway = resolve_host_gateway()?;
        let searxng_url = format!("http://{}:18766", host_gateway);

        let mut cp_cmd = Command::new("container");
        cp_cmd.args([
            "copy",
            &format!(
                "{}/.config/muthr/sandbox.d/container/provision.d/muthr-services.sh",
                home
            ),
            &format!("{}:/tmp/muthr-services.sh", container_id),
        ]);
        if !cp_cmd.status()?.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to copy provision script into services container"
            ));
        }

        let mut cp_lib_cmd = Command::new("container");
        cp_lib_cmd.args([
            "copy",
            &format!("{}/.config/muthr/sandbox.d/container/provision.d/lib", home),
            &format!("{}:/tmp", container_id),
        ]);
        if !cp_lib_cmd.status()?.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to copy provision library into services container"
            ));
        }

        let mut provision_cmd = Command::new("container");
        provision_cmd
            .args(["exec", "--env"])
            .arg(format!("MUTHR_SEARXNG_URL={}", searxng_url))
            .args([container_id, "bash", "/tmp/muthr-services.sh", container_id])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if provision_cmd.status()?.success() {
            eprintln!("info: muthr-services container provisioned");
        } else {
            return Err(color_eyre::eyre::eyre!(
                "muthr-services container provisioning failed"
            ));
        }
    }

    eprintln!("info: searxng:  http://127.0.0.1:18766 (browser access)");
    eprintln!("info: mcp:      stdio bridge via muthr-services over exec");

    Ok(())
}

pub async fn stop(dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services stop");
        return Ok(());
    }

    let container_id = "muthr-services";
    let searxng_container_id = "muthr-searxng";
    if !is_container_exists(container_id) && !is_container_exists(searxng_container_id) {
        return Ok(());
    }

    if is_container_exists(container_id) {
        let output = Command::new("container")
            .args(["stop", container_id])
            .output()
            .ok();

        match output {
            Some(out) if out.status.success() => eprintln!("info: stopped {}", container_id),
            Some(_) | None => eprintln!("warning: failed to stop {}", container_id),
        }
    }

    if is_container_exists(searxng_container_id) {
        let output = Command::new("container")
            .args(["stop", searxng_container_id])
            .output()
            .ok();

        match output {
            Some(out) if out.status.success() => {
                eprintln!("info: stopped {}", searxng_container_id)
            }
            Some(_) | None => eprintln!("warning: failed to stop {}", searxng_container_id),
        }
    }

    Ok(())
}

pub async fn status(output: crate::OutputFormat) -> Result<(), color_eyre::Report> {
    let container_id = "muthr-services";
    let searxng_container_id = "muthr-searxng";

    if !is_container_exists(container_id) && !is_container_exists(searxng_container_id) {
        return Ok(());
    }

    let status = if is_container_running(container_id) {
        "running"
    } else {
        "stopped"
    }
    .to_string();

    match output {
        crate::OutputFormat::Text => {
            eprintln!("muthr-services container: {}", status.to_lowercase());
            if is_container_exists(searxng_container_id) {
                let searxng_status = if is_container_running(searxng_container_id) {
                    "running"
                } else {
                    "stopped"
                };
                eprintln!("muthr-searxng container: {}", searxng_status);
            }
        }
        crate::OutputFormat::Json => {
            let payload = serde_json::json!({
                "name": container_id,
                "status": status.to_lowercase(),
                "searxng": if is_container_exists(searxng_container_id) {
                    serde_json::Value::String(
                        if is_container_running(searxng_container_id) {
                            "running"
                        } else {
                            "stopped"
                        }
                        .to_string(),
                    )
                } else {
                    serde_json::Value::Null
                }
            });
            println!("{}", serde_json::to_string(&payload)?);
        }
        crate::OutputFormat::Ndjson => {
            let payload = serde_json::json!({
                "name": container_id,
                "status": status.to_lowercase(),
                "searxng": if is_container_exists(searxng_container_id) {
                    serde_json::Value::String(
                        if is_container_running(searxng_container_id) {
                            "running"
                        } else {
                            "stopped"
                        }
                        .to_string(),
                    )
                } else {
                    serde_json::Value::Null
                }
            });
            println!("{}", serde_json::to_string(&payload)?);
        }
    }

    Ok(())
}

pub async fn restart(dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services restart");
        return Ok(());
    }
    stop(false).await?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    start(false).await?;
    Ok(())
}

fn is_container_exists(container_id: &str) -> bool {
    let output = Command::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .ok()
        .filter(|o| o.status.success());

    match output {
        Some(out) => serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout)
            .ok()
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("id").and_then(|v| v.as_str()).or_else(|| {
                        item.get("configuration")
                            .and_then(|v| v.get("id"))
                            .and_then(|v| v.as_str())
                    }) == Some(container_id)
                })
            }),
        None => false,
    }
}

fn is_container_running(container_id: &str) -> bool {
    let output = Command::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .ok();

    match output {
        Some(out) if out.status.success() => {
            serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout)
                .ok()
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        let id = item.get("id").and_then(|v| v.as_str()).or_else(|| {
                            item.get("configuration")
                                .and_then(|v| v.get("id"))
                                .and_then(|v| v.as_str())
                        });
                        let state = item
                            .get("status")
                            .and_then(|v| v.get("state"))
                            .and_then(|v| v.as_str())
                            .or_else(|| item.get("state").and_then(|v| v.as_str()));
                        id == Some(container_id) && state == Some("running")
                    })
                })
        }
        Some(_) | None => false,
    }
}

fn container_has_label(container_id: &str, key: &str, expected: &str) -> bool {
    let output = Command::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .ok();

    match output {
        Some(out) if out.status.success() => {
            serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout)
                .ok()
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        let id = item.get("id").and_then(|v| v.as_str()).or_else(|| {
                            item.get("configuration")
                                .and_then(|v| v.get("id"))
                                .and_then(|v| v.as_str())
                        });
                        let label = item
                            .get("configuration")
                            .and_then(|v| v.get("labels"))
                            .and_then(|v| v.get(key))
                            .and_then(|v| v.as_str());
                        id == Some(container_id) && label == Some(expected)
                    })
                })
        }
        Some(_) | None => false,
    }
}

fn ensure_searxng_settings(home: &str) -> Result<PathBuf, color_eyre::Report> {
    let settings_dir = PathBuf::from(home).join(".cache/muthr/searxng");
    std::fs::create_dir_all(&settings_dir)?;
    let settings_path = settings_dir.join("settings.yml");

    let settings = "use_default_settings: true\nsearch:\n  formats:\n    - html\n    - json\nserver:\n  limiter: false\n";
    std::fs::write(&settings_path, settings)?;

    Ok(settings_path)
}

fn is_container_provisioned(container_id: &str) -> bool {
    let output = Command::new("container")
        .args(["exec", container_id])
        .arg("bash")
        .arg("-c")
        .arg("test -f $HOME/mcp-stdio.sh && test -f $HOME/.local/lib/node_modules/mcp-searxng/dist/cli.js")
        .output()
        .ok();

    match output {
        Some(out) => out.status.success(),
        None => false,
    }
}

fn ensure_services_runtime_baseline(container_id: &str) -> Result<(), color_eyre::Report> {
    let marker = "/var/lib/muthr/services-baseline-v1";
    let marker_check = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            &format!("test -f {}", marker),
        ])
        .status()?;
    if marker_check.success() {
        return Ok(());
    }

    eprintln!("info: installing muthr-services runtime dependencies");
    let deps_status = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq bash ca-certificates curl nodejs npm",
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !deps_status.success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to install muthr-services container dependencies"
        ));
    }

    let marker_status = Command::new("container")
        .args([
            "exec",
            container_id,
            "sh",
            "-lc",
            &format!("mkdir -p /var/lib/muthr && touch {}", marker),
        ])
        .status()?;
    if !marker_status.success() {
        eprintln!("warning: failed to persist services baseline marker");
    }

    Ok(())
}

pub async fn delete(force: bool, dry_run: bool) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping services delete");
        return Ok(());
    }
    let container_id = "muthr-services";
    let searxng_container_id = "muthr-searxng";

    if !is_container_exists(container_id) && !is_container_exists(searxng_container_id) {
        return Ok(());
    }

    if !force && !std::io::stdout().is_terminal() {
        eprintln!("error: terminal required for deletion, use --yes");
        std::process::exit(77);
    }

    if is_container_exists(container_id) {
        eprintln!("info: deleting muthr-services container");

        let delete_status = Command::new("container")
            .args(["delete", "--force", container_id])
            .output()?;

        if !delete_status.status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to delete muthr-services container"
            ));
        }

        eprintln!("info: deleted {}", container_id);
    }

    if is_container_exists(searxng_container_id) {
        eprintln!("info: deleting muthr-searxng container");
        let delete_status = Command::new("container")
            .args(["delete", "--force", searxng_container_id])
            .output()?;
        if !delete_status.status.success() {
            return Err(color_eyre::eyre::eyre!(
                "failed to delete muthr-searxng container"
            ));
        }
        eprintln!("info: deleted {}", searxng_container_id);
    }

    Ok(())
}
