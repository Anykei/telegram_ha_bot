use super::*;
use crate::bot::personal::{FavoriteAction, PersonalPayload};

#[tokio::test]
async fn device_details_and_temperature_recheck_access_after_delayed_state_read(
) -> anyhow::Result<()> {
    for input in [false, true] {
        for revocation in ["view", "delete", "readd"] {
            let (config, ha) = setup().await?;
            let payload = if input {
                Payload::TemperatureInput { room: 1, device: 1 }
            } else {
                Payload::DeviceDetails { room: 1, device: 1 }
            };
            ha.pause_next_read.store(true, Ordering::SeqCst);
            let task_config = config.clone();
            let task = tokio::spawn(async move { router(payload, 42, task_config).await });
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                ha.read_started.notified(),
            )
            .await?;
            match revocation {
                "view" => {
                    db::access::set_device_access(42, "climate.bedroom", false, false, &config.db)
                        .await?
                }
                "delete" => db::delete_user(42, &config.db).await?,
                "readd" => {
                    db::delete_user(42, &config.db).await?;
                    db::add_user(42, &config.db).await?;
                }
                _ => unreachable!(),
            }
            ha.release_read.notify_one();
            if let Ok(view) =
                tokio::time::timeout(std::time::Duration::from_secs(2), task).await??
            {
                assert!(
                    !view.text.contains("climate.bedroom"),
                    "{revocation}: {}",
                    view.text
                );
                assert!(!view.text.contains("22°"), "{revocation}: {}", view.text);
                assert!(
                    !view.text.contains("temperature"),
                    "{revocation}: {}",
                    view.text
                );
                assert!(!matches!(
                    view.next_state,
                    Some(State::ExactTemperature { .. })
                ));
                assert!(view.notifications.is_empty());
            }
            assert!(ha.calls.lock().unwrap().is_empty());
        }
    }
    Ok(())
}

