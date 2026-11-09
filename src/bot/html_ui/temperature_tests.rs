use super::*;
use crate::ha::models::EntityAttributes;

#[tokio::test]
async fn heating_mode_waits_for_exact_ha_mode_and_does_not_change_temperature() -> Result<()> {
    let (config, ha) = config().await?;
    db::devices::sync_device("climate.test", "living", "Термостат", "climate", &config.db).await?;
    ha.states
        .lock()
        .unwrap()
        .insert("climate.test".into(), "off".into());
    ha.attributes.lock().unwrap().insert(
        "climate.test".into(),
        EntityAttributes {
            hvac_modes: Some(vec!["heat".into(), "off".into()]),
            temperature: Some(25.0),
            current_temperature: Some(24.6),
            target_temp_step: Some(0.5),
            min_temp: Some(5.0),
            max_temp: Some(35.0),
            ..Default::default()
        },
    );
    let (room, device) = device(&config, "climate.test").await?;
    let route = temperature::route(room, device);
    let fake = FakeTelegram::new().await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    for (cmd, mode) in [(DeviceCmd::TurnOn, "heat"), (DeviceCmd::TurnOff, "off")] {
        publish(&fake, &config, route.clone()).await?;
        let action = Payload::Control(ControlPayload::QuickAction { room, device, cmd });
        let before = ha.calls.lock().unwrap().len();
        let q = action_query(&config, &action);
        press(&fake, &config, &dialogue, q.clone()).await?;
        assert_eq!(
            ha.calls.lock().unwrap().last().unwrap(),
            "climate:set_hvac_mode:climate.test"
        );
        assert_eq!(
            ha.service_data.lock().unwrap().last().unwrap(),
            &json!({"hvac_mode":mode})
        );
        assert_eq!(current(&config).view.payload, route);
        assert_eq!(
            current(&config).pages[0].slider.as_ref().unwrap().value,
            25000
        );
        assert!(current(&config).pages[0].rows[0]
            .iter()
            .all(|b| b.kind == "wait"));
        press(&fake, &config, &dialogue, q).await?;
        let pending_q = action_query(&config, &action);
        press(&fake, &config, &dialogue, pending_q).await?;
        assert_eq!(ha.calls.lock().unwrap().len(), before + 1);
        // A temperature update or another device's mode must not confirm power.
        ha.attributes
            .lock()
            .unwrap()
            .get_mut("climate.test")
            .unwrap()
            .current_temperature = Some(25.0);
        ha.states
            .lock()
            .unwrap()
            .insert("climate.other".into(), mode.into());
        publish(&fake, &config, route.clone()).await?;
        assert_eq!(current(&config).pages[0].rows[0][1].kind, "wait");
        ha.states
            .lock()
            .unwrap()
            .insert("climate.test".into(), mode.into());
        publish(&fake, &config, route.clone()).await?;
        assert!(current(&config).pages[0].rows[0]
            .iter()
            .all(|b| b.kind == "action"));
    }
    // A previously issued action is revalidated against live HA capabilities.
    let q = action_query(
        &config,
        &Payload::Control(ControlPayload::QuickAction {
            room,
            device,
            cmd: DeviceCmd::TurnOn,
        }),
    );
    ha.attributes
        .lock()
        .unwrap()
        .get_mut("climate.test")
        .unwrap()
        .hvac_modes = Some(vec!["off".into()]);
    press(&fake, &config, &dialogue, q).await?;
    assert_eq!(
        ha.calls.lock().unwrap().len(),
        2,
        "Unsupported mode must not reach HA"
    );
    ha.attributes
        .lock()
        .unwrap()
        .get_mut("climate.test")
        .unwrap()
        .hvac_modes = Some(vec!["heat".into(), "off".into()]);
    publish(&fake, &config, route.clone()).await?;
    ha.fail_service.store(true, Ordering::Relaxed);
    let action = Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::TurnOn,
    });
    let q = action_query(&config, &action);
    press(&fake, &config, &dialogue, q).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 3);
    assert_eq!(current(&config).pages[0].rows[0][1].kind, "uncertain");
    let q = action_query(&config, &action);
    press(&fake, &config, &dialogue, q).await?;
    assert_eq!(
        ha.calls.lock().unwrap().len(),
        3,
        "An uncertain mode write must not retry"
    );
    ha.fail_service.store(false, Ordering::Relaxed);
    press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
    assert_eq!(current(&config).pages[0].rows[0][1].kind, "action");
    assert_eq!(
        ha.calls.lock().unwrap().len(),
        3,
        "Explicit recovery only reads the mode"
    );
    Ok(())
}

