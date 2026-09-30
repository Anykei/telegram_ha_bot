use super::*;
use crate::core::action_results::{ActionResult, Outcome};
use crate::core::commands::{self, CommandExecution, CommandSource};

async fn second_light(config: &Arc<AppConfig>, ha: &FakeHa) -> anyhow::Result<()> {
    db::devices::sync_device("light.second", "bedroom", "Свет", "light", &config.db).await?;
    db::subscriptions::toggle_hidden("light.second", &config.db).await?;
    sqlx::query("UPDATE devices SET alias = 'Свет' WHERE entity_id = 'light.bedroom'")
        .execute(&config.db)
        .await?;
    ha.states
        .lock()
        .unwrap()
        .push(entity("light.second", "off"));
    Ok(())
}

#[tokio::test]
async fn ambiguous_command_waits_for_stable_target_and_rejects_replay() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    second_light(&config, &ha).await?;
    let CommandExecution::NeedsChoice { choice_id } =
        commands::execute_text(42, false, CommandSource::Text, "включи свет", &config).await?
    else {
        panic!("Expected disambiguation");
    };
    assert!(ha.calls.lock().unwrap().is_empty());
    let choice = router(
        Payload::Command(CommandPayload::Choice {
            id: choice_id,
            page: 0,
        }),
        42,
        config.clone(),
    )
    .await?;
    let candidates = callbacks(&choice)
        .into_iter()
        .filter(|p| matches!(p, Payload::Command(CommandPayload::Choose { .. })))
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 2);
    assert!(choice
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .filter(|b| b.text.contains("Свет"))
        .all(|b| b.text.contains("Спальня")));
    assert!(commands::choose_target(99, choice_id, 3, &config)
        .await
        .is_err());
    let selected = commands::choose_target(42, choice_id, 3, &config).await?;
    assert!(matches!(selected, CommandExecution::Done { .. }));
    assert!(commands::choose_target(42, choice_id, 3, &config)
        .await
        .is_err());
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(ha.calls.lock().unwrap()[0].0, "light.turn_on");
    Ok(())
}

#[tokio::test]
async fn choice_rechecks_acl_and_new_input_closes_previous_choice() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    second_light(&config, &ha).await?;
    let CommandExecution::NeedsChoice { choice_id } =
        commands::execute_text(42, false, CommandSource::Text, "выключи свет", &config).await?
    else {
        panic!();
    };
    db::access::set_device_access(42, "light.bedroom", false, false, false, &config.db).await?;
    let choice = router(
        Payload::Command(CommandPayload::Choice {
            id: choice_id,
            page: 0,
        }),
        42,
        config.clone(),
    )
    .await?;
    assert!(
        !callbacks(&choice).contains(&Payload::Command(CommandPayload::Choose {
            id: choice_id,
            target: 3
        }))
    );
    assert!(commands::choose_target(42, choice_id, 3, &config)
        .await
        .is_err());
    assert!(ha.calls.lock().unwrap().is_empty());
    db::access::set_device_access(42, "light.bedroom", true, true, true, &config.db).await?;
    let CommandExecution::NeedsChoice { choice_id } =
        commands::execute_text(42, false, CommandSource::Text, "включи свет", &config).await?
    else {
        panic!();
    };
    assert!(matches!(
        commands::execute_text(42, false, CommandSource::Text, "привет", &config).await?,
        CommandExecution::NotACommand
    ));
    assert!(commands::choose_target(42, choice_id, 3, &config)
        .await
        .is_err());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn selected_critical_target_requires_original_pending_confirmation_once() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    second_light(&config, &ha).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let CommandExecution::NeedsChoice { choice_id } =
        commands::execute_text(42, false, CommandSource::Voice, "выключи свет", &config).await?
    else {
        panic!();
    };
    let CommandExecution::NeedsConfirmation { pending_id, .. } =
        commands::choose_target(42, choice_id, 3, &config).await?
    else {
        panic!();
    };
    assert!(ha.calls.lock().unwrap().is_empty());
    let (a, b) = tokio::join!(
        commands::confirm_pending(42, false, pending_id, &config),
        commands::confirm_pending(42, false, pending_id, &config)
    );
    a?;
    b?;
    commands::confirm_pending(42, false, pending_id, &config).await?;
    config.sessions.clear();
    let receipt = router(
        Payload::Command(CommandPayload::Result { id: pending_id }),
        42,
        config.clone(),
    )
    .await?;
    assert!(receipt.text.contains("1 из 1"));
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    let pending = db::pending_commands::get_pending(pending_id, 42, &config.db)
        .await?
        .unwrap();
    assert_eq!(pending.status, "executed");
    assert_eq!(
        serde_json::from_str::<ActionResult>(&pending.result_json.unwrap())?.outcome,
        Outcome::Sent
    );
    Ok(())
}

