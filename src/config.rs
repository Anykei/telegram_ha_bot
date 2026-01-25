use std::env;
use std::path::PathBuf;
use anyhow::{Result, ensure};
use log::{info, warn};

pub struct EnvPaths {
    pub options: PathBuf,
    pub database: PathBuf,
    pub migrations: PathBuf,
    pub ha_url: String,
    pub ha_token: String,
}

impl EnvPaths {
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

            ha_url: env::var("HA_URL")
                .unwrap_or_else(|_| "http://supervisor/core".to_string()),

            ha_token: env::var("SUPERVISOR_TOKEN").unwrap_or_default(),
        }
    }

    pub fn validate(self) -> Result<Self> {
        info!("--- Checking env variables ---");
        info!("📄 Options: {:?}", self.options);
        info!("🗄 Database: {:?}", self.database);
        info!("🛠 Migration: {:?}", self.migrations);
        info!("🔗 HA URL: {}", self.ha_url);

        ensure!(
                !self.ha_token.is_empty(),
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