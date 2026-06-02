use std::collections::HashSet;
use std::sync::Arc;

use crate::ha::HomeAssistantClient;
use crate::i18n::Language;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Deserialize;
use tokio::sync::{mpsc, Mutex, RwLock};

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
    pub default_language: Language,
    pub camera_default_clip_s: u32,
    pub camera_clip_intervals_s: Vec<u32>,
    pub camera_recording_max_tail_seconds: u32,
    pub camera_recording_max_segment_seconds: u32,
    pub camera_recording_max_parallel_jobs: usize,
    pub camera_recording_storage_root: String,
    pub camera_recording_tx: mpsc::Sender<crate::core::camera_recording::RecordingJob>,

    pub sessions: DashMap<u64, UserSession>,
    pub ui_locks: DashMap<u64, Arc<Mutex<()>>>,
    pub recording_sends_in_progress: DashMap<(u64, i64), DateTime<Utc>>,

    pub name_aliases: DashMap<String, String>,
    pub state_aliases: DashMap<String, std::collections::HashMap<String, String>>,
    pub ui_background_cache: Mutex<Option<UiBackgroundCache>>,
    pub runtime_status: RwLock<RuntimeStatus>,
}

impl AppConfig {
    pub fn ui_lock_for(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.ui_locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub fn start_recording_send(&self, user_id: u64, session_id: i64) -> bool {
        use dashmap::mapref::entry::Entry;

        match self
            .recording_sends_in_progress
            .entry((user_id, session_id))
        {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(Utc::now());
                true
            }
        }
    }

    pub fn finish_recording_send(&self, user_id: u64, session_id: i64) {
        self.recording_sends_in_progress
            .remove(&(user_id, session_id));
    }

    pub fn is_recording_send_in_progress(&self, user_id: u64, session_id: i64) -> bool {
        self.recording_sends_in_progress
            .contains_key(&(user_id, session_id))
    }

    pub fn state_alias_for_display(
        &self,
        entity_id: &str,
        state: &str,
        inverted: bool,
    ) -> Option<String> {
        let aliases = self.state_aliases.get(entity_id)?;

        aliases.get(state).cloned().or_else(|| {
            let logical_state =
                crate::core::presentation::StateFormatter::logical_state(state, inverted);
            aliases.get(&logical_state).cloned()
        })
    }

    pub fn set_state_alias_cache(&self, entity_id: &str, state: &str, alias: String) {
        let mut aliases = self.state_aliases.entry(entity_id.to_string()).or_default();
        aliases.insert(state.to_string(), alias);
    }

    pub fn remove_state_alias_cache(&self, entity_id: &str, state: &str) {
        if let Some(mut aliases) = self.state_aliases.get_mut(entity_id) {
            aliases.remove(state);
            if aliases.is_empty() {
                drop(aliases);
                self.state_aliases.remove(entity_id);
            }
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct NotificationData {
    pub human_state: String,
    pub recipients: Vec<i64>,
}

#[derive(Debug, Clone)]
pub struct UiBackgroundCache {
    pub camera_id: i64,
    pub captured_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub bytes: Option<Vec<u8>>,
    pub refreshing: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeStatus {
    pub last_maintenance_tick_at: Option<DateTime<Utc>>,
    pub last_ha_sync_at: Option<DateTime<Utc>>,
    pub last_ha_sync_error: Option<String>,
}
