use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::Utc;
use log::{error, info, warn};
use teloxide::prelude::*;
use teloxide::types::ChatId;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::models::AppConfig;

const DEFAULT_HEALTH_CHECK_ENABLED: i64 = 1;
const DEFAULT_HEALTH_CHECK_INTERVAL_S: u64 = 10;
const DEFAULT_HEALTH_CHECK_BATCH_SIZE: usize = 1;
const MAX_HEALTH_CHECK_BATCH_SIZE: usize = 16;

static CAMERA_HEALTH_CURSOR: AtomicUsize = AtomicUsize::new(0);

pub fn spawn_camera_health_worker(
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    info!("Core: Camera health worker started");
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("Core: Camera health worker stopped");
                    break;
                }
                _ = sleep(next_interval(&config).await) => {
                    if health_checks_enabled(&config).await {
                        check_all_cameras(&config).await;
                    }
                    send_pending_alerts(&bot, &config).await;
                }
            }
        }
    })
}

async fn next_interval(config: &Arc<AppConfig>) -> Duration {
    let seconds = db::settings::get_i64(db::settings::CAMERA_HEALTH_CHECK_INTERVAL_S, &config.db)
        .await
        .ok()
        .flatten()
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(DEFAULT_HEALTH_CHECK_INTERVAL_S)
        .max(1);

    Duration::from_secs(seconds)
}

async fn health_checks_enabled(config: &Arc<AppConfig>) -> bool {
    db::settings::get_i64(db::settings::CAMERA_HEALTH_CHECK_ENABLED, &config.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(DEFAULT_HEALTH_CHECK_ENABLED)
        != 0
}

async fn health_check_batch_size(config: &Arc<AppConfig>) -> usize {
    db::settings::get_i64(db::settings::CAMERA_HEALTH_CHECK_BATCH_SIZE, &config.db)
        .await
        .ok()
        .flatten()
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_HEALTH_CHECK_BATCH_SIZE)
        .clamp(1, MAX_HEALTH_CHECK_BATCH_SIZE)
}

async fn check_all_cameras(config: &Arc<AppConfig>) {
    let cameras = match db::cameras::list_enabled_cameras(&config.db).await {
        Ok(cameras) => cameras,
        Err(error) => {
            error!("Camera health: failed to list cameras: {}", error);
            return;
        }
    };

    if cameras.is_empty() {
        return;
    }

    let batch_size = health_check_batch_size(config).await.min(cameras.len());
    let start = CAMERA_HEALTH_CURSOR.fetch_add(batch_size, Ordering::Relaxed);

    for offset in 0..batch_size {
        let camera = &cameras[(start + offset) % cameras.len()];
        match crate::core::cameras::capture_snapshot(camera).await {
            Ok(bytes) if !bytes.is_empty() => {
                if let Err(error) =
                    db::camera_health::mark_check_ok(camera.id, bytes.len() as i64, &config.db)
                        .await
                {
                    error!(
                        "Camera health: failed to mark camera {} ok: {}",
                        camera.id, error
                    );
                }
            }
            Ok(_) => {
                if let Err(error) = db::camera_health::mark_error_kind(
                    camera.id,
                    db::camera_health::CameraHealthOperation::HealthCheck,
                    "empty snapshot",
                    &config.db,
                )
                .await
                {
                    error!(
                        "Camera health: failed to mark camera {} empty snapshot: {}",
                        camera.id, error
                    );
                }
            }
            Err(error) if crate::core::cameras::is_capacity_error(&error) => {
                log::debug!(
                    "Camera health probe deferred: native capacity busy, camera={}",
                    camera.id
                );
            }
            Err(error) => {
                let message = crate::core::cameras::sanitize_camera_error(&error);
                if let Err(error) = db::camera_health::mark_error_kind(
                    camera.id,
                    db::camera_health::CameraHealthOperation::HealthCheck,
                    &message,
                    &config.db,
                )
                .await
                {
                    error!(
                        "Camera health: failed to mark camera {} error: {}",
                        camera.id, error
                    );
                }
            }
        }
    }
}

