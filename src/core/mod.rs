pub(crate) mod camera_recording;
pub(crate) mod camera_recording_matcher;
pub(crate) mod cameras;
pub(crate) mod commands;
pub mod devices;
pub(crate) mod maintenance;
mod notification;
pub(crate) mod presentation;
pub(crate) mod types;
pub(crate) mod ui_background;
pub(crate) mod voice;

use crate::db;
use crate::models::{AppConfig, UserSession};
use chrono::{DateTime, Utc};
pub use maintenance::spawn_background_maintenance;
pub use notification::spawn_notification_processor;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct HeaderItem {
    pub icon: String,
    pub label: String,
    pub value: String,
    pub last_update: DateTime<Utc>,
}

impl AppConfig {
    pub async fn get_header_data(&self, user_id: u64) -> Vec<HeaderItem> {
        use crate::bot::utils::md;
        use crate::core::presentation::StateFormatter;
        use crate::i18n::t;
        let mut items = Vec::new();
        let lang = db::get_user_language(user_id, &self.db)
            .await
            .ok()
            .flatten()
            .unwrap_or(self.default_language);

        let window_mins = self.ttl_notifications;

        let runtime_status = self.runtime_status.read().await.clone();
        if let Some(shutdown_requested_at) = runtime_status.shutdown_requested_at {
            let reason = runtime_status
                .shutdown_reason
                .as_deref()
                .unwrap_or("shutdown signal");
            items.push(HeaderItem {
                icon: "🛑".into(),
                label: t(lang, "system.shutdown.label").into(),
                value: md::bold(&format!("{}: {}", t(lang, "system.shutdown.value"), reason)),
                last_update: shutdown_requested_at,
            });
        }

        // 1. Показываем активные записи камер как глобальный статус.
        match db::cameras::list_accessible_cameras(user_id, user_id == self.root_user, &self.db)
            .await
        {
            Ok(cameras) => {
                let camera_ids = cameras.iter().map(|camera| camera.id).collect::<Vec<_>>();
                match db::camera_recording_sessions::list_active_sessions_for_cameras(
                    &camera_ids,
                    &self.db,
                )
                .await
                {
                    Ok(sessions) => {
                        let mut active_by_camera = HashMap::new();
                        for session in sessions {
                            active_by_camera.entry(session.camera_id).or_insert(session);
                        }

                        for camera in cameras {
                            if let Some(session) = active_by_camera.get(&camera.id) {
                                let remaining = session
                                    .stop_after_at
                                    .signed_duration_since(Utc::now())
                                    .num_seconds()
                                    .max(0);
                                let remaining =
                                    crate::core::camera_recording::format_recording_duration(
                                        remaining,
                                    );

                                items.push(HeaderItem {
                                    icon: "🔴".into(),
                                    label: format!(
                                        "{} · {}",
                                        t(lang, "recording.header"),
                                        camera.name
                                    ),
                                    value: format!(
                                        "{} · {}",
                                        md::bold(t(lang, "recording.active")),
                                        md::plain(&format!(
                                            "{} {}",
                                            t(lang, "recording.remaining"),
                                            remaining
                                        ))
                                    ),
                                    last_update: session.last_event_at,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        error!("Ошибка БД при сборе активных записей для шапки: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Ошибка БД при сборе камер для шапки: {}", e);
            }
        }

        // 2. Получаем активные алерты
        match db::device_event_log::EventLogger::fetch_active_alerts(user_id, window_mins, &self.db)
            .await
        {
            Ok(alerts) => {
                for alert in alerts {
                    // А. Определяем домен и класс (для иконок)
                    let domain = alert.entity_id.split('.').next().unwrap_or("");
                    // В идеале alert должен содержать device_class из БД, если нет — используем ""
                    let class = "";

                    // Б. Получаем локализованное имя устройства (Алиас)
                    let name = self
                        .name_aliases
                        .get(&alert.entity_id)
                        .map(|r| r.value().clone())
                        .unwrap_or_else(|| alert.entity_id.clone());

                    // В. Получаем префикс комнаты (Breadcrumbs)
                    let room_prefix = if let Ok(Some(rid)) =
                        db::devices::get_room_id_by_entity(&alert.entity_id, &self.db).await
                    {
                        if let Ok(Some(room)) = db::rooms::get_room_by_id(rid, &self.db).await {
                            format!("{} • ", room.alias.as_deref().unwrap_or(&room.area))
                        } else {
                            "".to_string()
                        }
                    } else {
                        "".to_string()
                    };

                    // Г. Форматируем состояние и иконку через ядро
                    let inverted = db::devices::is_state_inverted(&alert.entity_id, &self.db)
                        .await
                        .unwrap_or(false);
                    let logical_state = StateFormatter::logical_state(&alert.last_state, inverted);
                    let state_alias =
                        self.state_alias_for_display(&alert.entity_id, &alert.last_state, inverted);
                    let icon = StateFormatter::get_icon(domain, class, &logical_state);
                    let human_state = StateFormatter::format_state_value_with_alias(
                        domain,
                        class,
                        &alert.last_state,
                        inverted,
                        state_alias.as_deref(),
                    );

                    // Д. Форматируем мета-информацию (счетчик)
                    let count_suffix = if alert.event_count > 1 {
                        format!(" [x{}]", alert.event_count)
                    } else {
                        "".to_string()
                    };

                    // Собираем элемент для шапки
                    items.push(HeaderItem {
                        icon: icon.into(),
                        label: format!("{}{}", room_prefix, name),
                        value: format!("{}{}", md::bold(&human_state), md::plain(&count_suffix)),
                        last_update: alert.last_updated,
                    });
                }
            }
            Err(e) => {
                error!("Ошибка БД при сборе данных для шапки: {}", e);
            }
        }

        // 3. Если событий не было — выводим "чистый" статус
        if items.is_empty() {
            items.push(HeaderItem {
                icon: "✅".into(), // Сменил 🏠 на ✅ для лучшего контраста при алерте
                label: t(lang, "system.label").into(),
                value: t(lang, "system.ok").into(),
                last_update: Utc::now(),
            });
        }

        items
    }
}

// TODO realization pinned in future
// pub async fn get_header_data(&self, user_id: u64) -> Vec<HeaderItem> {

// pub async fn get_header_data(&self, user_id: u64) -> Vec<HeaderItem> {
//     let mut items = Vec::new();

// let alerts = crate::db::active_alerts::get_user_alerts(&self.db, user_id).await.context("failed to get user alerts").unwrap();
//
// if let Some((eid, state, count, last_update_time)) = alerts.into_iter().next() {
//     let name = self.name_aliases.get(&eid)
//         .map(|s| s.clone())
//         .unwrap_or(eid);
//
//     // Senior Tip: если счетчик больше 1, пользователю полезно это видеть
//     let count_suffix = if count > 1 { format!(" (x{})", count) } else { "".to_string() };
//
//     items.push(HeaderItem {
//         icon: "🔔".into(),
//         label: "Последнее".into(),
//         value: format!("{}: {}{}", name, state, count_suffix),
//         last_update: last_update_time,
//     });
// }
//
// // --- 2. Персональные закрепленные сенсоры ---
// if let Some(session) = self.sessions.get(&user_id) {
//     for eid in &session.header_entities {
//         let name = self.name_aliases.get(eid)
//             .map(|s| s.clone())
//             .unwrap_or_else(|| eid.clone());
//
//         // if let Some(state_lock) = self.global_states.get(eid) {
//         //     let snapshot = state_lock.read();
//         //     items.push(HeaderItem {
//         //         icon: "📍".into(),
//         //         label: name,
//         //         value: snapshot.current_state.clone(),
//         //     });
//         // }
//     }
// }

// items
// }
// }

pub async fn update_user_state_with_mode(
    config: &Arc<AppConfig>,
    user_id: u64,
    msg_id: i32,
    context: &str,
    ui_message_mode: crate::models::UiMessageMode,
) {
    let context_owned = context.to_string();
    let now = Utc::now();
    let previous_context = config
        .sessions
        .get(&user_id)
        .map(|session| session.current_context.clone());

    if previous_context.as_deref() == Some(context) {
        debug!(
            "REFRESH USER STATE: user: {}, context: {}",
            user_id, context
        );
    } else {
        info!("UPDATE USER STATE: user: {}, context: {}", user_id, context);
    }

    config.sessions.insert(
        user_id,
        UserSession {
            last_menu_id: msg_id,
            current_context: context_owned.clone(),
            ui_message_mode,
            header_entities: config
                .sessions
                .get(&user_id)
                .map(|s| s.header_entities.clone())
                .unwrap_or_default(),
            recording_rule_wizard: config
                .sessions
                .get(&user_id)
                .and_then(|s| s.recording_rule_wizard.clone()),
            last_ui_refresh_at: Some(now),
            ui_refresh_blocked_until: None,
            last_seen_at: now,
        },
    );

    let pool = config.db.clone();
    let ctx = context_owned;

    crate::db::save_user_session(user_id, msg_id, &ctx, now, &pool).await;
}
