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

use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Command;

fn container_id_from_item(item: &serde_json::Value) -> Option<String> {
    item.get("id")
        .and_then(|v| v.as_str())
        .or_else(|| {
            item.get("configuration")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
        })
        .map(|v| v.to_string())
}

fn container_has_label(item: &serde_json::Value, key: &str, expected: &str) -> bool {
    item.get("configuration")
        .and_then(|v| v.get("labels"))
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == expected)
}

async fn list_containers() -> Result<Vec<serde_json::Value>, color_eyre::Report> {
    let output = Command::new("container")
        .args(["list", "--all", "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Ok(vec![]);
    }
    let items =
        serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout).unwrap_or_else(|_| vec![]);
    Ok(items)
}

fn check_container_cli() -> Result<(), color_eyre::Report> {
    let primary = std::process::Command::new("container")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let ok = matches!(primary, Ok(s) if s.success())
        || matches!(
            std::process::Command::new("container")
                .args(["system", "version"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
            Ok(s) if s.success()
        );

    if ok {
        eprintln!("ok: container CLI available");
        Ok(())
    } else {
        eprintln!("error: container CLI not found or unavailable");
        eprintln!("hint: install Apple container CLI and run 'container system start'");
        Err(color_eyre::eyre::eyre!("container CLI missing"))
    }
}

fn check_mlxcel() -> Result<(), color_eyre::Report> {
    let status = std::process::Command::new("mlxcel-server")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {
            eprintln!("ok: mlxcel-server available");
            Ok(())
        }
        _ => {
            eprintln!("error: mlxcel-server not found");
            eprintln!("hint: brew install lablup/tap/mlxcel");
            Err(color_eyre::eyre::eyre!("mlxcel-server missing"))
        }
    }
}

fn check_config() -> Result<(), color_eyre::Report> {
    let cfg = crate::config::load()?;
    cfg.print_resolved();
    eprintln!("ok: config loaded");
    Ok(())
}

async fn check_engine() -> Result<(), color_eyre::Report> {
    if crate::engine::is_running().await {
        eprintln!("ok: inference engine running");
    } else {
        eprintln!("info: inference engine not running");
    }
    Ok(())
}

async fn check_services() -> Result<(), color_eyre::Report> {
    let items = list_containers().await?;
    let mut services_exists = false;
    let mut searxng_exists = false;

    for item in items {
        if let Some(id) = container_id_from_item(&item) {
            if id == "muthr-services" {
                services_exists = true;
            }
            if id == "muthr-searxng" {
                searxng_exists = true;
            }
        }
    }

    if services_exists {
        eprintln!("ok: muthr-services container exists");
    } else {
        eprintln!("info: muthr-services container not created yet");
    }
    if searxng_exists {
        eprintln!("ok: muthr-searxng container exists");
    } else {
        eprintln!("info: muthr-searxng container not created yet");
    }

    Ok(())
}

fn check_locks_dir() -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let lock_dir = PathBuf::from(home).join(".cache/muthr");
    if lock_dir.exists() {
        eprintln!("ok: runtime cache directory exists");
    } else {
        eprintln!("info: runtime cache directory not present yet");
    }
    Ok(())
}

async fn check_managed_containers() -> Result<(), color_eyre::Report> {
    let items = list_containers().await?;
    let count = items
        .iter()
        .filter(|item| {
            container_id_from_item(item)
                .is_some_and(|id| id.starts_with("muthr-") || id == "muthr-services")
                && container_has_label(item, "muthr.managed", "true")
        })
        .count();

    eprintln!("info: managed containers detected: {}", count);
    Ok(())
}

pub async fn run() -> Result<(), color_eyre::Report> {
    eprintln!("muthr doctor");

    check_container_cli()?;
    check_mlxcel()?;
    check_config()?;
    check_engine().await?;
    check_services().await?;
    check_locks_dir()?;
    check_managed_containers().await?;

    eprintln!("ok: diagnostics completed");
    Ok(())
}
