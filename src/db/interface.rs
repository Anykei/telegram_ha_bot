//! Presentation preference belongs to the admitted user, never to a device action.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum InterfaceMode {
    #[default]
    Classic,
    Html,
}

pub async fn get(user: u64, pool: &SqlitePool) -> Result<InterfaceMode> {
    let value: Option<String> = sqlx::query_scalar("SELECT interface_mode FROM users WHERE id = ?")
        .bind(user as i64)
        .fetch_optional(pool)
        .await?;
    Ok(if value.as_deref() == Some("html") {
        InterfaceMode::Html
    } else {
        InterfaceMode::Classic
    })
}

pub async fn set(user: u64, mode: InterfaceMode, pool: &SqlitePool) -> Result<()> {
    let generation = super::user::admission_generation(user, pool)
        .await?
        .context("User admission revoked")?;
    let updated = sqlx::query(
        "UPDATE users SET interface_mode = ? WHERE id = ? AND admission_generation = ?",
    )
    .bind(if mode == InterfaceMode::Html {
        "html"
    } else {
        "classic"
    })
    .bind(user as i64)
    .bind(generation)
    .execute(pool)
    .await?;
    ensure!(updated.rows_affected() == 1, "User admission revoked");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn preferences_are_personal_and_old_admissions_cannot_restore_them() -> Result<()> {
        let config = crate::test_support::config().await;
        super::super::add_user(7, &config.db).await?;
        super::super::add_user(8, &config.db).await?;
        assert_eq!(get(7, &config.db).await?, InterfaceMode::Classic);
        set(7, InterfaceMode::Html, &config.db).await?;
        assert_eq!(get(7, &config.db).await?, InterfaceMode::Html);
        assert_eq!(get(8, &config.db).await?, InterfaceMode::Classic);
        let admission = crate::db::user::Admission {
            user_id: 7,
            generation: crate::db::admission_generation(7, &config.db)
                .await?
                .unwrap(),
        };
        crate::db::delete_user(7, &config.db).await?;
        crate::db::add_user(7, &config.db).await?;
        assert!(crate::db::user::with_admission(
            admission,
            set(7, InterfaceMode::Html, &config.db)
        )
        .await
        .is_err());
        assert_eq!(get(7, &config.db).await?, InterfaceMode::Classic);
        Ok(())
    }
}