#[tokio::test]
async fn temperature_slider_validates_drafts_and_waits_for_ha_target() -> Result<()> {
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
        EntityAttributes {
            temperature: Some(22.0),
            current_temperature: Some(21.0),
            target_temp_step: Some(0.5),
            min_temp: Some(16.0),
            max_temp: Some(30.0),
            unit_of_measurement: Some("°C".into()),
            ..Default::default()
        },
    );
    let (room, device) = device(&config, "climate.test").await?;
    let route = temperature::route(room, device);
    assert_eq!(route.for_refresh(), route);
    let fake = FakeTelegram::new().await?;
    publish(&fake, &config, route.clone()).await?;
    let session = current(&config);
    assert_eq!(session.pages.len(), 1);
    assert!(
        session.view.kb.inline_keyboard.iter().flatten().any(
            |b| matches!(&b.kind, teloxide::types::InlineKeyboardButtonKind::CallbackData(data)
            if Payload::from_string(data).ok()==Some(Payload::TemperatureInput {room, device}))
        ),
        "Full classic keyboard survives HTML delivery failure"
    );
    assert!(
        !session.actions.values().any(|p| matches!(
            p,
            Payload::TemperatureInput { .. }
                | Payload::Control(ControlPayload::QuickAction {
                    cmd: DeviceCmd::AdjustTemp(_),
                    ..
                })
        )),
        "Inline HTML does not duplicate its slider with legacy input controls"
    );
    assert_eq!(session.pages[0].kind, "temperature");
    assert!(session.pages[0].json()?.len() <= 1024);
    assert_eq!(session.pages[0].slider.as_ref().unwrap().value, 22000);
    assert_eq!(renderer(&session.view), TEMPERATURE_HTML);
    assert!(ha.calls.lock().unwrap().is_empty());
    for bad in [
        "s15500",
        "s30500",
        "s22250",
        "sNaN",
        "s22000.0",
        "s+22000",
        "s022000",
        "s2147483648",
    ] {
        assert!(
            temperature::resolve(&session.view, &session.actions, bad).is_none(),
            "{bad}"
        );
    }
    assert!(
        temperature::resolve(&session.view, &std::collections::HashMap::new(), "s23500").is_none()
    );
    let uncertain = presentation::prepare_with_pending(
        &session.view,
        "a".repeat(32),
        1,
        &std::collections::HashMap::from([(device, "uncertain")]),
    )?;
    assert_eq!(uncertain.pages.len(), 1);
    assert!(uncertain.pages[0].alert.contains("HA"));
    assert_eq!(uncertain.pages[0].rows[0][0].kind, "uncertain");
    let apply = temperature::resolve(&session.view, &session.actions, "s23500").unwrap();
    assert_eq!(
        apply,
        Payload::Control(ControlPayload::QuickAction {
            room,
            device,
            cmd: DeviceCmd::SetTemp(23.5)
        })
    );
    assert_eq!(device_write_origin(&config, 1, &apply), Some(route.clone()));
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let old_query = query(&session, "s23500");
    press(&fake, &config, &dialogue, old_query.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(current(&config).view.payload, route);
    assert_eq!(current(&config).pages[0].rows[0][0].kind, "wait");
    press(&fake, &config, &dialogue, old_query).await?;
    press(
        &fake,
        &config,
        &dialogue,
        query(&current(&config), "s24000"),
    )
    .await?;
    assert_eq!(
        ha.calls.lock().unwrap().len(),
        1,
        "No repeated writes while pending"
    );
    ha.attributes
        .lock()
        .unwrap()
        .get_mut("climate.test")
        .unwrap()
        .current_temperature = Some(23.5);
    publish(&fake, &config, route.clone()).await?;
    assert_eq!(
        current(&config).pages[0].slider.as_ref().unwrap().value,
        22000
    );
    assert_eq!(current(&config).pages[0].rows[0][0].kind, "wait");
    ha.attributes
        .lock()
        .unwrap()
        .get_mut("climate.test")
        .unwrap()
        .temperature = Some(23.5);
    publish(&fake, &config, route.clone()).await?;
    assert_eq!(
        current(&config).pages[0].slider.as_ref().unwrap().value,
        23500
    );
    assert_eq!(current(&config).pages[0].rows[0][0].kind, "action");
    ha.states
        .lock()
        .unwrap()
        .insert("climate.test".into(), "unavailable".into());
    let unavailable = router(route, 1, config.clone()).await?;
    assert!(unavailable.html_temperature.is_none());
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    config.climate_controls.stop_accepting();
    config.climate_controls.shutdown().await?;
    Ok(())
}
