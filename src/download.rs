use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::config;

pub async fn download(
    source: &str,
    file: Option<&str>,
    output: crate::OutputFormat,
) -> Result<(), color_eyre::Report> {
    let (repo, revision, filename) = match (source, file) {
        (url, None) if url.starts_with("http") && url.contains("huggingface.co") => {
            parse_hf_url(url)?
        }
        (repo, Some(file)) => (repo.to_string(), "main".to_string(), file.to_string()),
        _ => {
            eprintln!("error: muthr download <hf-repo> <filename> | <hf-url>");
            return Ok(());
        }
    };

    if !filename.ends_with(".gguf") {
        eprintln!("error: expected .gguf file");
        return Ok(());
    }

    let cfg = config::load()?;
    let model_dir = cfg.model_dir.unwrap_or_else(|| {
        if let Ok(home) = std::env::var("HOME") {
            format!("{}/opt/models", home)
        } else {
            "~/opt/models".to_string()
        }
    });

    let model_dir = model_dir
        .strip_prefix("~/")
        .map(|p| format!("{}/{}", std::env::var("HOME").unwrap_or_default(), p))
        .unwrap_or(model_dir);

    let model_subdir = PathBuf::from(&model_dir).join(&repo);
    let target_path = model_subdir.join(&filename);
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        repo, revision, filename
    );

    fs::create_dir_all(&model_subdir).await?;

    if target_path.exists() {
        eprintln!("warning: file exists, aborting to prevent overwrite");
        return Ok(());
    }

    if output == crate::OutputFormat::Text {
        eprintln!("info: fetching {}", filename);
    }

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

    let mut response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "download failed: {}",
            response.status()
        ));
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

    let tmp_file = format!("{}.tmp", target_path.display());
    let mut file = fs::File::create(&tmp_file).await?;

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        if show_progress {
            pb.inc(chunk.len() as u64);
        }
    }

    if show_progress {
        pb.finish_with_message("downloaded");
    }
    fs::rename(&tmp_file, &target_path).await?;

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

    Ok(())
}

fn parse_hf_url(url: &str) -> Result<(String, String, String), color_eyre::Report> {
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
    let marker_idx = parts
        .iter()
        .position(|p| *p == "resolve" || *p == "blob")
        .ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "invalid huggingface url, expected /<repo>/resolve/<revision>/<filename>"
            )
        })?;

    if marker_idx < 2 {
        return Err(color_eyre::eyre::eyre!(
            "invalid huggingface repository path"
        ));
    }

    let repo = parts[..marker_idx].join("/");
    let revision = parts
        .get(marker_idx + 1)
        .ok_or_else(|| color_eyre::eyre::eyre!("missing revision in huggingface url"))?;
    let filename = parts
        .get(marker_idx + 2..)
        .map(|slice| slice.join("/"))
        .ok_or_else(|| color_eyre::eyre::eyre!("missing filename in huggingface url"))?;

    if filename.is_empty() {
        return Err(color_eyre::eyre::eyre!("missing filename in url"));
    }

    Ok((repo, revision.to_string(), filename))
}
