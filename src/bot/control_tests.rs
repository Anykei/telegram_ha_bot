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
    fail_entity: Mutex<Option<String>>,
    stale_temperature: AtomicBool,
    pause_next_read: AtomicBool,
    read_started: tokio::sync::Notify,
    release_read: tokio::sync::Notify,
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
        Ok(self
            .states
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.entity_id.starts_with("scene.") || e.entity_id.starts_with("script."))
            .cloned()
            .collect())
    }
    async fn fetch_states_by_ids(&self, ids: &[String]) -> anyhow::Result<Vec<Entity>> {
        self.state_reads.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(!self.fail_read.load(Ordering::SeqCst), "HA offline");
        let states = self
            .states
            .lock()
            .unwrap()
            .iter()
            .filter(|e| ids.contains(&e.entity_id))
            .cloned()
            .collect();
        if self.pause_next_read.swap(false, Ordering::SeqCst) {
            self.read_started.notify_one();
            self.release_read.notified().await;
        }
        Ok(states)
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
        anyhow::ensure!(
            !self.fail_write.load(Ordering::SeqCst)
                && self.fail_entity.lock().unwrap().as_deref() != Some(id),
            "HA response lost after dispatch"
        );
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
    // This fixture represents an authenticated user update; UI persistence
    // itself no longer invents or advances an activity timestamp.
    let generation = db::admission_generation(42, &db).await?.unwrap();
    db::record_user_activity(42, &generation, Utc::now(), &db).await?;
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
        html_ui: Default::default(),
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
        admission_lifecycle: tokio::sync::Mutex::new(()),
        dialogue_generations: DashMap::new(),
        active_dialogues: DashMap::new(),
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
        notification_delivery: Default::default(),
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

#[tokio::test]
async fn long_log_pages_keep_all_entries_filter_and_owner_after_session_reset() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    for n in 0..30 {
        let message = format!("запись-{n:02}: {}", "данные 🏡 ".repeat(40));
        db::activity_log::log(
            db::activity_log::NewActivity {
                user_id: Some(1),
                kind: "device",
                entity_type: "device",
                entity_id: Some("light.bedroom"),
                action: "turn_on",
                status: "error",
                message: Some(&message),
            },
            &config.db,
        )
        .await?;
    }
    let base = Payload::Admin(AdminPayload::ActivityLog {
        filter: ActivityLogFilter::Errors,
    });
    let first = router(base.clone(), 1, config.clone()).await?;
    let Payload::TextPage { screen, .. } = first.payload else {
        panic!("Expected pages")
    };
    let mut viewed = String::new();
    let mut page = 0;
    loop {
        config.sessions.clear(); // Routes remain valid after losing the in-memory UI session.
        let current = router(Payload::TextPage { screen, page }, 1, config.clone()).await?;
        let text = current.get_plain_text();
        assert!(crate::bot::text_pages::text_len(&text) <= 4096);
        assert!(text.contains(t(Language::Ru, "common.updated")));
        assert!(callbacks(&current).contains(&Payload::Admin(AdminPayload::ListActions)));
        assert!(
            callbacks(&current).contains(&Payload::Admin(AdminPayload::ActivityLog {
                filter: ActivityLogFilter::All
            }))
        );
        assert!(callbacks(&current).contains(&Payload::TextPage { screen, page }));
        viewed.push_str(&text);
        if !callbacks(&current).contains(&Payload::TextPage {
            screen,
            page: page + 1,
        }) {
            break;
        }
        page += 1;
        assert!(page < 30);
    }
    assert!(page > 1);
    for n in 0..30 {
        assert!(
            viewed.contains(&format!("запись-{n:02}:")),
            "missing entry {n}"
        );
    }
    let other_user = router(Payload::TextPage { screen, page }, 42, config.clone()).await?;
    assert!(!other_user.get_plain_text().contains("запись-"));
    assert!(other_user
        .text
        .contains(t(Language::Ru, "common.page_expired")));
    let saved: serde_json::Value = serde_json::from_str(
        &db::ui_text_pages::context(screen, 1, &config.db)
            .await?
            .unwrap(),
    )?;
    assert_eq!(saved["source"], base.to_string());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn text_pages_recheck_device_access_and_never_replay_a_saved_command() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let long_name = format!("Доступное устройство {}", "длинное имя ".repeat(600));
    sqlx::query("UPDATE devices SET alias = ? WHERE id = 1")
        .bind(&long_name)
        .execute(&config.db)
        .await?;
    let mut legacy = View {
        text: long_name.clone(),
        payload: Payload::Control(ControlPayload::QuickAction {
            room: 1,
            device: 1,
            cmd: DeviceCmd::SetTemp(25.0),
        }),
        ..Default::default()
    };
    crate::bot::text_pages::prepare(&mut legacy, 42, &config).await?;
    let Payload::TextPage { screen, .. } = legacy.payload else {
        panic!("Expected pages")
    };
    router(Payload::TextPage { screen, page: 1 }, 42, config.clone()).await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    let card = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 1 },
    )
    .await?;
    let Payload::TextPage { screen, .. } = card.payload else {
        panic!("Expected pages")
    };
    let second = router(Payload::TextPage { screen, page: 1 }, 42, config.clone()).await?;
    assert!(matches!(second.payload, Payload::TextPage { page: 1, .. }));
    assert!(second
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .any(|b| b.text == t(Language::Ru, "control.refresh")));
    db::access::set_device_access(42, "climate.bedroom", false, false, &config.db).await?;
    let denied = router(Payload::TextPage { screen, page: 1 }, 42, config.clone()).await?;
    assert!(denied.alert.is_some());
    assert!(!denied.get_plain_text().contains("Доступное устройство"));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn camera_and_recording_mode_pages_restore_position_and_check_access() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    for n in 0..23 {
        sqlx::query("INSERT INTO cameras (key, name, room_id, stream_url, enabled) VALUES (?, ?, 1, 'rtsp://unused.invalid', 1)")
            .bind(format!("camera-{n}")).bind(format!("Камера {n:02}")).execute(&config.db).await?;
        let group =
            db::camera_recording_rule_groups::create_group(&format!("Режим {n:02}"), &config.db)
                .await?;
        db::camera_recording_rule_groups::cycle_group_access_scope(group, &config.db).await?;
    }
    let selection = Payload::Camera(CameraPayload::ListCamerasPage { page: 2 });
    let cameras = router(selection.clone(), 42, config.clone()).await?;
    assert_eq!(
        callbacks(&cameras)
            .iter()
            .filter(|p| matches!(p, Payload::Camera(CameraPayload::CameraDetail { .. })))
            .count(),
        3
    );
    let back = router(
        Payload::Camera(CameraPayload::ListCameras),
        42,
        config.clone(),
    )
    .await?;
    assert_eq!(callbacks(&cameras), callbacks(&back));
    assert!(callbacks(&back).contains(&Payload::Camera(CameraPayload::RecordingGroupModes)));
    let modes = router(
        Payload::Camera(CameraPayload::RecordingGroupModesPage { page: 2 }),
        42,
        config.clone(),
    )
    .await?;
    let details: Vec<_> = callbacks(&modes)
        .into_iter()
        .filter(|p| {
            matches!(
                p,
                Payload::Camera(CameraPayload::RecordingGroupModeDetail { .. })
            )
        })
        .collect();
    assert_eq!(details.len(), 3);
    let detail = router(details[0].clone(), 42, config.clone()).await?;
    assert!(callbacks(&detail).contains(&Payload::Camera(CameraPayload::RecordingGroupModes)));
    let restored = router(
        Payload::Camera(CameraPayload::RecordingGroupModes),
        42,
        config.clone(),
    )
    .await?;
    assert_eq!(callbacks(&modes), callbacks(&restored));
    sqlx::query("UPDATE camera_recording_rule_groups SET access_scope = 'admin_only'")
        .execute(&config.db)
        .await?;
    let denied = router(modes.payload, 42, config.clone()).await?;
    assert!(!callbacks(&denied).iter().any(|p| matches!(
        p,
        Payload::Camera(CameraPayload::RecordingGroupModeDetail { .. })
    )));
    db::access::set_room_access(42, 1, false, false, &config.db).await?;
    let denied = router(selection, 42, config.clone()).await?;
    assert!(!callbacks(&denied)
        .iter()
        .any(|p| matches!(p, Payload::Camera(CameraPayload::CameraDetail { .. }))));
    let admin = router(
        Payload::Admin(AdminPayload::RoomCamerasPage { room: 1, page: 2 }),
        1,
        config.clone(),
    )
    .await?;
    assert!(callbacks(&admin).contains(&Payload::Admin(AdminPayload::PromptAddCamera { room: 1 })));
    assert!(callbacks(&admin).contains(&Payload::Admin(AdminPayload::RecordingRules { room: 1 })));
    assert_eq!(
        callbacks(&admin)
            .iter()
            .filter(|p| matches!(p, Payload::Admin(AdminPayload::ConfirmDeleteCamera { .. })))
            .count(),
        3
    );
    let restored = router(
        Payload::Admin(AdminPayload::RoomCameras { room: 1 }),
        1,
        config.clone(),
    )
    .await?;
    assert_eq!(callbacks(&admin), callbacks(&restored));
    let denied = router(
        Payload::Admin(AdminPayload::RoomCamerasPage { room: 1, page: 2 }),
        42,
        config.clone(),
    )
    .await?;
    assert!(denied.alert.is_some());
    let enabled: i64 = sqlx::query_scalar("SELECT SUM(enabled) FROM camera_recording_rule_groups")
        .fetch_one(&config.db)
        .await?;
    assert_eq!(enabled, 23);
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn action_group_and_scene_pages_keep_all_targets_and_admin_controls() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    for n in 0..13 {
        db::action_groups::create_action_group(&format!("Группа {n:02}"), &config.db).await?;
        let id = format!("scene.test_{n:02}");
        ha.states.lock().unwrap().push(entity(&id, "off"));
        sqlx::query("INSERT INTO ha_native_targets (entity_id, domain, ha_name, access_scope) VALUES (?, 'scene', ?, 'all_users')")
            .bind(&id).bind(format!("Сцена {n:02}")).execute(&config.db).await?;
    }
    sqlx::query("UPDATE action_groups SET access_scope = 'all_users'")
        .execute(&config.db)
        .await?;
    let mut seen = std::collections::HashSet::new();
    for page in 0..3 {
        let view = open(&config, ControlPayload::ActionGroupsPage { page }).await?;
        for item in callbacks(&view).into_iter().filter(|p| {
            matches!(
                p,
                Payload::Control(
                    ControlPayload::ActionGroupDetail { .. }
                        | ControlPayload::HaNativeActionDetail { .. }
                )
            )
        }) {
            assert!(seen.insert(item.to_string()));
        }
    }
    assert_eq!(seen.len(), 26);
    let restored = open(&config, ControlPayload::ActionGroups).await?;
    let selected = open(&config, ControlPayload::ActionGroupsPage { page: 2 }).await?;
    assert_eq!(callbacks(&selected), callbacks(&restored));
    let admin = router(
        Payload::Admin(AdminPayload::ActionGroupsPage { page: 2 }),
        1,
        config.clone(),
    )
    .await?;
    assert!(callbacks(&admin).contains(&Payload::Admin(AdminPayload::PromptCreateActionGroup)));
    let restored = router(
        Payload::Admin(AdminPayload::ActionGroups),
        1,
        config.clone(),
    )
    .await?;
    assert_eq!(callbacks(&admin), callbacks(&restored));
    sqlx::query("UPDATE action_groups SET access_scope = 'admin_only'")
        .execute(&config.db)
        .await?;
    sqlx::query("UPDATE ha_native_targets SET access_scope = 'admin_only'")
        .execute(&config.db)
        .await?;
    let denied = open(&config, ControlPayload::ActionGroupsPage { page: 2 }).await?;
    assert!(!callbacks(&denied).iter().any(|p| matches!(
        p,
        Payload::Control(
            ControlPayload::ActionGroupDetail { .. } | ControlPayload::HaNativeActionDetail { .. }
        )
    )));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn paginated_prompt_keeps_dialogue_state_and_its_cancel_button() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    {
        let mut status = config.runtime_status.write().await;
        status.shutdown_requested_at = Some(Utc::now());
        status.shutdown_reason = Some("Подробный статус системы\n".repeat(400));
    }
    let group = db::action_groups::create_action_group("Свет", &config.db).await?;
    let first = router(
        Payload::Admin(AdminPayload::PromptRenameActionGroup { group }),
        1,
        config.clone(),
    )
    .await?;
    let Payload::TextPage { screen, .. } = first.payload else {
        panic!("Expected pages")
    };
    let second = router(Payload::TextPage { screen, page: 1 }, 1, config.clone()).await?;
    assert!(
        matches!(second.next_state, Some(State::RenameActionGroup { group_id }) if group_id == group)
    );
    assert!(callbacks(&second).contains(&Payload::Admin(AdminPayload::ActionGroupDetail { group })));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn long_command_receipt_survives_pages_without_reexecution_and_is_not_shared(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let mut view = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 1,
            cmd: DeviceCmd::SetTemp(24.0),
        },
    )
    .await?;
    let receipt = format!(
        "Начало результата\n{}\nКонец результата",
        "Подробный результат команды\n".repeat(450)
    );
    view.notice = Some(receipt.clone());
    crate::bot::text_pages::prepare(&mut view, 42, &config).await?;
    let Payload::TextPage { screen, .. } = view.payload else {
        panic!("Expected pages")
    };
    let last = router(
        Payload::TextPage {
            screen,
            page: u16::MAX,
        },
        42,
        config.clone(),
    )
    .await?;
    assert_eq!(last.notice, Some(receipt));
    assert!(last.get_plain_text().contains("Конец результата"));
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    let foreign = router(last.payload.clone(), 1, config.clone()).await?;
    assert!(!foreign.get_plain_text().contains("результат команды"));
    db::access::set_device_access(42, "climate.bedroom", false, false, &config.db).await?;
    let denied = router(last.payload, 42, config.clone()).await?;
    assert!(denied.alert.is_some());
    assert!(denied.notice.is_none());
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
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
    assert!(done.text.contains("Команды отправлены 1 из 1"));
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
    db::add_user(77, &config.db).await?;
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
    db::access::set_device_access(42, "light.bedroom", true, false, &config.db).await?;
    open(&config, ControlPayload::ConfirmAction { id: revoked }).await?;
    db::access::set_device_access(42, "light.bedroom", true, true, &config.db).await?;
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
        )
        .await?
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
    assert!(failed
        .text
        .contains("Результат отправки не удалось определить"));
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
    assert!(labels.contains("Люстра · light.bedroom"), "{labels}");
    assert!(labels.contains("Люстра · light.desk"), "{labels}");
    assert!(
        labels.contains("История: Термостат · Батарея · 97%"),
        "{labels}"
    );
    assert!(labels.contains("автоматический"));
    assert!(!labels.contains("97.00"));
    db::access::set_device_access(42, "light.bedroom", true, false, &config.db).await?;
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
    let manual = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 2,
            cmd: DeviceCmd::EnterManualInput,
        },
    )
    .await?;
    assert!(manual.alert.is_none(), "{:?}", manual.alert);
    assert!(matches!(
        manual.next_state,
        Some(State::WaitingForGraphInterval {
            device_id: 2,
            room_id: 1
        })
    ));
    assert!(ha.calls.lock().unwrap().is_empty());
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
        replies.push(
            config
                .climate_controls
                .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(direction))
                .await?,
        );
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
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?
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
        replies.push(
            config
                .climate_controls
                .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(direction))
                .await?,
        );
    }
    let mut targets = Vec::new();
    for reply in replies {
        targets.push(reply.await??.target);
    }
    assert_eq!(targets, vec![30.0, 30.0, 29.5]);
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    ha.fail_write.store(true, Ordering::SeqCst);
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let first = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(-1))
        .await?;
    ha.write_started.notified().await;
    let second = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(-1))
        .await?;
    ha.release_write.notify_one();
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
    let first = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.write_started.notified(),
    )
    .await?;
    let second = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
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
    assert!(matches!(
        view.payload,
        Payload::Control(ControlPayload::ReviewAction { .. })
    ));
    assert!(view.text.contains("Команды отправлены 1 из 1"));
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

