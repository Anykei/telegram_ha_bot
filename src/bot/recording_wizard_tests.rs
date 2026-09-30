use super::*;
use crate::bot::recording_rule_wizard::{self, RecordingRuleWizard, WizardTriggerMode};

fn snapshot(config: &AppConfig) -> RecordingRuleWizard {
    config
        .sessions
        .get(&1)
        .unwrap()
        .recording_rule_wizard
        .clone()
        .unwrap()
}
fn action(config: &AppConfig, action: AdminPayload) -> Payload {
    let wizard = snapshot(config);
    Payload::Admin(AdminPayload::WizardDraft {
        id: wizard.draft_id.unwrap(),
        revision: wizard.revision,
        action: Box::new(action),
    })
}
async fn open_wizard(config: &Arc<AppConfig>, action: AdminPayload) -> anyhow::Result<View> {
    router(self::action(config, action), 1, config.clone()).await
}

async fn fixture() -> anyhow::Result<(Arc<AppConfig>, Arc<FakeHa>, i64)> {
    let (config, ha) = setup().await?;
    db::add_user(1, &config.db).await?;
    let camera = camera_for_ui_tests(&config).await?;
    router(
        Payload::Admin(AdminPayload::StartRecordingRuleWizard { room: 1 }),
        1,
        config.clone(),
    )
    .await?;
    let device: i64 =
        sqlx::query_scalar("SELECT id FROM devices WHERE entity_id='sensor.temperature'")
            .fetch_one(&config.db)
            .await?;
    open_wizard(
        &config,
        AdminPayload::WizardPickEntity {
            room: 1,
            camera,
            device,
        },
    )
    .await?;
    open_wizard(
        &config,
        AdminPayload::WizardPickMode {
            room: 1,
            camera,
            device,
            mode: WizardTriggerMode::AnyChange,
        },
    )
    .await?;
    let group = db::camera_recording_rule_groups::create_group("Night", &config.db).await?;
    {
        let mut session = config.sessions.get_mut(&1).unwrap();
        let wizard = session.recording_rule_wizard.as_mut().unwrap();
        wizard.name = Some("Custom name".into());
        wizard.name_touched = true;
        wizard.tail_seconds = Some(120);
        wizard.retention_days = Some(90);
        wizard.max_segment_seconds = Some(240);
        wizard.cooldown_s = Some(123);
        wizard.cooldown_touched = true;
        wizard.group_ids = vec![group];
        wizard.active_time =
            db::camera_recording_rules::RecordingRuleActiveTime::window(1320, 420, 31)?;
        wizard.add_extra_condition(
            recording_rule_wizard::condition_from_input(
                "light.bedroom",
                db::camera_recording_rules::ConditionOperator::Is,
                "on",
            )
            .unwrap(),
        );
    }
    recording_rule_wizard::persist_draft(&config, 1).await?;
    Ok((config, ha, camera))
}

fn values(mut wizard: RecordingRuleWizard) -> RecordingRuleWizard {
    wizard.step.clear();
    wizard.revision = 0;
    wizard.return_to_summary = false;
    wizard.editing_text = false;
    wizard
}

