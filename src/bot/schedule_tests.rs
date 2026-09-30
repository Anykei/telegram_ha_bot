use super::*;
use crate::bot::schedule_drafts::SchedulePayload;
use crate::db::action_groups::{ActionScheduleCommand, ActionTargetRef};

#[tokio::test]
async fn schedule_ui_requires_explicit_activation_and_legacy_refresh_never_creates(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = db::action_groups::create_action_group("Evening", &config.db).await?;
    let target = ActionTargetRef::BotGroup(group);
    let old = Payload::Admin(AdminPayload::PromptCreateActionScheduleTime {
        target,
        command: ActionScheduleCommand::TurnOn,
    });
    router(old.for_refresh(), 1, config.clone()).await?;
    assert!(
        db::ui_drafts::latest(1, db::schedule_drafts::KIND, &config.db)
            .await?
            .is_none()
    );
    let start = router(old, 1, config.clone()).await?;
    let Some(State::ScheduleDraftTime { id, revision }) = start.next_state else {
        panic!("time input");
    };
    assert!(db::action_groups::list_action_schedules(target, &config.db)
        .await?
        .is_empty());
    let saved = db::ui_drafts::load(id, 1, db::schedule_drafts::KIND, &config.db)
        .await?
        .unwrap();
    let mut data: db::schedule_drafts::ScheduleDraft = serde_json::from_str(&saved.payload)?;
    data.time_minute = Some(720);
    let revision = db::ui_drafts::save(
        id,
        1,
        revision,
        &serde_json::to_string(&data)?,
        "review",
        "",
        true,
        24,
        &config.db,
    )
    .await?;
    let review = router(
        Payload::Schedule(SchedulePayload::Review { id }),
        1,
        config.clone(),
    )
    .await?;
    assert!(review.text.contains("Время сервера"));
    assert!(review.text.contains("Ближайший расчётный запуск"));
    assert!(
        db::action_groups::list_due_action_schedules(720, 1, "new", &config.db)
            .await?
            .is_empty()
    );
    let submit = Payload::Schedule(SchedulePayload::Save {
        id,
        revision,
        enabled: true,
    });
    router(submit.for_refresh(), 1, config.clone()).await?;
    assert!(db::action_groups::list_action_schedules(target, &config.db)
        .await?
        .is_empty());
    let denied = router(submit.clone(), 42, config.clone()).await?;
    assert!(denied.text.contains("Доступ запрещён"));
    router(submit.clone(), 1, config.clone()).await?;
    router(submit, 1, config.clone()).await?;
    let schedules = db::action_groups::list_action_schedules(target, &config.db).await?;
    assert_eq!(schedules.len(), 1);
    assert!(schedules[0].is_enabled());
    assert!(ha.calls.lock().unwrap().is_empty());
    let edit = router(
        Payload::Schedule(SchedulePayload::Edit {
            target,
            schedule: schedules[0].id,
        }),
        1,
        config.clone(),
    )
    .await?;
    assert!(edit.text.contains("Действующая версия работает"));
    let Payload::Schedule(SchedulePayload::Review { id }) = edit.payload else {
        panic!("draft");
    };
    let data = db::ui_drafts::load(id, 1, db::schedule_drafts::KIND, &config.db)
        .await?
        .unwrap();
    router(
        Payload::Schedule(SchedulePayload::Day {
            id,
            revision: data.revision,
            day: 0,
        }),
        1,
        config.clone(),
    )
    .await?;
    assert_eq!(
        db::action_groups::get_action_schedule(schedules[0].id, &config.db)
            .await?
            .unwrap()
            .days_mask,
        127
    );
    let data = db::ui_drafts::load(id, 1, db::schedule_drafts::KIND, &config.db)
        .await?
        .unwrap();
    router(
        Payload::Schedule(SchedulePayload::Cancel {
            id,
            revision: data.revision,
        }),
        1,
        config.clone(),
    )
    .await?;
    assert_eq!(
        db::action_groups::get_action_schedule(schedules[0].id, &config.db)
            .await?
            .unwrap()
            .days_mask,
        127
    );
    Ok(())
}

