use crate::core::types::Device;
use sqlx::Row;
use std::collections::HashMap;

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingTriggerCandidate {
    pub device_id: i64,
    pub entity_id: String,
    pub display_name: String,
    pub device_domain: String,
    pub device_class: String,
    pub room_id: i64,
    pub room_name: String,
}

/// Synchronizes a device with the database.
///
/// This function inserts or updates a device record in the database based on
/// the Home Assistant entity ID. It extracts the domain from the entity ID
/// and associates it with the appropriate room.
///
/// # Arguments
///
/// * `pool` - A reference to the SQLite connection pool
/// * `ha_entity_id` - The Home Assistant entity ID (e.g., "light.kitchen_led")
/// * `ha_area_id` - The Home Assistant area ID (e.g., "kitchen")
/// * `friendly_name` - The human-readable name of the device
/// * `device_class` - The class of the device (e.g., "light")
///
/// # Returns
///
/// Returns a `Result<()>` indicating success or failure of the operation
pub async fn sync_device(
    ha_entity_id: &str,
    ha_area_id: &str,
    friendly_name: &str,
    device_class: &str,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    let domain = ha_entity_id
        .split_once('.')
        .map(|(d, _)| d)
        .unwrap_or("unknown");

    sqlx::query(
        r#"
        INSERT INTO devices (room_id, entity_id, alias, ha_name, device_class, device_domain, archived)
        VALUES (
            (SELECT id FROM rooms WHERE area = ?1),
            ?2,
            ?3,
            ?3,
            ?4,
            ?5,
            0
        )
        ON CONFLICT(entity_id) DO UPDATE SET
            room_id = (SELECT id FROM rooms WHERE area = ?1),
            alias = CASE
                WHEN alias IS NULL
                  OR TRIM(alias) = ''
                  OR alias = entity_id
                  OR alias = COALESCE(ha_name, alias)
                THEN ?3
                ELSE alias
            END,
            ha_name = ?3,
            device_class = ?4,
            device_domain = ?5,
            archived = 0
        WHERE
            room_id IS NOT (SELECT id FROM rooms WHERE area = ?1)
            OR (
                CASE
                    WHEN alias IS NULL
                      OR TRIM(alias) = ''
                      OR alias = entity_id
                      OR alias = COALESCE(ha_name, alias)
                    THEN ?3
                    ELSE alias
                END
            ) IS NOT alias
            OR ha_name IS NOT ?3
            OR device_class IS NOT ?4
            OR device_domain IS NOT ?5
            OR archived != 0
        "#,
    )
    .bind(ha_area_id)
    .bind(ha_entity_id)
    .bind(friendly_name)
    .bind(device_class)
    .bind(domain)
    .execute(pool)
    .await?;

    // Auto-hide newly discovered devices (user must explicitly unhide them)
    // Uses INSERT OR IGNORE to only add if device doesn't exist in hidden_entities yet
    sqlx::query("INSERT OR IGNORE INTO hidden_entities (entity_id, hide) VALUES (?, 1)")
        .bind(ha_entity_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Retrieves all devices associated with a specific room.
///
/// # Arguments
///
/// * `room_id` - The ID of the room to query
/// * `pool` - A reference to the SQLite connection pool
///
/// # Returns
///
/// Returns a `Result<Vec<Device>>` containing all devices in the specified room
pub async fn get_devices_by_room(
    room_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Vec<Device>> {
    let rows =
        sqlx::query("SELECT id, entity_id, alias FROM devices WHERE room_id = ? AND archived = 0")
            .bind(room_id)
            .fetch_all(pool)
            .await?;

    let mut devices = Vec::with_capacity(rows.len());
    for row in rows {
        devices.push(Device {
            id: row.get("id"),
            entity_id: row.get("entity_id"),
            alias: row.get("alias"),
        });
    }

    Ok(devices)
}

/// Retrieves devices in a room that are visible to a specific user.
pub async fn get_devices_by_room_for_user(
    user_id: u64,
    is_admin: bool,
    room_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Vec<Device>> {
    if is_admin {
        return get_devices_by_room(room_id, pool).await;
    }

    let rows = sqlx::query(
        r#"
        SELECT d.id, d.entity_id, d.alias
        FROM devices d
        JOIN rooms r ON r.id = d.room_id
        LEFT JOIN user_room_access ura ON ura.room_id = d.room_id AND ura.user_id = ?
        LEFT JOIN user_device_access uda ON uda.entity_id = d.entity_id AND uda.user_id = ?
        WHERE d.room_id = ?
          AND d.archived = 0
          AND r.hide = 0
          AND COALESCE(uda.can_view, ura.can_view, 1) != 0
        ORDER BY d.id
        "#,
    )
    .bind(user_id as i64)
    .bind(user_id as i64)
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    let mut devices = Vec::with_capacity(rows.len());
    for row in rows {
        devices.push(Device {
            id: row.get("id"),
            entity_id: row.get("entity_id"),
            alias: row.get("alias"),
        });
    }

    Ok(devices)
}

/// Retrieves a device by its ID.
///
/// # Arguments
///
/// * `id` - The ID of the device to retrieve
/// * `pool` - A reference to the SQLite connection pool
///
/// # Returns
///
/// Returns a `Result<Option<Device>>` containing the device if found, or None if not found
pub async fn get_device_by_id(id: i64, pool: &sqlx::SqlitePool) -> sqlx::Result<Option<Device>> {
    sqlx::query_as::<_, Device>("SELECT id, entity_id, alias FROM devices WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn list_active_lights(pool: &sqlx::SqlitePool) -> anyhow::Result<Vec<Device>> {
    let rows = sqlx::query_as::<_, Device>(
        r#"
        SELECT id, entity_id, alias
        FROM devices
        WHERE archived = 0
          AND entity_id LIKE 'light.%'
        ORDER BY alias, entity_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Retrieves the room ID associated with a device entity.
///
/// # Arguments
///
/// * `pool` - A reference to the SQLite connection pool
/// * `entity_id` - The entity ID to look up
///
/// # Returns
///
/// Returns a `Result<Option<i64>>` containing the room ID if found, or None if not found
pub async fn get_room_id_by_entity(
    entity_id: &str,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query("SELECT room_id FROM devices WHERE entity_id = ?")
        .bind(entity_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| r.get::<i64, _>("room_id")))
}

/// Retrieves all device display names.
///
/// This function returns a mapping of entity IDs to their display names.
/// Display names are prioritized in this order:
/// 1. Manually set alias
/// 2. Name synchronized from Home Assistant
/// 3. The technical entity ID itself
///
/// # Arguments
///
/// * `pool` - A reference to the SQLite connection pool
///
/// # Returns
///
/// Returns a `Result<Vec<(String, String)>>` containing entity_id and display_name pairs
pub async fn get_all_display_names(
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        "SELECT entity_id, COALESCE(alias, ha_name, entity_id) as display_name FROM devices WHERE archived = 0"
    )
    .fetch_all(pool)
    .await?;

    let mapping = rows
        .into_iter()
        .map(|r| (r.get("entity_id"), r.get("display_name")))
        .collect();

    Ok(mapping)
}

#[allow(dead_code)]
pub async fn list_room_recording_trigger_candidates(
    room_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Vec<RecordingTriggerCandidate>> {
    let rows = sqlx::query_as::<_, RecordingTriggerCandidate>(
        r#"
        SELECT id AS device_id,
               entity_id,
               COALESCE(NULLIF(TRIM(alias), ''), NULLIF(TRIM(ha_name), ''), entity_id) AS display_name,
               COALESCE(
                   NULLIF(TRIM(device_domain), ''),
                   CASE
                       WHEN instr(entity_id, '.') > 0
                       THEN substr(entity_id, 1, instr(entity_id, '.') - 1)
                       ELSE ''
                   END
               ) AS device_domain,
               COALESCE(NULLIF(TRIM(device_class), ''), '') AS device_class,
               room_id,
               '' AS room_name
        FROM devices
        WHERE room_id = ?
          AND COALESCE(archived, 0) = 0
          AND COALESCE(
              NULLIF(TRIM(device_domain), ''),
              CASE
                  WHEN instr(entity_id, '.') > 0
                  THEN substr(entity_id, 1, instr(entity_id, '.') - 1)
                  ELSE ''
              END
          ) = 'binary_sensor'
          AND COALESCE(NULLIF(TRIM(device_class), ''), '') IN (
              'door',
              'window',
              'opening',
              'garage_door'
          )
        ORDER BY display_name, entity_id
        "#,
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn list_recording_wizard_candidates(
    current_room_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Vec<RecordingTriggerCandidate>> {
    let rows = sqlx::query_as::<_, RecordingTriggerCandidate>(
        r#"
        SELECT d.id AS device_id,
               d.entity_id,
               COALESCE(NULLIF(TRIM(d.alias), ''), NULLIF(TRIM(d.ha_name), ''), d.entity_id) AS display_name,
               COALESCE(
                   NULLIF(TRIM(d.device_domain), ''),
                   CASE
                       WHEN instr(d.entity_id, '.') > 0
                       THEN substr(d.entity_id, 1, instr(d.entity_id, '.') - 1)
                       ELSE ''
                   END
               ) AS device_domain,
               COALESCE(NULLIF(TRIM(d.device_class), ''), '') AS device_class,
               d.room_id,
               COALESCE(NULLIF(TRIM(r.alias), ''), NULLIF(TRIM(r.area), ''), 'Без комнаты') AS room_name
        FROM devices d
        LEFT JOIN rooms r ON r.id = d.room_id
        WHERE COALESCE(d.archived, 0) = 0
          AND COALESCE(d.entity_id, '') != ''
          AND COALESCE(
              NULLIF(TRIM(d.device_domain), ''),
              CASE
                  WHEN instr(d.entity_id, '.') > 0
                  THEN substr(d.entity_id, 1, instr(d.entity_id, '.') - 1)
                  ELSE ''
              END
          ) IN ('binary_sensor', 'sensor', 'number', 'switch', 'light')
        ORDER BY
          CASE WHEN d.room_id = ? THEN 0 ELSE 1 END,
          room_name COLLATE NOCASE,
          CASE
            WHEN device_domain = 'binary_sensor'
             AND device_class IN ('door', 'window', 'opening', 'garage_door') THEN 0
            WHEN device_domain = 'binary_sensor'
             AND device_class IN ('motion', 'occupancy', 'presence') THEN 1
            WHEN device_domain IN ('sensor', 'number') THEN 2
            WHEN device_domain IN ('switch', 'light') THEN 3
            ELSE 4
          END,
          display_name COLLATE NOCASE,
          d.entity_id COLLATE NOCASE
        "#,
    )
    .bind(current_room_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn get_recording_wizard_candidate(
    device_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Option<RecordingTriggerCandidate>> {
    let row = sqlx::query_as::<_, RecordingTriggerCandidate>(
        r#"
        SELECT d.id AS device_id,
               d.entity_id,
               COALESCE(NULLIF(TRIM(d.alias), ''), NULLIF(TRIM(d.ha_name), ''), d.entity_id) AS display_name,
               COALESCE(
                   NULLIF(TRIM(d.device_domain), ''),
                   CASE
                       WHEN instr(d.entity_id, '.') > 0
                       THEN substr(d.entity_id, 1, instr(d.entity_id, '.') - 1)
                       ELSE ''
                   END
               ) AS device_domain,
               COALESCE(NULLIF(TRIM(d.device_class), ''), '') AS device_class,
               d.room_id,
               COALESCE(NULLIF(TRIM(r.alias), ''), NULLIF(TRIM(r.area), ''), 'Без комнаты') AS room_name
        FROM devices d
        LEFT JOIN rooms r ON r.id = d.room_id
        WHERE d.id = ?
          AND COALESCE(d.archived, 0) = 0
          AND COALESCE(d.entity_id, '') != ''
          AND COALESCE(
              NULLIF(TRIM(d.device_domain), ''),
              CASE
                  WHEN instr(d.entity_id, '.') > 0
                  THEN substr(d.entity_id, 1, instr(d.entity_id, '.') - 1)
                  ELSE ''
              END
          ) IN ('binary_sensor', 'sensor', 'number', 'switch', 'light')
        "#,
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Archives devices that no longer exist in Home Assistant.
///
/// This function should be called after a full sync to mark devices
/// that were not updated during the sync as archived.
///
/// # Arguments
///
/// * `pool` - A reference to the SQLite connection pool
/// * `synced_entity_ids` - A list of entity IDs that were synced
///
/// # Returns
///
/// Returns a `Result<usize>` with the number of archived devices
pub async fn archive_missing_devices(
    synced_entity_ids: &[String],
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<usize> {
    if synced_entity_ids.is_empty() {
        return Ok(0);
    }

    // Build placeholders for IN clause
    let placeholders = synced_entity_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");

    let query_str = format!(
        "UPDATE devices SET archived = 1 WHERE entity_id NOT IN ({}) AND archived = 0",
        placeholders
    );

    let mut query = sqlx::query(&query_str);
    for entity_id in synced_entity_ids {
        query = query.bind(entity_id);
    }

    let result = query.execute(pool).await?;
    Ok(result.rows_affected() as usize)
}

pub async fn is_state_inverted(entity_id: &str, pool: &sqlx::SqlitePool) -> anyhow::Result<bool> {
    let value: i64 =
        sqlx::query_scalar("SELECT COALESCE(state_inverted, 0) FROM devices WHERE entity_id = ?")
            .bind(entity_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(0);

    Ok(value != 0)
}

pub async fn toggle_state_inversion(
    device_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<bool> {
    sqlx::query(
        r#"
        UPDATE devices
        SET state_inverted = CASE WHEN COALESCE(state_inverted, 0) = 0 THEN 1 ELSE 0 END
        WHERE id = ?
        "#,
    )
    .bind(device_id)
    .execute(pool)
    .await?;

    let value: i64 =
        sqlx::query_scalar("SELECT COALESCE(state_inverted, 0) FROM devices WHERE id = ?")
            .bind(device_id)
            .fetch_one(pool)
            .await?;

    Ok(value != 0)
}

pub async fn is_device_critical(entity_id: &str, pool: &sqlx::SqlitePool) -> anyhow::Result<bool> {
    let value: i64 =
        sqlx::query_scalar("SELECT COALESCE(critical, 0) FROM devices WHERE entity_id = ?")
            .bind(entity_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(0);

    Ok(value != 0)
}

pub async fn toggle_device_critical(
    device_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<bool> {
    sqlx::query(
        r#"
        UPDATE devices
        SET critical = CASE WHEN COALESCE(critical, 0) = 0 THEN 1 ELSE 0 END
        WHERE id = ?
        "#,
    )
    .bind(device_id)
    .execute(pool)
    .await?;

    let value: i64 = sqlx::query_scalar("SELECT COALESCE(critical, 0) FROM devices WHERE id = ?")
        .bind(device_id)
        .fetch_one(pool)
        .await?;

    Ok(value != 0)
}

pub async fn get_state_alias(
    entity_id: &str,
    original_state: &str,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Option<String>> {
    let alias = sqlx::query_scalar(
        "SELECT human_state FROM state_aliases WHERE entity_id = ? AND original_state = ?",
    )
    .bind(entity_id)
    .bind(original_state)
    .fetch_optional(pool)
    .await?;

    Ok(alias)
}

pub async fn get_state_aliases_for_entity(
    entity_id: &str,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT original_state, human_state FROM state_aliases WHERE entity_id = ? ORDER BY original_state",
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().collect())
}

pub async fn set_state_alias(
    entity_id: &str,
    original_state: &str,
    human_state: &str,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO state_aliases (entity_id, original_state, human_state)
        VALUES (?, ?, ?)
        ON CONFLICT(entity_id, original_state) DO UPDATE SET
            human_state = excluded.human_state
        "#,
    )
    .bind(entity_id)
    .bind(original_state)
    .bind(human_state)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_state_alias(
    entity_id: &str,
    original_state: &str,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM state_aliases WHERE entity_id = ? AND original_state = ?")
        .bind(entity_id)
        .bind(original_state)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> anyhow::Result<sqlx::SqlitePool> {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query(
            r#"
            CREATE TABLE rooms (
                id INTEGER PRIMARY KEY,
                area TEXT,
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

        Ok(pool)
    }

    #[tokio::test]
    async fn list_active_lights_returns_only_light_entities() -> anyhow::Result<()> {
        let pool = test_pool().await?;
        sqlx::query(
            r#"
            INSERT INTO devices (id, room_id, entity_id, alias, archived)
            VALUES
                (1, 1, 'light.hall', 'Hall', 0),
                (2, 1, 'switch.socket', 'Socket', 0),
                (3, 1, 'light.old', 'Old', 1)
            "#,
        )
        .execute(&pool)
        .await?;

        let lights = list_active_lights(&pool).await?;

        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].entity_id, "light.hall");

        Ok(())
    }

    #[tokio::test]
    async fn recording_trigger_candidates_return_room_door_sensors() -> anyhow::Result<()> {
        let pool = test_pool().await?;
        sqlx::query(
            r#"
            INSERT INTO devices (
                id, room_id, entity_id, alias, ha_name, device_class, device_domain, archived
            )
            VALUES
                (1, 4, 'binary_sensor.zamok_contact', 'Замок Дверь', 'Door', 'door', 'binary_sensor', 0),
                (2, 4, 'binary_sensor.motion', 'Motion', 'Motion', 'motion', 'binary_sensor', 0),
                (3, 4, 'light.hall', 'Hall', 'Hall', '', 'light', 0),
                (4, 5, 'binary_sensor.window', 'Window', 'Window', 'window', 'binary_sensor', 0),
                (5, 4, 'binary_sensor.old_door', 'Old Door', 'Old Door', 'door', 'binary_sensor', 1)
            "#,
        )
        .execute(&pool)
        .await?;

        let candidates = list_room_recording_trigger_candidates(4, &pool).await?;

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].device_id, 1);
        assert_eq!(candidates[0].entity_id, "binary_sensor.zamok_contact");
        assert_eq!(candidates[0].display_name, "Замок Дверь");
        assert_eq!(candidates[0].device_domain, "binary_sensor");
        assert_eq!(candidates[0].device_class, "door");

        Ok(())
    }

    #[tokio::test]
    async fn recording_wizard_candidates_include_supported_devices_from_all_rooms(
    ) -> anyhow::Result<()> {
        let pool = test_pool().await?;
        sqlx::query(
            r#"
            INSERT INTO rooms (id, area, alias)
            VALUES (4, 'hall', 'Прихожая'), (5, 'kitchen', 'Кухня')
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO devices (
                id, room_id, entity_id, alias, ha_name, device_class, device_domain, archived
            )
            VALUES
                (1, 5, 'sensor.kitchen_temperature', 'Температура кухня', 'Temp', 'temperature', 'sensor', 0),
                (2, 4, 'binary_sensor.zamok_contact', 'Замок Дверь', 'Door', 'door', 'binary_sensor', 0),
                (3, 5, 'number.heater_target', 'Целевая температура', 'Target', '', 'number', 0),
                (4, 4, 'button.skip', 'Skip', 'Skip', '', 'button', 0),
                (5, 5, 'sensor.old', 'Old', 'Old', 'temperature', 'sensor', 1)
            "#,
        )
        .execute(&pool)
        .await?;

        let candidates = list_recording_wizard_candidates(4, &pool).await?;

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.entity_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "binary_sensor.zamok_contact",
                "sensor.kitchen_temperature",
                "number.heater_target"
            ]
        );
        assert_eq!(candidates[0].room_id, 4);
        assert_eq!(candidates[0].room_name, "Прихожая");
        assert_eq!(candidates[1].room_name, "Кухня");

        Ok(())
    }

    #[tokio::test]
    async fn recording_wizard_candidate_rejects_unsupported_device_domain() -> anyhow::Result<()> {
        let pool = test_pool().await?;
        sqlx::query(
            r#"
            INSERT INTO rooms (id, area, alias)
            VALUES (4, 'hall', 'Прихожая')
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO devices (
                id, room_id, entity_id, alias, ha_name, device_class, device_domain, archived
            )
            VALUES
                (1, 4, 'button.skip', 'Skip', 'Skip', '', 'button', 0),
                (2, 4, 'binary_sensor.zamok_contact', 'Замок Дверь', 'Door', 'door', 'binary_sensor', 0)
            "#,
        )
        .execute(&pool)
        .await?;

        let unsupported = get_recording_wizard_candidate(1, &pool).await?;
        let supported = get_recording_wizard_candidate(2, &pool).await?;

        assert!(unsupported.is_none());
        assert_eq!(
            supported
                .as_ref()
                .map(|candidate| candidate.entity_id.as_str()),
            Some("binary_sensor.zamok_contact")
        );

        Ok(())
    }
}
