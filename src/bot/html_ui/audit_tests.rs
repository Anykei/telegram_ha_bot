//! Read-only screen-family parity audit. Graph/calendar substitutions have their
//! own contract tests in graph_tests; this checks the generic adapter end to end.
use super::*;
use crate::bot::personal::FavoriteAction;
use crate::i18n::Language;

#[path = "navigation_tests.rs"]
mod navigation_tests;

#[path = "admin_confirmation_tests.rs"]
mod admin_confirmation_tests;

#[path = "list_navigation_tests.rs"]
mod list_navigation_tests;

#[tokio::test]
async fn screen_family_actions_and_text_survive_html_adaptation() -> Result<()> {
    let (config, _) = mock_config().await?;
    let (room, lamp) = device(&config, "light.floor_lamp").await?;
    let (bedroom, climate) = device(&config, "climate.bedroom").await?;
    let (sensor_room, sensor) = device(&config, "sensor.bedroom_humidity").await?;
    let group = db::action_groups::create_action_group("Вечерний свет", &config.db).await?;
    let native: i64 =
        sqlx::query_scalar("SELECT id FROM ha_native_targets WHERE entity_id='scene.evening'")
            .fetch_one(&config.db)
            .await?;
    let camera = db::cameras::add_manual_camera(
        db::cameras::NewCamera {
            name: "Тестовая камера",
            room_id: room,
            stream_url: "mock://camera",
            snapshot_url: None,
            clip_seconds: 10,
        },
        &config.db,
    )
    .await?;
    config.camera_snapshot_cache.lock().await.insert(
        camera,
        crate::models::CameraSnapshotCache {
            bytes: Some(crate::bot::utils::UI_PLACEHOLDER_BYTES.to_vec()),
            captured_at: Some(chrono::Utc::now()),
            failed_at: Some(chrono::Utc::now()),
            last_warned_at: None,
            last_error: None,
            refreshing: false,
        },
    );
    for target in [
        FavoriteAction::Device { room, device: lamp },
        FavoriteAction::DeviceOn { room, device: lamp },
        FavoriteAction::DeviceOff { room, device: lamp },
        FavoriteAction::Device {
            room: bedroom,
            device: climate,
        },
        FavoriteAction::Device {
            room: sensor_room,
            device: sensor,
        },
        FavoriteAction::Camera { id: camera },
        FavoriteAction::Snapshot { id: camera },
        FavoriteAction::GroupOn { id: group },
        FavoriteAction::GroupOff { id: group },
        FavoriteAction::Native { id: native },
    ] {
        db::personal::add_favorite(1, &serde_json::to_string(&target)?, &config.db).await?;
    }
    let mut routes = vec![
        Payload::Home,
        Payload::Help,
        Payload::Control(ControlPayload::ListRooms),
        Payload::Control(ControlPayload::RoomDetail { room }),
        Payload::Control(ControlPayload::DeviceControl { room, device: lamp }),
        Payload::Control(ControlPayload::DeviceControl {
            room: bedroom,
            device: climate,
        }),
        Payload::DeviceDetails { room, device: lamp },
        Payload::DeviceDetails {
            room: sensor_room,
            device: sensor,
        },
        Payload::TemperatureInput {
            room: bedroom,
            device: climate,
        },
        Payload::Control(ControlPayload::ActionGroups),
        Payload::Control(ControlPayload::ActionGroupDetail { group }),
        Payload::Control(ControlPayload::HaNativeActionDetail { action: native }),
        Payload::GroupDevices { group, page: 0 },
    ];
    routes.extend(
        [
            PersonalPayload::Settings,
            PersonalPayload::Interface,
            PersonalPayload::QuietInput,
            PersonalPayload::Notifications { page: 0 },
            PersonalPayload::Subscription { device: lamp },
            PersonalPayload::Favorites { page: 0 },
            PersonalPayload::FavoriteOptions {
                target: FavoriteAction::Device { room, device: lamp },
            },
        ]
        .into_iter()
        .map(Payload::Personal),
    );
    routes.extend(
        [
            SettingsPayload::ListRooms,
            SettingsPayload::RoomDetail { room },
            SettingsPayload::DeviceDetail { room, device: lamp },
            SettingsPayload::StateAliases { room, device: lamp },
            SettingsPayload::EditName { room, device: lamp },
        ]
        .into_iter()
        .map(Payload::AdminSettings),
    );
    routes.extend(
        [
            CameraPayload::ListCameras,
            CameraPayload::CameraDetail { id: camera },
            CameraPayload::CameraDetails { id: camera },
            CameraPayload::RecordingArchive { camera },
            CameraPayload::RecordingArchiveFilters { camera },
            CameraPayload::RecordingGroupModes,
        ]
        .into_iter()
        .map(Payload::Camera),
    );
    routes.extend(
        [
            AdminPayload::ListActions,
            AdminPayload::ListUsers,
            AdminPayload::UserProfile { id: 1 },
            AdminPayload::UserRooms { id: 1 },
            AdminPayload::UserRoomDevices { id: 1, room },
            AdminPayload::PromptAddUser,
            AdminPayload::CameraRooms,
            AdminPayload::RoomCameras { room },
            AdminPayload::RoomCameraDetail { room, camera },
            AdminPayload::PromptAddCamera { room },
            AdminPayload::ConfirmDeleteCamera { room, camera },
            AdminPayload::CameraHealth { room, camera },
            AdminPayload::RecordingRules { room },
            AdminPayload::RecordingRuleGroups,
            AdminPayload::ActionGroups,
            AdminPayload::ActionGroupDetail { group },
            AdminPayload::ActionSchedules {
                target: db::action_groups::ActionTargetRef::BotGroup(group),
            },
            AdminPayload::UiBackground,
            AdminPayload::Status,
            AdminPayload::ConfirmBackup,
            AdminPayload::ActivityLog {
                filter: crate::bot::router::ActivityLogFilter::All,
            },
        ]
        .into_iter()
        .map(Payload::Admin),
    );
    let mut inventory = Vec::new();
    for lang in [Language::Ru, Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        for route in &routes {
            let view = router(route.clone(), 1, config.clone()).await?;
            let prepared = presentation::prepare(&view, "a".repeat(32), 1)
                .with_context(|| format!("HTML audit: {lang:?} {route:?}"))?;
            assert_complete(&view, &prepared)?;
            if !matches!(route, Payload::Home) {
                assert!(prepared.pages[0].back.is_some(), "Missing exit: {route:?}");
                assert!(
                    !matches!(prepared.pages[0].title.as_str(), "Дом" | "Home"),
                    "Wrong title: {route:?}"
                );
            }
            if view.next_state.is_some() {
                assert_eq!(prepared.pages[0].kind, "form", "{route:?}");
            }
            if matches!(route, Payload::Personal(PersonalPayload::Settings)) {
                let selected = prepared
                    .pages
                    .iter()
                    .flat_map(|p| p.rows.iter().flatten())
                    .filter(|b| b.kind == "selected")
                    .filter_map(|b| match prepared.actions.get(&b.key) {
                        Some(Payload::Personal(PersonalPayload::Language { english })) => {
                            Some(*english)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    selected,
                    vec![lang == Language::En],
                    "Selected language must match the real settings view"
                );
            }
            if matches!(route, Payload::Camera(_)) || view.html_media_controls {
                assert!(
                    prepared
                        .pages
                        .iter()
                        .all(|p| !p.bounded && p.list.is_none()),
                    "Media layout changed: {route:?}"
                );
                let old_back = view
                    .kb
                    .inline_keyboard
                    .iter()
                    .flatten()
                    .rev()
                    .find(|b| b.text == crate::i18n::t(lang, "common.back"));
                if let Some(teloxide::types::InlineKeyboardButton {
                    kind: teloxide::types::InlineKeyboardButtonKind::CallbackData(data),
                    ..
                }) = old_back
                {
                    let key = prepared.pages[0].back.as_deref().context("Media parent")?;
                    assert_eq!(
                        prepared.actions.get(key),
                        Some(&Payload::from_string(data)?),
                        "Media parent changed: {route:?}"
                    );
                }
            }
            // Export optional review evidence using only this isolated mock DB.
            // This catalog complements the targeted navigation/action tests;
            // generic parity alone cannot prove that an action is discoverable.
            inventory.push(json!({
                "route": format!("{route:?}"), "lang": format!("{lang:?}"),
                "pages": prepared.pages,
                "actions": prepared.actions.iter().map(|(key,p)| (key.clone(),format!("{p:?}"))).collect::<std::collections::BTreeMap<_,_>>(),
            }));
        }
    }
    if let Some(path) = std::env::var_os("HA_WIDGET_AUDIT_PATH") {
        std::fs::write(path, serde_json::to_vec_pretty(&inventory)?)?;
    }
    Ok(())
}

#[tokio::test]
async fn diagnostics_text_uses_wire_capacity_without_artificial_short_pages() -> Result<()> {
    let (config, _ha) = mock_config().await?;
    for index in 0..12 {
        db::activity_log::log(
            db::activity_log::NewActivity {
                user_id: Some(1),
                kind: "device",
                entity_type: "device",
                entity_id: Some("light.living"),
                action: "turn_on",
                status: "ok",
                message: Some(&format!("Событие {index}: свет включён")),
            },
            &config.db,
        )
        .await?;
    }
    let mut fixtures = Vec::new();
    for route in [
        Payload::Admin(AdminPayload::Status),
        Payload::Admin(AdminPayload::ActivityLog {
            filter: crate::bot::router::ActivityLogFilter::All,
        }),
    ] {
        let view = router(route.clone(), 1, config.clone()).await?;
        let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
        assert_complete(&view, &prepared)?;
        for (index, page) in prepared.pages.iter().enumerate() {
            fixtures
                .push(json!({"id":format!("{route:?}-{index}"),"renderer":"release","page":page}));
        }
        let first = &prepared.pages[0];
        assert!(
            first.body.chars().count() > 200,
            "{route:?}: prematurely split text"
        );
        assert!(first.json()?.len() > 950, "{route:?}: unused wire capacity");
        assert!(first.back.is_some());
        for page in &prepared.pages {
            assert!(page.json()?.len() <= 1000);
        }
        // A report that fits the wire budget must not be split into small
        // server pages; the client can measure and paginate it if needed.
        let mut short = view.clone();
        short.text = "Состояние: в норме.\n".repeat(12);
        short.kb.inline_keyboard.clear();
        let report = presentation::prepare(&short, "b".repeat(32), 1)?;
        assert_eq!(report.pages.len(), 1, "{route:?}: short report must fit");
        assert_eq!(report.pages[0].body, short.text);
    }
    if let Some(path) = std::env::var_os("HA_WIDGET_DIAGNOSTICS_PATH") {
        std::fs::write(path, serde_json::to_vec_pretty(&fixtures)?)?;
    }
    Ok(())
}
