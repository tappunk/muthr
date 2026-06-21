use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::preset::Preset;

fn replace_placeholders_in_value(value: &mut Value, replacements: &[(&str, &str)]) {
    match value {
        Value::String(s) => {
            for (from, to) in replacements {
                *s = s.replace(from, to);
            }
        }
        Value::Array(arr) => arr
            .iter_mut()
            .for_each(|v| replace_placeholders_in_value(v, replacements)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|v| replace_placeholders_in_value(v, replacements)),
        _ => {}
    }
}

pub fn generate_runtime_config(
    preset: &Preset,
    port: u16,
    mount_point: &Path,
) -> Result<PathBuf, color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let template_path = PathBuf::from(&home).join(".config/muthr/clients/opencode-config.json");

    let content = fs::read_to_string(&template_path)?;
    let primary_slot = preset
        .slots
        .first()
        .ok_or_else(|| color_eyre::eyre::eyre!("No slots found in preset"))?;

    let model_id = primary_slot.name.clone();
    let ctx_window = primary_slot.ctx_size.unwrap_or(200000);
    let ctx_str = ctx_window.to_string();

    let content = content.replace("__CTX_WINDOW__", &ctx_str);

    let mut config: Value = serde_json::from_str(&content)?;

    let port_str = port.to_string();
    let mount_str = mount_point.to_string_lossy().into_owned();

    let replacements: Vec<(&str, &str)> = vec![
        ("__CTX_WINDOW__", ctx_str.as_str()),
        ("__DEFAULT_MODEL__", model_id.as_str()),
        ("__LLAMA_PORT__", port_str.as_str()),
        ("__INJECTED_MOUNT_POINT__", mount_str.as_str()),
    ];

    replace_placeholders_in_value(&mut config, &replacements);

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
                    agent_obj.insert(
                        "model".to_string(),
                        Value::String(format!("llama-cpp/{}", model_id)),
                    );
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
    }

    let runtime_dir = PathBuf::from(&home).join(".cache/muthr/opencode_runtimes");
    fs::create_dir_all(&runtime_dir)?;

    let runtime_path = runtime_dir.join(format!("opencode-runtime-{}.json", preset.name));
    fs::write(&runtime_path, serde_json::to_string_pretty(&config)?)?;

    Ok(runtime_path)
}