#[tokio::test]
async fn navigation_refresh_restart_and_stale_callback_preserve_draft() -> anyhow::Result<()> {
    let (config, ha, camera) = fixture().await?;
    let before = snapshot(&config);
    let device = before.source_device_id.unwrap();
    let stale = action(&config, AdminPayload::WizardPickWizardTail { tail: 30 });
    for action in [
        AdminPayload::WizardPickCamera { room: 1, camera },
        AdminPayload::WizardPickEntity {
            room: 1,
            camera,
            device,
        },
        AdminPayload::WizardPickMode {
            room: 1,
            camera,
            device,
            mode: WizardTriggerMode::AnyChange,
        },
        AdminPayload::WizardConditions,
        AdminPayload::WizardNextTail,
        AdminPayload::WizardActiveTime,
        AdminPayload::WizardGroups,
        AdminPayload::WizardConfirmGroups,
    ] {
        open_wizard(&config, action).await?;
        assert_eq!(values(snapshot(&config)), values(before.clone()));
    }
    let id = before.draft_id.unwrap();
    let revision = snapshot(&config).revision;
    let refresh = Payload::Admin(AdminPayload::WizardResume { id });
    router(refresh.clone(), 1, config.clone()).await?;
    assert_eq!(snapshot(&config).revision, revision);
    config.sessions.clear();
    router(refresh, 1, config.clone()).await?;
    assert_eq!(values(snapshot(&config)), values(before));
    let stale_result = router(stale, 1, config.clone()).await?;
    assert!(stale_result.alert.is_some());
    assert_eq!(snapshot(&config).tail_seconds, Some(120));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn groups_back_has_linear_and_summary_destinations() -> anyhow::Result<()> {
    let (config, _, _) = fixture().await?;
    open_wizard(&config, AdminPayload::WizardActiveTime).await?;
    let groups = open_wizard(&config, AdminPayload::WizardGroups).await?;
    assert!(callbacks(&groups).iter().any(|p|matches!(p,Payload::Admin(AdminPayload::WizardDraft { action,.. }) if **action==AdminPayload::WizardActiveTime)));
    open_wizard(&config, AdminPayload::WizardConfirmGroups).await?;
    let groups = open_wizard(&config, AdminPayload::WizardGroups).await?;
    let back=callbacks(&groups).into_iter().filter(|p|matches!(p,Payload::Admin(AdminPayload::WizardDraft { action,.. }) if **action==AdminPayload::WizardConfirmGroups)).count();
    assert_eq!(back, 2); // Done and Back both return to the review.
    Ok(())
}

#[tokio::test]
async fn submit_is_bound_to_owner_and_idempotent_after_restart() -> anyhow::Result<()> {
    let (config, ha, _) = fixture().await?;
    open_wizard(&config, AdminPayload::WizardConfirmGroups).await?;
    let submit = action(&config, AdminPayload::WizardCreateCurrentRule);
    let denied = router(submit.clone(), 42, config.clone()).await?;
    assert!(denied.alert.is_some());
    assert!(db::camera_recording_rules::list_rules(&config.db)
        .await?
        .is_empty());
    let first = router(submit.clone(), 1, config.clone()).await?;
    config.sessions.clear();
    let repeated = router(submit, 1, config.clone()).await?;
    assert_eq!(first.payload, repeated.payload);
    assert_eq!(
        db::camera_recording_rules::list_rules(&config.db)
            .await?
            .len(),
        1
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[test]
fn bound_wizard_callbacks_fit_telegram_and_refresh_only_reads() {
    let payload = Payload::Admin(AdminPayload::WizardDraft {
        id: 1_000_000,
        revision: 1_000_000,
        action: Box::new(AdminPayload::WizardPickEntity {
            room: 1_000_000,
            camera: 1_000_000,
            device: 1_000_000,
        }),
    });
    assert!(payload.to_string().len() <= 64);
    assert_eq!(Payload::from_string(&payload.to_string()).unwrap(), payload);
    assert_eq!(
        payload.for_refresh(),
        Payload::Admin(AdminPayload::WizardResume { id: 1_000_000 })
    );
}

#[tokio::test]
async fn removed_source_can_be_repaired_and_cancel_affects_only_its_draft() -> anyhow::Result<()> {
    let (config, _, _) = fixture().await?;
    let before = snapshot(&config);
    sqlx::query("UPDATE devices SET archived=1 WHERE id=?")
        .bind(before.source_device_id)
        .execute(&config.db)
        .await?;
    let review = open_wizard(&config, AdminPayload::WizardConfirmGroups).await?;
    assert!(review.text.contains("сохранены"));
    assert_eq!(snapshot(&config).group_ids, before.group_ids);
    let resume = review.payload;
    open_wizard(&config, AdminPayload::WizardCancel).await?;
    let expired = router(resume, 1, config.clone()).await?;
    assert!(expired.alert.is_some());
    assert!(db::camera_recording_rules::list_rules(&config.db)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn active_time_presets_keep_selected_days_and_pre_roll_stays_in_draft() -> anyhow::Result<()>
{
    let (config, _, _) = fixture().await?;
    let days = snapshot(&config).active_time.days_mask;
    open_wizard(
        &config,
        AdminPayload::WizardPickActiveTimePreset {
            preset: RecordingRuleActiveTimePreset::Day,
        },
    )
    .await?;
    assert_eq!(snapshot(&config).active_time.days_mask, days);
    let enabled = snapshot(&config).pre_roll_enabled.unwrap_or(false);
    open_wizard(&config, AdminPayload::WizardTogglePreRoll).await?;
    assert_eq!(snapshot(&config).pre_roll_enabled, Some(!enabled));
    assert!(db::camera_recording_rules::list_rules(&config.db)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn cancelling_input_uses_bound_draft_and_expired_text_never_becomes_independent_create(
) -> anyhow::Result<()> {
    let (config, _, _) = fixture().await?;
    let form = open_wizard(&config, AdminPayload::WizardAdvancedCurrentText).await?;
    config.sessions.get_mut(&1).unwrap().current_context = form.payload.to_string();
    let original = snapshot(&config);
    // A newer unrelated draft must not replace an explicitly resumed older form.
    db::ui_drafts::create(
        1,
        recording_rule_wizard::DRAFT_KIND,
        &serde_json::to_string(&original)?,
        24,
        &config.db,
    )
    .await?;
    let cancel = crate::bot::handlers::cancel_input_payload(
        &State::AddRecordingRule { room_id: 1 },
        1,
        &config,
    )
    .await?;
    assert_eq!(
        cancel,
        Payload::Admin(AdminPayload::WizardResume {
            id: original.draft_id.unwrap()
        })
    );
    assert_eq!(snapshot(&config), original);
    sqlx::query("UPDATE ui_drafts SET expires_at=0 WHERE id=?")
        .bind(original.draft_id)
        .execute(&config.db)
        .await?;
    assert!(crate::bot::handlers::recording_text_draft(&config, 1, 1)
        .await
        .is_err());
    assert!(db::camera_recording_rules::list_rules(&config.db)
        .await?
        .is_empty());
    config.sessions.get_mut(&1).unwrap().current_context =
        Payload::Admin(AdminPayload::PromptAddRecordingRule { room: 1 }).to_string();
    assert!(crate::bot::handlers::recording_text_draft(&config, 1, 1)
        .await?
        .is_none());
    assert_eq!(
        crate::bot::handlers::cancel_input_payload(
            &State::AddRecordingRule { room_id: 1 },
            1,
            &config
        )
        .await?,
        Payload::Admin(AdminPayload::RecordingRules { room: 1 })
    );
    Ok(())
}

#[tokio::test]
async fn cancel_preserves_admin_settings_origin_and_paging_does_not_reopen_form(
) -> anyhow::Result<()> {
    let (config, _, _) = fixture().await?;
    config.sessions.get_mut(&1).unwrap().current_context =
        Payload::AdminSettings(SettingsPayload::DeviceDetail { room: 1, device: 2 }).to_string();
    assert_eq!(
        crate::bot::handlers::cancel_input_payload(
            &State::WaitingForName {
                room_id: 1,
                device_id: 2
            },
            1,
            &config
        )
        .await?,
        Payload::AdminSettings(SettingsPayload::DeviceDetail { room: 1, device: 2 })
    );
    let context = serde_json::json!({"source":Payload::AdminSettings(SettingsPayload::DeviceDetail { room:1,device:2 }).to_string(),"notice":null,"alert":null});
    let screen = db::ui_text_pages::register(1, &context.to_string(), &config.db).await?;
    config.sessions.get_mut(&1).unwrap().current_context =
        Payload::TextPage { screen, page: 0 }.to_string();
    assert_eq!(
        crate::bot::handlers::cancel_input_payload(
            &State::WaitingForName {
                room_id: 1,
                device_id: 2
            },
            1,
            &config
        )
        .await?,
        Payload::AdminSettings(SettingsPayload::DeviceDetail { room: 1, device: 2 })
    );
    let mut form = open_wizard(&config, AdminPayload::WizardAdvancedCurrentText).await?;
    form.text.push_str(&"\nLong form context".repeat(400));
    form.next_state = None; // /cancel exits the field while retaining the draft.
    crate::bot::text_pages::prepare(&mut form, 1, &config).await?;
    let page = router(form.payload, 1, config.clone()).await?;
    assert!(page.next_state.is_none());
    assert!(snapshot(&config).editing_text); // Explicit Continue can still restore the saved field.
    Ok(())
}

#[tokio::test]
async fn exact_temperature_text_queues_before_step_and_late_result_preserves_new_dialogue(
) -> anyhow::Result<()> {
    use crate::core::climate_control::TemperatureCommand;
    let (config, ha) = setup().await?;
    ha.pause_next_write.store(true, Ordering::SeqCst);
    ha.stale_temperature.store(true, Ordering::SeqCst);
    let chat = teloxide::types::ChatId(42);
    let dialogue = crate::bot::handlers::MyDialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::new(),
        chat,
    );
    let state = State::ExactTemperature {
        room_id: 1,
        device_id: 1,
    };
    dialogue.update(state.clone()).await?;
    let message: teloxide::types::Message = serde_json::from_value(serde_json::json!({
        "message_id":11,"date":1700000000,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Test"},"text":"20"
    }))?;
    let bot = teloxide::Bot::new("123:unused").set_api_url("http://127.0.0.1:1".parse()?);
    crate::bot::personal::handle_input(bot, message, config.clone(), dialogue.clone(), state)
        .await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ha.write_started.notified(),
    )
    .await?;
    let worker_token = config.view_updates.get(&42).unwrap().clone();
    let step = config
        .climate_controls
        .enqueue(config.clone(), 42, 1, TemperatureCommand::Step(1))
        .await?;
    config.cancel_view_update(42);
    let new_state = State::PersonalQuietHours;
    dialogue.update(new_state.clone()).await?;
    ha.release_write.notify_one();
    assert_eq!(step.await??.target, 20.5);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while Arc::strong_count(&worker_token) > 1 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(matches!(
        dialogue.get().await?,
        Some(State::PersonalQuietHours)
    ));
    let temperatures = ha
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, _, data)| data["temperature"].as_f64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(temperatures, vec![20.0, 20.5]);
    Ok(())
}

#[tokio::test]
async fn cancel_command_choice_returns_to_owned_read_context() -> anyhow::Result<()> {
    let (config, _, _) = fixture().await?;
    let back = Payload::Control(ControlPayload::DeviceControl { room: 1, device: 3 });
    let context = crate::core::commands::ChoiceContext {
        action: crate::core::commands::ChoiceAction::Snapshot,
        candidates: vec![1, 2],
        text: "entrance".into(),
        source: crate::core::commands::CommandSource::Text,
        engine: "deterministic".into(),
        back: back.clone(),
    };
    let id = db::command_choices::create(1, &serde_json::to_string(&context)?, &config.db).await?;
    config.sessions.get_mut(&1).unwrap().current_context =
        Payload::Command(CommandPayload::Choice { id, page: 0 }).to_string();
    assert_eq!(
        crate::bot::handlers::cancel_input_payload(&State::Idle, 1, &config).await?,
        back
    );
    let stranger_id =
        db::command_choices::create(42, &serde_json::to_string(&context)?, &config.db).await?;
    config.sessions.get_mut(&1).unwrap().current_context =
        Payload::Command(CommandPayload::Choice {
            id: stranger_id,
            page: 0,
        })
        .to_string();
    assert_eq!(
        crate::bot::handlers::cancel_input_payload(&State::Idle, 1, &config).await?,
        Payload::Home
    );
    Ok(())
}
