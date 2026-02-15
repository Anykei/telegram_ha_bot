use anyhow::Result;
use sqlx::FromRow;
use sqlx::{Row, SqlitePool};

#[derive(FromRow, Debug, Clone)]
pub struct Room {
    pub id: i64,
    pub area: String,
    pub alias: Option<String>,
    pub hide: bool,
}

/// Synchronize rooms from Home Assistant.
///
/// Inserts a new room or updates an existing room's alias if it's currently NULL.
/// The `hide` field is not modified during updates.
pub async fn sync_rooms_from_ha(
    entity_id: &str,
    default_name: &str,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO rooms (area, alias, hide)
        VALUES (?1, ?2, 0)
        ON CONFLICT(area) DO UPDATE SET
            -- If room already exists, we don't touch 'hide'.
            -- We can only update the technical name in alias,
            -- BUT only if it's currently NULL.
            alias = COALESCE(alias, ?2)
        "#
    )
    .bind(entity_id)
    .bind(default_name)
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to sync rooms from HA: {}", e))?;

    Ok(())
}

/// Upsert a room with the given parameters.
///
/// Inserts a new room or updates an existing room's alias and hide status.
pub async fn upsert_room(
    area: &str,
    alias: Option<String>,
    hide: bool,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO rooms (area, alias, hide)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(area) DO UPDATE SET
            alias = COALESCE(?2, alias),
            hide = ?3
        "#
    )
    .bind(area)
    .bind(alias)
    .bind(hide)
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to upsert room: {}", e))?;

    Ok(())
}

/// Get all non-hidden rooms.
pub async fn get_rooms(pool: &SqlitePool) -> Result<Vec<Room>> {
    let rooms = sqlx::query_as::<_, Room>(
        "SELECT id, area, alias, hide FROM rooms WHERE hide = 0"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to fetch rooms: {}", e))?;

    Ok(rooms)
}

/// Get a room by its ID.
pub async fn get_room_by_id(id: i64, pool: &SqlitePool) -> Result<Option<Room>> {
    let row = sqlx::query(
        "SELECT id, area, alias, hide FROM rooms WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to fetch room by ID: {}", e))?;

    Ok(row.map(|row| Room {
        id: row.get("id"),
        area: row.get("area"),
        alias: row.get("alias"), 
        hide: row.get("hide"),
    }))
}

/// Hide a room by setting its hide flag to true.
pub async fn hide_room(area: &str, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "UPDATE rooms SET hide = 1 WHERE area = ?"
    )
    .bind(area)
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to hide room: {}", e))?;

    Ok(())
}

/// Show a room by setting its hide flag to false.
pub async fn show_room(area: &str, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "UPDATE rooms SET hide = 0 WHERE area = ?"
    )
    .bind(area)
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to show room: {}", e))?;

    Ok(())
}