#[test]
fn new_callbacks_are_compact_and_refresh_to_reads() {
    use crate::bot::personal::{FavoriteAction, PersonalPayload};
    let variants = vec![
        Payload::Personal(PersonalPayload::AddFavorite {
            action: FavoriteAction::DeviceOn {
                room: i64::MAX,
                device: i64::MAX,
            },
        }),
        Payload::Personal(PersonalPayload::SetSubscription {
            device: i64::MAX,
            enabled: true,
        }),
        Payload::Schedule(SchedulePayload::Save {
            id: i64::MAX,
            revision: i64::MAX,
            enabled: true,
        }),
        Payload::Schedule(SchedulePayload::Day {
            id: i64::MAX,
            revision: i64::MAX,
            day: 6,
        }),
        Payload::TemperatureInput {
            room: i64::MAX,
            device: i64::MAX,
        },
        Payload::GroupDevices {
            group: i64::MAX,
            page: u16::MAX,
        },
    ];
    for payload in variants {
        assert!(payload.to_string().len() <= 64);
        assert_eq!(Payload::from_string(&payload.to_string()).unwrap(), payload);
        assert!(!matches!(
            payload.for_refresh(),
            Payload::Personal(
                PersonalPayload::AddFavorite { .. } | PersonalPayload::SetSubscription { .. }
            ) | Payload::Schedule(SchedulePayload::Save { .. } | SchedulePayload::Day { .. })
        ));
    }
}

#[test]
fn next_run_matches_current_minute_without_catching_up_or_repeating_last_run() {
    use chrono::{Local, TimeZone};
    let now = Local
        .with_ymd_and_hms(2026, 9, 29, 12, 0, 40)
        .single()
        .unwrap();
    let current = crate::bot::schedule_drafts::next_run(now, 720, 127, None).unwrap();
    assert_eq!(
        current.format("%Y-%m-%d %H:%M").to_string(),
        "2026-09-29 12:00"
    );
    let later =
        crate::bot::schedule_drafts::next_run(now, 720, 127, Some("2026-09-29 12:00")).unwrap();
    assert_eq!(
        later.format("%Y-%m-%d %H:%M").to_string(),
        "2026-09-30 12:00"
    );
    let missed = crate::bot::schedule_drafts::next_run(now, 719, 127, None).unwrap();
    assert_eq!(
        missed.format("%Y-%m-%d %H:%M").to_string(),
        "2026-09-30 11:59"
    );
}

