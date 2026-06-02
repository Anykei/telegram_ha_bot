use crate::ha::models::Entity;
use crate::ha::HomeAssistantClient;
use crate::models::AppConfig;
use anyhow::{Context, Result};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum InputIntent {
    DefineGraphInterval { device_id: i64, room_id: i64 },
}

#[derive(Debug)]
pub enum InteractionResult {
    Processed,
    RequiresDetail,
    RequiresInput(InputIntent),
    Error { error: String },
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceAction {
    Toggle,
    TurnOn,
    TurnOff,
    SetLevel(u8),
    SetTemperature(f32),
    GenerateChart(ChartParams),
    EnterManualInput,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartParams {
    pub period_hours: u32,
    pub offset_hours: i32, // 0 - текущее время, -24 - вчера и т.д.
}

#[async_trait::async_trait]
pub trait SmartEntity {
    fn get_info(&self) -> (&Entity, &str);
    fn render_button_text(&self, alias: &str) -> String;
    // Выполняет логику при нажатии (QuickAction)
    async fn on_click(
        &self,
        ha: &dyn HomeAssistantClient,
        action: DeviceAction,
    ) -> InteractionResult;
}

pub enum SmartDevice {
    Switch(Entity),
    Light(Entity),
    Climate(Entity),
    Sensor(Entity),
    BinarySensor(Entity),
    Number(Entity),
    Unknown(Entity),
}

impl SmartDevice {
    pub fn new(entity: Entity) -> Self {
        let domain = entity.entity_id.split('.').next().unwrap_or("");
        match domain {
            "switch" => Self::Switch(entity),
            "light" => Self::Light(entity),
            "climate" => Self::Climate(entity),
            "sensor" => Self::Sensor(entity),
            "binary_sensor" => Self::BinarySensor(entity),
            "number" => Self::Number(entity),
            _ => Self::Unknown(entity),
        }
    }
}

#[async_trait::async_trait]
impl SmartEntity for SmartDevice {
    fn get_info(&self) -> (&Entity, &str) {
        match self {
            Self::Light(e) => (e, "light"),
            Self::Switch(e) => (e, "switch"),
            Self::Climate(e) => (e, "climate"),
            Self::Sensor(e) => (e, "sensor"),
            Self::BinarySensor(e) => (e, "binary_sensor"),
            Self::Number(e) => (e, "number"),
            Self::Unknown(e) => {
                let d = e.entity_id.split('.').next().unwrap_or("unknown");
                (e, d)
            }
        }
    }

    fn render_button_text(&self, alias: &str) -> String {
        let (entity, domain) = self.get_info();
        let class = entity.device_class.as_deref().unwrap_or("");

        crate::core::presentation::StateFormatter::format_device_label(
            alias,
            domain,
            class,
            &entity.state,
        )
    }

    async fn on_click(
        &self,
        ha: &dyn HomeAssistantClient,
        action: DeviceAction,
    ) -> InteractionResult {
        let (entity, domain) = self.get_info();
        let entity_id = &entity.entity_id;

        match self {
            Self::Light(_) | Self::Switch(_) => {
                let service = match action {
                    DeviceAction::Toggle => "toggle",
                    DeviceAction::TurnOn => "turn_on",
                    DeviceAction::TurnOff => "turn_off",
                    DeviceAction::SetLevel(v) => {
                        let data = serde_json::json!({ "brightness": v });
                        return if ha
                            .call_service_with_data(domain, "turn_on", entity_id, data)
                            .await
                            .is_ok()
                        {
                            InteractionResult::Processed
                        } else {
                            InteractionResult::Error {
                                error: "Failed to set brightness".into(),
                            }
                        };
                    }
                    _ => "toggle",
                };

                if ha.call_service(domain, service, entity_id).await.is_ok() {
                    InteractionResult::Processed
                } else {
                    InteractionResult::Error {
                        error: "HA Service Call Failed".into(),
                    }
                }
            }

            Self::Climate(_) => match action {
                DeviceAction::SetTemperature(tmp) => {
                    let data = serde_json::json!({ "temperature": tmp });
                    if ha
                        .call_service_with_data("climate", "set_temperature", entity_id, data)
                        .await
                        .is_ok()
                    {
                        InteractionResult::RequiresDetail
                    } else {
                        InteractionResult::Error {
                            error: "Failed to set temperature".into(),
                        }
                    }
                }
                DeviceAction::Toggle => {
                    let _ = ha.call_service("climate", "toggle", entity_id).await;
                    InteractionResult::Processed
                }
                _ => InteractionResult::RequiresDetail,
            },
            Self::Sensor(_) | Self::BinarySensor(_) => match action {
                DeviceAction::GenerateChart(_) => InteractionResult::RequiresDetail,
                DeviceAction::EnterManualInput => {
                    InteractionResult::RequiresInput(InputIntent::DefineGraphInterval {
                        device_id: 0,
                        room_id: 0,
                    })
                }
                _ => InteractionResult::RequiresDetail,
            },

            Self::Number(_) => InteractionResult::RequiresDetail,

            Self::Unknown(e) => {
                let _ = ha.call_service(domain, "toggle", &e.entity_id).await;
                InteractionResult::Processed
            }
        }
    }
}

pub async fn handle_device_interaction(
    config: &Arc<AppConfig>,
    device_id: i64,
    action: DeviceAction,
) -> Result<InteractionResult> {
    let dev_db = crate::db::devices::get_device_by_id(device_id, &config.db)
        .await?
        .context("Device not found in database")?;

    let ha_state = config
        .ha_client
        .fetch_states_by_ids(&[dev_db.entity_id])
        .await?
        .into_iter()
        .next()
        .context("HA state not found")?;

    let smart_obj = SmartDevice::new(ha_state);
    let res = smart_obj.on_click(config.ha_client.as_ref(), action).await;
    if matches!(res, InteractionResult::Processed) {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha::client::HistoryResult;
    use crate::ha::{HomeAssistantClient, Room};
    use crate::models::UserSession;
    use chrono::Utc;
    use dashmap::DashMap;
    use sqlx::SqlitePool;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeHaClient {
        states: Vec<Entity>,
        calls: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    #[async_trait::async_trait]
    impl HomeAssistantClient for FakeHaClient {
        async fn health_check(&self) -> anyhow::Result<()> {
            Ok(())
        }

        async fn fetch_rooms(&self) -> anyhow::Result<Vec<Room>> {
            Ok(vec![])
        }

        async fn fetch_history(
            &self,
            _entity_id: &str,
            _hours: u32,
            _offset: i32,
        ) -> anyhow::Result<HistoryResult> {
            let now = Utc::now();
            Ok(HistoryResult {
                points: vec![(now, "on".to_string())],
                start_time: now,
                end_time: now,
            })
        }

        async fn fetch_states_by_ids(&self, entity_ids: &[String]) -> anyhow::Result<Vec<Entity>> {
            Ok(self
                .states
                .iter()
                .filter(|entity| entity_ids.contains(&entity.entity_id))
                .cloned()
                .collect())
        }

        async fn call_service(
            &self,
            domain: &str,
            service: &str,
            entity_id: &str,
        ) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push((
                domain.to_string(),
                service.to_string(),
                entity_id.to_string(),
            ));
            Ok(())
        }

        async fn call_service_with_data(
            &self,
            domain: &str,
            service: &str,
            entity_id: &str,
            _data: serde_json::Value,
        ) -> anyhow::Result<()> {
            self.call_service(domain, service, entity_id).await
        }
    }

    async fn test_pool() -> anyhow::Result<SqlitePool> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query(
            r#"
            CREATE TABLE devices (
                id INTEGER PRIMARY KEY,
                room_id INTEGER NOT NULL,
                entity_id TEXT NOT NULL UNIQUE,
                alias TEXT,
                device_class TEXT,
                device_domain TEXT,
                archived INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO devices (id, room_id, entity_id, alias, device_class, device_domain, archived)
            VALUES (1, 1, 'switch.test_lamp', 'Test Lamp', 'switch', 'switch', 0)
            "#,
        )
        .execute(&pool)
        .await?;

        Ok(pool)
    }

    #[tokio::test]
    async fn device_toggle_uses_fake_ha_client() -> anyhow::Result<()> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let ha_client = Arc::new(FakeHaClient {
            states: vec![Entity {
                entity_id: "switch.test_lamp".to_string(),
                name: "Test Lamp".to_string(),
                state: "off".to_string(),
                device_class: Some("switch".to_string()),
            }],
            calls: calls.clone(),
        });

        let (recording_tx, _recording_rx) =
            tokio::sync::mpsc::channel::<crate::core::camera_recording::RecordingJob>(1);
        let config = Arc::new(crate::models::AppConfig {
            ha_client,
            ha_url: "http://ha.local".to_string(),
            ha_token: "token".to_string(),
            db: test_pool().await?,
            root_user: 1,
            delete_notification_messages_timeout_s: 5,
            ttl_notifications: 60,
            background_maintenance_interval_s: 15,
            event_refresh_min_interval_s: 5,
            session_ttl_hours: 24,
            telegram_retry_after_extra_delay_s: 1,
            default_language: crate::i18n::Language::Ru,
            camera_default_clip_s: 10,
            camera_clip_intervals_s: vec![5, 10, 15],
            camera_recording_max_tail_seconds: 300,
            camera_recording_max_segment_seconds: 300,
            camera_recording_max_parallel_jobs: 4,
            camera_recording_storage_root: "data/recordings".to_string(),
            camera_recording_tx: recording_tx,
            voice_enabled: false,
            voice_stt_provider: crate::options::VoiceSttProvider::HaPipeline,
            voice_command_engine: crate::options::VoiceCommandEngine::LocalParser,
            voice_ha_pipeline_id: None,
            voice_stt_sample_rate: 16_000,
            voice_confirm_dangerous: true,
            voice_pending_ttl_s: 60,
            voice_max_audio_size_mb: 10,
            voice_max_audio_duration_s: 30,
            voice_stt_timeout_s: 45,
            voice_show_recognized_text: true,
            voice_response_format: crate::options::VoiceResponseFormat::Text,
            sessions: DashMap::<u64, UserSession>::new(),
            ui_locks: DashMap::new(),
            recording_sends_in_progress: DashMap::new(),
            name_aliases: DashMap::new(),
            state_aliases: DashMap::<String, HashMap<String, String>>::new(),
            ui_background_cache: tokio::sync::Mutex::new(None),
            runtime_status: tokio::sync::RwLock::new(crate::models::RuntimeStatus::default()),
        });

        let result = handle_device_interaction(&config, 1, DeviceAction::Toggle).await?;

        assert!(matches!(result, InteractionResult::Processed));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[(
                "switch".to_string(),
                "toggle".to_string(),
                "switch.test_lamp".to_string()
            )]
        );

        Ok(())
    }
}
