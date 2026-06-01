use anyhow::{Context, Result};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::macros::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{InputFile, InputMedia, InputMediaPhoto, MessageId, ParseMode};
use teloxide::{Bot, RequestError};

use super::models::View;
use crate::bot::router::{router, CameraPayload, Payload};
use crate::bot::State;
use crate::db;
use crate::models::AppConfig;

pub type MyDialogue = Dialogue<State, InMemStorage<State>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditViewResult {
    Updated,
    NotModified,
    MessageMissing,
}

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
                let _ = bot
                    .delete_message(chat_id, MessageId(session.last_menu_id))
                    .await;
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
    let payload = Payload::from_string(data).context("Critical: Binary payload decoding failed")?;

    let payload =
        handle_camera_media_action(&bot, msg.chat().id, user_id, payload, &config).await?;

    // 3. Роутинг
    let view = router(payload, user_id, config.clone()).await?;

    // 4. Оркестрация UI и State
    apply_view(
        &bot,
        &config,
        &dialogue,
        msg.chat().id,
        msg.id(),
        user_id,
        view,
    )
    .await
}

async fn handle_camera_media_action(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    payload: Payload,
    config: &Arc<AppConfig>,
) -> Result<Payload> {
    match payload {
        Payload::Camera(CameraPayload::Snapshot { id }) => {
            send_camera_snapshot(bot, chat_id, user_id, id, config).await?;
            Ok(Payload::Camera(CameraPayload::CameraDetail { id }))
        }
        Payload::Camera(CameraPayload::Clip { id, seconds }) => {
            send_camera_clip(bot, chat_id, user_id, id, seconds, config).await?;
            Ok(Payload::Camera(CameraPayload::CameraDetail { id }))
        }
        payload => Ok(payload),
    }
}

async fn send_camera_snapshot(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    config: &Arc<AppConfig>,
) -> Result<()> {
    let Some(camera) = db::cameras::get_accessible_camera(
        user_id,
        config.root_user == user_id,
        camera_id,
        &config.db,
    )
    .await?
    else {
        bot.send_message(chat_id, "Недостаточно прав для просмотра камеры.")
            .await?;
        return Ok(());
    };

    let bytes = match crate::core::cameras::capture_snapshot(&camera).await {
        Ok(bytes) => bytes,
        Err(error) => {
            bot.send_message(chat_id, format!("Не удалось получить снимок: {}", error))
                .await?;
            return Ok(());
        }
    };

    bot.send_photo(chat_id, InputFile::memory(bytes))
        .caption(format!("📸 {}", camera.name))
        .await?;

    Ok(())
}

