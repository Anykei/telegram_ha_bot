use super::Room;
use crate::ha::models::Entity;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use reqwest::{header, Client};
use serde::Deserialize;
use serde_json::json;
use urlencoding::encode;

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

#[async_trait::async_trait]
pub trait HomeAssistantClient: Send + Sync {
    fn is_mock(&self) -> bool {
        false
    }

    async fn health_check(&self) -> Result<()>;

    async fn fetch_rooms(&self) -> Result<Vec<Room>>;

    async fn fetch_history(
        &self,
        entity_id: &str,
        hours: u32,
        offset: i32,
    ) -> Result<HistoryResult>;

    /// Exact UTC boundaries, without converting a calendar selection to hours.
    async fn fetch_history_range(
        &self,
        _entity_id: &str,
        _start: DateTime<Utc>,
        _end: DateTime<Utc>,
    ) -> Result<HistoryResult> {
        anyhow::bail!("Exact history ranges are not supported by this client")
    }

    async fn fetch_states_by_ids(&self, entity_ids: &[String]) -> Result<Vec<Entity>>;

    async fn fetch_action_entities(&self) -> Result<Vec<Entity>>;

    async fn call_service(&self, domain: &str, service: &str, entity_id: &str) -> Result<()>;

    async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        entity_id: &str,
        data: serde_json::Value,
    ) -> Result<()>;
}

impl HAClient {
    pub fn new(url: String, token: String, timeout_secs: u64, connect_timeout: u64) -> Self {
        let mut headers = header::HeaderMap::new();
        let auth_header = format!("Bearer {}", token);
        let mut auth_val =
            header::HeaderValue::from_str(&auth_header).expect("Invalid token format");
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
        let res = self
            .client
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

        let body = res
            .text()
            .await
            .context("Failed to read template response body")?;
        serde_json::from_str::<T>(&body).with_context(|| {
            format!(
                "Failed to parse template response: {}",
                short_template_response(&body)
            )
        })
    }

    pub async fn fetch_rooms(&self) -> Result<Vec<Room>> {
        self.post_template(super::templates::ROOMS_TEMPLATE).await
    }

    pub async fn health_check(&self) -> Result<()> {
        let url = format!("{}/api/", self.url);
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to call HA health endpoint {}", url))?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("HA health check failed: {}", res.status()));
        }

        Ok(())
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

