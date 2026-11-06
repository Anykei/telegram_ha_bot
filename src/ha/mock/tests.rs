use super::*;
use crate::bot::router::{router, ControlPayload, DeviceCmd, Payload};
use std::sync::Arc;

async fn setup() -> Result<(MockHa, mpsc::Receiver<NotifyEvent>)> {
    let config = crate::test_support::config().await;
    let (tx, rx) = mpsc::channel(100);
    let mock = MockHa::open(config.db.clone(), tx).await?;
    mock.seed_inventory().await?;
    Ok((mock, rx))
}

async fn state(mock: &MockHa, id: &str) -> Result<Entity> {
    Ok(mock.fetch_states_by_ids(&[id.into()]).await?.remove(0))
}

#[tokio::test]
async fn commands_persist_across_reopen_and_publish_committed_events() -> Result<()> {
    let (mock, mut events) = setup().await?;
    mock.call_service("light", "turn_on", "light.floor_lamp")
        .await?;
    let event = events.try_recv()?;
    assert_eq!(
        (event.old_state.as_str(), event.new_state.as_str()),
        ("off", "on")
    );
    assert_eq!(
        state(&mock, "light.floor_lamp").await?.state,
        event.new_state
    );
    let history = mock.fetch_history("light.floor_lamp", 24, 0).await?;
    assert_eq!(history.points.last().unwrap().1, "on");
    assert!(history.points.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert!(history.points.iter().all(|(at, _)| *at <= history.end_time));

    sqlx::query("UPDATE hidden_entities SET hide = 1 WHERE entity_id = 'light.floor_lamp'")
        .execute(&mock.pool)
        .await?;
    let (tx, _) = mpsc::channel(10);
    let reopened = MockHa::open(mock.pool.clone(), tx).await?;
    reopened.seed_inventory().await?;
    assert_eq!(state(&reopened, "light.floor_lamp").await?.state, "on");
    assert_eq!(
        reopened
            .fetch_history("light.floor_lamp", 24, 0)
            .await?
            .points
            .len(),
        history.points.len()
    );
    let hidden: bool =
        sqlx::query_scalar("SELECT hide FROM hidden_entities WHERE entity_id = 'light.floor_lamp'")
            .fetch_one(&mock.pool)
            .await?;
    assert!(hidden, "Restart preserves manually hidden devices");
    let history = reopened.fetch_history("light.floor_lamp", 24, -48).await?;
    assert!(
        history.points.is_empty(),
        "No history is invented before initial seed"
    );
    Ok(())
}

#[tokio::test]
async fn climate_scenes_and_scripts_change_actual_shared_mock_state() -> Result<()> {
    let (mock, mut events) = setup().await?;
    mock.call_service_with_data(
        "climate",
        "set_temperature",
        "climate.bedroom",
        json!({"temperature":23.5}),
    )
    .await?;
    assert_eq!(
        state(&mock, "climate.bedroom")
            .await?
            .attributes
            .temperature,
        Some(23.5)
    );
    assert_eq!(events.try_recv()?.entity_id, "climate.bedroom");
    mock.call_service("climate", "toggle", "climate.bedroom")
        .await?;
    assert_eq!(state(&mock, "climate.bedroom").await?.state, "off");
    mock.call_service("scene", "turn_on", "scene.evening")
        .await?;
    assert_eq!(state(&mock, "light.floor_lamp").await?.state, "on");
    assert_eq!(state(&mock, "light.ceiling").await?.state, "off");
    mock.call_service("light", "turn_on", "light.kitchen")
        .await?;
    mock.call_service("script", "turn_on", "script.all_lights_off")
        .await?;
    for id in [
        "light.floor_lamp",
        "light.ceiling",
        "light.bedside",
        "light.kitchen",
    ] {
        assert_eq!(state(&mock, id).await?.state, "off");
    }
    assert_eq!(state(&mock, "light.hall").await?.state, "unavailable");
    assert_eq!(mock.fetch_action_entities().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn invalid_commands_fail_without_state_history_or_events() -> Result<()> {
    let (mock, mut events) = setup().await?;
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT entity_id, body FROM ha_mock_entities ORDER BY entity_id")
            .fetch_all(&mock.pool)
            .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ha_mock_history")
        .fetch_one(&mock.pool)
        .await?;
    for (domain, service, id, data) in [
        ("light", "turn_on", "light.missing", json!({})),
        ("light", "turn_on", "light.hall", json!({})),
        ("switch", "turn_on", "light.floor_lamp", json!({})),
        ("light", "explode", "light.floor_lamp", json!({})),
        (
            "binary_sensor",
            "toggle",
            "binary_sensor.front_door",
            json!({}),
        ),
        (
            "climate",
            "set_temperature",
            "climate.bedroom",
            json!({"temperature":99}),
        ),
        (
            "climate",
            "set_temperature",
            "climate.bedroom",
            json!({"temperature":22.1}),
        ),
        (
            "climate",
            "set_temperature",
            "climate.bedroom",
            json!({"temperature":null}),
        ),
        (
            "scene",
            "turn_on",
            "scene.evening",
            json!({"entity_id":"light.floor_lamp"}),
        ),
    ] {
        assert!(mock
            .call_service_with_data(domain, service, id, data)
            .await
            .is_err());
    }
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT entity_id, body FROM ha_mock_entities ORDER BY entity_id")
            .fetch_all(&mock.pool)
            .await?;
    assert_eq!(before, after);
    assert_eq!(
        count,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM ha_mock_history")
            .fetch_one(&mock.pool)
            .await?
    );
    assert!(events.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn regular_router_controls_mock_devices_and_preserves_confirmation() -> Result<()> {
    let mut config = crate::test_support::config().await;
    let (tx, _) = mpsc::channel(100);
    let mock = Arc::new(MockHa::open(config.db.clone(), tx).await?);
    mock.seed_inventory().await?;
    Arc::get_mut(&mut config).unwrap().ha_client = mock.clone();
    crate::db::add_user(1, &config.db).await?;
    let (device, room): (i64, i64) =
        sqlx::query_as("SELECT id, room_id FROM devices WHERE entity_id = 'light.floor_lamp'")
            .fetch_one(&config.db)
            .await?;
    let home = router(Payload::Home, 1, config.clone()).await?;
    assert!(home
        .notifications
        .iter()
        .any(|item| item.label == "Тестовый дом"));
    let room_view = router(
        Payload::Control(ControlPayload::RoomDetail { room }),
        1,
        config.clone(),
    )
    .await?;
    assert!(room_view
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .any(|button| button.text.contains("Торшер")));
    let action = Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::TurnOn,
    });
    router(action.clone(), 1, config.clone()).await?;
    assert_eq!(state(&mock, "light.floor_lamp").await?.state, "on");
    mock.call_service("light", "turn_off", "light.floor_lamp")
        .await?;
    sqlx::query("UPDATE devices SET critical=1 WHERE id=?")
        .bind(device)
        .execute(&config.db)
        .await?;
    let review = router(action, 1, config.clone()).await?;
    assert_eq!(state(&mock, "light.floor_lamp").await?.state, "off");
    let confirmation = review
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .find_map(|button| {
            if let teloxide::types::InlineKeyboardButtonKind::CallbackData(data) = &button.kind {
                if let Ok(payload @ Payload::Control(ControlPayload::ConfirmAction { .. })) =
                    Payload::from_string(data)
                {
                    return Some(payload);
                }
            }
            None
        })
        .context("Missing regular confirmation")?;
    router(confirmation, 1, config.clone()).await?;
    assert_eq!(state(&mock, "light.floor_lamp").await?.state, "on");
    Ok(())
}
