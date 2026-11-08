use anyhow::Result;
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
pub struct Confirmation {
    pub action_json: String,
    pub expires_at: i64,
    pub status: String,
    pub result_text: Option<String>,
    pub result_json: Option<String>,
    pub replacement_id: Option<i64>,
}

pub async fn create(user: u64, action: &str, pool: &SqlitePool) -> Result<i64> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE ui_confirmations SET status = 'cancelled' WHERE user_id = ? AND status = 'pending'",
    )
    .bind(user as i64)
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query(
        "INSERT INTO ui_confirmations (user_id, action_json, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user as i64)
    .bind(action)
    .bind(chrono::Utc::now().timestamp() + 120)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.last_insert_rowid())
}

pub async fn get(id: i64, user: u64, pool: &SqlitePool) -> Result<Option<Confirmation>> {
    Ok(sqlx::query_as("SELECT action_json, expires_at, status, result_text, result_json, replacement_id FROM ui_confirmations WHERE id = ? AND user_id = ?")
        .bind(id).bind(user as i64).fetch_optional(pool).await?)
}

/// Claim before sending anything to HA. A timeout or process restart must not replay the action.
pub async fn claim(id: i64, user: u64, pool: &SqlitePool) -> Result<bool> {
    Ok(sqlx::query("UPDATE ui_confirmations SET status = 'executing' WHERE id = ? AND user_id = ? AND status = 'pending' AND expires_at > CAST(strftime('%s', 'now') AS INTEGER)")
        .bind(id).bind(user as i64).execute(pool).await?.rows_affected() == 1)
}

pub async fn cancel(id: i64, user: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE ui_confirmations SET status = 'cancelled' WHERE id = ? AND user_id = ? AND status = 'pending'")
        .bind(id).bind(user as i64).execute(pool).await?;
    Ok(())
}

pub async fn finish(
    id: i64,
    result: &crate::core::action_results::ActionResult,
    text: &str,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query("UPDATE ui_confirmations SET status = ?, result_text = ?, result_json = ? WHERE id = ? AND status = 'executing'")
        .bind(if result.outcome == crate::core::action_results::Outcome::Sent { "executed" } else { "failed" })
        .bind(text).bind(serde_json::to_string(result)?).bind(id).execute(pool).await?;
    Ok(())
}

/// Reuse only a live, unclaimed operation with the same physical action and
/// critical-target consent snapshot. Rendering never extends its original TTL.
pub async fn create_independent(user: u64, action: &str, pool: &SqlitePool) -> Result<i64> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let id = reuse_pending(user, action, &mut tx).await?;
    tx.commit().await?;
    Ok(id)
}

fn reuse_key(action: &str) -> Result<String> {
    let proposal: serde_json::Value = serde_json::from_str(action)?;
    let critical = proposal
        .get("critical_targets")
        .and_then(serde_json::Value::as_array);
    let mut bindings = std::collections::BTreeSet::new();
    if let Some(sources) = proposal
        .get("displayed_devices")
        .and_then(serde_json::Value::as_array)
    {
        for source in sources {
            let Some(pair) = source.as_array() else {
                continue;
            };
            let Some(id) = pair.first().and_then(serde_json::Value::as_i64) else {
                continue;
            };
            let Some(entity) = pair.get(1).and_then(serde_json::Value::as_str) else {
                continue;
            };
            if critical.is_some_and(|ids| ids.iter().any(|target| target.as_i64() == Some(id))) {
                bindings.insert((id, entity));
            }
        }
    }
    Ok(serde_json::to_string(&serde_json::json!({
        "action": proposal.get("action").unwrap_or(&proposal),
        "critical_targets": proposal.get("critical_targets").cloned().unwrap_or_else(|| serde_json::json!([])),
        "critical_bindings": bindings,
    }))?)
}

