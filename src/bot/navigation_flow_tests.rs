use super::*;
use crate::bot::router::{router, CommandPayload};
use crate::bot::schedule_drafts::SchedulePayload;
use crate::db::action_groups::{ActionScheduleCommand, ActionTargetRef};
use crate::i18n::Language;
use crate::models::AppConfig;
use std::sync::Arc;
use teloxide::types::InlineKeyboardButtonKind;

async fn config() -> Result<Arc<AppConfig>> {
    let mut config = crate::test_support::config().await;
    let (tx, _) = tokio::sync::mpsc::channel(10);
    let ha = Arc::new(crate::ha::mock::MockHa::open(config.db.clone(), tx).await?);
    ha.seed_inventory().await?;
    Arc::get_mut(&mut config).unwrap().ha_client = ha;
    db::add_user(1, &config.db).await?;
    Ok(config)
}

fn callbacks(view: &View) -> Vec<Payload> {
    view.kb
        .inline_keyboard
        .iter()
        .flatten()
        .filter_map(|button| {
            let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                return None;
            };
            Payload::from_string(data).ok()
        })
        .collect()
}

#[tokio::test]
async fn schedule_time_cancel_returns_to_same_draft_without_cancelling_it() -> Result<()> {
    let config = config().await?;
    let group = db::action_groups::create_action_group("Evening", &config.db).await?;
    let target = ActionTargetRef::BotGroup(group);
    let initial = router(
        Payload::Schedule(SchedulePayload::Start {
            target,
            command: ActionScheduleCommand::TurnOn,
        }),
        1,
        config.clone(),
    )
    .await?;
    let Some(crate::bot::router::State::ScheduleDraftTime { id, revision }) = initial.next_state
    else {
        panic!("Expected time input")
    };
    let before = db::ui_drafts::load(id, 1, db::schedule_drafts::KIND, &config.db)
        .await?
        .unwrap();
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        let input = router(
            Payload::Schedule(SchedulePayload::Time { id, revision }),
            1,
            config.clone(),
        )
        .await?;
        assert_eq!(
            input.html_back,
            Some(Payload::Schedule(SchedulePayload::Review { id }))
        );
        let review = router(input.html_back.unwrap(), 1, config.clone()).await?;
        assert!(review.next_state.is_none());
        assert_eq!(
            review.payload,
            Payload::Schedule(SchedulePayload::Review { id })
        );
        assert!(callbacks(&review)
            .contains(&Payload::Schedule(SchedulePayload::Cancel { id, revision })));
        let after = db::ui_drafts::load(id, 1, db::schedule_drafts::KIND, &config.db)
            .await?
            .unwrap();
        assert_eq!(before.revision, after.revision);
        assert_eq!(before.payload, after.payload);
    }
    assert!(db::action_groups::list_action_schedules(target, &config.db)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn active_confirmation_exit_cancels_operation_instead_of_only_navigating() -> Result<()> {
    let config = config().await?;
    let (room, device): (i64, i64) =
        sqlx::query_as("SELECT room_id, id FROM devices WHERE entity_id = 'light.floor_lamp'")
            .fetch_one(&config.db)
            .await?;
    db::devices::toggle_device_critical(device, &config.db).await?;
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        let view = router(
            Payload::Control(ControlPayload::QuickAction {
                room,
                device,
                cmd: DeviceCmd::TurnOn,
            }),
            1,
            config.clone(),
        )
        .await?;
        let Payload::Control(ControlPayload::ReviewAction { id }) = view.payload else {
            panic!("Expected review")
        };
        assert_eq!(
            view.html_back,
            Some(Payload::Control(ControlPayload::CancelAction { id }))
        );
        assert_eq!(
            pending::get(id, 1, &config.db).await?.unwrap().status,
            "pending"
        );
        let cancelled = router(view.html_back.unwrap(), 1, config.clone()).await?;
        assert_eq!(
            pending::get(id, 1, &config.db).await?.unwrap().status,
            "cancelled"
        );
        assert_eq!(cancelled.html_kind.as_deref(), Some("menu"));
        assert!(!matches!(
            cancelled.html_back,
            Some(Payload::Control(ControlPayload::CancelAction { .. }))
        ));
        // An old Confirm can no longer execute the cancelled operation.
        router(
            Payload::Control(ControlPayload::ConfirmAction { id }),
            1,
            config.clone(),
        )
        .await?;
        let state = config
            .ha_client
            .fetch_states_by_ids(&["light.floor_lamp".into()])
            .await?;
        assert_eq!(state[0].state, "off");
    }
    Ok(())
}

#[tokio::test]
async fn pending_text_command_has_a_real_cancel_exit() -> Result<()> {
    let config = config().await?;
    let device: i64 =
        sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = 'light.floor_lamp'")
            .fetch_one(&config.db)
            .await?;
    let intent = crate::core::commands::CommandIntent::DeviceAction {
        device_id: device,
        action: crate::core::commands::SerializableDeviceAction::TurnOn,
    };
    let id = db::pending_commands::create(
        1,
        db::pending_commands::PendingCommandSource::Text,
        "test",
        &serde_json::to_string(&intent)?,
        None,
        chrono::Utc::now() + chrono::Duration::seconds(60),
        &config.db,
    )
    .await?;
    let view = router(
        Payload::Command(CommandPayload::Result { id }),
        1,
        config.clone(),
    )
    .await?;
    assert_eq!(
        view.html_back,
        Some(Payload::Command(CommandPayload::Cancel { id }))
    );
    // The callback handler delegates this branch to cancel_pending.
    crate::core::commands::cancel_pending(1, id, &config).await?;
    let result = router(
        Payload::Command(CommandPayload::Result { id }),
        1,
        config.clone(),
    )
    .await?;
    assert_eq!(result.html_back, Some(Payload::Home));
    assert_eq!(
        db::pending_commands::get_pending(id, 1, &config.db)
            .await?
            .unwrap()
            .status,
        "cancelled"
    );
    Ok(())
}

