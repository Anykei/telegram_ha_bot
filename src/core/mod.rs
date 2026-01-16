mod notification;
pub(crate) mod maintenance;

use std::sync::Arc;
use chrono::{DateTime, Utc};
pub use notification::spawn_notification_processor;
pub use maintenance::spawn_background_maintenance;
use crate::db;
use crate::models::{AppConfig, UserSession};

pub struct HeaderItem {
    pub icon: String,
    pub label: String,
    pub value: String,
    pub last_update: DateTime<Utc>,
}

impl AppConfig {

    pub async fn get_header_data(&self, user_id: u64) -> Vec<HeaderItem> {
        let mut items = Vec::new();

        // 1. Пытаемся получить агрегированные алерты за последние 30 минут
        // Мы используем 30 минут как "окно актуальности", это можно вынести в конфиг
        let window_mins = 30;

        match db::device_event_log::EventLogger::fetch_active_alerts(&self.db, user_id, window_mins).await {
            Ok(alerts) => {
                for alert in alerts {
                    // Достаем человеческое имя из DashMap (память)
                    let name = self.name_aliases
                        .get(&alert.entity_id)
                        .map(|s| s.clone())
                        .unwrap_or_else(|| alert.entity_id.clone());

                    // Формируем счетчик, если событий > 1 (например: "Открыто (x3)")
                    let count_suffix = if alert.event_count > 1 {
                        format!(" (x{})", alert.event_count)
                    } else {
                        "".to_string()
                    };

                    // Создаем элемент шапки
                    items.push(HeaderItem {
                        icon: "🔔".into(),
                        label: name,
                        value: format!("{}{}", alert.last_state, count_suffix),
                        last_update: alert.last_updated,
                    });
                }
            }
            Err(e) => {
                // Если база данных временно недоступна, логируем ошибку,
                // но не обрушиваем весь процесс рендеринга меню
                error!("Ошибка БД при сборе данных для шапки: {}", e);
            }
        }

        // 2. Если событий за 30 минут не было, выводим позитивный статус
        if items.is_empty() {
            items.push(HeaderItem {
                icon: "🏠".into(),
                label: "Система".into(),
                value: "Все спокойно".into(),
                last_update: Utc::now(),
            });
        }

        items
    }

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
}

pub async fn update_user_state(config: &Arc<AppConfig>, user_id: u64, msg_id: i32, context: &str) {
    let context_owned = context.to_string();

    // 1. МГНОВЕННО обновляем оперативную память (DashMap)
    // Это гарантирует, что "Живая шапка" сразу увидит новые координаты пользователя
    config.sessions.insert(user_id, UserSession {
        last_menu_id: msg_id,
        current_context: context_owned.clone(),
        // Сохраняем уже выбранные закрепленные сущности
        header_entities: config.sessions.get(&user_id)
            .map(|s| s.header_entities.clone())
            .unwrap_or_default(),
    });

    // 2. АСИНХРОННО пишем в базу данных
    let pool = config.db.clone();
    let ctx = context_owned;

    crate::db::save_user_session(&pool, user_id, msg_id, &ctx).await;
}