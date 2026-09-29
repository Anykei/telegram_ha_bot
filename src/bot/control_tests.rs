use super::*;
use crate::ha::client::HistoryResult;
use crate::ha::models::{Entity, EntityAttributes};
use crate::ha::{HomeAssistantClient, Room};
use chrono::{Duration, Utc};
use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use teloxide::types::InlineKeyboardButtonKind;

#[derive(Default)]
struct FakeHa {
    states: Mutex<Vec<Entity>>,
    calls: Mutex<Vec<(String, String, serde_json::Value)>>,
    fail_read: AtomicBool,
    empty_history: AtomicBool,
    fail_write: AtomicBool,
    stale_temperature: AtomicBool,
    pause_next_write: AtomicBool,
    write_started: tokio::sync::Notify,
    release_write: tokio::sync::Notify,
    state_reads: AtomicUsize,
    history_requests: Mutex<Vec<(u32, i32)>>,
}

#[async_trait::async_trait]
impl HomeAssistantClient for FakeHa {
    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn fetch_rooms(&self) -> anyhow::Result<Vec<Room>> {
        Ok(vec![])
    }
    async fn fetch_action_entities(&self) -> anyhow::Result<Vec<Entity>> {
        Ok(vec![])
    }
    async fn fetch_states_by_ids(&self, ids: &[String]) -> anyhow::Result<Vec<Entity>> {
        self.state_reads.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(!self.fail_read.load(Ordering::SeqCst), "HA offline");
        Ok(self
            .states
            .lock()
            .unwrap()
            .iter()
            .filter(|e| ids.contains(&e.entity_id))
            .cloned()
            .collect())
    }
    async fn fetch_history(
        &self,
        _: &str,
        hours: u32,
        offset: i32,
    ) -> anyhow::Result<HistoryResult> {
        self.history_requests.lock().unwrap().push((hours, offset));
        anyhow::ensure!(!self.fail_read.load(Ordering::SeqCst), "HA offline");
        let end_time = Utc::now() + Duration::hours(i64::from(offset));
        let start_time = end_time - Duration::hours(i64::from(hours));
        Ok(HistoryResult {
            start_time,
            end_time,
            points: if self.empty_history.load(Ordering::SeqCst) {
                vec![]
            } else {
                vec![(start_time, "20".into()), (end_time, "21".into())]
            },
        })
    }
    async fn call_service(&self, domain: &str, service: &str, id: &str) -> anyhow::Result<()> {
        self.call_service_with_data(domain, service, id, serde_json::json!({}))
            .await
    }
    async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        id: &str,
        data: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((format!("{domain}.{service}"), id.into(), data.clone()));
        if self.pause_next_write.swap(false, Ordering::SeqCst) {
            self.write_started.notify_one();
            self.release_write.notified().await;
        }
        anyhow::ensure!(!self.fail_write.load(Ordering::SeqCst), "HA service failed");
        if self.stale_temperature.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(target) = data["temperature"].as_f64() {
            let mut states = self.states.lock().unwrap();
            states
                .iter_mut()
                .find(|e| e.entity_id == id)
                .unwrap()
                .attributes
                .temperature = Some(target as f32);
        }
        Ok(())
    }
}

fn entity(id: &str, state: &str) -> Entity {
    Entity {
        entity_id: id.into(),
        name: id.into(),
        state: state.into(),
        device_class: None,
        attributes: Default::default(),
    }
}

async fn setup() -> anyhow::Result<(Arc<AppConfig>, Arc<FakeHa>)> {
    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
        .await?
        .run(&db)
        .await?;
    db::add_user(42, &db).await?;
    db::rooms::sync_rooms_from_ha("bedroom", "Спальня", &db).await?;
    for (id, name, class) in [
        ("climate.bedroom", "Термостат", "climate"),
        ("sensor.temperature", "Температура", "temperature"),
        ("light.bedroom", "Люстра", "light"),
    ] {
        db::devices::sync_device(id, "bedroom", name, class, &db).await?;
        db::subscriptions::toggle_hidden(id, &db).await?;
    }
    let mut climate = entity("climate.bedroom", "heat");
    climate.attributes = EntityAttributes {
        current_temperature: Some(20.0),
        temperature: Some(22.0),
        target_temp_step: Some(0.5),
        min_temp: Some(16.0),
        max_temp: Some(30.0),
        ..Default::default()
    };
    let ha = Arc::new(FakeHa {
        states: Mutex::new(vec![
            climate,
            entity("sensor.temperature", "20"),
            entity("light.bedroom", "on"),
        ]),
        ..Default::default()
    });
    let (camera_recording_tx, _) = tokio::sync::mpsc::channel(1);
    let config = Arc::new(AppConfig {
        db,
        ha_client: ha.clone(),
        ha_url: "http://unused.invalid".into(),
        ha_token: "unused".into(),
        root_user: 1,
        delete_notification_messages_timeout_s: 0,
        ttl_notifications: 60,
        background_maintenance_interval_s: 15,
        event_refresh_min_interval_s: 5,
        session_ttl_hours: 24,
        telegram_retry_after_extra_delay_s: 1,
        default_language: Language::Ru,
        camera_default_clip_s: 10,
        camera_clip_intervals_s: vec![10],
        camera_recording_max_tail_seconds: 300,
        camera_recording_max_segment_seconds: 300,
        camera_recording_max_parallel_jobs: 4,
        camera_recording_storage_root: "unused".into(),
        camera_recording_tx,
        voice_enabled: false,
        voice_stt_provider: Default::default(),
        voice_command_engine: Default::default(),
        voice_ha_pipeline_id: None,
        voice_stt_sample_rate: 16000,
        voice_confirm_dangerous: true,
        voice_pending_ttl_s: 60,
        voice_max_audio_size_mb: 10,
        voice_max_audio_duration_s: 30,
        voice_stt_timeout_s: 45,
        voice_show_recognized_text: false,
        voice_response_format: Default::default(),
        sessions: DashMap::new(),
        ui_locks: DashMap::new(),
        view_updates: DashMap::new(),
        climate_controls: Default::default(),
        recording_sends_in_progress: DashMap::new(),
        name_aliases: DashMap::new(),
        state_aliases: DashMap::new(),
        ui_background_cache: tokio::sync::Mutex::new(None),
        camera_snapshot_cache: tokio::sync::Mutex::new(Default::default()),
        camera_pre_roll_registry: Arc::new(
            crate::core::camera_pre_roll::CameraPreRollRegistry::new(),
        ),
        runtime_status: tokio::sync::RwLock::new(Default::default()),
    });
    Ok((config, ha))
}

