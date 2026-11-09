use super::*;
use crate::bot::personal::FavoriteAction;

fn back_target(config: &AppConfig) -> Payload {
    let session = current(config);
    let key = session.pages[0]
        .back
        .as_ref()
        .expect("Back must remain pinned");
    session
        .actions
        .get(key)
        .expect("Back must resolve to a read route")
        .clone()
}

async fn seed_origin_lists(config: &AppConfig) -> Result<()> {
    for index in 0..11 {
        let entity = format!("light.origin_{index}");
        db::devices::sync_device(
            &entity,
            "living",
            &format!("Origin {index}"),
            "light",
            &config.db,
        )
        .await?;
        let (room, device) = device(config, &entity).await?;
        db::personal::add_favorite(
            1,
            &serde_json::to_string(&FavoriteAction::Device { room, device })?,
            &config.db,
        )
        .await?;
    }
    sqlx::query("DELETE FROM hidden_entities")
        .execute(&config.db)
        .await?;
    Ok(())
}

#[tokio::test]
async fn origin_preserves_favorite_page_through_device_details_subscription_read_and_write(
) -> Result<()> {
    let (config, ha) = config().await?;
    seed_origin_lists(&config).await?;
    let fake = FakeTelegram::new().await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    publish(&fake, &config, Payload::Home).await?;
    let settings = Payload::Personal(PersonalPayload::Settings);
    press(&fake, &config, &dialogue, action_query(&config, &settings)).await?;
    let favorites = Payload::Personal(PersonalPayload::Favorites { page: 0 });
    press(&fake, &config, &dialogue, action_query(&config, &favorites)).await?;
    assert_eq!(back_target(&config), settings);
    let second = Payload::Personal(PersonalPayload::Favorites { page: 1 });
    press(&fake, &config, &dialogue, action_query(&config, &second)).await?;
    assert_eq!(
        back_target(&config),
        settings,
        "Paging is not a new parent level"
    );
    let target = current(&config)
        .actions
        .values()
        .find(|p| matches!(p, Payload::Control(ControlPayload::DeviceControl { .. })))
        .unwrap()
        .clone();
    let Payload::Control(ControlPayload::DeviceControl { room, device }) = target.clone() else {
        unreachable!()
    };
    press(&fake, &config, &dialogue, action_query(&config, &target)).await?;
    assert_eq!(back_target(&config), second);
    let write = Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::TurnOn,
    });
    press(&fake, &config, &dialogue, action_query(&config, &write)).await?;
    assert_eq!(back_target(&config), second);
    press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
    assert_eq!(back_target(&config), second);
    let details = Payload::DeviceDetails { room, device };
    press(&fake, &config, &dialogue, action_query(&config, &details)).await?;
    assert_eq!(back_target(&config), target);
    press(&fake, &config, &dialogue, action_query(&config, &target)).await?;
    assert_eq!(back_target(&config), second);
    let subscribe = Payload::Personal(PersonalPayload::SetSubscription {
        device,
        enabled: true,
    });
    press(&fake, &config, &dialogue, action_query(&config, &subscribe)).await?;
    assert_eq!(
        back_target(&config),
        target,
        "Subscription opened by main card returns to main card"
    );
    press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
    assert_eq!(back_target(&config), target);
    press(&fake, &config, &dialogue, action_query(&config, &target)).await?;
    assert_eq!(
        back_target(&config),
        second,
        "Returning from details must retain the favorite origin"
    );
    press(&fake, &config, &dialogue, action_query(&config, &second)).await?;
    assert_eq!(back_target(&config), settings);
    assert_eq!(
        ha.calls.lock().unwrap().len(),
        1,
        "Back/Read never repeat the device write"
    );
    config.html_ui.clear(1);
    Ok(())
}

