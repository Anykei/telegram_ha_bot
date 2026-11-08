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

async fn native_schedule(config: &Arc<AppConfig>, name: &str, minute: i64) -> anyhow::Result<i64> {
    let id = sqlx::query(
        "INSERT INTO ha_native_targets (entity_id, domain, ha_name) VALUES (?, 'script', ?)",
    )
    .bind(format!("script.{name}"))
    .bind(name)
    .execute(&config.db)
    .await?
    .last_insert_rowid();
    db::action_groups::create_action_schedule(
        ActionTargetRef::HaNative(id),
        ActionScheduleCommand::Execute,
        minute,
        127,
        &config.db,
    )
    .await
}

#[tokio::test]
async fn slow_schedule_does_not_block_other_minute_or_replay_after_crash() -> anyhow::Result<()> {
    use crate::core::action_schedules::{execute_reserved_at, plan_at};
    use crate::db::action_groups::runs;
    use chrono::TimeZone;
    let (config, ha) = setup().await?;
    native_schedule(&config, "a", 720).await?;
    let b = native_schedule(&config, "b", 721).await?;
    let first = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 50).unwrap();
    plan_at(&config, first).await?;
    let run_a = runs::pending(&config.db).await?.remove(0);
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let worker = {
        let config = config.clone();
        let run = run_a.clone();
        tokio::spawn(async move { execute_reserved_at(&config, &run, first.timestamp()).await })
    };
    ha.write_started.notified().await;
    // A is held by the fake HA until 12:02; B's 12:01 reservation and dispatch
    // complete while that barrier remains held. No wall-clock minute is needed.
    let second = first + Duration::seconds(11);
    plan_at(&config, second).await?;
    let run_b = runs::pending(&config.db)
        .await?
        .into_iter()
        .find(|run| run.schedule_id == b)
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        execute_reserved_at(&config, &run_b, second.timestamp()),
    )
    .await??;
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    let b_status: String =
        sqlx::query_scalar("SELECT status FROM action_schedule_runs WHERE id = ?")
            .bind(run_b.id)
            .fetch_one(&config.db)
            .await?;
    assert_eq!(b_status, "sent");
    // Simulate process death after HA accepted A but before the result commit.
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert_eq!(runs::recover_started(&config.db).await?, 1);
    execute_reserved_at(&config, &run_a, second.timestamp()).await?;
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    let a_status: String =
        sqlx::query_scalar("SELECT status FROM action_schedule_runs WHERE id = ?")
            .bind(run_a.id)
            .fetch_one(&config.db)
            .await?;
    assert_eq!(a_status, "unknown");
    Ok(())
}

