use std::fs;
use std::path::PathBuf;

pub fn download(source: &str, file: Option<&str>) -> Result<(), color_eyre::Report> {
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

    fs::create_dir_all(&model_subdir)?;

    if target_path.exists() {
        eprintln!(
            "[WARN] File exists at {:?}. Resuming download if incomplete...",
            target_path
        );
    } else {
        println!("[PROC] Fetching: {}", filename);
        println!("       From:    https://huggingface.co/{}", repo);
    }

    let tmp_file = format!("{}.tmp", target_path.display());
    let mut curl_opts: Vec<String> = vec![
        "-f".to_string(),
        "-L".to_string(),
        "-C".to_string(),
        "-".to_string(),
        "-o".to_string(),
        tmp_file.clone(),
    ];

    if let Ok(token) = std::env::var("HF_TOKEN") {
        curl_opts.push("-H".to_string());
        curl_opts.push(format!("Authorization: Bearer {}", token));
    }

    curl_opts.push(url);

    let mut child = std::process::Command::new("curl")
        .args(&curl_opts)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;

    let status = child.wait()?;

    if status.success() {
        let tmp_path = PathBuf::from(&tmp_file);
        if tmp_path.exists() {
            fs::rename(&tmp_path, &target_path)?;
        }
        println!("[ OK ] Download complete.");
        if let Ok(metadata) = fs::metadata(&target_path) {
            println!("       Size: {}", human_size(metadata.len()));
        }
    } else {
        eprintln!("[ERR ] Download failed, server returned an error, or process was interrupted.");
        let tmp_path = PathBuf::from(&tmp_file);
        if tmp_path.exists() {
            fs::remove_file(&tmp_path).ok();
        }
        return Err(color_eyre::eyre::eyre!("Download failed"));
    }

    Ok(())
}

fn parse_hf_url(url: &str) -> Result<(String, String), color_eyre::Report> {
    let tmp = url.trim_start_matches("https://");
    let tmp = tmp.trim_start_matches("http://");
    let tmp = tmp
        .strip_prefix("huggingface.co/")
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid HuggingFace URL format"))?;

    let repo_end = if tmp.contains("/blob/main/") {
        tmp.find("/blob/main/").unwrap()
    } else if tmp.contains("/raw/main/") {
        tmp.find("/raw/main/").unwrap()
    } else {
        return Err(color_eyre::eyre::eyre!("Invalid HuggingFace URL format"));
    };

    let repo = &tmp[..repo_end];
    let base = &tmp[repo_end + 11..]; // skip /blob/main/ or /raw/main/

    let filename = if !base.is_empty() {
        base.to_string()
    } else {
        url.rsplit('/').next().unwrap_or("").to_string()
    };

    Ok((repo.to_string(), filename))
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