#[tokio::test]
async fn rendered_device_views_recheck_access_at_delivery_including_text_pages(
) -> anyhow::Result<()> {
    for (payload, paged) in [
        (Payload::DeviceDetails { room: 1, device: 1 }, false),
        (Payload::DeviceDetails { room: 1, device: 1 }, true),
        (Payload::TemperatureInput { room: 1, device: 1 }, false),
        (
            Payload::Control(ControlPayload::DeviceControl { room: 1, device: 1 }),
            false,
        ),
    ] {
        for change in ["view", "control", "delete", "readd"] {
            if change == "control" && !matches!(payload, Payload::TemperatureInput { .. }) {
                continue;
            }
            let (config, _) = setup().await?;
            let mut view = router(payload.clone(), 42, config.clone()).await?;
            if paged {
                view.text.push_str(&"\nSensitive device state".repeat(500));
                crate::bot::text_pages::prepare(&mut view, 42, &config).await?;
                assert!(matches!(view.payload, Payload::TextPage { .. }));
            }
            match change {
                "view" => {
                    db::access::set_device_access(42, "climate.bedroom", false, false, &config.db)
                        .await?
                }
                "control" => {
                    db::access::set_device_access(42, "climate.bedroom", true, false, &config.db)
                        .await?
                }
                "delete" => db::delete_user(42, &config.db).await?,
                "readd" => {
                    db::delete_user(42, &config.db).await?;
                    db::add_user(42, &config.db).await?;
                }
                _ => unreachable!(),
            }
            // No server is needed: any attempted Telegram request is a test
            // failure, while the delivery guard must return before transport.
            let client = teloxide::net::default_reqwest_settings()
                .no_proxy()
                .build()?;
            let bot = teloxide::Bot::with_client("123:test", client)
                .set_api_url("http://127.0.0.1:9".parse()?);
            crate::bot::handlers::update_view_cancellable(
                &bot,
                teloxide::types::ChatId(42),
                teloxide::types::MessageId(7),
                42,
                view.clone(),
                config.clone(),
                None,
            )
            .await?;
            assert!(
                !crate::bot::handlers::send_new_view_inner(
                    &bot,
                    teloxide::types::ChatId(42),
                    42,
                    view,
                    config.clone(),
                    None,
                )
                .await?
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn device_view_retry_rechecks_access_after_telegram_delay() -> anyhow::Result<()> {
    let (config, _) = setup().await?;
    let payload = Payload::DeviceDetails { room: 1, device: 1 };
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &payload.to_string(),
        crate::models::UiMessageMode::Text,
    )
    .await;
    let mut view = router(payload, 42, config.clone()).await?;
    view.text.push_str(&"\nDevice attributes".repeat(100));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (bot, server) = mock_telegram_responses(vec![
        (429, serde_json::json!({"ok": false, "error_code": 429, "description": "Too Many Requests", "parameters": {"retry_after": 0}})),
    ], Some((started.clone(), release.clone()))).await?;
    let task_config = config.clone();
    let task = tokio::spawn(async move {
        crate::bot::handlers::update_view_cancellable(
            &bot,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            42,
            view,
            task_config,
            None,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified()).await?;
    db::access::set_device_access(42, "climate.bedroom", false, false, &config.db).await?;
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), task).await???;
    assert_eq!(server.await??.len(), 1);
    Ok(())
}

async fn personal(
    config: &Arc<AppConfig>,
    user: u64,
    request: PersonalPayload,
) -> anyhow::Result<View> {
    router(Payload::Personal(request), user, config.clone()).await
}

#[tokio::test]
async fn own_notifications_save_offline_idempotently_and_obey_admin_denial() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.fail_read.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        let view = personal(
            &config,
            42,
            PersonalPayload::SetSubscription {
                device: 3,
                enabled: true,
            },
        )
        .await?;
        assert!(view.alert.is_none());
        assert!(view.notice.is_some());
    }
    assert!(db::subscriptions::is_subscribed(42, "light.bedroom", &config.db).await?);
    assert!(!db::subscriptions::is_subscribed(1, "light.bedroom", &config.db).await?);
    db::access::set_device_access(42, "light.bedroom", true, true, &config.db).await?;
    db::access::toggle_device_notify_access(42, "light.bedroom", &config.db).await?;
    let disabled = personal(
        &config,
        42,
        PersonalPayload::SetSubscription {
            device: 3,
            enabled: false,
        },
    )
    .await?;
    assert!(disabled.alert.is_none());
    let denied = personal(
        &config,
        42,
        PersonalPayload::SetSubscription {
            device: 3,
            enabled: true,
        },
    )
    .await?;
    assert!(denied.alert.is_some());
    assert!(!db::subscriptions::is_subscribed(42, "light.bedroom", &config.db).await?);
    db::access::set_device_access(42, "light.bedroom", false, false, &config.db).await?;
    let hidden = personal(&config, 42, PersonalPayload::Subscription { device: 3 }).await?;
    assert!(!hidden.text.contains("Люстра"));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn favorites_are_owned_canonical_and_keep_critical_confirmation() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::add_user(77, &config.db).await?;
    let action = FavoriteAction::DeviceOff {
        room: 987654,
        device: 3,
    };
    personal(&config, 42, PersonalPayload::AddFavorite { action }).await?;
    let rows = db::personal::favorites(42, &config.db).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        serde_json::from_str::<FavoriteAction>(&rows[0].action)?,
        FavoriteAction::DeviceOff { room: 1, device: 3 }
    );
    personal(
        &config,
        77,
        PersonalPayload::RemoveFavorite {
            id: rows[0].id,
            page: 0,
        },
    )
    .await?;
    assert_eq!(db::personal::favorites(42, &config.db).await?.len(), 1);
    assert!(db::personal::favorites(77, &config.db).await?.is_empty());
    db::devices::toggle_device_critical(3, &config.db).await?;
    let favorites = personal(&config, 42, PersonalPayload::Favorites { page: 0 }).await?;
    let action = callbacks(&favorites)
        .into_iter()
        .find(|p| {
            matches!(
                p,
                Payload::Control(ControlPayload::QuickAction { device: 3, .. })
            )
        })
        .unwrap();
    let confirmation = router(action, 42, config.clone()).await?;
    assert!(matches!(
        confirmation.payload,
        Payload::Control(ControlPayload::ReviewAction { .. })
    ));
    assert!(ha.calls.lock().unwrap().is_empty());
    db::access::set_device_access(42, "light.bedroom", false, false, &config.db).await?;
    let favorites = personal(&config, 42, PersonalPayload::Favorites { page: 0 }).await?;
    assert!(!callbacks(&favorites).iter().any(|p| matches!(
        p,
        Payload::Control(ControlPayload::QuickAction { device: 3, .. })
    )));
    assert!(!favorites
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .any(|b| b.text.contains("Люстра")));
    Ok(())
}

