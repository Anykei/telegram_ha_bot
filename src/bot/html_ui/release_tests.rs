use super::*;
use crate::bot::router::{AdminPayload, CameraPayload, SettingsPayload};
use crate::ha::HomeAssistantClient;

#[path = "graph_tests.rs"]
mod graph_tests;

#[path = "help_tests.rs"]
mod help_tests;

#[path = "home_tests.rs"]
mod home_tests;

#[path = "interaction_tests.rs"]
mod interaction_tests;

#[path = "pending_tests.rs"]
mod pending_tests;

#[path = "camera_tests.rs"]
mod camera_tests;

#[path = "media_pending_tests.rs"]
mod media_pending_tests;

#[path = "origin_tests.rs"]
mod origin_tests;

#[path = "audit_tests.rs"]
mod audit_tests;

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
    assert_buttons_preserved(view, prepared)?;
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
    wait_idle(config).await
}

async fn wait_idle(config: &AppConfig) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while config.view_updates.contains_key(&1) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

fn input_message(text: &str) -> Result<teloxide::types::Message> {
    Ok(serde_json::from_value(
        json!({"message_id":100,"date":1,"from":{"id":1,"is_bot":false,"first_name":"Test"},"chat":{"id":1,"type":"private"},"text":text}),
    )?)
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
async fn pinned_back_returns_from_climate_through_each_parent_level() -> Result<()> {
    let (config, ha) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, device) = device(&config, "climate.bedroom").await?;
    let before = ha.fetch_states_by_ids(&["climate.bedroom".into()]).await?[0]
        .attributes
        .temperature;
    publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::DeviceControl { room, device }),
    )
    .await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    for parent in [
        Payload::Control(ControlPayload::RoomDetail { room }),
        Payload::Control(ControlPayload::ListRooms),
        Payload::Home,
    ] {
        let session = current(&config);
        let key = session.pages[0]
            .back
            .as_deref()
            .context("Missing pinned Back")?;
        assert!(session.pages.iter().all(|p| p.back.as_deref() == Some(key)));
        assert_eq!(session.actions[key], parent);
        // Use the same control from the last content page as from the first.
        let last = query(&session, &format!("p{}", session.pages.len() - 1));
        assert!(resolve(&fake.bot, &last, &config).await?.is_none());
        press(&fake, &config, &dialogue, query(&session, key)).await?;
        assert_eq!(current(&config).view.payload, parent);
    }
    let home = current(&config);
    assert!(home.pages[0].back.is_none());
    assert!(home
        .pages
        .iter()
        .skip(1)
        .all(|p| p.back.as_deref() == Some("p0")));
    assert_eq!(
        ha.fetch_states_by_ids(&["climate.bedroom".into()]).await?[0]
            .attributes
            .temperature,
        before
    );
    Ok(())
}

