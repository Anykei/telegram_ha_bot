use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Camera {
    pub id: i64,
    pub name: String,
    pub room_id: Option<i64>,
    pub stream_url: String,
    pub snapshot_url: Option<String>,
    pub clip_seconds: i64,
}

pub struct NewCamera<'a> {
    pub name: &'a str,
    pub room_id: i64,
    pub stream_url: &'a str,
    pub snapshot_url: Option<&'a str>,
    pub clip_seconds: u32,
}

pub async fn add_manual_camera(camera: NewCamera<'_>, pool: &SqlitePool) -> Result<i64> {
    let key = format!(
        "manual:{}:{}",
        camera.room_id,
        chrono::Utc::now().timestamp_millis()
    );

    let result = sqlx::query(
        r#"
        INSERT INTO cameras (key, name, room_id, stream_url, snapshot_url, clip_seconds, enabled, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, 1, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(key)
    .bind(camera.name.trim())
    .bind(camera.room_id)
    .bind(camera.stream_url.trim())
    .bind(camera.snapshot_url.map(str::trim))
    .bind(camera.clip_seconds as i64)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn list_room_cameras(room_id: i64, pool: &SqlitePool) -> Result<Vec<Camera>> {
    Ok(sqlx::query_as::<_, Camera>(
        r#"
        SELECT id, name, room_id, stream_url, snapshot_url, clip_seconds
        FROM cameras
        WHERE room_id = ? AND enabled != 0
        ORDER BY name
        "#,
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?)
}

pub async fn count_room_cameras(room_id: i64, pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM cameras WHERE room_id = ? AND enabled != 0",
    )
    .bind(room_id)
    .fetch_one(pool)
    .await?)
}

pub async fn get_camera(camera_id: i64, pool: &SqlitePool) -> Result<Option<Camera>> {
    Ok(sqlx::query_as::<_, Camera>(
        r#"
        SELECT id, name, room_id, stream_url, snapshot_url, clip_seconds
        FROM cameras
        WHERE id = ? AND enabled != 0
        "#,
    )
    .bind(camera_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn disable_camera(camera_id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE cameras SET enabled = 0, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(camera_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn list_accessible_cameras(
    user_id: u64,
    is_admin: bool,
    pool: &SqlitePool,
) -> Result<Vec<Camera>> {
    let cameras = sqlx::query_as::<_, Camera>(
        r#"
        SELECT id, name, room_id, stream_url, snapshot_url, clip_seconds
        FROM cameras
        WHERE enabled != 0
        ORDER BY name
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for camera in cameras {
        if can_view_camera(user_id, is_admin, &camera, pool).await? {
            result.push(camera);
        }
    }

    Ok(result)
}

pub async fn get_accessible_camera(
    user_id: u64,
    is_admin: bool,
    camera_id: i64,
    pool: &SqlitePool,
) -> Result<Option<Camera>> {
    let camera = sqlx::query_as::<_, Camera>(
        r#"
        SELECT id, name, room_id, stream_url, snapshot_url, clip_seconds
        FROM cameras
        WHERE id = ? AND enabled != 0
        "#,
    )
    .bind(camera_id)
    .fetch_optional(pool)
    .await?;

    let Some(camera) = camera else {
        return Ok(None);
    };

    if can_view_camera(user_id, is_admin, &camera, pool).await? {
        Ok(Some(camera))
    } else {
        Ok(None)
    }
}

async fn can_view_camera(
    user_id: u64,
    is_admin: bool,
    camera: &Camera,
    pool: &SqlitePool,
) -> Result<bool> {
    if is_admin {
        return Ok(true);
    }

    let Some(room_id) = camera.room_id else {
        return Ok(false);
    };

    crate::db::access::can_view_room(user_id, is_admin, room_id, pool).await
}