#[tokio::test]
async fn schedule_busy_target_expires_without_retry_and_planning_never_catches_up(
) -> anyhow::Result<()> {
    use crate::core::action_schedules::{execute_reserved_at, plan_at};
    use crate::db::action_groups::runs;
    use chrono::TimeZone;
    let (config, ha) = setup().await?;
    let id = native_schedule(&config, "a", 720).await?;
    let schedule = db::action_groups::get_action_schedule(id, &config.db)
        .await?
        .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 50).unwrap();
    plan_at(&config, now).await?;
    let first = runs::pending(&config.db).await?.remove(0);
    let _guard =
        crate::core::action_groups::try_lock_target(&config, schedule.target_ref()?).unwrap();
    execute_reserved_at(&config, &first, now.timestamp()).await?;
    assert_eq!(runs::pending(&config.db).await?.len(), 1);
    let later = now + Duration::seconds(70);
    plan_at(&config, later).await?;
    assert!(runs::pending(&config.db).await?.is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM action_schedule_runs WHERE id = ?")
            .bind(first.id)
            .fetch_one(&config.db)
            .await?,
        "skipped"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM action_schedule_runs")
        .fetch_one(&config.db)
        .await?;
    assert_eq!(count, 1);
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn schedules_observe_days_midnight_and_dst_local_minute_uniqueness() -> anyhow::Result<()> {
    use crate::core::action_schedules::plan_at;
    use crate::db::action_groups::runs;
    use chrono::{Datelike, TimeZone};
    let (config, _) = setup().await?;
    let id = native_schedule(&config, "daily", 150).await?;
    let zone = chrono_tz::Europe::Berlin;
    let overlap = zone.with_ymd_and_hms(2026, 10, 25, 2, 30, 50);
    let first = overlap.earliest().unwrap();
    let second = overlap.latest().unwrap();
    plan_at(&config, first).await?;
    let run = runs::pending(&config.db).await?.remove(0);
    plan_at(&config, second).await?;
    assert!(runs::pending(&config.db).await?.is_empty());
    let stored: (i64, String) =
        sqlx::query_as("SELECT deadline_at, status FROM action_schedule_runs WHERE id = ?")
            .bind(run.id)
            .fetch_one(&config.db)
            .await?;
    assert_eq!(stored, (first.timestamp() + 10, "skipped".into()));
    let midnight = zone.with_ymd_and_hms(2026, 10, 26, 0, 0, 1).unwrap();
    sqlx::query("UPDATE action_schedules SET time_minute = 0, days_mask = ? WHERE id = ?")
        .bind(1_i64 << midnight.weekday().num_days_from_monday())
        .bind(id)
        .execute(&config.db)
        .await?;
    plan_at(&config, midnight - Duration::seconds(2)).await?;
    assert!(runs::pending(&config.db).await?.is_empty());
    plan_at(&config, midnight).await?;
    assert_eq!(runs::pending(&config.db).await?.len(), 1);
    plan_at(&config, midnight + Duration::days(1)).await?;
    assert!(runs::pending(&config.db).await?.is_empty());
    // The missing spring-forward minute never exists and is not caught up.
    sqlx::query("UPDATE action_schedules SET time_minute = 150, days_mask = 127 WHERE id = ?")
        .bind(id)
        .execute(&config.db)
        .await?;
    plan_at(
        &config,
        zone.with_ymd_and_hms(2027, 3, 28, 3, 0, 0).unwrap(),
    )
    .await?;
    assert!(runs::pending(&config.db).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn partial_schedule_group_records_actual_targets_without_reexecution() -> anyhow::Result<()> {
    use crate::core::action_schedules::{execute_reserved_at, plan_at};
    use crate::db::action_groups::runs;
    use chrono::TimeZone;
    let (config, ha) = setup().await?;
    db::devices::sync_device("light.second", "bedroom", "Second", "light", &config.db).await?;
    db::subscriptions::toggle_hidden("light.second", &config.db).await?;
    let group = db::action_groups::create_action_group("Partial", &config.db).await?;
    db::action_groups::toggle_action_group_item(group, 3, &config.db).await?;
    let second: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = 'light.second'")
        .fetch_one(&config.db)
        .await?;
    db::action_groups::toggle_action_group_item(group, second, &config.db).await?;
    db::action_groups::create_action_schedule(
        ActionTargetRef::BotGroup(group),
        ActionScheduleCommand::TurnOff,
        720,
        127,
        &config.db,
    )
    .await?;
    *ha.fail_entity.lock().unwrap() = Some("light.second".into());
    let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap();
    plan_at(&config, now).await?;
    let run = runs::pending(&config.db).await?.remove(0);
    execute_reserved_at(&config, &run, now.timestamp()).await?;
    execute_reserved_at(&config, &run, now.timestamp()).await?;
    assert_eq!(runs::recover_started(&config.db).await?, 0);
    plan_at(&config, now).await?;
    assert!(runs::pending(&config.db).await?.is_empty());
    let stored: (String, String) =
        sqlx::query_as("SELECT status, result_json FROM action_schedule_runs WHERE id = ?")
            .bind(run.id)
            .fetch_one(&config.db)
            .await?;
    assert_eq!(stored.0, "partial");
    let results: Vec<crate::core::action_results::TargetResult> = serde_json::from_str(&stored.1)?;
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].outcome,
        crate::core::action_results::Outcome::Sent
    );
    assert_eq!(
        results[1].outcome,
        crate::core::action_results::Outcome::Unknown
    );
    assert_eq!(ha.calls.lock().unwrap().len(), 2);
    Ok(())
}

#[tokio::test]
async fn schedule_deadline_is_rechecked_after_claim_before_first_ha_request() -> anyhow::Result<()>
{
    use crate::core::action_schedules::{execute_reserved_with_clock, plan_at};
    use crate::db::action_groups::runs;
    use chrono::TimeZone;
    let (config, ha) = setup().await?;
    native_schedule(&config, "deadline", 720).await?;
    let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 59).unwrap();
    plan_at(&config, now).await?;
    let run = runs::pending(&config.db).await?.remove(0);
    let reads = AtomicUsize::new(0);
    execute_reserved_with_clock(&config, &run, || {
        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
            now.timestamp()
        } else {
            run.deadline_at
        }
    })
    .await?;
    assert!(ha.calls.lock().unwrap().is_empty());
    let status: String = sqlx::query_scalar("SELECT status FROM action_schedule_runs WHERE id = ?")
        .bind(run.id)
        .fetch_one(&config.db)
        .await?;
    assert_eq!(status, "skipped");
    Ok(())
}

