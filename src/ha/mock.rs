//! Stateful, network-free HA replacement for the regular application.
//! The caller supplies a dedicated SQLite database, never the real home database.
use super::{client::HistoryResult, models::Entity, HomeAssistantClient, NotifyEvent, Room};
use anyhow::{bail, ensure, Context, Result};
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sqlx::{SqliteConnection, SqlitePool};
use tokio::sync::{mpsc, Mutex};

#[cfg(test)]
mod tests;

pub struct MockHa {
    pool: SqlitePool,
    events: mpsc::Sender<NotifyEvent>,
    writes: Mutex<()>,
}

fn fixtures() -> Result<Vec<Room>> {
    Ok(serde_json::from_str(include_str!("mock/fixtures.json"))?)
}

impl MockHa {
    pub async fn open(pool: SqlitePool, events: mpsc::Sender<NotifyEvent>) -> Result<Self> {
        let mut tx = pool.begin().await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS ha_mock_entities (entity_id TEXT PRIMARY KEY, body TEXT NOT NULL)")
            .execute(&mut *tx).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS ha_mock_history (id INTEGER PRIMARY KEY, entity_id TEXT NOT NULL, at INTEGER NOT NULL, state TEXT NOT NULL)")
            .execute(&mut *tx).await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ha_mock_history_time ON ha_mock_history(entity_id, at)",
        )
        .execute(&mut *tx)
        .await?;
        let now = Utc::now();
        for entity in fixtures()?.into_iter().flat_map(|room| room.entities) {
            let inserted = sqlx::query("INSERT OR IGNORE INTO ha_mock_entities VALUES (?, ?)")
                .bind(&entity.entity_id)
                .bind(serde_json::to_string(&entity)?)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 1;
            if inserted {
                // Seed one day of explicitly synthetic sensor history. Later reads
                // use recorded changes and never fabricate successful commands.
                for hour in 0..=24 {
                    let at = now - Duration::hours(24 - hour);
                    let state = match entity.state.parse::<f64>() {
                        Ok(value) => format!(
                            "{:.1}",
                            value + (hour as f64 * std::f64::consts::PI / 12.0).sin() * 1.5
                        ),
                        Err(_) => entity.state.clone(),
                    };
                    sqlx::query(
                        "INSERT INTO ha_mock_history(entity_id, at, state) VALUES (?, ?, ?)",
                    )
                    .bind(&entity.entity_id)
                    .bind(at.timestamp_millis())
                    .bind(state)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(Self {
            pool,
            events,
            writes: Mutex::new(()),
        })
    }

    /// Populate ordinary bot inventory before polling starts. Existing aliases,
    /// visibility and access preferences remain under the user's control.
    pub async fn seed_inventory(&self) -> Result<()> {
        for room in self.fetch_rooms().await? {
            crate::db::rooms::sync_rooms_from_ha(&room.id, &room.name, &self.pool).await?;
            for entity in room.entities {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM devices WHERE entity_id = ?)")
                        .bind(&entity.entity_id)
                        .fetch_one(&self.pool)
                        .await?;
                crate::db::devices::sync_device(
                    &entity.entity_id,
                    &room.id,
                    &entity.name,
                    entity.device_class.as_deref().unwrap_or("undefined"),
                    &self.pool,
                )
                .await?;
                if !exists {
                    sqlx::query("UPDATE hidden_entities SET hide = 0 WHERE entity_id = ?")
                        .bind(&entity.entity_id)
                        .execute(&self.pool)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn entity(conn: &mut SqliteConnection, id: &str) -> Result<Entity> {
        let body: String =
            sqlx::query_scalar("SELECT body FROM ha_mock_entities WHERE entity_id = ?")
                .bind(id)
                .fetch_optional(conn)
                .await?
                .context("Unknown mock entity")?;
        Ok(serde_json::from_str(&body)?)
    }

    async fn change(&self, domain: &str, service: &str, id: &str, data: Value) -> Result<()> {
        ensure!(
            id.split_once('.')
                .is_some_and(|(actual, _)| actual == domain),
            "Mock service domain does not match entity"
        );
        ensure!(
            data.as_object()
                .is_some_and(|values| values.keys().all(|key| key == "temperature")),
            "Unsupported mock service data"
        );
        let _write = self.writes.lock().await;
        let mut tx = self.pool.begin().await?;
        let mut entity = Self::entity(&mut tx, id).await?;
        let mut changes = Vec::new();
        match (domain, service, id) {
            ("scene", "turn_on", "scene.evening")
            | ("script", "turn_on", "script.all_lights_off") => {
                ensure!(data == json!({}), "Mock scene accepts no parameters");
                let targets = if domain == "scene" {
                    vec![("light.floor_lamp", "on"), ("light.ceiling", "off")]
                } else {
                    vec![
                        ("light.floor_lamp", "off"),
                        ("light.ceiling", "off"),
                        ("light.bedside", "off"),
                        ("light.kitchen", "off"),
                    ]
                };
                for (target, state) in targets {
                    let mut target = Self::entity(&mut tx, target).await?;
                    let before = target.state.clone();
                    target.state = state.into();
                    changes.push((before, target));
                }
                let before = entity.state.clone();
                // A script finishes synchronously in the mock; scenes retain activation time.
                entity.state = if domain == "scene" {
                    Utc::now().to_rfc3339()
                } else {
                    "off".into()
                };
                changes.push((before, entity));
            }
            ("light" | "switch", "turn_on" | "turn_off" | "toggle", _) => {
                ensure!(data == json!({}), "Mock switch accepts no parameters");
                ensure!(
                    matches!(entity.state.as_str(), "on" | "off"),
                    "Mock device is unavailable"
                );
                let before = entity.state.clone();
                entity.state =
                    if service == "turn_on" || (service == "toggle" && before == "off") {
                        "on"
                    } else {
                        "off"
                    }
                    .into();
                changes.push((before, entity));
            }
            ("climate", "set_temperature", _) => {
                let target = data["temperature"]
                    .as_f64()
                    .context("Missing mock target temperature")?
                    as f32;
                ensure!(
                    entity.temperature_target_is_valid(target),
                    "Mock temperature is outside device limits"
                );
                let step = entity
                    .attributes
                    .target_temp_step
                    .context("Missing temperature step")?;
                ensure!(
                    (target / step - (target / step).round()).abs() < 0.001,
                    "Mock temperature does not match device step"
                );
                let before = entity.state.clone();
                entity.attributes.temperature = Some(target);
                changes.push((before, entity));
            }
            ("climate", "set_hvac_mode", _) => {
                let mode = data["hvac_mode"].as_str().context("Missing HVAC mode")?;
                ensure!(
                    entity.supports_hvac_mode(mode),
                    "Mock HVAC mode unavailable"
                );
                let before = entity.state.clone();
                entity.state = mode.into();
                changes.push((before, entity));
            }
            ("climate", "toggle" | "turn_on" | "turn_off", _) => {
                ensure!(
                    data == json!({}),
                    "Mock climate power accepts no parameters"
                );
                ensure!(
                    matches!(entity.state.as_str(), "heat" | "off"),
                    "Mock climate is unavailable"
                );
                let before = entity.state.clone();
                entity.state =
                    if service == "turn_on" || (service == "toggle" && before == "off") {
                        "heat"
                    } else {
                        "off"
                    }
                    .into();
                changes.push((before, entity));
            }
            _ => bail!("Unsupported mock service: {domain}.{service}"),
        }
        let now = Utc::now();
        let mut events = Vec::new();
        for (before, entity) in changes {
            sqlx::query("UPDATE ha_mock_entities SET body = ? WHERE entity_id = ?")
                .bind(serde_json::to_string(&entity)?)
                .bind(&entity.entity_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO ha_mock_history(entity_id, at, state) VALUES (?, ?, ?)")
                .bind(&entity.entity_id)
                .bind(now.timestamp_millis())
                .bind(&entity.state)
                .execute(&mut *tx)
                .await?;
            events.push(NotifyEvent {
                entity_id: entity.entity_id,
                old_state: before,
                new_state: entity.state,
                friendly_name: entity.name,
                device_class: entity.device_class,
                received_at: now,
            });
        }
        tx.commit().await?;
        for event in events {
            if self.events.try_send(event).is_err() {
                log::debug!("Mock HA event queue unavailable; state remains persisted");
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl HomeAssistantClient for MockHa {
    fn is_mock(&self) -> bool {
        true
    }
    async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1 FROM ha_mock_entities LIMIT 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }
    async fn fetch_rooms(&self) -> Result<Vec<Room>> {
        let mut rooms = fixtures()?;
        for room in &mut rooms {
            room.entities = self
                .fetch_states_by_ids(
                    &room
                        .entities
                        .iter()
                        .map(|entity| entity.entity_id.clone())
                        .collect::<Vec<_>>(),
                )
                .await?;
        }
        Ok(rooms)
    }
    async fn fetch_states_by_ids(&self, ids: &[String]) -> Result<Vec<Entity>> {
        let mut conn = self.pool.acquire().await?;
        let mut entities = Vec::new();
        for id in ids {
            entities.push(Self::entity(&mut conn, id).await?);
        }
        Ok(entities)
    }
    async fn fetch_action_entities(&self) -> Result<Vec<Entity>> {
        self.fetch_states_by_ids(&["scene.evening".into(), "script.all_lights_off".into()])
            .await
    }
    async fn fetch_history(&self, id: &str, hours: u32, offset: i32) -> Result<HistoryResult> {
        ensure!(
            (1..=24 * 31).contains(&hours) && (-24 * 366..=0).contains(&offset),
            "Unsupported mock history interval"
        );
        let end_time = Utc::now() + Duration::hours(offset.into());
        let start_time = end_time - Duration::hours(hours.into());
        self.fetch_history_range(id, start_time, end_time).await
    }
    async fn fetch_history_range(
        &self,
        id: &str,
        start_time: chrono::DateTime<Utc>,
        end_time: chrono::DateTime<Utc>,
    ) -> Result<HistoryResult> {
        ensure!(start_time < end_time, "History start must precede end");
        self.fetch_states_by_ids(&[id.into()]).await?;
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT at, state FROM ha_mock_history WHERE entity_id = ? AND at <= ? AND (at >= ? OR id = (SELECT id FROM ha_mock_history WHERE entity_id = ? AND at < ? ORDER BY at DESC, id DESC LIMIT 1)) ORDER BY at, id")
            .bind(id).bind(end_time.timestamp_millis()).bind(start_time.timestamp_millis())
            .bind(id).bind(start_time.timestamp_millis()).fetch_all(&self.pool).await?;
        let points = rows
            .into_iter()
            .map(|(at, state)| {
                Ok((
                    chrono::DateTime::from_timestamp_millis(at)
                        .context("Invalid mock history time")?,
                    state,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(HistoryResult {
            points,
            start_time,
            end_time,
        })
    }
    async fn call_service(&self, domain: &str, service: &str, id: &str) -> Result<()> {
        self.change(domain, service, id, json!({})).await
    }
    async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        id: &str,
        data: Value,
    ) -> Result<()> {
        self.change(domain, service, id, data).await
    }
}
