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

use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::config;
use crate::preset;

pub async fn download(
    source: &str,
    file: Option<&str>,
    output: crate::OutputFormat,
) -> Result<(), color_eyre::Report> {
    let request = resolve_request(source, file)?;

    let cfg = config::load()?;
    let raw_model_dir = cfg.model_dir.unwrap_or_else(|| {
        if let Ok(home) = std::env::var("HOME") {
            format!("{}/opt/models", home)
        } else {
            "~/opt/models".to_string()
        }
    });

    let model_dir = preset::expand_home(std::path::Path::new(&raw_model_dir));
    let model_subdir = model_dir.join(request.repo());

    fs::create_dir_all(&model_subdir).await?;

    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(token) = std::env::var("HF_TOKEN") {
        match reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token)) {
            Ok(auth_val) => {
                headers.insert(reqwest::header::AUTHORIZATION, auth_val);
            }
            Err(_) => {
                eprintln!("warning: ignoring invalid HF_TOKEN value");
            }
        }
    }

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    match request {
        DownloadRequest::Single {
            repo,
            revision,
            filename,
        } => {
            let target_path = model_subdir.join(&filename);
            if target_path.exists() {
                eprintln!("warning: file exists, aborting to prevent overwrite");
                return Ok(());
            }

            if output == crate::OutputFormat::Text {
                eprintln!("info: fetching {}", filename);
            }

            let url = build_resolve_url(&repo, &revision, &filename);
            download_file(&client, &url, &target_path, output).await?;

            let bytes = fs::metadata(&target_path).await.ok().map(|m| m.len());
            match output {
                crate::OutputFormat::Text => {
                    if let Some(size) = bytes {
                        eprintln!("info: done {} ({} bytes)", target_path.display(), size);
                    } else {
                        eprintln!("info: done");
                    }
                }
                crate::OutputFormat::Json | crate::OutputFormat::Ndjson => {
                    let payload = serde_json::json!({
                        "status": "done",
                        "path": target_path.to_string_lossy(),
                        "bytes": bytes,
                    });
                    println!("{}", serde_json::to_string(&payload)?);
                }
            }
        }
        DownloadRequest::Bundle { repo, revision } => {
            if output == crate::OutputFormat::Text {
                eprintln!("info: listing files for {repo}@{revision}");
            }

            let files = list_repo_files(&client, &repo, &revision).await?;
            if files.is_empty() {
                return Err(color_eyre::eyre::eyre!(
                    "no files found for repository {} at revision {}",
                    repo,
                    revision
                ));
            }

            let existing: Vec<std::path::PathBuf> = files
                .iter()
                .map(|f| model_subdir.join(f))
                .filter(|p| p.exists())
                .collect();
            if !existing.is_empty() {
                eprintln!(
                    "warning: {} files already exist, aborting to prevent overwrite",
                    existing.len()
                );
                return Ok(());
            }

            let mut downloaded = Vec::with_capacity(files.len());
            let mut downloaded_rel_paths = Vec::with_capacity(files.len());
            for file_path in files {
                if output == crate::OutputFormat::Text {
                    eprintln!("info: fetching {}", file_path);
                }
                let url = build_resolve_url(&repo, &revision, &file_path);
                let target_path = model_subdir.join(&file_path);
                download_file(&client, &url, &target_path, output).await?;
                downloaded_rel_paths.push(file_path.clone());
                downloaded.push(target_path);
            }

            if looks_like_mlx_bundle(&downloaded_rel_paths) {
                validate_mlx_bundle_dir(&model_subdir)?;
                if output == crate::OutputFormat::Text {
                    eprintln!("info: validated mlx bundle layout");
                }
            }

            match output {
                crate::OutputFormat::Text => {
                    eprintln!(
                        "info: done downloaded {} files into {}",
                        downloaded.len(),
                        model_subdir.display()
                    );
                }
                crate::OutputFormat::Json | crate::OutputFormat::Ndjson => {
                    let payload = serde_json::json!({
                        "status": "done",
                        "repo": repo,
                        "revision": revision,
                        "downloaded": downloaded.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
                        "count": downloaded.len(),
                    });
                    println!("{}", serde_json::to_string(&payload)?);
                }
            }
        }
    }

    Ok(())
}