pub(crate) async fn mock_telegram_edits(
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
            let length: Option<usize> = headers.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            });
            let body_bytes = if let Some(length) = length {
                while request.len() < body_start + length {
                    let mut chunk = [0; 4096];
                    let count = stream.read(&mut chunk).await?;
                    anyhow::ensure!(count > 0, "Incomplete HTTP body");
                    request.extend_from_slice(&chunk[..count]);
                }
                request[body_start..body_start + length].to_vec()
            } else {
                // InputFile::file streams multipart uploads with chunked encoding.
                let mut decoded = Vec::new();
                let mut cursor = body_start;
                loop {
                    let end = loop {
                        if let Some(index) = request[cursor..].windows(2).position(|w| w == b"\r\n")
                        {
                            break cursor + index;
                        }
                        let mut chunk = [0; 4096];
                        let count = stream.read(&mut chunk).await?;
                        anyhow::ensure!(count > 0, "Incomplete chunk header");
                        request.extend_from_slice(&chunk[..count]);
                    };
                    let size =
                        usize::from_str_radix(std::str::from_utf8(&request[cursor..end])?, 16)?;
                    cursor = end + 2;
                    if size == 0 {
                        break;
                    }
                    while request.len() < cursor + size + 2 {
                        let mut chunk = [0; 4096];
                        let count = stream.read(&mut chunk).await?;
                        anyhow::ensure!(count > 0, "Incomplete chunk body");
                        request.extend_from_slice(&chunk[..count]);
                    }
                    decoded.extend_from_slice(&request[cursor..cursor + size]);
                    cursor += size + 2;
                }
                decoded
            };
            let bytes = &body_bytes;
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
    assert!(!card.text.contains("climate.bedroom"));
    let details_payload = Payload::DeviceDetails { room: 1, device: 1 };
    assert!(callbacks(&card).contains(&details_payload));
    let details = router(details_payload, 42, config.clone()).await?;
    assert!(details.text.contains("climate.bedroom"));
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
    db::access::set_device_access(42, "light.bedroom", true, false, &config.db).await?;
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
    for payload in &sessions {
        if let Payload::Camera(CameraPayload::RecordingSession { session, .. }) = payload {
            db::camera_recording_sessions::soft_delete_session(*session, &config.db).await?;
        }
    }
    let shortened = router(selection.clone(), 42, config.clone()).await?;
    assert_eq!(
        shortened.payload,
        Payload::Camera(CameraPayload::RecordingArchivePage {
            camera,
            filter: "ready".into(),
            page: 0,
        })
    );
    assert_eq!(
        callbacks(&shortened)
            .iter()
            .filter(|p| matches!(p, Payload::Camera(CameraPayload::RecordingSession { .. })))
            .count(),
        10
    );
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
            text: "*_[]()~".repeat(80),
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
        View {
            text: "Полное содержимое 🏡\n".repeat(500),
            ..graph_update_view()
        },
        config.clone(),
    )
    .await?;
    let requests = server.await??;
    let sent_text = requests[0]["text"].as_str().unwrap();
    assert!(crate::bot::text_pages::text_len(sent_text) <= 4096);
    assert!(sent_text.contains("Страница 1 /"));
    assert!(matches!(
        Payload::from_string(&config.sessions.get(&42).unwrap().current_context)?,
        Payload::TextPage { page: 0, .. }
    ));
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

