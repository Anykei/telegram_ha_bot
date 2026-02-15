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
    pool: &sqlx::SqlitePool,
    ha_entity_id: &str,
    ha_area_id: &str,
    friendly_name: &str,
    device_class: &str,
) -> anyhow::Result<()> {
    let domain = ha_entity_id
        .split_once('.')
        .map(|(d, _)| d)
        .unwrap_or("unknown");

    sqlx::query(
        r#"
        INSERT INTO devices (room_id, entity_id, alias, device_class, device_domain)
        VALUES (
            (SELECT id FROM rooms WHERE area = ?1),
            ?2,
            ?3,
            ?4,
            ?5
        )
        ON CONFLICT(entity_id) DO UPDATE SET
            room_id = (SELECT id FROM rooms WHERE area = ?1),
            device_class = ?4,
            device_domain = ?5,
            alias = COALESCE(alias, ?3)
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
        "SELECT id, room_id, entity_id, alias, device_class, device_domain FROM devices WHERE room_id = ?"
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
        "SELECT id, room_id, entity_id, alias, device_class, device_domain FROM devices WHERE id = ?"
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
    pool: &sqlx::SqlitePool,
    entity_id: &str,
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
        "SELECT entity_id, COALESCE(alias, entity_id) as display_name FROM devices"
    )
    .fetch_all(pool)
    .await?;

    let mapping = rows
        .into_iter()
        .map(|r| (r.get("entity_id"), r.get("display_name")))
        .collect();

    Ok(mapping)
}