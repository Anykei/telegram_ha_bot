use anyhow::{ensure, Result};
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
pub struct Favorite {
    pub id: i64,
    pub action: String,
}

pub async fn favorites(owner: u64, pool: &SqlitePool) -> Result<Vec<Favorite>> {
    Ok(sqlx::query_as(
        "SELECT id, action FROM user_favorites WHERE owner = ? ORDER BY position, id",
    )
    .bind(owner as i64)
    .fetch_all(pool)
    .await?)
}

pub async fn add_favorite(owner: u64, action: &str, pool: &SqlitePool) -> Result<()> {
    let key = serde_json::from_str::<serde_json::Value>(action)
        .ok()
        .and_then(|value| {
            let object = value.as_object()?;
            let (kind, target) = object.iter().next()?;
            let id = target
                .get("device")
                .or_else(|| target.get("id"))?
                .as_i64()?;
            Some(format!("{kind}:{id}"))
        })
        .unwrap_or_else(|| action.to_string());
    sqlx::query("INSERT INTO user_favorites(owner, action, target_key, position) SELECT ?, ?, ?, COALESCE(MAX(position), 0) + 1 FROM user_favorites WHERE owner = ? ON CONFLICT(owner, target_key) DO UPDATE SET action = excluded.action")
        .bind(owner as i64).bind(action).bind(key).bind(owner as i64).execute(pool).await?;
    Ok(())
}

pub async fn remove_favorite(owner: u64, id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM user_favorites WHERE owner = ? AND id = ?")
        .bind(owner as i64)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn move_favorite_first(owner: u64, id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE user_favorites SET position = (SELECT COALESCE(MIN(position), 0) - 1 FROM user_favorites WHERE owner = ?) WHERE owner = ? AND id = ?")
        .bind(owner as i64).bind(owner as i64).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn set_subscription(
    owner: u64,
    device: i64,
    enabled: bool,
    is_root: bool,
    pool: &SqlitePool,
) -> Result<()> {
    let entity: Option<String> =
        sqlx::query_scalar("SELECT entity_id FROM devices WHERE id = ? AND archived = 0")
            .bind(device)
            .fetch_optional(pool)
            .await?;
    let entity = entity.ok_or_else(|| anyhow::anyhow!("Device unavailable"))?;
    ensure!(
        super::access::can_view_device(owner, is_root, device, pool).await?,
        "Access denied"
    );
    if enabled {
        ensure!(
            super::access::can_notify_entity_for_device_id(owner, is_root, device, pool).await?,
            "Notifications disabled by administrator"
        );
        sqlx::query("INSERT OR IGNORE INTO subscriptions(user_id, entity_id) VALUES (?, ?)")
            .bind(owner as i64)
            .bind(&entity)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("DELETE FROM subscriptions WHERE user_id = ? AND entity_id = ?")
            .bind(owner as i64)
            .bind(&entity)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn set_quiet(owner: u64, enabled: bool, pool: &SqlitePool) -> Result<()> {
    sqlx::query("INSERT INTO user_notification_schedule(user_id, quiet_enabled, quiet_from, quiet_to, critical_only) VALUES (?, ?, '23:00', '07:00', 1) ON CONFLICT(user_id) DO UPDATE SET quiet_enabled = excluded.quiet_enabled, quiet_from = COALESCE(quiet_from, '23:00'), quiet_to = COALESCE(quiet_to, '07:00'), updated_at = CURRENT_TIMESTAMP")
        .bind(owner as i64).bind(enabled).execute(pool).await?;
    Ok(())
}

pub fn parse_quiet_interval(text: &str) -> Result<(&str, &str)> {
    let (from, to) = text
        .trim()
        .split_once(['–', '-'])
        .ok_or_else(|| anyhow::anyhow!("HH:MM–HH:MM"))?;
    let (from, to) = (from.trim(), to.trim());
    ensure!(
        super::user_notification_schedule::valid_hh_mm(from)
            && super::user_notification_schedule::valid_hh_mm(to)
            && from != to,
        "HH:MM–HH:MM; start and end must differ"
    );
    Ok((from, to))
}

pub async fn set_quiet_interval(owner: u64, text: &str, pool: &SqlitePool) -> Result<()> {
    let (from, to) = parse_quiet_interval(text)?;
    sqlx::query("INSERT INTO user_notification_schedule(user_id, quiet_enabled, quiet_from, quiet_to, critical_only) VALUES (?, 0, ?, ?, 1) ON CONFLICT(user_id) DO UPDATE SET quiet_from = excluded.quiet_from, quiet_to = excluded.quiet_to, updated_at = CURRENT_TIMESTAMP")
        .bind(owner as i64).bind(from).bind(to).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn personal_preferences_are_owned_idempotent_and_do_not_activate_quiet_hours(
    ) -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        super::super::add_user(1, &pool).await?;
        super::super::add_user(2, &pool).await?;
        add_favorite(1, "device", &pool).await?;
        add_favorite(1, "device", &pool).await?;
        let id = favorites(1, &pool).await?[0].id;
        remove_favorite(2, id, &pool).await?;
        assert_eq!(favorites(1, &pool).await?.len(), 1);
        assert!(favorites(2, &pool).await?.is_empty());
        set_quiet_interval(1, "22:30–06:00", &pool).await?;
        assert_eq!(
            super::super::user_notification_schedule::get(1, &pool)
                .await?
                .unwrap()
                .quiet_enabled,
            0
        );
        set_quiet(1, true, &pool).await?;
        set_quiet(1, true, &pool).await?;
        sqlx::query(
            "UPDATE user_notification_schedule SET timezone = 'Europe/Moscow' WHERE user_id = 1",
        )
        .execute(&pool)
        .await?;
        set_quiet_interval(1, "23:00-08:00", &pool).await?;
        let saved = super::super::user_notification_schedule::get(1, &pool)
            .await?
            .unwrap();
        assert_eq!(saved.quiet_enabled, 1);
        assert_eq!(saved.timezone.as_deref(), Some("Europe/Moscow"));
        assert!(parse_quiet_interval("12:00-12:00").is_err());
        assert!(parse_quiet_interval("24:00-01:00").is_err());
        assert!(parse_quiet_interval("+1:00-01:00").is_err());
        Ok(())
    }
}
