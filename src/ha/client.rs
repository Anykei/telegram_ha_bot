use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use reqwest::{header, Client};
use serde::Deserialize;
use serde_json::json;
use urlencoding::encode;
use crate::ha::models::Entity;
use super::Room;

#[derive(Deserialize)]
struct HaHistoryItemFull {
    state: String,
    last_updated: DateTime<Utc>,
}

pub struct HAClient {
    url: String,
    client: Client,
}

pub struct HistoryResult {
    pub points: Vec<(DateTime<Utc>, String)>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
}

impl HAClient {
    pub fn new(url: String, token: String, timeout_secs: u64, connect_timeout: u64) -> Self {
        let mut headers = header::HeaderMap::new();
        let auth_header = format!("Bearer {}", token);
        let mut auth_val = header::HeaderValue::from_str(&auth_header)
            .expect("Invalid token format");
        auth_val.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, auth_val);

        Self {
            url: url.trim_end_matches('/').to_string(),
            client: Client::builder()
                .default_headers(headers)
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .connect_timeout(std::time::Duration::from_secs(connect_timeout))
                .build()
                .expect("Failed to build HA HTTP client"),
        }
    }

    /// Вспомогательный метод для выполнения запросов к Template API (Google Standard: DRY)
    async fn post_template<T: serde::de::DeserializeOwned>(&self, template: &str) -> Result<T> {
        let url = format!("{}/api/template", self.url);
        let res = self.client
            .post(&url)
            .json(&json!({ "template": template }))
            .send()
            .await
            .with_context(|| format!("Failed to send template to {}", url))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("HA API Error {}: {}", status, body));
        }

        res.json::<T>().await.context("Failed to parse template response")
    }

    pub async fn fetch_rooms(&self) -> Result<Vec<Room>> {
        self.post_template(super::templates::ROOMS_TEMPLATE).await
    }

    pub async fn fetch_history(
        &self,
        entity_id: &str,
        hours: u32,
        offset: i32,
    ) -> Result<HistoryResult> {
        let now = Utc::now();
        let end_time = now + Duration::hours(offset as i64);
        let start_time = end_time - Duration::hours(hours as i64);

        let start_iso = start_time.to_rfc3339();
        let end_iso = end_time.to_rfc3339();

        let url = format!(
            "{}/api/history/period/{}?end_time={}&filter_entity_id={}&no_attributes",
            self.url,
            encode(&start_iso),
            encode(&end_iso),
            entity_id
        );

        let res = self.client.get(&url).send().await.context("HA History API failure")?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("HA API returned error: {}", res.status()));
        }

        let raw_history: Vec<Vec<HaHistoryItemFull>> = res.json().await?;
        let mut points: Vec<(DateTime<Utc>, String)> = raw_history
            .into_iter()
            .flatten()
            .map(|item| (item.last_updated, item.state))
            .collect();

        if points.is_empty() {
            debug!("Gap detected for {}. Fetching last known state before {}", entity_id, start_iso);

            let backfill_url = format!("{}/api/states/{}", self.url, entity_id);
            if let Ok(resp) = self.client.get(&backfill_url).send().await {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    if let Some(s) = val["state"].as_str() {
                        points.push((start_time, s.to_string()));
                    }
                }
            }
        }

        if let Some((_, last_state)) = points.last().cloned() {
            points.push((end_time, last_state));
        }

        debug!("Fetched {} points for {} [{} -> {}]", points.len(), entity_id, start_iso, end_iso);

        Ok(HistoryResult {
            points,
            start_time,
            end_time,
        })
    }

    pub async fn fetch_states_by_ids(&self, entity_ids: &[String]) -> Result<Vec<Entity>> {
        if entity_ids.is_empty() { return Ok(vec![]); }

        let ids_json = serde_json::to_string(entity_ids)?;
        let template = format!(
            r#"[
              {{%- set items = {} -%}}
              {{%- for eid in items -%}}
              {{
                "entity_id": "{{{{ eid }}}}",
                "state": "{{{{ states(eid) }}}}",
                "friendly_name": "{{{{ state_attr(eid, 'friendly_name') | default('', true) | replace('"', '\\"') }}}}",
                "device_class": "{{{{ state_attr(eid, 'device_class') | default('', true) }}}}"
              }} {{{{ "," if not loop.last }}}}
              {{%- endfor -%}}
            ]"#,
            ids_json
        );

        self.post_template(&template).await
    }

    pub async fn call_service(&self, domain: &str, service: &str, entity_id: &str) -> Result<()> {
        let url = format!("{}/api/services/{}/{}", self.url, domain, service);
        let res = self.client.post(&url)
            .json(&json!({ "entity_id": entity_id }))
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Service call failed: {}", res.status()));
        }
        Ok(())
    }

    pub async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        entity_id: &str,
        data: serde_json::Value
    ) -> Result<()> {
        let url = format!("{}/api/services/{}/{}", self.url, domain, service);

        // Объединяем entity_id и дополнительные данные
        let mut body = data;
        body["entity_id"] = serde_json::json!(entity_id);

        let res = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to call HA service with data")?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("HA API Error {}: {}", status, body));
        }
        Ok(())
    }
}