#[tokio::test]
async fn device_state_and_primary_controls_fit_the_first_server_page() -> Result<()> {
    let (config, _) = mock_config().await?;
    for lang in [crate::i18n::Language::Ru, crate::i18n::Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        for entity in ["climate.bedroom", "light.floor_lamp"] {
            let (room, device) = device(&config, entity).await?;
            let view = router(
                Payload::Control(ControlPayload::DeviceControl { room, device }),
                1,
                config.clone(),
            )
            .await?;
            let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
            assert_complete(&view, &prepared)?;
            let first = &prepared.pages[0];
            assert_eq!(first.body, view.html_summary.as_deref().unwrap());
            assert!(!first.body.contains("Тестовый дом"));
            assert!(!first.body.contains("Test home"));
            for (key, payload) in &prepared.actions {
                if matches!(
                    payload,
                    Payload::TemperatureInput { .. }
                        | Payload::Control(ControlPayload::QuickAction {
                            cmd: DeviceCmd::AdjustTemp(_) | DeviceCmd::TurnOn | DeviceCmd::TurnOff,
                            ..
                        })
                ) {
                    assert!(
                        first.rows.iter().flatten().any(|b| &b.key == key),
                        "Primary action moved off first page: {payload:?}"
                    );
                }
            }
            let wire: Value = serde_json::from_str(&first.json()?)?;
            assert!(wire.get("rows").is_none());
            assert!(wire["b"].as_array().unwrap().iter().all(|row| row
                .as_array()
                .unwrap()
                .iter()
                .all(Value::is_array)));
            let decoded: presentation::Page = serde_json::from_value(wire)?;
            assert_eq!(decoded.rows.len(), first.rows.len());
        }
    }
    let fake = FakeTelegram::new().await?;
    publish(&fake, &config, Payload::Home).await?;
    let calls = fake.calls();
    let sent = &calls
        .iter()
        .find(|(method, _)| method == "sendWidget")
        .unwrap()
        .1;
    assert_eq!(sent["width"], 0);
    assert_eq!(sent["height"], 600);
    assert_eq!(sent["caption"], "Test home · Virtual devices");
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

#[tokio::test]
async fn html_temperature_form_keeps_dialogue_on_read_and_uses_regular_input_validation(
) -> Result<()> {
    let (config, ha) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, climate) = device(&config, "climate.bedroom").await?;
    let card = Payload::Control(ControlPayload::DeviceControl {
        room,
        device: climate,
    });
    publish(&fake, &config, card.clone()).await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let open = Payload::TemperatureInput {
        room,
        device: climate,
    };
    press(&fake, &config, &dialogue, action_query(&config, &open)).await?;
    let form = current(&config);
    assert!(form.pages.iter().all(|p| p.kind == "form"));
    assert!(form.pages[0].hint.contains("сообщением"));
    assert!(form.pages.iter().skip(1).all(|p| p.hint.is_empty()));
    assert!(matches!(
        dialogue.get().await?,
        Some(crate::bot::State::ExactTemperature { .. })
    ));

    fake.clear();
    handlers::refresh_current_view(
        &fake.bot,
        &config,
        1,
        ChatId(1),
        MessageId(form.message),
        &form.view.payload.to_string(),
    )
    .await?;
    assert!(
        fake.calls().is_empty(),
        "Heartbeat must not replace a form or create a new draft"
    );
    press(&fake, &config, &dialogue, query(&form, "read")).await?;
    assert_eq!(current(&config).pages[0].rev, form.pages[0].rev);
    let state = dialogue
        .get()
        .await?
        .context("Form dialogue lost after refresh")?;
    crate::bot::personal::handle_input(
        fake.bot.clone(),
        input_message("не число")?,
        config.clone(),
        dialogue.clone(),
        state.clone(),
    )
    .await?;
    assert!(matches!(
        dialogue.get().await?,
        Some(crate::bot::State::ExactTemperature { .. })
    ));
    assert_eq!(
        ha.fetch_states_by_ids(&["climate.bedroom".into()]).await?[0]
            .attributes
            .temperature,
        Some(22.0)
    );
    crate::bot::personal::handle_input(
        fake.bot.clone(),
        input_message("23,5")?,
        config.clone(),
        dialogue.clone(),
        state,
    )
    .await?;
    wait_idle(&config).await?;
    assert_eq!(
        ha.fetch_states_by_ids(&["climate.bedroom".into()]).await?[0]
            .attributes
            .temperature,
        Some(23.5)
    );
    assert!(current(&config).view.next_state.is_none());
    assert_eq!(current(&config).pages[0].kind, "climate");
    assert!(dialogue.get().await?.is_none());

    press(&fake, &config, &dialogue, action_query(&config, &open)).await?;
    let cancel = action_query(&config, &card);
    press(&fake, &config, &dialogue, cancel.clone()).await?;
    assert!(current(&config).view.next_state.is_none());
    assert!(dialogue.get().await?.is_none());
    assert!(resolve(&fake.bot, &cancel, &config).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn html_schedule_form_pagination_and_read_do_not_create_another_draft() -> Result<()> {
    let (config, _) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let native: i64 =
        sqlx::query_scalar("SELECT id FROM ha_native_targets WHERE entity_id='scene.evening'")
            .fetch_one(&config.db)
            .await?;
    let target = db::action_groups::ActionTargetRef::HaNative(native);
    publish(
        &fake,
        &config,
        Payload::Admin(AdminPayload::ActionSchedules { target }),
    )
    .await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let snapshot = current(&config);
    let (key, _) = snapshot
        .actions
        .iter()
        .find(|(_, p)| {
            matches!(
                p,
                Payload::Admin(AdminPayload::PromptCreateActionScheduleTime { .. })
            )
        })
        .context("Missing create schedule button")?;
    press(&fake, &config, &dialogue, query(&snapshot, key)).await?;
    let form = current(&config);
    assert!(form.view.next_state.is_some());
    let before: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT id, revision, payload FROM ui_drafts WHERE kind=?")
            .bind(db::schedule_drafts::KIND)
            .fetch_all(&config.db)
            .await?;
    assert_eq!(before.len(), 1);
    if form.pages.len() > 1 {
        press(&fake, &config, &dialogue, query(&form, "p1")).await?;
    }
    for _ in 0..2 {
        press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
    }
    let after: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT id, revision, payload FROM ui_drafts WHERE kind=?")
            .bind(db::schedule_drafts::KIND)
            .fetch_all(&config.db)
            .await?;
    assert_eq!(before, after);
    assert!(dialogue.get().await?.is_some());
    Ok(())
}

#[tokio::test]
async fn long_html_forms_preserve_all_content_without_classic_pagination() -> Result<()> {
    let (config, _) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let mut view = router(
        Payload::Personal(PersonalPayload::QuietInput),
        1,
        config.clone(),
    )
    .await?;
    view.text
        .push_str(&"\nПолная инструкция 🏠 без обрезки текста и потери действий.".repeat(220));
    crate::bot::text_pages::prepare(&mut view, 1, &config).await?;
    assert!(view.text_page.is_none());
    let prepared = presentation::prepare(&view, "e".repeat(32), 1)?;
    assert_complete(&view, &prepared)?;
    assert!(prepared.pages.len() > 10);
    assert_eq!(
        prepared.pages.iter().filter(|p| !p.hint.is_empty()).count(),
        1
    );
    assert!(
        handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view.clone(), config.clone(), None)
            .await?
    );
    assert_eq!(current(&config).view.text, view.text);

    db::interface::set(1, db::interface::InterfaceMode::Classic, &config.db).await?;
    crate::bot::text_pages::prepare(&mut view, 1, &config).await?;
    assert!(view.text_page.is_some());
    let text = view.get_plain_text();
    let source_page = presentation::prepare(&view, "e".repeat(32), 1)?;
    assert_eq!(
        source_page
            .pages
            .iter()
            .map(|p| p.body.as_str())
            .collect::<String>(),
        text
    );
    Ok(())
}

#[tokio::test]
async fn long_html_delivery_failure_uses_a_bounded_classic_page_and_saved_context() -> Result<()> {
    let (config, _) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let mut view = router(
        Payload::Personal(PersonalPayload::QuietInput),
        1,
        config.clone(),
    )
    .await?;
    view.text
        .push_str(&"\nДлинная инструкция, все строки должны оставаться доступными.".repeat(200));
    assert!(view.text_page.is_none());
    *fake.fail.lock().unwrap() = Some("sendWidget".into());
    assert!(
        handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view, config.clone(), None).await?
    );
    let calls = fake.calls();
    let (_, sent) = calls
        .iter()
        .find(|(method, _)| method.eq_ignore_ascii_case("sendMessage"))
        .context("Missing classic fallback")?;
    let text = sent["text"].as_str().context("Missing text")?;
    assert!(crate::bot::text_pages::text_len(text) <= 4096);
    let session = config.sessions.get(&1).unwrap();
    assert_eq!(session.ui_message_mode, UiMessageMode::Text);
    assert!(matches!(
        Payload::from_string(&session.current_context)?,
        Payload::TextPage { .. }
    ));
    assert_eq!(
        db::interface::get(1, &config.db).await?,
        db::interface::InterfaceMode::Html
    );
    Ok(())
}
