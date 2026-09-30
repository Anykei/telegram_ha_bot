use log::{error, info, warn};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use teloxide::prelude::ChatId;
use teloxide::types::MessageId;
use teloxide::Bot;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::bot::router::{AdminPayload, ControlPayload, Payload};
use crate::bot::utils::md;
use crate::db;
use crate::ha::NotifyEvent;
use crate::models::{AppConfig, NotificationData, NotificationScope, UserSession};

const UI_REFRESH_CAPACITY: usize = 256;
const UI_REFRESH_WORKERS: usize = 4;

struct RefreshTarget {
    message_id: MessageId,
    context: String,
    generation: String,
}

#[derive(Default)]
pub(crate) struct LatestRefreshes {
    pending: Mutex<BTreeMap<u64, RefreshTarget>>,
    ready: tokio::sync::Notify,
    closed: AtomicBool,
}

impl LatestRefreshes {
    fn insert(&self, user_id: u64, target: RefreshTarget) -> bool {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.closed.load(Ordering::Acquire)
            || (pending.len() >= UI_REFRESH_CAPACITY && !pending.contains_key(&user_id))
        {
            return false;
        }
        pending.insert(user_id, target);
        self.ready.notify_one();
        true
    }

    fn close(&self) {
        // Serialize closure with inserts: a producer that has passed an earlier
        // check cannot add work after the consumer has finished draining.
        let _pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.closed.store(true, Ordering::Release);
        self.ready.notify_one();
    }

    fn take(&self) -> Option<(u64, RefreshTarget)> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_first()
    }

    fn len(&self) -> usize {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

/// Heartbeat and HA events share the same bounded, coalescing delivery queue.
/// Snapshot session and admission together, without holding the lifecycle lock
/// during rendering, Telegram calls, or a rate-limit wait.
pub(crate) async fn enqueue_current_refresh(
    config: &AppConfig,
    user_id: u64,
) -> anyhow::Result<bool> {
    enqueue_refresh(
        config,
        user_id,
        &config.notification_delivery.refreshes,
        None,
    )
    .await
}

async fn enqueue_refresh(
    config: &AppConfig,
    user_id: u64,
    refreshes: &LatestRefreshes,
    expected: Option<(MessageId, &str)>,
) -> anyhow::Result<bool> {
    if refreshes.closed.load(Ordering::Acquire) {
        return Ok(false);
    }
    let accepted = {
        let _lifecycle = config.admission_lifecycle.lock().await;
        let Some(generation) = db::admission_generation(user_id, &config.db).await? else {
            return Ok(false);
        };
        let now = Utc::now();
        let Some(target) = config.sessions.get(&user_id).and_then(|session| {
            if session.is_ui_refresh_blocked(now)
                || expected.is_some_and(|(message_id, context)| {
                    session.last_menu_id != message_id.0 || session.current_context != context
                })
            {
                return None;
            }
            Some(RefreshTarget {
                message_id: MessageId(session.last_menu_id),
                context: session.current_context.clone(),
                generation,
            })
        }) else {
            return Ok(false);
        };
        let accepted = refreshes.insert(user_id, target);
        if accepted {
            if let Some(mut session) = config.sessions.get_mut(&user_id) {
                session.last_ui_refresh_at = Some(now);
            }
        }
        accepted
    };
    let mut status = config.runtime_status.write().await;
    status.ui_refresh_queued = refreshes.len();
    if !accepted && !refreshes.closed.load(Ordering::Acquire) {
        status.ui_refresh_overflow += 1;
        warn!(
            "operation=ui_refresh owner={} stage=enqueue result=overflow",
            user_id
        );
    }
    Ok(accepted)
}

pub fn spawn_notification_processor(
    rx: mpsc::Receiver<NotifyEvent>,
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    info!("Core: Notification processor started");

    tokio::spawn(async move {
        let refreshes = &config.notification_delivery.refreshes;
        tokio::join!(
            process_inputs(rx, &config, refreshes, cancel_token),
            process_refreshes(bot, &config, refreshes),
        );
    })
}

async fn process_inputs(
    mut rx: mpsc::Receiver<NotifyEvent>,
    config: &Arc<AppConfig>,
    refreshes: &LatestRefreshes,
    cancel_token: CancellationToken,
) {
    let mut draining = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled(), if !draining => {
                rx.close();
                refreshes.close();
                draining = true;
                info!("operation=ha_event stage=shutdown queued={}", rx.len());
            }
            event = rx.recv() => {
                match event {
                    Some(event) => {
                        config.runtime_status.write().await.ha_events_queued = rx.len();
                        if let Err(e) = process_and_dispatch(config.clone(), event, refreshes).await {
                            error!("operation=ha_event stage=optional_delivery result=failed reason={}", e);
                        }
                    }
                    None => break,
                }
            }
        }
    }
    refreshes.close();
}

