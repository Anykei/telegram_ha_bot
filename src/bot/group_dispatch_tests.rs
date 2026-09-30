use super::*;
use crate::core::action_groups::{execute_action_group, ActionActor};
use crate::db::action_groups::{ActionGroupCommand, ActionScheduleCommand, ActionTargetRef};

#[derive(Clone, Copy, Debug)]
enum PolicyChange {
    Disable,
    Delete,
    Restrict,
}

async fn group_fixture() -> anyhow::Result<(Arc<AppConfig>, Arc<FakeHa>, i64)> {
    let (config, ha) = setup().await?;
    second_light(&config, &ha).await?;
    let group = confirmation_test_group(&config).await?;
    db::action_groups::toggle_action_group_item(group, 4, &config.db).await?;
    db::add_user(config.root_user, &config.db).await?;
    Ok((config, ha, group))
}

async fn change_policy(config: &AppConfig, group: i64, change: PolicyChange) -> anyhow::Result<()> {
    match change {
        PolicyChange::Disable => {
            sqlx::query("UPDATE action_groups SET enabled = 0 WHERE id = ?")
                .bind(group)
                .execute(&config.db)
                .await?;
        }
        PolicyChange::Delete => db::action_groups::delete_action_group(group, &config.db).await?,
        PolicyChange::Restrict => {
            sqlx::query("UPDATE action_groups SET access_scope = 'admin_only' WHERE id = ?")
                .bind(group)
                .execute(&config.db)
                .await?;
        }
    }
    Ok(())
}

async fn barrier(notification: &tokio::sync::Notify) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(2), notification.notified()).await?;
    Ok(())
}