async fn send_pending_alerts(bot: &Bot, config: &Arc<AppConfig>) {
    let alerts = match db::camera_health::list_pending_alerts(&config.db).await {
        Ok(alerts) => alerts,
        Err(error) => {
            error!("Camera health: failed to list pending alerts: {}", error);
            return;
        }
    };

    if alerts.is_empty() {
        return;
    }

    let recipients = match db::list_admin_users(config.root_user, &config.db).await {
        Ok(recipients) => recipients,
        Err(error) => {
            error!("Camera health: failed to list alert recipients: {}", error);
            return;
        }
    };

    for alert in alerts {
        let camera = match db::cameras::get_camera(alert.camera_id, &config.db).await {
            Ok(camera) => camera,
            Err(error) => {
                error!(
                    "Camera health: failed to load camera {} for alert: {}",
                    alert.camera_id, error
                );
                None
            }
        };
        let camera_name = camera
            .as_ref()
            .map(|camera| camera.name.as_str())
            .unwrap_or("unknown");
        let message = format_alert(camera_name, &alert);
        let alert_kind = alert.alert_kind();
        let Some(state_changed_at) = alert.state_changed_at() else {
            warn!(
                "Camera health: alert for camera {} has no state timestamp",
                alert.camera_id
            );
            continue;
        };
        let mut all_sent = true;

        for recipient in &recipients {
            let recipient_id = *recipient as i64;
            match db::camera_health::alert_delivery_sent(
                alert.camera_id,
                alert_kind,
                recipient_id,
                state_changed_at,
                &config.db,
            )
            .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    all_sent = false;
                    warn!(
                        "Camera health: failed to read alert delivery for camera {}, recipient {}: {}",
                        alert.camera_id, recipient, error
                    );
                    continue;
                }
            }

            let status = match bot
                .send_message(ChatId(recipient_id), message.clone())
                .await
            {
                Ok(sent) => {
                    crate::bot::utils::spawn_delayed_delete(
                        bot.clone(),
                        ChatId(*recipient as i64),
                        sent.id,
                        config.delete_notification_messages_timeout_s,
                    );
                    if let Err(error) = db::camera_health::mark_alert_delivery_sent(
                        alert.camera_id,
                        alert_kind,
                        recipient_id,
                        state_changed_at,
                        &config.db,
                    )
                    .await
                    {
                        all_sent = false;
                        warn!(
                            "Camera health: failed to mark alert delivery for camera {}, recipient {}: {}",
                            alert.camera_id, recipient, error
                        );
                    }
                    "ok"
                }
                Err(error) => {
                    all_sent = false;
                    let error_text = error.to_string();
                    if let Err(delivery_error) = db::camera_health::mark_alert_delivery_error(
                        alert.camera_id,
                        alert_kind,
                        recipient_id,
                        state_changed_at,
                        &error_text,
                        &config.db,
                    )
                    .await
                    {
                        warn!(
                            "Camera health: failed to mark alert delivery error for camera {}, recipient {}: {}",
                            alert.camera_id, recipient, delivery_error
                        );
                    }
                    warn!(
                        "Camera health: failed to send alert to {}: {}",
                        recipient, error
                    );
                    "error"
                }
            };

            let entity_id = alert.camera_id.to_string();
            let action = if alert.health_state == db::camera_health::HEALTH_STATE_DEGRADED {
                "degraded_alert"
            } else {
                "recovery_alert"
            };
            let _ = db::activity_log::log(
                db::activity_log::NewActivity {
                    user_id: Some(*recipient),
                    kind: "camera",
                    entity_type: "camera",
                    entity_id: Some(&entity_id),
                    action,
                    status,
                    message: Some(&message),
                },
                &config.db,
            )
            .await;
        }

        if !all_sent {
            warn!(
                "Camera health: alert for camera {} stays pending because at least one recipient failed",
                alert.camera_id
            );
            continue;
        }

        let result = if alert.health_state == db::camera_health::HEALTH_STATE_DEGRADED {
            db::camera_health::mark_degradation_alert_sent(
                alert.camera_id,
                state_changed_at,
                &config.db,
            )
            .await
        } else {
            db::camera_health::mark_recovery_alert_sent(
                alert.camera_id,
                state_changed_at,
                &config.db,
            )
            .await
        };
        if let Err(error) = result {
            error!(
                "Camera health: failed to mark alert sent for camera {}: {}",
                alert.camera_id, error
            );
        }
    }
}