async fn process_refreshes(bot: Bot, config: &Arc<AppConfig>, refreshes: &LatestRefreshes) {
    let mut jobs = JoinSet::new();
    loop {
        while jobs.len() < UI_REFRESH_WORKERS {
            let Some((user_id, target)) = refreshes.take() else {
                break;
            };
            let bot = bot.clone();
            let config = config.clone();
            jobs.spawn(async move {
                let admission = db::user::Admission {
                    user_id,
                    generation: target.generation.clone(),
                };
                db::user::with_admission(admission, async move {
                    // Pending work cannot restore a screen superseded by navigation.
                    let current = config.sessions.get(&user_id).is_some_and(|s| {
                        s.last_menu_id == target.message_id.0 && s.current_context == target.context
                    });
                    if current
                        && db::admission_is_current(user_id, &target.generation, &config.db)
                            .await
                            .unwrap_or(false)
                    {
                        if let Err(error) = crate::bot::handlers::refresh_current_view(
                            &bot,
                            &config,
                            user_id,
                            ChatId(user_id as i64),
                            target.message_id,
                            &target.context,
                        )
                        .await
                        {
                            warn!(
                                "operation=ui_refresh owner={} result=failed reason={}",
                                user_id, error
                            );
                        }
                    }
                })
                .await;
            });
        }
        config.runtime_status.write().await.ui_refresh_queued = refreshes.len();
        if refreshes.closed.load(Ordering::Acquire) && refreshes.len() == 0 && jobs.is_empty() {
            break;
        }
        tokio::select! {
            _ = refreshes.ready.notified() => {},
            Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                if let Err(error) = result { warn!("operation=ui_refresh result=failed reason={error}"); }
            }
        }
    }
}

