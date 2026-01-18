use std::collections::HashSet;
use std::sync::Arc;
use crate::ha::HAClient;
use dashmap::DashMap;
use serde::Deserialize;


pub struct UserSession {
    pub last_menu_id: i32,
    pub current_context: String,
    pub header_entities: HashSet<String>,
}

pub struct AppConfig {
    pub ha_client: Arc<HAClient>,
    pub db: sqlx::SqlitePool,
    pub root_user: u64,

    pub delete_chart_timeout_s: u64,
    pub delete_help_messages_timeout_s: u64,
    pub delete_notification_messages_timeout_s: u64,
    pub delete_error_messages_timeout_s: u64,
    pub leak_time_notification_m:u64,
    pub background_maintenance_interval_s:u64,

    pub sessions: DashMap<u64, UserSession>,

    pub name_aliases: DashMap<String, String>,
    pub state_aliases: DashMap<String, std::collections::HashMap<String, String>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct NotificationData {
    pub entity_id: String,
    pub display_name: String,
    pub human_state: String,
    pub recipients: Vec<i64>,
}