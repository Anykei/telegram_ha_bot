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

/// Точка входа для команд. Соответствует Google Standard по очистке ресурсов.
pub async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    let user_id = msg.from.as_ref().context("User missing")?.id.0;
    let chat_id = msg.chat.id;

    log::info!("Processing command {:?} for user {}", cmd, user_id);

    match cmd {
        Command::Start => {
            // Сбрасываем диалог и удаляем старое меню
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

/// Основной диспетчер нажатий на кнопки.
pub async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    // 1. Мгновенно гасим spinner в Telegram (UX Standard)
    let _ = bot.answer_callback_query(q.id).await;

    let data = q.data.as_ref().context("No callback data")?;
    let user_id = q.from.id.0;
    let msg = q.message.as_ref().context("Message missing")?;

    // 2. Декодирование (Infallible logic)
    let payload = Payload::from_string(data)
        .context("Critical: Binary payload decoding failed")?;

    // 3. Роутинг
    let view = router(payload, user_id, config.clone()).await?;

    // 4. Оркестрация UI и State
    apply_view(&bot, &config, &dialogue, msg.chat().id, msg.id(), user_id, view).await
}

/// Live-обновление интерфейса без изменения состояния диалога.
pub async fn render_current_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user_id: u64,
    chat_id: ChatId,
    message_id: MessageId,
    context: &str,
) -> Result<()> {
    let payload = Payload::from_string(context).context("Context decoding failed")?;
    let view = router(payload, user_id, config.clone()).await?;

    update_view(bot, chat_id, message_id, user_id, view, config.clone()).await
}

/// Атомарно применяет изменения стейта и обновляет сообщение.
async fn apply_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    dialogue: &MyDialogue,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    view: View,
) -> Result<()> {
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

    // 1. Подготовка медиа-контента (Zero-copy path)
    let input_file = match &view.image {
        Some(v) => InputFile::memory(v.clone()),
        None => InputFile::memory(crate::bot::utils::UI_PLACEHOLDER_BYTES),
    };

    let media = InputMedia::Photo(
        InputMediaPhoto::new(input_file)
            .caption(&text)
            .parse_mode(ParseMode::MarkdownV2)
    );

    // 2. Пытаемся выполнить edit (Optimistic update)
    // ВАЖНО: мы всегда используем edit_message_media, так как наш бот теперь
    // всегда работает в режиме Photo (даже если это прозрачный пиксель).
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
            let err_str = e.to_string();
            if err_str.contains("message to edit not found") {
                log::warn!("Detected ghost message for user {}. Re-anchoring UI.", user_id);
            } else {
                log::info!("UI Mode transition for user {}: {}", user_id, err_str);
            }

            // Принудительно отправляем новое сообщение
            send_new_view(bot, chat_id, user_id, view, config).await?;

            // Пытаемся удалить старое, но игнорируем ошибку,
            // так как мы уже знаем, что его может не быть
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
    let payload_str = view.payload.to_string();

    // Исправлено: send_photo принимает InputFile, а не InputMedia
    let input_file = match view.image {
        Some(v) => InputFile::memory(v),
        None => InputFile::memory(crate::bot::utils::UI_PLACEHOLDER_BYTES),
    };

    let sent = bot.send_photo(chat_id, input_file)
        .caption(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(view.kb)
        .await?;

    // Критическая правка: сохраняем ID СООБЩЕНИЯ БОТА (sent.id), а не входящего апдейта
    crate::core::update_user_state(&config, user_id, sent.id.0, &payload_str).await;
    Ok(())
}

// --- ОБРАБОТЧИКИ ДИАЛОГОВ ---

pub async fn handle_custom_interval(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (device_id, room_id): (i64, i64),
) -> Result<()> {
    let text = msg.text().unwrap_or("").trim();

    // Пытаемся распарсить ввод
    if let Ok(hours) = text.parse::<u32>() {
        let new_payload = Payload::Control(crate::bot::router::ControlPayload::QuickAction {
            room: room_id,
            device: device_id,
            cmd: crate::bot::router::DeviceCmd::ShowChart { h: hours, o: 0 },
        });

        // Завершаем диалог с ПЕРЕХОДОМ на новый график
        return finalize_dialogue(bot, dialogue, msg, config, Some(new_payload)).await;
    }

    // Если ввод невалиден - уведомляем и выходим со старым контекстом
    let err_msg = bot.send_message(msg.chat.id, "⚠️ Ошибка: введите целое число часов.").await?;
    crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);

    finalize_dialogue(bot, dialogue, msg, config, None).await
}

/// Завершает диалог, очищает чат и обновляет интерфейс.
/// Соответствует Google Style Guide: инкапсуляция побочных эффектов и атомарная работа с памятью.
async fn finalize_dialogue(
    bot: Bot,
    dialog_manager: MyDialogue,
    incoming_msg: Message,
    app_config: Arc<AppConfig>,
    explicit_payload: Option<Payload>, // Новый контекст (если есть)
) -> Result<()> {
    let user_id = incoming_msg.from.as_ref().context("User context missing")?.id.0;
    let chat_id = incoming_msg.chat.id;

    // 1. Сбрасываем состояние диалога в Telegram
    dialog_manager.exit().await?;

    // 2. Удаляем сообщение пользователя (Cleanup)
    let _ = bot.delete_message(chat_id, incoming_msg.id).await;

    // 3. БЕЗОПАСНОЕ ИЗВЛЕЧЕНИЕ ДАННЫХ (Scoped Lock)
    // Мы ограничиваем время жизни блокировки DashMap этим блоком { }
    let (message_id, context_str) = {
        let session = app_config.sessions.get(&user_id)
            .context("Session expired during input")?;

        let mid = MessageId(session.last_menu_id);

        // Если передан новый payload - используем его, иначе берем старый из базы
        let ctx = match explicit_payload {
            Some(p) => p.to_string(),
            None => session.current_context.clone(),
        };

        (mid, ctx)
    }; // <-- Блокировка DashMap автоматически снимается ЗДЕСЬ (Drop)

    // 4. Обновляем UI (Теперь .await безопасен, так как лок отпущен)
    render_current_view(&bot, &app_config, user_id, chat_id, message_id, &context_str).await?;

    Ok(())
}