fn callbacks(view: &View) -> Vec<Payload> {
    view.kb
        .inline_keyboard
        .iter()
        .flatten()
        .filter_map(|button| match &button.kind {
            InlineKeyboardButtonKind::CallbackData(data) => {
                Some(Payload::from_string(data).unwrap())
            }
            _ => None,
        })
        .collect()
}

async fn open(config: &Arc<AppConfig>, payload: ControlPayload) -> anyhow::Result<View> {
    router(Payload::Control(payload), 42, config.clone()).await
}

fn confirmation_id(view: &View) -> i64 {
    match view.payload {
        Payload::Control(ControlPayload::ReviewAction { id }) => id,
        _ => panic!("Expected confirmation: {:?}", view),
    }
}

#[tokio::test]
async fn critical_light_confirmation_freezes_toggle_and_is_consumed_once() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let view = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::Toggle,
        },
    )
    .await?;
    let id = confirmation_id(&view);
    assert!(view.text.contains("Выключить"));
    assert!(view.text.contains("Люстра"));
    router(view.payload, 42, config.clone()).await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    ha.states.lock().unwrap()[2].state = "off".into();
    let (first, duplicate) = tokio::join!(
        open(&config, ControlPayload::ConfirmAction { id }),
        open(&config, ControlPayload::ConfirmAction { id }),
    );
    first?;
    duplicate?;
    let done = open(&config, ControlPayload::ReviewAction { id }).await?;
    assert!(done.text.contains("Команда отправлена"));
    assert!(!callbacks(&done).contains(&Payload::Control(ControlPayload::ConfirmAction { id })));
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    let calls = ha.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "light.turn_off");
    Ok(())
}

#[tokio::test]
async fn critical_confirmation_checks_owner_cancel_expiry_and_revoked_access() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let command = ControlPayload::QuickAction {
        room: 1,
        device: 3,
        cmd: DeviceCmd::TurnOff,
    };
    let id = confirmation_id(&open(&config, command.clone()).await?);
    let stranger = router(
        Payload::Control(ControlPayload::ConfirmAction { id }),
        77,
        config.clone(),
    )
    .await?;
    assert!(!stranger.text.contains("Люстра"));
    open(&config, ControlPayload::CancelAction { id }).await?;
    let cancelled = open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert!(cancelled.text.contains("Отменено"));
    let expired = confirmation_id(&open(&config, command.clone()).await?);
    sqlx::query("UPDATE ui_confirmations SET expires_at = 0 WHERE id = ?")
        .bind(expired)
        .execute(&config.db)
        .await?;
    let view = open(&config, ControlPayload::ConfirmAction { id: expired }).await?;
    assert!(view.text.contains("устарело"));
    let revoked = confirmation_id(&open(&config, command).await?);
    db::access::set_device_access(42, "light.bedroom", true, false, true, &config.db).await?;
    open(&config, ControlPayload::ConfirmAction { id: revoked }).await?;
    db::access::set_device_access(42, "light.bedroom", true, true, true, &config.db).await?;
    open(&config, ControlPayload::ConfirmAction { id: revoked }).await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn critical_temperature_approves_exact_target_and_rechecks_limits() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(1, &config.db).await?;
    let command = ControlPayload::QuickAction {
        room: 1,
        device: 1,
        cmd: DeviceCmd::AdjustTemp(1),
    };
    let view = open(&config, command.clone()).await?;
    let id = confirmation_id(&view);
    assert!(view.text.contains("22.5"));
    assert!(ha.calls.lock().unwrap().is_empty());
    let bypass = config
        .climate_controls
        .enqueue(
            config.clone(),
            42,
            1,
            crate::core::climate_control::TemperatureCommand::Step(1),
        )?
        .await?;
    assert!(bypass.is_err());
    ha.states.lock().unwrap()[0].attributes.temperature = Some(25.0);
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(ha.calls.lock().unwrap()[0].2["temperature"], 22.5);
    let id = confirmation_id(&open(&config, command).await?);
    ha.states.lock().unwrap()[0].attributes.max_temp = Some(22.5);
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn uncertain_confirmed_command_is_not_retried() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let id = confirmation_id(
        &open(
            &config,
            ControlPayload::QuickAction {
                room: 1,
                device: 3,
                cmd: DeviceCmd::TurnOff,
            },
        )
        .await?,
    );
    ha.fail_write.store(true, Ordering::SeqCst);
    let failed = open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert!(failed.text.contains("Не удалось"));
    ha.fail_write.store(false, Ordering::SeqCst);
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    router(failed.payload, 42, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn critical_group_confirmation_rejects_changed_membership_and_covers_admin_buttons(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::add_user(config.root_user, &config.db).await?;
    let group = db::action_groups::create_action_group("Весь свет", &config.db).await?;
    db::action_groups::toggle_action_group_item(group, 3, &config.db).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let action = Payload::Admin(AdminPayload::ExecuteActionGroup {
        group,
        command: db::action_groups::ActionGroupCommand::TurnOff,
    });
    let id = confirmation_id(&router(action.clone(), config.root_user, config.clone()).await?);
    db::devices::sync_device("switch.desk", "bedroom", "Стол", "switch", &config.db).await?;
    ha.states.lock().unwrap().push(entity("switch.desk", "on"));
    db::action_groups::toggle_action_group_item(group, 4, &config.db).await?;
    router(
        Payload::Control(ControlPayload::ConfirmAction { id }),
        config.root_user,
        config.clone(),
    )
    .await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    let id = confirmation_id(&router(action, config.root_user, config.clone()).await?);
    let done = router(
        Payload::Control(ControlPayload::ConfirmAction { id }),
        config.root_user,
        config.clone(),
    )
    .await?;
    router(done.payload, config.root_user, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    assert!(ha
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| call.0.ends_with("turn_off")));
    Ok(())
}

#[tokio::test]
async fn room_labels_distinguish_devices_and_readonly_buttons_open_details() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::sync_device(
        "sensor.battery",
        "bedroom",
        "Термостат",
        "battery",
        &config.db,
    )
    .await?;
    db::devices::sync_device("light.desk", "bedroom", "Люстра", "light", &config.db).await?;
    for id in ["sensor.battery", "light.desk"] {
        db::subscriptions::toggle_hidden(id, &config.db).await?;
    }
    let mut battery = entity("sensor.battery", "97.00");
    battery.device_class = Some("battery".into());
    {
        let mut states = ha.states.lock().unwrap();
        states[0].state = "auto".into();
        states.push(battery);
        states.push(entity("light.desk", "on"));
    }
    let room = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    let labels = room
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        labels.contains("Выключить: Люстра · light.bedroom"),
        "{labels}"
    );
    assert!(
        labels.contains("Выключить: Люстра · light.desk"),
        "{labels}"
    );
    assert!(
        labels.contains("История: Термостат · Батарея · 97%"),
        "{labels}"
    );
    assert!(labels.contains("автоматический"));
    assert!(!labels.contains("97.00"));
    db::access::set_device_access(42, "light.bedroom", true, false, true, &config.db).await?;
    let room = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    assert!(
        callbacks(&room).contains(&Payload::Control(ControlPayload::DeviceControl {
            room: 1,
            device: 3
        }))
    );
    assert!(!callbacks(&room).iter().any(|p| matches!(
        p,
        Payload::Control(ControlPayload::QuickAction { device: 3, .. })
    )));
    let detail = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 3 },
    )
    .await?;
    assert!(detail.text.contains(t(Language::Ru, "control.view_only")));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn climate_opens_from_room_and_refresh_never_sends_commands() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let room = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    let payload = callbacks(&room)
        .into_iter()
        .find(|p| {
            matches!(
                p,
                Payload::Control(ControlPayload::DeviceControl { device: 1, .. })
            )
        })
        .unwrap_or_else(|| panic!("Missing climate button: {room:?}"));
    let view = router(payload.clone(), 42, config.clone()).await?;
    assert_eq!(view.payload, payload);
    assert!(view.text.contains("Термостат"));
    assert!(view.text.contains("Сейчас: 20°"));
    assert!(view.text.contains("Задано: 22°"));
    assert!(view.text.contains("обогрев"));
    for _ in 0..3 {
        router(view.payload.clone(), 42, config.clone()).await?;
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    let new_target = callbacks(&view)
        .into_iter()
        .find(|p| {
            matches!(
                p,
                Payload::Control(ControlPayload::QuickAction {
                    cmd: DeviceCmd::AdjustTemp(1),
                    ..
                })
            )
        })
        .unwrap();
    let updated = router(new_target, 42, config.clone()).await?;
    assert!(updated.text.contains("Задано: 22.5°"));
    assert_eq!(updated.payload, payload);
    let calls = ha.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "climate.set_temperature");
    assert_eq!(calls[0].2["temperature"], 22.5);
    Ok(())
}

