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

use tokio::process::Command as AsyncCommand;

async fn discover_sandbox_containers() -> Vec<String> {
    let output = AsyncCommand::new("container")
        .args(["list", "--all", "--format", "json"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success());

    match output {
        Some(out) => serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout)
            .ok()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item
                            .get("id")
                            .or_else(|| item.get("ID"))
                            .or_else(|| item.get("Id"))
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                item.get("configuration")
                                    .or_else(|| item.get("Configuration"))
                                    .or_else(|| item.get("config"))
                                    .or_else(|| item.get("Config"))
                                    .and_then(|v| {
                                        v.get("id").or_else(|| v.get("ID")).or_else(|| v.get("Id"))
                                    })
                                    .and_then(|v| v.as_str())
                            })?;
                        let labels = item
                            .get("configuration")
                            .or_else(|| item.get("Configuration"))
                            .or_else(|| item.get("config"))
                            .or_else(|| item.get("Config"))
                            .and_then(|v| v.get("labels").or_else(|| v.get("Labels")));
                        let managed = labels
                            .and_then(|v| v.get("muthr.managed"))
                            .and_then(|v| v.as_str())
                            .is_some_and(|v| v == "true");
                        let owner_project = labels
                            .and_then(|v| v.get("muthr.owner"))
                            .and_then(|v| v.as_str())
                            .is_some_and(|v| v == "project");
                        if id.starts_with("muthr-")
                            && id != "muthr-services"
                            && id != "muthr-searxng"
                            && managed
                            && owner_project
                        {
                            Some(id.to_string())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

async fn stop_container(name: String, timeout_secs: u64, verbose: bool) {
    if verbose {
        eprintln!("info: stopping container {}", name);
    }

    let status = AsyncCommand::new("container")
        .args(["stop", "--time", &timeout_secs.to_string(), &name])
        .output()
        .await;

    match status {
        Ok(out) if out.status.success() => eprintln!("info: stopped {}", name),
        Ok(_) | Err(_) => eprintln!("warning: failed to stop {}", name),
    }
}

async fn stop_engine(verbose: bool) {
    let mut had_any = false;
    let default_runtime = crate::config::load()
        .ok()
        .and_then(|cfg| cfg.default_engine_runtime)
        .unwrap_or_else(|| "mlxcel".to_string());

    if crate::engine::is_running().await {
        had_any = true;
        if let Err(err) = crate::engine::stop_all().await {
            eprintln!("warning: failed to stop inference engine: {}", err);
        }
    }

    if had_any {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if crate::engine::is_running().await {
            eprintln!("warning: inference engine still running after stop request; retrying");
            if let Err(err) = crate::engine::stop(&default_runtime).await {
                eprintln!("warning: second stop attempt failed: {}", err);
            }
        }
    }

    if verbose && had_any && !crate::engine::is_running().await {
        eprintln!("info: inference engine stopped");
    }
}

pub async fn run(
    verbose: bool,
    timeout_secs: Option<u64>,
    _yes: bool,
    dry_run: bool,
) -> Result<(), color_eyre::Report> {
    if dry_run {
        eprintln!("info: dry run, skipping shutdown actions");
        return Ok(());
    }
    let _lock =
        crate::lifecycle::acquire("container-lifecycle", std::time::Duration::from_secs(20))
            .await?;
    let timeout = timeout_secs.unwrap_or(30);

    if verbose {
        eprintln!("info: scanning containers");
    }

    let sandboxes = discover_sandbox_containers().await;

    for container in sandboxes {
        stop_container(container.clone(), timeout, verbose).await;
    }

    stop_container("muthr-services".to_string(), timeout, verbose).await;
    stop_container("muthr-searxng".to_string(), timeout, verbose).await;

    if verbose {
        eprintln!("info: stopping inference engine");
    }
    stop_engine(verbose).await;

    if verbose {
        eprintln!("info: shutdown complete");
    }

    Ok(())
}