#[tokio::test]
async fn favorite_device_moving_rooms_keeps_one_stable_saved_target() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let action = FavoriteAction::Device { room: 1, device: 3 };
    personal(
        &config,
        42,
        PersonalPayload::AddFavorite {
            action: action.clone(),
        },
    )
    .await?;
    let id = db::personal::favorites(42, &config.db).await?[0].id;
    db::rooms::sync_rooms_from_ha("kitchen", "Кухня", &config.db).await?;
    sqlx::query("UPDATE devices SET room_id = 2 WHERE id = 3")
        .execute(&config.db)
        .await?;
    personal(&config, 42, PersonalPayload::AddFavorite { action }).await?;
    let rows = db::personal::favorites(42, &config.db).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(
        serde_json::from_str::<FavoriteAction>(&rows[0].action)?,
        FavoriteAction::Device { room: 2, device: 3 }
    );
    let favorites = personal(&config, 42, PersonalPayload::Favorites { page: 0 }).await?;
    assert!(
        callbacks(&favorites).contains(&Payload::Control(ControlPayload::DeviceControl {
            room: 2,
            device: 3
        }))
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn detail_and_temperature_pages_ignore_forged_room_context() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::rooms::sync_rooms_from_ha("private", "Secret room", &config.db).await?;
    db::access::set_room_access(42, 2, false, false, &config.db).await?;
    let details = router(
        Payload::DeviceDetails { room: 2, device: 1 },
        42,
        config.clone(),
    )
    .await?;
    assert_eq!(
        details.payload,
        Payload::DeviceDetails { room: 1, device: 1 }
    );
    let input = router(
        Payload::TemperatureInput { room: 2, device: 1 },
        42,
        config.clone(),
    )
    .await?;
    assert_eq!(
        input.payload,
        Payload::TemperatureInput { room: 1, device: 1 }
    );
    assert!(!input.text.contains("Secret room"));
    assert!(matches!(
        input.next_state,
        Some(State::ExactTemperature {
            room_id: 1,
            device_id: 1
        })
    ));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn paused_favorites_render_safe_links_and_only_visible_actions_get_operations(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::add_user(config.root_user, &config.db).await?;
    for n in 0..12 {
        let id = db::action_groups::create_action_group(&format!("Группа {n}"), &config.db).await?;
        db::action_groups::toggle_action_group_item(id, 3, &config.db).await?;
        db::personal::add_favorite(
            1,
            &serde_json::to_string(&FavoriteAction::GroupOn { id })?,
            &config.db,
        )
        .await?;
        if n == 0 {
            sqlx::query("UPDATE action_groups SET enabled = 0 WHERE id = ?")
                .bind(id)
                .execute(&config.db)
                .await?;
        }
    }
    let home = router(Payload::Home, 1, config.clone()).await?;
    assert!(home
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .any(|b| b.text.contains("Группа 0") && b.text.contains("Недоступно")));
    assert!(
        callbacks(&home).contains(&Payload::Control(ControlPayload::ActionGroupDetail {
            group: 1
        }))
    );
    let operations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ui_confirmations")
        .fetch_one(&config.db)
        .await?;
    assert_eq!(operations, 3);
    personal(&config, 1, PersonalPayload::Favorites { page: 1 }).await?;
    let operations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ui_confirmations")
        .fetch_one(&config.db)
        .await?;
    assert_eq!(operations, 5);
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn unavailable_device_favorite_remains_visible_with_safe_navigation() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    personal(
        &config,
        42,
        PersonalPayload::AddFavorite {
            action: FavoriteAction::DeviceOff { room: 1, device: 3 },
        },
    )
    .await?;
    for fail_read in [false, true] {
        ha.fail_read.store(fail_read, Ordering::SeqCst);
        ha.states.lock().unwrap()[2].state = "unavailable".into();
        let home = router(Payload::Home, 42, config.clone()).await?;
        assert!(home
            .kb
            .inline_keyboard
            .iter()
            .flatten()
            .any(|b| b.text.contains("Люстра") && b.text.contains("HA недоступен")));
        assert!(!callbacks(&home)
            .iter()
            .any(|p| matches!(p, Payload::Control(ControlPayload::QuickAction { .. }))));
    }
    db::access::set_device_access(42, "light.bedroom", false, false, &config.db).await?;
    let before = ha.state_reads.load(Ordering::SeqCst);
    let hidden = router(Payload::Home, 42, config.clone()).await?;
    assert!(!hidden
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .any(|b| b.text.contains("Люстра")));
    assert_eq!(ha.state_reads.load(Ordering::SeqCst), before);
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn deleting_user_clears_owned_ui_state_without_affecting_another_user() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    db::add_user(77, &config.db).await?;
    let group = db::action_groups::create_action_group("Удаление пользователя", &config.db).await?;
    db::action_groups::toggle_action_group_item(group, 3, &config.db).await?;
    sqlx::query("UPDATE action_groups SET access_scope = 'all_users' WHERE id = ?")
        .bind(group)
        .execute(&config.db)
        .await?;
    let mut operations = Vec::new();
    for user in [42_u64, 77] {
        let button = crate::bot::confirmations::prepare_legacy_callback(
            Payload::Control(ControlPayload::ExecuteActionGroup {
                group,
                command: db::action_groups::ActionGroupCommand::TurnOff,
            }),
            user,
            user as i64,
            10,
            config.clone(),
        )
        .await?;
        let Payload::Control(ControlPayload::ConfirmAction { id }) = button else {
            panic!("Expected persisted operation");
        };
        operations.push(id);
        db::personal::add_favorite(
            user,
            &serde_json::to_string(&FavoriteAction::GroupOn { id: group })?,
            &config.db,
        )
        .await?;
        db::personal::set_quiet_interval(user, "22:00–06:00", &config.db).await?;
        db::personal::set_quiet(user, true, &config.db).await?;
        db::ui_drafts::create(user, "recording_rule", "{}", 24, &config.db).await?;
        db::user_display::capture(user, "Имя", None, Some("username"), &config.db).await?;
        db::user_display::set_alias(user, Some("Административное имя"), &config.db).await?;
        db::command_choices::create(user, "{}", &config.db).await?;
        db::pending_commands::create(
            user,
            db::pending_commands::PendingCommandSource::Text,
            "command",
            "{}",
            None,
            Utc::now() + Duration::seconds(60),
            &config.db,
        )
        .await?;
        db::ui_text_pages::register(user, "owned page", &config.db).await?;
        db::set_user_language(user, Language::En, &config.db).await?;
        for namespace in ["list", "chart", "archive", "group-items"] {
            db::settings::set_string(
                &format!("ui:{namespace}:{user}:1"),
                "owned preference",
                &config.db,
            )
            .await?;
        }
    }
    for key in [
        "ui:archive:420:1",
        "ui:other:42:1",
        "ui_background_camera_id",
    ] {
        db::settings::set_string(key, "keep", &config.db).await?;
    }
    let owned_tables = [
        ("ui_confirmations", "user_id"),
        ("pending_commands", "user_id"),
        ("command_choices", "user_id"),
        ("ui_text_pages", "user_id"),
        ("user_favorites", "owner"),
        ("ui_drafts", "owner_id"),
        ("user_display_metadata", "user_id"),
        ("user_notification_schedule", "user_id"),
    ];
    db::delete_user(42, &config.db).await?;
    assert!(!db::user_exists(42, &config.db).await);
    for (table, owner_column) in owned_tables {
        // Identifiers are fixed test constants; only owner values are supplied dynamically.
        let query = format!("SELECT COUNT(*) FROM {table} WHERE {owner_column} = ?");
        let deleted: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(query.as_str()))
            .bind(42)
            .fetch_one(&config.db)
            .await?;
        let retained: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(query.as_str()))
            .bind(77)
            .fetch_one(&config.db)
            .await?;
        assert_eq!(deleted, 0, "{table} retained deleted user's data");
        assert_eq!(retained, 1, "{table} changed another user's data");
    }
    // Legacy callback identities remain as replay tombstones after their
    // executable confirmation is deleted, including across owner re-admission.
    for (user, operation) in [(42_i64, operations[0]), (77, operations[1])] {
        let saved: Vec<i64> =
            sqlx::query_scalar("SELECT confirmation_id FROM ui_action_buttons WHERE user_id = ?")
                .bind(user)
                .fetch_all(&config.db)
                .await?;
        assert_eq!(saved, vec![operation]);
    }
    for namespace in ["list", "chart", "archive", "group-items"] {
        assert!(
            db::settings::get_string(&format!("ui:{namespace}:42:1"), &config.db)
                .await?
                .is_none()
        );
        assert_eq!(
            db::settings::get_string(&format!("ui:{namespace}:77:1"), &config.db)
                .await?
                .as_deref(),
            Some("owned preference")
        );
    }
    for key in [
        "ui:archive:420:1",
        "ui:other:42:1",
        "ui_background_camera_id",
    ] {
        assert_eq!(
            db::settings::get_string(key, &config.db).await?.as_deref(),
            Some("keep")
        );
    }
    assert!(db::ui_confirmations::get(operations[0], 42, &config.db)
        .await?
        .is_none());
    assert!(db::ui_confirmations::get(operations[1], 77, &config.db)
        .await?
        .is_some());
    assert_eq!(
        db::get_user_language(77, &config.db).await?,
        Some(Language::En)
    );
    db::add_user(42, &config.db).await?;
    assert!(db::personal::favorites(42, &config.db).await?.is_empty());
    assert!(db::user_notification_schedule::get(42, &config.db)
        .await?
        .is_none());
    assert!(db::ui_drafts::latest(42, "recording_rule", &config.db)
        .await?
        .is_none());
    assert!(db::user_display::get(42, &config.db)
        .await?
        .admin_alias
        .is_none());
    assert!(!db::ui_confirmations::claim(operations[0], 42, &config.db).await?);
    let replay = crate::bot::confirmations::prepare_legacy_callback(
        Payload::Control(ControlPayload::ExecuteActionGroup {
            group,
            command: db::action_groups::ActionGroupCommand::TurnOff,
        }),
        42,
        42,
        10,
        config.clone(),
    )
    .await?;
    assert_eq!(
        replay,
        Payload::Control(ControlPayload::ConfirmAction { id: operations[0] })
    );
    router(replay, 42, config.clone()).await?;
    assert!(db::ui_confirmations::get(operations[0], 42, &config.db)
        .await?
        .is_none());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}
