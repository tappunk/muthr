use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::preset::Preset;

pub fn generate_runtime_config(
    preset: &Preset,
    port: u16,
    mount_point: &Path,
) -> Result<PathBuf, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let template_path = PathBuf::from(&home).join(".config/muthr/opencode-config.json");

    let content = fs::read_to_string(&template_path)?;
    let content = content
        .replace("__CTX_WINDOW__", "16000")
        .replace("__DEFAULT_MODEL__", "placeholder-model")
        .replace("__LLAMA_PORT__", "8080");
    let mut config: Value = serde_json::from_str(&content)?;

    let primary_slot = preset
        .slots
        .first()
        .ok_or_else(|| color_eyre::eyre::eyre!("No slots found in preset"))?;

    let model_id = format!("01-{}", primary_slot.name);
    let ctx_window = primary_slot.ctx_size.unwrap_or(200000);

    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "model".to_string(),
            Value::String(format!("llama-cpp/{}", model_id)),
        );
        obj.insert(
            "small_model".to_string(),
            Value::String(format!("llama-cpp/{}", model_id)),
        );

        if let Some(agent) = obj.get_mut("agent").and_then(|a| a.as_object_mut()) {
            for (_name, agent_cfg) in agent.iter_mut() {
                if let Some(agent_obj) = agent_cfg.as_object_mut() {
                    if let Some(model_val) = agent_obj.get("model") {
                        if let Some(s) = model_val.as_str() {
                            if s.contains("placeholder-model") {
                                agent_obj.insert(
                                    "model".to_string(),
                                    Value::String(format!("llama-cpp/{}", model_id)),
                                );
                            }
                        }
                    }
                }
            }
        }

        if let Some(provider) = obj.get_mut("provider").and_then(|p| p.get_mut("llama-cpp")) {
            if let Some(p_obj) = provider.as_object_mut() {
                let mut options = Map::new();
                options.insert(
                    "baseURL".to_string(),
                    Value::String(format!("http://host.lima.internal:{}/v1", port)),
                );
                p_obj.insert("options".to_string(), Value::Object(options));

                let mut models_map = Map::new();
                let mut inner_model = Map::new();
                inner_model.insert("name".to_string(), Value::String(model_id.clone()));
                inner_model.insert("tools".to_string(), Value::Bool(true));
                inner_model.insert(
                    "context_window".to_string(),
                    Value::Number(ctx_window.into()),
                );

                let mut limit_map = Map::new();
                limit_map.insert("context".to_string(), Value::Number(ctx_window.into()));
                limit_map.insert("output".to_string(), Value::Number(8192.into()));
                inner_model.insert("limit".to_string(), Value::Object(limit_map));

                models_map.insert(model_id, Value::Object(inner_model));
                p_obj.insert("models".to_string(), Value::Object(models_map));
            }
        }

        if let Some(mcp) = obj.get_mut("mcp").and_then(|m| m.get_mut("filesystem")) {
            if let Some(m_obj) = mcp.as_object_mut() {
                m_obj.insert(
                    "command".to_string(),
                    serde_json::json!(["mcp-server-filesystem", mount_point.to_string_lossy()]),
                );
            }
        }
    }

    let runtime_dir = PathBuf::from(&home).join(".cache/muthr/opencode_runtimes");
    fs::create_dir_all(&runtime_dir)?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let runtime_path = runtime_dir.join(format!("opencode-runtime-{}.json", timestamp));
    fs::write(&runtime_path, serde_json::to_string_pretty(&config)?)?;

    Ok(runtime_path)
}

pub fn generate_host_config(
    preset: &Preset,
    port: u16,
) -> Result<serde_json::Value, color_eyre::Report> {
    let primary_slot = preset
        .slots
        .first()
        .ok_or_else(|| color_eyre::eyre::eyre!("No slots found in preset"))?;

    let model_id = format!("01-{}", primary_slot.name);

    let mut models_config = serde_json::Map::new();
    let mut model_entry = serde_json::Map::new();
    model_entry.insert(
        "name".to_string(),
        serde_json::json!(primary_slot.name.clone()),
    );
    model_entry.insert("tools".to_string(), serde_json::json!(true));

    let ctx_window = primary_slot.ctx_size.unwrap_or(200000);

    model_entry.insert("context_window".to_string(), serde_json::json!(ctx_window));
    let mut limit = serde_json::Map::new();
    limit.insert("context".to_string(), serde_json::json!(ctx_window));
    limit.insert("output".to_string(), serde_json::json!(8192));
    model_entry.insert("limit".to_string(), serde_json::Value::Object(limit));
    models_config.insert(model_id.clone(), serde_json::Value::Object(model_entry));

    for slot in &preset.slots {
        if slot.index > 1 {
            let mut slot_entry = serde_json::Map::new();
            slot_entry.insert("name".to_string(), serde_json::json!(slot.name.clone()));
            slot_entry.insert("tools".to_string(), serde_json::json!(true));
            slot_entry.insert(
                "context_window".to_string(),
                serde_json::json!(slot.ctx_size.unwrap_or(16384)),
            );
            let mut limit = serde_json::Map::new();
            limit.insert(
                "context".to_string(),
                serde_json::json!(slot.ctx_size.unwrap_or(16384)),
            );
            limit.insert("output".to_string(), serde_json::json!(8192));
            slot_entry.insert("limit".to_string(), serde_json::Value::Object(limit));
            let slot_id = format!("0{}-{}", slot.index, slot.name);
            models_config.insert(slot_id, serde_json::Value::Object(slot_entry));
        }
    }

    let mut provider_map = serde_json::Map::new();
    provider_map.insert(
        "npm".to_string(),
        serde_json::json!("@ai-sdk/openai-compatible"),
    );
    provider_map.insert(
        "name".to_string(),
        serde_json::json!("llama-cpp (localhost)"),
    );
    let mut options = serde_json::Map::new();
    let config_port = preset.global.port.unwrap_or(port as u32) as u16;
    options.insert(
        "baseURL".to_string(),
        serde_json::json!(format!("http://127.0.0.1:{}/v1", config_port)),
    );
    provider_map.insert("options".to_string(), serde_json::Value::Object(options));
    provider_map.insert(
        "models".to_string(),
        serde_json::Value::Object(models_config),
    );

    let mut config = serde_json::Map::new();
    config.insert(
        "model".to_string(),
        serde_json::json!(format!("llama-cpp/{}", model_id)),
    );
    config.insert(
        "provider".to_string(),
        serde_json::Value::Object(provider_map),
    );

    Ok(serde_json::Value::Object(config))
}