async fn send_camera_clip(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    seconds: u32,
    config: &Arc<AppConfig>,
) -> Result<()> {
    let Some(camera) = db::cameras::get_accessible_camera(
        user_id,
        config.root_user == user_id,
        camera_id,
        &config.db,
    )
    .await?
    else {
        bot.send_message(chat_id, "Недостаточно прав для просмотра камеры.")
            .await?;
        return Ok(());
    };

    let seconds = seconds.clamp(1, 120);
    let status = bot
        .send_message(
            chat_id,
            format!("Готовлю видео {}с с камеры «{}»...", seconds, camera.name),
        )
        .await?;

    let bytes = match crate::core::cameras::capture_clip(&camera, seconds).await {
        Ok(bytes) => bytes,
        Err(error) => {
            bot.edit_message_text(
                chat_id,
                status.id,
                format!("Не удалось записать видео: {}", error),
            )
            .await?;
            return Ok(());
        }
    };

    bot.send_video(chat_id, InputFile::memory(bytes))
        .caption(format!("🎞 {} · {}с", camera.name, seconds))
        .await?;
    let _ = bot.delete_message(chat_id, status.id).await;

    Ok(())
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

/// Фоновое live-обновление: только редактирует существующее сообщение.
/// Если сообщение удалено, сессия считается устаревшей и очищается.
pub async fn refresh_current_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user_id: u64,
    chat_id: ChatId,
    message_id: MessageId,
    context: &str,
) -> Result<()> {
    if !is_current_session(config, user_id, message_id, context) {
        log::debug!(
            "Skip stale refresh before render for user {} context {}",
            user_id,
            context
        );
        return Ok(());
    }

    let payload = Payload::from_string(context).context("Context decoding failed")?;
    let view = router(payload, user_id, config.clone()).await?;
    let payload_str = view.payload.to_string();
    let text = view.get_text();
    let ui_lock = config.ui_lock_for(user_id);
    let _guard = ui_lock.lock().await;

    if !is_current_session(config, user_id, message_id, context) {
        log::debug!(
            "Skip stale refresh before edit for user {} context {}",
            user_id,
            context
        );
        return Ok(());
    }

    match edit_existing_view(bot, chat_id, message_id, &view, &text).await {
        Ok(EditViewResult::Updated) => {
            crate::core::update_user_state(config, user_id, message_id.0, &payload_str).await;
            Ok(())
        }
        Ok(EditViewResult::NotModified) => Ok(()),
        Ok(EditViewResult::MessageMissing) => {
            log::warn!(
                "Live refresh target message is missing for user {}. Clearing session.",
                user_id
            );
            clear_ghost_session(config, user_id).await;
            Ok(())
        }
        Err(RequestError::RetryAfter(retry_after)) => {
            let blocked_until =
                block_user_ui_refresh(config, user_id, retry_after.chrono_duration());
            log_telegram_retry_after(
                "Live refresh",
                user_id,
                retry_after.seconds(),
                blocked_until,
            );
            Ok(())
        }
        Err(e) => {
            log_telegram_refresh_error("Live refresh", user_id, &e);
            Err(e.into())
        }
    }
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
    let ui_lock = config.ui_lock_for(user_id);
    let _guard = ui_lock.lock().await;
    let text = view.get_text();
    let payload_str = view.payload.to_string();

    match edit_existing_view(bot, chat_id, message_id, &view, &text).await {
        Ok(EditViewResult::Updated) => {
            crate::core::update_user_state(&config, user_id, message_id.0, &payload_str).await;
            Ok(())
        }
        Ok(EditViewResult::NotModified) => Ok(()),
        Ok(EditViewResult::MessageMissing) => {
            log::warn!(
                "Detected ghost message for user {}. Re-anchoring UI.",
                user_id
            );
            reanchor_view(bot, chat_id, message_id, user_id, view, config).await
        }
        Err(e @ RequestError::RetryAfter(_)) => {
            log_telegram_refresh_error("UI update", user_id, &e);
            Err(e.into())
        }
        Err(e) => {
            log::info!("UI Mode transition for user {}: {}", user_id, e);
            reanchor_view(bot, chat_id, message_id, user_id, view, config).await
        }
    }
}

fn is_current_session(
    config: &Arc<AppConfig>,
    user_id: u64,
    message_id: MessageId,
    context: &str,
) -> bool {
    config.sessions.get(&user_id).is_some_and(|session| {
        session.last_menu_id == message_id.0 && session.current_context == context
    })
}

async fn edit_existing_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    view: &View,
    text: &str,
) -> std::result::Result<EditViewResult, RequestError> {
    let kb = view.kb.clone();
    let input_file = match &view.image {
        Some(v) => InputFile::memory(v.clone()),
        None => InputFile::memory(crate::bot::utils::UI_PLACEHOLDER_BYTES),
    };

    let media = InputMedia::Photo(
        InputMediaPhoto::new(input_file)
            .caption(text)
            .parse_mode(ParseMode::MarkdownV2),
    );

    let res = bot
        .edit_message_media(chat_id, message_id, media)
        .reply_markup(kb)
        .await;

    match res {
        Ok(_) => Ok(EditViewResult::Updated),
        Err(RequestError::Api(teloxide::ApiError::MessageNotModified)) => {
            Ok(EditViewResult::NotModified)
        }
        Err(e) if is_missing_message_error(&e) => Ok(EditViewResult::MessageMissing),
        Err(e) => Err(e),
    }
}

async fn reanchor_view(
    bot: &Bot,
    chat_id: ChatId,
    old_message_id: MessageId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
) -> Result<()> {
    send_new_view(bot, chat_id, user_id, view, config).await?;

    let b = bot.clone();
    tokio::spawn(async move {
        let _ = b.delete_message(chat_id, old_message_id).await;
    });

    Ok(())
}

async fn clear_ghost_session(config: &Arc<AppConfig>, user_id: u64) {
    config.sessions.remove(&user_id);
    config.ui_locks.remove(&user_id);

    if let Err(e) = db::clear_user_session(user_id, &config.db).await {
        log::error!("Failed to clear ghost session for user {}: {}", user_id, e);
    }
}

fn block_user_ui_refresh(
    config: &Arc<AppConfig>,
    user_id: u64,
    retry_after: chrono::Duration,
) -> DateTime<Utc> {
    let extra_delay_s =
        i64::try_from(config.telegram_retry_after_extra_delay_s).unwrap_or(i64::MAX);
    let blocked_until = Utc::now() + retry_after + chrono::Duration::seconds(extra_delay_s);

    if let Some(mut session) = config.sessions.get_mut(&user_id) {
        session.ui_refresh_blocked_until = Some(blocked_until);
    }

    blocked_until
}

