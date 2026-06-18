use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub async fn download(source: &str, file: Option<&str>) -> Result<(), color_eyre::Report> {
    let (repo, filename) = match (source, file) {
        (url, None) if url.starts_with("http") && url.contains("huggingface.co") => {
            parse_hf_url(url)?
        }
        (repo, Some(file)) => (repo.to_string(), file.to_string()),
        _ => {
            eprintln!("[ERR ] Usage: muthr download <hf-repo> <filename> | <hf-url>");
            eprintln!("  Example: muthr download unsloth/Qwen3.6-35B-A3B-GGUF Qwen3.6-35B-A3B-UD-Q4_K_M.gguf");
            return Ok(());
        }
    };

    if !filename.ends_with(".gguf") {
        eprintln!("[ERR ] Expected a .gguf file, got '{}'", filename);
        return Ok(());
    }

    let home = std::env::var("HOME")?;
    let model_dir =
        std::env::var("LLAMA_MODEL_DIR").unwrap_or_else(|_| format!("{}/opt/models", home));

    let model_subdir = PathBuf::from(&model_dir).join(&repo);
    let target_path = model_subdir.join(&filename);
    let url = format!("https://huggingface.co/{}/resolve/main/{}", repo, filename);

    fs::create_dir_all(&model_subdir).await?;

    if target_path.exists() {
        eprintln!(
            "[WARN] File exists at {:?}. Aborting to prevent overwrite.",
            target_path
        );
        return Ok(());
    }

    println!("[PROC] Fetching: {}", filename);
    println!("       From:    https://huggingface.co/{}", repo);

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

    println!("[ OK ] Download complete.");
    if let Ok(metadata) = fs::metadata(&target_path).await {
        println!("       Size: {}", human_size(metadata.len()));
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

    let separators = ["/blob/main/", "/raw/main/"];
    let mut split_res = None;
    for sep in separators {
        if let Some(idx) = tmp.find(sep) {
            let repo = tmp[..idx].to_string();
            let filename = tmp[idx + sep.len()..].to_string();
            split_res = Some((repo, filename));
            break;
        }
    }

    let (repo, filename) =
        split_res.ok_or_else(|| color_eyre::eyre::eyre!("Invalid HuggingFace URL"))?;

    if filename.is_empty() {
        return Err(color_eyre::eyre::eyre!("Missing filename in URL"));
    }

    Ok((repo, filename))
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    format!("{:.1} {}", size, UNITS[unit_idx])
}