#[tokio::test]
async fn live_refresh_cannot_overwrite_a_newer_visit_to_the_same_screen() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let context =
        Payload::Control(ControlPayload::DeviceControl { room: 1, device: 1 }).to_string();
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let (bot, mut server) = mock_telegram_edits(vec![(200, telegram_message(7, true))]).await?;
    let job = {
        let config = config.clone();
        let context = context.clone();
        tokio::spawn(async move {
            crate::bot::handlers::refresh_current_view(
                &bot,
                &config,
                42,
                teloxide::types::ChatId(42),
                teloxide::types::MessageId(7),
                &context,
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.read_started.notified(),
    )
    .await?;
    config.cancel_view_update(42);
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let token = config.begin_view_update(42);
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    config.finish_view_update(42, &token);
    ha.release_read.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), job).await???;
    let response = tokio::time::timeout(std::time::Duration::from_millis(100), &mut server).await;
    server.abort();
    assert!(
        response.is_err(),
        "An old HA response was uploaded over a newer visit to the same screen"
    );
    assert_eq!(config.sessions.get(&42).unwrap().current_context, context);
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn cancelled_live_refresh_cannot_clear_session_after_telegram_missing_message(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let context = Payload::Control(ControlPayload::RoomDetail { room: 1 }).to_string();
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (bot, server) = mock_telegram_responses(
        vec![(
            400,
            serde_json::json!({"ok":false,
        "error_code":400,"description":"Bad Request: message to edit not found"}),
        )],
        Some((started.clone(), release.clone())),
    )
    .await?;
    let job = {
        let config = config.clone();
        tokio::spawn(async move {
            crate::bot::handlers::refresh_current_view(
                &bot,
                &config,
                42,
                teloxide::types::ChatId(42),
                teloxide::types::MessageId(7),
                &context,
            )
            .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified()).await?;
    let latest = config.begin_view_update(42);
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), job).await???;
    assert_eq!(server.await??.len(), 1);
    assert!(config.sessions.contains_key(&42));
    assert!(Arc::ptr_eq(&config.view_updates.get(&42).unwrap(), &latest));
    assert!(!latest.is_cancelled());
    assert!(ha.calls.lock().unwrap().is_empty());
    config.finish_view_update(42, &latest);
    Ok(())
}

#[tokio::test]
async fn concurrent_live_refresh_does_not_supersede_loading_screen() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let context = Payload::Control(ControlPayload::RoomDetail { room: 1 }).to_string();
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let token = config.begin_view_update(42);
    let (bot, server) = mock_telegram_edits(vec![]).await?;
    crate::bot::handlers::refresh_current_view(
        &bot,
        &config,
        42,
        teloxide::types::ChatId(42),
        teloxide::types::MessageId(7),
        &context,
    )
    .await?;
    assert!(server.await??.is_empty());
    assert_eq!(ha.state_reads.load(Ordering::SeqCst), 0);
    assert!(Arc::ptr_eq(&config.view_updates.get(&42).unwrap(), &token));
    assert!(!token.is_cancelled());
    config.finish_view_update(42, &token);
    Ok(())
}