#[tokio::test]
async fn view_only_user_can_open_sensor_graph_and_climate_but_cannot_change_temperature(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::access::set_room_access(42, 1, true, false, &config.db).await?;
    let room = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    let sensor = callbacks(&room)
        .into_iter()
        .find(|p| {
            matches!(
                p,
                Payload::Control(ControlPayload::DeviceControl { device: 2, .. })
            )
        })
        .unwrap_or_else(|| panic!("Missing sensor button: {room:?}"));
    let graph = router(sensor, 42, config.clone()).await?;
    assert!(graph.alert.is_none(), "{:?}", graph.alert);
    assert!(graph.image.is_some());
    assert!(matches!(
        graph.payload,
        Payload::Control(ControlPayload::QuickAction {
            cmd: DeviceCmd::ShowChart { .. },
            ..
        })
    ));
    let climate = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 1 },
    )
    .await?;
    assert!(climate.text.contains("только просмотр"));
    assert!(!callbacks(&climate).iter().any(|p| matches!(
        p,
        Payload::Control(ControlPayload::QuickAction {
            cmd: DeviceCmd::SetTemp(_) | DeviceCmd::AdjustTemp(_),
            ..
        })
    )));
    let denied = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 1,
            cmd: DeviceCmd::SetTemp(25.0),
        },
    )
    .await?;
    assert!(denied.alert.is_some());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn climate_bounds_and_unavailable_data_do_not_produce_unsafe_commands() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    ha.states.lock().unwrap()[0].attributes.temperature = Some(30.0);
    let view = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 1 },
    )
    .await?;
    let directions: Vec<i8> = callbacks(&view)
        .into_iter()
        .filter_map(|p| match p {
            Payload::Control(ControlPayload::QuickAction {
                cmd: DeviceCmd::AdjustTemp(v),
                ..
            }) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(directions, vec![-1]);
    for target in [31.0, 15.0, f32::NAN] {
        let failed = open(
            &config,
            ControlPayload::QuickAction {
                room: 1,
                device: 1,
                cmd: DeviceCmd::SetTemp(target),
            },
        )
        .await?;
        assert!(failed.alert.is_some());
    }
    ha.states.lock().unwrap()[0].state = "unavailable".into();
    let view = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 1 },
    )
    .await?;
    assert!(!callbacks(&view)
        .iter()
        .any(|p| matches!(p, Payload::Control(ControlPayload::QuickAction { .. }))));
    let failed = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 1,
            cmd: DeviceCmd::SetTemp(22.0),
        },
    )
    .await?;
    assert!(failed.alert.is_some());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn climate_rapid_steps_preserve_order_even_when_ha_reports_old_target() -> anyhow::Result<()>
{
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.stale_temperature.store(true, Ordering::SeqCst);
    let mut replies = Vec::new();
    for direction in [1, 1, -1, 1] {
        replies.push(config.climate_controls.enqueue(
            config.clone(),
            42,
            1,
            TemperatureCommand::Step(direction),
        )?);
    }
    let mut targets = Vec::new();
    for reply in replies {
        targets.push(reply.await??.target);
    }
    assert_eq!(targets, vec![22.5, 23.0, 22.5, 23.0]);
    assert_eq!(
        ha.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.2["temperature"].as_f64().unwrap())
            .collect::<Vec<_>>(),
        vec![22.5, 23.0, 22.5, 23.0]
    );
    // A separate change made in HA becomes the new base, not the remembered target.
    ha.states.lock().unwrap()[0].attributes.temperature = Some(25.0);
    let next = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))?
        .await??;
    assert_eq!(next.target, 25.5);
    Ok(())
}

