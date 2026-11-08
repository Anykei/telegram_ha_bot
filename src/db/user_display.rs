use crate::i18n::Language;
use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct UserDisplayMetadata {
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub admin_alias: Option<String>,
}

impl UserDisplayMetadata {
    pub fn name(&self, user_id: u64, lang: Language) -> String {
        self.admin_alias
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                self.display_name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
            })
            .map(str::to_string)
            .unwrap_or_else(|| match lang {
                Language::Ru => format!("Пользователь #{user_id}"),
                Language::En => format!("User #{user_id}"),
            })
    }
}

pub async fn get(user_id: u64, pool: &SqlitePool) -> Result<UserDisplayMetadata> {
    Ok(sqlx::query_as(
        "SELECT display_name, username, admin_alias FROM user_display_metadata WHERE user_id = ?",
    )
    .bind(i64::try_from(user_id)?)
    .fetch_optional(pool)
    .await?
    .unwrap_or_default())
}

pub async fn capture(
    user_id: u64,
    first_name: &str,
    last_name: Option<&str>,
    username: Option<&str>,
    pool: &SqlitePool,
) -> Result<()> {
    let name = format!("{} {}", first_name, last_name.unwrap_or(""))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    sqlx::query("INSERT INTO user_display_metadata (user_id, display_name, username) VALUES (?, ?, ?) ON CONFLICT(user_id) DO UPDATE SET display_name = excluded.display_name, username = excluded.username WHERE display_name IS NOT excluded.display_name OR username IS NOT excluded.username")
        .bind(i64::try_from(user_id)?).bind(name).bind(username).execute(pool).await?;
    Ok(())
}

pub async fn set_alias(user_id: u64, alias: Option<&str>, pool: &SqlitePool) -> Result<()> {
    let alias = alias
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty());
    anyhow::ensure!(
        alias
            .as_ref()
            .is_none_or(|value| value.chars().count() <= 80),
        "Alias must be at most 80 characters"
    );
    sqlx::query("INSERT INTO user_display_metadata (user_id, admin_alias) VALUES (?, ?) ON CONFLICT(user_id) DO UPDATE SET admin_alias = excluded.admin_alias")
        .bind(i64::try_from(user_id)?).bind(alias).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn telegram_updates_preserve_admin_alias_and_never_grant_access() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        assert_eq!(
            get(42, &pool).await?.name(42, Language::Ru),
            "Пользователь #42"
        );
        capture(42, "Иван", Some("Петров"), Some("ivan"), &pool).await?;
        assert_eq!(get(42, &pool).await?.name(42, Language::Ru), "Иван Петров");
        set_alias(42, Some("  Домашний\nадминистратор  "), &pool).await?;
        capture(42, "Другое имя", None, None, &pool).await?;
        let metadata = get(42, &pool).await?;
        assert_eq!(metadata.name(42, Language::Ru), "Домашний администратор");
        assert!(metadata.username.is_none());
        assert!(!crate::db::user_exists(42, &pool).await);
        set_alias(42, None, &pool).await?;
        assert_eq!(get(42, &pool).await?.name(42, Language::Ru), "Другое имя");
        assert_eq!(get(43, &pool).await?.name(43, Language::En), "User #43");
        assert!(set_alias(42, Some(&"界".repeat(81)), &pool).await.is_err());
        Ok(())
    }
}