fn build_resolve_url(repo: &str, revision: &str, filename: &str) -> String {
    format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        repo, revision, filename
    )
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    target_path: &std::path::Path,
    output: crate::OutputFormat,
) -> Result<(), color_eyre::Report> {
    let mut response = client.get(url).send().await?;

    if !response.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "download failed for {}: {}",
            url,
            response.status()
        ));
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let total_size = response.content_length().unwrap_or(0);
    let show_progress = crate::ui::is_human_output(output);
    let pb = ProgressBar::new(total_size);
    if show_progress {
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
                .progress_chars("#>-"),
        );
    }

    let parent = target_path.parent().ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "invalid target path without parent: {}",
            target_path.display()
        )
    })?;
    let tmp_host_file = NamedTempFile::new_in(parent)?;
    let mut file = fs::File::from_std(tmp_host_file.reopen()?);

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        if show_progress {
            pb.inc(chunk.len() as u64);
        }
    }

    if show_progress {
        pb.finish_with_message("downloaded");
    }
    file.flush().await?;
    drop(file);
    tmp_host_file
        .into_temp_path()
        .persist(target_path)
        .map_err(|e| e.error)?;

    Ok(())
}

async fn list_repo_files(
    client: &reqwest::Client,
    repo: &str,
    revision: &str,
) -> Result<Vec<String>, color_eyre::Report> {
    let url = format!(
        "https://huggingface.co/api/models/{}/tree/{}?recursive=1",
        repo, revision
    );
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "failed to list repository files: {}",
            response.status()
        ));
    }

    let entries: Vec<HfTreeEntry> = response.json().await?;
    let files = entries
        .into_iter()
        .filter(|entry| entry.entry_type == "file")
        .map(|entry| entry.path)
        .collect();

    Ok(files)
}

#[derive(Debug, Deserialize)]
struct HfTreeEntry {
    #[serde(rename = "type")]
    entry_type: String,
    path: String,
}

enum DownloadRequest {
    Single {
        repo: String,
        revision: String,
        filename: String,
    },
    Bundle {
        repo: String,
        revision: String,
    },
}

impl DownloadRequest {
    fn repo(&self) -> &str {
        match self {
            DownloadRequest::Single { repo, .. } => repo,
            DownloadRequest::Bundle { repo, .. } => repo,
        }
    }
}

fn resolve_request(
    source: &str,
    file: Option<&str>,
) -> Result<DownloadRequest, color_eyre::Report> {
    match (source, file) {
        (src, Some(filename)) => {
            if src.starts_with("http") {
                let parsed = parse_hf_source(src)?;
                let (repo, revision) = match parsed {
                    HfSource::File { repo, revision, .. } => (repo, revision),
                    HfSource::Tree { repo, revision } => (repo, revision),
                    HfSource::Repo { repo } => (repo, "main".to_string()),
                };
                Ok(DownloadRequest::Single {
                    repo,
                    revision,
                    filename: filename.to_string(),
                })
            } else {
                Ok(DownloadRequest::Single {
                    repo: src.to_string(),
                    revision: "main".to_string(),
                    filename: filename.to_string(),
                })
            }
        }
        (src, None) => {
            if src.starts_with("http") {
                match parse_hf_source(src)? {
                    HfSource::File {
                        repo,
                        revision,
                        filename,
                    } => Ok(DownloadRequest::Single {
                        repo,
                        revision,
                        filename,
                    }),
                    HfSource::Tree { repo, revision } => {
                        Ok(DownloadRequest::Bundle { repo, revision })
                    }
                    HfSource::Repo { repo } => Ok(DownloadRequest::Bundle {
                        repo,
                        revision: "main".to_string(),
                    }),
                }
            } else {
                Ok(DownloadRequest::Bundle {
                    repo: src.to_string(),
                    revision: "main".to_string(),
                })
            }
        }
    }
}

enum HfSource {
    File {
        repo: String,
        revision: String,
        filename: String,
    },
    Tree {
        repo: String,
        revision: String,
    },
    Repo {
        repo: String,
    },
}

