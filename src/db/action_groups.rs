use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const ACCESS_ALL_USERS: &str = "all_users";
pub const TARGET_BOT_GROUP: &str = "bot_group";
pub const TARGET_HA_NATIVE: &str = "ha_native";
pub const COMMAND_TURN_ON: &str = "turn_on";
pub const COMMAND_TURN_OFF: &str = "turn_off";
pub const COMMAND_EXECUTE: &str = "execute";
pub const ALL_DAYS_MASK: i64 = 0b111_1111;

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum ActionGroupCommand {
    Toggle,
    TurnOn,
    TurnOff,
}

impl ActionGroupCommand {
    pub fn as_service(self) -> Option<&'static str> {
        match self {
            Self::Toggle => None,
            Self::TurnOn => Some(COMMAND_TURN_ON),
            Self::TurnOff => Some(COMMAND_TURN_OFF),
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum ActionScheduleCommand {
    TurnOn,
    TurnOff,
    Execute,
}

impl ActionScheduleCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnOn => COMMAND_TURN_ON,
            Self::TurnOff => COMMAND_TURN_OFF,
            Self::Execute => COMMAND_EXECUTE,
        }
    }

    pub fn from_db(value: &str) -> Result<Self> {
        match value {
            COMMAND_TURN_ON => Ok(Self::TurnOn),
            COMMAND_TURN_OFF => Ok(Self::TurnOff),
            COMMAND_EXECUTE => Ok(Self::Execute),
            _ => Err(anyhow!("Unknown action schedule command: {}", value)),
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum ActionTargetRef {
    BotGroup(i64),
    HaNative(i64),
}

impl ActionTargetRef {
    pub fn target_type(self) -> &'static str {
        match self {
            Self::BotGroup(_) => TARGET_BOT_GROUP,
            Self::HaNative(_) => TARGET_HA_NATIVE,
        }
    }

    pub fn target_id(self) -> i64 {
        match self {
            Self::BotGroup(id) | Self::HaNative(id) => id,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionGroupItemsFilter {
    All,
    Selected,
    Unselected,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct ActionGroup {
    pub id: i64,
    pub name: String,
    pub access_scope: String,
    pub enabled: i64,
    pub items_count: i64,
    pub schedules_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ActionGroup {
    pub fn is_enabled(&self) -> bool {
        self.enabled != 0
    }

    pub fn is_visible_to_all_users(&self) -> bool {
        self.access_scope == ACCESS_ALL_USERS
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct ActionGroupDevice {
    pub device_id: i64,
    pub entity_id: String,
    pub display_name: String,
    pub device_domain: String,
    pub room_id: i64,
    pub room_name: String,
    pub archived: i64,
    pub is_selected: i64,
}

impl ActionGroupDevice {
    pub fn is_selected(&self) -> bool {
        self.is_selected != 0
    }

    pub fn is_archived(&self) -> bool {
        self.archived != 0
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct HaNativeTarget {
    pub id: i64,
    pub entity_id: String,
    pub domain: String,
    pub ha_name: String,
    pub display_name: Option<String>,
    pub access_scope: String,
    pub enabled: i64,
    pub archived: i64,
    pub schedules_count: i64,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl HaNativeTarget {
    pub fn is_enabled(&self) -> bool {
        self.enabled != 0
    }

    pub fn is_archived(&self) -> bool {
        self.archived != 0
    }

    pub fn is_visible_to_all_users(&self) -> bool {
        self.access_scope == ACCESS_ALL_USERS
    }

    pub fn display_name(&self) -> &str {
        self.display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.ha_name)
    }
}

#[derive(Debug, Clone)]
pub struct HaNativeDiscovery {
    pub entity_id: String,
    pub domain: String,
    pub ha_name: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct ActionSchedule {
    pub id: i64,
    pub target_type: String,
    pub target_id: i64,
    pub command: String,
    pub time_minute: i64,
    pub days_mask: i64,
    pub enabled: i64,
    pub last_run_at: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ActionSchedule {
    pub fn target_ref(&self) -> Result<ActionTargetRef> {
        match self.target_type.as_str() {
            TARGET_BOT_GROUP => Ok(ActionTargetRef::BotGroup(self.target_id)),
            TARGET_HA_NATIVE => Ok(ActionTargetRef::HaNative(self.target_id)),
            _ => Err(anyhow!("Unknown action schedule target type")),
        }
    }

    pub fn command(&self) -> Result<ActionScheduleCommand> {
        ActionScheduleCommand::from_db(&self.command)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled != 0
    }
}

pub async fn list_action_groups(pool: &SqlitePool) -> Result<Vec<ActionGroup>> {
    action_group_query(
        r#"
        SELECT g.id, g.name, g.access_scope, g.enabled,
               COUNT(DISTINCT i.device_id) AS items_count,
               COUNT(DISTINCT s.id) AS schedules_count,
               g.created_at, g.updated_at
        FROM action_groups g
        LEFT JOIN action_group_items i ON i.group_id = g.id
        LEFT JOIN action_schedules s
          ON s.target_type = 'bot_group' AND s.target_id = g.id
        GROUP BY g.id
        ORDER BY g.name COLLATE NOCASE
        "#,
        pool,
    )
    .await
}

pub async fn list_visible_action_groups(
    is_admin: bool,
    pool: &SqlitePool,
) -> Result<Vec<ActionGroup>> {
    if is_admin {
        return list_action_groups(pool).await;
    }

    Ok(sqlx::query_as::<_, ActionGroup>(
        r#"
        SELECT g.id, g.name, g.access_scope, g.enabled,
               COUNT(DISTINCT i.device_id) AS items_count,
               COUNT(DISTINCT s.id) AS schedules_count,
               g.created_at, g.updated_at
        FROM action_groups g
        LEFT JOIN action_group_items i ON i.group_id = g.id
        LEFT JOIN action_schedules s
          ON s.target_type = 'bot_group' AND s.target_id = g.id
        WHERE g.access_scope = 'all_users'
          AND g.enabled = 1
        GROUP BY g.id
        ORDER BY g.name COLLATE NOCASE
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_action_group(group_id: i64, pool: &SqlitePool) -> Result<Option<ActionGroup>> {
    Ok(sqlx::query_as::<_, ActionGroup>(
        r#"
        SELECT g.id, g.name, g.access_scope, g.enabled,
               COUNT(DISTINCT i.device_id) AS items_count,
               COUNT(DISTINCT s.id) AS schedules_count,
               g.created_at, g.updated_at
        FROM action_groups g
        LEFT JOIN action_group_items i ON i.group_id = g.id
        LEFT JOIN action_schedules s
          ON s.target_type = 'bot_group' AND s.target_id = g.id
        WHERE g.id = ?
        GROUP BY g.id
        "#,
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?)
}

async fn action_group_query(sql: &str, pool: &SqlitePool) -> Result<Vec<ActionGroup>> {
    Ok(sqlx::query_as::<_, ActionGroup>(sql)
        .fetch_all(pool)
        .await?)
}

pub async fn create_action_group(name: &str, pool: &SqlitePool) -> Result<i64> {
    let name = normalize_name(name)?;
    let result = sqlx::query("INSERT INTO action_groups (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(result.last_insert_rowid())
}

pub async fn rename_action_group(group_id: i64, name: &str, pool: &SqlitePool) -> Result<()> {
    let name = normalize_name(name)?;
    let result = sqlx::query(
        "UPDATE action_groups SET name = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(name)
    .bind(group_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "Action group not found");
    Ok(())
}

pub async fn delete_action_group(group_id: i64, pool: &SqlitePool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM action_schedules WHERE target_type = 'bot_group' AND target_id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("DELETE FROM action_groups WHERE id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await?;
    ensure!(result.rows_affected() > 0, "Action group not found");
    tx.commit().await?;
    Ok(())
}

pub async fn toggle_action_group_access(group_id: i64, pool: &SqlitePool) -> Result<String> {
    let result = sqlx::query(
        r#"
        UPDATE action_groups
        SET access_scope = CASE
                WHEN access_scope = 'all_users' THEN 'admin_only'
                ELSE 'all_users'
            END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(group_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "Action group not found");

    let scope =
        sqlx::query_scalar::<_, String>("SELECT access_scope FROM action_groups WHERE id = ?")
            .bind(group_id)
            .fetch_one(pool)
            .await?;
    Ok(scope)
}

pub async fn toggle_action_group_enabled(group_id: i64, pool: &SqlitePool) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE action_groups
        SET enabled = CASE WHEN enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(group_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "Action group not found");

    let enabled = sqlx::query_scalar::<_, i64>("SELECT enabled FROM action_groups WHERE id = ?")
        .bind(group_id)
        .fetch_one(pool)
        .await?;
    Ok(enabled != 0)
}

pub async fn list_action_group_items(
    group_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<ActionGroupDevice>> {
    Ok(sqlx::query_as::<_, ActionGroupDevice>(
        r#"
        SELECT d.id AS device_id,
               d.entity_id,
               COALESCE(NULLIF(TRIM(d.alias), ''), NULLIF(TRIM(d.ha_name), ''), d.entity_id) AS display_name,
               COALESCE(NULLIF(TRIM(d.device_domain), ''),
                   CASE WHEN instr(d.entity_id, '.') > 0 THEN substr(d.entity_id, 1, instr(d.entity_id, '.') - 1) ELSE '' END
               ) AS device_domain,
               d.room_id,
               COALESCE(NULLIF(TRIM(r.alias), ''), NULLIF(TRIM(r.area), ''), 'Без комнаты') AS room_name,
               COALESCE(d.archived, 0) AS archived,
               1 AS is_selected
        FROM action_group_items i
        JOIN devices d ON d.id = i.device_id
        LEFT JOIN rooms r ON r.id = d.room_id
        WHERE i.group_id = ?
        ORDER BY room_name COLLATE NOCASE, device_domain COLLATE NOCASE, display_name COLLATE NOCASE
        "#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_action_group_device_candidates(
    group_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<ActionGroupDevice>> {
    Ok(sqlx::query_as::<_, ActionGroupDevice>(
        r#"
        SELECT d.id AS device_id,
               d.entity_id,
               COALESCE(NULLIF(TRIM(d.alias), ''), NULLIF(TRIM(d.ha_name), ''), d.entity_id) AS display_name,
               COALESCE(NULLIF(TRIM(d.device_domain), ''),
                   CASE WHEN instr(d.entity_id, '.') > 0 THEN substr(d.entity_id, 1, instr(d.entity_id, '.') - 1) ELSE '' END
               ) AS device_domain,
               d.room_id,
               COALESCE(NULLIF(TRIM(r.alias), ''), NULLIF(TRIM(r.area), ''), 'Без комнаты') AS room_name,
               COALESCE(d.archived, 0) AS archived,
               CASE WHEN i.device_id IS NULL THEN 0 ELSE 1 END AS is_selected
        FROM devices d
        LEFT JOIN rooms r ON r.id = d.room_id
        LEFT JOIN action_group_items i ON i.group_id = ? AND i.device_id = d.id
        WHERE COALESCE(
                  NULLIF(TRIM(d.device_domain), ''),
                  CASE WHEN instr(d.entity_id, '.') > 0 THEN substr(d.entity_id, 1, instr(d.entity_id, '.') - 1) ELSE '' END
              ) IN ('light', 'switch')
          AND (COALESCE(d.archived, 0) = 0 OR i.device_id IS NOT NULL)
        ORDER BY room_name COLLATE NOCASE, device_domain COLLATE NOCASE, display_name COLLATE NOCASE
        "#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

pub async fn toggle_action_group_item(
    group_id: i64,
    device_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    let group_exists =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM action_groups WHERE id = ?")
            .bind(group_id)
            .fetch_one(pool)
            .await?
            > 0;
    ensure!(group_exists, "Action group not found");

    let selected = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM action_group_items WHERE group_id = ? AND device_id = ?",
    )
    .bind(group_id)
    .bind(device_id)
    .fetch_one(pool)
    .await?
        > 0;

    if selected {
        sqlx::query("DELETE FROM action_group_items WHERE group_id = ? AND device_id = ?")
            .bind(group_id)
            .bind(device_id)
            .execute(pool)
            .await?;
        Ok(false)
    } else {
        let domain = sqlx::query_scalar::<_, String>(
            r#"
            SELECT COALESCE(NULLIF(TRIM(device_domain), ''),
                CASE WHEN instr(entity_id, '.') > 0 THEN substr(entity_id, 1, instr(entity_id, '.') - 1) ELSE '' END
            )
            FROM devices
            WHERE id = ? AND COALESCE(archived, 0) = 0
            "#,
        )
        .bind(device_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow!("Device not found or archived"))?;
        ensure!(
            matches!(domain.as_str(), "light" | "switch"),
            "Unsupported device domain"
        );

        sqlx::query("INSERT OR IGNORE INTO action_group_items (group_id, device_id) VALUES (?, ?)")
            .bind(group_id)
            .bind(device_id)
            .execute(pool)
            .await?;
        Ok(true)
    }
}

pub async fn sync_ha_native_targets(
    discovered: &[HaNativeDiscovery],
    archive_missing_on_empty: bool,
    pool: &SqlitePool,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let now = Utc::now();
    for target in discovered {
        ensure!(
            matches!(target.domain.as_str(), "script" | "scene"),
            "Unsupported HA-native domain"
        );
        sqlx::query(
            r#"
            INSERT INTO ha_native_targets (entity_id, domain, ha_name, last_seen_at)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(entity_id) DO UPDATE SET
                domain = excluded.domain,
                ha_name = excluded.ha_name,
                archived = 0,
                last_seen_at = excluded.last_seen_at,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&target.entity_id)
        .bind(&target.domain)
        .bind(&target.ha_name)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    if !discovered.is_empty() || archive_missing_on_empty {
        let entity_ids = discovered
            .iter()
            .map(|target| target.entity_id.clone())
            .collect::<Vec<_>>();
        if entity_ids.is_empty() {
            sqlx::query(
                "UPDATE ha_native_targets SET archived = 1, updated_at = CURRENT_TIMESTAMP",
            )
            .execute(&mut *tx)
            .await?;
        } else {
            let placeholders = std::iter::repeat_n("?", entity_ids.len())
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "UPDATE ha_native_targets SET archived = 1, updated_at = CURRENT_TIMESTAMP WHERE entity_id NOT IN ({})",
                placeholders
            );
            let mut query = sqlx::query(&query);
            for entity_id in entity_ids {
                query = query.bind(entity_id);
            }
            query.execute(&mut *tx).await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

pub async fn list_ha_native_targets(pool: &SqlitePool) -> Result<Vec<HaNativeTarget>> {
    Ok(sqlx::query_as::<_, HaNativeTarget>(
        r#"
        SELECT h.id, h.entity_id, h.domain, h.ha_name, h.display_name, h.access_scope,
               h.enabled, h.archived, COUNT(DISTINCT s.id) AS schedules_count,
               h.last_seen_at, h.created_at, h.updated_at
        FROM ha_native_targets h
        LEFT JOIN action_schedules s
          ON s.target_type = 'ha_native' AND s.target_id = h.id
        GROUP BY h.id
        ORDER BY h.archived ASC, COALESCE(NULLIF(TRIM(h.display_name), ''), h.ha_name, h.entity_id) COLLATE NOCASE
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn list_visible_ha_native_targets(
    is_admin: bool,
    pool: &SqlitePool,
) -> Result<Vec<HaNativeTarget>> {
    if is_admin {
        return list_ha_native_targets(pool).await;
    }

    Ok(sqlx::query_as::<_, HaNativeTarget>(
        r#"
        SELECT h.id, h.entity_id, h.domain, h.ha_name, h.display_name, h.access_scope,
               h.enabled, h.archived, COUNT(DISTINCT s.id) AS schedules_count,
               h.last_seen_at, h.created_at, h.updated_at
        FROM ha_native_targets h
        LEFT JOIN action_schedules s
          ON s.target_type = 'ha_native' AND s.target_id = h.id
        WHERE h.access_scope = 'all_users'
          AND h.enabled = 1
          AND h.archived = 0
        GROUP BY h.id
        ORDER BY COALESCE(NULLIF(TRIM(h.display_name), ''), h.ha_name, h.entity_id) COLLATE NOCASE
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_ha_native_target(
    target_id: i64,
    pool: &SqlitePool,
) -> Result<Option<HaNativeTarget>> {
    Ok(sqlx::query_as::<_, HaNativeTarget>(
        r#"
        SELECT h.id, h.entity_id, h.domain, h.ha_name, h.display_name, h.access_scope,
               h.enabled, h.archived, COUNT(DISTINCT s.id) AS schedules_count,
               h.last_seen_at, h.created_at, h.updated_at
        FROM ha_native_targets h
        LEFT JOIN action_schedules s
          ON s.target_type = 'ha_native' AND s.target_id = h.id
        WHERE h.id = ?
        GROUP BY h.id
        "#,
    )
    .bind(target_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn toggle_ha_native_target_access(target_id: i64, pool: &SqlitePool) -> Result<String> {
    let result = sqlx::query(
        r#"
        UPDATE ha_native_targets
        SET access_scope = CASE
                WHEN access_scope = 'all_users' THEN 'admin_only'
                ELSE 'all_users'
            END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(target_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "HA-native target not found");
    let scope =
        sqlx::query_scalar::<_, String>("SELECT access_scope FROM ha_native_targets WHERE id = ?")
            .bind(target_id)
            .fetch_one(pool)
            .await?;
    Ok(scope)
}

pub async fn toggle_ha_native_target_enabled(target_id: i64, pool: &SqlitePool) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE ha_native_targets
        SET enabled = CASE WHEN enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(target_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "HA-native target not found");
    let enabled =
        sqlx::query_scalar::<_, i64>("SELECT enabled FROM ha_native_targets WHERE id = ?")
            .bind(target_id)
            .fetch_one(pool)
            .await?;
    Ok(enabled != 0)
}

pub async fn set_ha_native_target_alias(
    target_id: i64,
    display_name: &str,
    pool: &SqlitePool,
) -> Result<()> {
    let name = normalize_name(display_name)?;
    let result = sqlx::query(
        "UPDATE ha_native_targets SET display_name = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(name)
    .bind(target_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "HA-native target not found");
    Ok(())
}

pub async fn reset_ha_native_target_alias(target_id: i64, pool: &SqlitePool) -> Result<()> {
    let result = sqlx::query(
        "UPDATE ha_native_targets SET display_name = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(target_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "HA-native target not found");
    Ok(())
}

pub async fn list_action_schedules(
    target: ActionTargetRef,
    pool: &SqlitePool,
) -> Result<Vec<ActionSchedule>> {
    Ok(sqlx::query_as::<_, ActionSchedule>(
        r#"
        SELECT id, target_type, target_id, command, time_minute, days_mask, enabled,
               last_run_at, created_at, updated_at
        FROM action_schedules
        WHERE target_type = ? AND target_id = ?
        ORDER BY time_minute, id
        "#,
    )
    .bind(target.target_type())
    .bind(target.target_id())
    .fetch_all(pool)
    .await?)
}

pub async fn get_action_schedule(
    schedule_id: i64,
    pool: &SqlitePool,
) -> Result<Option<ActionSchedule>> {
    Ok(sqlx::query_as::<_, ActionSchedule>(
        r#"
        SELECT id, target_type, target_id, command, time_minute, days_mask, enabled,
               last_run_at, created_at, updated_at
        FROM action_schedules
        WHERE id = ?
        "#,
    )
    .bind(schedule_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn create_action_schedule(
    target: ActionTargetRef,
    command: ActionScheduleCommand,
    time_minute: i64,
    days_mask: i64,
    pool: &SqlitePool,
) -> Result<i64> {
    validate_schedule(target, command, time_minute, days_mask, pool).await?;
    let result = sqlx::query(
        r#"
        INSERT INTO action_schedules (target_type, target_id, command, time_minute, days_mask)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(target.target_type())
    .bind(target.target_id())
    .bind(command.as_str())
    .bind(time_minute)
    .bind(days_mask)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

pub async fn update_action_schedule_time(
    schedule_id: i64,
    time_minute: i64,
    pool: &SqlitePool,
) -> Result<()> {
    ensure!((0..=1439).contains(&time_minute), "Invalid schedule time");
    let result = sqlx::query(
        "UPDATE action_schedules SET time_minute = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(time_minute)
    .bind(schedule_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "Action schedule not found");
    Ok(())
}

pub async fn update_action_schedule_days(
    schedule_id: i64,
    days_mask: i64,
    pool: &SqlitePool,
) -> Result<()> {
    ensure!(
        (1..=ALL_DAYS_MASK).contains(&days_mask),
        "Invalid schedule days"
    );
    let result = sqlx::query(
        "UPDATE action_schedules SET days_mask = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(days_mask)
    .bind(schedule_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "Action schedule not found");
    Ok(())
}

pub async fn cycle_action_schedule_command(schedule_id: i64, pool: &SqlitePool) -> Result<String> {
    let schedule = get_action_schedule(schedule_id, pool)
        .await?
        .ok_or_else(|| anyhow!("Schedule not found"))?;
    ensure!(
        schedule.target_type == TARGET_BOT_GROUP,
        "HA-native schedule command is fixed"
    );
    let next = match schedule.command.as_str() {
        COMMAND_TURN_ON => COMMAND_TURN_OFF,
        COMMAND_TURN_OFF => COMMAND_TURN_ON,
        _ => COMMAND_TURN_ON,
    };
    let result = sqlx::query(
        "UPDATE action_schedules SET command = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(next)
    .bind(schedule_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "Action schedule not found");
    Ok(next.to_string())
}

pub async fn toggle_action_schedule_enabled(schedule_id: i64, pool: &SqlitePool) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE action_schedules
        SET enabled = CASE WHEN enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(schedule_id)
    .execute(pool)
    .await?;
    ensure!(result.rows_affected() > 0, "Action schedule not found");
    let enabled = sqlx::query_scalar::<_, i64>("SELECT enabled FROM action_schedules WHERE id = ?")
        .bind(schedule_id)
        .fetch_one(pool)
        .await?;
    Ok(enabled != 0)
}

pub async fn delete_action_schedule(schedule_id: i64, pool: &SqlitePool) -> Result<()> {
    let result = sqlx::query("DELETE FROM action_schedules WHERE id = ?")
        .bind(schedule_id)
        .execute(pool)
        .await?;
    ensure!(result.rows_affected() > 0, "Action schedule not found");
    Ok(())
}

pub async fn list_due_action_schedules(
    time_minute: i64,
    day_bit: i64,
    run_key: &str,
    pool: &SqlitePool,
) -> Result<Vec<ActionSchedule>> {
    Ok(sqlx::query_as::<_, ActionSchedule>(
        r#"
        SELECT s.id, s.target_type, s.target_id, s.command, s.time_minute, s.days_mask, s.enabled,
               s.last_run_at, s.created_at, s.updated_at
        FROM action_schedules s
        LEFT JOIN action_groups g
          ON s.target_type = 'bot_group' AND s.target_id = g.id
        LEFT JOIN ha_native_targets h
          ON s.target_type = 'ha_native' AND s.target_id = h.id
        WHERE s.enabled = 1
          AND s.time_minute = ?
          AND (s.days_mask & ?) != 0
          AND COALESCE(s.last_run_at, '') != ?
          AND (
              (s.target_type = 'bot_group' AND g.id IS NOT NULL AND g.enabled = 1)
              OR
              (s.target_type = 'ha_native' AND h.id IS NOT NULL AND h.enabled = 1 AND h.archived = 0)
          )
        ORDER BY s.id ASC
        "#,
    )
    .bind(time_minute)
    .bind(day_bit)
    .bind(run_key)
    .fetch_all(pool)
    .await?)
}

pub async fn mark_action_schedule_run(
    schedule_id: i64,
    run_key: &str,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        "UPDATE action_schedules SET last_run_at = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(run_key)
    .bind(schedule_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn validate_schedule(
    target: ActionTargetRef,
    command: ActionScheduleCommand,
    time_minute: i64,
    days_mask: i64,
    pool: &SqlitePool,
) -> Result<()> {
    ensure!((0..=1439).contains(&time_minute), "Invalid schedule time");
    ensure!(
        (1..=ALL_DAYS_MASK).contains(&days_mask),
        "Invalid schedule days"
    );
    match target {
        ActionTargetRef::BotGroup(group_id) => {
            ensure!(
                matches!(
                    command,
                    ActionScheduleCommand::TurnOn | ActionScheduleCommand::TurnOff
                ),
                "Bot-owned group schedule supports only turn_on/turn_off"
            );
            ensure!(
                get_action_group(group_id, pool).await?.is_some(),
                "Action group not found"
            );
        }
        ActionTargetRef::HaNative(target_id) => {
            ensure!(
                matches!(command, ActionScheduleCommand::Execute),
                "HA-native schedule supports only execute"
            );
            let target = get_ha_native_target(target_id, pool)
                .await?
                .ok_or_else(|| anyhow!("HA-native target not found"))?;
            ensure!(
                matches!(target.domain.as_str(), "script" | "scene"),
                "Unsupported HA-native target domain"
            );
            ensure!(!target.is_archived(), "HA-native target is archived");
        }
    }
    Ok(())
}

fn normalize_name(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "Name is empty");
    ensure!(name.chars().count() <= 80, "Name is too long");
    Ok(name.to_string())
}

pub fn format_time_minute(time_minute: i64) -> String {
    format!("{:02}:{:02}", time_minute / 60, time_minute % 60)
}

pub fn parse_time_minute(value: &str) -> Result<i64> {
    let Some((hours, minutes)) = value.trim().split_once(':') else {
        return Err(anyhow!("Введите время в формате HH:MM"));
    };
    let hours: i64 = hours.parse().map_err(|_| anyhow!("Некорректный час"))?;
    let minutes: i64 = minutes
        .parse()
        .map_err(|_| anyhow!("Некорректные минуты"))?;
    ensure!((0..=23).contains(&hours), "Час должен быть от 00 до 23");
    ensure!(
        (0..=59).contains(&minutes),
        "Минуты должны быть от 00 до 59"
    );
    Ok(hours * 60 + minutes)
}

pub fn format_days_mask(days_mask: i64) -> &'static str {
    match days_mask {
        0b111_1111 => "ежедневно",
        0b001_1111 => "Пн-Пт",
        0b110_0000 => "Сб-Вс",
        _ => "выбранные дни",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> Result<SqlitePool> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        sqlx::query(
            r#"
            CREATE TABLE rooms (
                id INTEGER PRIMARY KEY,
                area TEXT NOT NULL UNIQUE,
                alias TEXT,
                hide INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE devices (
                id INTEGER PRIMARY KEY,
                room_id INTEGER NOT NULL,
                entity_id TEXT NOT NULL UNIQUE,
                alias TEXT,
                ha_name TEXT,
                device_class TEXT,
                device_domain TEXT,
                archived INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await?;
        for statement in include_str!("../../migrations/20260607110000_add_action_groups.sql")
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
        {
            sqlx::query(statement).execute(&pool).await?;
        }
        Ok(pool)
    }

    #[tokio::test]
    async fn ha_native_sync_preserves_local_settings_and_archives_missing() -> Result<()> {
        let pool = test_pool().await?;
        sync_ha_native_targets(
            &[HaNativeDiscovery {
                entity_id: "script.good_night".to_string(),
                domain: "script".to_string(),
                ha_name: "Good night".to_string(),
            }],
            true,
            &pool,
        )
        .await?;

        let target = list_ha_native_targets(&pool).await?.remove(0);
        assert_eq!(target.access_scope, "admin_only");
        toggle_ha_native_target_access(target.id, &pool).await?;
        set_ha_native_target_alias(target.id, "Спокойной ночи", &pool).await?;

        sync_ha_native_targets(
            &[HaNativeDiscovery {
                entity_id: "script.good_night".to_string(),
                domain: "script".to_string(),
                ha_name: "Good night updated".to_string(),
            }],
            true,
            &pool,
        )
        .await?;
        let target = get_ha_native_target(target.id, &pool).await?.unwrap();
        assert_eq!(target.access_scope, "all_users");
        assert_eq!(target.display_name.as_deref(), Some("Спокойной ночи"));
        assert_eq!(target.ha_name, "Good night updated");
        assert!(!target.is_archived());

        sync_ha_native_targets(&[], false, &pool).await?;
        let target = get_ha_native_target(target.id, &pool).await?.unwrap();
        assert!(!target.is_archived());

        sync_ha_native_targets(&[], true, &pool).await?;
        let target = get_ha_native_target(target.id, &pool).await?.unwrap();
        assert!(target.is_archived());

        sync_ha_native_targets(
            &[HaNativeDiscovery {
                entity_id: "scene.away".to_string(),
                domain: "scene".to_string(),
                ha_name: "Away".to_string(),
            }],
            true,
            &pool,
        )
        .await?;
        let target = get_ha_native_target(target.id, &pool).await?.unwrap();
        assert!(target.is_archived());
        Ok(())
    }

    #[tokio::test]
    async fn schedules_validate_commands_and_allow_duplicates() -> Result<()> {
        let pool = test_pool().await?;
        let group_id = create_action_group("Свет", &pool).await?;
        let target = ActionTargetRef::BotGroup(group_id);

        let first = create_action_schedule(
            target,
            ActionScheduleCommand::TurnOn,
            450,
            ALL_DAYS_MASK,
            &pool,
        )
        .await?;
        let second = create_action_schedule(
            target,
            ActionScheduleCommand::TurnOn,
            450,
            ALL_DAYS_MASK,
            &pool,
        )
        .await?;
        assert_ne!(first, second);

        assert!(create_action_schedule(
            target,
            ActionScheduleCommand::Execute,
            450,
            ALL_DAYS_MASK,
            &pool
        )
        .await
        .is_err());
        assert!(
            create_action_schedule(target, ActionScheduleCommand::TurnOn, 450, 0, &pool)
                .await
                .is_err()
        );

        delete_action_group(group_id, &pool).await?;
        let schedules = list_action_schedules(target, &pool).await?;
        assert!(schedules.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn due_schedules_skip_paused_and_archived_targets() -> Result<()> {
        let pool = test_pool().await?;
        let group_id = create_action_group("Свет", &pool).await?;
        let group_target = ActionTargetRef::BotGroup(group_id);
        create_action_schedule(
            group_target,
            ActionScheduleCommand::TurnOn,
            450,
            ALL_DAYS_MASK,
            &pool,
        )
        .await?;

        let due = list_due_action_schedules(450, 1, "2026-06-07 07:30", &pool).await?;
        assert_eq!(due.len(), 1);

        toggle_action_group_enabled(group_id, &pool).await?;
        let due = list_due_action_schedules(450, 1, "2026-06-07 07:30", &pool).await?;
        assert!(due.is_empty());

        toggle_action_group_enabled(group_id, &pool).await?;
        sync_ha_native_targets(
            &[HaNativeDiscovery {
                entity_id: "script.good_night".to_string(),
                domain: "script".to_string(),
                ha_name: "Good night".to_string(),
            }],
            true,
            &pool,
        )
        .await?;
        let native = list_ha_native_targets(&pool).await?.remove(0);
        create_action_schedule(
            ActionTargetRef::HaNative(native.id),
            ActionScheduleCommand::Execute,
            450,
            ALL_DAYS_MASK,
            &pool,
        )
        .await?;

        let due = list_due_action_schedules(450, 1, "2026-06-07 07:30", &pool).await?;
        assert_eq!(due.len(), 2);

        toggle_ha_native_target_enabled(native.id, &pool).await?;
        let due = list_due_action_schedules(450, 1, "2026-06-07 07:30", &pool).await?;
        assert_eq!(due.len(), 1);

        toggle_ha_native_target_enabled(native.id, &pool).await?;
        sync_ha_native_targets(
            &[HaNativeDiscovery {
                entity_id: "scene.away".to_string(),
                domain: "scene".to_string(),
                ha_name: "Away".to_string(),
            }],
            true,
            &pool,
        )
        .await?;
        assert!(create_action_schedule(
            ActionTargetRef::HaNative(native.id),
            ActionScheduleCommand::Execute,
            450,
            ALL_DAYS_MASK,
            &pool,
        )
        .await
        .is_err());
        let due = list_due_action_schedules(450, 1, "2026-06-07 07:30", &pool).await?;
        assert_eq!(due.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn device_candidates_hide_archived_unselected_but_keep_selected() -> Result<()> {
        let pool = test_pool().await?;
        sqlx::query("INSERT INTO rooms (id, area, alias) VALUES (1, 'living', 'Гостиная')")
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO devices (id, room_id, entity_id, alias, ha_name, device_domain, archived)
            VALUES
              (1, 1, 'light.ceiling', 'Потолок', 'Ceiling', 'light', 0),
              (2, 1, 'switch.old', 'Старый', 'Old', 'switch', 1),
              (3, 1, 'sensor.temp', 'Температура', 'Temp', 'sensor', 0)
            "#,
        )
        .execute(&pool)
        .await?;
        let group_id = create_action_group("Свет", &pool).await?;

        let candidates = list_action_group_device_candidates(group_id, &pool).await?;
        assert_eq!(
            candidates
                .iter()
                .map(|item| item.entity_id.as_str())
                .collect::<Vec<_>>(),
            vec!["light.ceiling"]
        );

        sqlx::query("INSERT INTO action_group_items (group_id, device_id) VALUES (?, 2)")
            .bind(group_id)
            .execute(&pool)
            .await?;
        let candidates = list_action_group_device_candidates(group_id, &pool).await?;
        assert!(candidates.iter().any(|item| item.entity_id == "switch.old"
            && item.is_archived()
            && item.is_selected()));
        Ok(())
    }
}
