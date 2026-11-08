use super::*;
use crate::core::action_results::{ActionResult, Outcome};
use crate::core::commands::{self, CommandExecution, CommandSource};

#[path = "critical_claim_tests.rs"]
mod critical_claim;

#[path = "group_dispatch_tests.rs"]
mod group_dispatch;

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
    db::access::set_device_access(42, "light.bedroom", false, false, &config.db).await?;
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
    db::access::set_device_access(42, "light.bedroom", true, true, &config.db).await?;
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
    db::access::set_device_access(42, "light.batch_9", false, false, &config.db).await?;
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
    let commands::CommandIntent::ScopedLightsAction { targets, .. } =
        serde_json::from_str(&row.intent_json)?
    else {
        panic!("Approved target snapshot missing");
    };
    assert_eq!(targets.len(), 2);
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
    db::access::set_device_access(42, "light.second", false, false, &config.db).await?;
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

async fn confirmation_test_group(config: &Arc<AppConfig>) -> anyhow::Result<i64> {
    let group = db::action_groups::create_action_group("Критичность", &config.db).await?;
    db::action_groups::toggle_action_group_item(group, 3, &config.db).await?;
    sqlx::query("UPDATE action_groups SET access_scope = 'all_users' WHERE id = ?")
        .bind(group)
        .execute(&config.db)
        .await?;
    Ok(group)
}

fn generated_group_buttons(view: &View) -> Vec<Payload> {
    callbacks(view)
        .into_iter()
        .filter(|p| {
            matches!(
                p,
                Payload::Control(
                    ControlPayload::ConfirmAction { .. } | ControlPayload::ReviewAction { .. }
                )
            )
        })
        .fold(Vec::new(), |mut buttons, payload| {
            if !buttons.contains(&payload) {
                buttons.push(payload);
            }
            buttons
        })
}

