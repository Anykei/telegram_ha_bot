use anyhow::{Context, Result};
use std::sync::Arc;

use teloxide::macros::BotCommands;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{InputFile, InputMedia, InputMediaPhoto, MessageId, ParseMode};
use teloxide::{RequestError, Bot};

use crate::bot::router::{router, Payload};
use crate::bot::State;
use crate::models::{AppConfig};
use super::models::View;

pub type MyDialogue = Dialogue<State, InMemStorage<State>>;

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Доступные команды:")]
pub enum Command {
    #[command(description = "Показать главное меню")]
    Start,
}

/// Обработчик команд. Выполняет инициализацию сессии.
pub async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    let user_id = msg.from.as_ref().context("User missing")?.id.0;
    let chat_id = msg.chat.id;

    log::info!("Command: {:?} from user {}", cmd, user_id);

    match cmd {
        Command::Start => {
            dialogue.exit().await?;

            if let Some(session) = config.sessions.get(&user_id) {
                let _ = bot.delete_message(chat_id, MessageId(session.last_menu_id)).await;
            }

            let view = router(Payload::Home, user_id, config.clone()).await?;
            send_new_view(&bot, chat_id, user_id, view, config).await?;
        }
    }

    let _ = bot.delete_message(chat_id, msg.id).await;
    Ok(())
}

/// Точка входа для callback-запросов от кнопок.
pub async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    // Сразу отвечаем Telegram, чтобы убрать индикатор загрузки на кнопке
    let _ = bot.answer_callback_query(q.id.clone()).await;

    let data = q.data.as_ref().context("No callback data")?;
    let user_id = q.from.id.0;
    let msg = q.message.as_ref().context("Message missing")?;

    // Декодируем компактный бинарный формат
    let payload = Payload::from_string(data)
        .context("Critical: Failed to decode binary payload")?;

    // Роутинг: получение логического представления экрана
    let view = router(payload, user_id, config.clone()).await?;

    // Применяем изменения состояния и обновляем UI
    apply_view(&bot, &config, &dialogue, msg.chat().id, msg.id(), user_id, view).await
}

/// Метод для фонового обновления интерфейса (Live Updates).
pub async fn render_current_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user_id: u64,
    chat_id: ChatId,
    message_id: MessageId,
    context: &str,
) -> Result<()> {
    let payload = Payload::from_string(context).context("Failed to decode context")?;
    let view = router(payload, user_id, config.clone()).await?;

    update_view(bot, chat_id, message_id, user_id, view, config.clone()).await
}

/// Координирует изменение состояния диалога и обновление сообщения.
async fn apply_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    dialogue: &MyDialogue,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    view: View,
) -> Result<()> {
    // Синхронизация состояния диалога
    if let Some(new_state) = view.next_state.clone() {
        dialogue.update(new_state).await?;
    } else {
        dialogue.exit().await?;
    }

    update_view(bot, chat_id, message_id, user_id, view, config.clone()).await
}

pub async fn update_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
) -> anyhow::Result<()> {
    let text = view.get_text();
    let kb = view.kb.clone();
    let payload_str = view.payload.to_string();

    let input_file = match view.image.clone() {
        Some(v) => InputFile::memory(v),
        None => InputFile::memory(crate::bot::utils::UI_PLACEHOLDER_BYTES),
    };

    let media = InputMedia::Photo(
        InputMediaPhoto::new(input_file)
            .caption(&text)
            .parse_mode(ParseMode::MarkdownV2)
    );

    let res = bot.edit_message_media(chat_id, message_id, media)
        .reply_markup(kb)
        .await;

    match res {
        Ok(_) => {
            crate::core::update_user_state(&config, user_id, message_id.0, &payload_str).await;
            Ok(())
        }
        Err(RequestError::Api(teloxide::ApiError::MessageNotModified)) => Ok(()),
        Err(e) => {
            log::info!("Transitioning UI mode for user {}: {}", user_id, e);

            send_new_view(bot, chat_id, user_id, view, config).await?;

            let b = bot.clone();
            tokio::spawn(async move {
                let _ = b.delete_message(chat_id, message_id).await;
            });
            Ok(())
        }
    }
}

async fn send_new_view(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>
) -> Result<()> {
    let text = view.get_text();

    let input_file = match view.image.clone() {
        Some(v) => InputFile::memory(v),
        None => InputFile::memory(crate::bot::utils::UI_PLACEHOLDER_BYTES),
    };

    let media = InputMedia::Photo(
        InputMediaPhoto::new(input_file)
            .caption(&text)
            .parse_mode(ParseMode::MarkdownV2)
    );

    let sent = bot.send_photo(chat_id, InputFile::from(media))
        .caption(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(view.kb)
        .await?;

    crate::core::update_user_state(&config, user_id, sent.id.0, &view.payload.to_string()).await;
    Ok(())
}

pub async fn handle_new_name(
    bot: Bot,
    dialogue: MyDialogue,
    msg: Message,
    config: Arc<AppConfig>,
    (device_id, _room_id): (i64, i64),
) -> Result<()> {
    if let Some(text) = msg.text() {
        let name = text.trim();
        if !name.is_empty() {
            // crate::db::devices::update_device_alias(&config.db, device_id, name).await?;
            // config.name_aliases.insert(...);
        }
    }
    finalize_dialogue(bot, dialogue, msg, config).await
}

pub async fn handle_custom_interval(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (device_id, room_id): (i64, i64)
) -> Result<()> {
    if let Some(text) = msg.text() {
        if let Ok(hours) = text.parse::<u32>() {
            let new_payload = Payload::Control(crate::bot::router::ControlPayload::QuickAction {
                room: room_id,
                device: device_id,
                cmd: crate::bot::router::DeviceCmd::ShowChart { h: hours, o: 0 }
            });
            crate::core::update_user_state(&config, msg.from.as_ref().unwrap().id.0, 0, &new_payload.to_string()).await;
        }
    }
    finalize_dialogue(bot, dialogue, msg, config).await
}

async fn finalize_dialogue(bot: Bot, dialogue: MyDialogue, msg: Message, config: Arc<AppConfig>) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0;
    let chat_id = msg.chat.id;

    dialogue.exit().await?;
    let _ = bot.delete_message(chat_id, msg.id).await;

    if let Some(session) = config.sessions.get(&user_id) {
        render_current_view(&bot, &config, user_id, chat_id, MessageId(session.last_menu_id), &session.current_context).await?;
    }
    Ok(())
}