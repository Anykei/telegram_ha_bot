use super::action_groups::{ActionScheduleCommand, ActionTargetRef};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
pub const KIND: &str = "action_schedule";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleDraft {
    pub target: ActionTargetRef,
    pub command: ActionScheduleCommand,
    pub time_minute: Option<i64>,
    pub days_mask: i64,
    pub enabled: bool,
    pub existing: Option<(i64, i64)>,
}
impl ScheduleDraft {
    pub async fn from_existing(
        id: i64,
        target: ActionTargetRef,
        pool: &SqlitePool,
    ) -> Result<Self> {
        let mut tx = pool.begin().await?;
        let row: Option<(String,i64,String,i64,i64,bool,i64)> = sqlx::query_as("SELECT target_type,target_id,command,time_minute,days_mask,enabled,revision FROM action_schedules WHERE id=?").bind(id).fetch_optional(&mut *tx).await?;
        let (kind, target_id, command, time_minute, days_mask, enabled, revision) =
            row.context("Schedule missing")?;
        ensure!(
            target.target_type() == kind && target.target_id() == target_id,
            "Schedule target changed"
        );
        tx.commit().await?;
        Ok(Self {
            target,
            command: ActionScheduleCommand::from_db(&command)?,
            time_minute: Some(time_minute),
            days_mask,
            enabled,
            existing: Some((id, revision)),
        })
    }
}

pub async fn latest_for_target(
    owner: u64,
    target: ActionTargetRef,
    pool: &SqlitePool,
) -> Result<Option<super::ui_drafts::UiDraft>> {
    let target_json = serde_json::to_string(&target)?;
    Ok(sqlx::query_as("SELECT * FROM ui_drafts WHERE owner_id=? AND kind=? AND status IN ('editing','review') AND expires_at>? AND json_extract(payload,'$.target')=? ORDER BY updated_at DESC,id DESC LIMIT 1")
        .bind(owner as i64).bind(KIND).bind(chrono::Utc::now().timestamp()).bind(target_json).fetch_optional(pool).await?)
}