#[tokio::test]
async fn lost_response_is_unknown_and_both_result_stores_never_replay() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let CommandExecution::NeedsConfirmation { pending_id, .. } =
        commands::execute_text(42, false, CommandSource::Text, "выключи люстру", &config).await?
    else {
        panic!();
    };
    ha.fail_write.store(true, Ordering::SeqCst);
    commands::confirm_pending(42, false, pending_id, &config).await?;
    let pending = db::pending_commands::get_pending(pending_id, 42, &config.db)
        .await?
        .unwrap();
    assert_eq!(
        serde_json::from_str::<ActionResult>(&pending.result_json.unwrap())?.outcome,
        Outcome::Unknown
    );
    commands::confirm_pending(42, false, pending_id, &config).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    let proposal = open(
        &config,
        ControlPayload::QuickAction {
            room: 1,
            device: 3,
            cmd: DeviceCmd::TurnOn,
        },
    )
    .await?;
    let id = confirmation_id(&proposal);
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    let saved = db::ui_confirmations::get(id, 42, &config.db)
        .await?
        .unwrap();
    assert_eq!(
        serde_json::from_str::<ActionResult>(&saved.result_json.unwrap())?.outcome,
        Outcome::Unknown
    );
    let view = open(&config, ControlPayload::ReviewAction { id }).await?;
    assert!(view.text.contains("не удалось определить"));
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    Ok(())
}

#[tokio::test]
async fn ten_target_group_saves_partial_outcome_and_filters_revoked_names() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = db::action_groups::create_action_group("Группа", &config.db).await?;
    sqlx::query("UPDATE action_groups SET access_scope = 'all_users' WHERE id = ?")
        .bind(group)
        .execute(&config.db)
        .await?;
    for n in 0..10 {
        let entity_id = format!("light.batch_{n}");
        db::devices::sync_device(
            &entity_id,
            "bedroom",
            &format!("Лампа {n}"),
            "light",
            &config.db,
        )
        .await?;
        db::subscriptions::toggle_hidden(&entity_id, &config.db).await?;
        let id: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = ?")
            .bind(&entity_id)
            .fetch_one(&config.db)
            .await?;
        sqlx::query("INSERT INTO action_group_items(group_id, device_id) VALUES (?, ?)")
            .bind(group)
            .bind(id)
            .execute(&config.db)
            .await?;
        ha.states.lock().unwrap().push(entity(&entity_id, "off"));
    }
    *ha.fail_entity.lock().unwrap() = Some("light.batch_9".into());
    let result = open(
        &config,
        ControlPayload::ExecuteActionGroup {
            group,
            command: db::action_groups::ActionGroupCommand::TurnOn,
        },
    )
    .await?;
    assert!(result.text.contains("9 из 10"));
    assert!(result.text.contains("неизвестно: 1"));
    assert!(result.text.contains("Лампа 9"));
    let id = confirmation_id(&result);
    let saved = db::ui_confirmations::get(id, 42, &config.db)
        .await?
        .unwrap();
    assert_eq!(
        serde_json::from_str::<ActionResult>(&saved.result_json.unwrap())?.outcome,
        Outcome::Partial
    );
    db::access::set_device_access(42, "light.batch_9", false, false, false, &config.db).await?;
    config.sessions.clear();
    let reopened = router(result.payload.for_refresh(), 42, config.clone()).await?;
    assert!(reopened.text.contains("9 из 10"));
    assert!(!reopened.text.contains("Лампа 9"));
    assert!(reopened.text.contains("Недоступная цель"));
    open(&config, ControlPayload::ConfirmAction { id }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 10);
    Ok(())
}