#[tokio::test]
async fn climate_queue_obeys_limits_and_stops_after_uncertain_service_failure() -> anyhow::Result<()>
{
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.stale_temperature.store(true, Ordering::SeqCst);
    ha.states.lock().unwrap()[0].attributes.temperature = Some(29.5);
    let mut replies = Vec::new();
    for direction in [1, 1, -1] {
        replies.push(config.climate_controls.enqueue(
            config.clone(),
            42,
            1,
            TemperatureCommand::Step(direction),
        )?);
    }
    let mut targets = Vec::new();
    for reply in replies {
        targets.push(reply.await??.target);
    }
    assert_eq!(targets, vec![30.0, 30.0, 29.5]);
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    ha.fail_write.store(true, Ordering::SeqCst);
    let first =
        config
            .climate_controls
            .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(-1))?;
    let second =
        config
            .climate_controls
            .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(-1))?;
    assert!(first.await?.is_err());
    assert!(second.await?.is_err());
    assert_eq!(
        ha.calls.lock().unwrap().len(),
        3,
        "Failed device command must not be retried"
    );
    Ok(())
}

#[tokio::test]
async fn climate_queue_rechecks_access_before_each_command() -> anyhow::Result<()> {
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let first =
        config
            .climate_controls
            .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.write_started.notified(),
    )
    .await?;
    let second =
        config
            .climate_controls
            .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))?;
    db::access::set_room_access(42, 1, true, false, &config.db).await?;
    ha.release_write.notify_one();
    assert!(first.await?.is_ok());
    assert!(second.await?.is_err());
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn climate_command_is_not_repeated_when_telegram_requests_retry() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Text,
    )
    .await;
    let mut view = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 1,
            cmd: DeviceCmd::AdjustTemp(1),
        },
    )
    .await?;
    assert!(view.alert.is_none());
    assert!(view.notice.as_ref().unwrap().contains("22.5°"));
    // Exercise the text fallback for a screen longer than a photo caption.
    view.text
        .push_str(&"\nДополнительные данные состояния".repeat(50));
    let (bot, server) = mock_telegram_edits(vec![
        (429, serde_json::json!({"ok": false, "error_code": 429, "description": "Too Many Requests", "parameters": {"retry_after": 1}})),
        (200, serde_json::json!({"ok": true, "result": {"message_id": 7, "date": 1_700_000_000, "chat": {"id": 42, "type": "private"}, "text": "ok"}})),
    ]).await?;
    let token = config.begin_view_update(42);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::bot::handlers::update_view_cancellable(
            &bot,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            42,
            view,
            config.clone(),
            Some(&token),
        ),
    )
    .await??;
    assert_eq!(server.await??.len(), 2);
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    config.finish_view_update(42, &token);
    Ok(())
}

#[tokio::test]
async fn service_error_preserves_navigation_and_refresh_does_not_repeat_action(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.fail_write.store(true, Ordering::SeqCst);
    let error = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::Toggle,
        },
    )
    .await?;
    assert!(error.alert.as_ref().unwrap().contains("Обновите состояние"));
    assert_eq!(
        error.payload,
        Payload::Control(ControlPayload::RoomDetail { room: 1 })
    );
    let buttons = callbacks(&error);
    assert!(buttons.contains(&Payload::Home));
    router(buttons[0].clone(), 42, config.clone()).await?;
    router(error.payload, 42, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    ha.fail_read.store(true, Ordering::SeqCst);
    let offline = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    assert!(offline.alert.is_some());
    assert!(callbacks(&offline).contains(&Payload::Home));
    Ok(())
}

#[tokio::test]
async fn viewing_commands_cannot_toggle_a_light() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::access::set_room_access(42, 1, true, false, &config.db).await?;
    open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::ShowChart { h: 24, o: 0 },
        },
    )
    .await?;
    open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::EnterManualInput,
        },
    )
    .await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn repeated_light_and_switch_buttons_keep_the_same_requested_state() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::sync_device("switch.desk", "bedroom", "Стол", "switch", &config.db).await?;
    db::subscriptions::toggle_hidden("switch.desk", &config.db).await?;
    ha.states.lock().unwrap().push(entity("switch.desk", "off"));
    let room = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    for (id, expected) in [(3, DeviceCmd::TurnOff), (4, DeviceCmd::TurnOn)] {
        let button = callbacks(&room)
            .into_iter()
            .find(|p| {
                matches!(p,
                    Payload::Control(ControlPayload::QuickAction { device, .. }) if *device == id
                )
            })
            .unwrap();
        assert_eq!(
            button,
            Payload::Control(ControlPayload::QuickAction {
                room: 1,
                device: id,
                cmd: expected
            })
        );
        for _ in 0..2 {
            let view = router(button.clone(), 42, config.clone()).await?;
            assert!(view.alert.is_none());
            assert_eq!(
                view.payload,
                Payload::Control(ControlPayload::RoomDetail { room: 1 })
            );
        }
    }
    assert_eq!(
        ha.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.0.clone())
            .collect::<Vec<_>>(),
        [
            "light.turn_off",
            "light.turn_off",
            "switch.turn_on",
            "switch.turn_on"
        ]
    );
    Ok(())
}

#[tokio::test]
async fn action_group_refresh_never_repeats_execution() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = db::action_groups::create_action_group("Свет", &config.db).await?;
    db::action_groups::toggle_action_group_item(group, 3, &config.db).await?;
    let view = router(
        Payload::Control(ControlPayload::ExecuteActionGroup {
            group,
            command: db::action_groups::ActionGroupCommand::TurnOff,
        }),
        config.root_user,
        config.clone(),
    )
    .await?;
    assert!(view.alert.is_none());
    assert_eq!(
        view.payload,
        Payload::Control(ControlPayload::ActionGroupDetail { group })
    );
    router(view.payload.clone(), config.root_user, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(ha.calls.lock().unwrap()[0].0, "light.turn_off");
    Ok(())
}

#[tokio::test]
async fn chart_navigation_loads_state_once_and_keeps_selected_scale() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let chart = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 2,
            cmd: DeviceCmd::ShowChart { h: 72, o: -24 },
        },
    )
    .await?;
    assert!(chart.image.is_some(), "{:?}", chart.alert);
    assert_eq!(ha.state_reads.load(Ordering::SeqCst), 1);
    assert_eq!(*ha.history_requests.lock().unwrap(), vec![(72, -24)]);
    let current = chart
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .find(|button| button.text == "🔄 Текущее")
        .unwrap();
    let InlineKeyboardButtonKind::CallbackData(data) = &current.kind else {
        panic!("Expected callback");
    };
    let current = router(Payload::from_string(data)?, 42, config.clone()).await?;
    assert!(current.image.is_some());
    let context = current.payload.to_string();
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    crate::bot::handlers::refresh_current_view(
        &teloxide::Bot::new("123:test"),
        &config,
        42,
        teloxide::types::ChatId(42),
        teloxide::types::MessageId(7),
        &context,
    )
    .await?;
    assert_eq!(ha.state_reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        *ha.history_requests.lock().unwrap(),
        vec![(72, -24), (72, 0)]
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn graph_rejects_zero_and_excessive_intervals_before_loading_history() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    for (h, o) in [(0, 0), (u32::MAX, 0), (24, i32::MIN), (24, 1)] {
        let view = open(
            &config,
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::ShowChart { h, o },
            },
        )
        .await?;
        assert!(view.alert.is_some());
        assert!(!view.kb.inline_keyboard.is_empty());
    }
    assert!(ha.history_requests.lock().unwrap().is_empty());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

