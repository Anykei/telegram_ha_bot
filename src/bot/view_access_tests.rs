use super::*;
use crate::bot::personal::PersonalPayload;

// Use only alphanumeric characters so Markdown escaping cannot hide a leak
// from the assertion that inspects captured Telegram request bodies.
const PRIVATE_STATE: &str = "PRIVATESTATEVALUE";

fn disclosed_state(ha: &FakeHa) {
    ha.states
        .lock()
        .unwrap()
        .iter_mut()
        .find(|entity| entity.entity_id == "sensor.temperature")
        .unwrap()
        .state = PRIVATE_STATE.into();
}

async fn prepare_delivery(config: &Arc<AppConfig>, view: &View) {
    crate::core::update_user_state_with_mode(
        config,
        42,
        7,
        &view.payload.to_string(),
        if view.text_page.is_some() {
            crate::models::UiMessageMode::Text
        } else {
            crate::models::UiMessageMode::Photo
        },
    )
    .await;
}

async fn deliver(config: &Arc<AppConfig>, view: View) -> anyhow::Result<Vec<serde_json::Value>> {
    let (bot, mut server) = mock_telegram_edits(vec![(200, telegram_message(7, true))]).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        crate::bot::handlers::update_view_cancellable(
            &bot,
            teloxide::types::ChatId(42),
            teloxide::types::MessageId(7),
            42,
            view,
            config.clone(),
            None,
        ),
    )
    .await??;
    match tokio::time::timeout(std::time::Duration::from_millis(100), &mut server).await {
        Ok(result) => result?,
        Err(_) => {
            server.abort();
            let _ = server.await;
            Ok(vec![])
        }
    }
}

fn assert_no_private_data(requests: &[serde_json::Value]) {
    assert!(
        !serde_json::to_string(requests)
            .unwrap()
            .contains(PRIVATE_STATE),
        "Rendered device data crossed the delivery boundary after access changed: {requests:?}"
    );
}

#[tokio::test]
async fn room_and_subscription_data_recheck_acl_after_delayed_ha_read() -> anyhow::Result<()> {
    for payload in [
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
        Payload::Personal(PersonalPayload::Subscription { device: 2 }),
    ] {
        for change in ["room", "device", "entity"] {
            let (config, ha) = setup().await?;
            disclosed_state(&ha);
            ha.pause_next_read.store(true, Ordering::SeqCst);
            let task_config = config.clone();
            let request = payload.clone();
            let task = tokio::spawn(async move { router(request, 42, task_config).await });
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                ha.read_started.notified(),
            )
            .await?;
            match change {
                "room" => db::access::set_room_access(42, 1, false, false, &config.db).await?,
                "device" => {
                    // The room stays open; its ACL alone cannot authorize an
                    // old keyboard containing this now-hidden device.
                    db::access::set_device_access(
                        42,
                        "sensor.temperature",
                        false,
                        false,
                        &config.db,
                    )
                    .await?;
                }
                "entity" => {
                    sqlx::query("UPDATE devices SET entity_id='sensor.replacement' WHERE id=2")
                        .execute(&config.db)
                        .await?;
                }
                _ => unreachable!(),
            }
            ha.release_read.notify_one();
            if let Ok(view) =
                tokio::time::timeout(std::time::Duration::from_secs(2), task).await??
            {
                prepare_delivery(&config, &view).await;
                assert_no_private_data(&deliver(&config, view).await?);
            }
            assert!(ha.calls.lock().unwrap().is_empty());
        }
    }
    Ok(())
}