async fn reuse_pending(user: u64, action: &str, conn: &mut sqlx::SqliteConnection) -> Result<i64> {
    let key = reuse_key(action)?;
    let now = chrono::Utc::now().timestamp();
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM ui_confirmations WHERE user_id = ? AND reuse_key = ? AND status = 'pending' AND expires_at > ? ORDER BY id LIMIT 1")
        .bind(user as i64).bind(&key).bind(now).fetch_optional(&mut *conn).await?;
    if let Some(id) = existing {
        // Display labels/back navigation can change without changing consent.
        sqlx::query("UPDATE ui_confirmations SET action_json = ? WHERE id = ?")
            .bind(action)
            .bind(id)
            .execute(conn)
            .await?;
        return Ok(id);
    }
    Ok(sqlx::query("INSERT INTO ui_confirmations(user_id, action_json, reuse_key, expires_at) VALUES (?, ?, ?, ?)")
        .bind(user as i64).bind(action).bind(key).bind(now + 120)
        .execute(conn).await?.last_insert_rowid())
}

/// Cancel the stale auto-action and create/reuse a distinct review token. The
/// original callback can show the replacement but can never confirm it.
pub async fn replace_for_review(
    id: i64,
    user: u64,
    action: &str,
    pool: &SqlitePool,
) -> Result<Option<i64>> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let record: Option<(String, i64, Option<i64>)> = sqlx::query_as("SELECT status, expires_at, replacement_id FROM ui_confirmations WHERE id = ? AND user_id = ?")
        .bind(id).bind(user as i64).fetch_optional(&mut *tx).await?;
    let Some((status, expires, replacement)) = record else {
        return Ok(None);
    };
    if replacement.is_some() {
        return Ok(replacement);
    }
    if status != "pending" || expires <= chrono::Utc::now().timestamp() {
        return Ok(None);
    }
    let changed = sqlx::query(
        "UPDATE ui_confirmations SET status = 'cancelled' WHERE id = ? AND status = 'pending' AND expires_at > CAST(strftime('%s', 'now') AS INTEGER)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?.rows_affected();
    if changed == 0 {
        return Ok(None);
    }
    let replacement = reuse_pending(user, action, &mut tx).await?;
    sqlx::query("UPDATE ui_confirmations SET replacement_id = ? WHERE id = ?")
        .bind(replacement)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(replacement))
}

/// Keep receipts for begun operations; discard only unused, expired proposals.
/// Legacy identity tombstones deliberately survive this cleanup.
pub async fn purge_expired(pool: &SqlitePool) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM ui_confirmations WHERE status IN ('pending', 'cancelled') AND expires_at <= ?",
    )
    .bind(chrono::Utc::now().timestamp())
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn callback_operation(
    user: u64,
    key: &str,
    pool: &SqlitePool,
) -> Result<Option<(i64, bool)>> {
    Ok(sqlx::query_as("SELECT confirmation_id, requires_review FROM ui_action_buttons WHERE user_id = ? AND button_key = ?")
        .bind(user as i64).bind(key).fetch_optional(pool).await?)
}