pub async fn commit(
    id: i64,
    owner: u64,
    revision: i64,
    enabled: bool,
    pool: &SqlitePool,
) -> Result<i64> {
    let draft = super::ui_drafts::load(id, owner, KIND, pool)
        .await?
        .context("Draft expired")?;
    if let Some(saved) = draft.committed_target_id {
        return Ok(saved);
    }
    ensure!(
        draft.revision == revision,
        "Draft changed; reopen its current version"
    );
    let data: ScheduleDraft = serde_json::from_str(&draft.payload)?;
    let minute = data.time_minute.context("Enter time first")?;
    ensure!(
        (0..1440).contains(&minute) && (1..=127).contains(&data.days_mask),
        "Choose a valid time and at least one day"
    );
    ensure!(
        matches!(
            (data.target, data.command),
            (
                ActionTargetRef::BotGroup(_),
                ActionScheduleCommand::TurnOn | ActionScheduleCommand::TurnOff
            ) | (ActionTargetRef::HaNative(_), ActionScheduleCommand::Execute)
        ),
        "Command does not match target"
    );
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query("UPDATE ui_drafts SET updated_at=updated_at WHERE id=? AND owner_id=? AND kind=? AND revision=? AND expires_at>? AND status IN ('editing','review')")
        .bind(id).bind(owner as i64).bind(KIND).bind(revision).bind(now).execute(&mut *tx).await?;
    if claimed.rows_affected() != 1 {
        let saved: Option<i64> = sqlx::query_scalar("SELECT committed_target_id FROM ui_drafts WHERE id=? AND owner_id=? AND status='saved'").bind(id).bind(owner as i64).fetch_optional(&mut *tx).await?.flatten();
        if let Some(saved) = saved {
            return Ok(saved);
        }
        anyhow::bail!("Draft changed or expired; reopen its current version");
    }
    let valid: bool = match data.target {
        ActionTargetRef::BotGroup(target) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM action_groups WHERE id=?)").bind(target).fetch_one(&mut *tx).await?,
        ActionTargetRef::HaNative(target) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ha_native_targets WHERE id=? AND archived=0 AND domain IN ('script','scene'))").bind(target).fetch_one(&mut *tx).await?,
    };
    ensure!(valid, "Target unavailable; draft retained");
    let saved = if let Some((existing, expected)) = data.existing {
        let changed = sqlx::query("UPDATE action_schedules SET command=?,time_minute=?,days_mask=?,enabled=?,updated_at=CURRENT_TIMESTAMP WHERE id=? AND revision=? AND target_type=? AND target_id=?")
            .bind(data.command.as_str()).bind(minute).bind(data.days_mask).bind(enabled).bind(existing).bind(expected).bind(data.target.target_type()).bind(data.target.target_id()).execute(&mut *tx).await?;
        ensure!(changed.rows_affected()==1, "Schedule changed concurrently. Active version preserved; reopen and compare your draft.");
        existing
    } else {
        sqlx::query("INSERT INTO action_schedules(target_type,target_id,command,time_minute,days_mask,enabled) VALUES(?,?,?,?,?,?)")
            .bind(data.target.target_type()).bind(data.target.target_id()).bind(data.command.as_str()).bind(minute).bind(data.days_mask).bind(enabled).execute(&mut *tx).await?.last_insert_rowid()
    };
    sqlx::query("UPDATE ui_drafts SET status='saved',committed_target_id=?,revision=revision+1,updated_at=? WHERE id=?").bind(saved).bind(now).bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO activity_log(user_id,kind,entity_type,entity_id,action,status,message) VALUES(?,'action_group','action_schedule',?,'schedule.commit','ok',?)")
        .bind(owner as i64).bind(saved.to_string()).bind(format!("draft={id};enabled={enabled}")).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn draft_is_not_due_commit_is_idempotent_and_edit_conflict_preserves_last_run(
    ) -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let group = super::super::action_groups::create_action_group("Lights", &pool).await?;
        let data = ScheduleDraft {
            target: ActionTargetRef::BotGroup(group),
            command: ActionScheduleCommand::TurnOn,
            time_minute: Some(720),
            days_mask: 127,
            enabled: false,
            existing: None,
        };
        let draft =
            super::super::ui_drafts::create(1, KIND, &serde_json::to_string(&data)?, 24, &pool)
                .await?;
        assert!(super::super::action_groups::list_due_action_schedules(
            720,
            1,
            "2026-09-29 12:00",
            &pool
        )
        .await?
        .is_empty());
        let saved = commit(draft.id, 1, 0, false, &pool).await?;
        assert_eq!(commit(draft.id, 1, 0, true, &pool).await?, saved);
        assert!(
            !super::super::action_groups::get_action_schedule(saved, &pool)
                .await?
                .unwrap()
                .is_enabled()
        );
        super::super::action_groups::mark_action_schedule_run(saved, "previous", &pool).await?;
        let edit = ScheduleDraft::from_existing(saved, data.target, &pool).await?;
        let draft =
            super::super::ui_drafts::create(1, KIND, &serde_json::to_string(&edit)?, 24, &pool)
                .await?;
        super::super::action_groups::update_action_schedule_days(saved, 1, &pool).await?;
        assert!(commit(draft.id, 1, 0, true, &pool).await.is_err());
        let current = super::super::action_groups::get_action_schedule(saved, &pool)
            .await?
            .unwrap();
        assert_eq!(current.days_mask, 1);
        assert_eq!(current.last_run_at.as_deref(), Some("previous"));
        assert_eq!(
            super::super::ui_drafts::load(draft.id, 1, KIND, &pool)
                .await?
                .unwrap()
                .status,
            "editing"
        );
        assert!(commit(draft.id, 2, 0, true, &pool).await.is_err());
        Ok(())
    }
}
