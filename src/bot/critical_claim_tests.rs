use super::*;
use crate::core::devices::{CriticalConsent, CriticalConsentRequired, DeviceAction};

async fn wait_for_ha_read(ha: &FakeHa) -> anyhow::Result<()> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.read_started.notified(),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn ordinary_button_rechecks_criticality_after_slow_ha_read() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let request_config = config.clone();
    let request = tokio::spawn(async move {
        open(
            &request_config,
            ControlPayload::QuickAction {
                room: 1,
                device: 3,
                cmd: DeviceCmd::TurnOff,
            },
        )
        .await
    });
    wait_for_ha_read(&ha).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    ha.release_read.notify_one();
    let rejected = request.await??;
    assert!(rejected.alert.is_some());
    assert!(ha.calls.lock().unwrap().is_empty());

    // A fresh, explicitly approved operation still succeeds exactly once.
    let view = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::TurnOff,
        },
    )
    .await?;
    let id = confirmation_id(&view);
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn ordinary_text_command_rechecks_criticality_after_slow_ha_read() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let request_config = config.clone();
    let request = tokio::spawn(async move {
        commands::execute_text(
            42,
            false,
            CommandSource::Text,
            "выключи люстру",
            &request_config,
        )
        .await
    });
    wait_for_ha_read(&ha).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    ha.release_read.notify_one();
    let CommandExecution::Done { message, .. } = request.await?? else {
        panic!("Expected a rejected send result");
    };
    assert!(message.contains("нового подтверждения"), "{message}");
    assert!(ha.calls.lock().unwrap().is_empty());
    let CommandExecution::NeedsConfirmation { pending_id, .. } =
        commands::execute_text(42, false, CommandSource::Text, "выключи люстру", &config).await?
    else {
        panic!();
    };
    commands::confirm_pending(42, false, pending_id, &config).await?;
    commands::confirm_pending(42, false, pending_id, &config).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn queued_temperature_rechecks_criticality_after_slow_ha_read() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let response = config
        .climate_controls
        .enqueue(
            config.clone(),
            42,
            1,
            crate::core::climate_control::TemperatureCommand::Step(1),
        )
        .await?;
    wait_for_ha_read(&ha).await?;
    db::devices::toggle_device_critical(1, &config.db).await?;
    ha.release_read.notify_one();
    let error = response
        .await?
        .err()
        .expect("Unreviewed critical target must be rejected");
    assert!(error.is::<CriticalConsentRequired>());
    assert!(ha.calls.lock().unwrap().is_empty());
    let view = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 1,
            cmd: DeviceCmd::SetTemp(23.0),
        },
    )
    .await?;
    let id = confirmation_id(&view);
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(
        ha.calls.lock().unwrap()[0].2["temperature"],
        serde_json::json!(23.0)
    );
    config.climate_controls.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn critical_consent_is_limited_to_its_device_entity_and_action() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let generation = db::admission_generation(42, &config.db).await?.unwrap();
    for scope in [
        CriticalConsent::new(4, "light.bedroom".into(), DeviceAction::TurnOff),
        CriticalConsent::new(3, "light.other".into(), DeviceAction::TurnOff),
        CriticalConsent::new(3, "light.bedroom".into(), DeviceAction::TurnOn),
    ] {
        let error = crate::core::devices::handle_device_interaction_with_consent(
            &config,
            42,
            &generation,
            3,
            DeviceAction::TurnOff,
            None,
            Some(&scope),
        )
        .await
        .unwrap_err();
        assert!(error.is::<CriticalConsentRequired>());
    }
    let wrong_temperature = CriticalConsent::new(
        1,
        "climate.bedroom".into(),
        DeviceAction::SetTemperature(24.0),
    );
    db::devices::toggle_device_critical(1, &config.db).await?;
    let response = config
        .climate_controls
        .enqueue_confirmed(config.clone(), 42, 1, 23.0, wrong_temperature)
        .await?;
    assert!(response
        .await?
        .err()
        .unwrap()
        .is::<CriticalConsentRequired>());
    assert!(ha.calls.lock().unwrap().is_empty());
    config.climate_controls.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn rendering_rebound_critical_group_cannot_replace_old_buttons_consent() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    let group = confirmation_test_group(&config).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let old_screen = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
    let Payload::Control(ControlPayload::ReviewAction { id: old }) =
        generated_group_buttons(&old_screen).remove(0)
    else {
        panic!();
    };
    sqlx::query("UPDATE devices SET entity_id = 'light.rebound' WHERE id = 3")
        .execute(&config.db)
        .await?;
    ha.states
        .lock()
        .unwrap()
        .iter_mut()
        .find(|entity| entity.entity_id == "light.bedroom")
        .unwrap()
        .entity_id = "light.rebound".into();
    let new_screen = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
    let Payload::Control(ControlPayload::ReviewAction { id: new }) =
        generated_group_buttons(&new_screen).remove(0)
    else {
        panic!();
    };
    assert_ne!(
        old, new,
        "An old callback must retain its original entity binding"
    );
    open(&config, ControlPayload::ConfirmAction { id: old }).await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    let rejected = db::ui_confirmations::get(old, 42, &config.db)
        .await?
        .unwrap();
    let result: ActionResult = serde_json::from_str(&rejected.result_json.unwrap())?;
    assert_eq!(result.outcome, Outcome::Failed);
    open(&config, ControlPayload::ConfirmAction { id: new }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(ha.calls.lock().unwrap()[0].1, "light.rebound");
    Ok(())
}

