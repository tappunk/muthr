use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::config;

pub async fn download(source: &str, file: Option<&str>) -> Result<(), color_eyre::Report> {
    let (repo, filename) = match (source, file) {
        (url, None) if url.starts_with("http") && url.contains("huggingface.co") => {
            parse_hf_url(url)?
        }
        (repo, Some(file)) => (repo.to_string(), file.to_string()),
        _ => {
            eprintln!("err: muthr download <hf-repo> <filename> | <hf-url>");
            return Ok(());
        }
    };

    if !filename.ends_with(".gguf") {
        eprintln!("err: expected .gguf file");
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
    let url = format!("https://huggingface.co/{}/resolve/main/{}", repo, filename);

    fs::create_dir_all(&model_subdir).await?;

    if target_path.exists() {
        eprintln!("warn: file exists, aborting to prevent overwrite");
        return Ok(());
    }

    println!("fetching {}", filename);

    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(token) = std::env::var("HF_TOKEN") {
        let auth_val = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))?;
        headers.insert(reqwest::header::AUTHORIZATION, auth_val);
    }

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    let mut response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "Download failed: {}",
            response.status()
        ));
    }

    let total_size = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
            .progress_chars("#>-"),
    );

    let tmp_file = format!("{}.tmp", target_path.display());
    let mut file = fs::File::create(&tmp_file).await?;

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message("Downloaded");
    fs::rename(&tmp_file, &target_path).await?;

    if let Ok(metadata) = fs::metadata(&target_path).await {
        println!("done {} ({} bytes)", target_path.display(), metadata.len());
    } else {
        println!("done");
    }

    Ok(())
}

fn parse_hf_url(url: &str) -> Result<(String, String), color_eyre::Report> {
    let tmp = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let tmp = tmp
        .strip_prefix("huggingface.co/")
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid HuggingFace URL format"))?;

    let mut parts = tmp.splitn(3, '/');
    let repo = parts
        .next()
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid HuggingFace URL"))?;
    let _rev_or_blob = parts.next();
    let filename = parts.next().ok_or_else(|| {
        color_eyre::eyre::eyre!("Invalid HuggingFace URL — expected repo/blob/revision/filename")
    })?;

    if filename.is_empty() {
        return Err(color_eyre::eyre::eyre!("Missing filename in URL"));
    }

    Ok((repo.to_string(), filename.to_string()))
}
