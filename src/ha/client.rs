use super::Room;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use chrono::{DateTime, Utc};
use log::{debug, info};
use reqwest::header;

pub struct HAClient {
    pub url: String,
    pub token: String,
    client: reqwest::Client,
}

impl HAClient {
    pub fn new(url: String, token: String, timeout:u64, connect_timeout:u64) -> Self {
        let mut headers = header::HeaderMap::new();
        let mut auth_val = header::HeaderValue::from_str(&format!("Bearer {}", token))
            .expect("Invalid token format");
        auth_val.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, auth_val);

        Self {
            url: url.trim_end_matches('/').to_string(),
            token,
            client: reqwest::Client::builder()
                .default_headers(headers)
                .timeout(std::time::Duration::from_secs(timeout))
                .connect_timeout(std::time::Duration::from_secs(connect_timeout))
                .build()
                .expect("Failed to build HA HTTP client"),
        }
    }

    pub async fn fetch_rooms(&self) -> Result<Vec<Room>> {
        let template = super::templates::ROOMS_TEMPLATE;
        let url = format!("{}/api/template", self.url);

        let res = self.client
            .post(url)
            .json(&json!({ "template": template }))
            .send()
            .await
            .context("Network error to fetch rooms")?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("HA API Error (fetch_rooms) {}: {}", status, body));
        }

        Ok(res.json().await?)
    }

    pub async fn toggle(&self, entity_id: &str) -> Result<()> {
        let url = format!("{}/api/services/homeassistant/toggle", self.url);

        let res = self.client.post(url)
            .json(&json!({ "entity_id": entity_id }))
            .send()
            .await
            .context("Network error toggle")?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("HA API Error (toggle) {}: {}", status, body));
        }
        Ok(())
    }

    pub async fn fetch_history(&self, entity_id: &str, hours: u64) -> Result<Vec<(DateTime<Utc>, String)>> {
        let now = Utc::now();
        let duration = chrono::Duration::hours(hours as i64);
        let start_time = now - duration;

        let start_time_str = start_time.to_rfc3339();
        let end_time_str = now.to_rfc3339();

        let start_time_encoded = urlencoding::encode(&start_time_str);
        let end_time_encoded = urlencoding::encode(&end_time_str);

        let url = format!(
            "{}/api/history/period/{}?end_time={}&filter_entity_id={}&minimal_response",
            self.url,
            start_time_encoded,
            end_time_encoded, // Теперь HA отдаст всё до текущего момента
            entity_id
        );

        let res = self.client
            .get(&url)
            .send()
            .await
            .context("Network error to fetch history")?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("HA API Error {}: {}", status, body));
        }

        let history: Vec<Vec<Value>> = res.json().await?;
        let mut data = Vec::new();

        if let Some(entity_states) = history.first() {
            for state_obj in entity_states {
                let state_str = match &state_obj["state"] {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };

                let time_str = state_obj["last_updated"]
                    .as_str()
                    .or_else(|| state_obj["last_changed"].as_str())
                    .unwrap_or("");

                if let Ok(time) = DateTime::parse_from_rfc3339(time_str) {
                    data.push((time.with_timezone(&Utc), state_str));
                }
            }
        }

        if data.is_empty() {
            debug!("Data history empty,  {}", entity_id);
            let current = self.client.get(format!("{}/api/states/{}", self.url, entity_id)).send().await?;
            if let Ok(state_val) = current.json::<Value>().await {
                if let Some(s) = state_val["state"].as_str() {
                    data.push((start_time, s.to_string()));
                }
            }
        }

        info!("Complete received {} points for {}", data.len(), entity_id);
        Ok(data)
    }
}