fn parse_hf_source(url: &str) -> Result<HfSource, color_eyre::Report> {
    let tmp = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let tmp = tmp
        .split(['?', '#'])
        .next()
        .ok_or_else(|| color_eyre::eyre::eyre!("invalid huggingface url"))?;
    let tmp = tmp
        .strip_prefix("huggingface.co/")
        .ok_or_else(|| color_eyre::eyre::eyre!("invalid huggingface url format"))?;

    let parts: Vec<&str> = tmp.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return Err(color_eyre::eyre::eyre!(
            "invalid huggingface repository path"
        ));
    }

    let marker_idx = parts
        .iter()
        .position(|p| *p == "resolve" || *p == "blob" || *p == "tree");

    match marker_idx {
        Some(idx) if parts[idx] == "tree" => {
            if idx < 2 {
                return Err(color_eyre::eyre::eyre!(
                    "invalid huggingface repository path"
                ));
            }
            let repo = parts[..idx].join("/");
            let revision = parts
                .get(idx + 1)
                .map(|s| (*s).to_string())
                .unwrap_or_else(|| "main".to_string());
            Ok(HfSource::Tree { repo, revision })
        }
        Some(idx) if parts[idx] == "resolve" || parts[idx] == "blob" => {
            if idx < 2 {
                return Err(color_eyre::eyre::eyre!(
                    "invalid huggingface repository path"
                ));
            }
            let repo = parts[..idx].join("/");
            let revision = parts
                .get(idx + 1)
                .ok_or_else(|| color_eyre::eyre::eyre!("missing revision in huggingface url"))?;
            let filename = parts
                .get(idx + 2..)
                .map(|slice| slice.join("/"))
                .ok_or_else(|| color_eyre::eyre::eyre!("missing filename in huggingface url"))?;
            if filename.is_empty() {
                return Err(color_eyre::eyre::eyre!("missing filename in url"));
            }
            Ok(HfSource::File {
                repo,
                revision: (*revision).to_string(),
                filename,
            })
        }
        Some(_) => Err(color_eyre::eyre::eyre!(
            "unsupported huggingface url format"
        )),
        None => {
            let repo = format!("{}/{}", parts[0], parts[1]);
            Ok(HfSource::Repo { repo })
        }
    }
}

fn looks_like_mlx_bundle(files: &[String]) -> bool {
    files.iter().any(|f| f.ends_with(".safetensors"))
        || files.iter().any(|f| f == "model.safetensors.index.json")
}

