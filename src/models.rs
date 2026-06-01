use std::collections::HashSet;
use std::sync::Arc;

use crate::ha::HomeAssistantClient;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

pub struct UserSession {
    pub last_menu_id: i32,
    pub current_context: String,
    pub header_entities: HashSet<String>,
    pub last_ui_refresh_at: Option<DateTime<Utc>>,
    pub ui_refresh_blocked_until: Option<DateTime<Utc>>,
    pub last_seen_at: DateTime<Utc>,
}

impl UserSession {
    pub fn is_ui_refresh_blocked(&self, now: DateTime<Utc>) -> bool {
        self.ui_refresh_blocked_until
            .map(|blocked_until| blocked_until > now)
            .unwrap_or(false)
    }
}

pub struct AppConfig {
    pub ha_client: Arc<dyn HomeAssistantClient>,
    pub db: sqlx::SqlitePool,
    pub root_user: u64,

    // pub delete_chart_timeout_s: u64,
    // pub delete_help_messages_timeout_s: u64,
    pub delete_notification_messages_timeout_s: u64,
    // pub delete_error_messages_timeout_s: u64,
    pub ttl_notifications: u64,
    pub background_maintenance_interval_s: u64,
    pub event_refresh_min_interval_s: u64,
    pub session_ttl_hours: u64,
    pub telegram_retry_after_extra_delay_s: u64,
    pub camera_default_clip_s: u32,
    pub camera_clip_intervals_s: Vec<u32>,

    pub sessions: DashMap<u64, UserSession>,
    pub ui_locks: DashMap<u64, Arc<Mutex<()>>>,

    pub name_aliases: DashMap<String, String>,
    pub state_aliases: DashMap<String, std::collections::HashMap<String, String>>,
    pub runtime_status: RwLock<RuntimeStatus>,
}

impl AppConfig {
    pub fn ui_lock_for(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.ui_locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct NotificationData {
    pub human_state: String,
    pub recipients: Vec<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeStatus {
    pub last_maintenance_tick_at: Option<DateTime<Utc>>,
    pub last_ha_sync_at: Option<DateTime<Utc>>,
    pub last_ha_sync_error: Option<String>,
}
