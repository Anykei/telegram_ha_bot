use sqlx::Row;
use crate::core::types::Device;

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
        INSERT INTO devices (room_id, entity_id, alias, device_class, device_domain, archived)
        VALUES (
            (SELECT id FROM rooms WHERE area = ?1),
            ?2,
            ?3,
            ?4,
            ?5,
            0
        )
        ON CONFLICT(entity_id) DO UPDATE SET
            room_id = (SELECT id FROM rooms WHERE area = ?1),
            device_class = ?4,
            device_domain = ?5,
            alias = COALESCE(alias, ?3),
            archived = 0
        "#
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
    sqlx::query(
        "INSERT OR IGNORE INTO hidden_entities (entity_id, hide) VALUES (?, 1)"
    )
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
    let rows = sqlx::query(
        "SELECT id, room_id, entity_id, alias, device_class, device_domain, archived FROM devices WHERE room_id = ? AND archived = 0"
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    let mut devices = Vec::with_capacity(rows.len());
    for row in rows {
        devices.push(Device {
            id: row.get("id"),
            room_id: row.get("room_id"),
            entity_id: row.get("entity_id"),
            alias: row.get("alias"),
            device_class: row.get("device_class"),
            device_domain: row.get("device_domain"),
            archived: row.get("archived"),
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
pub async fn get_device_by_id(
    id: i64,
    pool: &sqlx::SqlitePool,
) -> sqlx::Result<Option<Device>> {
    sqlx::query_as::<_, Device>(
        "SELECT id, room_id, entity_id, alias, device_class, device_domain, archived FROM devices WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(pool)
    .await
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
/// 2. Friendly name from Home Assistant
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
        "SELECT entity_id, COALESCE(alias, entity_id) as display_name FROM devices WHERE archived = 0"
    )
    .fetch_all(pool)
    .await?;

    let mapping = rows
        .into_iter()
        .map(|r| (r.get("entity_id"), r.get("display_name")))
        .collect();

    Ok(mapping)
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