#[tokio::test]
async fn saved_device_confirmation_keeps_original_room_provenance_after_move() -> anyhow::Result<()>
{
    let (config, _) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let proposal = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::TurnOff,
        },
    )
    .await?;
    let id = confirmation_id(&proposal);
    db::rooms::sync_rooms_from_ha("other", "Other room", &config.db).await?;
    sqlx::query(
        "UPDATE devices SET room_id = (SELECT id FROM rooms WHERE area = 'other') WHERE id = 3",
    )
    .execute(&config.db)
    .await?;
    db::access::set_room_access(42, 1, false, false, &config.db).await?;
    let review = open(&config, ControlPayload::ReviewAction { id }).await?;
    assert!(review
        .displayed_resources
        .contains(&crate::core::types::DisplaySource::Room { id: 1 }));
    assert!(
        !crate::bot::view_access::sources_allowed(&review.displayed_resources, 42, &config).await?
    );
    Ok(())
}

async fn assert_expired_claims_after_wait(
    pool: &sqlx::SqlitePool,
    writer_lock: bool,
) -> anyhow::Result<()> {
    db::add_user(42, pool).await?;
    let expires = Utc::now() + Duration::seconds(2);
    let ui = db::ui_confirmations::create_independent(42, "{}", pool).await?;
    sqlx::query("UPDATE ui_confirmations SET expires_at = ? WHERE id = ?")
        .bind(expires.timestamp())
        .bind(ui)
        .execute(pool)
        .await?;
    let text = db::pending_commands::create(
        42,
        db::pending_commands::PendingCommandSource::Text,
        "test",
        "{}",
        None,
        expires,
        pool,
    )
    .await?;
    let choice = db::command_choices::create_until(42, "{}", expires.timestamp(), pool).await?;
    let save_draft = db::ui_drafts::create(42, "test", "{}", 24, pool).await?;
    let cancel_draft = db::ui_drafts::create(42, "test", "{}", 24, pool).await?;
    let group = db::action_groups::create_action_group("Expiry regression", pool).await?;
    let schedule = db::schedule_drafts::ScheduleDraft {
        target: db::action_groups::ActionTargetRef::BotGroup(group),
        command: db::action_groups::ActionScheduleCommand::TurnOff,
        time_minute: Some(720),
        days_mask: 127,
        enabled: false,
        existing: None,
    };
    let schedule_draft = db::ui_drafts::create(
        42,
        db::schedule_drafts::KIND,
        &serde_json::to_string(&schedule)?,
        24,
        pool,
    )
    .await?;
    sqlx::query("UPDATE ui_drafts SET expires_at = ? WHERE owner_id = 42")
        .bind(expires.timestamp())
        .execute(pool)
        .await?;
    let mut held = pool.acquire().await?;
    if writer_lock {
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *held).await?;
    }
    let ui_pool = pool.clone();
    let ui_claim = tokio::spawn(async move { db::ui_confirmations::claim(ui, 42, &ui_pool).await });
    let text_pool = pool.clone();
    let text_claim =
        tokio::spawn(async move { db::pending_commands::claim(text, 42, &text_pool).await });
    let choice_pool = pool.clone();
    let choice_claim =
        tokio::spawn(async move { db::command_choices::claim(choice, 42, &choice_pool).await });
    let save_pool = pool.clone();
    let save_claim = tokio::spawn(async move {
        db::ui_drafts::save(
            save_draft.id,
            42,
            0,
            "{}",
            "review",
            "",
            true,
            24,
            &save_pool,
        )
        .await
    });
    let cancel_pool = pool.clone();
    let cancel_claim =
        tokio::spawn(
            async move { db::ui_drafts::cancel(cancel_draft.id, 42, 0, &cancel_pool).await },
        );
    let schedule_pool = pool.clone();
    let schedule_claim = tokio::spawn(async move {
        db::schedule_drafts::commit(schedule_draft.id, 42, 0, true, &schedule_pool).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    assert!(!ui_claim.is_finished() && !text_claim.is_finished() && !choice_claim.is_finished());
    assert!(
        !save_claim.is_finished() && !cancel_claim.is_finished() && !schedule_claim.is_finished()
    );
    let remaining = (expires - Utc::now()).to_std().unwrap_or_default();
    tokio::time::sleep(remaining + std::time::Duration::from_millis(50)).await;
    if writer_lock {
        sqlx::query("ROLLBACK").execute(&mut *held).await?;
    }
    drop(held);
    assert!(
        !ui_claim.await??,
        "UI claim must evaluate its deadline after the wait"
    );
    assert!(
        !text_claim.await??,
        "Text claim must evaluate its deadline after the wait"
    );
    assert!(
        !choice_claim.await??,
        "Choice claim must evaluate its deadline after the wait"
    );
    assert!(
        save_claim.await?.is_err(),
        "An expired draft cannot be renewed while waiting"
    );
    assert!(cancel_claim.await?.is_err());
    assert!(
        schedule_claim.await?.is_err(),
        "An expired schedule draft cannot be committed"
    );
    let schedules: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM action_schedules")
        .fetch_one(pool)
        .await?;
    assert_eq!(schedules, 0);
    assert_eq!(
        db::ui_confirmations::get(ui, 42, pool)
            .await?
            .unwrap()
            .status,
        "pending"
    );
    assert_eq!(
        db::pending_commands::get_pending(text, 42, pool)
            .await?
            .unwrap()
            .status,
        "pending"
    );
    Ok(())
}

#[tokio::test]
async fn confirmation_claims_reject_expiry_while_waiting_for_pool_connection() -> anyhow::Result<()>
{
    let config = crate::test_support::config().await;
    assert_expired_claims_after_wait(&config.db, false).await
}

#[tokio::test]
async fn confirmation_claims_reject_expiry_while_waiting_for_sqlite_write_lock(
) -> anyhow::Result<()> {
    let path = std::env::temp_dir().join(format!(
        "telegram_claim_deadline_{}_{}.sqlite",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap()
    ));
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
        .await?
        .run(&pool)
        .await?;
    let result = assert_expired_claims_after_wait(&pool, true).await;
    pool.close().await;
    std::fs::remove_file(path)?;
    result
}
