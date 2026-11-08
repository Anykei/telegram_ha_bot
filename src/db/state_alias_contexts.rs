//! Short, owner-bound callback references for exact (possibly long) HA states.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

const KIND: &str = "state_alias_context";

#[derive(Serialize, Deserialize)]
pub struct StateAliasContext {
    pub device_id: i64,
    pub entity_id: String,
    pub state: String,
    pub admission_generation: String,
}

pub async fn register(user: u64, context: &StateAliasContext, pool: &SqlitePool) -> Result<i64> {
    let payload = serde_json::to_string(context)?;
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO ui_drafts(owner_id,kind,payload,updated_at,expires_at)
         SELECT ?1,?2,?3,?4,?4+3600
         WHERE EXISTS(SELECT 1 FROM users WHERE id=?1 AND admission_generation=?5)
           AND NOT EXISTS(SELECT 1 FROM ui_drafts WHERE owner_id=?1 AND kind=?2 AND payload=?3 AND expires_at>?4 AND status='editing')",
    ).bind(user as i64).bind(KIND).bind(&payload).bind(now).bind(&context.admission_generation)
        .execute(&mut *tx).await?;
    let id: i64 = sqlx::query_scalar(
        "SELECT id FROM ui_drafts WHERE owner_id=? AND kind=? AND payload=? AND expires_at>? AND status='editing'",
    ).bind(user as i64).bind(KIND).bind(payload).bind(now)
        .fetch_optional(&mut *tx).await?.context("State alias context unavailable")?;
    // Limit references even when a sensor reports a new state on every render.
    sqlx::query("DELETE FROM ui_drafts WHERE owner_id=?1 AND kind=?2 AND id NOT IN (SELECT id FROM ui_drafts WHERE owner_id=?1 AND kind=?2 ORDER BY id DESC LIMIT 256)")
        .bind(user as i64).bind(KIND).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn load(
    id: i64,
    user: u64,
    device: i64,
    pool: &SqlitePool,
) -> Result<Option<StateAliasContext>> {
    let Some(draft) = super::ui_drafts::load(id, user, KIND, pool).await? else {
        return Ok(None);
    };
    let context: StateAliasContext = serde_json::from_str(&draft.payload)?;
    if context.device_id != device
        || !super::user::admission_is_current(user, &context.admission_generation, pool).await?
    {
        return Ok(None);
    }
    let current = super::devices::get_device_by_id(device, pool).await?;
    Ok(current
        .filter(|device| device.entity_id == context.entity_id)
        .map(|_| context))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exact_state_context_is_reused_and_rejects_foreign_expired_and_old_admission(
    ) -> Result<()> {
        let config = crate::test_support::config().await;
        crate::db::add_user(1, &config.db).await?;
        crate::db::rooms::sync_rooms_from_ha("test", "Test", &config.db).await?;
        crate::db::devices::sync_device("sensor.long", "test", "Long", "sensor", &config.db)
            .await?;
        let state = "Очень длинное состояние 🔥 ".repeat(20);
        let context = StateAliasContext {
            device_id: 1,
            entity_id: "sensor.long".into(),
            state: state.clone(),
            admission_generation: crate::db::admission_generation(1, &config.db)
                .await?
                .unwrap(),
        };
        let id = register(1, &context, &config.db).await?;
        assert_eq!(register(1, &context, &config.db).await?, id);
        assert_eq!(load(id, 1, 1, &config.db).await?.unwrap().state, state);
        assert!(load(id, 42, 1, &config.db).await?.is_none());
        assert!(load(id, 1, 2, &config.db).await?.is_none());
        sqlx::query("UPDATE ui_drafts SET expires_at=0 WHERE id=?")
            .bind(id)
            .execute(&config.db)
            .await?;
        assert!(load(id, 1, 1, &config.db).await?.is_none());
        let fresh = register(1, &context, &config.db).await?;
        sqlx::query("UPDATE users SET admission_generation='new' WHERE id=1")
            .execute(&config.db)
            .await?;
        assert!(load(fresh, 1, 1, &config.db).await?.is_none());
        Ok(())
    }
}