async fn mock_telegram_edits(
    responses: Vec<(u16, serde_json::Value)>,
) -> anyhow::Result<(
    teloxide::Bot,
    tokio::task::JoinHandle<anyhow::Result<Vec<serde_json::Value>>>,
)> {
    mock_telegram_responses(responses, None).await
}

async fn mock_telegram_responses(
    responses: Vec<(u16, serde_json::Value)>,
    pause_first: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
) -> anyhow::Result<(
    teloxide::Bot,
    tokio::task::JoinHandle<anyhow::Result<Vec<serde_json::Value>>>,
)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let api_url = format!("http://{}", listener.local_addr()?).parse()?;
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            let body_start;
            loop {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).await?;
                anyhow::ensure!(count > 0, "Incomplete HTTP request");
                request.extend_from_slice(&chunk[..count]);
                if let Some(index) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    body_start = index + 4;
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&request[..body_start]);
            let request_path = headers.lines().next().unwrap().to_string();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            while request.len() < body_start + length {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).await?;
                anyhow::ensure!(count > 0, "Incomplete HTTP body");
                request.extend_from_slice(&chunk[..count]);
            }
            let bytes = &request[body_start..body_start + length];
            let mut parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap_or_else(
                |_| serde_json::json!({"multipart": String::from_utf8_lossy(bytes)}),
            );
            parsed["method"] = request_path.into();
            requests.push(parsed);
            if requests.len() == 1 {
                if let Some((started, release)) = &pause_first {
                    started.notify_one();
                    release.notified().await;
                }
            }
            let body = body.to_string();
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await?;
        }
        Ok(requests)
    });
    Ok((teloxide::Bot::new("123:test").set_api_url(api_url), server))
}

