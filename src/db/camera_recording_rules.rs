use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, Local, Timelike, Utc};
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

pub const ACTIVE_TIME_ALL_DAYS_MASK: i64 = 0b111_1111;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingRuleActiveTime {
    pub enabled: bool,
    pub from_minute: i64,
    pub to_minute: i64,
    pub days_mask: i64,
}

impl RecordingRuleActiveTime {
    pub fn always() -> Self {
        Self {
            enabled: false,
            from_minute: 0,
            to_minute: 0,
            days_mask: ACTIVE_TIME_ALL_DAYS_MASK,
        }
    }

    pub fn window(from_minute: i64, to_minute: i64, days_mask: i64) -> Result<Self> {
        validate_active_time_window(from_minute, to_minute, days_mask)?;
        Ok(Self {
            enabled: true,
            from_minute,
            to_minute,
            days_mask,
        })
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
    pub active_time_enabled: i64,
    pub active_from_minute: Option<i64>,
    pub active_to_minute: Option<i64>,
    pub active_days_mask: i64,
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

    pub fn active_time(&self) -> RecordingRuleActiveTime {
        if self.active_time_enabled == 0 {
            return RecordingRuleActiveTime::always();
        }

        RecordingRuleActiveTime {
            enabled: true,
            from_minute: self.active_from_minute.unwrap_or(-1),
            to_minute: self.active_to_minute.unwrap_or(-1),
            days_mask: self.active_days_mask,
        }
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
               active_time_enabled, active_from_minute, active_to_minute, active_days_mask,
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
               active_time_enabled, active_from_minute, active_to_minute, active_days_mask,
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
               r.active_time_enabled, r.active_from_minute, r.active_to_minute, r.active_days_mask,
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
               r.noise_enabled, r.active_time_enabled, r.active_from_minute, r.active_to_minute,
               r.active_days_mask, r.noise_summary_sent_at, r.last_completed_at, r.deleted_at
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

pub async fn update_rule_active_time(
    rule_id: i64,
    active_time: RecordingRuleActiveTime,
    pool: &SqlitePool,
) -> Result<()> {
    if !active_time.enabled {
        return reset_rule_active_time(rule_id, pool).await;
    }
    validate_active_time_window(
        active_time.from_minute,
        active_time.to_minute,
        active_time.days_mask,
    )?;

    let result = sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET active_time_enabled = 1,
            active_from_minute = ?,
            active_to_minute = ?,
            active_days_mask = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(active_time.from_minute)
    .bind(active_time.to_minute)
    .bind(active_time.days_mask)
    .bind(rule_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule not found"));
    }

    Ok(())
}

pub async fn reset_rule_active_time(rule_id: i64, pool: &SqlitePool) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET active_time_enabled = 0,
            active_from_minute = NULL,
            active_to_minute = NULL,
            active_days_mask = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(ACTIVE_TIME_ALL_DAYS_MASK)
    .bind(rule_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule not found"));
    }

    Ok(())
}

pub async fn update_rule_active_days_mask(
    rule_id: i64,
    days_mask: i64,
    pool: &SqlitePool,
) -> Result<()> {
    validate_days_mask(days_mask)?;
    let result = sqlx::query(
        r#"
        UPDATE camera_recording_rules
        SET active_days_mask = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL AND active_time_enabled != 0
        "#,
    )
    .bind(days_mask)
    .bind(rule_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule active time is not enabled"));
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
    if rule.active_time().enabled {
        update_rule_active_time(new_rule_id, rule.active_time(), pool).await?;
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

pub fn parse_active_time_window(raw: &str) -> std::result::Result<RecordingRuleActiveTime, String> {
    let (from, to) = raw
        .trim()
        .split_once('-')
        .ok_or_else(|| "Введите время в формате HH:MM-HH:MM.".to_string())?;
    let from_minute = parse_hhmm(from)?;
    let to_minute = parse_hhmm(to)?;
    RecordingRuleActiveTime::window(from_minute, to_minute, ACTIVE_TIME_ALL_DAYS_MASK)
        .map_err(active_time_error_text)
}

pub fn active_time_matches_now(rule: &RecordingRule) -> bool {
    active_time_matches_at(rule.active_time(), Local::now())
}

pub fn active_time_matches_at(active_time: RecordingRuleActiveTime, now: DateTime<Local>) -> bool {
    if !active_time.enabled {
        return true;
    }
    if validate_active_time_window(
        active_time.from_minute,
        active_time.to_minute,
        active_time.days_mask,
    )
    .is_err()
    {
        return false;
    }

    let minute = i64::from(now.hour() * 60 + now.minute());
    let today = now.weekday().num_days_from_monday();
    let from = active_time.from_minute;
    let to = active_time.to_minute;

    if from < to {
        return day_enabled(active_time.days_mask, today) && minute >= from && minute < to;
    }

    if minute >= from {
        return day_enabled(active_time.days_mask, today);
    }
    if minute < to {
        let previous_day = (today + 6) % 7;
        return day_enabled(active_time.days_mask, previous_day);
    }
    false
}

pub fn active_time_is_valid(active_time: RecordingRuleActiveTime) -> bool {
    !active_time.enabled
        || validate_active_time_window(
            active_time.from_minute,
            active_time.to_minute,
            active_time.days_mask,
        )
        .is_ok()
}

pub fn format_minute(minute: i64) -> String {
    let minute = minute.clamp(0, 1439);
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

pub fn active_time_error_text(error: anyhow::Error) -> String {
    let text = error.to_string();
    if text.contains("invalid time") {
        "Время должно быть в диапазоне 00:00-23:59.".to_string()
    } else if text.contains("same time") {
        "Начало и конец не должны совпадать. Для круглосуточного режима выберите «Всегда»."
            .to_string()
    } else if text.contains("days mask") {
        "Нужно выбрать хотя бы один день недели.".to_string()
    } else {
        text
    }
}

fn validate_active_time_window(from_minute: i64, to_minute: i64, days_mask: i64) -> Result<()> {
    if !(0..=1439).contains(&from_minute) || !(0..=1439).contains(&to_minute) {
        return Err(anyhow!("invalid time minute"));
    }
    if from_minute == to_minute {
        return Err(anyhow!("same time"));
    }
    validate_days_mask(days_mask)?;
    Ok(())
}

fn validate_days_mask(days_mask: i64) -> Result<()> {
    if !(1..=ACTIVE_TIME_ALL_DAYS_MASK).contains(&days_mask) {
        return Err(anyhow!("invalid days mask"));
    }
    Ok(())
}

fn parse_hhmm(value: &str) -> std::result::Result<i64, String> {
    let (hour, minute) = value
        .trim()
        .split_once(':')
        .ok_or_else(|| "Введите время в формате HH:MM-HH:MM.".to_string())?;
    let hour = hour
        .parse::<i64>()
        .map_err(|_| "Часы должны быть числом от 00 до 23.".to_string())?;
    let minute = minute
        .parse::<i64>()
        .map_err(|_| "Минуты должны быть числом от 00 до 59.".to_string())?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return Err("Время должно быть в диапазоне 00:00-23:59.".to_string());
    }
    Ok(hour * 60 + minute)
}

fn day_enabled(days_mask: i64, day_index: u32) -> bool {
    let bit = 1_i64 << day_index;
    days_mask & bit != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local_at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid local datetime")
    }

    fn recording_rule_with_active_time(
        active_from_minute: Option<i64>,
        active_to_minute: Option<i64>,
        active_days_mask: i64,
    ) -> RecordingRule {
        RecordingRule {
            id: 1,
            name: "Test".to_string(),
            camera_id: 1,
            condition_logic: ConditionLogic::Any.as_str().to_string(),
            tail_seconds: 60,
            max_segment_seconds: 60,
            cooldown_s: 0,
            retention_days: 30,
            enabled: 1,
            notify_enabled: 0,
            noise_enabled: 0,
            active_time_enabled: 1,
            active_from_minute,
            active_to_minute,
            active_days_mask,
            noise_summary_sent_at: None,
            last_completed_at: None,
            deleted_at: None,
        }
    }

    #[test]
    fn active_time_regular_window_matches_inside_only() {
        let active = RecordingRuleActiveTime::window(8 * 60, 20 * 60, ACTIVE_TIME_ALL_DAYS_MASK)
            .expect("active time");

        assert!(active_time_matches_at(active, local_at(2026, 6, 1, 12, 0)));
        assert!(!active_time_matches_at(active, local_at(2026, 6, 1, 21, 0)));
    }

    #[test]
    fn active_time_midnight_window_uses_previous_day_for_after_midnight() {
        let friday_mask = 1_i64 << 4;
        let active =
            RecordingRuleActiveTime::window(22 * 60, 7 * 60, friday_mask).expect("active time");

        assert!(active_time_matches_at(active, local_at(2026, 6, 5, 23, 0)));
        assert!(active_time_matches_at(active, local_at(2026, 6, 6, 6, 30)));
        assert!(!active_time_matches_at(active, local_at(2026, 6, 6, 23, 0)));
    }

    #[test]
    fn active_time_days_mask_uses_monday_bit_zero() {
        let monday_mask = 1_i64;
        let active =
            RecordingRuleActiveTime::window(8 * 60, 20 * 60, monday_mask).expect("active time");

        assert!(active_time_matches_at(active, local_at(2026, 6, 1, 12, 0)));
        assert!(!active_time_matches_at(active, local_at(2026, 6, 2, 12, 0)));
    }

    #[test]
    fn invalid_enabled_active_time_does_not_match() {
        let rule = recording_rule_with_active_time(None, Some(20 * 60), ACTIVE_TIME_ALL_DAYS_MASK);
        let active = rule.active_time();

        assert!(!active_time_is_valid(active));
        assert!(!active_time_matches_at(active, local_at(2026, 6, 1, 12, 0)));
    }
}