#[tokio::test]
async fn two_schedule_executors_share_one_dispatch_right() -> anyhow::Result<()> {
    use crate::core::action_schedules::{execute_reserved_at, plan_at};
    use crate::db::action_groups::runs;
    use chrono::TimeZone;
    let (config, ha) = setup().await?;
    native_schedule(&config, "concurrent", 720).await?;
    let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 20).unwrap();
    plan_at(&config, now).await?;
    let run = runs::pending(&config.db).await?.remove(0);
    let (a, b) = tokio::join!(
        execute_reserved_at(&config, &run, now.timestamp()),
        execute_reserved_at(&config, &run, now.timestamp())
    );
    a?;
    b?;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert!(runs::pending(&config.db).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn shutdown_stops_remaining_schedule_group_requests() -> anyhow::Result<()> {
    use crate::core::action_schedules::{execute_reserved_at, plan_at};
    use crate::db::action_groups::runs;
    use chrono::TimeZone;
    let (config, ha) = setup().await?;
    db::devices::sync_device("light.second", "bedroom", "Second", "light", &config.db).await?;
    db::subscriptions::toggle_hidden("light.second", &config.db).await?;
    let second: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = 'light.second'")
        .fetch_one(&config.db)
        .await?;
    let group = db::action_groups::create_action_group("Shutdown", &config.db).await?;
    db::action_groups::toggle_action_group_item(group, 3, &config.db).await?;
    db::action_groups::toggle_action_group_item(group, second, &config.db).await?;
    db::action_groups::create_action_schedule(
        ActionTargetRef::BotGroup(group),
        ActionScheduleCommand::TurnOff,
        720,
        127,
        &config.db,
    )
    .await?;
    let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap();
    plan_at(&config, now).await?;
    let run = runs::pending(&config.db).await?.remove(0);
    ha.pause_next_write.store(true, Ordering::SeqCst);
    let execution = {
        let config = config.clone();
        let run = run.clone();
        tokio::spawn(async move { execute_reserved_at(&config, &run, now.timestamp()).await })
    };
    ha.write_started.notified().await;
    config.runtime_status.write().await.shutdown_requested_at = Some(Utc::now());
    ha.release_write.notify_one();
    execution.await??;
    assert_eq!(ha.calls.lock().unwrap().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM action_schedule_runs WHERE id = ?")
            .bind(run.id)
            .fetch_one(&config.db)
            .await?,
        "partial"
    );
    Ok(())
}