#[tokio::test]
async fn group_items_restore_filter_page_and_keep_admin_access() -> anyhow::Result<()> {
    use db::action_groups::ActionGroupItemsFilter;
    let (config, ha) = setup().await?;
    let group = db::action_groups::create_action_group("Свет", &config.db).await?;
    for n in 0..12 {
        let entity_id = format!("light.extra_{n:02}");
        db::devices::sync_device(
            &entity_id,
            "bedroom",
            &format!("Свет {n}"),
            "light",
            &config.db,
        )
        .await?;
        let device_id: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = ?")
            .bind(&entity_id)
            .fetch_one(&config.db)
            .await?;
        db::action_groups::toggle_action_group_item(group, device_id, &config.db).await?;
        ha.states.lock().unwrap().push(entity(&entity_id, "off"));
    }
    let selection = Payload::Admin(AdminPayload::ActionGroupItems {
        group,
        page: 1,
        filter: ActionGroupItemsFilter::Selected,
    });
    let selected = router(selection.clone(), config.root_user, config.clone()).await?;
    assert_eq!(selected.payload, selection);
    let back = callbacks(&selected)
        .into_iter()
        .find(|p| matches!(p, Payload::Admin(AdminPayload::ActionGroupDetail { .. })))
        .unwrap();
    let detail = router(back.clone(), config.root_user, config.clone()).await?;
    assert!(callbacks(&detail).contains(&selection));
    let returned = router(selection.clone(), config.root_user, config.clone()).await?;
    assert_eq!(callbacks(&returned), callbacks(&selected));
    let denied = router(selection, 42, config.clone()).await?;
    assert!(denied.alert.is_some());
    assert!(
        db::settings::get_string(&format!("ui:group-items:42:{group}"), &config.db)
            .await?
            .is_none()
    );
    // Changing the filter is still an explicit choice, even after restoring Selected.
    let all = Payload::Admin(AdminPayload::ActionGroupItems {
        group,
        page: 0,
        filter: ActionGroupItemsFilter::All,
    });
    router(all.clone(), config.root_user, config.clone()).await?;
    let detail = router(back, config.root_user, config.clone()).await?;
    assert!(callbacks(&detail).contains(&all));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn custom_graph_interval_keeps_viewed_date_without_changing_dialogue_format(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.empty_history.store(true, Ordering::SeqCst);
    let previous = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 2,
            cmd: DeviceCmd::ShowChart { h: 72, o: -48 },
        },
    )
    .await?;
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &previous.payload.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let chat = teloxide::types::ChatId(42);
    let dialogue = crate::bot::handlers::MyDialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::new(),
        chat,
    );
    dialogue
        .update(State::WaitingForGraphInterval {
            room_id: 1,
            device_id: 2,
        })
        .await?;
    let message: teloxide::types::Message = serde_json::from_value(serde_json::json!({
        "message_id": 9, "date": 1_700_000_000, "chat": {"id":42,"type":"private"},
        "from": {"id":42,"is_bot":false,"first_name":"Test"}, "text":"48",
    }))?;
    let (bot, server) = mock_telegram_edits(vec![
        (200, serde_json::json!({"ok":true,"result":true})),
        (200, telegram_message(7, true)),
    ])
    .await?;
    crate::bot::handlers::handle_custom_interval(
        bot,
        message,
        config.clone(),
        dialogue.clone(),
        (2, 1),
    )
    .await?;
    assert_eq!(server.await??.len(), 2);
    assert!(dialogue.get().await?.is_none());
    assert_eq!(
        ha.history_requests.lock().unwrap().as_slice(),
        &[(72, -48), (48, -48)]
    );
    assert_eq!(
        Payload::from_string(&config.sessions.get(&42).unwrap().current_context)?,
        Payload::Control(ControlPayload::QuickAction {
            room: 1,
            device: 2,
            cmd: DeviceCmd::ShowChart { h: 48, o: -48 }
        })
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    assert!(!config.view_updates.contains_key(&42));
    Ok(())
}