#[tokio::test]
async fn abandoned_claim_reads_unknown_without_reexecution() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let CommandExecution::NeedsConfirmation { pending_id, .. } =
        commands::execute_text(42, false, CommandSource::Text, "включи люстру", &config).await?
    else {
        panic!();
    };
    assert!(db::pending_commands::claim(pending_id, 42, &config.db).await?);
    config.sessions.clear();
    commands::confirm_pending(42, false, pending_id, &config).await?;
    assert!(commands::pending_result_text(42, pending_id, &config)
        .await?
        .contains("не удалось определить"));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn readonly_camera_ambiguity_preserves_video_duration_and_engine() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let first = camera_for_ui_tests(&config).await?;
    sqlx::query("UPDATE cameras SET name = 'Вход' WHERE id = ?")
        .bind(first)
        .execute(&config.db)
        .await?;
    let second = sqlx::query("INSERT INTO cameras (key, name, room_id, stream_url, enabled) VALUES ('second', 'Вход дальний', 1, 'rtsp://unused.invalid', 1)").execute(&config.db).await?.last_insert_rowid();
    sqlx::query("INSERT INTO user_profiles(user_id, voice_command_engine) VALUES (42, 'ha_conversation_readonly') ON CONFLICT(user_id) DO UPDATE SET voice_command_engine = excluded.voice_command_engine").execute(&config.db).await?;
    let CommandExecution::NeedsChoice { choice_id } =
        commands::execute_voice_text(42, false, "видео вход 15с", &config).await?
    else {
        panic!("Expected camera choice");
    };
    let row = db::command_choices::get(choice_id, 42, &config.db)
        .await?
        .unwrap();
    let context: commands::ChoiceContext = serde_json::from_str(&row.context_json)?;
    assert_eq!(context.source, CommandSource::Voice);
    assert_eq!(context.engine, "ha_conversation_readonly");
    assert!(
        matches!(commands::choose_target(42, choice_id, second, &config).await?, CommandExecution::SendCameraClip { camera_id, seconds: 15 } if camera_id == second)
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn multi_target_clarification_executes_nothing_until_all_parts_resolve() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    second_light(&config, &ha).await?;
    db::devices::sync_device("switch.desk", "bedroom", "Стол", "switch", &config.db).await?;
    db::subscriptions::toggle_hidden("switch.desk", &config.db).await?;
    ha.states.lock().unwrap().push(entity("switch.desk", "off"));
    let CommandExecution::NeedsChoice { choice_id } = commands::execute_text(
        42,
        false,
        CommandSource::Text,
        "включи свет и стол",
        &config,
    )
    .await?
    else {
        panic!();
    };
    assert!(ha.calls.lock().unwrap().is_empty());
    let CommandExecution::NeedsConfirmation { pending_id, .. } =
        commands::choose_target(42, choice_id, 3, &config).await?
    else {
        panic!();
    };
    assert!(ha.calls.lock().unwrap().is_empty());
    commands::confirm_pending(42, false, pending_id, &config).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    assert!(ha
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.1 == "switch.desk"));
    Ok(())
}

