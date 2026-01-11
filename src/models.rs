use std::sync::Arc;
use crate::ha::HAClient;

pub struct AppConfig {
    pub ha_client: Arc<HAClient>,
    pub db: sqlx::SqlitePool,
    pub root_user: u64,

    pub delete_chart_timeout_s: u64,
    pub delete_help_messages_timeout_s: u64,
    pub delete_error_messages_timeout_s: u64,
}