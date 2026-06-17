use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Slot {
    pub index: u8,
    pub name: String,
    pub model_path: Option<PathBuf>,
    pub ctx_size: Option<u32>,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
    pub temp: Option<f32>,
    pub top_p: Option<f32>,
    pub min_p: Option<f32>,
    pub top_k: Option<u32>,
    pub repeat_penalty: Option<f32>,
    pub repeat_last_n: Option<u32>,
    pub load_on_startup: bool,
    pub jinja: Option<bool>,
    pub parallel: Option<u32>,
    pub image_min_tokens: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GlobalSettings {
    pub n_gpu_layers: i32,
    pub flash_attn: bool,
    pub mlock: bool,
    pub batch_size: Option<u32>,
    pub ubatch_size: Option<u32>,
    pub threads: Option<u32>,
    pub threads_batch: Option<u32>,
    pub host: Option<String>,
    pub port: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Preset {
    pub name: String,
    pub path: PathBuf,
    pub global: GlobalSettings,
    pub slots: Vec<Slot>,
}

fn parse_slot_section(section_name: &str, values: &HashMap<String, String>) -> Slot {
    let parts: Vec<&str> = section_name.splitn(2, '-').collect();
    let index = parts[0].parse().unwrap_or(0);
    let name = if parts.len() > 1 {
        parts[1].to_string()
    } else {
        section_name.to_string()
    };

    let load_on_startup = values
        .get("load-on-startup")
        .map(|v| v.trim() == "true")
        .unwrap_or(false);

    Slot {
        index,
        name,
        model_path: values.get("model").map(|p| PathBuf::from(p.clone())),
        ctx_size: values.get("ctx-size").and_then(|v| v.trim().parse().ok()),
        cache_type_k: values.get("cache-type-k").cloned(),
        cache_type_v: values.get("cache-type-v").cloned(),
        temp: values.get("temp").and_then(|v| v.trim().parse().ok()),
        top_p: values.get("top-p").and_then(|v| v.trim().parse().ok()),
        min_p: values.get("min-p").and_then(|v| v.trim().parse().ok()),
        top_k: values.get("top-k").and_then(|v| v.trim().parse().ok()),
        repeat_penalty: values
            .get("repeat-penalty")
            .and_then(|v| v.trim().parse().ok()),
        repeat_last_n: values
            .get("repeat-last-n")
            .and_then(|v| v.trim().parse().ok()),
        load_on_startup,
        jinja: values.get("jinja").map(|v| v.trim() == "true"),
        parallel: values.get("parallel").and_then(|v| v.trim().parse().ok()),
        image_min_tokens: values
            .get("image-min-tokens")
            .and_then(|v| v.trim().parse().ok()),
    }
}

fn parse_global_section(values: &HashMap<String, String>) -> GlobalSettings {
    let n_gpu_layers = values
        .get("n-gpu-layers")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(-1);

    let flash_attn = values
        .get("flash-attn")
        .map(|v| v.trim() == "true")
        .unwrap_or(false);
    let mlock = values
        .get("mlock")
        .map(|v| v.trim() == "true")
        .unwrap_or(false);

    GlobalSettings {
        n_gpu_layers,
        flash_attn,
        mlock,
        batch_size: values.get("batch-size").and_then(|v| v.trim().parse().ok()),
        ubatch_size: values
            .get("ubatch-size")
            .and_then(|v| v.trim().parse().ok()),
        threads: values.get("threads").and_then(|v| v.trim().parse().ok()),
        threads_batch: values
            .get("threads-batch")
            .and_then(|v| v.trim().parse().ok()),
        host: values.get("host").cloned(),
        port: values.get("port").and_then(|v| v.trim().parse().ok()),
    }
}

pub fn parse_preset(path: &Path) -> Result<Preset, color_eyre::Report> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid preset filename"))?
        .to_string();

    let content = fs::read_to_string(path)?;

    let mut global_values: HashMap<String, String> = HashMap::new();
    let mut current_section = String::from("*");
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    let mut current_values: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let next_section = trimmed[1..trimmed.len() - 1].to_string();
            if current_section == "*" {
                global_values.clone_from(&current_values);
            } else {
                sections.push((current_section.clone(), current_values.clone()));
            }
            current_values.clear();
            current_section = next_section;
            continue;
        }
        if let Some(eq_pos) = trimmed.find('=') {
            let key = trimmed[..eq_pos].trim().to_string();
            let value = trimmed[eq_pos + 1..].trim().to_string();
            current_values.insert(key, value);
        }
    }

    if current_section == "*" {
        global_values.clone_from(&current_values);
    } else {
        sections.push((current_section, current_values));
    }

    let mut slots: Vec<Slot> = Vec::new();
    for (sec_name, sec_vals) in sections {
        if sec_name != "*" {
            let slot = parse_slot_section(&sec_name, &sec_vals);
            slots.push(slot);
        }
    }

    slots.sort_by_key(|s| s.index);

    Ok(Preset {
        name,
        path: path.to_path_buf(),
        global: parse_global_section(&global_values),
        slots,
    })
}

pub fn list_presets() -> Result<Vec<Preset>, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let presets_dir = PathBuf::from(&home).join(".config/muthr/llama/presets");

    if !presets_dir.exists() {
        return Ok(Vec::new());
    }

    let mut presets: Vec<Preset> = Vec::new();
    for entry in fs::read_dir(presets_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "ini") {
            match parse_preset(&path) {
                Ok(preset) => presets.push(preset),
                Err(e) => eprintln!("[WARN] Failed to parse {:?}: {}", path, e),
            }
        }
    }

    presets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(presets)
}

pub fn resolve_preset(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(&home).join(format!(".config/muthr/llama/presets/{}.ini", name));
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

pub fn resolve_opencode_config(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(&home).join(format!(".config/opencode/opencode-{}.json", name));
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

pub fn expand_home(path: &Path) -> PathBuf {
    let mut expanded = PathBuf::new();
    let mut components = path.components().peekable();

    let starts_with_home = components
        .peek()
        .and_then(|c| {
            if let std::path::Component::Normal(s) = c {
                Some(s)
            } else {
                None
            }
        })
        .map(|s| s.to_str() == Some("~"))
        .unwrap_or(false);

    if starts_with_home {
        components.next();
        if let Ok(home) = std::env::var("HOME") {
            expanded.push(&home);
        }
    }

    for comp in components {
        expanded.push(comp);
    }

    expanded
}

pub fn model_name_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    let suffixes = [
        "-q4_k_m", "-Q4_K_M", "-q4-k-m", "-q5_k_m", "-Q5_K_M", "-q5-k-m", "-q8_0", "-Q8_0",
        "-q8-0", "-q3_k_s", "-Q3_K_S", "-q2_k", "-Q2_K",
    ];

    suffixes
        .iter()
        .fold(stem, |s, suffix| s.strip_suffix(suffix).unwrap_or(s))
        .to_string()
}
