use crate::db;
use crate::models::AppConfig;
use anyhow::{anyhow, ensure, Result};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const HA_NATIVE_SYNC_THROTTLE: Duration = Duration::from_secs(30);

static ACTION_TARGET_LOCKS: OnceLock<DashMap<String, Arc<Mutex<()>>>> = OnceLock::new();
static HA_NATIVE_SYNC_LAST_AT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static HA_NATIVE_EMPTY_SYNC_COUNT: OnceLock<AtomicUsize> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub enum ActionActor {
    User(u64),
    Schedule { schedule_id: i64 },
}

impl ActionActor {
    fn user_id(self) -> Option<u64> {
        match self {
            Self::User(user_id) => Some(user_id),
            Self::Schedule { .. } => None,
        }
    }

    fn is_admin(self, config: &AppConfig) -> bool {
        matches!(self, Self::User(user_id) if user_id == config.root_user)
            || matches!(self, Self::Schedule { .. })
    }

    fn label(self) -> String {
        match self {
            Self::User(user_id) => format!("user:{}", user_id),
            Self::Schedule { schedule_id } => format!("schedule:{}", schedule_id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActionTargetError {
    pub entity_id: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct ActionExecutionResult {
    pub requested: usize,
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: Vec<ActionTargetError>,
    pub message: String,
}

impl ActionExecutionResult {
    pub fn status(&self) -> &'static str {
        if self.failed.is_empty() && self.succeeded > 0 {
            "ok"
        } else if self.succeeded > 0 {
            "partial"
        } else {
            "error"
        }
    }

    pub fn user_message(&self) -> String {
        if self.status() == "ok" {
            return self.message.clone();
        }

        let mut text = if self.status() == "partial" {
            format!(
                "⚠️ Выполнено частично: {} / {}.",
                self.succeeded, self.attempted
            )
        } else {
            format!(
                "⚠️ Выполнить не удалось: {} / {}.",
                self.succeeded, self.attempted
            )
        };
        for error in self.failed.iter().take(4) {
            text.push_str(&format!("\n{}: {}", error.entity_id, error.error));
        }
        text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateState {
    AllOn,
    AllOff,
    Mixed,
    Unknown,
}

impl AggregateState {
    pub fn dynamic_command(self) -> db::action_groups::ActionGroupCommand {
        match self {
            Self::AllOn => db::action_groups::ActionGroupCommand::TurnOff,
            Self::AllOff | Self::Mixed | Self::Unknown => {
                db::action_groups::ActionGroupCommand::TurnOn
            }
        }
    }
}

pub async fn sync_ha_native_targets(config: &Arc<AppConfig>) -> Result<()> {
    let sync_lock = HA_NATIVE_SYNC_LAST_AT.get_or_init(|| Mutex::new(None));
    let mut last_at = sync_lock.lock().await;
    if last_at.is_some_and(|instant| instant.elapsed() < HA_NATIVE_SYNC_THROTTLE) {
        return Ok(());
    }

    let entities = config.ha_client.fetch_action_entities().await?;
    let discovered = entities
        .into_iter()
        .filter_map(|entity| {
            let (domain, _) = entity.entity_id.split_once('.')?;
            if !matches!(domain, "script" | "scene") {
                return None;
            }
            Some(db::action_groups::HaNativeDiscovery {
                entity_id: entity.entity_id.clone(),
                domain: domain.to_string(),
                ha_name: if entity.name.trim().is_empty() {
                    entity.entity_id
                } else {
                    entity.name
                },
            })
        })
        .collect::<Vec<_>>();
    let archive_missing_on_empty = if discovered.is_empty() {
        HA_NATIVE_EMPTY_SYNC_COUNT
            .get_or_init(|| AtomicUsize::new(0))
            .fetch_add(1, Ordering::Relaxed)
            + 1
            >= 2
    } else {
        HA_NATIVE_EMPTY_SYNC_COUNT
            .get_or_init(|| AtomicUsize::new(0))
            .store(0, Ordering::Relaxed);
        true
    };
    db::action_groups::sync_ha_native_targets(&discovered, archive_missing_on_empty, &config.db)
        .await?;
    *last_at = Some(Instant::now());
    Ok(())
}

pub async fn aggregate_group_state(
    group_id: i64,
    user_id: u64,
    is_admin: bool,
    config: &Arc<AppConfig>,
) -> Result<AggregateState> {
    let devices = db::action_groups::list_action_group_items(group_id, &config.db).await?;
    let mut entity_ids = Vec::new();
    for device in devices {
        if device.is_archived() || !matches!(device.device_domain.as_str(), "light" | "switch") {
            continue;
        }
        if db::access::can_control_device(user_id, is_admin, device.device_id, &config.db).await? {
            entity_ids.push(device.entity_id);
        }
    }
    if entity_ids.is_empty() {
        return Ok(AggregateState::Unknown);
    }

    let states = config.ha_client.fetch_states_by_ids(&entity_ids).await?;
    Ok(aggregate_states(
        states.iter().map(|entity| entity.state.as_str()),
    ))
}

pub fn aggregate_states<'a>(states: impl IntoIterator<Item = &'a str>) -> AggregateState {
    let mut seen = 0usize;
    let mut on = 0usize;
    let mut off = 0usize;
    for state in states {
        seen += 1;
        match state {
            "on" => on += 1,
            "off" => off += 1,
            _ => {}
        }
    }

    if seen == 0 || (on == 0 && off == 0) {
        AggregateState::Unknown
    } else if on == seen {
        AggregateState::AllOn
    } else if off == seen {
        AggregateState::AllOff
    } else {
        AggregateState::Mixed
    }
}

pub async fn execute_action_group(
    group_id: i64,
    command: db::action_groups::ActionGroupCommand,
    actor: ActionActor,
    config: Arc<AppConfig>,
) -> Result<ActionExecutionResult> {
    let lock = target_lock(format!("bot_group:{}", group_id));
    let _guard = lock
        .try_lock()
        .map_err(|_| anyhow!("Действие уже выполняется"))?;

    let group = db::action_groups::get_action_group(group_id, &config.db)
        .await?
        .ok_or_else(|| anyhow!("Группа не найдена"))?;
    ensure!(group.is_enabled(), "Группа на паузе");

    let is_admin = actor.is_admin(&config);
    if !is_admin {
        ensure!(
            group.is_visible_to_all_users(),
            "Недостаточно прав для этой группы"
        );
    }

    let devices = db::action_groups::list_action_group_items(group_id, &config.db).await?;
    ensure!(!devices.is_empty(), "В группе нет устройств");

    let mut targets = Vec::new();
    for device in devices {
        if device.is_archived() || !matches!(device.device_domain.as_str(), "light" | "switch") {
            continue;
        }
        if let ActionActor::User(user_id) = actor {
            if !db::access::can_control_device(user_id, is_admin, device.device_id, &config.db)
                .await?
            {
                continue;
            }
        }
        targets.push(device);
    }
    ensure!(!targets.is_empty(), "Нет доступных устройств");

    let service = match command {
        db::action_groups::ActionGroupCommand::Toggle => {
            let entity_ids = targets
                .iter()
                .map(|target| target.entity_id.clone())
                .collect::<Vec<_>>();
            let states = config.ha_client.fetch_states_by_ids(&entity_ids).await?;
            aggregate_states(states.iter().map(|entity| entity.state.as_str()))
                .dynamic_command()
                .as_service()
                .unwrap_or(db::action_groups::COMMAND_TURN_ON)
        }
        db::action_groups::ActionGroupCommand::TurnOn => db::action_groups::COMMAND_TURN_ON,
        db::action_groups::ActionGroupCommand::TurnOff => db::action_groups::COMMAND_TURN_OFF,
    };

    let mut succeeded = 0usize;
    let mut failed = Vec::new();
    for target in &targets {
        match config
            .ha_client
            .call_service(&target.device_domain, service, &target.entity_id)
            .await
        {
            Ok(()) => succeeded += 1,
            Err(error) => failed.push(ActionTargetError {
                entity_id: target.entity_id.clone(),
                error: crate::db::sanitize_error(&error.to_string()),
            }),
        }
    }

    let result = ActionExecutionResult {
        requested: group.items_count as usize,
        attempted: targets.len(),
        succeeded,
        failed,
        message: format!(
            "Команда отправлена: {} · {} устройств",
            group.name, succeeded
        ),
    };
    log_action_group_execution(&config, actor, group.id, service, &result).await;
    Ok(result)
}

pub async fn execute_ha_native_target(
    target_id: i64,
    actor: ActionActor,
    config: Arc<AppConfig>,
) -> Result<ActionExecutionResult> {
    let lock = target_lock(format!("ha_native:{}", target_id));
    let _guard = lock
        .try_lock()
        .map_err(|_| anyhow!("Действие уже выполняется"))?;

    let target = db::action_groups::get_ha_native_target(target_id, &config.db)
        .await?
        .ok_or_else(|| anyhow!("Действие не найдено"))?;
    ensure!(target.is_enabled(), "Действие на паузе");
    ensure!(
        !target.is_archived(),
        "Действие не найдено в Home Assistant"
    );
    ensure!(
        matches!(target.domain.as_str(), "script" | "scene"),
        "Неподдерживаемый тип"
    );

    let is_admin = actor.is_admin(&config);
    if !is_admin {
        ensure!(
            target.is_visible_to_all_users(),
            "Недостаточно прав для этого действия"
        );
    }

    let service = "turn_on";
    let mut result = ActionExecutionResult {
        requested: 1,
        attempted: 1,
        succeeded: 0,
        failed: Vec::new(),
        message: format!("Запущено: {}", target.display_name()),
    };
    if let Err(error) = config
        .ha_client
        .call_service(&target.domain, service, &target.entity_id)
        .await
    {
        result.failed.push(ActionTargetError {
            entity_id: target.entity_id.clone(),
            error: crate::db::sanitize_error(&error.to_string()),
        });
    } else {
        result.succeeded = 1;
    }

    log_ha_native_execution(&config, actor, target.id, &result).await;
    Ok(result)
}

pub async fn execute_action_target(
    target: db::action_groups::ActionTargetRef,
    command: db::action_groups::ActionScheduleCommand,
    actor: ActionActor,
    config: Arc<AppConfig>,
) -> Result<ActionExecutionResult> {
    match (target, command) {
        (
            db::action_groups::ActionTargetRef::BotGroup(group_id),
            db::action_groups::ActionScheduleCommand::TurnOn,
        ) => {
            execute_action_group(
                group_id,
                db::action_groups::ActionGroupCommand::TurnOn,
                actor,
                config,
            )
            .await
        }
        (
            db::action_groups::ActionTargetRef::BotGroup(group_id),
            db::action_groups::ActionScheduleCommand::TurnOff,
        ) => {
            execute_action_group(
                group_id,
                db::action_groups::ActionGroupCommand::TurnOff,
                actor,
                config,
            )
            .await
        }
        (
            db::action_groups::ActionTargetRef::HaNative(target_id),
            db::action_groups::ActionScheduleCommand::Execute,
        ) => execute_ha_native_target(target_id, actor, config).await,
        _ => Err(anyhow!("Недопустимая команда расписания")),
    }
}

fn target_lock(key: String) -> Arc<Mutex<()>> {
    ACTION_TARGET_LOCKS
        .get_or_init(DashMap::new)
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

async fn log_action_group_execution(
    config: &Arc<AppConfig>,
    actor: ActionActor,
    group_id: i64,
    service: &str,
    result: &ActionExecutionResult,
) {
    let entity_id = group_id.to_string();
    let action = match service {
        db::action_groups::COMMAND_TURN_ON => "action_group.execute_turn_on",
        db::action_groups::COMMAND_TURN_OFF => "action_group.execute_turn_off",
        _ => "action_group.execute",
    };
    let message = format!(
        "{}; actor={}; requested={}, attempted={}, succeeded={}, failed={}",
        result.message,
        actor.label(),
        result.requested,
        result.attempted,
        result.succeeded,
        result.failed.len()
    );
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: actor.user_id(),
            kind: "action_group",
            entity_type: "action_group",
            entity_id: Some(&entity_id),
            action,
            status: result.status(),
            message: Some(&message),
        },
        &config.db,
    )
    .await;
}

async fn log_ha_native_execution(
    config: &Arc<AppConfig>,
    actor: ActionActor,
    target_id: i64,
    result: &ActionExecutionResult,
) {
    let entity_id = target_id.to_string();
    let message = format!(
        "{}; actor={}; attempted={}, succeeded={}, failed={}",
        result.message,
        actor.label(),
        result.attempted,
        result.succeeded,
        result.failed.len()
    );
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: actor.user_id(),
            kind: "ha_native_target",
            entity_type: "ha_native_target",
            entity_id: Some(&entity_id),
            action: "ha_native_target.execute",
            status: result.status(),
            message: Some(&message),
        },
        &config.db,
    )
    .await;
}

pub fn state_map_by_entity(states: Vec<crate::ha::models::Entity>) -> HashMap<String, String> {
    states
        .into_iter()
        .map(|entity| (entity.entity_id, entity.state))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha::client::HistoryResult;
    use crate::ha::models::Entity;
    use crate::ha::{HomeAssistantClient, Room};
    use crate::models::{AppConfig, RuntimeStatus};
    use crate::options::{VoiceCommandEngine, VoiceResponseFormat, VoiceSttProvider};
    use chrono::Utc;
    use dashmap::DashMap;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Default)]
    struct FakeHaClient {
        calls: Arc<StdMutex<Vec<(String, String, String)>>>,
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
            Ok(entity_ids
                .iter()
                .map(|entity_id| Entity {
                    entity_id: entity_id.clone(),
                    name: entity_id.clone(),
                    state: "off".to_string(),
                    device_class: None,
                })
                .collect())
        }