fn graph_update_view() -> View {
    View {
        // Keep these retry/cancellation fixtures in Telegram's text fallback.
        text: format!("График за 72 часа\n{}", "Данные истории\n".repeat(100)),
        payload: Payload::Control(ControlPayload::QuickAction {
            room: 1,
            device: 2,
            cmd: DeviceCmd::ShowChart { h: 72, o: 0 },
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn long_caption_transition_retries_message_without_repeating_confirmed_command(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let id = confirmation_id(
        &open(
            &config,
            ControlPayload::QuickAction {
                room: 1,
                device: 3,
                cmd: DeviceCmd::TurnOff,
            },
        )
        .await?,
    );
    let mut view = open(&config, ControlPayload::ConfirmAction { id }).await?;
    view.text
        .push_str(&"\nДополнительные данные состояния".repeat(50));
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let (bot, server) = mock_telegram_edits(vec![
        (429, serde_json::json!({"ok": false, "error_code": 429, "description": "Too Many Requests", "parameters": {"retry_after": 1}})),
        (200, serde_json::json!({"ok": true, "result": {"message_id": 8, "date": 1_700_000_000, "chat": {"id": 42, "type": "private"}, "text": "ok"}})),
        (200, serde_json::json!({"ok": true, "result": true})),
    ]).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(6),
        crate::bot::handlers::update_view(
            &bot,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            42,
            view,
            config.clone(),
        ),
    )
    .await??;
    let requests = tokio::time::timeout(std::time::Duration::from_secs(3), server).await???;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["text"], requests[1]["text"]);
    assert_eq!(requests[2]["message_id"], 7);
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    let session = config.sessions.get(&42).unwrap();
    assert_eq!(session.ui_message_mode, crate::models::UiMessageMode::Text);
    assert_eq!(session.last_menu_id, 8);
    Ok(())
}

#[tokio::test]
async fn telegram_rate_limit_retries_edit_without_repeating_device_action() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Text,
    )
    .await;
    let (bot, server) = mock_telegram_edits(vec![
        (429, serde_json::json!({"ok": false, "error_code": 429, "description": "Too Many Requests", "parameters": {"retry_after": 1}})),
        (200, serde_json::json!({"ok": true, "result": {"message_id": 7, "date": 1_700_000_000, "chat": {"id": 42, "type": "private"}, "text": "ok"}})),
    ]).await?;
    let started = std::time::Instant::now();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::bot::handlers::update_view(
            &bot,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            42,
            graph_update_view(),
            config.clone(),
        ),
    )
    .await??;
    let requests = server.await??;
    assert_eq!(requests.len(), 2);
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    assert!(requests
        .iter()
        .all(|request| request["text"].as_str().unwrap().contains("72 часа")));
    assert_eq!(
        config.sessions.get(&42).unwrap().current_context,
        graph_update_view().payload.to_string()
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn new_graph_or_back_cancels_pending_retry_without_blocking_ui() -> anyhow::Result<()> {
    let (config, _) = setup().await?;
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Text,
    )
    .await;
    let (bot, server) = mock_telegram_edits(vec![
        (429, serde_json::json!({"ok": false, "error_code": 429, "description": "Too Many Requests", "parameters": {"retry_after": 60}})),
    ]).await?;
    let first = config.begin_view_update(42);
    let worker_config = config.clone();
    let worker_token = first.clone();
    let worker = tokio::spawn(async move {
        crate::bot::handlers::update_view_cancellable(
            &bot,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            42,
            graph_update_view(),
            worker_config,
            Some(&worker_token),
        )
        .await
    });
    assert_eq!(server.await??.len(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !config
            .sessions
            .get(&42)
            .unwrap()
            .is_ui_refresh_blocked(Utc::now())
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let second = config.begin_view_update(42);
    assert!(first.is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(1), worker).await???;
    config.finish_view_update(42, &first);
    assert!(Arc::ptr_eq(&config.view_updates.get(&42).unwrap(), &second));
    assert!(config.ui_lock_for(42).try_lock().is_ok());
    assert_eq!(
        config.sessions.get(&42).unwrap().current_context,
        Payload::Home.to_string()
    );
    config.cancel_view_update(42);
    assert!(second.is_cancelled());
    assert!(!config.view_updates.contains_key(&42));
    Ok(())
}

#[tokio::test]
async fn camera_preview_keeps_original_time_after_failure_and_never_uses_another_camera(
) -> anyhow::Result<()> {
    let (config, _) = setup().await?;
    let id = db::cameras::add_manual_camera(
        db::cameras::NewCamera {
            name: "Вход",
            room_id: 1,
            stream_url: "rtsp://unused.invalid/stream",
            snapshot_url: None,
            clip_seconds: 10,
        },
        &config.db,
    )
    .await?;
    let received_at = Utc::now() - Duration::minutes(5);
    config.camera_snapshot_cache.lock().await.insert(
        id,
        crate::models::CameraSnapshotCache {
            bytes: Some(vec![1, 2, 3]),
            captured_at: Some(received_at),
            failed_at: Some(Utc::now()),
            last_error: Some("private diagnostic details".into()),
            last_warned_at: None,
            refreshing: false,
        },
    );
    let payload = Payload::Camera(CameraPayload::CameraDetail { id });
    let expected_time = received_at
        .with_timezone(&chrono::Local)
        .format("%d.%m.%Y %H:%M:%S %:z")
        .to_string();
    for _ in 0..2 {
        let view = router(payload.clone(), 42, config.clone()).await?;
        assert_eq!(view.image, Some(vec![1, 2, 3]));
        assert!(view.text.contains(&expected_time));
        assert!(view.text.contains("последний доступный кадр"));
        assert!(!view.text.contains("private diagnostic details"));
    }
    {
        let mut cache = config.camera_snapshot_cache.lock().await;
        let preview = cache.get_mut(&id).unwrap();
        preview.bytes = None;
        preview.captured_at = None;
    }
    let missing = router(payload, 42, config.clone()).await?;
    assert!(missing.text.contains("Кадр недоступен"));
    assert_eq!(
        missing.image.as_deref(),
        Some(crate::bot::utils::UI_PLACEHOLDER_BYTES)
    );
    Ok(())
}

#[tokio::test]
async fn hidden_devices_cannot_be_opened_via_view_route() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::access::set_room_access(42, 1, false, false, &config.db).await?;
    for device in [1, 2] {
        let view = open(&config, ControlPayload::DeviceControl { room: 1, device }).await?;
        assert!(view.alert.is_some());
        assert!(view.image.is_none());
        assert!(!view.text.contains("Задано:"));
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn climate_respects_reported_step_and_handles_range_and_missing_targets() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    db::set_user_language(42, Language::En, &config.db).await?;
    {
        let mut states = ha.states.lock().unwrap();
        states[0].attributes.temperature = Some(16.0);
        states[0].attributes.target_temp_step = Some(1.0);
    }
    let view = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 1 },
    )
    .await?;
    let directions: Vec<i8> = callbacks(&view)
        .into_iter()
        .filter_map(|p| match p {
            Payload::Control(ControlPayload::QuickAction {
                cmd: DeviceCmd::AdjustTemp(v),
                ..
            }) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(directions, vec![1]);
    assert!(view.text.contains("Target temperature: 16°"));
    {
        let mut states = ha.states.lock().unwrap();
        states[0].state = "heat_cool".into();
        states[0].attributes = serde_json::from_value(serde_json::json!({
            "current_temperature": 21, "temperature": null,
            "target_temp_low": 20, "target_temp_high": 24,
            "min_temp": 16, "max_temp": 30,
        }))?;
    }
    let range = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 1 },
    )
    .await?;
    assert!(range.text.contains("20° – 24°"));
    assert!(!callbacks(&range)
        .iter()
        .any(|p| matches!(p, Payload::Control(ControlPayload::QuickAction { .. }))));
    {
        let mut states = ha.states.lock().unwrap();
        states[0].state = "heat".into();
        states[0].attributes = Default::default();
    }
    let missing = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 1 },
    )
    .await?;
    assert!(missing.text.contains("Target temperature: unavailable"));
    assert!(!callbacks(&missing)
        .iter()
        .any(|p| matches!(p, Payload::Control(ControlPayload::QuickAction { .. }))));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn missing_and_offline_devices_remain_visible_without_action_buttons() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.states
        .lock()
        .unwrap()
        .retain(|e| e.entity_id != "light.bedroom");
    let missing = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    assert!(
        callbacks(&missing).contains(&Payload::Control(ControlPayload::DeviceControl {
            room: 1,
            device: 3
        }))
    );
    assert!(missing
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .any(|b| b.text.contains("Люстра") && b.text.contains("Недоступно")));
    ha.fail_read.store(true, Ordering::SeqCst);
    let offline = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    assert!(offline.alert.as_deref().unwrap().contains("Home Assistant"));
    assert_eq!(
        callbacks(&offline)
            .iter()
            .filter(|p| matches!(p, Payload::Control(ControlPayload::DeviceControl { .. })))
            .count(),
        3
    );
    assert!(!callbacks(&offline)
        .iter()
        .any(|p| matches!(p, Payload::Control(ControlPayload::QuickAction { .. }))));
    assert!(callbacks(&offline).contains(&Payload::Home));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn room_pages_restore_position_and_recheck_revoked_access() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    for n in 0..22 {
        let id = format!("sensor.extra_{n:02}");
        db::devices::sync_device(
            &id,
            "bedroom",
            &format!("Датчик {n:02}"),
            "temperature",
            &config.db,
        )
        .await?;
        db::subscriptions::toggle_hidden(&id, &config.db).await?;
        ha.states.lock().unwrap().push(entity(&id, "20.00"));
    }
    let second = open(&config, ControlPayload::RoomPage { room: 1, page: 1 }).await?;
    let back = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    assert_eq!(callbacks(&second), callbacks(&back));
    assert_eq!(
        callbacks(&second)
            .iter()
            .filter(|p| matches!(
                p,
                Payload::Control(
                    ControlPayload::DeviceControl { .. } | ControlPayload::QuickAction { .. }
                )
            ))
            .count(),
        10
    );
    assert!(
        callbacks(&second).contains(&Payload::Control(ControlPayload::RoomPage {
            room: 1,
            page: 2
        }))
    );
    // Settings retain their own origin and page without changing control navigation.
    let settings = router(
        Payload::AdminSettings(SettingsPayload::RoomPage { room: 1, page: 0 }),
        1,
        config.clone(),
    )
    .await?;
    assert!(callbacks(&settings).contains(&Payload::AdminSettings(SettingsPayload::ListRooms)));
    db::access::set_room_access(42, 1, false, false, &config.db).await?;
    let denied = open(&config, ControlPayload::RoomPage { room: 1, page: 1 }).await?;
    assert!(denied.alert.is_some());
    assert!(!callbacks(&denied)
        .iter()
        .any(|p| matches!(p, Payload::Control(ControlPayload::DeviceControl { .. }))));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn graph_error_empty_history_and_manual_cancel_keep_interval() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let selection = ControlPayload::QuickAction {
        room: 1,
        device: 2,
        cmd: DeviceCmd::ShowChart { h: 72, o: -48 },
    };
    ha.empty_history.store(true, Ordering::SeqCst);
    let empty = open(&config, selection.clone()).await?;
    assert!(empty.alert.is_none());
    assert!(empty.text.contains("Нет данных за выбранный период"));
    assert!(empty.image.is_none()); // Normal configured background remains available.
    assert_eq!(empty.payload, Payload::Control(selection.clone()));
    let manual = ControlPayload::QuickAction {
        room: 1,
        device: 2,
        cmd: DeviceCmd::EnterManualInput,
    };
    for _ in 0..2 {
        let prompt = open(&config, manual.clone()).await?;
        assert!(callbacks(&prompt).contains(&Payload::Control(selection.clone())));
        assert!(matches!(
            prompt.next_state,
            Some(State::WaitingForGraphInterval {
                device_id: 2,
                room_id: 1
            })
        ));
    }
    ha.fail_read.store(true, Ordering::SeqCst);
    let failed = open(&config, selection.clone()).await?;
    assert!(failed.alert.is_some());
    assert_eq!(failed.payload, Payload::Control(selection.clone()));
    assert!(callbacks(&failed).contains(&Payload::Control(selection)));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn climate_and_confirmation_use_actual_room_and_show_limits() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let card = open(
        &config,
        ControlPayload::DeviceControl {
            room: 999,
            device: 1,
        },
    )
    .await?;
    assert!(card.header.as_deref().unwrap().contains("Спальня"));
    assert!(card.text.contains("Пределы: 16° – 30°"));
    assert!(card.text.contains("Шаг: 0.5°"));
    assert!(card.text.contains("climate.bedroom"));
    assert!(callbacks(&card).contains(&Payload::Control(ControlPayload::RoomDetail { room: 1 })));
    db::devices::toggle_device_critical(3, &config.db).await?;
    let confirm = open(
        &config,
        ControlPayload::QuickAction {
            room: 999,
            device: 3,
            cmd: DeviceCmd::TurnOff,
        },
    )
    .await?;
    assert!(confirm.text.contains("Спальня"));
    db::access::set_device_access(42, "light.bedroom", true, false, true, &config.db).await?;
    let denied = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::TurnOff,
        },
    )
    .await?;
    assert!(denied.text.contains(t(Language::Ru, "access.denied.text")));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn legacy_saved_command_refresh_never_sends_physical_command() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let legacy = Payload::Control(ControlPayload::QuickAction {
        room: 1,
        device: 3,
        cmd: DeviceCmd::Toggle,
    });
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &legacy.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let (bot, server) = mock_telegram_edits(vec![(200, telegram_message(7, true))]).await?;
    crate::bot::handlers::refresh_current_view(
        &bot,
        &config,
        42,
        teloxide::types::ChatId(42),
        teloxide::types::MessageId(7),
        &legacy.to_string(),
    )
    .await?;
    let requests = server.await??;
    assert!(requests[0]["method"]
        .as_str()
        .unwrap()
        .contains("EditMessageMedia"));
    assert!(ha.calls.lock().unwrap().is_empty());
    assert_eq!(
        config.sessions.get(&42).unwrap().current_context,
        Payload::Control(ControlPayload::RoomDetail { room: 1 }).to_string()
    );
    Ok(())
}