fn wizard_action(config: &AppConfig, action: AdminPayload) -> Payload {
    let wizard = config
        .sessions
        .get(&1)
        .unwrap()
        .recording_rule_wizard
        .clone()
        .unwrap();
    Payload::Admin(AdminPayload::WizardDraft {
        id: wizard.draft_id.unwrap(),
        revision: wizard.revision,
        action: Box::new(action),
    })
}

fn assert_bound_wizard_exit(view: &View, expected: AdminPayload) -> Payload {
    let exit = view.html_back.clone().expect("Wizard needs a pinned exit");
    let Payload::Admin(AdminPayload::WizardDraft { action, .. }) = &exit else {
        panic!("Exit must be revision-bound: {exit:?}")
    };
    assert_eq!(**action, expected);
    assert!(callbacks(view).contains(&exit));
    exit
}

#[tokio::test]
async fn wizard_pinned_exit_keeps_draft_revision_and_rejects_stale_navigation() -> Result<()> {
    let config = config().await?;
    let (room, device): (i64, i64) = sqlx::query_as(
        "SELECT room_id, id FROM devices WHERE entity_id = 'binary_sensor.front_door'",
    )
    .fetch_one(&config.db)
    .await?;
    let camera = db::cameras::add_manual_camera(
        db::cameras::NewCamera {
            name: "Test camera",
            room_id: room,
            stream_url: "mock://camera",
            snapshot_url: None,
            clip_seconds: 10,
        },
        &config.db,
    )
    .await?;
    router(
        Payload::Admin(AdminPayload::StartRecordingRuleWizard { room }),
        1,
        config.clone(),
    )
    .await?;
    let source = router(
        wizard_action(&config, AdminPayload::WizardPickCamera { room, camera }),
        1,
        config.clone(),
    )
    .await?;
    assert_bound_wizard_exit(&source, AdminPayload::StartRecordingRuleWizard { room });
    let modes = router(
        wizard_action(
            &config,
            AdminPayload::WizardPickEntity {
                room,
                camera,
                device,
            },
        ),
        1,
        config.clone(),
    )
    .await?;
    let old_exit =
        assert_bound_wizard_exit(&modes, AdminPayload::WizardPickCamera { room, camera });
    db::add_user(2, &config.db).await?;
    let denied = router(old_exit.clone(), 2, config.clone()).await?;
    assert!(!denied.text.contains("Test camera"));
    assert!(!matches!(
        denied.payload,
        Payload::Admin(AdminPayload::WizardResume { .. })
    ));
    let conditions = router(
        wizard_action(
            &config,
            AdminPayload::WizardPickMode {
                room,
                camera,
                device,
                mode: crate::bot::recording_rule_wizard::WizardTriggerMode::OpenAndClose,
            },
        ),
        1,
        config.clone(),
    )
    .await?;
    assert_bound_wizard_exit(
        &conditions,
        AdminPayload::WizardPickEntity {
            room,
            camera,
            device,
        },
    );
    let before = config
        .sessions
        .get(&1)
        .unwrap()
        .recording_rule_wizard
        .clone()
        .unwrap();
    let stale = router(old_exit, 1, config.clone()).await?;
    assert!(stale
        .alert
        .as_deref()
        .unwrap_or_default()
        .contains("устарел"));
    let after = config
        .sessions
        .get(&1)
        .unwrap()
        .recording_rule_wizard
        .clone()
        .unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.step, after.step);
    assert_eq!(before.source_mode, after.source_mode);
    let tail = router(
        wizard_action(&config, AdminPayload::WizardNextTail),
        1,
        config.clone(),
    )
    .await?;
    assert_bound_wizard_exit(&tail, AdminPayload::WizardConditions);
    let retention = router(
        wizard_action(&config, AdminPayload::WizardPickWizardTail { tail: 60 }),
        1,
        config.clone(),
    )
    .await?;
    assert_bound_wizard_exit(&retention, AdminPayload::WizardNextTail);
    let active = router(
        wizard_action(
            &config,
            AdminPayload::WizardPickWizardRetention { retention: 30 },
        ),
        1,
        config.clone(),
    )
    .await?;
    assert_bound_wizard_exit(&active, AdminPayload::WizardPickWizardTail { tail: 60 });
    let days = router(
        wizard_action(&config, AdminPayload::WizardActiveDays),
        1,
        config.clone(),
    )
    .await?;
    assert_bound_wizard_exit(&days, AdminPayload::WizardActiveTime);
    let advanced = router(
        wizard_action(&config, AdminPayload::WizardAdvancedCurrentText),
        1,
        config.clone(),
    )
    .await?;
    assert_bound_wizard_exit(&advanced, AdminPayload::WizardConfirmGroups);
    assert!(advanced.next_state.is_some());
    let review = router(advanced.html_back.unwrap(), 1, config.clone()).await?;
    assert!(review.next_state.is_none());
    assert_bound_wizard_exit(
        &review,
        AdminPayload::WizardPickWizardRetention { retention: 30 },
    );
    Ok(())
}
