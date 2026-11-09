use super::*;
use crate::ha::models::EntityAttributes;

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
