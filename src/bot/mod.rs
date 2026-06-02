pub(crate) mod format;
pub(crate) mod handlers;
pub(crate) mod models;
pub(crate) mod notification;
pub(crate) mod router;
mod screens;
pub(crate) mod text_commands;
pub(crate) mod utils;

pub(crate) use crate::bot::router::State;
use crate::db;
use crate::models::AppConfig;
use std::sync::Arc;
use std::time::Duration;
use teloxide::net;
use teloxide::types::UpdateKind;
use teloxide::{
    dispatching::{dialogue::InMemStorage, UpdateHandler},
    prelude::*,
    types::Update,
};

/// Инициализирует экземпляр бота.
pub fn init(token: String) -> Bot {
    let client = net::default_reqwest_settings()
        .timeout(Duration::from_secs(90))
        .build()
        .expect("Telegram HTTP client creation failed");

    Bot::with_client(token, client)
}

/// Строит дерево обработки обновлений (Update Hierarchy).
/// Соответствует Google Standard: разделение ответственности между уровнями фильтрации.
pub fn schema() -> UpdateHandler<anyhow::Error> {
    // 1. Фильтр авторизации: проверяет права доступа пользователя.
    let auth_filter = dptree::filter_async(|update: Update, config: Arc<AppConfig>| async move {
        let Some(user) = update.from() else {
            return false;
        };

        let user_id = user.id.0;

        // Root пользователь имеет безусловный доступ.
        if config.root_user == user_id {
            return true;
        }

        // Проверка наличия пользователя в белом списке БД.
        db::user_exists(user_id, &config.db).await
    });

    // 2. Ветка команд: обрабатывает системные команды (начинающиеся с /).
    let command_handler = Update::filter_message()
        .filter_command::<handlers::Command>()
        .endpoint(handlers::handle_command);

    // 3. Ветка Callback-запросов: обрабатывает нажатия инлайн-кнопок.
    let callback_handler = Update::filter_callback_query().endpoint(handlers::handle_callback);

    // 4. Ветка Диалогов: обрабатывает текстовый ввод в зависимости от состояния.
    let message_dialogues = Update::filter_message()
        // Игнорируем команды, чтобы они не перехватывались диалогом.
        .filter(|msg: Message| msg.text().is_none_or(|t| !t.starts_with('/')))
        .branch(
            dptree::filter_map(|state: State| match state {
                State::WaitingForStateAlias {
                    device_id,
                    original_state,
                    room_id,
                } => Some((device_id, original_state, room_id)),
                _ => None,
            })
            .endpoint(handlers::handle_state_alias_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::WaitingForGraphInterval { device_id, room_id } => Some((device_id, room_id)),
                _ => None,
            })
            .endpoint(handlers::handle_custom_interval),
        )
        .branch(
            dptree::filter(|state: State| matches!(state, State::AddUser { .. }))
                .endpoint(handlers::handle_add_user_input),
        )
        .branch(
            dptree::filter(|state: State| matches!(state, State::DeleteUser { .. }))
                .endpoint(handlers::handle_delete_user_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::AddCamera { room_id } => Some(room_id),
                _ => None,
            })
            .endpoint(handlers::handle_add_camera_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::AddRecordingRule { room_id } => Some(room_id),
                _ => None,
            })
            .endpoint(handlers::handle_add_recording_rule_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::EditRecordingRule { room_id, rule_id } => Some((room_id, rule_id)),
                _ => None,
            })
            .endpoint(handlers::handle_edit_recording_rule_input),
        )
        .branch(
            dptree::filter_map(
                |state: State, msg: Message, config: Arc<AppConfig>| match state {
                    State::Idle => recording_rule_room_from_current_context(&msg, &config),
                    _ => None,
                },
            )
            .endpoint(handlers::handle_add_recording_rule_input),
        )
        // Поглощаем сообщения в состоянии Idle, чтобы они не падали в Unhandled Update.
        .branch(
            dptree::filter(|state: State| matches!(state, State::Idle))
                .endpoint(handlers::handle_idle_text),
        );

    // 5. Итоговое дерево (Main Entry Point)
    dptree::entry()
        // Инъекция хранилища состояний диалогов.
        .enter_dialogue::<Update, InMemStorage<State>, State>()
        .chain(auth_filter)
        .branch(command_handler)
        .branch(callback_handler)
        .branch(message_dialogues)
        .endpoint(|update: Update, state: State| async move {
            let user_id = update.from().map(|u| u.id.0).unwrap_or(0);

            let update_type = match &update.kind {
                UpdateKind::Message(m) => m.text().unwrap_or("[no text]"),
                UpdateKind::CallbackQuery(q) => q.data.as_deref().unwrap_or("[no data]"),
                UpdateKind::EditedMessage(_) => "EditedMessage",
                UpdateKind::InlineQuery(_) => "InlineQuery",
                _ => "Other",
            };

            log::warn!(
                "⚠️ Unhandled Update: ID={:?}, User={}, Type={}, State={:?}",
                update.id,
                user_id,
                update_type,
                state
            );

            Ok::<(), anyhow::Error>(())
        })
}

fn recording_rule_room_from_current_context(msg: &Message, config: &Arc<AppConfig>) -> Option<i64> {
    let user_id = msg.from.as_ref()?.id.0;
    let text = msg.text()?.trim();
    if !looks_like_recording_rule_input(text) {
        return None;
    }

    let session = config.sessions.get(&user_id)?;
    match router::Payload::from_string(&session.current_context).ok()? {
        router::Payload::Admin(router::AdminPayload::RecordingRules { room }) => Some(room),
        _ => None,
    }
}

fn looks_like_recording_rule_input(text: &str) -> bool {
    let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
    lines >= 8 && text.contains(';')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_rule_input_detector_accepts_rule_form() {
        let text = "Тест: свет в коридоре\n3\nany\nlight.corridor;changed_from_to;off;on;\nlight.corridor;changed_from_to;on;off;\n60\n300\n0\n30";

        assert!(looks_like_recording_rule_input(text));
    }

    #[test]
    fn recording_rule_input_detector_rejects_plain_message() {
        assert!(!looks_like_recording_rule_input("привет"));
    }
}