#[tokio::test]
async fn mass_lights_remain_mass_commands_and_pending_acl_failure_keeps_actual_counts(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    second_light(&config, &ha).await?;
    let CommandExecution::NeedsConfirmation { pending_id, .. } =
        commands::execute_text(1, true, CommandSource::Text, "turn off all lights", &config)
            .await?
    else {
        panic!("Mass command must not ask for one target");
    };
    let row = db::pending_commands::get_pending(pending_id, 1, &config.db)
        .await?
        .unwrap();
    let commands::CommandIntent::MultiDeviceAction { device_ids, .. } =
        serde_json::from_str(&row.intent_json)?
    else {
        panic!("Approved target snapshot missing");
    };
    assert_eq!(device_ids.len(), 2);
    assert!(ha.calls.lock().unwrap().is_empty());
    let CommandExecution::NeedsConfirmation { pending_id, .. } = commands::execute_text(
        42,
        false,
        CommandSource::Text,
        "выключи light.bedroom и light.second",
        &config,
    )
    .await?
    else {
        panic!();
    };
    db::access::set_device_access(42, "light.second", false, false, false, &config.db).await?;
    commands::confirm_pending(42, false, pending_id, &config).await?;
    let row = db::pending_commands::get_pending(pending_id, 42, &config.db)
        .await?
        .unwrap();
    let result: ActionResult = serde_json::from_str(&row.result_json.unwrap())?;
    assert_eq!(result.outcome, Outcome::Partial);
    assert_eq!(
        result
            .targets
            .iter()
            .filter(|t| t.outcome == Outcome::Failed)
            .count(),
        1
    );
    assert_eq!(
        result
            .targets
            .iter()
            .filter(|t| t.outcome == Outcome::Sent)
            .count(),
        1
    );
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn ordinary_group_generated_and_legacy_buttons_share_consumed_receipts_on_replay(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = db::action_groups::create_action_group("Повтор", &config.db).await?;
    db::action_groups::toggle_action_group_item(group, 3, &config.db).await?;
    sqlx::query("UPDATE action_groups SET access_scope = 'all_users' WHERE id = ?")
        .bind(group)
        .execute(&config.db)
        .await?;
    let screen = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    let button = callbacks(&screen)
        .into_iter()
        .find(|p| matches!(p, Payload::Control(ControlPayload::ConfirmAction { .. })))
        .unwrap();
    let receipt = router(button.clone(), 42, config.clone()).await?;
    router(button, 42, config.clone()).await?;
    router(receipt.payload.for_refresh(), 42, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    let legacy = Payload::Control(ControlPayload::ExecuteActionGroup {
        group,
        command: db::action_groups::ActionGroupCommand::TurnOn,
    });
    let (a, b) = tokio::join!(
        crate::bot::confirmations::prepare_legacy_callback(
            legacy.clone(),
            42,
            42,
            999,
            config.clone()
        ),
        crate::bot::confirmations::prepare_legacy_callback(
            legacy.clone(),
            42,
            42,
            999,
            config.clone()
        ),
    );
    let a = a?;
    assert_eq!(a, b?);
    router(a.clone(), 42, config.clone()).await?;
    config.sessions.clear();
    let restored =
        crate::bot::confirmations::prepare_legacy_callback(legacy, 42, 42, 999, config.clone())
            .await?;
    assert_eq!(a, restored);
    router(restored, 42, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    Ok(())
}

#[tokio::test]
async fn full_voice_errors_after_dispatch_never_fall_back_to_local_write() -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for response in [
        None,
        Some(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ),
    ] {
        let (mut config, ha) = setup().await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let cfg = Arc::get_mut(&mut config).unwrap();
        cfg.ha_url = url;
        cfg.voice_command_engine = crate::options::VoiceCommandEngine::HaConversationFull;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let _ = socket.read(&mut bytes).await;
            if let Some(response) = response {
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let result = commands::execute_voice_text(42, false, "выключи люстру", &config).await?;
        server.await?;
        assert!(
            matches!(result, CommandExecution::Done { message } if message.contains("не удалось определить"))
        );
        assert!(ha.calls.lock().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn full_voice_connect_failure_keeps_local_fallback() -> anyhow::Result<()> {
    let (mut config, ha) = setup().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    drop(listener);
    let cfg = Arc::get_mut(&mut config).unwrap();
    cfg.ha_url = url;
    cfg.voice_command_engine = crate::options::VoiceCommandEngine::HaConversationFull;
    assert!(matches!(
        commands::execute_voice_text(42, false, "выключи люстру", &config).await?,
        CommandExecution::Done { .. }
    ));
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}