fn format_alert(camera_name: &str, alert: &db::camera_health::PendingHealthAlert) -> String {
    if alert.health_state == db::camera_health::HEALTH_STATE_DEGRADED {
        format!(
            "⚠️ Камера недоступна\nКамера: {}\nОшибка: {}\nТип: {}\nОшибок подряд: {}",
            camera_name,
            alert.last_error.as_deref().unwrap_or("unknown"),
            alert.last_failure_kind.as_deref().unwrap_or("health_check"),
            alert.consecutive_failures
        )
    } else {
        let recovered_at = alert
            .recovered_at
            .map(crate::bot::format::datetime)
            .unwrap_or_else(|| crate::bot::format::datetime(Utc::now()));
        format!(
            "✅ Камера восстановилась\nКамера: {}\nПоследняя успешная проверка: {}",
            camera_name, recovered_at
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delayed_telegram_ack_does_not_acknowledge_a_new_health_transition(
    ) -> anyhow::Result<()> {
        use anyhow::Context;
        use serde_json::json;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for recovery in [false, true] {
            let config = crate::test_support::config().await;
            db::rooms::sync_rooms_from_ha("health", "Health", &config.db).await?;
            let camera_id = db::cameras::add_manual_camera(
                db::cameras::NewCamera {
                    name: "Health barrier camera",
                    room_id: 1,
                    stream_url: "rtsp://unused",
                    snapshot_url: None,
                    clip_seconds: 5,
                },
                &config.db,
            )
            .await?;
            db::settings::set_i64(db::settings::CAMERA_HEALTH_FAILURE_THRESHOLD, 1, &config.db)
                .await?;
            db::settings::set_i64(
                db::settings::CAMERA_HEALTH_RECOVERY_SUCCESSES,
                1,
                &config.db,
            )
            .await?;
            db::camera_health::mark_error(camera_id, "first incident", &config.db).await?;
            if recovery {
                db::camera_health::mark_check_ok(camera_id, 100, &config.db).await?;
            }
            let old = db::camera_health::list_pending_alerts(&config.db)
                .await?
                .remove(0);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let url = format!("http://{}", listener.local_addr()?).parse()?;
            let client = teloxide::net::default_reqwest_settings()
                .no_proxy()
                .build()?;
            let bot = Bot::with_client("123:test", client).set_api_url(url);
            let (requests_tx, mut requests_rx) = tokio::sync::mpsc::channel(2);
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let mut release = Some(release_rx);
                for _ in 0..2 {
                    let (mut socket, _) = listener.accept().await?;
                    let mut bytes = Vec::new();
                    let body_start = loop {
                        let mut chunk = [0; 4096];
                        let size = socket.read(&mut chunk).await?;
                        anyhow::ensure!(size > 0, "Incomplete Telegram mock request");
                        bytes.extend_from_slice(&chunk[..size]);
                        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                            break index + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&bytes[..body_start]);
                    assert!(headers
                        .lines()
                        .next()
                        .unwrap()
                        .to_lowercase()
                        .contains("sendmessage"));
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .context("Missing request length")?;
                    while bytes.len() < body_start + length {
                        let mut chunk = [0; 4096];
                        let size = socket.read(&mut chunk).await?;
                        anyhow::ensure!(size > 0, "Incomplete Telegram mock body");
                        bytes.extend_from_slice(&chunk[..size]);
                    }
                    let body: serde_json::Value =
                        serde_json::from_slice(&bytes[body_start..body_start + length])?;
                    requests_tx.send(body).await?;
                    if let Some(release) = release.take() {
                        release.await?;
                    }
                    let response = json!({"ok":true,"result":{"message_id":7,"date":1_700_000_000,"chat":{"id":1,"type":"private"},"text":"ok"}}).to_string();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await?;
                }
                Ok::<_, anyhow::Error>(())
            });
            let pending_bot = bot.clone();
            let pending_config = config.clone();
            let pending = tokio::spawn(async move {
                send_pending_alerts(&pending_bot, &pending_config).await;
            });
            let request = tokio::time::timeout(Duration::from_secs(5), requests_rx.recv())
                .await?
                .unwrap();
            assert!(request["text"].as_str().unwrap().contains(if recovery {
                "восстановилась"
            } else {
                "недоступна"
            }));

            if recovery {
                db::camera_health::mark_error(camera_id, "second incident", &config.db).await?;
                db::camera_health::mark_check_ok(camera_id, 200, &config.db).await?;
            } else {
                db::camera_health::mark_check_ok(camera_id, 200, &config.db).await?;
                db::camera_health::mark_error(camera_id, "second incident", &config.db).await?;
            }
            let new = db::camera_health::list_pending_alerts(&config.db)
                .await?
                .remove(0);
            assert_eq!(new.alert_kind(), old.alert_kind());
            assert!(new.state_changed_at() > old.state_changed_at());
            release_tx.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), pending).await??;

            let pending_alerts = db::camera_health::list_pending_alerts(&config.db).await?;
            assert_eq!(
                pending_alerts.len(),
                1,
                "Delayed ack consumed the new transition"
            );
            assert_eq!(pending_alerts[0].state_changed_at(), new.state_changed_at());
            assert!(
                db::camera_health::alert_delivery_sent(
                    camera_id,
                    old.alert_kind(),
                    1,
                    old.state_changed_at().unwrap(),
                    &config.db
                )
                .await?
            );
            assert!(
                !db::camera_health::alert_delivery_sent(
                    camera_id,
                    new.alert_kind(),
                    1,
                    new.state_changed_at().unwrap(),
                    &config.db
                )
                .await?
            );

            tokio::time::timeout(Duration::from_secs(5), send_pending_alerts(&bot, &config))
                .await?;
            assert!(
                requests_rx.recv().await.is_some(),
                "New transition was not sent"
            );
            assert!(db::camera_health::list_pending_alerts(&config.db)
                .await?
                .is_empty());
            assert!(
                db::camera_health::alert_delivery_sent(
                    camera_id,
                    new.alert_kind(),
                    1,
                    new.state_changed_at().unwrap(),
                    &config.db
                )
                .await?
            );
            server.await??;
        }
        Ok(())
    }

    #[tokio::test]
    async fn http_success_requires_decodable_image_for_health_and_snapshot_cache(
    ) -> anyhow::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let html = b"<html><body>camera login page</body></html>".to_vec();
        let payload = Arc::new(std::sync::Mutex::new(html.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/snapshot", listener.local_addr()?);
        let server_payload = payload.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = socket.read(&mut request).await;
                let body = server_payload.lock().unwrap().clone();
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
        });
        let config = crate::test_support::config().await;
        db::rooms::sync_rooms_from_ha("test", "Test", &config.db).await?;
        let camera_id = db::cameras::add_manual_camera(
            db::cameras::NewCamera {
                name: "snapshot",
                room_id: 1,
                stream_url: "rtsp://unused",
                snapshot_url: Some(&url),
                clip_seconds: 5,
            },
            &config.db,
        )
        .await?;
        db::settings::set_i64(db::settings::CAMERA_HEALTH_FAILURE_THRESHOLD, 1, &config.db).await?;
        db::settings::set_i64(
            db::settings::CAMERA_HEALTH_RECOVERY_SUCCESSES,
            1,
            &config.db,
        )
        .await?;
        let camera = db::cameras::get_camera(camera_id, &config.db)
            .await?
            .unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(8, 8).write_to(&mut png, image::ImageFormat::Png)?;
        let valid = png.into_inner();
        for invalid in [html, valid[..valid.len() / 2].to_vec()] {
            *payload.lock().unwrap() = invalid;
            check_all_cameras(&config).await;
            let health = db::camera_health::get(camera_id, &config.db)
                .await?
                .unwrap();
            assert_eq!(
                health.health_state,
                db::camera_health::HEALTH_STATE_DEGRADED
            );
            assert_eq!(health.consecutive_successes, 0);
            assert!(health.last_snapshot_ok_at.is_none());
            assert!(health.recovered_at.is_none());
            config.camera_snapshot_cache.lock().await.clear();
            let preview =
                crate::core::camera_snapshots::resolve(config.clone(), camera.clone()).await;
            assert!(preview.bytes.is_none());
            assert!(preview.received_at.is_none());
            assert!(preview.failed);
        }
        *payload.lock().unwrap() = valid.clone();
        check_all_cameras(&config).await;
        let healthy = db::camera_health::get(camera_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(
            healthy.health_state,
            db::camera_health::HEALTH_STATE_HEALTHY
        );
        assert!(healthy.last_snapshot_ok_at.is_some());
        assert!(healthy.recovered_at.is_some());
        config.camera_snapshot_cache.lock().await.clear();
        let preview = crate::core::camera_snapshots::resolve(config.clone(), camera.clone()).await;
        assert_eq!(preview.bytes.as_deref(), Some(valid.as_slice()));
        assert!(preview.received_at.is_some());
        assert!(!preview.failed);
        let old_capture = Utc::now() - chrono::Duration::seconds(20);
        config
            .camera_snapshot_cache
            .lock()
            .await
            .get_mut(&camera_id)
            .unwrap()
            .captured_at = Some(old_capture);
        *payload.lock().unwrap() = b"<html>temporary error</html>".to_vec();
        crate::core::camera_snapshots::resolve(config.clone(), camera.clone()).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if config
                    .camera_snapshot_cache
                    .lock()
                    .await
                    .get(&camera_id)
                    .is_some_and(|cache| !cache.refreshing)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let stale = crate::core::camera_snapshots::resolve(config.clone(), camera).await;
        assert_eq!(stale.bytes.as_deref(), Some(valid.as_slice()));
        assert_eq!(stale.received_at, Some(old_capture));
        assert!(stale.failed);
        check_all_cameras(&config).await;
        let degraded = db::camera_health::get(camera_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(
            degraded.health_state,
            db::camera_health::HEALTH_STATE_DEGRADED
        );
        assert_eq!(degraded.last_snapshot_ok_at, healthy.last_snapshot_ok_at);
        server.abort();
        let _ = server.await;
        Ok(())
    }

    #[test]
    fn degraded_alert_contains_context() {
        let alert = db::camera_health::PendingHealthAlert {
            camera_id: 1,
            health_state: db::camera_health::HEALTH_STATE_DEGRADED.to_string(),
            last_error: Some("timeout".to_string()),
            last_failure_kind: Some("snapshot".to_string()),
            consecutive_failures: 3,
            degraded_at: Some(Utc::now()),
            recovered_at: None,
        };

        let text = format_alert("Front", &alert);
        assert!(text.contains("Front"));
        assert!(text.contains("timeout"));
        assert!(text.contains("snapshot"));
        assert!(text.contains("3"));
    }
}
