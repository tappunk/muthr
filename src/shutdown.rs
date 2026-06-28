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

use crate::engine;
use crate::engine::EngineRuntime;

const DEFAULT_TIMEOUT_SECS: u64 = 30;

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
                        let id = item.get("id").and_then(|v| v.as_str()).or_else(|| {
                            item.get("configuration")
                                .and_then(|v| v.get("id"))
                                .and_then(|v| v.as_str())
                        })?;
                        if id.starts_with("muthr-")
                            && id != "muthr-services"
                            && id != "muthr-searxng"
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

async fn stop_runtime(runtime: EngineRuntime, verbose: bool) {
    if let Err(err) = engine::stop(runtime).await {
        eprintln!("warning: failed to stop {} runtime: {}", runtime, err);
    }

    if engine::is_running(runtime).await {
        eprintln!(
            "warning: {} runtime still running after stop request; retrying",
            runtime
        );
        if let Err(err) = engine::stop(runtime).await {
            eprintln!(
                "warning: second stop attempt failed for {}: {}",
                runtime, err
            );
        }
    }

    if verbose && !engine::is_running(runtime).await {
        eprintln!("info: {} runtime stopped", runtime);
    }
}

pub async fn run(verbose: bool, timeout_secs: Option<u64>, _yes: bool, dry_run: bool) {
    if dry_run {
        eprintln!("info: dry run, skipping shutdown actions");
        return;
    }
    let timeout = timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);

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
    stop_runtime(EngineRuntime::Mlxcel, verbose).await;

    if verbose {
        eprintln!("info: shutdown complete");
    }
}
