use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingRuleGroup {
    pub id: i64,
    pub name: String,
    pub enabled: i64,
    pub rules_count: i64,
    pub created_at: DateTime<Utc>,
}

impl RecordingRuleGroup {
    pub fn is_enabled(&self) -> bool {
        self.enabled != 0
    }
}

pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<RecordingRuleGroup>> {
    Ok(sqlx::query_as::<_, RecordingRuleGroup>(
        r#"
        SELECT g.id, g.name, g.enabled, g.created_at, COUNT(r.id) AS rules_count
        FROM camera_recording_rule_groups g
        LEFT JOIN camera_recording_rule_group_items i ON i.group_id = g.id
        LEFT JOIN camera_recording_rules r ON r.id = i.rule_id AND r.deleted_at IS NULL
        GROUP BY g.id, g.name, g.enabled, g.created_at
        ORDER BY g.name
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn create_group(name: &str, pool: &SqlitePool) -> Result<i64> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Group name is empty"));
    }

    sqlx::query(
        r#"
        INSERT INTO camera_recording_rule_groups (name, enabled, updated_at)
        VALUES (?, 1, CURRENT_TIMESTAMP)
        ON CONFLICT(name) DO UPDATE SET updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(trimmed)
    .execute(pool)
    .await?;

    Ok(
        sqlx::query_scalar("SELECT id FROM camera_recording_rule_groups WHERE name = ?")
            .bind(trimmed)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn ensure_default_groups(pool: &SqlitePool) -> Result<()> {
    for name in ["Охрана", "Тест", "Ночь", "Двери"] {
        create_group(name, pool).await?;
    }
    Ok(())
}

pub async fn toggle_group_enabled(group_id: i64, pool: &SqlitePool) -> Result<bool> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rule_groups
        SET enabled = CASE WHEN enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(group_id)
    .execute(pool)
    .await?;

    let enabled: Option<i64> =
        sqlx::query_scalar("SELECT enabled FROM camera_recording_rule_groups WHERE id = ?")
            .bind(group_id)
            .fetch_optional(pool)
            .await?;
    enabled
        .map(|value| value != 0)
        .ok_or_else(|| anyhow!("Recording rule group not found"))
}

pub async fn toggle_rule_in_group(rule_id: i64, group_id: i64, pool: &SqlitePool) -> Result<bool> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM camera_recording_rule_group_items WHERE rule_id = ? AND group_id = ?",
    )
    .bind(rule_id)
    .bind(group_id)
    .fetch_optional(pool)
    .await?;

    if exists.is_some() {
        sqlx::query(
            "DELETE FROM camera_recording_rule_group_items WHERE rule_id = ? AND group_id = ?",
        )
        .bind(rule_id)
        .bind(group_id)
        .execute(pool)
        .await?;
        Ok(false)
    } else {
        sqlx::query(
            "INSERT INTO camera_recording_rule_group_items (rule_id, group_id) VALUES (?, ?)",
        )
        .bind(rule_id)
        .bind(group_id)
        .execute(pool)
        .await?;
        Ok(true)
    }
}

pub async fn list_rule_group_ids(rule_id: i64, pool: &SqlitePool) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT group_id
        FROM camera_recording_rule_group_items
        WHERE rule_id = ?
        ORDER BY group_id
        "#,
    )
    .bind(rule_id)
    .fetch_all(pool)
    .await?)
}

pub async fn rule_groups_enabled(rule_id: i64, pool: &SqlitePool) -> Result<bool> {
    let disabled_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM camera_recording_rule_group_items i
        JOIN camera_recording_rule_groups g ON g.id = i.group_id
        WHERE i.rule_id = ? AND g.enabled = 0
        "#,
    )
    .bind(rule_id)
    .fetch_one(pool)
    .await?;

    Ok(disabled_count == 0)
}