fn is_missing_message_error(error: &RequestError) -> bool {
    matches!(
        error,
        RequestError::Api(teloxide::ApiError::MessageToEditNotFound)
            | RequestError::Api(teloxide::ApiError::MessageIdInvalid)
    )
}

fn log_telegram_refresh_error(context: &str, user_id: u64, error: &RequestError) {
    match error {
        RequestError::RetryAfter(retry_after) => {
            log::warn!(
                "{} for user {} hit Telegram rate limit: retry after {}s",
                context,
                user_id,
                retry_after.seconds()
            );
        }
        _ => log::warn!("{} for user {} failed: {}", context, user_id, error),
    }
}

fn log_telegram_retry_after(
    context: &str,
    user_id: u64,
    retry_after_s: u32,
    blocked_until: DateTime<Utc>,
) {
    log::warn!(
        "{} for user {} hit Telegram rate limit: retry after {}s, UI refresh blocked until {}",
        context,
        user_id,
        retry_after_s,
        blocked_until.to_rfc3339()
    );
}

async fn send_new_view(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
) -> Result<()> {
    let text = view.get_text();
    let payload_str = view.payload.to_string();

    // Исправлено: send_photo принимает InputFile, а не InputMedia
    let input_file = match view.image {
        Some(v) => InputFile::memory(v),
        None => InputFile::memory(crate::bot::utils::UI_PLACEHOLDER_BYTES),
    };

    let sent = bot
        .send_photo(chat_id, input_file)
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
    let err_msg = bot
        .send_message(msg.chat.id, "⚠️ Ошибка: введите целое число часов.")
        .await?;
    crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);

    finalize_dialogue(bot, dialogue, msg, config, None).await
}

pub async fn handle_add_user_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    match parse_user_id(&msg) {
        Ok(user_id) => {
            crate::db::add_user(user_id, &config.db).await?;
            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(crate::bot::router::AdminPayload::ListUsers)),
            )
            .await
        }
        Err(text) => {
            let err_msg = bot.send_message(msg.chat.id, text).await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);
            finalize_dialogue(bot, dialogue, msg, config, None).await
        }
    }
}

pub async fn handle_delete_user_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    match parse_user_id(&msg) {
        Ok(user_id) if user_id == config.root_user => {
            let err_msg = bot
                .send_message(msg.chat.id, "Root пользователя нельзя удалить.")
                .await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);
            finalize_dialogue(bot, dialogue, msg, config, None).await
        }
        Ok(user_id) => {
            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(
                    crate::bot::router::AdminPayload::ConfirmDeleteUser { id: user_id },
                )),
            )
            .await
        }
        Err(text) => {
            let err_msg = bot.send_message(msg.chat.id, text).await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);
            finalize_dialogue(bot, dialogue, msg, config, None).await
        }
    }
}

pub async fn handle_add_camera_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    room_id: i64,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    match parse_camera_input(msg.text().unwrap_or(""), config.camera_default_clip_s) {
        Ok(input) => {
            crate::db::cameras::add_manual_camera(
                crate::db::cameras::NewCamera {
                    name: &input.name,
                    room_id,
                    stream_url: &input.stream_url,
                    snapshot_url: input.snapshot_url.as_deref(),
                    clip_seconds: input.clip_seconds,
                },
                &config.db,
            )
            .await?;

            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(
                    crate::bot::router::AdminPayload::RoomCameras { room: room_id },
                )),
            )
            .await
        }
        Err(text) => {
            let err_msg = bot.send_message(msg.chat.id, text).await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 8);
            finalize_dialogue(bot, dialogue, msg, config, None).await
        }
    }
}

fn parse_user_id(msg: &Message) -> std::result::Result<u64, &'static str> {
    let text = msg.text().unwrap_or("").trim();
    text.parse::<u64>()
        .map_err(|_| "Ошибка: введите числовой Telegram ID.")
        .and_then(|id| {
            if id == 0 {
                Err("Ошибка: Telegram ID не может быть 0.")
            } else {
                Ok(id)
            }
        })
}

#[derive(Debug)]
struct CameraInput {
    name: String,
    stream_url: String,
    snapshot_url: Option<String>,
    clip_seconds: u32,
}