#[tokio::test]
async fn rendered_room_and_subscription_pages_keep_provenance_until_delivery() -> anyhow::Result<()>
{
    for payload in [
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
        Payload::Personal(PersonalPayload::Subscription { device: 2 }),
    ] {
        for paged in [false, true] {
            let (config, ha) = setup().await?;
            disclosed_state(&ha);
            let mut view = router(payload.clone(), 42, config.clone()).await?;
            assert!(serde_json::to_string(&view)?.contains(PRIVATE_STATE));
            if paged {
                view.text.push_str(&"\nLong screen content".repeat(500));
                crate::bot::text_pages::prepare(&mut view, 42, &config).await?;
                assert!(matches!(view.payload, Payload::TextPage { .. }));
            }
            prepare_delivery(&config, &view).await;
            db::access::set_device_access(42, "sensor.temperature", false, false, &config.db)
                .await?;
            assert_no_private_data(&deliver(&config, view).await?);
        }
    }
    Ok(())
}

#[tokio::test]
async fn room_state_retry_checks_original_device_provenance_after_telegram_delay(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    disclosed_state(&ha);
    let view = router(
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
        42,
        config.clone(),
    )
    .await?;
    prepare_delivery(&config, &view).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (bot, server) = mock_telegram_responses(vec![
        (429, serde_json::json!({"ok":false,"error_code":429,"description":"Too Many Requests","parameters":{"retry_after":0}})),
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
    db::access::set_device_access(42, "sensor.temperature", false, false, &config.db).await?;
    release.notify_one();
    // The first request was authorized. The fake server closes after it, so
    // any retry after revocation would fail this successful completion check.
    tokio::time::timeout(std::time::Duration::from_secs(4), task).await???;
    let requests = server.await??;
    assert_eq!(requests.len(), 1);
    assert!(serde_json::to_string(&requests)?.contains(PRIVATE_STATE));
    Ok(())
}

#[tokio::test]
async fn allowed_room_and_subscription_data_still_reach_telegram() -> anyhow::Result<()> {
    for payload in [
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
        Payload::Personal(PersonalPayload::Subscription { device: 2 }),
    ] {
        let (config, ha) = setup().await?;
        disclosed_state(&ha);
        let view = router(payload, 42, config.clone()).await?;
        prepare_delivery(&config, &view).await;
        let requests = deliver(&config, view).await?;
        assert_eq!(requests.len(), 1);
        assert!(serde_json::to_string(&requests)?.contains(PRIVATE_STATE));
    }
    Ok(())
}

#[tokio::test]
async fn header_device_state_rechecks_view_and_notification_access_at_delivery(
) -> anyhow::Result<()> {
    for revoke_notification_only in [false, true] {
        let (config, _) = setup().await?;
        db::subscriptions::toggle_subscription(42, "sensor.temperature", &config.db).await?;
        db::device_event_log::EventLogger::record_event(
            "sensor.temperature",
            PRIVATE_STATE,
            &config.db,
        )
        .await?;
        let view = router(Payload::Home, 42, config.clone()).await?;
        assert!(view.notifications.iter().any(|item| matches!(&item.source,
            Some(crate::core::types::DisplaySource::Device { id: 2, entity_id }) if entity_id == "sensor.temperature")));
        assert!(view.get_plain_text().contains(PRIVATE_STATE));
        prepare_delivery(&config, &view).await;
        if revoke_notification_only {
            assert!(
                !db::access::toggle_device_notify_access(42, "sensor.temperature", &config.db)
                    .await?
            );
        } else {
            db::access::set_device_access(42, "sensor.temperature", false, false, &config.db)
                .await?;
        }
        assert_no_private_data(&deliver(&config, view).await?);
    }
    Ok(())
}

#[tokio::test]
async fn text_page_receipt_keeps_original_sources_when_new_home_has_none() -> anyhow::Result<()> {
    let (config, _) = setup().await?;
    db::personal::add_favorite(
        42,
        &serde_json::to_string(&crate::bot::personal::FavoriteAction::Device {
            room: 1,
            device: 2,
        })?,
        &config.db,
    )
    .await?;
    let mut original = router(Payload::Home, 42, config.clone()).await?;
    assert!(!original.displayed_resources.is_empty());
    original.notice = Some(format!("Previous device value: {PRIVATE_STATE}"));
    original.text.push_str(&"\nLong home content".repeat(500));
    crate::bot::text_pages::prepare(&mut original, 42, &config).await?;
    assert!(matches!(original.payload, Payload::TextPage { .. }));
    db::access::set_device_access(42, "sensor.temperature", false, false, &config.db).await?;
    let fresh = router(Payload::Home, 42, config.clone()).await?;
    assert!(fresh.displayed_resources.is_empty());
    // Restoring the old receipt must retain its original source, even though
    // the rerendered Home no longer has any device buttons or values.
    let restored = router(original.payload, 42, config.clone()).await?;
    assert!(restored
        .notice
        .as_deref()
        .is_some_and(|notice| notice.contains(PRIVATE_STATE)));
    assert!(!restored.displayed_resources.is_empty());
    prepare_delivery(&config, &restored).await;
    assert_no_private_data(&deliver(&config, restored).await?);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum NamedResource {
    RoomList,
    EmptyRoom,
    Group,
    GroupList,
    Native,
    GroupFavorite,
    NativeFavorite,
    RecordingGroup,
    RecordingGroupList,
}

async fn named_resource_view(
    case: NamedResource,
    config: &Arc<AppConfig>,
    ha: &FakeHa,
) -> anyhow::Result<View> {
    let payload = match case {
        NamedResource::RecordingGroup | NamedResource::RecordingGroupList => {
            let group =
                db::camera_recording_rule_groups::create_group(PRIVATE_STATE, &config.db).await?;
            sqlx::query("UPDATE camera_recording_rule_groups SET access_scope='all_users', enabled=0 WHERE id=?")
                .bind(group).execute(&config.db).await?;
            if matches!(case, NamedResource::RecordingGroupList) {
                Payload::Camera(CameraPayload::RecordingGroupModes)
            } else {
                Payload::Camera(CameraPayload::RecordingGroupModeDetail { group })
            }
        }
        NamedResource::RoomList | NamedResource::EmptyRoom => {
            db::rooms::sync_rooms_from_ha("private-room", PRIVATE_STATE, &config.db).await?;
            match case {
                NamedResource::RoomList => Payload::Control(ControlPayload::ListRooms),
                _ => Payload::Control(ControlPayload::RoomDetail { room: 2 }),
            }
        }
        NamedResource::Group | NamedResource::GroupList | NamedResource::GroupFavorite => {
            let group = db::action_groups::create_action_group(PRIVATE_STATE, &config.db).await?;
            sqlx::query("UPDATE action_groups SET access_scope='all_users' WHERE id=?")
                .bind(group)
                .execute(&config.db)
                .await?;
            match case {
                NamedResource::GroupList => Payload::Control(ControlPayload::ActionGroups),
                NamedResource::GroupFavorite => {
                    db::personal::add_favorite(
                        42,
                        &serde_json::to_string(&crate::bot::personal::FavoriteAction::GroupOn {
                            id: group,
                        })?,
                        &config.db,
                    )
                    .await?;
                    Payload::Home
                }
                _ => Payload::Control(ControlPayload::ActionGroupDetail { group }),
            }
        }
        NamedResource::Native | NamedResource::NativeFavorite => {
            let mut target = entity("script.private", "off");
            target.name = PRIVATE_STATE.into();
            ha.states.lock().unwrap().push(target);
            sqlx::query("INSERT INTO ha_native_targets (entity_id, domain, ha_name, access_scope) VALUES ('script.private', 'script', ?, 'all_users')")
                .bind(PRIVATE_STATE).execute(&config.db).await?;
            if matches!(case, NamedResource::NativeFavorite) {
                db::personal::add_favorite(
                    42,
                    &serde_json::to_string(&crate::bot::personal::FavoriteAction::Native {
                        id: 1,
                    })?,
                    &config.db,
                )
                .await?;
                Payload::Home
            } else {
                Payload::Control(ControlPayload::HaNativeActionDetail { action: 1 })
            }
        }
    };
    router(payload, 42, config.clone()).await
}

async fn revoke_named_resource(case: NamedResource, config: &AppConfig) -> anyhow::Result<()> {
    match case {
        NamedResource::RecordingGroup | NamedResource::RecordingGroupList => {
            sqlx::query("UPDATE camera_recording_rule_groups SET access_scope='admin_only'")
                .execute(&config.db)
                .await?;
        }
        NamedResource::RoomList | NamedResource::EmptyRoom => {
            db::access::set_room_access(42, 2, false, false, &config.db).await?;
        }
        NamedResource::Group | NamedResource::GroupList | NamedResource::GroupFavorite => {
            sqlx::query("UPDATE action_groups SET access_scope='admin_only'")
                .execute(&config.db)
                .await?;
        }
        NamedResource::Native | NamedResource::NativeFavorite => {
            sqlx::query("UPDATE ha_native_targets SET access_scope='admin_only'")
                .execute(&config.db)
                .await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn room_group_and_native_names_recheck_access_before_direct_and_paged_delivery(
) -> anyhow::Result<()> {
    for case in [
        NamedResource::RoomList,
        NamedResource::EmptyRoom,
        NamedResource::Group,
        NamedResource::GroupList,
        NamedResource::Native,
        NamedResource::GroupFavorite,
        NamedResource::NativeFavorite,
        NamedResource::RecordingGroup,
        NamedResource::RecordingGroupList,
    ] {
        for paged in [false, true] {
            let (config, ha) = setup().await?;
            let mut view = named_resource_view(case, &config, &ha).await?;
            assert!(
                serde_json::to_string(&view)?.contains(PRIVATE_STATE),
                "{case:?}"
            );
            assert!(!view.displayed_resources.is_empty(), "{case:?}");
            if paged {
                view.text.push_str(&"\nLong resource details".repeat(500));
                crate::bot::text_pages::prepare(&mut view, 42, &config).await?;
                assert!(matches!(view.payload, Payload::TextPage { .. }));
            }
            prepare_delivery(&config, &view).await;
            revoke_named_resource(case, &config).await?;
            assert_no_private_data(&deliver(&config, view).await?);
            assert!(ha.calls.lock().unwrap().is_empty());
        }
    }
    Ok(())
}

#[tokio::test]
async fn allowed_room_group_and_native_names_still_reach_telegram() -> anyhow::Result<()> {
    for case in [
        NamedResource::RoomList,
        NamedResource::EmptyRoom,
        NamedResource::Group,
        NamedResource::Native,
        NamedResource::GroupFavorite,
        NamedResource::NativeFavorite,
        NamedResource::RecordingGroup,
        NamedResource::RecordingGroupList,
    ] {
        let (config, ha) = setup().await?;
        let view = named_resource_view(case, &config, &ha).await?;
        prepare_delivery(&config, &view).await;
        let requests = deliver(&config, view).await?;
        assert_eq!(requests.len(), 1, "{case:?}");
        assert!(
            serde_json::to_string(&requests)?.contains(PRIVATE_STATE),
            "{case:?}"
        );
        assert!(ha.calls.lock().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn named_resource_delivery_rechecks_disabled_archived_and_native_identity(
) -> anyhow::Result<()> {
    for (case, change) in [
        (NamedResource::Group, "UPDATE action_groups SET enabled=0"),
        (
            NamedResource::Native,
            "UPDATE ha_native_targets SET enabled=0",
        ),
        (
            NamedResource::Native,
            "UPDATE ha_native_targets SET archived=1",
        ),
        (
            NamedResource::Native,
            "UPDATE ha_native_targets SET entity_id='script.replacement'",
        ),
    ] {
        let (config, ha) = setup().await?;
        let view = named_resource_view(case, &config, &ha).await?;
        prepare_delivery(&config, &view).await;
        sqlx::query(change).execute(&config.db).await?;
        assert_no_private_data(&deliver(&config, view).await?);
    }
    Ok(())
}

#[tokio::test]
async fn room_source_preserves_individual_device_grant_and_rechecks_header_breadcrumb(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    sqlx::query("UPDATE rooms SET alias=? WHERE id=1")
        .bind(PRIVATE_STATE)
        .execute(&config.db)
        .await?;
    db::access::set_room_access(42, 1, false, false, &config.db).await?;
    db::access::set_device_access(42, "sensor.temperature", true, false, &config.db).await?;
    let view = router(
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
        42,
        config.clone(),
    )
    .await?;
    prepare_delivery(&config, &view).await;
    assert!(serde_json::to_string(&deliver(&config, view).await?)?.contains(PRIVATE_STATE));

    db::subscriptions::toggle_subscription(42, "sensor.temperature", &config.db).await?;
    db::device_event_log::EventLogger::record_event("sensor.temperature", "21", &config.db).await?;
    let view = router(Payload::Home, 42, config.clone()).await?;
    assert!(view.get_plain_text().contains(PRIVATE_STATE));
    assert!(view.notifications.iter().any(|item| item
        .additional_sources
        .contains(&crate::core::types::DisplaySource::Room { id: 1 })));
    // The device stays visible after moving. Its old room breadcrumb no longer is.
    db::rooms::sync_rooms_from_ha("new-room", "Public room", &config.db).await?;
    sqlx::query("UPDATE devices SET room_id=2 WHERE id=2")
        .execute(&config.db)
        .await?;
    assert!(db::access::can_view_device(42, false, 2, &config.db).await?);
    assert!(!db::rooms::can_open_room(42, false, 1, &config.db).await?);
    prepare_delivery(&config, &view).await;
    assert_no_private_data(&deliver(&config, view).await?);
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn empty_room_and_group_names_capture_sources_before_delayed_ha_read() -> anyhow::Result<()> {
    for case in [NamedResource::EmptyRoom, NamedResource::Group] {
        let (config, ha) = setup().await?;
        let payload = named_resource_view(case, &config, &ha).await?.payload;
        ha.pause_next_read.store(true, Ordering::SeqCst);
        let render_config = config.clone();
        let render = tokio::spawn(async move { router(payload, 42, render_config).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ha.read_started.notified(),
        )
        .await?;
        revoke_named_resource(case, &config).await?;
        ha.release_read.notify_one();
        if let Ok(view) = tokio::time::timeout(std::time::Duration::from_secs(2), render).await?? {
            prepare_delivery(&config, &view).await;
            assert_no_private_data(&deliver(&config, view).await?);
        }
        assert!(ha.calls.lock().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn restored_group_and_native_receipts_keep_the_original_name_sources() -> anyhow::Result<()> {
    for case in [NamedResource::GroupFavorite, NamedResource::NativeFavorite] {
        let (config, ha) = setup().await?;
        let mut original = named_resource_view(case, &config, &ha).await?;
        original.notice = Some(format!("Previous target: {PRIVATE_STATE}"));
        original
            .text
            .push_str(&"\nLong receipt content".repeat(500));
        crate::bot::text_pages::prepare(&mut original, 42, &config).await?;
        revoke_named_resource(case, &config).await?;
        let fresh = router(Payload::Home, 42, config.clone()).await?;
        assert!(fresh.displayed_resources.is_empty());
        let restored = router(original.payload, 42, config.clone()).await?;
        assert!(restored
            .notice
            .as_deref()
            .is_some_and(|value| value.contains(PRIVATE_STATE)));
        assert!(!restored.displayed_resources.is_empty());
        prepare_delivery(&config, &restored).await;
        assert_no_private_data(&deliver(&config, restored).await?);
    }
    Ok(())
}
