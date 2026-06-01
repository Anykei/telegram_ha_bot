use anyhow::Result;
use sqlx::FromRow;
use sqlx::{Row, SqlitePool};

#[derive(FromRow, Debug, Clone)]
pub struct Room {
    pub id: i64,
    pub area: String,
    pub alias: Option<String>,
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
        "#,
    )
    .bind(entity_id)
    .bind(default_name)
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to sync rooms from HA: {}", e))?;

    Ok(())
}

/// Get all non-hidden rooms.
pub async fn get_rooms(pool: &SqlitePool) -> Result<Vec<Room>> {
    let rooms = sqlx::query_as::<_, Room>("SELECT id, area, alias FROM rooms WHERE hide = 0")
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch rooms: {}", e))?;

    Ok(rooms)
}

/// Get rooms visible to a specific user.
pub async fn get_rooms_for_user(
    user_id: u64,
    is_admin: bool,
    pool: &SqlitePool,
) -> Result<Vec<Room>> {
    if is_admin {
        return get_rooms(pool).await;
    }

    let rooms = sqlx::query_as::<_, Room>(
        r#"
        SELECT r.id, r.area, r.alias
        FROM rooms r
        LEFT JOIN user_room_access ura ON ura.room_id = r.id AND ura.user_id = ?
        WHERE r.hide = 0 AND COALESCE(ura.can_view, 1) != 0
        ORDER BY r.id
        "#,
    )
    .bind(user_id as i64)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to fetch user rooms: {}", e))?;

    Ok(rooms)
}

/// Get a room by its ID.
pub async fn get_room_by_id(id: i64, pool: &SqlitePool) -> Result<Option<Room>> {
    let row = sqlx::query("SELECT id, area, alias FROM rooms WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch room by ID: {}", e))?;

    Ok(row.map(|row| Room {
        id: row.get("id"),
        area: row.get("area"),
        alias: row.get("alias"),
    }))
}
