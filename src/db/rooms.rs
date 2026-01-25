use anyhow::Result;
use sqlx::FromRow;
use sqlx::{Row, SqlitePool};

#[derive(FromRow, Debug)]
pub struct Room{
    pub id: i64,
    pub area: String,
    pub alias: Option<String>,
    pub hide: bool,
}

pub async fn sync_rooms_from_ha(
    pool: &SqlitePool,
    entity_id: &str,
    default_name: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO rooms (area, alias, hide)
        VALUES (?1, ?2, 0)
        ON CONFLICT(area) DO UPDATE SET
            -- Если комната уже есть, мы НЕ трогаем 'hide'.
            -- Мы можем только обновить техническое имя в alias,
            -- НО только если там еще пусто (NULL).
            alias = COALESCE(alias, ?2)
        "#
    )
        .bind(entity_id)
        .bind(default_name)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn upsert_room(
    pool: &SqlitePool,
    entity_id: &str,
    alias: Option<String>,
    hide: bool,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO rooms (area, alias, hide)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(entity_id) DO UPDATE SET
            alias = COALESCE(?2, alias),
            hide = ?3
        "#
    )
        .bind(entity_id)
        .bind(alias)
        .bind(hide)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_rooms(pool: &SqlitePool) -> Result<Vec<Room>> {
    let rooms = sqlx::query_as::<_, Room>(
        "SELECT id, area, alias, hide FROM rooms WHERE hide = 0")
        .fetch_all(pool)
        .await?;

    Ok(rooms)
}

pub async fn get_room_by_id(id: i64, pool: &SqlitePool) -> Result<Option<Room>> {
    let row = sqlx::query(
        "SELECT id, area, alias, hide FROM rooms WHERE id = ?"
    )
        .bind(id)
        .fetch_optional(pool)
        .await?;

    if let Some(row) = row {
        Ok(Some(Room {
            id: row.get("id"),
            area: row.get("area"),
            alias: row.get("alias"), 
            hide: row.get("hide"),
        }))
    } else {
        Ok(None)
    }
}