#[tokio::test]
async fn origin_subscription_returns_to_notification_page_and_home_clears_the_trail() -> Result<()>
{
    let (config, ha) = config().await?;
    seed_origin_lists(&config).await?;
    let fake = FakeTelegram::new().await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    publish(&fake, &config, Payload::Personal(PersonalPayload::Settings)).await?;
    let first = Payload::Personal(PersonalPayload::Notifications { page: 0 });
    press(&fake, &config, &dialogue, action_query(&config, &first)).await?;
    let second = Payload::Personal(PersonalPayload::Notifications { page: 1 });
    press(&fake, &config, &dialogue, action_query(&config, &second)).await?;
    let subscription = current(&config)
        .actions
        .values()
        .find(|p| matches!(p, Payload::Personal(PersonalPayload::Subscription { .. })))
        .unwrap()
        .clone();
    press(
        &fake,
        &config,
        &dialogue,
        action_query(&config, &subscription),
    )
    .await?;
    assert_eq!(back_target(&config), second);
    press(&fake, &config, &dialogue, action_query(&config, &second)).await?;
    assert_eq!(
        back_target(&config),
        Payload::Personal(PersonalPayload::Settings)
    );
    press(
        &fake,
        &config,
        &dialogue,
        action_query(&config, &Payload::Home),
    )
    .await?;
    let favorites = Payload::Personal(PersonalPayload::Favorites { page: 0 });
    press(&fake, &config, &dialogue, action_query(&config, &favorites)).await?;
    assert_eq!(
        back_target(&config),
        Payload::Home,
        "Old settings origin was cleared by Home"
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn origin_does_not_survive_a_new_admission_generation() -> Result<()> {
    let (config, _) = config().await?;
    seed_origin_lists(&config).await?;
    let fake = FakeTelegram::new().await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let favorites = Payload::Personal(PersonalPayload::Favorites { page: 1 });
    publish(&fake, &config, favorites.clone()).await?;
    let target = current(&config)
        .actions
        .values()
        .find(|p| matches!(p, Payload::Control(ControlPayload::DeviceControl { .. })))
        .unwrap()
        .clone();
    let Payload::Control(ControlPayload::DeviceControl { room, .. }) = target else {
        unreachable!()
    };
    press(&fake, &config, &dialogue, action_query(&config, &target)).await?;
    assert_eq!(back_target(&config), favorites);
    sqlx::query("UPDATE users SET admission_generation='new-origin-admission' WHERE id=1")
        .execute(&config.db)
        .await?;
    publish(&fake, &config, target).await?;
    assert_eq!(
        back_target(&config),
        Payload::Control(ControlPayload::RoomDetail { room })
    );
    Ok(())
}

#[tokio::test]
async fn origin_survives_critical_confirmation_without_replacing_cancel() -> Result<()> {
    for confirm in [false, true] {
        let (config, ha) = config().await?;
        seed_origin_lists(&config).await?;
        sqlx::query("UPDATE devices SET critical=1")
            .execute(&config.db)
            .await?;
        let fake = FakeTelegram::new().await?;
        let dialogue = Dialogue::new(
            teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
            ChatId(1),
        );
        let favorites = Payload::Personal(PersonalPayload::Favorites { page: 1 });
        publish(&fake, &config, favorites.clone()).await?;
        let target = current(&config)
            .actions
            .values()
            .find(|p| matches!(p, Payload::Control(ControlPayload::DeviceControl { .. })))
            .unwrap()
            .clone();
        press(&fake, &config, &dialogue, action_query(&config, &target)).await?;
        assert_eq!(back_target(&config), favorites);
        let review = current(&config)
            .actions
            .values()
            .find(|p| {
                matches!(
                    p,
                    Payload::Control(
                        ControlPayload::ReviewAction { .. }
                            | ControlPayload::QuickAction {
                                cmd: DeviceCmd::TurnOn,
                                ..
                            }
                    )
                )
            })
            .unwrap()
            .clone();
        press(&fake, &config, &dialogue, action_query(&config, &review)).await?;
        let Payload::Control(ControlPayload::ReviewAction { id }) = current(&config).view.payload
        else {
            panic!("Critical write must open the persisted review before executing")
        };
        assert_eq!(
            back_target(&config),
            Payload::Control(ControlPayload::CancelAction { id })
        );
        assert!(ha.calls.lock().unwrap().is_empty());
        let outcome = Payload::Control(if confirm {
            ControlPayload::ConfirmAction { id }
        } else {
            ControlPayload::CancelAction { id }
        });
        press(&fake, &config, &dialogue, action_query(&config, &outcome)).await?;
        assert_eq!(
            back_target(&config),
            target,
            "Only the completed receipt uses the stored origin"
        );
        press(&fake, &config, &dialogue, query(&current(&config), "read")).await?;
        assert_eq!(ha.calls.lock().unwrap().len(), usize::from(confirm));
        press(&fake, &config, &dialogue, action_query(&config, &target)).await?;
        assert_eq!(back_target(&config), favorites);
        config.html_ui.clear(1);
    }
    Ok(())
}

#[tokio::test]
async fn origin_direct_personal_child_exits_through_unvisited_parents_without_loops() -> Result<()>
{
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    for (entry, parents) in [
        (
            Payload::Personal(PersonalPayload::FavoriteOptions {
                target: FavoriteAction::Device { room: 1, device: 1 },
            }),
            vec![
                Payload::Control(ControlPayload::DeviceControl { room: 1, device: 1 }),
                Payload::Control(ControlPayload::RoomDetail { room: 1 }),
                Payload::Control(ControlPayload::ListRooms),
                Payload::Home,
            ],
        ),
        (
            Payload::Personal(PersonalPayload::Subscription { device: 1 }),
            vec![
                Payload::Personal(PersonalPayload::Notifications { page: 0 }),
                Payload::Personal(PersonalPayload::Settings),
                Payload::Home,
            ],
        ),
    ] {
        config.html_ui.clear(1);
        publish(&fake, &config, entry).await?;
        for parent in parents {
            assert_eq!(back_target(&config), parent);
            press(&fake, &config, &dialogue, action_query(&config, &parent)).await?;
            assert_eq!(current(&config).view.payload, parent);
        }
        assert!(current(&config).pages[0].back.is_none());
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}