fn parse_camera_input(
    text: &str,
    default_clip_seconds: u32,
) -> std::result::Result<CameraInput, String> {
    let parts: Vec<String> = text
        .lines()
        .flat_map(|line| line.split(';'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if parts.len() < 2 {
        return Err(camera_input_help(default_clip_seconds));
    }

    let name = parts[0].clone();
    let stream_url = parts[1].clone();

    if name.len() > 80 {
        return Err("Название камеры должно быть не длиннее 80 символов.".to_string());
    }

    if !is_supported_stream_url(&stream_url) {
        return Err(
            "RTSP URL должен начинаться с rtsp://, rtsps://, http:// или https://.".to_string(),
        );
    }

    let clip_seconds = match parts.get(2) {
        Some(value) => value
            .parse::<u32>()
            .map_err(|_| "Интервал видео должен быть числом секунд.".to_string())?,
        None => default_clip_seconds,
    };

    if !(1..=120).contains(&clip_seconds) {
        return Err("Интервал видео должен быть от 1 до 120 секунд.".to_string());
    }

    let snapshot_url = parts.get(3).cloned();
    if let Some(snapshot_url) = snapshot_url.as_deref() {
        if !is_supported_snapshot_url(snapshot_url) {
            return Err("Snapshot URL должен начинаться с http:// или https://.".to_string());
        }
    }

    Ok(CameraInput {
        name,
        stream_url,
        snapshot_url,
        clip_seconds,
    })
}

fn is_supported_stream_url(value: &str) -> bool {
    value.starts_with("rtsp://")
        || value.starts_with("rtsps://")
        || value.starts_with("http://")
        || value.starts_with("https://")
}

fn is_supported_snapshot_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn camera_input_help(default_clip_seconds: u32) -> String {
    format!(
        "Ошибка: введите минимум название и RTSP URL.\n\nФормат:\nНазвание\nRTSP URL\nИнтервал видео в секундах\nSnapshot URL необязательно\n\nПример:\nВход\nrtsp://user:pass@192.168.1.50:554/stream1\n{}",
        default_clip_seconds
    )
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
    let user_id = incoming_msg
        .from
        .as_ref()
        .context("User context missing")?
        .id
        .0;
    let chat_id = incoming_msg.chat.id;

    // 1. Сбрасываем состояние диалога в Telegram
    dialog_manager.exit().await?;

    // 2. Удаляем сообщение пользователя (Cleanup)
    let _ = bot.delete_message(chat_id, incoming_msg.id).await;

    // 3. БЕЗОПАСНОЕ ИЗВЛЕЧЕНИЕ ДАННЫХ (Scoped Lock)
    // Мы ограничиваем время жизни блокировки DashMap этим блоком { }
    let (message_id, context_str) = {
        let session = app_config
            .sessions
            .get(&user_id)
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
    render_current_view(
        &bot,
        &app_config,
        user_id,
        chat_id,
        message_id,
        &context_str,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_message_errors_are_classified_as_ghost_sessions() {
        assert!(is_missing_message_error(&RequestError::Api(
            teloxide::ApiError::MessageToEditNotFound,
        )));
        assert!(is_missing_message_error(&RequestError::Api(
            teloxide::ApiError::MessageIdInvalid,
        )));
        assert!(!is_missing_message_error(&RequestError::Api(
            teloxide::ApiError::MessageNotModified,
        )));
    }

    #[test]
    fn camera_input_accepts_multiline_format() {
        let input = parse_camera_input(
            "Вход\nrtsp://user:pass@192.168.1.50:554/stream1\n15\nhttps://ha.local/snapshot.jpg",
            10,
        )
        .expect("camera input should parse");

        assert_eq!(input.name, "Вход");
        assert_eq!(
            input.stream_url,
            "rtsp://user:pass@192.168.1.50:554/stream1"
        );
        assert_eq!(input.clip_seconds, 15);
        assert_eq!(
            input.snapshot_url.as_deref(),
            Some("https://ha.local/snapshot.jpg")
        );
    }

    #[test]
    fn camera_input_uses_default_clip_seconds() {
        let input = parse_camera_input("Гараж; rtsp://192.168.1.51/live", 10)
            .expect("camera input should parse");

        assert_eq!(input.name, "Гараж");
        assert_eq!(input.clip_seconds, 10);
        assert!(input.snapshot_url.is_none());
    }

    #[test]
    fn camera_input_rejects_invalid_interval() {
        let error = parse_camera_input("Гараж\nrtsp://192.168.1.51/live\n300", 10)
            .expect_err("invalid interval should fail");

        assert!(error.contains("от 1 до 120"));
    }
}
