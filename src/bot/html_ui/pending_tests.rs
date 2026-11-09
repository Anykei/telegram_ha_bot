use super::*;
use crate::core::devices::{handle_device_interaction, DeviceAction};
use std::time::{Duration, Instant};

fn control_kind(config: &AppConfig, device: i64) -> String {
    let session = current(config);
    session.pages.iter().flat_map(|p| p.rows.iter().flatten())
        .find(|b| matches!(session.actions.get(&b.key), Some(Payload::Control(ControlPayload::QuickAction {device: id, cmd, ..}))
            if *id == device && crate::bot::router::action_requires_control(cmd)))
        .expect("Device control must remain visible").kind.clone()
}

#[tokio::test]
async fn thermostat_waits_for_target_attribute_not_room_temperature() -> Result<()> {
    let (config, ha) = config().await?;
    db::devices::sync_device("climate.test", "living", "Термостат", "climate", &config.db).await?;
    sqlx::query("DELETE FROM hidden_entities WHERE entity_id='climate.test'")
        .execute(&config.db)
        .await?;
    ha.states
        .lock()
        .unwrap()
        .insert("climate.test".into(), "heat".into());
    ha.attributes.lock().unwrap().insert(
        "climate.test".into(),
        crate::ha::models::EntityAttributes {
            temperature: Some(22.0),
            current_temperature: Some(21.0),
            target_temp_step: Some(0.5),
            min_temp: Some(5.0),
            max_temp: Some(30.0),
            unit_of_measurement: Some("°C".into()),
            ..Default::default()
        },
    );
    let fake = FakeTelegram::new().await?;
    let (room, climate) = device(&config, "climate.test").await?;
    let route = Payload::Control(ControlPayload::DeviceControl {
        room,
        device: climate,
    });
    publish(&fake, &config, route.clone()).await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let up = Payload::Control(ControlPayload::QuickAction {
        room,
        device: climate,
        cmd: DeviceCmd::AdjustTemp(1),
    });
    press(&fake, &config, &dialogue, action_query(&config, &up)).await?;
    assert_eq!(control_kind(&config, climate), "wait");
    press(&fake, &config, &dialogue, action_query(&config, &up)).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    ha.attributes
        .lock()
        .unwrap()
        .get_mut("climate.test")
        .unwrap()
        .current_temperature = Some(22.5);
    publish(&fake, &config, route.clone()).await?;
    assert_eq!(
        control_kind(&config, climate),
        "wait",
        "Actual room temperature is not the requested setpoint"
    );
    assert_eq!(current(&config).pages[0].status[0][1], "22°C");
    assert_eq!(current(&config).pages[0].status[1][1], "22.5°C");
    ha.attributes
        .lock()
        .unwrap()
        .get_mut("climate.test")
        .unwrap()
        .temperature = Some(22.5);
    publish(&fake, &config, route).await?;
    assert_eq!(control_kind(&config, climate), "action");
    assert_eq!(current(&config).pages[0].status[0][1], "22.5°C");
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    config.climate_controls.stop_accepting();
    config.climate_controls.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn clearing_old_widget_does_not_allow_its_worker_to_release_new_operation() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.living").await?;
    let route = Payload::Control(ControlPayload::RoomDetail { room });
    publish(&fake, &config, route.clone()).await?;
    let generation = current(&config).generation;
    let state = ha
        .fetch_states_by_ids(&["light.living".into()])
        .await?
        .remove(0);
    let old =
        pending::begin(&config, 1, &generation, lamp, &state, &DeviceAction::TurnOn)?.unwrap();
    config.html_ui.clear(1);
    publish(&fake, &config, route.clone()).await?;
    let new =
        pending::begin(&config, 1, &generation, lamp, &state, &DeviceAction::TurnOn)?.unwrap();
    old.finish_after(&config, false, Duration::ZERO);
    tokio::task::yield_now().await;
    assert_eq!(
        config.html_ui.pending.kinds(1, &generation).get(&lamp),
        Some(&"wait")
    );
    new.finish(&config, true);
    sqlx::query("UPDATE devices SET archived=1 WHERE id=?")
        .bind(lamp)
        .execute(&config.db)
        .await?;
    tokio::time::timeout(Duration::from_secs(2), async {
        while config.html_ui.pending.blocks(1, lamp, &generation) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn delayed_state_keeps_command_locked_across_revisions_and_navigation() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.living").await?;
    let room_view = Payload::Control(ControlPayload::RoomDetail { room });
    publish(&fake, &config, room_view.clone()).await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let on = Payload::Control(ControlPayload::QuickAction {
        room,
        device: lamp,
        cmd: DeviceCmd::TurnOn,
    });
    press(&fake, &config, &dialogue, action_query(&config, &on)).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(control_kind(&config, lamp), "wait");
    let first_rev = current(&config).pages[0].rev.clone();
    press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
    assert_ne!(current(&config).pages[0].rev, first_rev);
    assert_eq!(
        control_kind(&config, lamp),
        "wait",
        "An ACK or refresh is not confirmation"
    );
    // Even a forged repeat using the newest revision cannot send another write.
    press(&fake, &config, &dialogue, action_query(&config, &on)).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    let generation = current(&config).generation;
    assert!(
        handle_device_interaction(&config, 1, &generation, lamp, DeviceAction::TurnOn, None)
            .await
            .is_err()
    );
    assert_eq!(
        ha.calls.lock().unwrap().len(),
        1,
        "Dispatch also rejects duplicates"
    );
    let details = Payload::Control(ControlPayload::DeviceControl { room, device: lamp });
    press(&fake, &config, &dialogue, action_query(&config, &details)).await?;
    assert_eq!(
        control_kind(&config, lamp),
        "wait",
        "Navigation cannot clear the pending device"
    );
    ha.states
        .lock()
        .unwrap()
        .insert("light.unrelated".into(), "on".into());
    publish(&fake, &config, room_view.clone()).await?;
    assert_eq!(control_kind(&config, lamp), "wait");
    ha.states
        .lock()
        .unwrap()
        .insert("light.living".into(), "on".into());
    // Exercise the same refresh path used by the existing HA-event worker.
    let session = config.sessions.get(&1).unwrap();
    let (message, context) = (
        MessageId(session.last_menu_id),
        session.current_context.clone(),
    );
    drop(session);
    handlers::refresh_current_view(&fake.bot, &config, 1, ChatId(1), message, &context).await?;
    assert_eq!(control_kind(&config, lamp), "action");
    assert!(!config.html_ui.pending.blocks(1, lamp, &generation));
    assert!(current(&config).actions.values().any(|p| *p
        == Payload::Control(ControlPayload::QuickAction {
            room,
            device: lamp,
            cmd: DeviceCmd::TurnOff
        })));
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn pending_timeout_requires_known_state_and_never_retries_a_command() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.living").await?;
    let route = Payload::Control(ControlPayload::RoomDetail { room });
    publish(&fake, &config, route.clone()).await?;
    let generation = current(&config).generation;
    let state = ha
        .fetch_states_by_ids(&["light.living".into()])
        .await?
        .remove(0);
    let pending =
        pending::begin(&config, 1, &generation, lamp, &state, &DeviceAction::TurnOn)?.unwrap();
    pending.finish_after(&config, true, Duration::from_millis(10));
    tokio::time::timeout(Duration::from_secs(2), async {
        while config.html_ui.pending.kinds(1, &generation).get(&lamp) != Some(&"uncertain") {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    publish(&fake, &config, route.clone()).await?;
    assert_eq!(control_kind(&config, lamp), "uncertain");
    ha.fail_reads.store(true, Ordering::Relaxed);
    config
        .html_ui
        .pending
        .reconcile(&config, 1, &generation)
        .await;
    assert!(config.html_ui.pending.blocks(1, lamp, &generation));
    ha.fail_reads.store(false, Ordering::Relaxed);
    ha.states
        .lock()
        .unwrap()
        .insert("light.living".into(), "unavailable".into());
    config
        .html_ui
        .pending
        .reconcile(&config, 1, &generation)
        .await;
    assert!(config.html_ui.pending.blocks(1, lamp, &generation));
    ha.states
        .lock()
        .unwrap()
        .insert("light.living".into(), "off".into());
    config
        .html_ui
        .pending
        .reconcile(&config, 1, &generation)
        .await;
    publish(&fake, &config, route).await?;
    assert_eq!(control_kind(&config, lamp), "action");
    assert!(ha.calls.lock().unwrap().is_empty(), "Recovery is read-only");
    Ok(())
}

#[tokio::test]
async fn inflight_write_and_old_snapshot_cannot_release_pending_control() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.living").await?;
    let route = Payload::Control(ControlPayload::RoomDetail { room });
    publish(&fake, &config, route.clone()).await?;
    let generation = current(&config).generation;
    let old_read = Instant::now();
    let state = ha
        .fetch_states_by_ids(&["light.living".into()])
        .await?
        .remove(0);
    let pending =
        pending::begin(&config, 1, &generation, lamp, &state, &DeviceAction::TurnOn)?.unwrap();
    ha.states
        .lock()
        .unwrap()
        .insert("light.living".into(), "on".into());
    publish(&fake, &config, route.clone()).await?;
    config
        .html_ui
        .pending
        .reconcile(&config, 1, &generation)
        .await;
    assert_eq!(
        control_kind(&config, lamp),
        "wait",
        "Do not release an in-flight write"
    );
    pending.finish(&config, true);
    let states = ha.fetch_states_by_ids(&["light.living".into()]).await?;
    config
        .html_ui
        .pending
        .observe_states(1, &generation, old_read, &states);
    assert!(config.html_ui.pending.blocks(1, lamp, &generation));
    publish(&fake, &config, route).await?;
    assert_eq!(control_kind(&config, lamp), "action");
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn failed_dispatch_keeps_uncertain_state_until_read_or_late_confirmation() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.living").await?;
    let route = Payload::Control(ControlPayload::RoomDetail { room });
    publish(&fake, &config, route.clone()).await?;
    let generation = current(&config).generation;
    ha.fail_service.store(true, Ordering::Relaxed);
    let outcome =
        handle_device_interaction(&config, 1, &generation, lamp, DeviceAction::TurnOn, None)
            .await?;
    assert!(matches!(
        outcome,
        crate::core::devices::InteractionResult::Error { .. }
    ));
    publish(&fake, &config, route.clone()).await?;
    assert_eq!(control_kind(&config, lamp), "uncertain");
    ha.states
        .lock()
        .unwrap()
        .insert("light.living".into(), "on".into());
    publish(&fake, &config, route).await?;
    assert_eq!(
        control_kind(&config, lamp),
        "action",
        "A late observed state is sufficient"
    );
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    config.html_ui.clear(1);
    assert!(config.html_ui.pending.kinds(1, &generation).is_empty());
    Ok(())
}
