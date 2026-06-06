use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionLogic {
    All,
    Any,
}

impl ConditionLogic {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Any => "any",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "all" => Self::All,
            _ => Self::Any,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionOperator {
    ChangedTo,
    ChangedFromTo,
    Is,
    IsNot,
    Contains,
    Above,
    Below,
}

impl ConditionOperator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChangedTo => "changed_to",
            Self::ChangedFromTo => "changed_from_to",
            Self::Is => "is",
            Self::IsNot => "is_not",
            Self::Contains => "contains",
            Self::Above => "above",
            Self::Below => "below",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "changed_from_to" => Self::ChangedFromTo,
            "is" => Self::Is,
            "is_not" => Self::IsNot,
            "contains" => Self::Contains,
            "above" => Self::Above,
            "below" => Self::Below,
            _ => Self::ChangedTo,
        }
    }

    pub fn is_event_operator(self) -> bool {
        matches!(self, Self::ChangedTo | Self::ChangedFromTo)
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingRule {
    pub id: i64,
    pub name: String,
    pub camera_id: i64,
    pub condition_logic: String,
    pub tail_seconds: i64,
    pub max_segment_seconds: i64,
    pub cooldown_s: i64,
    pub retention_days: i64,
    pub enabled: i64,
    pub notify_enabled: i64,
    pub noise_enabled: i64,
    pub noise_summary_sent_at: Option<DateTime<Utc>>,
    pub last_completed_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl RecordingRule {
    pub fn logic(&self) -> ConditionLogic {
        ConditionLogic::from_str(&self.condition_logic)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled != 0 && self.deleted_at.is_none()
    }

    pub fn notifications_enabled(&self) -> bool {
        self.notify_enabled != 0
    }

    pub fn noise_enabled(&self) -> bool {
        self.noise_enabled != 0
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingRuleCondition {
    pub id: i64,
    pub rule_id: i64,
    pub entity_id: String,
    pub operator: String,
    pub from_state: Option<String>,
    pub to_state: Option<String>,
    pub value: Option<String>,
}

impl RecordingRuleCondition {
    pub fn operator(&self) -> ConditionOperator {
        ConditionOperator::from_str(&self.operator)
    }
}

pub struct NewRecordingRule<'a> {
    pub name: &'a str,
    pub camera_id: i64,
    pub condition_logic: ConditionLogic,
    pub tail_seconds: i64,
    pub max_segment_seconds: i64,
    pub cooldown_s: i64,
    pub retention_days: i64,
}

pub struct NewRecordingCondition<'a> {
    pub rule_id: i64,
    pub entity_id: &'a str,
    pub operator: ConditionOperator,
    pub from_state: Option<&'a str>,
    pub to_state: Option<&'a str>,
    pub value: Option<&'a str>,
}

pub struct NewRecordingConditionDraft<'a> {
    pub entity_id: &'a str,
    pub operator: ConditionOperator,
    pub from_state: Option<&'a str>,
    pub to_state: Option<&'a str>,
    pub value: Option<&'a str>,
}

pub async fn create_rule(rule: NewRecordingRule<'_>, pool: &SqlitePool) -> Result<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO camera_recording_rules (
            name, camera_id, condition_logic, tail_seconds, max_segment_seconds,
            cooldown_s, retention_days, enabled, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, 1, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(rule.name.trim())
    .bind(rule.camera_id)
    .bind(rule.condition_logic.as_str())
    .bind(rule.tail_seconds)
    .bind(rule.max_segment_seconds)
    .bind(rule.cooldown_s)
    .bind(rule.retention_days)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

#[allow(dead_code)]
pub async fn create_rule_with_conditions_and_group(
    rule: NewRecordingRule<'_>,
    conditions: &[NewRecordingConditionDraft<'_>],
    group_id: Option<i64>,
    pool: &SqlitePool,
) -> Result<i64> {
    let group_ids = group_id.into_iter().collect::<Vec<_>>();
    create_rule_with_conditions_and_groups(rule, conditions, &group_ids, pool).await
}

pub async fn create_rule_with_conditions_and_groups(
    rule: NewRecordingRule<'_>,
    conditions: &[NewRecordingConditionDraft<'_>],
    group_ids: &[i64],
    pool: &SqlitePool,
) -> Result<i64> {
    if conditions.is_empty() {
        return Err(anyhow!("Recording rule must have at least one condition"));
    }

    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"
        INSERT INTO camera_recording_rules (
            name, camera_id, condition_logic, tail_seconds, max_segment_seconds,
            cooldown_s, retention_days, enabled, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, 1, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(rule.name.trim())
    .bind(rule.camera_id)
    .bind(rule.condition_logic.as_str())
    .bind(rule.tail_seconds)
    .bind(rule.max_segment_seconds)
    .bind(rule.cooldown_s)
    .bind(rule.retention_days)
    .execute(&mut *tx)
    .await?;
    let rule_id = result.last_insert_rowid();

    for condition in conditions {
        sqlx::query(
            r#"
            INSERT INTO camera_recording_rule_conditions (
                rule_id, entity_id, operator, from_state, to_state, value
            )
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(rule_id)
        .bind(condition.entity_id.trim())
        .bind(condition.operator.as_str())
        .bind(condition.from_state.map(str::trim))
        .bind(condition.to_state.map(str::trim))
        .bind(condition.value.map(str::trim))
        .execute(&mut *tx)
        .await?;
    }

    for group_id in group_ids {
        let result = sqlx::query(
            r#"
            INSERT INTO camera_recording_rule_group_items (rule_id, group_id)
            SELECT ?, ?
            WHERE EXISTS (
                SELECT 1 FROM camera_recording_rule_groups WHERE id = ?
            )
            "#,
        )
        .bind(rule_id)
        .bind(group_id)
        .bind(group_id)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(anyhow!("Recording rule group not found"));
        }
    }

    tx.commit().await?;
    Ok(rule_id)
}

pub async fn add_condition(condition: NewRecordingCondition<'_>, pool: &SqlitePool) -> Result<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO camera_recording_rule_conditions (
            rule_id, entity_id, operator, from_state, to_state, value
        )
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(condition.rule_id)
    .bind(condition.entity_id.trim())
    .bind(condition.operator.as_str())
    .bind(condition.from_state.map(str::trim))
    .bind(condition.to_state.map(str::trim))
    .bind(condition.value.map(str::trim))
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn list_rules(pool: &SqlitePool) -> Result<Vec<RecordingRule>> {
    Ok(sqlx::query_as::<_, RecordingRule>(
        r#"
        SELECT id, name, camera_id, condition_logic, tail_seconds, max_segment_seconds,
               cooldown_s, retention_days, enabled, notify_enabled, noise_enabled,
               noise_summary_sent_at, last_completed_at, deleted_at
        FROM camera_recording_rules
        WHERE deleted_at IS NULL
        ORDER BY name
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_rule(rule_id: i64, pool: &SqlitePool) -> Result<Option<RecordingRule>> {
    Ok(sqlx::query_as::<_, RecordingRule>(
        r#"
        SELECT id, name, camera_id, condition_logic, tail_seconds, max_segment_seconds,
               cooldown_s, retention_days, enabled, notify_enabled, noise_enabled,
               noise_summary_sent_at, last_completed_at, deleted_at
        FROM camera_recording_rules
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(rule_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn get_rule_for_room(
    rule_id: i64,
    room_id: i64,
    pool: &SqlitePool,
) -> Result<Option<RecordingRule>> {
    Ok(sqlx::query_as::<_, RecordingRule>(
        r#"
        SELECT r.id, r.name, r.camera_id, r.condition_logic, r.tail_seconds, r.max_segment_seconds,
               r.cooldown_s, r.retention_days, r.enabled, r.notify_enabled, r.noise_enabled,
               r.noise_summary_sent_at, r.last_completed_at, r.deleted_at
        FROM camera_recording_rules r
        JOIN cameras c ON c.id = r.camera_id
        WHERE r.id = ?
          AND r.deleted_at IS NULL
          AND c.room_id = ?
          AND c.enabled != 0
        "#,
    )
    .bind(rule_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_conditions(
    rule_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingRuleCondition>> {
    Ok(sqlx::query_as::<_, RecordingRuleCondition>(
        r#"
        SELECT id, rule_id, entity_id, operator, from_state, to_state, value
        FROM camera_recording_rule_conditions
        WHERE rule_id = ?
        ORDER BY id
        "#,
    )
    .bind(rule_id)
    .fetch_all(pool)
    .await?)
}

pub async fn delete_condition(rule_id: i64, condition_id: i64, pool: &SqlitePool) -> Result<()> {
    let condition_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM camera_recording_rule_conditions WHERE rule_id = ?",
    )
    .bind(rule_id)
    .fetch_one(pool)
    .await?;

    if condition_count <= 1 {
        return Err(anyhow!("Recording rule must keep at least one condition"));
    }

    let result =
        sqlx::query("DELETE FROM camera_recording_rule_conditions WHERE id = ? AND rule_id = ?")
            .bind(condition_id)
            .bind(rule_id)
            .execute(pool)
            .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule condition not found"));
    }

    Ok(())
}

pub async fn find_candidate_rules_by_entity(
    entity_id: &str,
    pool: &SqlitePool,
) -> Result<Vec<(RecordingRule, Vec<RecordingRuleCondition>)>> {
    let rules = sqlx::query_as::<_, RecordingRule>(
        r#"
        SELECT DISTINCT r.id, r.name, r.camera_id, r.condition_logic, r.tail_seconds,
               r.max_segment_seconds, r.cooldown_s, r.retention_days, r.enabled, r.notify_enabled,
               r.noise_enabled, r.noise_summary_sent_at, r.last_completed_at, r.deleted_at
        FROM camera_recording_rules r
        JOIN camera_recording_rule_conditions c ON c.rule_id = r.id
        WHERE r.enabled != 0
          AND r.deleted_at IS NULL
          AND c.entity_id = ?
        ORDER BY r.id
        "#,
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for rule in rules {
        let conditions = list_conditions(rule.id, pool).await?;
        result.push((rule, conditions));
    }

    Ok(result)
}

pub async fn toggle_rule_enabled(rule_id: i64, pool: &SqlitePool) -> Result<bool> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET enabled = CASE WHEN enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(rule_id)
    .execute(pool)
    .await?;

    let enabled: Option<i64> = sqlx::query_scalar(
        "SELECT enabled FROM camera_recording_rules WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(rule_id)
    .fetch_optional(pool)
    .await?;

    enabled
        .map(|value| value != 0)
        .ok_or_else(|| anyhow!("Recording rule not found"))
}

pub async fn toggle_rule_notifications(rule_id: i64, pool: &SqlitePool) -> Result<bool> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET notify_enabled = CASE WHEN notify_enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(rule_id)
    .execute(pool)
    .await?;

    let enabled: Option<i64> = sqlx::query_scalar(
        "SELECT notify_enabled FROM camera_recording_rules WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(rule_id)
    .fetch_optional(pool)
    .await?;

    enabled
        .map(|value| value != 0)
        .ok_or_else(|| anyhow!("Recording rule not found"))
}

pub async fn toggle_rule_noise(rule_id: i64, pool: &SqlitePool) -> Result<bool> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET noise_enabled = CASE WHEN noise_enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(rule_id)
    .execute(pool)
    .await?;

    let enabled: Option<i64> = sqlx::query_scalar(
        "SELECT noise_enabled FROM camera_recording_rules WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(rule_id)
    .fetch_optional(pool)
    .await?;

    enabled
        .map(|value| value != 0)
        .ok_or_else(|| anyhow!("Recording rule not found"))
}

pub async fn update_rule_logic(
    rule_id: i64,
    logic: ConditionLogic,
    pool: &SqlitePool,
) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET condition_logic = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(logic.as_str())
    .bind(rule_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule not found"));
    }

    Ok(())
}

pub async fn update_rule_recording_options(
    rule_id: i64,
    tail_seconds: Option<i64>,
    max_segment_seconds: Option<i64>,
    cooldown_s: Option<i64>,
    retention_days: Option<i64>,
    pool: &SqlitePool,
) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET tail_seconds = COALESCE(?, tail_seconds),
            max_segment_seconds = COALESCE(?, max_segment_seconds),
            cooldown_s = COALESCE(?, cooldown_s),
            retention_days = COALESCE(?, retention_days),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(tail_seconds)
    .bind(max_segment_seconds)
    .bind(cooldown_s)
    .bind(retention_days)
    .bind(rule_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule not found"));
    }

    Ok(())
}

pub async fn mark_noise_summary_sent(
    rule_id: i64,
    at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET noise_summary_sent_at = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(at)
    .bind(rule_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn update_rule_replace_conditions(
    rule_id: i64,
    rule: NewRecordingRule<'_>,
    conditions: &[NewRecordingCondition<'_>],
    pool: &SqlitePool,
) -> Result<()> {
    let mut tx = pool.begin().await?;

    let result = sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET name = ?,
            camera_id = ?,
            condition_logic = ?,
            tail_seconds = ?,
            max_segment_seconds = ?,
            cooldown_s = ?,
            retention_days = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(rule.name.trim())
    .bind(rule.camera_id)
    .bind(rule.condition_logic.as_str())
    .bind(rule.tail_seconds)
    .bind(rule.max_segment_seconds)
    .bind(rule.cooldown_s)
    .bind(rule.retention_days)
    .bind(rule_id)
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule not found"));
    }

    sqlx::query("DELETE FROM camera_recording_rule_conditions WHERE rule_id = ?")
        .bind(rule_id)
        .execute(&mut *tx)
        .await?;

    for condition in conditions {
        sqlx::query(
            r#"
            INSERT INTO camera_recording_rule_conditions (
                rule_id, entity_id, operator, from_state, to_state, value
            )
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(rule_id)
        .bind(condition.entity_id.trim())
        .bind(condition.operator.as_str())
        .bind(condition.from_state.map(str::trim))
        .bind(condition.to_state.map(str::trim))
        .bind(condition.value.map(str::trim))
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn duplicate_rule(rule_id: i64, pool: &SqlitePool) -> Result<i64> {
    let rule = get_rule(rule_id, pool)
        .await?
        .ok_or_else(|| anyhow!("Recording rule not found"))?;
    let conditions = list_conditions(rule_id, pool).await?;
    let new_name = format!("{} copy", rule.name);

    let new_rule_id = create_rule(
        NewRecordingRule {
            name: &new_name,
            camera_id: rule.camera_id,
            condition_logic: rule.logic(),
            tail_seconds: rule.tail_seconds,
            max_segment_seconds: rule.max_segment_seconds,
            cooldown_s: rule.cooldown_s,
            retention_days: rule.retention_days,
        },
        pool,
    )
    .await?;

    if !rule.notifications_enabled() {
        toggle_rule_notifications(new_rule_id, pool).await?;
    }

    for condition in conditions {
        add_condition(
            NewRecordingCondition {
                rule_id: new_rule_id,
                entity_id: &condition.entity_id,
                operator: condition.operator(),
                from_state: condition.from_state.as_deref(),
                to_state: condition.to_state.as_deref(),
                value: condition.value.as_deref(),
            },
            pool,
        )
        .await?;
    }

    Ok(new_rule_id)
}

pub async fn soft_delete_rule(rule_id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET deleted_at = ?, enabled = 0, updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(Utc::now())
    .bind(rule_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_rule_completed(
    rule_id: i64,
    completed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET last_completed_at = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(completed_at)
    .bind(rule_id)
    .execute(pool)
    .await?;

    Ok(())
}
