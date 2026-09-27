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

// A room can be opened to reach an individually granted device. This does not
// grant room-wide access (in particular to cameras); those checks stay in access.
const USER_ROOMS_QUERY: &str = r#"
    SELECT r.id, r.area, r.alias
    FROM rooms r
    LEFT JOIN user_room_access ura ON ura.room_id = r.id AND ura.user_id = ?1
    LEFT JOIN user_profiles up ON up.user_id = ?1
    WHERE r.hide = 0 AND (?2 IS NULL OR r.id = ?2)
      AND (
        COALESCE(ura.can_view, CASE WHEN COALESCE(up.role, 'user') IN ('user', 'admin') THEN 1 ELSE 0 END) != 0
        OR EXISTS (
            SELECT 1 FROM devices d
            JOIN user_device_access uda ON uda.entity_id = d.entity_id AND uda.user_id = ?1
            WHERE d.room_id = r.id AND d.archived = 0 AND uda.can_view != 0
        )
      )
    ORDER BY r.id
"#;

/// Get rooms available in device navigation, including individual device grants.
pub async fn get_rooms_for_user(
    user_id: u64,
    is_admin: bool,
    pool: &SqlitePool,
) -> Result<Vec<Room>> {
    if is_admin {
        return get_rooms(pool).await;
    }

    let rooms = sqlx::query_as::<_, Room>(USER_ROOMS_QUERY)
        .bind(user_id as i64)
        .bind(None::<i64>)
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch user rooms: {}", e))?;

    Ok(rooms)
}

/// Authorize room navigation only; devices must still pass their own ACL checks.
pub async fn can_open_room(
    user_id: u64,
    is_admin: bool,
    room_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    if is_admin {
        return Ok(true);
    }
    crate::db::access::get_user_role(user_id, pool).await?;
    Ok(sqlx::query_as::<_, Room>(USER_ROOMS_QUERY)
        .bind(user_id as i64)
        .bind(room_id)
        .fetch_optional(pool)
        .await?
        .is_some())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{access, cameras, devices};

    async fn navigation_pool(role: &str) -> Result<SqlitePool> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        sqlx::query("INSERT INTO users (id) VALUES (10)")
            .execute(&pool)
            .await?;
        access::set_user_role(10, role, &pool).await?;
        sync_rooms_from_ha("room", "Room", &pool).await?;
        sqlx::query("INSERT INTO devices (id, room_id, entity_id) VALUES (1, 1, 'light.granted'), (2, 1, 'light.other')")
            .execute(&pool).await?;
        cameras::add_manual_camera(
            cameras::NewCamera {
                name: "Camera",
                room_id: 1,
                stream_url: "rtsp://unused",
                snapshot_url: None,
                clip_seconds: 10,
            },
            &pool,
        )
        .await?;
        Ok(pool)
    }

    #[tokio::test]
    async fn individual_grants_allow_navigation_without_room_wide_access() -> Result<()> {
        for role in ["guest", "child", "user", "invalid"] {
            let pool = navigation_pool(role).await?;
            if role == "user" {
                access::set_room_access(10, 1, false, false, &pool).await?;
            }
            assert!(get_rooms_for_user(10, false, &pool).await?.is_empty());
            assert!(!can_open_room(10, false, 1, &pool).await?);

            for can_control in [false, true] {
                access::set_device_access(10, "light.granted", true, can_control, true, &pool)
                    .await?;
                let rooms = get_rooms_for_user(10, false, &pool).await?;
                assert_eq!(rooms.len(), 1);
                assert_eq!(rooms[0].id, 1);
                assert!(can_open_room(10, false, 1, &pool).await?);
                let visible = devices::get_devices_by_room_for_user(10, false, 1, &pool).await?;
                assert_eq!(visible.len(), 1);
                assert_eq!(visible[0].entity_id, "light.granted");
                assert_eq!(
                    access::can_control_device(10, false, 1, &pool).await?,
                    can_control
                );
                assert!(!access::can_view_device(10, false, 2, &pool).await?);
                assert!(!access::can_control_device(10, false, 2, &pool).await?);
                assert!(!access::can_notify_entity(10, "light.other", &pool).await?);
                assert!(!access::can_view_room(10, false, 1, &pool).await?);
                assert!(cameras::list_accessible_cameras(10, false, &pool)
                    .await?
                    .is_empty());
                assert!(cameras::get_accessible_camera(10, false, 1, &pool)
                    .await?
                    .is_none());
            }
            access::set_device_access(10, "light.granted", false, false, false, &pool).await?;
            assert!(get_rooms_for_user(10, false, &pool).await?.is_empty());
            assert!(!can_open_room(10, false, 1, &pool).await?);
        }
        Ok(())
    }

    #[tokio::test]
    async fn navigation_respects_archival_global_hiding_and_existing_room_grants() -> Result<()> {
        let pool = navigation_pool("guest").await?;
        access::set_device_access(10, "light.granted", true, true, true, &pool).await?;
        for (statement, undo) in [
            (
                "UPDATE devices SET archived = 1 WHERE id = 1",
                "UPDATE devices SET archived = 0 WHERE id = 1",
            ),
            ("UPDATE rooms SET hide = 1", "UPDATE rooms SET hide = 0"),
        ] {
            sqlx::query(statement).execute(&pool).await?;
            assert!(get_rooms_for_user(10, false, &pool).await?.is_empty());
            assert!(!can_open_room(10, false, 1, &pool).await?);
            sqlx::query(undo).execute(&pool).await?;
            assert!(can_open_room(10, false, 1, &pool).await?);
        }
        access::set_device_access(10, "light.granted", false, false, false, &pool).await?;
        access::set_room_access(10, 1, true, false, &pool).await?;
        assert_eq!(get_rooms_for_user(10, false, &pool).await?.len(), 1);
        assert!(can_open_room(10, false, 1, &pool).await?);
        assert!(access::can_view_device(10, false, 2, &pool).await?);
        assert!(!access::can_control_device(10, false, 2, &pool).await?);
        assert!(cameras::get_accessible_camera(10, false, 1, &pool)
            .await?
            .is_some());
        assert!(!can_open_room(10, false, 999, &pool).await?);

        let pool = navigation_pool("user").await?;
        assert_eq!(get_rooms_for_user(10, false, &pool).await?.len(), 1);
        assert!(can_open_room(10, false, 1, &pool).await?);
        assert!(access::can_control_device(10, false, 2, &pool).await?);
        assert_eq!(get_rooms_for_user(10, true, &pool).await?.len(), 1);
        assert!(can_open_room(10, true, 1, &pool).await?);
        Ok(())
    }
}
