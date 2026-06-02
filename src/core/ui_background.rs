use crate::db;
use crate::models::{AppConfig, UiBackgroundCache};
use chrono::{Duration, Utc};
use std::sync::Arc;

pub const DEFAULT_UI_BACKGROUND_REFRESH_S: u32 = 15;
pub const UI_BACKGROUND_INTERVALS_S: &[u32] = &[5, 15, 30];
const UI_BACKGROUND_FAILURE_BACKOFF_S: i64 = 30;

pub async fn resolve(config: Arc<AppConfig>, user_id: u64) -> Option<Vec<u8>> {
    let camera_id = db::settings::get_i64(db::settings::UI_BACKGROUND_CAMERA_ID, &config.db)
        .await
        .ok()
        .flatten()?;
    let is_admin = config.root_user == user_id;
    let camera = db::cameras::get_accessible_camera(user_id, is_admin, camera_id, &config.db)
        .await
        .ok()
        .flatten()?;
    let refresh_s = db::settings::get_u32_or(
        db::settings::UI_BACKGROUND_REFRESH_S,
        DEFAULT_UI_BACKGROUND_REFRESH_S,
        &config.db,
    )
    .await
    .clamp(5, 120);

    let now = Utc::now();
    let mut cache = config.ui_background_cache.lock().await;
    if let Some(cache) = cache.as_mut().filter(|cache| cache.camera_id == camera_id) {
        if let (Some(bytes), Some(captured_at)) = (&cache.bytes, cache.captured_at) {
            let fresh_for = Duration::seconds(i64::from(refresh_s));
            if now - captured_at < fresh_for {
                return Some(bytes.clone());
            }
        }

        let bytes = cache.bytes.clone();
        if cache.failed_at.is_some_and(|failed_at| {
            now - failed_at < Duration::seconds(UI_BACKGROUND_FAILURE_BACKOFF_S)
        }) {
            return bytes;
        }

        if !cache.refreshing {
            cache.refreshing = true;
            spawn_refresh(config.clone(), camera);
        }
        return bytes;
    }

    *cache = Some(UiBackgroundCache {
        camera_id,
        captured_at: None,
        failed_at: None,
        bytes: None,
        refreshing: true,
    });
    drop(cache);

    spawn_refresh(config, camera);
    None
}

pub async fn clear(config: &AppConfig) {
    let mut cache = config.ui_background_cache.lock().await;
    *cache = None;
}

fn spawn_refresh(config: Arc<AppConfig>, camera: db::cameras::Camera) {
    tokio::spawn(async move {
        let camera_id = camera.id;
        let result = crate::core::cameras::capture_snapshot(&camera).await;
        let mut cache = config.ui_background_cache.lock().await;

        let Some(cache) = cache.as_mut().filter(|cache| cache.camera_id == camera_id) else {
            return;
        };

        cache.refreshing = false;
        match result {
            Ok(bytes) => {
                cache.bytes = Some(bytes);
                cache.captured_at = Some(Utc::now());
                cache.failed_at = None;
            }
            Err(error) => {
                cache.failed_at = Some(Utc::now());
                log::warn!(
                    "Failed to refresh UI camera background {}: {}",
                    camera_id,
                    error
                );
            }
        }
    });
}

pub async fn selected_camera_id(config: &AppConfig) -> Option<i64> {
    db::settings::get_i64(db::settings::UI_BACKGROUND_CAMERA_ID, &config.db)
        .await
        .ok()
        .flatten()
}

pub async fn refresh_interval_s(config: &AppConfig) -> u32 {
    db::settings::get_u32_or(
        db::settings::UI_BACKGROUND_REFRESH_S,
        DEFAULT_UI_BACKGROUND_REFRESH_S,
        &config.db,
    )
    .await
}

pub fn next_interval(current: u32) -> u32 {
    let index = UI_BACKGROUND_INTERVALS_S
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    UI_BACKGROUND_INTERVALS_S[(index + 1) % UI_BACKGROUND_INTERVALS_S.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_background_interval_cycles_known_values() {
        assert_eq!(next_interval(5), 15);
        assert_eq!(next_interval(15), 30);
        assert_eq!(next_interval(30), 5);
    }

    #[test]
    fn ui_background_interval_recovers_from_unknown_value() {
        assert_eq!(next_interval(120), 15);
    }
}
