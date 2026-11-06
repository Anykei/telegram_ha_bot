use super::*;
use crate::bot::router::{AdminPayload, CameraPayload, SettingsPayload};
use crate::ha::HomeAssistantClient;

async fn mock_config() -> Result<(Arc<AppConfig>, Arc<crate::ha::mock::MockHa>)> {
    let mut config = crate::test_support::config().await;
    let (tx, _) = tokio::sync::mpsc::channel(100);
    let ha = Arc::new(crate::ha::mock::MockHa::open(config.db.clone(), tx).await?);
    ha.seed_inventory().await?;
    let inner = Arc::get_mut(&mut config).unwrap();
    inner.html_ui.enabled = true;
    inner.ha_client = ha.clone();
    db::add_user(1, &config.db).await?;
    let generation = db::admission_generation(1, &config.db).await?.unwrap();
    db::record_user_activity(1, &generation, chrono::Utc::now(), &config.db).await?;
    db::interface::set(1, db::interface::InterfaceMode::Html, &config.db).await?;
    let discovered = ha
        .fetch_action_entities()
        .await?
        .into_iter()
        .map(|e| db::action_groups::HaNativeDiscovery {
            domain: e.entity_id.split('.').next().unwrap().into(),
            entity_id: e.entity_id,
            ha_name: e.name,
        })
        .collect::<Vec<_>>();
    db::action_groups::sync_ha_native_targets(&discovered, true, &config.db).await?;
    Ok((config, ha))
}

async fn device(config: &AppConfig, id: &str) -> Result<(i64, i64)> {
    Ok(
        sqlx::query_as("SELECT room_id, id FROM devices WHERE entity_id=?")
            .bind(id)
            .fetch_one(&config.db)
            .await?,
    )
}

fn assert_complete(view: &View, prepared: &presentation::Prepared) -> Result<()> {
    let rendered = prepared
        .pages
        .iter()
        .flat_map(|p| p.rows.iter().flatten())
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>();
    let original = view
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(rendered, original);
    assert!(prepared
        .pages
        .iter()
        .map(|p| p.body.as_str())
        .collect::<String>()
        .contains(&view.text));
    for page in &prepared.pages {
        assert!(page.json()?.len() <= 1000);
        assert!(serde_json::to_string(&json!({"page":page}))?.len() <= 1024);
    }
    Ok(())
}

fn current(config: &AppConfig) -> WidgetSession {
    config.html_ui.sessions.get(&1).unwrap().clone()
}

fn action_query(config: &AppConfig, payload: &Payload) -> CallbackQuery {
    let session = current(config);
    let (key, _) = session
        .actions
        .iter()
        .find(|(_, p)| *p == payload)
        .expect("Action must come from the rendered screen");
    query(&session, key)
}