async fn process_and_dispatch(
    config: Arc<AppConfig>,
    event: NotifyEvent,
    refreshes: &LatestRefreshes,
) -> anyhow::Result<()> {
    if event.new_state == event.old_state {
        return Ok(());
    }
    log::debug!("Core: New state change {}", event.entity_id);

    if let Err(error) =
        crate::core::camera_recording_matcher::process_event(config.clone(), &event).await
    {
        config.runtime_status.write().await.ha_rule_failures += 1;
        error!(
            "operation=ha_event stage=recording_rules result=failed reason={}",
            error
        );
    }
    config.runtime_status.write().await.ha_events_processed += 1;

    if let Err(error) = db::device_event_log::EventLogger::record_event(
        &event.entity_id,
        &event.new_state,
        &config.db,
    )
    .await
    {
        warn!(
            "operation=ha_event stage=event_log result=failed reason={}",
            error
        );
    }

    let room_id_opt = db::devices::get_room_id_by_entity(&event.entity_id, &config.db).await?;

    let recipients = db::subscriptions::get_subscribers(&event.entity_id, &config.db).await?;

    let recipients_set: std::collections::HashSet<u64> =
        recipients.iter().map(|&id| id as u64).collect();

    let now = Utc::now();
    let mut refresh_targets = Vec::new();

    for entry in config.sessions.iter() {
        let user_id = *entry.key();
        let session = entry.value();

        let is_watching = room_id_opt.is_some_and(|rid| is_user_watching_room(session, rid));

        let is_subscriber = recipients_set.contains(&user_id);

        if (is_watching || is_subscriber)
            && is_live_refresh_allowed(session)
            && !session.is_ui_refresh_blocked(now)
            && can_refresh_after_event(session, now, config.event_refresh_min_interval_s)
        {
            refresh_targets.push((
                user_id,
                MessageId(session.last_menu_id),
                session.current_context.clone(),
            ));
        }
    }

    for (user_id, message_id, context) in refresh_targets {
        enqueue_refresh(&config, user_id, refreshes, Some((message_id, &context))).await?;
    }

    // let recipients = db::subscriptions::get_subscribers(&config.db, &event.entity_id).await?;
    if !recipients.is_empty() {
        use crate::core::presentation::StateFormatter;

        let room_prefix = if let Some(rid) = room_id_opt {
            if let Ok(Some(room)) = db::rooms::get_room_by_id(rid, &config.db).await {
                format!(
                    "{} • ",
                    md::bold(room.alias.as_deref().unwrap_or(&room.area))
                )
            } else {
                "".to_string()
            }
        } else {
            "".to_string()
        };

        // Определяем домен и класс для форматирования
        let domain = event.entity_id.split('.').next().unwrap_or("");
        let class = event.device_class.as_deref().unwrap_or("");

        // Используем наше ядро для красоты
        let inverted = db::devices::is_state_inverted(&event.entity_id, &config.db)
            .await
            .unwrap_or(false);
        let logical_state = StateFormatter::logical_state(&event.new_state, inverted);
        let state_alias =
            config.state_alias_for_display(&event.entity_id, &event.new_state, inverted);
        let icon = StateFormatter::get_icon(domain, class, &logical_state);
        let human_state = StateFormatter::format_state_value_with_alias(
            domain,
            class,
            &event.new_state,
            inverted,
            state_alias.as_deref(),
        );

        let display_name = config
            .name_aliases
            .get(&event.entity_id)
            .map(|r| r.value().clone())
            .unwrap_or_else(|| event.friendly_name.clone());

        let message_text = format!(
            "{}{} {}: {}",
            icon,
            room_prefix,
            md::plain(&display_name),
            md::bold(&human_state)
        );
        let critical = db::devices::is_device_critical(&event.entity_id, &config.db)
            .await
            .unwrap_or(false);

        let data = NotificationData {
            human_state: message_text,
            recipients,
            critical,
            scope: NotificationScope::Device(event.entity_id.clone()),
        };
        crate::core::notification_delivery::enqueue(&config, data).await?;
    }
    Ok(())
}

fn is_user_watching_room(session: &UserSession, room_id: i64) -> bool {
    if let Ok(payload) = Payload::from_string(&session.current_context) {
        match payload {
            Payload::Control(ControlPayload::RoomDetail { room }) => room == room_id,
            Payload::Control(ControlPayload::DeviceControl { room, .. }) => room == room_id,
            Payload::Control(ControlPayload::QuickAction { room, .. }) => room == room_id,
            _ => false,
        }
    } else {
        false
    }
}

fn is_live_refresh_allowed(session: &UserSession) -> bool {
    !matches!(
        Payload::from_string(&session.current_context),
        Ok(Payload::Admin(AdminPayload::ActivityLog { .. }))
    )
}

