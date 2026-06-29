use crate::db;
use crate::models::{AppConfig, CameraSnapshotCache};
use chrono::{Duration, Utc};
use std::sync::Arc;
use std::time::Duration as StdDuration;

const CAMERA_SNAPSHOT_REFRESH_S: i64 = 15;
const CAMERA_SNAPSHOT_FAILURE_BACKOFF_S: i64 = 30;
const CAMERA_SNAPSHOT_WARNING_COOLDOWN_S: i64 = 300;
const CAMERA_SNAPSHOT_INITIAL_WAIT: StdDuration = StdDuration::from_millis(1200);
const CAMERA_SNAPSHOT_WAIT_STEP: StdDuration = StdDuration::from_millis(100);

pub(crate) async fn resolve(
    config: Arc<AppConfig>,
    camera: db::cameras::Camera,
) -> Option<Vec<u8>> {
    let camera_id = camera.id;
    let now = Utc::now();
    let mut cache_by_camera = config.camera_snapshot_cache.lock().await;

    if let Some(cache) = cache_by_camera.get_mut(&camera_id) {
        if let (Some(bytes), Some(captured_at)) = (&cache.bytes, cache.captured_at) {
            if now - captured_at < Duration::seconds(CAMERA_SNAPSHOT_REFRESH_S) {
                return Some(bytes.clone());
            }
        }

        let bytes = cache.bytes.clone();
        if cache.failed_at.is_some_and(|failed_at| {
            now - failed_at < Duration::seconds(CAMERA_SNAPSHOT_FAILURE_BACKOFF_S)
        }) {
            return bytes;
        }

        if !cache.refreshing {
            cache.refreshing = true;
            spawn_refresh(config.clone(), camera);
        }
        if bytes.is_none() {
            drop(cache_by_camera);
            return wait_for_initial_capture(config, camera_id).await;
        }
        return bytes;
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

async fn wait_for_initial_capture(config: Arc<AppConfig>, camera_id: i64) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + CAMERA_SNAPSHOT_INITIAL_WAIT;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return None;
        }

        let sleep_for = (deadline - now).min(CAMERA_SNAPSHOT_WAIT_STEP);
        tokio::time::sleep(sleep_for).await;

        let cache_by_camera = config.camera_snapshot_cache.lock().await;
        let Some(cache) = cache_by_camera.get(&camera_id) else {
            return None;
        };
        if let Some(bytes) = &cache.bytes {
            return Some(bytes.clone());
        }
        if !cache.refreshing {
            return None;
        }
    }
}

fn spawn_refresh(config: Arc<AppConfig>, camera: db::cameras::Camera) {
    tokio::spawn(async move {
        let camera_id = camera.id;
        let camera_name = camera.name.clone();
        let result = crate::core::cameras::capture_snapshot(&camera).await;
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
