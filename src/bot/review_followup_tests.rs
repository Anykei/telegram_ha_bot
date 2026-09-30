use super::*;

#[tokio::test]
async fn long_state_buttons_use_short_context_and_keep_exact_state() -> anyhow::Result<()> {
    use crate::bot::router::SettingsPayload;
    let (config, ha) = setup().await?;
    let state = "Длинное состояние датчика 🔥 ".repeat(12);
    ha.states
        .lock()
        .unwrap()
        .iter_mut()
        .find(|e| e.entity_id == "sensor.temperature")
        .unwrap()
        .state = state.clone();
    db::devices::set_state_alias("sensor.temperature", &state, "Проверка", &config.db).await?;
    let view = router(
        Payload::AdminSettings(SettingsPayload::StateAliases { room: 1, device: 2 }),
        1,
        config.clone(),
    )
    .await?;
    for button in view.kb.inline_keyboard.iter().flatten() {
        if let InlineKeyboardButtonKind::CallbackData(data) = &button.kind {
            assert!(data.len() <= 64, "{} bytes: {}", data.len(), button.text);
        }
    }
    let edit = callbacks(&view)
        .into_iter()
        .find(|p| {
            matches!(
                p,
                Payload::AdminSettings(SettingsPayload::StateAliasEditContext { .. })
            )
        })
        .unwrap();
    let reset = callbacks(&view)
        .into_iter()
        .find(|p| {
            matches!(
                p,
                Payload::AdminSettings(SettingsPayload::StateAliasResetContext { .. })
            )
        })
        .unwrap();
    let edit_view = router(edit, 1, config.clone()).await?;
    assert!(
        matches!(edit_view.next_state, Some(State::WaitingForStateAlias {original_state, device_id: 2, ..}) if original_state == state)
    );
    assert!(edit_view.payload.to_string().len() <= 64);
    router(reset.clone(), 42, config.clone()).await?;
    assert_eq!(
        db::devices::get_state_alias("sensor.temperature", &state, &config.db)
            .await?
            .as_deref(),
        Some("Проверка")
    );
    router(reset, 1, config.clone()).await?;
    assert!(
        db::devices::get_state_alias("sensor.temperature", &state, &config.db)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn paginated_name_form_saves_and_preserves_admin_settings_origin() -> anyhow::Result<()> {
    use crate::bot::router::SettingsPayload;
    use teloxide::dispatching::dialogue::InMemStorage;
    let (config, _) = setup().await?;
    let source = Payload::AdminSettings(SettingsPayload::EditName { room: 1, device: 3 });
    let mut view = router(source, 1, config.clone()).await?;
    let Some(State::WaitingForNameDraft {
        device_id: 3,
        room_id: 1,
        draft_id,
    }) = view.next_state.clone()
    else {
        panic!("Expected name form")
    };
    view.notifications.push(crate::core::HeaderItem {
        source: None,
        additional_sources: Vec::new(),
        icon: "🔔".into(),
        label: "Статус".repeat(1500),
        value: "on".into(),
        last_update: Utc::now(),
    });
    crate::bot::text_pages::prepare(&mut view, 1, &config).await?;
    assert!(matches!(view.payload, Payload::TextPage { .. }));
    let dialogue = teloxide::dispatching::dialogue::Dialogue::new(
        InMemStorage::<State>::new(),
        teloxide::types::ChatId(1),
    );
    dialogue.update(view.next_state.unwrap()).await?;
    let generation = db::admission_generation(1, &config.db).await?.unwrap();
    crate::core::record_user_activity_for_admission(&config, 1, &generation).await?;
    crate::core::update_user_state_with_mode(
        &config,
        1,
        7,
        &view.payload.to_string(),
        crate::models::UiMessageMode::Text,
    )
    .await;
    let msg: teloxide::types::Message = serde_json::from_value(
        serde_json::json!({"message_id": 8, "date": 1_700_000_000, "from": {"id":1,"is_bot":false,"first_name":"Root"}, "chat":{"id":1,"type":"private"},"text":"После пагинации"}),
    )?;
    let (bot, server) = mock_telegram_edits(vec![
        (200, serde_json::json!({"ok":true,"result":true})),
        (200, serde_json::json!({"ok":true,"result":{"message_id":7,"date":1_700_000_000,"chat":{"id":1,"type":"private"},"text":"ok"}})),
    ]).await?;
    crate::bot::handlers::handle_device_name_input(
        bot,
        msg,
        config.clone(),
        dialogue,
        (3, 1, Some(draft_id)),
    )
    .await?;
    assert_eq!(
        db::devices::get_device_by_id(3, &config.db)
            .await?
            .unwrap()
            .alias
            .as_deref(),
        Some("После пагинации")
    );
    assert_eq!(
        config.name_aliases.get("light.bedroom").unwrap().as_str(),
        "После пагинации"
    );
    assert!(matches!(
        Payload::from_string(&config.sessions.get(&1).unwrap().current_context)?,
        Payload::AdminSettings(SettingsPayload::DeviceDetail { device: 3, .. })
    ));
    server.abort();
    Ok(())
}

#[tokio::test]
async fn legacy_text_page_does_not_restore_receipts_without_source_metadata() -> anyhow::Result<()>
{
    let (config, _) = setup().await?;
    let context = serde_json::json!({
        "source": Payload::Home.to_string(),
        "notice": "PRIVATEOLDDEVICESTATE",
        "alert": "PRIVATEOLDDEVICEERROR",
        "suppress_input": false,
    });
    let screen = db::ui_text_pages::register(42, &context.to_string(), &config.db).await?;
    let view = router(Payload::TextPage { screen, page: 0 }, 42, config).await?;
    assert!(!view.get_plain_text().contains("PRIVATEOLDDEVICE"));
    Ok(())
}