#[tokio::test]
async fn foreground_result_supersedes_a_loading_live_refresh_of_the_same_card() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    let payload = Payload::Control(ControlPayload::DeviceControl { room: 1, device: 1 });
    let context = payload.to_string();
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let (bot, server) = mock_telegram_edits(vec![(200, telegram_message(7, true))]).await?;
    let job = {
        let config = config.clone();
        let bot = bot.clone();
        tokio::spawn(async move {
            crate::bot::handlers::refresh_current_view(
                &bot,
                &config,
                42,
                teloxide::types::ChatId(42),
                teloxide::types::MessageId(7),
                &context,
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.read_started.notified(),
    )
    .await?;
    crate::bot::handlers::update_view(
        &bot,
        teloxide::types::ChatId(42),
        teloxide::types::MessageId(7),
        42,
        View {
            compact: true,
            text: "Температура: 23°".into(),
            payload,
            ..Default::default()
        },
        config.clone(),
    )
    .await?;
    ha.release_read.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), job).await???;
    assert_eq!(server.await??.len(), 1);
    assert!(!config.view_updates.contains_key(&42));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn archive_filter_picker_is_compact_complete_and_preserves_return_context(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let camera = camera_for_ui_tests(&config).await?;
    let mut rule_ids = Vec::new();
    for _ in 0..30 {
        rule_ids.push(sqlx::query("INSERT INTO camera_recording_rules (name, camera_id) VALUES ('Одинаковое длинное название правила камеры', ?)")
            .bind(camera).execute(&config.db).await?.last_insert_rowid());
    }
    let archive = router(
        Payload::Camera(CameraPayload::RecordingArchivePage {
            camera,
            filter: "ready".into(),
            page: 0,
        }),
        42,
        config.clone(),
    )
    .await?;
    assert_eq!(archive.kb.inline_keyboard[0].len(), 3);
    assert!(!callbacks(&archive).iter().any(|payload| matches!(payload, Payload::Camera(CameraPayload::RecordingArchiveFiltered { filter, .. }) if filter.starts_with("rule:"))));
    let selection = Payload::Camera(CameraPayload::RecordingArchivePage {
        camera,
        filter: "ready".into(),
        page: 2,
    });
    db::settings::set_string(
        &format!("ui:archive:42:{camera}"),
        &selection.to_string(),
        &config.db,
    )
    .await?;
    let filters = router(
        Payload::Camera(CameraPayload::RecordingArchiveFilters { camera }),
        42,
        config.clone(),
    )
    .await?;
    assert!(callbacks(&filters).contains(&selection));
    let mut found = Vec::new();
    for page in 0..3 {
        let rules = router(
            Payload::Camera(CameraPayload::RecordingArchiveRules { camera, page }),
            42,
            config.clone(),
        )
        .await?;
        let choices = callbacks(&rules)
            .into_iter()
            .filter_map(|payload| match payload {
                Payload::Camera(CameraPayload::RecordingArchiveRule { rule, .. }) => Some(rule),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(choices.len(), 10);
        found.extend(choices);
    }
    assert_eq!(found, rule_ids);
    let preview = router(
        Payload::Camera(CameraPayload::RecordingArchiveRule {
            camera,
            rule: rule_ids[29],
            page: 2,
        }),
        42,
        config.clone(),
    )
    .await?;
    assert!(preview
        .text
        .contains("Одинаковое длинное название правила камеры"));
    assert!(
        callbacks(&preview).contains(&Payload::Camera(CameraPayload::RecordingArchiveRules {
            camera,
            page: 2
        }))
    );
    assert_eq!(
        db::settings::get_string(&format!("ui:archive:42:{camera}"), &config.db).await?,
        Some(selection.to_string())
    );
    let apply = Payload::Camera(CameraPayload::RecordingArchiveFiltered {
        camera,
        filter: format!("rule:{}", rule_ids[29]),
    });
    let applied = router(apply, 42, config.clone()).await?;
    assert_eq!(
        applied.payload,
        Payload::Camera(CameraPayload::RecordingArchivePage {
            camera,
            filter: format!("rule:{}", rule_ids[29]),
            page: 0
        })
    );
    assert!(applied.text.contains("Показано: 0"));
    sqlx::query("UPDATE camera_recording_rules SET deleted_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(rule_ids[29])
        .execute(&config.db)
        .await?;
    let deleted = router(applied.payload, 42, config.clone()).await?;
    assert!(deleted.alert.is_some());
    assert_eq!(
        deleted.payload,
        Payload::Camera(CameraPayload::RecordingArchivePage {
            camera,
            filter: "all".into(),
            page: 0
        })
    );
    db::access::set_room_access(42, 1, false, false, &config.db).await?;
    for payload in [
        CameraPayload::RecordingArchiveFilters { camera },
        CameraPayload::RecordingArchiveRules { camera, page: 0 },
        CameraPayload::RecordingArchiveRule {
            camera,
            rule: rule_ids[0],
            page: 0,
        },
    ] {
        let denied = router(Payload::Camera(payload), 42, config.clone()).await?;
        assert!(denied.alert.is_some());
        assert!(!denied.text.contains("Одинаковое"));
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn admin_user_names_and_aliases_keep_identity_pagination_and_root_permissions(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    for id in 100..112 {
        db::add_user(id, &config.db).await?;
        db::user_display::capture(id, "Одинаковое имя", None, Some("same_name"), &config.db)
            .await?;
    }
    let first = router(Payload::Admin(AdminPayload::ListUsers), 1, config.clone()).await?;
    let names: Vec<_> = first
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .filter(|button| button.text.contains("Одинаковое имя"))
        .map(|button| button.text.clone())
        .collect();
    // Root now has its own persisted admission and occupies one row.
    assert_eq!(names.len(), 8);
    assert!(names
        .iter()
        .all(|name| name.contains(" · #") && name.ends_with(" · user")));
    assert!(first
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .any(|button| button.text.contains("Пользователь #42")));
    let second = router(
        Payload::Admin(AdminPayload::ListUsersPage { page: 1 }),
        1,
        config.clone(),
    )
    .await?;
    assert_eq!(
        callbacks(&second)
            .iter()
            .filter(|payload| matches!(payload, Payload::Admin(AdminPayload::UserProfile { .. })))
            .count(),
        4
    );
    let restored = router(Payload::Admin(AdminPayload::ListUsers), 1, config.clone()).await?;
    assert_eq!(callbacks(&restored), callbacks(&second));
    let prompt = router(
        Payload::Admin(AdminPayload::EditUserAlias { id: 111 }),
        1,
        config.clone(),
    )
    .await?;
    assert!(matches!(
        prompt.next_state,
        Some(State::EditUserAlias { user_id: 111 })
    ));
    assert!(callbacks(&prompt).contains(&Payload::Admin(AdminPayload::UserProfile { id: 111 })));
    db::user_display::set_alias(111, Some("Семья"), &config.db).await?;
    let profile = router(
        Payload::Admin(AdminPayload::UserProfile { id: 111 }),
        1,
        config.clone(),
    )
    .await?;
    assert!(profile.text.contains("Семья"));
    assert!(profile.text.contains("ID: 111"));
    assert!(
        callbacks(&profile).contains(&Payload::Admin(AdminPayload::ConfirmDeleteUser { id: 111 }))
    );
    db::access::set_user_role(42, "admin", &config.db).await?;
    let denied = router(
        Payload::Admin(AdminPayload::EditUserAlias { id: 111 }),
        42,
        config.clone(),
    )
    .await?;
    assert!(denied.alert.is_some());
    assert!(!denied.text.contains("Семья"));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[path = "command_tests.rs"]
mod command_ux;

#[path = "schedule_tests.rs"]
mod schedule_ux;

#[path = "recording_wizard_tests.rs"]
mod recording_wizard_tests;

#[path = "personal_tests.rs"]
mod personal_ux;

#[tokio::test]
async fn revoked_user_late_telegram_edit_does_not_restore_admission() -> anyhow::Result<()> {
    for readd in [false, true] {
        let (config, _) = setup().await?;
        let context = Payload::Home.to_string();
        crate::core::update_user_state_with_mode(
            &config,
            42,
            7,
            &context,
            crate::models::UiMessageMode::Text,
        )
        .await;
        let mut view = router(Payload::Home, 42, config.clone()).await?;
        view.text.push_str(&"long screen ".repeat(150));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (bot, server) = mock_telegram_responses(vec![(200, serde_json::json!({"ok": true, "result": {"message_id": 7, "date": 1_700_000_000, "chat": {"id": 42, "type": "private"}, "text": "ok"}}))], Some((started.clone(), release.clone()))).await?;
        let job_config = config.clone();
        let job = tokio::spawn(async move {
            crate::bot::handlers::update_view_cancellable(
                &bot,
                teloxide::types::ChatId(42),
                teloxide::types::MessageId(7),
                42,
                view,
                job_config,
                None,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified()).await?;
        let mut deletion = Payload::Admin(crate::bot::router::AdminPayload::DeleteUser { id: 42 });
        // The real administrative route commits revocation while the edit is outstanding.
        let _ = router(deletion.clone(), 1, config.clone()).await?;
        if readd {
            db::add_user(42, &config.db).await?;
        }
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), job).await???;
        server.await??;
        assert!(!config.sessions.contains_key(&42));
        assert!(db::get_all_active_sessions(&config.db).await?.is_empty());
        assert_eq!(db::user_exists(42, &config.db).await, readd);
        if !readd {
            assert!(router(Payload::Home, 42, config.clone()).await.is_err());
        }
        deletion = Payload::Admin(crate::bot::router::AdminPayload::DeleteUser { id: 1 });
        let root_delete = router(deletion, 1, config.clone()).await?;
        assert!(root_delete.alert.is_some());
        assert!(db::user_exists(1, &config.db).await);
    }
    Ok(())
}

#[tokio::test]
async fn device_rename_form_validates_owner_state_and_updates_cache() -> anyhow::Result<()> {
    use crate::bot::router::{SettingsPayload, State};
    use teloxide::dispatching::dialogue::InMemStorage;
    let (config, _) = setup().await?;
    let payload = Payload::AdminSettings(SettingsPayload::EditName { room: 1, device: 3 });
    let view = router(payload.clone(), 1, config.clone()).await?;
    let Some(State::WaitingForNameDraft {
        device_id: 3,
        room_id: 1,
        draft_id,
    }) = view.next_state
    else {
        panic!("Expected name draft")
    };
    let storage = InMemStorage::<State>::new();
    let dialogue =
        teloxide::dispatching::dialogue::Dialogue::new(storage, teloxide::types::ChatId(1));
    dialogue
        .update(State::WaitingForNameDraft {
            device_id: 3,
            room_id: 1,
            draft_id,
        })
        .await?;
    crate::core::update_user_state_with_mode(
        &config,
        1,
        7,
        &payload.to_string(),
        crate::models::UiMessageMode::Text,
    )
    .await;
    let msg: teloxide::types::Message = serde_json::from_value(
        serde_json::json!({"message_id": 8, "date": 1_700_000_000, "from": {"id":1,"is_bot":false,"first_name":"Root"}, "chat":{"id":1,"type":"private"},"text":"  Новая лампа 🏠_*  "}),
    )?;
    let (bot, server) = mock_telegram_edits(vec![
        (200, serde_json::json!({"ok":true,"result":true})),
        (200, serde_json::json!({"ok":true,"result":{"message_id":7,"date":1_700_000_000,"chat":{"id":1,"type":"private"},"text":"ok"}})),
    ]).await?;
    // A stale form does not write, independently of transport availability.
    dialogue.update(State::Idle).await?;
    crate::bot::handlers::handle_device_name_input(
        bot.clone(),
        msg.clone(),
        config.clone(),
        dialogue.clone(),
        (3, 1, Some(draft_id)),
    )
    .await?;
    assert_ne!(
        db::devices::get_device_by_id(3, &config.db)
            .await?
            .unwrap()
            .alias
            .as_deref(),
        Some("Новая лампа 🏠_*")
    );
    dialogue
        .update(State::WaitingForNameDraft {
            device_id: 3,
            room_id: 1,
            draft_id,
        })
        .await?;
    crate::bot::handlers::handle_device_name_input(
        bot,
        msg,
        config.clone(),
        dialogue,
        (3, 1, Some(draft_id)),
    )
    .await?;
    assert_eq!(
        db::devices::get_device_by_id(3, &config.db)
            .await?
            .unwrap()
            .alias
            .as_deref(),
        Some("Новая лампа 🏠_*")
    );
    assert_eq!(
        config.name_aliases.get("light.bedroom").unwrap().as_str(),
        "Новая лампа 🏠_*"
    );
    server.abort();
    Ok(())
}

#[tokio::test]
async fn climate_shutdown_rejects_queued_work_and_joins_started_ha_request() -> anyhow::Result<()> {
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let started = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.write_started.notified(),
    )
    .await?;
    let queued = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    config.climate_controls.stop_accepting();
    assert!(config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await
        .is_err());
    let joining_config = config.clone();
    let joining = tokio::spawn(async move { joining_config.climate_controls.shutdown().await });
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    // The in-flight service remains alive; its deterministic barrier is released
    // only after admission stopped and the second command entered the queue.
    ha.release_write.notify_one();
    assert!(started.await?.is_ok());
    assert!(queued.await?.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(2), joining).await???;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    config.climate_controls.shutdown().await?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum RevocationDuringDeviceRead {
    Delete,
    DeleteAndReadd,
    DenyControl,
}

async fn assert_device_write_rechecks_access_after_slow_read(
    revocation: RevocationDuringDeviceRead,
    confirmed: bool,
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let mut payload = ControlPayload::QuickAction {
        room: 1,
        device: 3,
        cmd: DeviceCmd::TurnOff,
    };
    if confirmed {
        db::devices::toggle_device_critical(3, &config.db).await?;
        let id = confirmation_id(&open(&config, payload).await?);
        payload = ControlPayload::ConfirmAction { id };
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let request_config = config.clone();
    let request = tokio::spawn(async move { open(&request_config, payload).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.read_started.notified(),
    )
    .await?;
    // The state snapshot was read under the old admission and ACL. Resume it
    // only after the revocation transaction commits, without a timing sleep.
    match revocation {
        RevocationDuringDeviceRead::Delete => db::delete_user(42, &config.db).await?,
        RevocationDuringDeviceRead::DeleteAndReadd => {
            db::delete_user(42, &config.db).await?;
            db::add_user(42, &config.db).await?;
        }
        RevocationDuringDeviceRead::DenyControl => {
            db::access::set_device_access(42, "light.bedroom", true, false, &config.db).await?;
        }
    }
    ha.release_read.notify_one();
    // Both a rejected request and an access-denied view are valid UI outcomes.
    let _outcome = tokio::time::timeout(std::time::Duration::from_secs(2), request).await??;
    assert!(
        ha.calls.lock().unwrap().is_empty(),
        "A stale state read dispatched a physical write: {revocation:?}, confirmed={confirmed}"
    );
    Ok(())
}

#[tokio::test]
async fn device_write_after_slow_read_rejects_deleted_user() -> anyhow::Result<()> {
    assert_device_write_rechecks_access_after_slow_read(RevocationDuringDeviceRead::Delete, false)
        .await
}

#[tokio::test]
async fn device_write_after_slow_read_rejects_previous_admission_after_readd() -> anyhow::Result<()>
{
    assert_device_write_rechecks_access_after_slow_read(
        RevocationDuringDeviceRead::DeleteAndReadd,
        false,
    )
    .await
}

#[tokio::test]
async fn device_write_after_slow_read_rechecks_current_control_permission() -> anyhow::Result<()> {
    assert_device_write_rechecks_access_after_slow_read(
        RevocationDuringDeviceRead::DenyControl,
        false,
    )
    .await
}

#[tokio::test]
async fn confirmed_device_write_after_slow_read_rechecks_admission_and_acl() -> anyhow::Result<()> {
    for revocation in [
        RevocationDuringDeviceRead::Delete,
        RevocationDuringDeviceRead::DeleteAndReadd,
        RevocationDuringDeviceRead::DenyControl,
    ] {
        assert_device_write_rechecks_access_after_slow_read(revocation, true).await?;
    }
    Ok(())
}

#[tokio::test]
async fn text_device_write_after_slow_read_rechecks_admission_and_acl() -> anyhow::Result<()> {
    use crate::core::commands::{execute_text, CommandExecution, CommandSource};
    for revocation in [
        RevocationDuringDeviceRead::Delete,
        RevocationDuringDeviceRead::DeleteAndReadd,
        RevocationDuringDeviceRead::DenyControl,
    ] {
        let (config, ha) = setup().await?;
        ha.pause_next_read.store(true, Ordering::SeqCst);
        let request_config = config.clone();
        let request = tokio::spawn(async move {
            execute_text(
                42,
                false,
                CommandSource::Text,
                "выключи люстра",
                &request_config,
            )
            .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ha.read_started.notified(),
        )
        .await?;
        match revocation {
            RevocationDuringDeviceRead::Delete => db::delete_user(42, &config.db).await?,
            RevocationDuringDeviceRead::DeleteAndReadd => {
                db::delete_user(42, &config.db).await?;
                db::add_user(42, &config.db).await?;
            }
            RevocationDuringDeviceRead::DenyControl => {
                db::access::set_device_access(42, "light.bedroom", true, false, &config.db).await?;
            }
        }
        ha.release_read.notify_one();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), request).await??;
        assert!(
            ha.calls.lock().unwrap().is_empty(),
            "Stale text command reached HA: {revocation:?}"
        );
        match outcome {
            Err(_) => {}
            Ok(CommandExecution::Done { message, .. }) => assert!(
                message.contains("Команды не отправлены"),
                "Unexpected success: {message}"
            ),
            other => panic!("Expected failed text command, got {other:?}"),
        }
    }
    Ok(())
}

#[tokio::test]
async fn stale_recording_group_callback_cannot_mutate_or_log_success_after_revocation(
) -> anyhow::Result<()> {
    for revocation in ["delete", "readd", "scope"] {
        let (config, ha) = setup().await?;
        let group = db::camera_recording_rule_groups::create_group("Guarded", &config.db).await?;
        db::camera_recording_rule_groups::cycle_group_access_scope(group, &config.db).await?;
        let token = config.begin_view_update(42);
        let ctx = RenderContext {
            displayed_resources: Default::default(),
            user_id: 42,
            admission_generation: db::admission_generation(42, &config.db).await?.unwrap(),
            config: config.clone(),
            notifications: vec![],
            is_admin: false,
            lang: Language::Ru,
            settings_origin: SettingsOrigin::Home,
        };
        match revocation {
            "delete" => db::delete_user(42, &config.db).await?,
            "readd" => {
                db::delete_user(42, &config.db).await?;
                db::add_user(42, &config.db).await?;
            }
            "scope" => {
                db::camera_recording_rule_groups::cycle_group_access_scope(group, &config.db)
                    .await?;
            }
            _ => unreachable!(),
        }
        config.cancel_view_update(42);
        assert!(token.is_cancelled());
        let view = router_camera(ctx, CameraPayload::ToggleRecordingGroupMode { group }).await?;
        assert!(
            view.alert.is_some(),
            "stale {revocation} callback was not rejected"
        );
        assert!(view.notice.is_none());
        assert!(
            db::camera_recording_rule_groups::get_group(group, &config.db)
                .await?
                .unwrap()
                .is_enabled()
        );
        let successes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM activity_log WHERE action IN ('rule_group.enable','rule_group.pause') AND status='ok'")
            .fetch_one(&config.db).await?;
        assert_eq!(successes, 0);
        assert!(ha.calls.lock().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn background_redraw_preserves_user_activity_in_memory_and_persisted_session(
) -> anyhow::Result<()> {
    let (config, _) = setup().await?;
    let payload = Payload::DeviceDetails { room: 1, device: 1 };
    let context = payload.to_string();
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let generation = db::admission_generation(42, &config.db).await?.unwrap();
    let inactive_since = Utc::now() - Duration::hours(25);
    db::record_user_activity(42, &generation, inactive_since, &config.db).await?;
    {
        let mut session = config.sessions.get_mut(&42).unwrap();
        session.last_seen_at = inactive_since;
        session.last_persisted_at = Some(inactive_since);
    }
    let (bot, server) = mock_telegram_edits(vec![
        (200, telegram_message(7, true)),
        (200, telegram_message(7, true)),
    ])
    .await?;
    for _ in 0..2 {
        crate::bot::handlers::refresh_current_view(
            &bot,
            &config,
            42,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            &context,
        )
        .await?;
        let session = config.sessions.get(&42).unwrap();
        assert_eq!(session.last_seen_at, inactive_since);
        assert!(session.last_ui_refresh_at.unwrap() > inactive_since);
        drop(session);
        assert_eq!(
            db::user_activity_at(42, &generation, &config.db).await?,
            Some(inactive_since)
        );
    }
    assert_eq!(server.await??.len(), 2);
    // Restart restoration and a later UI reanchor must keep the same inactivity
    // lease, even when they must write the changed message ID and context.
    config.sessions.remove(&42);
    crate::core::update_user_state_for_admission(
        &config,
        42,
        8,
        &context,
        crate::models::UiMessageMode::Photo,
        &generation,
    )
    .await;
    assert_eq!(
        config.sessions.get(&42).unwrap().last_seen_at,
        inactive_since
    );
    let persisted = db::get_all_active_sessions(&config.db).await?;
    assert_eq!(
        persisted
            .iter()
            .find(|(user, _, _, _)| *user == 42)
            .unwrap()
            .3,
        inactive_since
    );
    let cutoff = Utc::now() - Duration::hours(config.session_ttl_hours as i64);
    assert!(config.sessions.get(&42).unwrap().last_seen_at < cutoff);
    crate::core::maintenance::cleanup_expired_sessions(&config).await;
    assert!(!config.sessions.contains_key(&42));
    assert!(db::get_all_active_sessions(&config.db).await?.is_empty());
    crate::core::update_user_state_for_admission(
        &config,
        42,
        9,
        &context,
        crate::models::UiMessageMode::Photo,
        &generation,
    )
    .await;
    assert!(
        !config.sessions.contains_key(&42),
        "late redraw recreated an expired session"
    );
    assert!(db::get_all_active_sessions(&config.db).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn accepted_callback_updates_activity_even_without_a_new_view() -> anyhow::Result<()> {
    let (config, _) = setup().await?;
    let context = Payload::Home.to_string();
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &context,
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let generation = db::admission_generation(42, &config.db).await?.unwrap();
    let inactive_since = Utc::now() - Duration::hours(25);
    db::record_user_activity(42, &generation, inactive_since, &config.db).await?;
    config.sessions.get_mut(&42).unwrap().last_seen_at = inactive_since;
    let update: teloxide::types::Update = serde_json::from_str(&serde_json::json!({
        "update_id": 1, "callback_query": {
            "id": "activity", "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "chat_instance": "private", "data": "expired callback",
            "message": {"message_id": 7, "date": 1700000000, "chat": {"id": 42, "type": "private"}, "text": "Home"}
        }
    }).to_string())?;
    let (bot, server) =
        mock_telegram_edits(vec![(200, serde_json::json!({"ok": true, "result": true}))]).await?;
    let input = teloxide::dptree::deps![
        update,
        bot,
        config.clone(),
        teloxide::dispatching::dialogue::InMemStorage::<State>::new()
    ];
    let outcome = crate::bot::schema().dispatch(input).await;
    assert!(matches!(outcome, std::ops::ControlFlow::Break(Ok(()))));
    let session = config.sessions.get(&42).unwrap();
    assert!(session.last_seen_at > inactive_since);
    assert_eq!(session.last_menu_id, 7);
    assert_eq!(session.current_context, context);
    let activity = session.last_seen_at;
    drop(session);
    assert_eq!(
        db::user_activity_at(42, &generation, &config.db).await?,
        Some(activity)
    );
    assert_eq!(server.await??.len(), 1);
    db::delete_user(42, &config.db).await?;
    db::add_user(42, &config.db).await?;
    assert!(!crate::core::record_user_activity_for_admission(&config, 42, &generation).await?);
    Ok(())
}

#[path = "review_followup_tests.rs"]
mod review_followup_tests;

#[path = "view_access_tests.rs"]
mod view_access_tests;

#[tokio::test]
async fn late_criticality_rejection_reports_no_send_in_both_languages() -> anyhow::Result<()> {
    for (lang, expected) in [
        (Language::Ru, "Команда не отправлена: устройство стало критическим. Откройте действие заново и подтвердите его."),
        (Language::En, "Command not sent: the device became critical. Reopen the action and confirm it."),
    ] {
        for (device, cmd) in [(3, DeviceCmd::TurnOff), (1, DeviceCmd::SetTemp(23.0))] {
            let (config, ha) = setup().await?;
            db::set_user_language(42, lang, &config.db).await?;
            ha.pause_next_read.store(true, Ordering::SeqCst);
            let request_config = config.clone();
            let request = tokio::spawn(async move {
                open(&request_config, ControlPayload::QuickAction { room: 1, device, cmd }).await
            });
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                ha.read_started.notified(),
            ).await?;
            db::devices::toggle_device_critical(device, &config.db).await?;
            ha.release_read.notify_one();
            let view = tokio::time::timeout(std::time::Duration::from_secs(2), request).await???;
            assert_eq!(view.alert.as_deref(), Some(expected));
            assert!(view.notice.is_none());
            assert!(ha.calls.lock().unwrap().is_empty());
            assert!(callbacks(&view).contains(&Payload::Home));
            assert!(!callbacks(&view).iter().any(|payload| matches!(payload,
                Payload::Control(ControlPayload::QuickAction { .. } | ControlPayload::ConfirmAction { .. }))));
            config.climate_controls.shutdown().await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn session_expiration_rechecks_activity_after_waiting_for_lifecycle() -> anyhow::Result<()> {
    let (config, _) = setup().await?;
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Home.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let generation = db::admission_generation(42, &config.db).await?.unwrap();
    let old = Utc::now() - Duration::hours(25);
    db::record_user_activity(42, &generation, old, &config.db).await?;
    config.sessions.get_mut(&42).unwrap().last_seen_at = old;
    let lifecycle = config.admission_lifecycle.lock().await;
    let cleanup = crate::core::maintenance::cleanup_expired_sessions(&config);
    tokio::pin!(cleanup);
    // Candidate collection runs before the first lock await. Refresh activity
    // while cleanup is waiting at precisely that boundary.
    assert!(futures_util::poll!(cleanup.as_mut()).is_pending());
    let active = Utc::now();
    db::record_user_activity(42, &generation, active, &config.db).await?;
    config.sessions.get_mut(&42).unwrap().last_seen_at = active;
    drop(lifecycle);
    cleanup.await;
    assert_eq!(config.sessions.get(&42).unwrap().last_seen_at, active);
    assert_eq!(db::get_all_active_sessions(&config.db).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn light_name_opens_card_and_separate_power_button_keeps_existing_command(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let room = open(&config, ControlPayload::RoomDetail { room: 1 }).await?;
    let link = Payload::Control(ControlPayload::DeviceControl { room: 1, device: 3 });
    let row = room.kb.inline_keyboard.iter().find(|row| {
        row.first().is_some_and(|button| matches!(&button.kind,
            teloxide::types::InlineKeyboardButtonKind::CallbackData(data) if Payload::from_string(data).ok().as_ref()==Some(&link)))
    }).expect("The device name must open its card");
    assert_eq!(row.len(), 2);
    assert!(
        matches!(&row[1].kind, teloxide::types::InlineKeyboardButtonKind::CallbackData(data)
        if Payload::from_string(data).ok()==Some(Payload::Control(ControlPayload::QuickAction { room:1,device:3,cmd:DeviceCmd::TurnOff })))
    );
    let card = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 3 },
    )
    .await?;
    assert_eq!(card.html_title.as_deref(), Some("Люстра"));
    let room_name = db::rooms::get_room_by_id(1, &config.db)
        .await?
        .unwrap()
        .display_name()
        .to_owned();
    assert_eq!(card.html_context.as_deref(), Some(room_name.as_str()));
    assert!(
        callbacks(&card).contains(&Payload::Control(ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::TurnOff
        }))
    );
    assert!(
        ha.calls.lock().unwrap().is_empty(),
        "Opening a room or card cannot turn a device off"
    );
    db::access::set_device_access(42, "light.bedroom", true, false, &config.db).await?;
    let readonly = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 3 },
    )
    .await?;
    assert!(!callbacks(&readonly).iter().any(|payload| matches!(
        payload,
        Payload::Control(ControlPayload::QuickAction { .. })
    )));
    Ok(())
}

#[tokio::test]
async fn light_card_power_action_keeps_critical_confirmation_and_one_time_execution(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    sqlx::query("UPDATE devices SET critical = 1 WHERE id = 3")
        .execute(&config.db)
        .await?;
    let card = open(
        &config,
        ControlPayload::DeviceControl { room: 1, device: 3 },
    )
    .await?;
    let action = callbacks(&card)
        .into_iter()
        .find(|payload| {
            matches!(
                payload,
                Payload::Control(ControlPayload::QuickAction { device: 3, .. })
            )
        })
        .expect("Power action in the light card");
    let review = router(action, 42, config.clone()).await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    let confirmation = callbacks(&review)
        .into_iter()
        .find(|payload| {
            matches!(
                payload,
                Payload::Control(ControlPayload::ConfirmAction { .. })
            )
        })
        .expect("Critical control must ask for confirmation");
    router(confirmation.clone(), 42, config.clone()).await?;
    router(confirmation, 42, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}
