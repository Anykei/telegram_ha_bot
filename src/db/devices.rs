use sqlx::Row;
use crate::core::types::Device;

pub async fn sync_device(
    pool: &sqlx::SqlitePool,
    ha_entity_id: &str,  // "light.kitchen_led"
    ha_area_id: &str,    // "kitchen" (это строка из HA)
    friendly_name: &str, // "Кухонная подсветка"
    device_class: &str         // "light"
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
        .bind(ha_area_id)   // ?1
        .bind(ha_entity_id) // ?2
        .bind(friendly_name) // ?3
        .bind(device_class)  // ?4
        .bind(domain)        // ?5
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_devices_by_room(room_id: i64, pool: &sqlx::SqlitePool) -> anyhow::Result<Vec<Device>> {
    let rows = sqlx::query(
        "SELECT id, room_id, entity_id, alias, device_class, device_domain FROM devices WHERE room_id = ?"
    )
        .bind(room_id)
        .fetch_all(pool) // Используем переданный pool
        .await?;

    let mut devices = Vec::with_capacity(rows.len());
    for row in rows {
        use sqlx::Row; // Нужно для работы метода .get()

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

pub async fn get_device_by_id(id: i64, pool: &sqlx::SqlitePool) -> sqlx::Result<Option<Device>> {
    sqlx::query_as::<_, Device>(
        "SELECT id, room_id, entity_id, alias, device_class, device_domain FROM devices WHERE id = ?"
    )
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn get_room_id_by_entity(
    pool: &sqlx::SqlitePool,
    entity_id: &str
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query("SELECT room_id FROM devices WHERE entity_id = ?")
        .bind(entity_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| r.get::<i64, _>("room_id")))
}

pub async fn get_all_display_names(pool: &sqlx::SqlitePool) -> anyhow::Result<Vec<(String, String)>> {
    // COALESCE выбирает первый не-NULL аргумент:
    // 1. Сначала наш ручной alias
    // 2. Если его нет — friendly_name из HA
    // 3. Если и его нет — сам технический entity_id
    let rows = sqlx::query(
        "SELECT entity_id, COALESCE(alias, entity_id) as display_name FROM devices"
    )
        .fetch_all(pool)
        .await?;

    let mapping = rows.into_iter()
        .map(|r| (r.get("entity_id"), r.get("display_name")))
        .collect();

    Ok(mapping)
}