#[tokio::test]
async fn group_button_stops_after_policy_revocation_and_preserves_partial_receipt(
) -> anyhow::Result<()> {
    for change in [
        PolicyChange::Disable,
        PolicyChange::Delete,
        PolicyChange::Restrict,
    ] {
        let (config, ha, group) = group_fixture().await?;
        let view = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
        let button = generated_group_buttons(&view).remove(0);
        let Payload::Control(ControlPayload::ConfirmAction { id }) = button.clone() else {
            panic!();
        };
        ha.pause_next_write.store(true, Ordering::SeqCst);
        let request = tokio::spawn(router(button.clone(), 42, config.clone()));
        barrier(&ha.write_started).await?;
        change_policy(&config, group, change).await?;
        ha.release_write.notify_one();
        request.await??;
        let record = db::ui_confirmations::get(id, 42, &config.db)
            .await?
            .unwrap();
        let result: ActionResult = serde_json::from_str(&record.result_json.unwrap())?;
        assert_eq!(result.outcome, Outcome::Partial, "{change:?}");
        assert_eq!(result.targets.len(), 2);
        assert_eq!(result.targets[0].outcome, Outcome::Sent);
        assert_eq!(result.targets[1].outcome, Outcome::Failed);
        assert!(result.targets[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("Группа"));
        router(button, 42, config.clone()).await?;
        assert_eq!(ha.calls.lock().unwrap().len(), 1, "{change:?}");
    }
    Ok(())
}

#[tokio::test]
async fn group_toggle_read_rechecks_policy_for_user_root_and_schedule() -> anyhow::Result<()> {
    for actor in [
        ActionActor::User(42),
        ActionActor::User(1),
        ActionActor::Schedule { schedule_id: 1 },
    ] {
        for change in [
            PolicyChange::Disable,
            PolicyChange::Delete,
            PolicyChange::Restrict,
        ] {
            let (config, ha, group) = group_fixture().await?;
            ha.pause_next_read.store(true, Ordering::SeqCst);
            let request = tokio::spawn(execute_action_group(
                group,
                ActionGroupCommand::Toggle,
                actor,
                config.clone(),
            ));
            barrier(&ha.read_started).await?;
            change_policy(&config, group, change).await?;
            ha.release_read.notify_one();
            let result = request.await??;
            let permitted =
                matches!(change, PolicyChange::Restrict) && !matches!(actor, ActionActor::User(42));
            assert_eq!(
                result.result.outcome,
                if permitted {
                    Outcome::Sent
                } else {
                    Outcome::Failed
                },
                "{actor:?} {change:?}"
            );
            assert_eq!(
                ha.calls.lock().unwrap().len(),
                if permitted { 2 } else { 0 },
                "{actor:?} {change:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn group_between_calls_rechecks_pause_delete_and_preserves_admin_scope_policy(
) -> anyhow::Result<()> {
    for actor in [
        ActionActor::User(1),
        ActionActor::Schedule { schedule_id: 1 },
    ] {
        for change in [
            PolicyChange::Disable,
            PolicyChange::Delete,
            PolicyChange::Restrict,
        ] {
            let (config, ha, group) = group_fixture().await?;
            ha.pause_next_write.store(true, Ordering::SeqCst);
            let request = tokio::spawn(execute_action_group(
                group,
                ActionGroupCommand::TurnOff,
                actor,
                config.clone(),
            ));
            barrier(&ha.write_started).await?;
            change_policy(&config, group, change).await?;
            ha.release_write.notify_one();
            let result = request.await??;
            let permitted = matches!(change, PolicyChange::Restrict);
            assert_eq!(
                result.result.outcome,
                if permitted {
                    Outcome::Sent
                } else {
                    Outcome::Partial
                },
                "{actor:?} {change:?}"
            );
            assert_eq!(
                ha.calls.lock().unwrap().len(),
                if permitted { 2 } else { 1 }
            );
            assert_eq!(result.result.targets[0].outcome, Outcome::Sent);
        }
    }
    Ok(())
}

#[tokio::test]
async fn allowed_group_button_sends_each_member_once_and_never_replays() -> anyhow::Result<()> {
    let (config, ha, group) = group_fixture().await?;
    let view = open(&config, ControlPayload::ActionGroupDetail { group }).await?;
    let button = generated_group_buttons(&view).remove(0);
    router(button.clone(), 42, config.clone()).await?;
    router(button, 42, config.clone()).await?;
    let calls = ha.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].1, calls[1].1);
    Ok(())
}

#[tokio::test]
async fn persisted_schedule_group_stops_on_pause_delete_and_retains_its_scope_policy(
) -> anyhow::Result<()> {
    use crate::core::action_schedules::{execute_reserved_at, plan_at};
    use crate::db::action_groups::runs;
    use chrono::TimeZone;
    for change in [
        PolicyChange::Disable,
        PolicyChange::Delete,
        PolicyChange::Restrict,
    ] {
        let (config, ha, group) = group_fixture().await?;
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
        let request = {
            let config = config.clone();
            let run = run.clone();
            tokio::spawn(async move { execute_reserved_at(&config, &run, now.timestamp()).await })
        };
        barrier(&ha.write_started).await?;
        change_policy(&config, group, change).await?;
        ha.release_write.notify_one();
        request.await??;
        execute_reserved_at(&config, &run, now.timestamp()).await?;
        let saved: runs::ScheduleRun =
            sqlx::query_as("SELECT * FROM action_schedule_runs WHERE id = ?")
                .bind(run.id)
                .fetch_one(&config.db)
                .await?;
        let permitted = matches!(change, PolicyChange::Restrict);
        assert_eq!(
            saved.status,
            if permitted { "sent" } else { "partial" },
            "{change:?}"
        );
        let results: Vec<crate::core::action_results::TargetResult> =
            serde_json::from_str(&saved.result_json)?;
        assert_eq!(results[0].outcome, Outcome::Sent);
        assert_eq!(
            results[1].outcome,
            if permitted {
                Outcome::Sent
            } else {
                Outcome::Failed
            }
        );
        assert_eq!(
            ha.calls.lock().unwrap().len(),
            if permitted { 2 } else { 1 }
        );
    }
    Ok(())
}