async fn press(
    fake: &FakeTelegram,
    config: &Arc<AppConfig>,
    dialogue: &handlers::MyDialogue,
    q: CallbackQuery,
) -> Result<()> {
    // Dispatcher normally initialises an absent dialogue to Idle before invoking a handler.
    if dialogue.get().await?.is_none() {
        dialogue.update(crate::bot::State::Idle).await?;
    }
    handlers::handle_callback(fake.bot.clone(), q, config.clone(), dialogue.clone()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while config.view_updates.contains_key(&1) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn release_navigation_keeps_scenarios_admin_settings_and_camera_controls_in_html(
) -> Result<()> {
    let (config, _) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.floor_lamp").await?;
    let (bedroom, climate) = device(&config, "climate.bedroom").await?;
    let group = db::action_groups::create_action_group("Вечерний свет", &config.db).await?;
    let native: i64 =
        sqlx::query_scalar("SELECT id FROM ha_native_targets WHERE entity_id='scene.evening'")
            .fetch_one(&config.db)
            .await?;
    let camera = sqlx::query("INSERT INTO cameras(key,name,room_id,stream_url) VALUES('virtual','Тестовая камера',?,'mock://camera')")
        .bind(room).execute(&config.db).await?.last_insert_rowid();
    let routes = vec![
        Payload::Home,
        Payload::Control(ControlPayload::ActionGroups),
        Payload::Control(ControlPayload::ActionGroupDetail { group }),
        Payload::Control(ControlPayload::HaNativeActionDetail { action: native }),
        Payload::Control(ControlPayload::DeviceControl {
            room: bedroom,
            device: climate,
        }),
        Payload::GroupDevices { group, page: 0 },
        Payload::Admin(AdminPayload::ListActions),
        Payload::Admin(AdminPayload::ListUsers),
        Payload::Admin(AdminPayload::UserProfile { id: 1 }),
        Payload::Admin(AdminPayload::UserRooms { id: 1 }),
        Payload::Admin(AdminPayload::ActionGroups),
        Payload::Admin(AdminPayload::ActionGroupDetail { group }),
        Payload::Admin(AdminPayload::ActionSchedules {
            target: db::action_groups::ActionTargetRef::BotGroup(group),
        }),
        Payload::Admin(AdminPayload::RecordingRules { room }),
        Payload::Admin(AdminPayload::RoomCameras { room }),
        Payload::AdminSettings(SettingsPayload::ListRooms),
        Payload::AdminSettings(SettingsPayload::DeviceDetail { room, device: lamp }),
        Payload::Camera(CameraPayload::ListCameras),
        Payload::Camera(CameraPayload::CameraDetails { id: camera }),
        Payload::Camera(CameraPayload::RecordingArchive { camera }),
        Payload::Camera(CameraPayload::RecordingArchiveFilters { camera }),
        Payload::Camera(CameraPayload::RecordingGroupModes),
    ];
    for payload in routes {
        let view = router(payload.clone(), 1, config.clone()).await?;
        let prepared = presentation::prepare(&view, "a".repeat(32), 1)
            .with_context(|| format!("Unsupported release route {payload:?}"))?;
        assert_complete(&view, &prepared)?;
        assert!(
            handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view, config.clone(), None)
                .await?
        );
        assert_eq!(
            config.sessions.get(&1).unwrap().ui_message_mode,
            UiMessageMode::Html
        );
    }
    assert!(!fake
        .calls()
        .iter()
        .any(|(method, _)| method.eq_ignore_ascii_case("sendPhoto")));
    Ok(())
}

#[tokio::test]
async fn html_confirmation_executes_once_and_refresh_never_repeats_command() -> Result<()> {
    let (config, ha) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.floor_lamp").await?;
    sqlx::query("UPDATE devices SET critical=1 WHERE id=?")
        .bind(lamp)
        .execute(&config.db)
        .await?;
    publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::DeviceControl { room, device: lamp }),
    )
    .await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let q = action_query(
        &config,
        &Payload::Control(ControlPayload::QuickAction {
            room,
            device: lamp,
            cmd: DeviceCmd::TurnOn,
        }),
    );
    press(&fake, &config, &dialogue, q).await?;
    let review = current(&config);
    assert!(review.pages.iter().all(|p| p.kind == "confirmation"));
    assert!(review
        .pages
        .iter()
        .flat_map(|p| p.rows.iter().flatten())
        .any(|b| b.kind == "danger"));
    assert_eq!(
        ha.fetch_states_by_ids(&["light.floor_lamp".into()]).await?[0].state,
        "off"
    );
    let (key, _) = review
        .actions
        .iter()
        .find(|(_, p)| matches!(p, Payload::Control(ControlPayload::ConfirmAction { .. })))
        .unwrap();
    let confirm = query(&review, key);
    press(&fake, &config, &dialogue, confirm.clone()).await?;
    press(&fake, &config, &dialogue, confirm).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ha_mock_history WHERE entity_id='light.floor_lamp'",
    )
    .fetch_one(&config.db)
    .await?;
    assert_eq!(count, 26, "25 seed states and exactly one command");
    assert_eq!(
        ha.fetch_states_by_ids(&["light.floor_lamp".into()]).await?[0].state,
        "on"
    );
    press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM ha_mock_history WHERE entity_id='light.floor_lamp'"
        )
        .fetch_one(&config.db)
        .await?,
        count
    );
    Ok(())
}
