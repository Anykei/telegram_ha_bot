pub(crate) mod cameras;
pub mod devices;
pub(crate) mod maintenance;
mod notification;
pub(crate) mod presentation;
pub(crate) mod types;

use crate::db;
use crate::models::{AppConfig, UserSession};
use chrono::{DateTime, Utc};
pub use maintenance::spawn_background_maintenance;
pub use notification::spawn_notification_processor;
use serde::{Deserialize, Serialize};
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
        let mut items = Vec::new();

        let window_mins = self.ttl_notifications;

        // 1. Получаем активные алерты
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
                    let icon = StateFormatter::get_icon(domain, class, &alert.last_state);
                    let human_state =
                        StateFormatter::format_state_value(domain, class, &alert.last_state);

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

        // 2. Если событий не было — выводим "чистый" статус
        if items.is_empty() {
            items.push(HeaderItem {
                icon: "✅".into(), // Сменил 🏠 на ✅ для лучшего контраста при алерте
                label: "Система".into(),
                value: "Все спокойно".into(),
                last_update: Utc::now(),
            });
        }

        items
    }
}

/// TODO realization pinned in future
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

pub async fn update_user_state(config: &Arc<AppConfig>, user_id: u64, msg_id: i32, context: &str) {
    info!("UPDATE USER STATE: user: {}, context: {}", user_id, context);
    let context_owned = context.to_string();
    let now = Utc::now();

    config.sessions.insert(
        user_id,
        UserSession {
            last_menu_id: msg_id,
            current_context: context_owned.clone(),
            header_entities: config
                .sessions
                .get(&user_id)
                .map(|s| s.header_entities.clone())
                .unwrap_or_default(),
            last_ui_refresh_at: Some(now),
            ui_refresh_blocked_until: None,
            last_seen_at: now,
        },
    );

    let pool = config.db.clone();
    let ctx = context_owned;

    crate::db::save_user_session(user_id, msg_id, &ctx, now, &pool).await;
}