fn can_refresh_after_event(session: &UserSession, now: DateTime<Utc>, min_interval_s: u64) -> bool {
    session
        .last_ui_refresh_at
        .map(|last_refresh| {
            now.signed_duration_since(last_refresh).num_seconds() >= min_interval_s as i64
        })
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[tokio::test]
    async fn blocked_telegram_does_not_delay_thousand_event_transitions() {
        let config = crate::test_support::config().await;
        db::add_user(42, &config.db).await.unwrap();
        db::rooms::sync_rooms_from_ha("kitchen", "Kitchen", &config.db)
            .await
            .unwrap();
        db::devices::sync_device("binary_sensor.door", "kitchen", "Door", "door", &config.db)
            .await
            .unwrap();
        db::subscriptions::toggle_hidden("binary_sensor.door", &config.db)
            .await
            .unwrap();
        db::subscriptions::toggle_subscription(42, "binary_sensor.door", &config.db)
            .await
            .unwrap();
        assert_eq!(
            db::subscriptions::get_subscribers("binary_sensor.door", &config.db)
                .await
                .unwrap(),
            vec![42]
        );
        let telegram_barrier = super::super::notification_delivery::hold_delivery(&config, 8);
        let (tx, rx) = mpsc::channel(100);
        let producer = tokio::spawn(async move {
            for index in 0..1000 {
                tx.send(NotifyEvent {
                    received_at: Utc::now(),
                    entity_id: "binary_sensor.door".into(),
                    old_state: (index % 2).to_string(),
                    new_state: ((index + 1) % 2).to_string(),
                    friendly_name: "Door".into(),
                    device_class: Some("door".into()),
                })
                .await
                .unwrap();
            }
        });
        let refreshes = LatestRefreshes::default();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            process_inputs(rx, &config, &refreshes, CancellationToken::new()),
        )
        .await
        .expect("events must drain while Telegram is still blocked");
        producer.await.unwrap();
        let status = config.runtime_status.read().await;
        assert_eq!(status.ha_events_processed, 1000);
        assert_eq!(status.ha_rule_failures, 0);
        assert_eq!(status.notifications_queued, 8);
        assert_eq!(status.notification_overflow, 992);
        assert_eq!(refreshes.len(), 0);
        drop(telegram_barrier);
    }

    #[test]
    fn refresh_queue_is_bounded_and_keeps_latest_per_user() {
        let refreshes = LatestRefreshes::default();
        for user_id in 0..UI_REFRESH_CAPACITY as u64 {
            assert!(refreshes.insert(
                user_id,
                RefreshTarget {
                    message_id: MessageId(1),
                    context: "old".into(),
                    generation: "test".into()
                }
            ));
        }
        assert!(!refreshes.insert(
            9999,
            RefreshTarget {
                message_id: MessageId(1),
                context: "overflow".into(),
                generation: "test".into()
            }
        ));
        assert!(refreshes.insert(
            0,
            RefreshTarget {
                message_id: MessageId(2),
                context: "latest".into(),
                generation: "test".into()
            }
        ));
        assert_eq!(refreshes.len(), UI_REFRESH_CAPACITY);
        let (user_id, target) = refreshes.take().unwrap();
        assert_eq!(user_id, 0);
        assert_eq!(target.context, "latest");
    }

    #[tokio::test]
    async fn heartbeat_refresh_shares_capacity_and_shutdown_rejects_late_enqueues() {
        let config = crate::test_support::config().await;
        for user_id in 1..=UI_REFRESH_CAPACITY as u64 + 1 {
            db::add_user(user_id, &config.db).await.unwrap();
            config.sessions.insert(user_id, session_for(Payload::Home));
            assert_eq!(
                enqueue_current_refresh(&config, user_id).await.unwrap(),
                user_id <= UI_REFRESH_CAPACITY as u64
            );
        }
        let refreshes = &config.notification_delivery.refreshes;
        assert_eq!(refreshes.len(), UI_REFRESH_CAPACITY);
        assert_eq!(config.runtime_status.read().await.ui_refresh_overflow, 1);
        // Coalescing updates the same slot even when the queue is full.
        config.sessions.get_mut(&1).unwrap().last_menu_id = 2;
        assert!(enqueue_current_refresh(&config, 1).await.unwrap());
        let (user_id, target) = refreshes.take().unwrap();
        assert_eq!(user_id, 1);
        assert_eq!(target.message_id, MessageId(2));
        assert_eq!(
            Some(target.generation),
            db::admission_generation(1, &config.db).await.unwrap()
        );
        // A producer already waiting for the lifecycle mutex cannot insert after close.
        let lifecycle = config.admission_lifecycle.lock().await;
        let late = {
            let config = config.clone();
            tokio::spawn(async move { enqueue_current_refresh(&config, 1).await })
        };
        tokio::task::yield_now().await;
        assert!(!late.is_finished());
        refreshes.close();
        drop(lifecycle);
        assert!(!late.await.unwrap().unwrap());
        assert_eq!(refreshes.len(), UI_REFRESH_CAPACITY - 1);
        assert_eq!(config.runtime_status.read().await.ui_refresh_overflow, 1);
    }

    #[tokio::test]
    async fn heartbeat_snapshot_cannot_mix_old_session_and_new_admission() {
        let config = crate::test_support::config().await;
        db::add_user(42, &config.db).await.unwrap();
        config.sessions.insert(42, session_for(Payload::Home));
        let old_generation = db::admission_generation(42, &config.db)
            .await
            .unwrap()
            .unwrap();
        let lifecycle = config.admission_lifecycle.lock().await;
        let pending = {
            let config = config.clone();
            tokio::spawn(async move { enqueue_current_refresh(&config, 42).await })
        };
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        // Same lock order as administrative removal/session initialization.
        db::delete_user(42, &config.db).await.unwrap();
        config.sessions.remove(&42);
        db::add_user(42, &config.db).await.unwrap();
        let mut session = session_for(Payload::Home);
        session.last_menu_id = 2;
        config.sessions.insert(42, session);
        drop(lifecycle);
        assert!(pending.await.unwrap().unwrap());
        let (user_id, target) = config.notification_delivery.refreshes.take().unwrap();
        assert_eq!(user_id, 42);
        assert_eq!(target.message_id, MessageId(2));
        assert_ne!(target.generation, old_generation);
        assert!(db::admission_is_current(42, &target.generation, &config.db)
            .await
            .unwrap());
        assert!(!db::admission_is_current(42, &old_generation, &config.db)
            .await
            .unwrap());
    }

    fn session_for(payload: Payload) -> UserSession {
        UserSession {
            last_menu_id: 1,
            current_context: payload.to_string(),
            ui_message_mode: crate::models::UiMessageMode::Photo,
            header_entities: HashSet::new(),
            recording_rule_wizard: None,
            last_ui_refresh_at: None,
            ui_refresh_blocked_until: None,
            last_seen_at: Utc::now(),
            last_persisted_at: None,
        }
    }

    #[test]
    fn detects_room_context_from_compact_payload() {
        let session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));

        assert!(is_user_watching_room(&session, 42));
        assert!(!is_user_watching_room(&session, 7));
    }

    #[test]
    fn ignores_invalid_legacy_context() {
        let session = UserSession {
            last_menu_id: 1,
            current_context: r#"{"Control":{"RoomDetail":{"room":42}}}"#.to_string(),
            ui_message_mode: crate::models::UiMessageMode::Photo,
            header_entities: HashSet::new(),
            recording_rule_wizard: None,
            last_ui_refresh_at: None,
            ui_refresh_blocked_until: None,
            last_seen_at: Utc::now(),
            last_persisted_at: None,
        };

        assert!(!is_user_watching_room(&session, 42));
    }

    #[test]
    fn activity_log_is_not_live_refreshed_by_ha_events() {
        let session = session_for(Payload::Admin(AdminPayload::ActivityLog {
            filter: crate::bot::router::ActivityLogFilter::All,
        }));

        assert!(!is_live_refresh_allowed(&session));
    }

    #[test]
    fn event_refresh_cooldown_allows_session_without_timestamp() {
        let session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));

        assert!(can_refresh_after_event(&session, Utc::now(), 5));
    }

    #[test]
    fn event_refresh_cooldown_blocks_recent_refresh() {
        let mut session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));
        session.last_ui_refresh_at = Some(Utc::now());

        assert!(!can_refresh_after_event(&session, Utc::now(), 5));
    }

    #[test]
    fn ui_refresh_block_state_expires_by_time() {
        let now = Utc::now();
        let mut session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));

        session.ui_refresh_blocked_until = Some(now + chrono::Duration::seconds(10));
        assert!(session.is_ui_refresh_blocked(now));

        session.ui_refresh_blocked_until = Some(now - chrono::Duration::seconds(1));
        assert!(!session.is_ui_refresh_blocked(now));
    }
}