        self.fetch_history_range(entity_id, start_time, end_time)
            .await
    }

    pub async fn fetch_history_range(
        &self,
        entity_id: &str,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<HistoryResult> {
        anyhow::ensure!(start_time < end_time, "History start must precede end");

        let start_iso = start_time.to_rfc3339();
        let end_iso = end_time.to_rfc3339();

        let url = format!(
            "{}/api/history/period/{}?end_time={}&filter_entity_id={}&no_attributes&significant_changes_only=0",
            self.url,
            encode(&start_iso),
            encode(&end_iso),
            encode(entity_id)
        );

        let res = self
            .client
            .get(&url)
            .send()
            .await
            .context("HA History API failure")?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("HA API returned error: {}", res.status()));
        }

        let raw_history: Vec<Vec<HaHistoryItemFull>> = res.json().await?;
        let mut points: Vec<(DateTime<Utc>, String)> = raw_history
            .into_iter()
            .flatten()
            .map(|item| (item.last_updated, item.state))
            .collect();

        // The history endpoint includes a recorded state at the start boundary
        // by default (skip_initial_state is deliberately absent). Never replace
        // missing recorder data with the current /api/states value.
        points.retain(|(time, _)| *time <= end_time);
        points.sort_by_key(|(time, _)| *time);

        debug!(
            "Fetched {} points for {} [{} -> {}]",
            points.len(),
            entity_id,
            start_iso,
            end_iso
        );

        Ok(HistoryResult {
            points,
            start_time,
            end_time,
        })
    }

    pub async fn fetch_states_by_ids(&self, entity_ids: &[String]) -> Result<Vec<Entity>> {
        if entity_ids.is_empty() {
            return Ok(vec![]);
        }

        let ids_json = serde_json::to_string(entity_ids)?;
        let template = format!(
            r#"[
              {{%- set items = {} -%}}
              {{%- for eid in items -%}}
              {{
                "entity_id": {{{{ eid | to_json }}}},
                "state": {{{{ states(eid) | to_json }}}},
                "name": {{{{ (state_attr(eid, 'friendly_name') | default(eid, true)) | to_json }}}},
                "device_class": {{{{ (state_attr(eid, 'device_class') | default('', true)) | to_json }}}},
                "attributes": {{%- if eid.startswith('climate.') -%}} {{
                  "current_temperature": {{{{ state_attr(eid, 'current_temperature') | to_json }}}},
                  "temperature": {{{{ state_attr(eid, 'temperature') | to_json }}}},
                  "target_temp_low": {{{{ state_attr(eid, 'target_temp_low') | to_json }}}},
                  "target_temp_high": {{{{ state_attr(eid, 'target_temp_high') | to_json }}}},
                  "target_temp_step": {{{{ state_attr(eid, 'target_temp_step') | to_json }}}},
                  "min_temp": {{{{ state_attr(eid, 'min_temp') | to_json }}}},
                  "max_temp": {{{{ state_attr(eid, 'max_temp') | to_json }}}},
                  "unit_of_measurement": {{{{ state_attr(eid, 'unit_of_measurement') | to_json }}}}
                }} {{%- else -%}} {{
                  "unit_of_measurement": {{{{ state_attr(eid, 'unit_of_measurement') | to_json }}}}
                }} {{%- endif -%}}
              }} {{{{ "," if not loop.last }}}}
              {{%- endfor -%}}
            ]"#,
            ids_json
        );

        self.post_template(&template).await
    }

    pub async fn fetch_action_entities(&self) -> Result<Vec<Entity>> {
        let template = r#"[
          {%- set items = states.script | list + states.scene | list -%}
          {%- for item in items -%}
          {
            "entity_id": {{ item.entity_id | to_json }},
            "state": {{ item.state | to_json }},
            "name": {{ (state_attr(item.entity_id, 'friendly_name') | default(item.entity_id, true)) | to_json }},
            "device_class": {{ (state_attr(item.entity_id, 'device_class') | default('', true)) | to_json }}
          } {{ "," if not loop.last }}
          {%- endfor -%}
        ]"#;

        self.post_template(template).await
    }

    pub async fn call_service(&self, domain: &str, service: &str, entity_id: &str) -> Result<()> {
        let url = format!("{}/api/services/{}/{}", self.url, domain, service);
        let res = self
            .client
            .post(&url)
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
        data: serde_json::Value,
    ) -> Result<()> {
        let url = format!("{}/api/services/{}/{}", self.url, domain, service);

        // Объединяем entity_id и дополнительные данные
        let mut body = data;
        body["entity_id"] = serde_json::json!(entity_id);

        let res = self
            .client
            .post(&url)
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

fn short_template_response(body: &str) -> String {
    let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= 500 {
        return normalized;
    }

    let mut short = normalized.chars().take(500).collect::<String>();
    short.push_str("...");
    short
}

#[async_trait::async_trait]
impl HomeAssistantClient for HAClient {
    async fn health_check(&self) -> Result<()> {
        HAClient::health_check(self).await
    }

    async fn fetch_rooms(&self) -> Result<Vec<Room>> {
        HAClient::fetch_rooms(self).await
    }

    async fn fetch_history(
        &self,
        entity_id: &str,
        hours: u32,
        offset: i32,
    ) -> Result<HistoryResult> {
        HAClient::fetch_history(self, entity_id, hours, offset).await
    }

    async fn fetch_history_range(
        &self,
        entity_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<HistoryResult> {
        HAClient::fetch_history_range(self, entity_id, start, end).await
    }

    async fn fetch_states_by_ids(&self, entity_ids: &[String]) -> Result<Vec<Entity>> {
        HAClient::fetch_states_by_ids(self, entity_ids).await
    }

    async fn fetch_action_entities(&self) -> Result<Vec<Entity>> {
        HAClient::fetch_action_entities(self).await
    }

    async fn call_service(&self, domain: &str, service: &str, entity_id: &str) -> Result<()> {
        HAClient::call_service(self, domain, service, entity_id).await
    }

    async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        entity_id: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        HAClient::call_service_with_data(self, domain, service, entity_id, data).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn empty_history_never_reads_or_inserts_current_state() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = HAClient::new(
            format!("http://{}", listener.local_addr()?),
            "test".into(),
            2,
            2,
        );
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            loop {
                let Ok(accepted) =
                    tokio::time::timeout(std::time::Duration::from_millis(250), listener.accept())
                        .await
                else {
                    break;
                };
                let (mut stream, _) = accepted?;
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut chunk = [0u8; 2048];
                    let count = stream.read(&mut chunk).await?;
                    anyhow::ensure!(count > 0, "Incomplete request");
                    request.extend_from_slice(&chunk[..count]);
                }
                let request = String::from_utf8(request)?;
                let path = request.lines().next().unwrap().to_string();
                let body = if path.contains("/api/states/") {
                    r#"{"state":"99"}"#
                } else {
                    "[]"
                };
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await?;
                requests.push(path);
            }
            Ok::<_, anyhow::Error>(requests)
        });
        let history = client.fetch_history("sensor.temperature", 24, -168).await?;
        assert!(history.points.is_empty());
        assert_eq!(history.end_time - history.start_time, Duration::hours(24));
        let start = DateTime::parse_from_rfc3339("2026-11-06T20:31:00+03:00")?.with_timezone(&Utc);
        let end = DateTime::parse_from_rfc3339("2026-11-07T08:07:00+03:00")?.with_timezone(&Utc);
        let exact = client
            .fetch_history_range("sensor.temperature", start, end)
            .await?;
        assert_eq!((exact.start_time, exact.end_time), (start, end));
        assert!(client
            .fetch_history_range("sensor.temperature", end, start)
            .await
            .is_err());
        let requests = server.await??;
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("/api/history/period/"));
        assert!(requests[0].contains("significant_changes_only=0"));
        assert!(!requests[0].contains("skip_initial_state"));
        assert!(requests[1].contains(&*encode(&start.to_rfc3339())));
        assert!(requests[1].contains(&format!("end_time={}", encode(&end.to_rfc3339()))));
        Ok(())
    }
}
