use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize, Debug)]
pub struct ModelList {
    pub data: Vec<ModelInfo>,
}

#[derive(Deserialize, Debug)]
pub struct ModelInfo {
    pub id: String,
    #[serde(rename = "status")]
    _status: ModelStatus,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub struct ModelStatus {
    pub value: String,
}

pub async fn verify_health(host: &str, port: u16) -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    let url = format!("http://{}:{}/health", host, port);
    client.get(&url).send().await.is_ok()
}

pub async fn query_models(
    host: &str,
    port: u16,
    field: &str,
) -> Result<String, color_eyre::Report> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let url = format!("http://{}:{}/v1/models", host, port);
    let response = client.get(&url).send().await?;
    let json: serde_json::Value = response.json().await?;

    let result = extract_field(&json, field);
    Ok(result)
}

fn extract_field(json: &serde_json::Value, field: &str) -> String {
    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        for item in data {
            if let (Some(id), Some(status)) = (item.get("id"), item.get("status")) {
                if let Some(value) = status.get("value").and_then(|v| v.as_str()) {
                    if value == "loaded" {
                        match field {
                            ".data[] | select(.status.value == \"loaded\") | .id" => {
                                return id.as_str().unwrap_or("").to_string();
                            }
                            ".data[] | select(.status.value == \"loaded\") | .meta.n_ctx" => {
                                if let Some(n_ctx) = item
                                    .get("meta")
                                    .and_then(|m| m.get("n_ctx"))
                                    .and_then(|v| v.as_u64())
                                {
                                    return n_ctx.to_string();
                                }
                                return "16000".to_string();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    String::new()
}

pub async fn get_loaded_model(host: &str, port: u16) -> Result<String, color_eyre::Report> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let url = format!("http://{}:{}/v1/models", host, port);
    let response = client.get(&url).send().await?;
    let json: serde_json::Value = response.json().await?;

    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        for item in data {
            if let (Some(id), Some(status)) = (item.get("id"), item.get("status")) {
                if let Some(value) = status.get("value").and_then(|v| v.as_str()) {
                    if value == "loaded" {
                        return Ok(id.as_str().unwrap_or("").to_string());
                    }
                }
            }
        }
    }

    Err(color_eyre::eyre::eyre!("No loaded model found"))
}

pub async fn get_ctx_window(host: &str, port: u16) -> Result<u32, color_eyre::Report> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let url = format!("http://{}:{}/v1/models", host, port);
    let response = client.get(&url).send().await?;
    let json: serde_json::Value = response.json().await?;

    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        for item in data {
            if let (Some(_id), Some(status)) = (item.get("id"), item.get("status")) {
                if let Some(value) = status.get("value").and_then(|v| v.as_str()) {
                    if value == "loaded" {
                        if let Some(n_ctx) = item
                            .get("meta")
                            .and_then(|m| m.get("n_ctx"))
                            .and_then(|v| v.as_u64())
                        {
                            return Ok(n_ctx as u32);
                        }
                    }
                }
            }
        }
    }

    Ok(16000)
}

pub async fn poll_loaded_model(
    host: &str,
    port: u16,
    max_retries: u32,
    interval_secs: f32,
) -> Result<String, color_eyre::Report> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let url = format!("http://{}:{}/v1/models", host, port);

    for i in 1..=max_retries {
        if let Ok(response) = client.get(&url).send().await {
            if let Ok(json) = response.json::<serde_json::Value>().await {
                if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                    for item in data {
                        if let (Some(id), Some(status)) = (item.get("id"), item.get("status")) {
                            if let Some(value) = status.get("value").and_then(|v| v.as_str()) {
                                if value == "loaded" {
                                    return Ok(id.as_str().unwrap_or("").to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        if i % 5 == 0 {
            eprintln!(
                "[WARN] Still waiting for loaded model... ({}/{})",
                i, max_retries
            );
        }

        tokio::time::sleep(Duration::from_secs_f32(interval_secs)).await;
    }

    Err(color_eyre::eyre::eyre!(
        "Timeout: Could not get loaded model from llama-server"
    ))
}
