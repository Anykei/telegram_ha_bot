use std::sync::Arc;

use crate::models::AppConfig;

struct NoNetworkHa;

#[async_trait::async_trait]
impl crate::ha::HomeAssistantClient for NoNetworkHa {
    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn fetch_rooms(&self) -> anyhow::Result<Vec<crate::ha::Room>> {
        Ok(vec![])
    }
    async fn fetch_history(
        &self,
        _: &str,
        _: u32,
        _: i32,
    ) -> anyhow::Result<crate::ha::client::HistoryResult> {
        anyhow::bail!("HA history was not expected in this test")
    }
    async fn fetch_states_by_ids(
        &self,
        _: &[String],
    ) -> anyhow::Result<Vec<crate::ha::models::Entity>> {
        Ok(vec![])
    }
    async fn fetch_action_entities(&self) -> anyhow::Result<Vec<crate::ha::models::Entity>> {
        Ok(vec![])
    }
    async fn call_service(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        anyhow::bail!("Physical command was not expected")
    }
    async fn call_service_with_data(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: serde_json::Value,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Physical command was not expected")
    }
}

pub async fn config() -> Arc<AppConfig> {
    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
        .await
        .unwrap()
        .run(&db)
        .await
        .unwrap();
    let (camera_recording_tx, _) = tokio::sync::mpsc::channel(32);
    Arc::new(AppConfig {
        ha_client: Arc::new(NoNetworkHa),
        ha_url: "http://unused.invalid".into(),
        ha_token: "unused".into(),
        db,
        root_user: 1,
        delete_notification_messages_timeout_s: 0,
        ttl_notifications: 60,
        background_maintenance_interval_s: 15,
        event_refresh_min_interval_s: 0,
        session_ttl_hours: 24,
        telegram_retry_after_extra_delay_s: 1,
        default_language: crate::i18n::Language::Ru,
        camera_default_clip_s: 10,
        camera_clip_intervals_s: vec![10],
        camera_recording_max_tail_seconds: 300,
        camera_recording_max_segment_seconds: 300,
        camera_recording_max_parallel_jobs: 4,
        camera_recording_storage_root: "unused".into(),
        camera_recording_tx,
        voice_enabled: false,
        voice_stt_provider: Default::default(),
        voice_command_engine: Default::default(),
        voice_ha_pipeline_id: None,
        voice_stt_sample_rate: 16000,
        voice_confirm_dangerous: true,
        voice_pending_ttl_s: 60,
        voice_max_audio_size_mb: 10,
        voice_max_audio_duration_s: 30,
        voice_stt_timeout_s: 30,
        voice_show_recognized_text: true,
        voice_response_format: Default::default(),
        sessions: Default::default(),
        ui_locks: Default::default(),
        admission_lifecycle: Default::default(),
        dialogue_generations: Default::default(),
        active_dialogues: Default::default(),
        view_updates: Default::default(),
        climate_controls: Default::default(),
        recording_sends_in_progress: Default::default(),
        name_aliases: Default::default(),
        state_aliases: Default::default(),
        ui_background_cache: Default::default(),
        camera_snapshot_cache: Default::default(),
        camera_pre_roll_registry: Arc::new(
            crate::core::camera_pre_roll::CameraPreRollRegistry::new(),
        ),
        runtime_status: Default::default(),
        notification_delivery: Default::default(),
    })
}
