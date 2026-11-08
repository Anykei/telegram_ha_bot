use super::*;

fn recovery_query(session: &WidgetSession) -> CallbackQuery {
    let mut q = query(session, "read");
    q.data = Some("r1:home".into());
    q
}

fn assert_denied_only(fake: &FakeTelegram) -> Result<()> {
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "answerWidgetQuery");
    assert_eq!(
        serde_json::from_str::<Value>(calls[0].1["data"].as_str().unwrap())?,
        json!({"denied":true})
    );
    Ok(())
}

#[tokio::test]
async fn recovery_replaces_closed_widget_with_home_without_replaying_command() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let old = publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
    )
    .await?;
    let key = old
        .actions
        .iter()
        .find(|(_, p)| {
            matches!(
                p,
                Payload::Control(ControlPayload::QuickAction {
                    cmd: DeviceCmd::TurnOn,
                    ..
                })
            )
        })
        .map(|(key, _)| key.clone())
        .context("Missing room control")?;
    // Invalidate the old room's captured resource identity while retaining Home access.
    sqlx::query("UPDATE devices SET entity_id='light.replaced' WHERE id=1")
        .execute(&config.db)
        .await?;
    fake.clear();
    resolve(&fake.bot, &query(&old, &key), &config).await?;
    assert_denied_only(&fake)?;
    fake.clear();
    let request = recovery_query(&old);
    assert!(resolve(&fake.bot, &request, &config).await?.is_none());
    let new = config.html_ui.sessions.get(&1).unwrap().clone();
    assert_eq!(new.view.payload, Payload::Home);
    assert_ne!(new.message, old.message);
    assert_eq!(config.sessions.get(&1).unwrap().last_menu_id, new.message);
    assert!(new.pages.iter().all(|p| p.back.is_none()));
    let calls = fake.calls();
    assert_eq!(
        calls
            .iter()
            .map(|(m, _)| m.to_ascii_lowercase())
            .collect::<Vec<_>>(),
        ["answerwidgetquery", "sendwidget", "deletemessage"]
    );
    assert_eq!(calls[1].1["html"], HTML);
    assert_eq!(calls[2].1["message_id"], old.message);
    assert!(ha.calls.lock().unwrap().is_empty());
    // A delayed second recovery from the deleted card cannot replace the new menu.
    fake.clear();
    resolve(&fake.bot, &request, &config).await?;
    assert_denied_only(&fake)?;
    assert_eq!(
        config.html_ui.sessions.get(&1).unwrap().message,
        new.message
    );
    Ok(())
}

#[tokio::test]
async fn recovery_uses_persisted_anchor_after_restart() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let old = publish(&fake, &config, Payload::Personal(PersonalPayload::Settings)).await?;
    config.html_ui.sessions.remove(&1);
    config.sessions.get_mut(&1).unwrap().ui_message_mode = UiMessageMode::Photo;
    // Authentication registers a handle even if the dialogue has no stored state.
    config.active_dialogues.insert(
        1,
        Dialogue::new(
            teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
            ChatId(1),
        ),
    );
    fake.clear();
    resolve(&fake.bot, &recovery_query(&old), &config).await?;
    assert_eq!(
        config.html_ui.sessions.get(&1).unwrap().view.payload,
        Payload::Home
    );
    assert_eq!(
        config.sessions.get(&1).unwrap().ui_message_mode,
        UiMessageMode::Html
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn recovery_cancels_stale_view_and_input_but_keeps_pending_device_lock() -> Result<()> {
    use crate::core::devices::DeviceAction;
    use crate::ha::HomeAssistantClient;
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let old = publish(&fake, &config, Payload::Home).await?;
    let state = ha
        .fetch_states_by_ids(&["light.living".into()])
        .await?
        .remove(0);
    let device: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE entity_id='light.living'")
        .fetch_one(&config.db)
        .await?;
    let operation = pending::begin(
        &config,
        1,
        &old.generation,
        device,
        &state,
        &DeviceAction::TurnOn,
    )?
    .unwrap();
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    dialogue
        .update(crate::bot::State::WaitingForName {
            device_id: device,
            room_id: 1,
        })
        .await?;
    config.active_dialogues.insert(1, dialogue.clone());
    let old_update = config.begin_view_update(1);
    resolve(&fake.bot, &recovery_query(&old), &config).await?;
    assert!(old_update.is_cancelled());
    assert!(matches!(
        dialogue.get().await?,
        Some(crate::bot::State::Idle)
    ));
    assert!(config.html_ui.pending.blocks(1, device, &old.generation));
    assert!(ha.calls.lock().unwrap().is_empty());
    operation.finish_after(&config, false, std::time::Duration::ZERO);
    Ok(())
}

#[tokio::test]
async fn recovery_rechecks_user_chat_preference_and_generation() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let old = publish(&fake, &config, Payload::Home).await?;
    let request = recovery_query(&old);
    let mut foreign = request.clone();
    foreign.from.id = UserId(2);
    fake.clear();
    resolve(&fake.bot, &foreign, &config).await?;
    assert_denied_only(&fake)?;
    db::interface::set(1, db::interface::InterfaceMode::Classic, &config.db).await?;
    fake.clear();
    resolve(&fake.bot, &request, &config).await?;
    assert_denied_only(&fake)?;
    db::interface::set(1, db::interface::InterfaceMode::Html, &config.db).await?;
    db::delete_user(1, &config.db).await?;
    fake.clear();
    resolve(&fake.bot, &request, &config).await?;
    assert_denied_only(&fake)?;
    db::add_user(1, &config.db).await?;
    db::interface::set(1, db::interface::InterfaceMode::Html, &config.db).await?;
    fake.clear();
    resolve(&fake.bot, &request, &config).await?;
    assert_denied_only(&fake)?;
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn recovery_keeps_original_message_if_replacement_fails() -> Result<()> {
    let (config, _) = config().await?;
    let fake = FakeTelegram::new().await?;
    let old = publish(&fake, &config, Payload::Home).await?;
    *fake.fail.lock().unwrap() = Some("sendWidget".into());
    fake.clear();
    assert!(resolve(&fake.bot, &recovery_query(&old), &config)
        .await
        .is_err());
    assert_eq!(config.sessions.get(&1).unwrap().last_menu_id, old.message);
    assert_eq!(
        config.html_ui.sessions.get(&1).unwrap().message,
        old.message
    );
    assert!(!fake
        .calls()
        .iter()
        .any(|(m, _)| m.eq_ignore_ascii_case("DeleteMessage")));
    Ok(())
}
