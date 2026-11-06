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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DeviceAction {
    Toggle,
    TurnOn,
    TurnOff,
    SetLevel(u8),
    SetTemperature(f32),
    AdjustTemperature(i8),
    GenerateChart(ChartParams),
    EnterManualInput,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChartParams {
    pub period_hours: u32,
    pub offset_hours: i32, // 0 - текущее время, -24 - вчера и т.д.
}

/// An active target captured when the user reviews a scoped command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceTargetSnapshot {
    pub device_id: i64,
    pub entity_id: String,
    pub room_id: i64,
}

impl DeviceTargetSnapshot {
    pub async fn is_current(&self, pool: &sqlx::SqlitePool) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM devices WHERE id = ? AND entity_id = ? AND room_id = ? AND archived = 0)",
        )
        .bind(self.device_id)
        .bind(&self.entity_id)
        .bind(self.room_id)
        .fetch_one(pool)
        .await?)
    }
}

#[derive(Debug)]
pub struct DeviceTargetChanged;

impl std::fmt::Display for DeviceTargetChanged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Device target changed; command was not sent")
    }
}

impl std::error::Error for DeviceTargetChanged {}

/// Scope reviewed by the user. Only a successfully claimed server operation may
/// pass this scope to dispatch; possessing an unclaimed proposal is insufficient.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct CriticalConsent {
    device_id: i64,
    entity_id: String,
    action: DeviceAction,
}

impl CriticalConsent {
    pub(crate) fn new(device_id: i64, entity_id: String, action: DeviceAction) -> Self {
        Self {
            device_id,
            entity_id,
            action,
        }
    }

    pub(crate) fn matches(&self, device_id: i64, entity_id: &str, action: &DeviceAction) -> bool {
        self.device_id == device_id && self.entity_id == entity_id && &self.action == action
    }

    pub(crate) fn targets_device(&self, device_id: i64) -> bool {
        self.device_id == device_id
    }
}

#[derive(Debug)]
pub(crate) struct CriticalConsentRequired;

impl std::fmt::Display for CriticalConsentRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Critical device requires a new confirmation / Критичное устройство требует нового подтверждения")
    }
}

impl std::error::Error for CriticalConsentRequired {}

pub(crate) async fn validate_critical_consent(
    pool: &sqlx::SqlitePool,
    device_id: i64,
    entity_id: &str,
    action: &DeviceAction,
    consent: Option<&CriticalConsent>,
) -> Result<()> {
    let critical: Option<bool> = sqlx::query_scalar(
        "SELECT COALESCE(critical, 0) FROM devices WHERE id = ? AND entity_id = ? AND archived = 0",
    )
    .bind(device_id)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?;
    let critical = critical.ok_or(DeviceTargetChanged)?;
    let matches = consent.is_some_and(|scope| scope.matches(device_id, entity_id, action));
    anyhow::ensure!(
        matches || (!critical && consent.is_none()),
        CriticalConsentRequired
    );
    Ok(())
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
                    _ => return InteractionResult::RequiresDetail,
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
                DeviceAction::AdjustTemperature(direction) => {
                    if !matches!(direction, -1 | 1) {
                        return InteractionResult::Error {
                            error: "Invalid temperature step".into(),
                        };
                    }
                    let Some(current) = entity.attributes.temperature else {
                        return InteractionResult::Error {
                            error: "Temperature unavailable".into(),
                        };
                    };
                    let step = entity
                        .attributes
                        .target_temp_step
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .unwrap_or(0.5);
                    let target =
                        ((current + f32::from(direction) * step) * 1000.0).round() / 1000.0;
                    self.on_click(ha, DeviceAction::SetTemperature(target))
                        .await
                }
                DeviceAction::SetTemperature(tmp) => {
                    if !entity.temperature_target_is_valid(tmp) {
                        return InteractionResult::Error {
                            error: "Temperature target is unavailable or outside device limits"
                                .into(),
                        };
                    }
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
                    match ha.call_service("climate", "toggle", entity_id).await {
                        Ok(()) => InteractionResult::Processed,
                        Err(error) => InteractionResult::Error {
                            error: error.to_string(),
                        },
                    }
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
                if !matches!(action, DeviceAction::Toggle) {
                    return InteractionResult::RequiresDetail;
                }
                match ha.call_service(domain, "toggle", &e.entity_id).await {
                    Ok(()) => InteractionResult::Processed,
                    Err(error) => InteractionResult::Error {
                        error: error.to_string(),
                    },
                }
            }
        }
    }
}

pub async fn handle_device_interaction(
    config: &Arc<AppConfig>,
    user_id: u64,
    admission_generation: &str,
    device_id: i64,
    action: DeviceAction,
    expected_target: Option<&DeviceTargetSnapshot>,
) -> Result<InteractionResult> {
    handle_device_interaction_with_consent(
        config,
        user_id,
        admission_generation,
        device_id,
        action,
        expected_target,
        None,
    )
    .await
}

