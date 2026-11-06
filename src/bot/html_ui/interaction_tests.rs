use super::*;

#[tokio::test]
async fn room_toggle_keeps_device_on_its_page_and_refresh_does_not_repeat_command() -> Result<()> {
    let (config, ha) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, lamp) = device(&config, "light.floor_lamp").await?;
    // Fill a second source list page with real mock targets, then use a later
    // HTML page within it. Exercise both independent pagination layers.
    for n in 0..14 {
        let id = format!("light.pagination_{n}");
        let name = format!("Подсветка рабочего пространства {n}");
        db::devices::sync_device(&id, "living_room", &name, "light", &config.db).await?;
        sqlx::query("UPDATE hidden_entities SET hide=0 WHERE entity_id=?")
            .bind(&id)
            .execute(&config.db)
            .await?;
        sqlx::query("INSERT INTO ha_mock_entities(entity_id,body) VALUES(?,?)")
            .bind(&id)
            .bind(json!({"entity_id":id,"name":name,"state":"off"}).to_string())
            .execute(&config.db)
            .await?;
    }
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    for list_page in [0, 1] {
        publish(
            &fake,
            &config,
            Payload::Control(ControlPayload::RoomPage {
                room,
                page: list_page,
            }),
        )
        .await?;
        let original = current(&config);
        let (index, target) = original
            .pages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, page)| {
                page.rows
                    .iter()
                    .flatten()
                    .find_map(|b| match &original.actions[&b.key] {
                        Payload::Control(ControlPayload::QuickAction {
                            device,
                            cmd: DeviceCmd::TurnOn,
                            ..
                        }) => Some((index, *device)),
                        _ => None,
                    })
            })
            .context("Need a visible mock action")?;
        assert!(index > 0, "Test must exercise a later HTML page");
        assert!(
            resolve(&fake.bot, &query(&original, &format!("p{index}")), &config)
                .await?
                .is_none()
        );
        let before = current(&config);
        let layout = |session: &WidgetSession| {
            session
                .pages
                .iter()
                .map(|p| {
                    p.rows
                        .iter()
                        .flatten()
                        .map(|b| b.key.clone())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let original_layout = layout(&before);
        let command = action_query(
            &config,
            &Payload::Control(ControlPayload::QuickAction {
                room,
                device: target,
                cmd: DeviceCmd::TurnOn,
            }),
        );
        press(&fake, &config, &dialogue, command.clone()).await?;
        let after = current(&config);
        assert_eq!(
            after.view.payload,
            Payload::Control(ControlPayload::RoomDetail { room })
        );
        assert_eq!(after.message, before.message);
        assert_eq!(after.index, index);
        assert_eq!(
            layout(&after),
            original_layout,
            "Command receipt must not repartition controls"
        );
        assert!(after.pages[index]
            .rows
            .iter()
            .flatten()
            .any(|b| after.actions[&b.key]
                == Payload::Control(ControlPayload::QuickAction {
                    room,
                    device: target,
                    cmd: DeviceCmd::TurnOff
                })));
        assert!(!after.pages[index].notice.is_empty());
        press(&fake, &config, &dialogue, command).await?; // old revision cannot replay
        press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
        let refreshed = current(&config);
        assert_eq!(refreshed.index, index);
        assert_eq!(layout(&refreshed), original_layout);
        let entity = db::devices::get_device_by_id(target, &config.db)
            .await?
            .unwrap()
            .entity_id;
        assert_eq!(
            ha.fetch_states_by_ids(std::slice::from_ref(&entity))
                .await?[0]
                .state,
            "on"
        );
        let writes: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM ha_mock_history WHERE entity_id=?")
                .bind(entity)
                .fetch_one(&config.db)
                .await?;
        assert_eq!(writes, 1, "Only one accepted command was executed");
    }
    // Opening a device is still explicit navigation through its name.
    let card = Payload::Control(ControlPayload::DeviceControl { room, device: lamp });
    publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::RoomPage { room, page: 0 }),
    )
    .await?;
    press(&fake, &config, &dialogue, action_query(&config, &card)).await?;
    assert_eq!(current(&config).view.payload, card);
    Ok(())
}