fn validate_mlx_bundle_dir(dir: &std::path::Path) -> Result<(), color_eyre::Report> {
    let required_files = [
        "config.json",
        "tokenizer_config.json",
        "model.safetensors.index.json",
    ];

    let mut missing = Vec::new();
    for name in required_files {
        if !dir.join(name).exists() {
            missing.push(name.to_string());
        }
    }

    let has_tokenizer = dir.join("tokenizer.json").exists()
        || dir.join("tokenizer.model").exists()
        || dir.join("vocab.json").exists();
    if !has_tokenizer {
        missing.push("tokenizer.json|tokenizer.model|vocab.json".to_string());
    }

    let index_path = dir.join("model.safetensors.index.json");
    if index_path.exists() {
        let raw = std::fs::read_to_string(&index_path)?;
        let json: serde_json::Value = serde_json::from_str(&raw)?;

        if let Some(weight_map) = json.get("weight_map") {
            let weight_map = weight_map.as_object().ok_or_else(|| {
                color_eyre::eyre::eyre!("invalid mlx index: 'weight_map' must be an object")
            })?;

            let mut required_shards = std::collections::HashSet::new();
            for shard in weight_map.values() {
                let shard_name = shard.as_str().ok_or_else(|| {
                    color_eyre::eyre::eyre!("invalid mlx index: shard filename must be a string")
                })?;
                required_shards.insert(shard_name.to_string());
            }

            for shard in required_shards {
                if !dir.join(&shard).exists() {
                    missing.push(shard);
                }
            }
        }
    }

    let has_weight_shard = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("model-") && name.ends_with(".safetensors")
        });
    if !has_weight_shard {
        missing.push("model-*.safetensors".to_string());
    }

    if !missing.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "mlx bundle validation failed, missing required files: {}",
            missing.join(", ")
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DownloadRequest, HfSource, looks_like_mlx_bundle, parse_hf_source, resolve_request,
        validate_mlx_bundle_dir,
    };

    #[test]
    fn parse_tree_url() {
        let source =
            parse_hf_source("https://huggingface.co/mlx-community/Qwen3.6-35B-A3B-4bit/tree/main")
                .expect("tree url should parse");

        match source {
            HfSource::Tree { repo, revision } => {
                assert_eq!(repo, "mlx-community/Qwen3.6-35B-A3B-4bit");
                assert_eq!(revision, "main");
            }
            _ => panic!("expected tree source"),
        }
    }

    #[test]
    fn parse_resolve_url() {
        let source = parse_hf_source(
            "https://huggingface.co/mlx-community/Qwen3.6-35B-A3B-4bit/resolve/main/config.json",
        )
        .expect("resolve url should parse");

        match source {
            HfSource::File {
                repo,
                revision,
                filename,
            } => {
                assert_eq!(repo, "mlx-community/Qwen3.6-35B-A3B-4bit");
                assert_eq!(revision, "main");
                assert_eq!(filename, "config.json");
            }
            _ => panic!("expected file source"),
        }
    }

    #[test]
    fn resolve_repo_without_filename_as_bundle() {
        let request = resolve_request("mlx-community/Qwen3.6-35B-A3B-4bit", None)
            .expect("repo source should resolve");

        match request {
            DownloadRequest::Bundle { repo, revision } => {
                assert_eq!(repo, "mlx-community/Qwen3.6-35B-A3B-4bit");
                assert_eq!(revision, "main");
            }
            _ => panic!("expected bundle request"),
        }
    }

    #[test]
    fn mlx_bundle_detection() {
        assert!(looks_like_mlx_bundle(&[
            "config.json".to_string(),
            "model.safetensors.index.json".to_string(),
        ]));
        assert!(!looks_like_mlx_bundle(&[
            "README.md".to_string(),
            "weights.bin".to_string(),
        ]));
    }

    #[test]
    fn validate_mlx_bundle_dir_requires_expected_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("config.json"), "{}").expect("write config");
        std::fs::write(temp.path().join("tokenizer_config.json"), "{}").expect("write tok cfg");
        std::fs::write(temp.path().join("tokenizer.json"), "{}").expect("write tokenizer");
        std::fs::write(temp.path().join("model.safetensors.index.json"), "{}")
            .expect("write index");
        std::fs::write(temp.path().join("model-00001-of-00001.safetensors"), "")
            .expect("write shard");

        validate_mlx_bundle_dir(temp.path()).expect("should validate mlx bundle");
    }

    #[test]
    fn validate_mlx_bundle_dir_fails_when_missing_weights() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("config.json"), "{}").expect("write config");
        std::fs::write(temp.path().join("tokenizer_config.json"), "{}").expect("write tok cfg");
        std::fs::write(temp.path().join("tokenizer.json"), "{}").expect("write tokenizer");
        std::fs::write(temp.path().join("model.safetensors.index.json"), "{}")
            .expect("write index");

        let err = validate_mlx_bundle_dir(temp.path()).expect_err("should fail validation");
        assert!(err.to_string().contains("model-*.safetensors"));
    }

    #[test]
    fn validate_mlx_bundle_dir_fails_when_index_shards_missing() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("config.json"), "{}").expect("write config");
        std::fs::write(temp.path().join("tokenizer_config.json"), "{}").expect("write tok cfg");
        std::fs::write(temp.path().join("tokenizer.json"), "{}").expect("write tokenizer");
        std::fs::write(
            temp.path().join("model.safetensors.index.json"),
            r#"{"weight_map":{"a":"model-00001-of-00002.safetensors","b":"model-00002-of-00002.safetensors"}}"#,
        )
        .expect("write index");
        std::fs::write(temp.path().join("model-00001-of-00002.safetensors"), "")
            .expect("write shard");

        let err = validate_mlx_bundle_dir(temp.path()).expect_err("should fail validation");
        assert!(err.to_string().contains("model-00002-of-00002.safetensors"));
    }
}