        async fn fetch_action_entities(&self) -> anyhow::Result<Vec<Entity>> {
            Ok(vec![])
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

    #[test]
    fn aggregate_states_drives_dynamic_toggle() {
        assert_eq!(aggregate_states(["on", "on"]), AggregateState::AllOn);
        assert_eq!(
            aggregate_states(["on", "on"]).dynamic_command(),
            db::action_groups::ActionGroupCommand::TurnOff
        );
        assert_eq!(aggregate_states(["on", "off"]), AggregateState::Mixed);
        assert_eq!(
            aggregate_states(["on", "off"]).dynamic_command(),
            db::action_groups::ActionGroupCommand::TurnOn
        );
        assert_eq!(aggregate_states(["unknown"]), AggregateState::Unknown);
    }

    #[tokio::test]
    async fn ha_native_script_calls_script_turn_on() -> anyhow::Result<()> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        sqlx::query(
            r#"
            CREATE TABLE ha_native_targets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id TEXT NOT NULL UNIQUE,
                domain TEXT NOT NULL,
                ha_name TEXT NOT NULL,
                display_name TEXT,
                access_scope TEXT NOT NULL DEFAULT 'admin_only',
                enabled INTEGER NOT NULL DEFAULT 1,
                archived INTEGER NOT NULL DEFAULT 0,
                last_seen_at TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE action_schedules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_type TEXT NOT NULL,
                target_id INTEGER NOT NULL,
                command TEXT NOT NULL,
                time_minute INTEGER NOT NULL,
                days_mask INTEGER NOT NULL DEFAULT 127,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_run_at TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE activity_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER,
                kind TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                entity_id TEXT,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                message TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO ha_native_targets (id, entity_id, domain, ha_name, access_scope) VALUES (1, 'script.good_night', 'script', 'Good night', 'all_users')",
        )
        .execute(&pool)
        .await?;

        let calls = Arc::new(StdMutex::new(Vec::new()));
        let config = test_config(pool, calls.clone());
        let result = execute_ha_native_target(1, ActionActor::User(42), config.clone()).await?;

        assert_eq!(result.status(), "ok");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[(
                "script".to_string(),
                "turn_on".to_string(),
                "script.good_night".to_string()
            )]
        );
        Ok(())
    }

    fn test_config(
        db: sqlx::SqlitePool,
        calls: Arc<StdMutex<Vec<(String, String, String)>>>,
    ) -> Arc<AppConfig> {
        let (recording_tx, _recording_rx) =
            tokio::sync::mpsc::channel::<crate::core::camera_recording::RecordingJob>(1);
        Arc::new(AppConfig {
            ha_client: Arc::new(FakeHaClient { calls }),
            ha_url: "http://ha".to_string(),
            ha_token: "token".to_string(),
            db,
            root_user: 1,
            delete_notification_messages_timeout_s: 5,
            ttl_notifications: 60,
            background_maintenance_interval_s: 60,
            event_refresh_min_interval_s: 1,
            session_ttl_hours: 24,
            telegram_retry_after_extra_delay_s: 1,
            default_language: crate::i18n::Language::Ru,
            camera_default_clip_s: 10,
            camera_clip_intervals_s: vec![10],
            camera_recording_max_tail_seconds: 300,
            camera_recording_max_segment_seconds: 300,
            camera_recording_max_parallel_jobs: 4,
            camera_recording_storage_root: "data/recordings".to_string(),
            camera_recording_tx: recording_tx,
            voice_enabled: false,
            voice_stt_provider: VoiceSttProvider::HaPipeline,
            voice_command_engine: VoiceCommandEngine::LocalParser,
            voice_ha_pipeline_id: None,
            voice_stt_sample_rate: 16_000,
            voice_confirm_dangerous: true,
            voice_pending_ttl_s: 60,
            voice_max_audio_size_mb: 10,
            voice_max_audio_duration_s: 30,
            voice_stt_timeout_s: 45,
            voice_show_recognized_text: false,
            voice_response_format: VoiceResponseFormat::Text,
            sessions: DashMap::new(),
            ui_locks: DashMap::new(),
            recording_sends_in_progress: DashMap::new(),
            name_aliases: DashMap::new(),
            state_aliases: DashMap::new(),
            ui_background_cache: tokio::sync::Mutex::new(None),
            camera_snapshot_cache: tokio::sync::Mutex::new(HashMap::new()),
            runtime_status: tokio::sync::RwLock::new(RuntimeStatus::default()),
        })
    }
}
