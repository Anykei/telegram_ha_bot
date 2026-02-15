use sqlx::SqlitePool;

pub async fn is_subscribed(user_id: i64, entity_id: &str, pool: &SqlitePool) -> anyhow::Result<bool> {
    let exists= sqlx::query("SELECT 1 FROM subscriptions WHERE user_id = ? AND entity_id = ?")
        .bind(user_id).bind(entity_id)
        .fetch_optional(pool).await?
        .is_some();
    Ok(exists)
}

pub async fn is_hidden(entity_id: &str, pool: &SqlitePool) -> anyhow::Result<bool> {
    // Check the actual hide value, not just existence
    let hide_value: Option<i64> = sqlx::query_scalar(
        "SELECT hide FROM hidden_entities WHERE entity_id = ?"
    )
    .bind(entity_id)
    .fetch_optional(pool)
    .await?;
    
    // Return true only if hide = 1, false if hide = 0 or row doesn't exist
    Ok(hide_value.map_or(false, |val| val != 0))
}

pub async fn get_subscribers(pool: &SqlitePool, entity_id: &str) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>("SELECT user_id FROM subscriptions WHERE entity_id = ?")
        .bind(entity_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Toggles subscription for user to entity. Returns true if now subscribed, false if unsubscribed.
pub async fn toggle_subscription(
    pool: &SqlitePool,
    user_id: i64,
    entity_id: &str,
) -> anyhow::Result<bool> {
    // 1. Check if subscription exists
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM subscriptions WHERE user_id = ? AND entity_id = ?)"
    )
        .bind(user_id)
        .bind(entity_id)
        .fetch_one(pool)
        .await?;

    if exists {
        // 2. Если есть — удаляем (отписка)
        sqlx::query("DELETE FROM subscriptions WHERE user_id = ? AND entity_id = ?")
            .bind(user_id)
            .bind(entity_id)
            .execute(pool)
            .await?;
        Ok(false) // Теперь не подписан
    } else {
        // 3. Если нет — добавляем (подписка)
        sqlx::query("INSERT INTO subscriptions (user_id, entity_id) VALUES (?, ?)")
            .bind(user_id)
            .bind(entity_id)
            .execute(pool)
            .await?;
        Ok(true) // Теперь подписан
    }
}

pub async fn entity_exists(entity_id: &str, pool: &SqlitePool) -> anyhow::Result<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM hidden_entities WHERE entity_id = ?)"
    )
        .bind(entity_id)
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

/// Toggles the hidden flag for an entity. If entity doesn't exist, creates it with hide=1.
/// Returns true if entity is now hidden (hide=1), false if visible (hide=0).
pub async fn toggle_hidden(
    pool: &SqlitePool,
    entity_id: &str,
) -> anyhow::Result<bool> {
    // Step 1: Check current state
    let current_hide: Option<i64> = sqlx::query_scalar(
        "SELECT hide FROM hidden_entities WHERE entity_id = ?"
    )
    .bind(entity_id)
    .fetch_optional(pool)
    .await?;

    match current_hide {
        Some(hide_val) => {
            // Row exists: toggle from current value to opposite
            let new_hide = if hide_val == 0 { 1 } else { 0 };
            sqlx::query(
                "UPDATE hidden_entities SET hide = ? WHERE entity_id = ?"
            )
            .bind(new_hide)
            .bind(entity_id)
            .execute(pool)
            .await?;
            
            log::info!("Toggled hidden: {} from {} to {}", entity_id, hide_val, new_hide);
            Ok(new_hide != 0)
        }
        None => {
            // Row doesn't exist: create with hide=0 (visible)
            sqlx::query(
                "INSERT INTO hidden_entities (entity_id, hide) VALUES (?, 0)"
            )
            .bind(entity_id)
            .execute(pool)
            .await?;
            
            log::info!("Created unhidden: {}", entity_id);
            Ok(false) // Now visible
        }
    }
}