#[tokio::test]
async fn explicit_chart_with_long_caption_keeps_photo_and_pages_during_telegram_retry(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let payload = Payload::Control(ControlPayload::QuickAction {
        room: 1,
        device: 2,
        cmd: DeviceCmd::ShowChart { h: 24, o: 0 },
    });
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &payload.to_string(),
        crate::models::UiMessageMode::Photo,
    )
    .await;
    let mut view = router(payload, 42, config.clone()).await?;
    view.text = "История температуры 🌡\n".repeat(400);
    view.image = Some(b"explicit-chart-preserved".to_vec());
    let (bot,server)=mock_telegram_edits(vec![
        (429,serde_json::json!({"ok":false,"error_code":429,"description":"Too Many Requests: retry after 1","parameters":{"retry_after":1}})),
        (200,telegram_message(7,true)),
    ]).await?;
    crate::bot::handlers::update_view(
        &bot,
        teloxide::types::ChatId(42),
        teloxide::types::MessageId(7),
        42,
        view,
        config.clone(),
    )
    .await?;
    for request in server.await?? {
        assert!(request["multipart"]
            .as_str()
            .unwrap()
            .contains("explicit-chart-preserved"));
    }
    let session = config.sessions.get(&42).unwrap();
    assert_eq!(session.ui_message_mode, crate::models::UiMessageMode::Photo);
    assert!(matches!(
        Payload::from_string(&session.current_context)?,
        Payload::TextPage { .. }
    ));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn paginated_schedule_time_prompt_preserves_input_and_never_activates_draft(
) -> anyhow::Result<()> {
    let (config, ha) = setup().await?;
    let group = db::action_groups::create_action_group("Long title", &config.db).await?;
    sqlx::query("UPDATE action_groups SET name = ? WHERE id = ?")
        .bind("Длинное имя сценария 🌡 ".repeat(500))
        .bind(group)
        .execute(&config.db)
        .await?;
    let target = ActionTargetRef::BotGroup(group);
    let prompt = router(
        Payload::Schedule(SchedulePayload::Start {
            target,
            command: ActionScheduleCommand::TurnOn,
        }),
        1,
        config.clone(),
    )
    .await?;
    let Some(State::ScheduleDraftTime { id, revision }) = prompt.next_state else {
        panic!("The initial prompt must accept schedule time");
    };
    let Payload::TextPage { screen, page: 0 } = prompt.payload else {
        panic!("The long target name must use text pagination");
    };
    for page in [1, 0, 2] {
        let view = router(Payload::TextPage { screen, page }, 1, config.clone()).await?;
        assert!(
            matches!(view.next_state, Some(State::ScheduleDraftTime { id: current_id, revision: current_revision }) if current_id == id && current_revision == revision)
        );
        assert!(crate::bot::text_pages::text_len(&view.get_plain_text()) <= 4096);
        assert!(
            callbacks(&view).contains(&Payload::Admin(AdminPayload::ActionSchedules { target }))
        );
    }
    let draft = db::ui_drafts::load(id, 1, db::schedule_drafts::KIND, &config.db)
        .await?
        .unwrap();
    assert_eq!(draft.revision, revision);
    let data: db::schedule_drafts::ScheduleDraft = serde_json::from_str(&draft.payload)?;
    assert!(data.time_minute.is_none());
    assert!(db::action_groups::list_action_schedules(target, &config.db)
        .await?
        .is_empty());
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn delayed_schedule_time_saves_draft_without_replacing_newer_dialogue() -> anyhow::Result<()>
{
    let (config, ha) = setup().await?;
    let group = db::action_groups::create_action_group("Evening", &config.db).await?;
    let target = ActionTargetRef::BotGroup(group);
    let start = router(
        Payload::Schedule(SchedulePayload::Start {
            target,
            command: ActionScheduleCommand::TurnOn,
        }),
        1,
        config.clone(),
    )
    .await?;
    let Some(State::ScheduleDraftTime { id, revision }) = start.next_state else {
        panic!("Expected schedule time prompt");
    };
    let dialogue = crate::bot::handlers::MyDialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::new(),
        teloxide::types::ChatId(1),
    );
    dialogue
        .update(State::ScheduleDraftTime { id, revision })
        .await?;
    let msg = serde_json::from_value(serde_json::json!({
        "message_id": 9, "date": 1, "chat": {"id": 1, "type": "private"},
        "from": {"id": 1, "is_bot": false, "first_name": "Root"}, "text": "12:30"
    }))?;
    let bot = teloxide::Bot::with_client(
        "test-token",
        teloxide::net::default_reqwest_settings()
            .no_proxy()
            .build()?,
    );
    let connection = config.db.acquire().await?;
    let job = {
        let config = config.clone();
        let dialogue = dialogue.clone();
        tokio::spawn(async move {
            crate::bot::schedule_drafts::handle_time(bot, msg, config, dialogue, (id, revision))
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !config.view_updates.contains_key(&1) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let latest = config.begin_view_update(1);
    dialogue.update(State::PersonalQuietHours).await?;
    drop(connection);
    tokio::time::timeout(std::time::Duration::from_secs(2), job).await???;
    assert!(matches!(
        dialogue.get().await?,
        Some(State::PersonalQuietHours)
    ));
    assert!(!latest.is_cancelled());
    assert!(Arc::ptr_eq(&config.view_updates.get(&1).unwrap(), &latest));
    let draft = db::ui_drafts::load(id, 1, db::schedule_drafts::KIND, &config.db)
        .await?
        .unwrap();
    let data: db::schedule_drafts::ScheduleDraft = serde_json::from_str(&draft.payload)?;
    assert_eq!(data.time_minute, Some(750));
    assert!(db::action_groups::list_action_schedules(target, &config.db)
        .await?
        .is_empty());
    assert!(ha.calls.lock().unwrap().is_empty());
    config.finish_view_update(1, &latest);
    Ok(())
}
