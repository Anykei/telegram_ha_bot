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
            db::camera_health::mark_degradation_alert_sent(alert.camera_id, &config.db).await
        } else {
            db::camera_health::mark_recovery_alert_sent(alert.camera_id, &config.db).await
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
