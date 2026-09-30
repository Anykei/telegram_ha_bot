use std::sync::{Arc, OnceLock};

use teloxide::Bot;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::models::{AppConfig, NotificationData, NotificationScope};

pub const NOTIFICATION_QUEUE_CAPACITY: usize = 256;
pub const NOTIFICATION_WORKERS: usize = 2;

#[derive(Default)]
pub struct NotificationDelivery {
    pub(crate) refreshes: super::notification::LatestRefreshes,
    tx: OnceLock<mpsc::Sender<QueuedNotification>>,
}

#[cfg(test)]
pub(crate) fn hold_delivery(config: &AppConfig, capacity: usize) -> impl Sized {
    let (tx, rx) = mpsc::channel(capacity);
    assert!(config.notification_delivery.tx.set(tx).is_ok());
    // Retaining the receiver models a Telegram transport held at a barrier.
    rx
}

struct QueuedNotification {
    user_id: u64,
    generation: String,
    text: String,
    critical: bool,
    scope: NotificationScope,
}

pub async fn enqueue(config: &AppConfig, data: NotificationData) -> anyhow::Result<()> {
    for recipient in data.recipients {
        let user_id = u64::try_from(recipient)?;
        let Some(generation) = crate::db::admission_generation(user_id, &config.db).await? else {
            continue;
        };
        let item = QueuedNotification {
            user_id,
            generation,
            text: data.human_state.clone(),
            critical: data.critical,
            scope: data.scope.clone(),
        };
        let accepted = config
            .notification_delivery
            .tx
            .get()
            .is_some_and(|tx| tx.try_send(item).is_ok());
        let mut status = config.runtime_status.write().await;
        status.notifications_queued = config
            .notification_delivery
            .tx
            .get()
            .map(|tx| tx.max_capacity() - tx.capacity())
            .unwrap_or(0);
        if !accepted {
            status.notification_overflow += 1;
            drop(status);
            log::warn!(
                "operation=notification owner={} stage=enqueue result=notification_overflow",
                user_id
            );
            if let Err(error) = crate::db::activity_log::log(
                crate::db::activity_log::NewActivity {
                    user_id: Some(user_id),
                    kind: "notification",
                    entity_type: "user",
                    entity_id: None,
                    action: "notification_overflow",
                    status: "error",
                    message: None,
                },
                &config.db,
            )
            .await
            {
                log::warn!(
                    "operation=notification stage=overflow_log result=failed reason={error}"
                );
            }
        }
    }
    Ok(())
}

pub fn spawn_worker(bot: Bot, config: Arc<AppConfig>, cancel: CancellationToken) -> JoinHandle<()> {
    let (tx, mut rx) = mpsc::channel(NOTIFICATION_QUEUE_CAPACITY);
    assert!(
        config.notification_delivery.tx.set(tx).is_ok(),
        "notification worker already started"
    );
    tokio::spawn(async move {
        let mut jobs = JoinSet::new();
        let mut draining = false;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !draining => {
                    draining = true;
                    rx.close();
                    log::info!("operation=notification stage=shutdown queued={} active={}", rx.len(), jobs.len());
                }
                Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                    let mut status = config.runtime_status.write().await;
                    status.notifications_active = jobs.len();
                    if !matches!(result, Ok(Ok(()))) {
                        status.notifications_failed += 1;
                        log::warn!("operation=notification stage=delivery result=failed");
                    }
                }
                item = rx.recv(), if jobs.len() < NOTIFICATION_WORKERS => {
                    let Some(item) = item else { break; };
                    jobs.spawn(deliver(bot.clone(), config.clone(), item));
                    let mut status = config.runtime_status.write().await;
                    status.notifications_queued = rx.len();
                    status.notifications_active = jobs.len();
                }
            }
        }
        while let Some(result) = jobs.join_next().await {
            let mut status = config.runtime_status.write().await;
            status.notifications_active = jobs.len();
            if !matches!(result, Ok(Ok(()))) {
                status.notifications_failed += 1;
            }
        }
    })
}

async fn deliver(bot: Bot, config: Arc<AppConfig>, item: QueuedNotification) -> anyhow::Result<()> {
    if !crate::db::admission_is_current(item.user_id, &item.generation, &config.db).await? {
        log::info!(
            "operation=notification owner={} result=revoked",
            item.user_id
        );
        return Ok(());
    }
    let allowed = match &item.scope {
        NotificationScope::Device(entity) => {
            crate::db::subscriptions::get_subscribers(entity, &config.db)
                .await?
                .contains(&(item.user_id as i64))
        }
        NotificationScope::Camera(camera) => crate::db::cameras::get_accessible_camera(
            item.user_id,
            item.user_id == config.root_user,
            *camera,
            &config.db,
        )
        .await?
        .is_some(),
    };
    if !allowed
        || !crate::db::admission_is_current(item.user_id, &item.generation, &config.db).await?
    {
        return Ok(());
    }
    crate::bot::notification::deliver_notification(
        bot,
        config,
        item.user_id as i64,
        item.generation,
        item.text,
        item.critical,
        item.scope,
    )
    .await
}