pub async fn bind_callback(
    user: u64,
    key: &str,
    action: &str,
    review: bool,
    pool: &SqlitePool,
) -> Result<(i64, bool)> {
    let mut tx = pool.begin().await?;
    // Acquire SQLite's writer lock before the read so two concurrent adaptations serialize.
    let candidate = sqlx::query(
        "INSERT INTO ui_confirmations(user_id, action_json, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user as i64)
    .bind(action)
    .bind(chrono::Utc::now().timestamp() + 120)
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    sqlx::query("INSERT OR IGNORE INTO ui_action_buttons(user_id, button_key, confirmation_id, requires_review) VALUES (?, ?, ?, ?)")
        .bind(user as i64).bind(key).bind(candidate).bind(review).execute(&mut *tx).await?;
    let saved: (i64, bool) = sqlx::query_as("SELECT confirmation_id, requires_review FROM ui_action_buttons WHERE user_id = ? AND button_key = ?")
        .bind(user as i64).bind(key).fetch_one(&mut *tx).await?;
    if saved.0 != candidate {
        sqlx::query("DELETE FROM ui_confirmations WHERE id = ?")
            .bind(candidate)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(critical: &[i64], description: &str) -> String {
        serde_json::json!({
            "action": {"Group": {"group": 7, "command": "TurnOff", "targets": [3]}},
            "critical_targets": critical,
            "description": description,
            "back": "Home",
        })
        .to_string()
    }

    #[tokio::test]
    async fn equivalent_pending_reuses_identity_without_extending_expiry() -> Result<()> {
        let config = crate::test_support::config().await;
        crate::db::add_user(42, &config.db).await?;
        crate::db::add_user(43, &config.db).await?;
        let action = proposal(&[], "original");
        let (a, b) = tokio::join!(
            create_independent(42, &action, &config.db),
            create_independent(42, &action, &config.db),
        );
        let id = a?;
        assert_eq!(id, b?);
        let expiry = chrono::Utc::now().timestamp() + 30;
        sqlx::query("UPDATE ui_confirmations SET expires_at = ? WHERE id = ?")
            .bind(expiry)
            .bind(id)
            .execute(&config.db)
            .await?;
        assert_eq!(
            create_independent(42, &proposal(&[], "renamed"), &config.db).await?,
            id
        );
        let saved = get(id, 42, &config.db).await?.unwrap();
        assert_eq!(saved.expires_at, expiry);
        assert!(saved.action_json.contains("renamed"));
        assert_ne!(create_independent(43, &action, &config.db).await?, id);
        assert_ne!(
            create_independent(42, &proposal(&[3], "critical"), &config.db).await?,
            id
        );
        assert!(claim(id, 42, &config.db).await?);
        assert_ne!(create_independent(42, &action, &config.db).await?, id);
        assert!(!claim(id, 42, &config.db).await?);
        Ok(())
    }

    #[tokio::test]
    async fn expired_cleanup_preserves_receipts_and_legacy_replay_tombstones() -> Result<()> {
        let config = crate::test_support::config().await;
        crate::db::add_user(42, &config.db).await?;
        let action = proposal(&[], "original");
        let pending = create_independent(42, &action, &config.db).await?;
        let cancelled = create_independent(42, &proposal(&[3], "critical"), &config.db).await?;
        cancel(cancelled, 42, &config.db).await?;
        let (legacy, review) =
            bind_callback(42, "message:old_button", &action, false, &config.db).await?;
        assert!(!review);
        let mut retained = Vec::new();
        for status in ["executing", "executed", "failed"] {
            let id = create(42, &action, &config.db).await?;
            sqlx::query("UPDATE ui_confirmations SET status = ? WHERE id = ?")
                .bind(status)
                .bind(id)
                .execute(&config.db)
                .await?;
            retained.push(id);
        }
        sqlx::query("UPDATE ui_confirmations SET expires_at = 0")
            .execute(&config.db)
            .await?;
        assert_eq!(purge_expired(&config.db).await?, 3);
        for id in [pending, cancelled, legacy] {
            assert!(get(id, 42, &config.db).await?.is_none());
            assert!(!claim(id, 42, &config.db).await?);
        }
        for id in retained {
            assert!(get(id, 42, &config.db).await?.is_some());
            assert!(!claim(id, 42, &config.db).await?);
        }
        assert_eq!(
            bind_callback(42, "message:old_button", &action, false, &config.db).await?,
            (legacy, false)
        );
        sqlx::query("DELETE FROM users WHERE id = 42")
            .execute(&config.db)
            .await?;
        crate::db::add_user(42, &config.db).await?;
        assert_eq!(
            callback_operation(42, "message:old_button", &config.db).await?,
            Some((legacy, false))
        );
        assert!(!claim(legacy, 42, &config.db).await?);
        let fresh = create_independent(42, &action, &config.db).await?;
        assert!(fresh > legacy);
        Ok(())
    }

    #[tokio::test]
    async fn replacement_is_atomic_and_old_operation_never_claims_review() -> Result<()> {
        let config = crate::test_support::config().await;
        crate::db::add_user(42, &config.db).await?;
        let old = create_independent(42, &proposal(&[], "ordinary"), &config.db).await?;
        let critical = proposal(&[3], "new review");
        let (a, b) = tokio::join!(
            replace_for_review(old, 42, &critical, &config.db),
            replace_for_review(old, 42, &critical, &config.db),
        );
        let review = a?.unwrap();
        assert_eq!(b?, Some(review));
        assert_ne!(review, old);
        assert!(!claim(old, 42, &config.db).await?);
        assert!(claim(review, 42, &config.db).await?);
        assert_eq!(
            replace_for_review(old, 42, &critical, &config.db).await?,
            Some(review)
        );
        assert!(!claim(review, 42, &config.db).await?);
        Ok(())
    }
}
