pub(crate) mod handlers;
mod utils;
pub(crate) mod notification;
pub(crate) mod models;
mod screens;
pub(crate) mod router;

use std::sync::Arc;
use teloxide::{
    dispatching::{dialogue::InMemStorage, UpdateHandler},
    prelude::*,
    types::Update,
};
use teloxide::types::UpdateKind;
pub(crate) use crate::bot::router::State;
use crate::models::AppConfig;
use crate::db;

/// Инициализирует экземпляр бота.
pub fn init(token: String) -> Bot {
    Bot::new(token)
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
    let callback_handler = Update::filter_callback_query()
        .endpoint(handlers::handle_callback);

    // 4. Ветка Диалогов: обрабатывает текстовый ввод в зависимости от состояния.
    let message_dialogues = Update::filter_message()
        // Игнорируем команды, чтобы они не перехватывались диалогом.
        .filter(|msg: Message| msg.text().map_or(true, |t| !t.starts_with('/')))
        // .branch(
        //     dptree::filter_map(|state: State| match state {
        //         State::WaitingForName { device_id, room_id } => Some((device_id, room_id)),
        //         _ => None,
        //     })
        //         .endpoint(handlers::handle_new_name),
        // )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::WaitingForGraphInterval { device_id, room_id } => Some((device_id, room_id)),
                _ => None,
            })
                .endpoint(handlers::handle_custom_interval),
        )
        // Поглощаем сообщения в состоянии Idle, чтобы они не падали в Unhandled Update.
        .branch(
            dptree::filter(|state: State| matches!(state, State::Idle))
                .endpoint(|bot: Bot, msg: Message| async move {
                    log::info!("Ignored junk message from user {}: {:?}", msg.chat.id, msg.text());
                    // Опционально: подчищаем чат за пользователем.
                    let _ = bot.delete_message(msg.chat.id, msg.id).await;
                    Ok(())
                })
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