fn telegram_message(id: i32, photo: bool) -> serde_json::Value {
    let mut message = serde_json::json!({"message_id": id, "date": 1_700_000_000, "chat": {"id": 42, "type": "private"}});
    if photo {
        message["photo"] = serde_json::json!([{"file_id":"image", "file_unique_id":"image", "width":1, "height":1}]);
    } else {
        message["text"] = "ok".into();
    }
    serde_json::json!({"ok": true, "result": message})
}

#[tokio::test]
async fn cancelled_photo_to_text_replacement_cannot_commit_old_context() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (bot, server) = mock_telegram_responses(
        vec![
            (200, telegram_message(8, false)),
            (200, serde_json::json!({"ok": true, "result": true})),
        ],
        Some((started.clone(), release.clone())),
    )
    .await?;
    let token = config.begin_view_update(42);
    let job = {
        let config = config.clone();
        tokio::spawn(async move {
            crate::bot::handlers::update_view_cancellable(
                &bot,
                teloxide::types::ChatId(42),
                teloxide::types::MessageId(7),
                42,
                graph_update_view(),
                config,
                Some(&token),
            )
            .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified()).await?;
    let latest = config.begin_view_update(42);
    release.notify_one();
    job.await??;
    let requests = server.await??;
    assert_eq!(requests[1]["message_id"], 8); // Delete the cancelled replacement, keep the current menu.
    let session = config.sessions.get(&42).unwrap();
    assert_eq!(session.last_menu_id, 7);
    assert_eq!(session.current_context, Payload::Home.to_string());
    assert_eq!(session.ui_message_mode, crate::models::UiMessageMode::Photo);
    assert!(!latest.is_cancelled());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

async fn camera_for_ui_tests(config: &AppConfig) -> anyhow::Result<i64> {
    db::cameras::add_manual_camera(
        db::cameras::NewCamera {
            name: "Вход",
            room_id: 1,
            stream_url: "rtsp://unused.invalid/stream",
            snapshot_url: None,
            clip_seconds: 10,
        },
        &config.db,
    )
    .await
}

#[tokio::test]
async fn archive_pages_keep_filter_on_back_and_refresh_without_revealing_denied_camera(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let camera = camera_for_ui_tests(&config).await?;
    let rule =
        sqlx::query("INSERT INTO camera_recording_rules (name, camera_id) VALUES ('Дверь', ?)")
            .bind(camera)
            .execute(&config.db)
            .await?
            .last_insert_rowid();
    for n in 0..36 {
        sqlx::query("INSERT INTO camera_recording_sessions (event_group_id, rule_id, camera_id, trigger_summary, status, first_event_at, last_event_at, stop_after_at, expires_at, created_at) VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
            .bind(format!("event-{n}")).bind(rule).bind(camera).bind(format!("Событие {n}"))
            .bind(if n == 0 { "failed" } else { "ready" }).bind(Utc::now() - Duration::minutes(n))
            .execute(&config.db).await?;
    }
    let selection = Payload::Camera(CameraPayload::RecordingArchivePage {
        camera,
        filter: "ready".into(),
        page: 3,
    });
    let selected = router(selection.clone(), 42, config.clone()).await?;
    assert_eq!(selected.payload, selection);
    let sessions: Vec<_> = callbacks(&selected)
        .into_iter()
        .filter(|p| matches!(p, Payload::Camera(CameraPayload::RecordingSession { .. })))
        .collect();
    assert_eq!(sessions.len(), 5); // Older records beyond the former 30-record limit.
    let detail = router(sessions[0].clone(), 42, config.clone()).await?;
    let back = callbacks(&detail)
        .into_iter()
        .find(|p| matches!(p, Payload::Camera(CameraPayload::RecordingArchive { .. })))
        .unwrap();
    let returned = router(back, 42, config.clone()).await?;
    assert_eq!(returned.payload, selection);
    assert_eq!(callbacks(&returned), callbacks(&selected));
    let refreshed = router(returned.payload, 42, config.clone()).await?;
    assert_eq!(refreshed.payload, selection);
    db::access::set_room_access(42, 1, false, false, &config.db).await?;
    let denied = router(selection, 42, config.clone()).await?;
    assert!(denied.alert.is_some());
    assert!(!denied.text.contains("Событие"));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn background_graph_snapshot_and_long_text_switch_back_to_camera_background(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let camera = camera_for_ui_tests(&config).await?;
    db::settings::set_i64(db::settings::UI_BACKGROUND_CAMERA_ID, camera, &config.db).await?;
    *config.ui_background_cache.lock().await = Some(crate::models::UiBackgroundCache {
        camera_id: camera,
        captured_at: Some(Utc::now()),
        failed_at: None,
        last_warned_at: None,
        last_error: None,
        bytes: Some(b"configured-camera-background".to_vec()),
        refreshing: false,
    });
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    for (image, expected) in [
        (None, "configured-camera-background"),
        (Some(b"graph-image".to_vec()), "graph-image"),
        (Some(b"snapshot-image".to_vec()), "snapshot-image"),
    ] {
        let (bot, server) = mock_telegram_edits(vec![(200, telegram_message(7, true))]).await?;
        let view = View {
            compact: true,
            text: "Экран".into(),
            image,
            ..Default::default()
        };
        crate::bot::handlers::update_view(
            &bot,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            42,
            view,
            config.clone(),
        )
        .await?;
        let requests = server.await??;
        assert!(requests[0]["multipart"]
            .as_str()
            .unwrap()
            .contains(expected));
    }
    let (bot, server) = mock_telegram_edits(vec![
        (200, telegram_message(8, false)),
        (200, serde_json::json!({"ok": true, "result": true})),
    ])
    .await?;
    crate::bot::handlers::update_view(
        &bot,
        teloxide::types::ChatId(42),
        teloxide::types::MessageId(7),
        42,
        graph_update_view(),
        config.clone(),
    )
    .await?;
    server.await??;
    assert_eq!(
        config.sessions.get(&42).unwrap().ui_message_mode,
        crate::models::UiMessageMode::Text
    );
    let (bot, server) = mock_telegram_edits(vec![
        (200, telegram_message(9, true)),
        (200, serde_json::json!({"ok": true, "result": true})),
    ])
    .await?;
    // Even a button from the previous message must target the latest active menu.
    crate::bot::handlers::update_view(
        &bot,
        teloxide::types::ChatId(42),
        teloxide::types::MessageId(7),
        42,
        View {
            compact: true,
            text: "Главное меню".into(),
            ..Default::default()
        },
        config.clone(),
    )
    .await?;
    let requests = server.await??;
    assert!(requests[0]["multipart"]
        .as_str()
        .unwrap()
        .contains("configured-camera-background"));
    assert_eq!(requests[1]["message_id"], 8);
    assert_eq!(
        config.sessions.get(&42).unwrap().ui_message_mode,
        crate::models::UiMessageMode::Photo
    );
    assert_eq!(config.sessions.get(&42).unwrap().last_menu_id, 9);
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn rooms_page_returns_to_same_position_and_handles_shrinking_list() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    for n in 0..12 {
        let area = format!("area_{n}");
        db::rooms::sync_rooms_from_ha(&area, &format!("Комната {n}"), &config.db).await?;
        db::devices::sync_device(
            &format!("sensor.area_{n}"),
            &area,
            "Датчик",
            "temperature",
            &config.db,
        )
        .await?;
    }
    let second = open(&config, ControlPayload::RoomsPage { page: 1 }).await?;
    let restored = open(&config, ControlPayload::ListRooms).await?;
    assert_eq!(callbacks(&second), callbacks(&restored));
    assert!(callbacks(&second).contains(&Payload::Control(ControlPayload::RoomsPage { page: 0 })));
    sqlx::query("UPDATE rooms SET hide = 1 WHERE area != 'bedroom'")
        .execute(&config.db)
        .await?;
    let shorter = open(&config, ControlPayload::ListRooms).await?;
    assert!(callbacks(&shorter).contains(&Payload::Control(ControlPayload::RoomDetail { room: 1 })));
    assert!(!callbacks(&shorter)
        .iter()
        .any(|p| matches!(p, Payload::Control(ControlPayload::RoomsPage { .. }))));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[test]
fn legacy_callback_bytes_and_dialogue_state_keep_their_meaning() -> anyhow::Result<()> {
    for (bytes, payload) in [
        (vec![0], Payload::Home),
        (vec![1, 0], Payload::Control(ControlPayload::ListRooms)),
        (
            vec![1, 3, 2, 6, 0],
            Payload::Control(ControlPayload::QuickAction {
                room: 1,
                device: 3,
                cmd: DeviceCmd::Toggle,
            }),
        ),
        (
            vec![1, 3, 2, 4, 5, 24, 0],
            Payload::Control(ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::ShowChart { h: 24, o: 0 },
            }),
        ),
    ] {
        assert_eq!(postcard::from_bytes::<Payload>(&bytes)?, payload);
        assert_eq!(postcard::to_allocvec(&payload)?, bytes);
    }
    let state: State =
        serde_json::from_str(r#"{"WaitingForGraphInterval":{"device_id":2,"room_id":1}}"#)?;
    assert!(matches!(
        state,
        State::WaitingForGraphInterval {
            device_id: 2,
            room_id: 1
        }
    ));
    Ok(())
}
