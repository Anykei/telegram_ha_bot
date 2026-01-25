use sqlx::SqlitePool;

pub async fn is_subscribed(user_id: i64, entity_id: &str, pool: &SqlitePool) -> bool {
    sqlx::query("SELECT 1 FROM subscriptions WHERE user_id = ? AND entity_id = ?")
        .bind(user_id).bind(entity_id)
        .fetch_optional(pool).await.unwrap_or(None).is_some()
}

pub async fn is_hidden(entity_id: &str, pool: &SqlitePool) -> bool {
    sqlx::query("SELECT 1 FROM hidden_entities WHERE entity_id = ?")
        .bind(entity_id)
        .fetch_optional(pool).await.unwrap_or(None).is_some()
}

pub async fn get_subscribers(pool: &SqlitePool, entity_id: &str) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>("SELECT user_id FROM subscriptions WHERE entity_id = ?")
        .bind(entity_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn toggle_subscription(
    pool: &SqlitePool,
    user_id: i64,
    entity_id: &str,
) -> anyhow::Result<bool> {
    // 1. Проверяем, существует ли запись
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

pub async fn toggle_hidden(
    pool: &SqlitePool,
    entity_id: &str,
) -> anyhow::Result<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM hidden_entities WHERE entity_id = ?)"
    )
        .bind(entity_id)
        .fetch_one(pool)
        .await?;

    if exists {
        sqlx::query("DELETE FROM hidden_entities WHERE entity_id = ?")
            .bind(entity_id)
            .execute(pool)
            .await?;
        Ok(false) // Больше не скрыто
    } else {
        sqlx::query("INSERT INTO hidden_entities (entity_id) VALUES (?)")
            .bind(entity_id)
            .execute(pool)
            .await?;
        Ok(true) // Теперь скрыто
    }
}