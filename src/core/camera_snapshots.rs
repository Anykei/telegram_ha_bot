use crate::db;
use crate::models::{AppConfig, CameraSnapshotCache};
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use std::time::Duration as StdDuration;

const CAMERA_SNAPSHOT_REFRESH_S: i64 = 15;
const CAMERA_SNAPSHOT_FAILURE_BACKOFF_S: i64 = 30;
const CAMERA_SNAPSHOT_WARNING_COOLDOWN_S: i64 = 300;
const CAMERA_SNAPSHOT_INITIAL_WAIT: StdDuration = StdDuration::from_millis(1200);
const CAMERA_SNAPSHOT_WAIT_STEP: StdDuration = StdDuration::from_millis(100);

#[derive(Default)]
pub(crate) struct SnapshotPreview {
    pub bytes: Option<Vec<u8>>,
    pub received_at: Option<DateTime<Utc>>,
    pub refreshing: bool,
    pub failed: bool,
}

impl SnapshotPreview {
    fn from_cache(cache: &CameraSnapshotCache) -> Self {
        Self {
            bytes: cache.bytes.clone(),
            received_at: cache.captured_at,
            refreshing: cache.refreshing,
            failed: cache.failed_at.is_some(),
        }
    }

    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        self.received_at
            .is_some_and(|at| now - at >= Duration::seconds(CAMERA_SNAPSHOT_REFRESH_S))
    }
}

pub(crate) async fn resolve(
    config: Arc<AppConfig>,
    camera: db::cameras::Camera,
) -> SnapshotPreview {
    let camera_id = camera.id;
    let now = Utc::now();
    let mut cache_by_camera = config.camera_snapshot_cache.lock().await;

    if let Some(cache) = cache_by_camera.get_mut(&camera_id) {
        if let (Some(_), Some(captured_at)) = (&cache.bytes, cache.captured_at) {
            if now - captured_at < Duration::seconds(CAMERA_SNAPSHOT_REFRESH_S) {
                return SnapshotPreview::from_cache(cache);
            }
        }

        if cache.failed_at.is_some_and(|failed_at| {
            now - failed_at < Duration::seconds(CAMERA_SNAPSHOT_FAILURE_BACKOFF_S)
        }) {
            return SnapshotPreview::from_cache(cache);
        }

        if !cache.refreshing {
            cache.refreshing = true;
            spawn_refresh(config.clone(), camera);
        }
        if cache.bytes.is_none() {
            drop(cache_by_camera);
            return wait_for_initial_capture(config, camera_id).await;
        }
        return SnapshotPreview::from_cache(cache);
    }

    cache_by_camera.insert(
        camera_id,
        CameraSnapshotCache {
            captured_at: None,
            failed_at: None,
            last_warned_at: None,
            last_error: None,
            bytes: None,
            refreshing: true,
        },
    );
    drop(cache_by_camera);

    spawn_refresh(config.clone(), camera);
    wait_for_initial_capture(config, camera_id).await
}

async fn wait_for_initial_capture(config: Arc<AppConfig>, camera_id: i64) -> SnapshotPreview {
    let deadline = tokio::time::Instant::now() + CAMERA_SNAPSHOT_INITIAL_WAIT;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            let cache_by_camera = config.camera_snapshot_cache.lock().await;
            return cache_by_camera
                .get(&camera_id)
                .map(SnapshotPreview::from_cache)
                .unwrap_or_default();
        }

        let sleep_for = (deadline - now).min(CAMERA_SNAPSHOT_WAIT_STEP);
        tokio::time::sleep(sleep_for).await;

        let cache_by_camera = config.camera_snapshot_cache.lock().await;
        let Some(cache) = cache_by_camera.get(&camera_id) else {
            return SnapshotPreview::default();
        };
        if cache.bytes.is_some() || !cache.refreshing {
            return SnapshotPreview::from_cache(cache);
        }
    }
}

fn spawn_refresh(config: Arc<AppConfig>, camera: db::cameras::Camera) {
    tokio::spawn(async move {
        let camera_id = camera.id;
        let camera_name = camera.name.clone();
        let result = match pre_roll_snapshot(config.clone(), camera_id).await {
            Some(bytes) => Ok(bytes),
            None => crate::core::cameras::capture_snapshot(&camera).await,
        };
        let mut cache_by_camera = config.camera_snapshot_cache.lock().await;

        let Some(cache) = cache_by_camera.get_mut(&camera_id) else {
            return;
        };

        cache.refreshing = false;
        match result {
            Ok(bytes) if !bytes.is_empty() => {
                cache.bytes = Some(bytes);
                cache.captured_at = Some(Utc::now());
                cache.failed_at = None;
                cache.last_error = None;
            }
            Ok(_) => {
                mark_refresh_failed(
                    cache,
                    camera_id,
                    &camera_name,
                    "Snapshot URL вернул пустой файл",
                );
            }
            Err(error) => {
                mark_refresh_failed(
                    cache,
                    camera_id,
                    &camera_name,
                    &crate::core::cameras::sanitize_camera_error(&error),
                );
            }
        }
    });
}

async fn pre_roll_snapshot(config: Arc<AppConfig>, camera_id: i64) -> Option<Vec<u8>> {
    if !config.camera_pre_roll_registry.has_ready_buffer(camera_id) {
        return None;
    }

    let now = Utc::now();
    let tmp_path = std::env::temp_dir().join(format!(
        "telegram_ha_bot_preroll_snapshot_{}_{}.mp4",
        camera_id,
        now.timestamp_millis()
    ));
    let write_result = config
        .camera_pre_roll_registry
        .write_to_file(camera_id, 3, now, now, tmp_path.clone())
        .await;

    let bytes = match write_result {
        Ok(_) => match crate::core::cameras::extract_video_preview(tmp_path.clone()).await {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            Ok(_) => None,
            Err(error) => {
                log::debug!(
                    "Camera pre-roll snapshot preview failed: camera={}, error={:#}",
                    camera_id,
                    error
                );
                None
            }
        },
        Err(error) => {
            log::debug!(
                "Camera pre-roll snapshot unavailable: camera={}, error={:#}",
                camera_id,
                error
            );
            None
        }
    };

    let _ = tokio::fs::remove_file(&tmp_path).await;
    bytes
}

fn mark_refresh_failed(
    cache: &mut CameraSnapshotCache,
    camera_id: i64,
    camera_name: &str,
    error: &str,
) {
    let now = Utc::now();
    let repeated = cache.last_error.as_deref() == Some(error);
    let warning_cooled_down = cache.last_warned_at.is_none_or(|warned_at| {
        now - warned_at >= Duration::seconds(CAMERA_SNAPSHOT_WARNING_COOLDOWN_S)
    });
    cache.failed_at = Some(now);
    cache.last_error = Some(error.to_string());

    if repeated && !warning_cooled_down {
        log::debug!(
            "Camera detail preview repeated error: camera={} {}, error={}",
            camera_id,
            camera_name,
            error
        );
    } else {
        cache.last_warned_at = Some(now);
        log::warn!(
            "Camera detail preview unavailable: camera={} {}, error={}",
            camera_id,
            camera_name,
            error
        );
    }
}