pub(crate) async fn handle_device_interaction_with_consent(
    config: &Arc<AppConfig>,
    user_id: u64,
    admission_generation: &str,
    device_id: i64,
    action: DeviceAction,
    expected_target: Option<&DeviceTargetSnapshot>,
    consent: Option<&CriticalConsent>,
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

    anyhow::ensure!(
        config
            .runtime_status
            .read()
            .await
            .shutdown_requested_at
            .is_none(),
        "Application is stopping; command was not sent"
    );
    // HA reads may outlive removal/re-admission or an ACL change. Keep the
    // caller's original admission and validate after the last external read.
    let access = crate::db::access::get_effective_device_access(
        user_id,
        user_id == config.root_user,
        device_id,
        &config.db,
    )
    .await?;
    let needs_control = !matches!(
        action,
        DeviceAction::GenerateChart(_) | DeviceAction::EnterManualInput
    );
    anyhow::ensure!(
        access.can_view && (!needs_control || access.can_control),
        "Device access revoked; command was not sent"
    );
    anyhow::ensure!(
        crate::db::user::admission_is_current(user_id, admission_generation, &config.db).await?,
        "User admission revoked; command was not sent"
    );
    if let Some(expected) = expected_target {
        // The DB target can move, be archived or be replaced while HA is slow.
        // Validate the reviewed scope and the actual HA target at dispatch,
        // including for root, whose ACL otherwise permits every device.
        anyhow::ensure!(
            expected.device_id == device_id
                && ha_state.entity_id == expected.entity_id
                && expected.is_current(&config.db).await?,
            DeviceTargetChanged
        );
    }
    if needs_control {
        validate_critical_consent(&config.db, device_id, &ha_state.entity_id, &action, consent)
            .await?;
    }
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
        response_entity_id: Option<String>,
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
                .map(|mut entity| {
                    if let Some(id) = &self.response_entity_id {
                        entity.entity_id = id.clone();
                    }
                    entity
                })
                .collect())
        }

        async fn fetch_action_entities(&self) -> anyhow::Result<Vec<Entity>> {
            Ok(self
                .states
                .iter()
                .filter(|entity| {
                    entity.entity_id.starts_with("script.")
                        || entity.entity_id.starts_with("scene.")
                })
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
        sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
            .await?
            .run(&pool)
            .await?;
        crate::db::add_user(1, &pool).await?;
        crate::db::rooms::sync_rooms_from_ha("test", "Test", &pool).await?;

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
                attributes: Default::default(),
            }],
            calls: calls.clone(),
            response_entity_id: None,
        });

        let (recording_tx, _recording_rx) =
            tokio::sync::mpsc::channel::<crate::core::camera_recording::RecordingJob>(1);
        let config = Arc::new(crate::models::AppConfig {
            html_ui: Default::default(),
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
            admission_lifecycle: tokio::sync::Mutex::new(()),
            dialogue_generations: DashMap::new(),
            active_dialogues: DashMap::new(),
            ui_locks: DashMap::new(),
            view_updates: DashMap::new(),
            climate_controls: Default::default(),
            recording_sends_in_progress: DashMap::new(),
            name_aliases: DashMap::new(),
            state_aliases: DashMap::<String, HashMap<String, String>>::new(),
            ui_background_cache: tokio::sync::Mutex::new(None),
            camera_snapshot_cache: tokio::sync::Mutex::new(HashMap::new()),
            camera_pre_roll_registry: Arc::new(
                crate::core::camera_pre_roll::CameraPreRollRegistry::new(),
            ),
            runtime_status: tokio::sync::RwLock::new(crate::models::RuntimeStatus::default()),
            notification_delivery: Default::default(),
        });

        let generation = crate::db::user::admission_generation(1, &config.db)
            .await?
            .context("Test user admission missing")?;
        let result =
            handle_device_interaction(&config, 1, &generation, 1, DeviceAction::Toggle, None)
                .await?;

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

    #[tokio::test]
    async fn scoped_device_rejects_ha_response_for_an_unreviewed_entity() -> Result<()> {
        let mut config = crate::test_support::config().await;
        crate::db::add_user(1, &config.db).await?;
        crate::db::rooms::sync_rooms_from_ha("test", "Test", &config.db).await?;
        crate::db::devices::sync_device("light.reviewed", "test", "Reviewed", "light", &config.db)
            .await?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        Arc::get_mut(&mut config).unwrap().ha_client = Arc::new(FakeHaClient {
            states: vec![Entity {
                entity_id: "light.reviewed".into(),
                name: "Reviewed".into(),
                state: "on".into(),
                device_class: Some("light".into()),
                attributes: Default::default(),
            }],
            calls: calls.clone(),
            response_entity_id: Some("light.unreviewed".into()),
        });
        let snapshot = DeviceTargetSnapshot {
            device_id: 1,
            entity_id: "light.reviewed".into(),
            room_id: 1,
        };
        assert!(snapshot.is_current(&config.db).await?);
        let generation = crate::db::admission_generation(1, &config.db)
            .await?
            .unwrap();
        let error = handle_device_interaction(
            &config,
            1,
            &generation,
            1,
            DeviceAction::TurnOff,
            Some(&snapshot),
        )
        .await
        .unwrap_err();
        assert!(error.is::<DeviceTargetChanged>());
        assert!(calls.lock().unwrap().is_empty());
        Ok(())
    }
}