#[tokio::test]
async fn stale_group_button_requires_new_critical_review_for_user_admin_and_legacy(
) -> anyhow::Result<()> {
    for (user, legacy) in [(42, false), (1, false), (42, true), (1, true)] {
        let (config, ha) = setup().await?;
        let group = confirmation_test_group(&config).await?;
        let is_admin = user == config.root_user;
        let screen = router(
            if is_admin {
                Payload::Admin(AdminPayload::ActionGroupDetail { group })
            } else {
                Payload::Control(ControlPayload::ActionGroupDetail { group })
            },
            user,
            config.clone(),
        )
        .await?;
        let old = if legacy {
            let command = db::action_groups::ActionGroupCommand::TurnOff;
            crate::bot::confirmations::prepare_legacy_callback(
                if is_admin {
                    Payload::Admin(AdminPayload::ExecuteActionGroup { group, command })
                } else {
                    Payload::Control(ControlPayload::ExecuteActionGroup { group, command })
                },
                user,
                user as i64,
                700,
                config.clone(),
            )
            .await?
        } else {
            generated_group_buttons(&screen).remove(0)
        };
        let Payload::Control(ControlPayload::ConfirmAction { id: old_id }) = old.clone() else {
            panic!("ordinary group must be immediately executable");
        };
        db::devices::toggle_device_critical(3, &config.db).await?;
        let (a, b) = tokio::join!(
            router(old.clone(), user, config.clone()),
            router(old.clone(), user, config.clone())
        );
        let reviewed = a?;
        let id = confirmation_id(&reviewed);
        assert_ne!(id, old_id);
        assert_eq!(confirmation_id(&b?), id);
        assert!(reviewed.text.contains("Люстра"));
        assert!(
            callbacks(&reviewed).contains(&Payload::Control(ControlPayload::ConfirmAction { id }))
        );
        assert!(ha.calls.lock().unwrap().is_empty());
        let stale = db::ui_confirmations::get(old_id, user, &config.db)
            .await?
            .unwrap();
        assert_eq!(stale.status, "cancelled");
        assert_eq!(stale.replacement_id, Some(id));
        router(old.clone(), user, config.clone()).await?;
        assert!(ha.calls.lock().unwrap().is_empty());
        router(
            Payload::Control(ControlPayload::ConfirmAction { id }),
            user,
            config.clone(),
        )
        .await?;
        router(old, user, config.clone()).await?;
        router(
            Payload::Control(ControlPayload::ConfirmAction { id }),
            user,
            config.clone(),
        )
        .await?;
        assert_eq!(
            ha.calls.lock().unwrap().len(),
            1,
            "user={user} legacy={legacy}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn pending_group_review_is_replaced_when_another_member_becomes_critical(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = confirmation_test_group(&config).await?;
    second_light(&config, &ha).await?;
    db::action_groups::toggle_action_group_item(group, 4, &config.db).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let screen = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
    let Payload::Control(ControlPayload::ReviewAction { id: old }) =
        generated_group_buttons(&screen).remove(0)
    else {
        panic!();
    };
    db::devices::toggle_device_critical(4, &config.db).await?;
    let review = open(&config, ControlPayload::ConfirmAction { id: old }).await?;
    let new = confirmation_id(&review);
    assert_ne!(old, new);
    assert!(ha.calls.lock().unwrap().is_empty());
    open(&config, ControlPayload::ConfirmAction { id: new }).await?;
    open(&config, ControlPayload::ConfirmAction { id: old }).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    Ok(())
}

#[tokio::test]
async fn group_rechecks_new_critical_member_between_dispatches_without_replaying(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = confirmation_test_group(&config).await?;
    second_light(&config, &ha).await?;
    db::action_groups::toggle_action_group_item(group, 4, &config.db).await?;
    let screen = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
    let button = generated_group_buttons(&screen).remove(0);
    let Payload::Control(ControlPayload::ConfirmAction { id }) = button.clone() else {
        panic!();
    };
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let execution = tokio::spawn(router(button.clone(), 42, config.clone()));
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.write_started.notified(),
    )
    .await?;
    db::devices::toggle_device_critical(4, &config.db).await?;
    ha.release_write.notify_one();
    execution.await??;
    router(button, 42, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(ha.calls.lock().unwrap()[0].1, "light.bedroom");
    let saved = db::ui_confirmations::get(id, 42, &config.db)
        .await?
        .unwrap();
    let result: ActionResult = serde_json::from_str(&saved.result_json.unwrap())?;
    assert_eq!(result.outcome, Outcome::Partial);
    assert_eq!(
        result.targets.iter().find(|t| t.id == 4).unwrap().outcome,
        Outcome::Failed
    );
    Ok(())
}

#[tokio::test]
async fn group_dispatch_requires_user_consent_but_preserves_schedule_policy() -> anyhow::Result<()>
{
    use crate::core::action_groups::{execute_action_group, ActionActor};
    let (config, ha) = setup().await?;
    let group = confirmation_test_group(&config).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    for user in [42, config.root_user] {
        db::add_user(user, &config.db).await?;
        let rejected = execute_action_group(
            group,
            db::action_groups::ActionGroupCommand::TurnOff,
            ActionActor::User(user),
            config.clone(),
        )
        .await?;
        assert_eq!(rejected.result.outcome, Outcome::Failed);
        assert!(ha.calls.lock().unwrap().is_empty());
    }
    let scheduled = execute_action_group(
        group,
        db::action_groups::ActionGroupCommand::TurnOff,
        ActionActor::Schedule { schedule_id: 1 },
        config.clone(),
    )
    .await?;
    assert_eq!(scheduled.result.outcome, Outcome::Sent);
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn legacy_group_review_rebuilds_metadata_before_recording_display_sources(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = confirmation_test_group(&config).await?;
    db::devices::toggle_device_critical(3, &config.db).await?;
    let screen = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
    let Payload::Control(ControlPayload::ReviewAction { id }) =
        generated_group_buttons(&screen).remove(0)
    else {
        panic!();
    };
    let saved = db::ui_confirmations::get(id, 42, &config.db)
        .await?
        .unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&saved.action_json)?;
    old.as_object_mut().unwrap().remove("displayed_devices");
    old["description"] = "Previously captured private name".into();
    sqlx::query("UPDATE ui_confirmations SET action_json = ? WHERE id = ?")
        .bind(serde_json::to_string(&old)?)
        .bind(id)
        .execute(&config.db)
        .await?;
    let reviewed = open(&config, ControlPayload::ReviewAction { id }).await?;
    assert!(!reviewed.text.contains("Previously captured private name"));
    assert!(reviewed.text.contains("Люстра"));
    assert!(reviewed
        .displayed_resources
        .contains(&crate::core::types::DisplaySource::Device {
            id: 3,
            entity_id: "light.bedroom".into(),
        }));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn group_display_provenance_blocks_revoked_aggregate_contributors_after_slow_ha_read(
) -> anyhow::Result<()> {
    for list in [false, true] {
        let (config, ha) = setup().await?;
        let group = confirmation_test_group(&config).await?;
        // The last target contributes to the aggregate but is beyond the twelve
        // names printed on a group detail page.
        let mut revoked = (3, "light.bedroom".to_owned());
        for index in 0..13 {
            let entity_id = format!("light.provenance_{index:02}");
            db::devices::sync_device(
                &entity_id,
                "bedroom",
                &format!("zzz_{index:02}"),
                "light",
                &config.db,
            )
            .await?;
            db::subscriptions::toggle_hidden(&entity_id, &config.db).await?;
            let id: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = ?")
                .bind(&entity_id)
                .fetch_one(&config.db)
                .await?;
            db::action_groups::toggle_action_group_item(group, id, &config.db).await?;
            ha.states.lock().unwrap().push(entity(&entity_id, "on"));
            revoked = (id, entity_id);
        }
        let payload = if list {
            ControlPayload::ActionGroups
        } else {
            ControlPayload::ActionGroupDetail { group }
        };
        ha.pause_next_read.store(true, Ordering::SeqCst);
        let rendering = tokio::spawn(router(Payload::Control(payload), 42, config.clone()));
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ha.read_started.notified(),
        )
        .await?;
        db::access::set_device_access(42, &revoked.1, false, false, &config.db).await?;
        ha.release_read.notify_one();
        let view = rendering.await??;
        assert!(view
            .displayed_resources
            .contains(&crate::core::types::DisplaySource::Device {
                id: revoked.0,
                entity_id: revoked.1,
            }));
        let bot = teloxide::Bot::with_client(
            "123:test",
            teloxide::net::default_reqwest_settings()
                .no_proxy()
                .build()?,
        )
        .set_api_url("http://127.0.0.1:9".parse()?);
        assert!(
            !crate::bot::handlers::send_new_view_inner(
                &bot,
                teloxide::types::ChatId(42),
                42,
                view,
                config,
                None
            )
            .await?,
            "list={list}"
        );
        assert!(ha.calls.lock().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn thirty_group_renders_reuse_pending_and_expiry_cleanup_cannot_replay_old_buttons(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = confirmation_test_group(&config).await?;
    let first =
        generated_group_buttons(&open(&config, ControlPayload::ActionGroupDetail { group }).await?);
    assert_eq!(first.len(), 2);
    let expiry = chrono::Utc::now().timestamp() + 30;
    sqlx::query("UPDATE ui_confirmations SET expires_at = ?")
        .bind(expiry)
        .execute(&config.db)
        .await?;
    for _ in 1..30 {
        assert_eq!(
            generated_group_buttons(
                &open(&config, ControlPayload::ActionGroupDetail { group }).await?
            ),
            first
        );
    }
    let records: Vec<(i64, i64)> = sqlx::query_as("SELECT id, expires_at FROM ui_confirmations")
        .fetch_all(&config.db)
        .await?;
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|r| r.1 == expiry));
    assert!(ha.calls.lock().unwrap().is_empty());
    sqlx::query("UPDATE ui_confirmations SET expires_at = 0")
        .execute(&config.db)
        .await?;
    assert_eq!(db::ui_confirmations::purge_expired(&config.db).await?, 2);
    let fresh =
        generated_group_buttons(&open(&config, ControlPayload::ActionGroupDetail { group }).await?);
    assert!(fresh.iter().all(|p| !first.contains(p)));
    for old in first {
        let receipt = router(old, 42, config.clone()).await?;
        assert!(!callbacks(&receipt)
            .iter()
            .any(|p| matches!(p, Payload::Control(ControlPayload::ConfirmAction { .. }))));
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    router(fresh[0].clone(), 42, config.clone()).await?;
    sqlx::query("UPDATE ui_confirmations SET expires_at = 0")
        .execute(&config.db)
        .await?;
    assert_eq!(db::ui_confirmations::purge_expired(&config.db).await?, 1);
    router(fresh[0].clone(), 42, config.clone()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
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
            matches!(result, CommandExecution::Done { message, .. } if message.contains("не удалось определить"))
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

#[tokio::test]
async fn bulk_light_scope_is_frozen_and_confirmation_cannot_be_disabled() -> anyhow::Result<()> {
    for (source, text) in [
        (CommandSource::Text, "выключи весь свет на кухне"),
        (CommandSource::Voice, "turn off all lights in kitchen"),
    ] {
        let (mut config, ha) = setup().await?;
        Arc::get_mut(&mut config).unwrap().voice_confirm_dangerous = false;
        db::rooms::sync_rooms_from_ha("kitchen", "Кухня", &config.db).await?;
        db::devices::sync_device("light.kitchen", "kitchen", "Кухня", "light", &config.db).await?;
        db::subscriptions::toggle_hidden("light.kitchen", &config.db).await?;
        ha.states
            .lock()
            .unwrap()
            .push(entity("light.kitchen", "on"));
        let execution = if source == CommandSource::Voice {
            commands::execute_voice_text(1, true, text, &config).await?
        } else {
            commands::execute_text(1, true, source, text, &config).await?
        };
        let CommandExecution::NeedsConfirmation { pending_id } = execution else {
            panic!("Bulk commands must be reviewed")
        };
        let message = commands::pending_result_text(1, pending_id, &config).await?;
        assert!(message.contains("Кухня"));
        assert!(message.contains("1 устройств"));
        assert!(ha.calls.lock().unwrap().is_empty());
        // New room members are never included in an already approved snapshot.
        db::devices::sync_device("light.new_kitchen", "kitchen", "Новый", "light", &config.db)
            .await?;
        db::subscriptions::toggle_hidden("light.new_kitchen", &config.db).await?;
        ha.states
            .lock()
            .unwrap()
            .push(entity("light.new_kitchen", "on"));
        commands::confirm_pending(1, true, pending_id, &config).await?;
        commands::confirm_pending(1, true, pending_id, &config).await?;
        assert_eq!(ha.calls.lock().unwrap().len(), 1);
        assert_eq!(ha.calls.lock().unwrap()[0].1, "light.kitchen");
    }
    Ok(())
}

#[tokio::test]
async fn bulk_light_scope_refuses_unknown_ambiguous_and_excluded_rooms() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    for text in [
        "выключи свет в двух комнатах",
        "turn off lights in two rooms",
        "выключи весь свет на кухне",
        "turn off all lights in kitchen",
        "выключи весь свет кроме кухни",
        "turn off all lights except kitchen",
        "выключи свет в комнатах",
        "turn off lights in rooms",
    ] {
        assert!(
            commands::execute_text(1, true, CommandSource::Text, text, &config)
                .await
                .is_err(),
            "{text}"
        );
    }
    db::rooms::sync_rooms_from_ha("kitchen1", "Кухня", &config.db).await?;
    db::rooms::sync_rooms_from_ha("kitchen2", "Кухня", &config.db).await?;
    assert!(commands::execute_text(
        1,
        true,
        CommandSource::Text,
        "выключи весь свет на кухне",
        &config
    )
    .await
    .is_err());
    assert!(
        commands::execute_text(42, false, CommandSource::Text, "выключи весь свет", &config)
            .await
            .is_err()
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    let CommandExecution::NeedsConfirmation { pending_id } =
        commands::execute_text(1, true, CommandSource::Text, "выключи весь свет", &config).await?
    else {
        panic!()
    };
    let message = commands::pending_result_text(1, pending_id, &config).await?;
    assert!(message.contains("Весь дом"));
    // A device moved after review is no longer a valid frozen target.
    sqlx::query("UPDATE devices SET room_id = 2 WHERE entity_id = 'light.bedroom'")
        .execute(&config.db)
        .await?;
    commands::confirm_pending(1, true, pending_id, &config).await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn climate_queue_rejects_old_admission_after_delete_and_readd() -> anyhow::Result<()> {
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let first = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    ha.write_started.notified().await;
    let queued = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    db::delete_user(42, &config.db).await?;
    db::add_user(42, &config.db).await?;
    let new_request = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    ha.release_write.notify_one();
    assert!(first.await?.is_ok());
    assert!(queued.await?.is_err());
    assert!(new_request.await?.is_ok());
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    Ok(())
}

#[tokio::test]
async fn climate_rechecks_admission_after_slow_state_read() -> anyhow::Result<()> {
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let queued = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    ha.read_started.notified().await;
    db::delete_user(42, &config.db).await?;
    db::add_user(42, &config.db).await?;
    ha.release_read.notify_one();
    assert!(queued.await?.is_err());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn shutdown_rejects_queued_temperature_without_repeating_inflight_action(
) -> anyhow::Result<()> {
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let first = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    ha.write_started.notified().await;
    let second = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    config.runtime_status.write().await.shutdown_requested_at = Some(Utc::now());
    ha.release_write.notify_one();
    assert!(first.await?.is_ok());
    assert!(second.await?.is_err());
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn shutdown_during_device_state_read_prevents_dispatch() -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    ha.pause_next_read.store(true, Ordering::SeqCst);
    let command = {
        let config = config.clone();
        tokio::spawn(async move {
            commands::execute_text(42, false, CommandSource::Text, "выключи люстру", &config).await
        })
    };
    ha.read_started.notified().await;
    config.runtime_status.write().await.shutdown_requested_at = Some(Utc::now());
    ha.release_read.notify_one();
    command.await??;
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn named_devices_starting_with_light_words_keep_ordinary_resolution() -> anyhow::Result<()> {
    for user in [42, 1] {
        for (alias, command) in [
            ("Свет стола", "включи свет стола"),
            ("Свет кухни", "включи свет кухни"),
            ("Light desk", "turn on light desk"),
            ("Lights desk", "turn on lights desk"),
            ("Light kitchen", "turn on light kitchen"),
            ("Lights kitchen", "turn on lights kitchen"),
            ("All Night Light", "turn on all night light"),
            ("Свет ночного стола", "включи свет ночного стола"),
        ] {
            let (config, ha) = setup().await?;
            // Deliberately put the named device outside a matching room name:
            // the name must not be reinterpreted as that room's entire scope.
            db::rooms::sync_rooms_from_ha("kitchen", "Кухня", &config.db).await?;
            db::rooms::sync_rooms_from_ha("desk", "Стол", &config.db).await?;
            for room in ["kitchen", "desk"] {
                let entity_id = format!("light.other_{room}");
                db::devices::sync_device(&entity_id, room, "Other", "light", &config.db).await?;
                db::subscriptions::toggle_hidden(&entity_id, &config.db).await?;
                ha.states.lock().unwrap().push(entity(&entity_id, "off"));
            }
            sqlx::query("UPDATE devices SET alias = ? WHERE id = 3")
                .bind(alias)
                .execute(&config.db)
                .await?;
            let execution =
                commands::execute_text(user, user == 1, CommandSource::Text, command, &config)
                    .await?;
            match execution {
                CommandExecution::Done { .. } => {}
                CommandExecution::NeedsChoice { choice_id } => {
                    // Room-qualified matches can be ambiguous under the ordinary
                    // resolver. Selection must remain a single frozen device.
                    assert!(ha.calls.lock().unwrap().is_empty());
                    assert!(matches!(
                        commands::choose_target(user, choice_id, 3, &config).await?,
                        CommandExecution::Done { .. }
                    ));
                }
                other => panic!("user={user} command={command}: {other:?}"),
            }
            let calls = ha.calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "user={user} command={command}");
            assert_eq!(calls[0].1, "light.bedroom", "user={user} command={command}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn literal_location_aliases_keep_existing_choice_semantics_without_bulk_privilege(
) -> anyhow::Result<()> {
    for user in [42, 1] {
        for (alias, command) in [
            ("Свет на кухне", "включи свет на кухне"),
            ("Light in kitchen", "turn on light in kitchen"),
            ("Lights in kitchen", "turn on lights in kitchen"),
        ] {
            let (config, ha) = setup().await?;
            sqlx::query("UPDATE devices SET alias = ? WHERE id = 3")
                .bind(alias)
                .execute(&config.db)
                .await?;
            let execution = commands::execute_voice_text(user, user == 1, command, &config).await?;
            assert!(
                matches!(execution, CommandExecution::Done { .. }),
                "user={user} command={command}: {execution:?}"
            );
            assert_eq!(ha.calls.lock().unwrap().len(), 1);
            assert_eq!(ha.calls.lock().unwrap()[0].1, "light.bedroom");
        }
    }
    // Two identically named ordinary devices still ask for a stable target.
    let (config, ha) = setup().await?;
    second_light(&config, &ha).await?;
    sqlx::query("UPDATE devices SET alias = 'Свет стола' WHERE entity_id LIKE 'light.%'")
        .execute(&config.db)
        .await?;
    assert!(matches!(
        commands::execute_text(42, false, CommandSource::Text, "включи свет стола", &config)
            .await?,
        CommandExecution::NeedsChoice { .. }
    ));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn explicit_bulk_grammar_overrides_literal_names_and_preserves_complete_room_names(
) -> anyhow::Result<()> {
    let (mut config, ha) = setup().await?;
    Arc::get_mut(&mut config).unwrap().voice_confirm_dangerous = false;
    db::rooms::sync_rooms_from_ha("living_room", "Living room", &config.db).await?;
    db::devices::sync_device("light.living", "living_room", "Other", "light", &config.db).await?;
    db::subscriptions::toggle_hidden("light.living", &config.db).await?;
    ha.states.lock().unwrap().push(entity("light.living", "on"));
    sqlx::query("UPDATE devices SET alias = 'All lights in living room' WHERE id = 3")
        .execute(&config.db)
        .await?;
    assert!(commands::execute_text(
        42,
        false,
        CommandSource::Text,
        "turn off all lights in living room",
        &config
    )
    .await
    .is_err());
    let CommandExecution::NeedsConfirmation { pending_id } = commands::execute_text(
        1,
        true,
        CommandSource::Text,
        "turn off all lights in living room",
        &config,
    )
    .await?
    else {
        panic!("explicit bulk phrase must stay scoped and require confirmation")
    };
    let message = commands::pending_result_text(1, pending_id, &config).await?;
    assert!(message.contains("Living room"));
    assert!(message.contains("1 устройств"));
    assert!(ha.calls.lock().unwrap().is_empty());
    commands::confirm_pending(1, true, pending_id, &config).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(ha.calls.lock().unwrap()[0].1, "light.living");
    // Even literal aliases cannot turn an excluded/indefinite scope into a write.
    for (alias, text) in [
        ("Весь свет кроме кухни", "выключи весь свет кроме кухни"),
        ("Свет в двух комнатах", "выключи свет в двух комнатах"),
        (
            "All lights except kitchen",
            "turn off all lights except kitchen",
        ),
        ("Свет везде кроме кухни", "выключи свет везде кроме кухни"),
        ("Везде свет кроме кухни", "выключи везде свет кроме кухни"),
        (
            "Lights everywhere except kitchen",
            "turn off lights everywhere except kitchen",
        ),
        (
            "Everywhere lights except kitchen",
            "turn off everywhere lights except kitchen",
        ),
    ] {
        sqlx::query("UPDATE devices SET alias = ? WHERE id = 3")
            .bind(alias)
            .execute(&config.db)
            .await?;
        for user in [42, 1] {
            assert!(
                commands::execute_text(user, user == 1, CommandSource::Text, text, &config)
                    .await
                    .is_err(),
                "user={user}: {text}"
            );
        }
    }
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    Ok(())
}

async fn assert_bulk_snapshot_change_during_read_is_rejected(
    mutation: &'static str,
) -> anyhow::Result<()> {
    for command in ["выключи весь свет в спальне", "выключи весь свет"]
    {
        let (config, ha) = setup().await?;
        db::rooms::sync_rooms_from_ha("kitchen", "Кухня", &config.db).await?;
        let CommandExecution::NeedsConfirmation { pending_id, .. } =
            commands::execute_text(1, true, CommandSource::Text, command, &config).await?
        else {
            panic!("expected frozen bulk confirmation")
        };
        ha.pause_next_read.store(true, Ordering::SeqCst);
        let confirmation = {
            let config = config.clone();
            tokio::spawn(
                async move { commands::confirm_pending(1, true, pending_id, &config).await },
            )
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ha.read_started.notified(),
        )
        .await?;
        sqlx::query(mutation).execute(&config.db).await?;
        ha.release_read.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), confirmation).await???;

        assert!(
            ha.calls.lock().unwrap().is_empty(),
            "snapshot changed during HA read: {command}; {mutation}"
        );
        let pending = db::pending_commands::get_pending(pending_id, 1, &config.db)
            .await?
            .unwrap();
        let result: ActionResult = serde_json::from_str(pending.result_json.as_deref().unwrap())?;
        assert_eq!(
            result.outcome,
            Outcome::Failed,
            "rejected before dispatch, never unknown: {mutation}"
        );
        assert_eq!(result.targets.len(), 1);
        assert_eq!(result.targets[0].id, 3);
        assert_eq!(result.targets[0].outcome, Outcome::Failed);
        assert_eq!(pending.status, "failed");
        let reads = ha.state_reads.load(Ordering::SeqCst);
        commands::confirm_pending(1, true, pending_id, &config).await?;
        assert!(ha.calls.lock().unwrap().is_empty());
        assert_eq!(
            ha.state_reads.load(Ordering::SeqCst),
            reads,
            "consumed confirmation must not reread/retry"
        );
    }
    Ok(())
}

#[tokio::test]
async fn bulk_snapshot_room_change_during_ha_read_is_rejected_before_dispatch() -> anyhow::Result<()>
{
    assert_bulk_snapshot_change_during_read_is_rejected(
        "UPDATE devices SET room_id = (SELECT id FROM rooms WHERE area = 'kitchen') WHERE id = 3",
    )
    .await
}

#[tokio::test]
async fn bulk_snapshot_archive_during_ha_read_is_rejected_before_dispatch() -> anyhow::Result<()> {
    assert_bulk_snapshot_change_during_read_is_rejected(
        "UPDATE devices SET archived = 1 WHERE id = 3",
    )
    .await
}

#[tokio::test]
async fn bulk_snapshot_entity_change_during_ha_read_is_rejected_before_dispatch(
) -> anyhow::Result<()> {
    assert_bulk_snapshot_change_during_read_is_rejected(
        "UPDATE devices SET entity_id = 'light.replaced' WHERE id = 3",
    )
    .await
}

#[tokio::test]
async fn bulk_snapshot_changed_target_is_partial_without_widening_or_retry() -> anyhow::Result<()> {
    for command in ["выключи весь свет в спальне", "выключи весь свет"]
    {
        let (config, ha) = setup().await?;
        db::rooms::sync_rooms_from_ha("kitchen", "Кухня", &config.db).await?;
        db::devices::sync_device(
            "light.second",
            "bedroom",
            "B unchanged",
            "light",
            &config.db,
        )
        .await?;
        db::subscriptions::toggle_hidden("light.second", &config.db).await?;
        ha.states.lock().unwrap().push(entity("light.second", "on"));
        sqlx::query("UPDATE devices SET alias = 'A changed' WHERE id = 3")
            .execute(&config.db)
            .await?;
        let unchanged: i64 =
            sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = 'light.second'")
                .fetch_one(&config.db)
                .await?;
        let CommandExecution::NeedsConfirmation { pending_id, .. } =
            commands::execute_text(1, true, CommandSource::Text, command, &config).await?
        else {
            panic!("expected frozen bulk confirmation")
        };
        ha.pause_next_read.store(true, Ordering::SeqCst);
        let confirmation = {
            let config = config.clone();
            tokio::spawn(
                async move { commands::confirm_pending(1, true, pending_id, &config).await },
            )
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ha.read_started.notified(),
        )
        .await?;
        sqlx::query("UPDATE devices SET room_id = (SELECT id FROM rooms WHERE area = 'kitchen') WHERE id = 3")
            .execute(&config.db).await?;
        // Neither a new member of the original room nor a new whole-home member
        // may join this already claimed operation while its HA read is pending.
        for room in ["bedroom", "kitchen"] {
            let entity_id = format!("light.new_{room}");
            db::devices::sync_device(&entity_id, room, "New", "light", &config.db).await?;
            db::subscriptions::toggle_hidden(&entity_id, &config.db).await?;
            ha.states.lock().unwrap().push(entity(&entity_id, "on"));
        }
        ha.release_read.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), confirmation).await???;

        assert_eq!(
            ha.calls.lock().unwrap().len(),
            1,
            "only unchanged approved target: {command}"
        );
        assert_eq!(ha.calls.lock().unwrap()[0].1, "light.second");
        let pending = db::pending_commands::get_pending(pending_id, 1, &config.db)
            .await?
            .unwrap();
        let result: ActionResult = serde_json::from_str(pending.result_json.as_deref().unwrap())?;
        assert_eq!(result.outcome, Outcome::Partial);
        assert_eq!(result.targets.len(), 2);
        assert_eq!(
            result
                .targets
                .iter()
                .find(|target| target.id == 3)
                .unwrap()
                .outcome,
            Outcome::Failed
        );
        assert_eq!(
            result
                .targets
                .iter()
                .find(|target| target.id == unchanged)
                .unwrap()
                .outcome,
            Outcome::Sent
        );
        assert!(result
            .targets
            .iter()
            .all(|target| target.outcome != Outcome::Unknown));
        let reads = ha.state_reads.load(Ordering::SeqCst);
        commands::confirm_pending(1, true, pending_id, &config).await?;
        assert_eq!(ha.calls.lock().unwrap().len(), 1);
        assert_eq!(ha.state_reads.load(Ordering::SeqCst), reads);
    }
    Ok(())
}
