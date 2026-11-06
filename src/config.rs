use anyhow::{ensure, Result};
use log::{info, warn};
use std::env;
use std::path::PathBuf;

pub struct EnvPaths {
    pub options: PathBuf,
    pub database: PathBuf,
    pub migrations: PathBuf,
    pub ha_url: String,
    pub ha_token: String,
}

impl EnvPaths {
    /// Mock runs cannot inherit real HA credentials, inventory, cameras or schedules.
    pub fn with_mock_ha(mut self) -> Result<Self> {
        let mut name = self
            .database
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("DATABASE_PATH must name a database file"))?
            .to_os_string();
        name.push(".mock.sqlite3");
        self.database.set_file_name(name);
        self.ha_url = "mock://home-assistant".into();
        self.ha_token.clear();
        Ok(self)
    }

    pub fn load() -> Self {
        dotenvy::dotenv().ok();

        Self {
            options: env::var("OPTIONS_PATH")
                .unwrap_or_else(|_| "options.json".to_string())
                .into(),

            database: env::var("DATABASE_PATH")
                .unwrap_or_else(|_| "bot_data.db".to_string())
                .into(),

            migrations: env::var("MIGRATIONS_PATH")
                .unwrap_or_else(|_| "./migrations".to_string())
                .into(),

            ha_url: env::var("HA_URL").unwrap_or_else(|_| "http://supervisor/core".to_string()),

            ha_token: env::var("SUPERVISOR_TOKEN")
                .unwrap_or(env::var("HA_TOKEN").unwrap_or_default()),
        }
    }

    pub fn validate(self) -> Result<Self> {
        info!("--- Checking env variables ---");
        info!("📄 Options: {:?}", self.options);
        info!("🗄 Database: {:?}", self.database);
        info!("🛠 Migration: {:?}", self.migrations);
        info!("🔗 HA URL: {}", self.ha_url);

        ensure!(
            self.ha_url == "mock://home-assistant" || !self.ha_token.is_empty(),
            "Critical Error: HA_TOKEN not set!"
        );

        if !self.migrations.exists() {
            warn!("⚠️ Folder migration not found {:?}", self.migrations);
        }

        Ok(self)
    }

    pub fn db_url(&self) -> String {
        format!("sqlite://{}", self.database.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> EnvPaths {
        EnvPaths {
            options: "data/options.json".into(),
            database: "data/bot_data.db".into(),
            migrations: "migrations".into(),
            ha_url: "https://real-home.invalid".into(),
            ha_token: "secret".into(),
        }
    }

    #[test]
    fn mock_isolates_storage_and_removes_real_endpoint_and_credentials() -> Result<()> {
        let mock = paths().with_mock_ha()?.validate()?;
        assert_eq!(
            mock.database,
            PathBuf::from("data/bot_data.db.mock.sqlite3")
        );
        assert_eq!(mock.ha_url, "mock://home-assistant");
        assert!(mock.ha_token.is_empty());
        assert_eq!(mock.options, paths().options);
        let mut live = paths();
        live.ha_token.clear();
        assert!(live.validate().is_err());
        let live = paths().validate()?;
        assert_eq!(live.database, paths().database);
        assert_eq!(live.ha_url, paths().ha_url);
        Ok(())
    }
}
