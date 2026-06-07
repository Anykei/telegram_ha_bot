use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::macros::BotCommands;
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, InputFile, InputMedia, InputMediaPhoto, MessageId,
    ParseMode,
};
use teloxide::{Bot, RequestError};

use super::models::View;
use crate::bot::router::{
    router, AdminPayload, CameraPayload, CommandPayload, Payload, RecordingRuleEditField,
};
use crate::bot::State;
use crate::core::commands::{CommandExecution, CommandSource};
use crate::db;
use crate::models::AppConfig;
use crate::models::UiMessageMode;

pub type MyDialogue = Dialogue<State, InMemStorage<State>>;

const TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES: u64 = 50_000_000;
const TELEGRAM_MIN_COMPRESSED_VIDEO_BYTES: u64 = 500_000;
const RECORDING_VIDEO_MESSAGE_TTL_SECONDS: u64 = 60;
const TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS: usize = 1024;
const TELEGRAM_TEXT_MESSAGE_LIMIT_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditViewResult {
    Updated(UiMessageMode),
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

pub async fn handle_idle_text(bot: Bot, msg: Message, config: Arc<AppConfig>) -> Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let user_id = user.id.0;
    let chat_id = msg.chat.id;
    let text = msg.text().unwrap_or("").trim();

    if text.is_empty() {
        let _ = bot.delete_message(chat_id, msg.id).await;
        return Ok(());
    }

    match handle_text_command(&bot, chat_id, user_id, text, config.clone()).await {
        Ok(true) => {
            let _ = bot.delete_message(chat_id, msg.id).await;
        }
        Ok(false) => {
            log::info!(
                "Ignored junk message from user {}: {:?}",
                msg.chat.id,
                msg.text()
            );
            let _ = bot.delete_message(chat_id, msg.id).await;
        }
        Err(error) => {
            let error = error.to_string();
            let _ = db::activity_log::log(
                db::activity_log::NewActivity {
                    user_id: Some(user_id),
                    kind: "text_command",
                    entity_type: "command",
                    entity_id: None,
                    action: "parse",
                    status: "error",
                    message: Some(&error),
                },
                &config.db,
            )
            .await;
            let sent = bot
                .send_message(chat_id, format!("Не удалось выполнить команду: {}", error))
                .await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 10);
            let _ = bot.delete_message(chat_id, msg.id).await;
        }
    }

    Ok(())
}

pub async fn handle_text_command_message(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
) -> Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let user_id = user.id.0;
    let chat_id = msg.chat.id;
    let text = msg.text().unwrap_or("").trim();

    if text.is_empty() {
        let _ = bot.delete_message(chat_id, msg.id).await;
        return Ok(());
    }

    match handle_text_command(&bot, chat_id, user_id, text, config.clone()).await {
        Ok(true) => {
            let _ = bot.delete_message(chat_id, msg.id).await;
        }
        Ok(false) => {
            let sent = bot
                .send_message(
                    chat_id,
                    "Команда не распознана. Примеры: свет коридор вкл, включи свет в коридоре, снимок вход.",
                )
                .await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 12);
            let _ = bot.delete_message(chat_id, msg.id).await;
        }
        Err(error) => {
            let error = error.to_string();
            let _ = db::activity_log::log(
                db::activity_log::NewActivity {
                    user_id: Some(user_id),
                    kind: "text_command",
                    entity_type: "command",
                    entity_id: None,
                    action: "parse",
                    status: "error",
                    message: Some(&error),
                },
                &config.db,
            )
            .await;
            let sent = bot
                .send_message(chat_id, format!("Не удалось выполнить команду: {}", error))
                .await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 10);
            let _ = bot.delete_message(chat_id, msg.id).await;
        }
    }

    Ok(())
}

pub(crate) fn looks_like_text_command(text: &str) -> bool {
    let normalized = crate::bot::text_commands::normalize_command_text(text);
    normalized.starts_with("снимок ")
        || normalized.starts_with("видео ")
        || normalized.starts_with("архив ")
        || crate::bot::text_commands::parse_device_text_command(&normalized).is_some()
}

pub async fn handle_voice_message(bot: Bot, msg: Message, config: Arc<AppConfig>) -> Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let user_id = user.id.0;
    let chat_id = msg.chat.id;

    spawn_voice_command(bot.clone(), msg.clone(), user_id, chat_id, config);
    let _ = bot.delete_message(chat_id, msg.id).await;
    Ok(())
}

fn spawn_voice_command(
    bot: Bot,
    msg: Message,
    user_id: u64,
    chat_id: ChatId,
    config: Arc<AppConfig>,
) {
    tokio::spawn(async move {
        let result = handle_voice_message_inner(&bot, &msg, user_id, chat_id, config.clone()).await;
        if let Err(error) = result {
            handle_voice_command_error(&bot, chat_id, user_id, error, &config).await;
        }
    });
}

async fn handle_voice_command_error(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    error: anyhow::Error,
    config: &Arc<AppConfig>,
) {
    let error = error.to_string();
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(user_id),
            kind: "voice_command",
            entity_type: "command",
            entity_id: None,
            action: "voice",
            status: "error",
            message: Some(&error),
        },
        &config.db,
    )
    .await;
    let sent = bot
        .send_message(
            chat_id,
            format!("Не удалось выполнить голосовую команду: {}", error),
        )
        .await;
    if let Ok(sent) = sent {
        crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 12);
    }
}

async fn handle_voice_message_inner(
    bot: &Bot,
    msg: &Message,
    user_id: u64,
    chat_id: ChatId,
    config: Arc<AppConfig>,
) -> Result<()> {
    if !config.voice_enabled {
        anyhow::bail!("голосовые команды выключены");
    }

    if !db::access::can_use_voice(user_id, config.root_user == user_id, &config.db).await? {
        anyhow::bail!("голосовые команды выключены для пользователя");
    }

    if config.voice_stt_provider != crate::options::VoiceSttProvider::HaPipeline {
        anyhow::bail!("выбранный STT provider пока не реализован");
    }
    let response_format = config.voice_response_format;

    let voice = msg.voice().context("voice-сообщение не найдено")?;
    if voice.duration.seconds() > config.voice_max_audio_duration_s {
        anyhow::bail!(
            "voice слишком длинный: максимум {}с",
            config.voice_max_audio_duration_s
        );
    }

    let max_bytes = config.voice_max_audio_size_mb.saturating_mul(1024 * 1024);
    if u64::from(voice.file.size) > max_bytes {
        anyhow::bail!(
            "voice слишком большой: максимум {} MB",
            config.voice_max_audio_size_mb
        );
    }

    let file = bot.get_file(voice.file.id.clone()).await?;
    if u64::from(file.size) > max_bytes {
        anyhow::bail!(
            "voice слишком большой: максимум {} MB",
            config.voice_max_audio_size_mb
        );
    }

    let path = temp_voice_path(user_id);
    let mut dst = tokio::fs::File::create(&path)
        .await
        .context("Не удалось создать временный voice-файл")?;
    bot.download_file(&file.path, &mut dst).await?;
    drop(dst);

    let progress = bot
        .send_message(
            chat_id,
            format!(
                "⏳ Распознаю голосовую команду... таймаут {}с",
                config.voice_stt_timeout_s
            ),
        )
        .await?;
    crate::bot::utils::spawn_delayed_delete(
        bot.clone(),
        chat_id,
        progress.id,
        config.voice_stt_timeout_s + 10,
    );

    let result = async {
        let metadata = tokio::fs::metadata(&path)
            .await
            .context("Не удалось проверить скачанный voice-файл")?;
        if metadata.len() > max_bytes {
            anyhow::bail!(
                "voice слишком большой: максимум {} MB",
                config.voice_max_audio_size_mb
            );
        }

        let pcm = crate::core::voice::decode_audio_file_to_pcm_mono(
            path.clone(),
            config.voice_stt_sample_rate,
        )
        .await?;
        let estimated =
            crate::core::voice::estimated_pcm_duration(pcm.len(), config.voice_stt_sample_rate);
        if estimated.as_secs() > u64::from(config.voice_max_audio_duration_s) + 1 {
            anyhow::bail!(
                "voice слишком длинный: максимум {}с",
                config.voice_max_audio_duration_s
            );
        }

        let recognized = crate::ha::assist_pipeline::transcribe_pcm(
            &config.ha_url,
            &config.ha_token,
            config.voice_ha_pipeline_id.as_deref(),
            config.voice_stt_sample_rate,
            config.voice_stt_timeout_s,
            &pcm,
        )
        .await?;

        if matches!(
            response_format,
            crate::options::VoiceResponseFormat::Voice | crate::options::VoiceResponseFormat::Both
        ) {
            log::debug!("Voice/TTS response format is configured; MVP sends text responses");
        }

        if config.voice_show_recognized_text {
            let sent = bot
                .send_message(chat_id, format!("Распознано: {}", recognized))
                .await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 12);
        }

        let execution = crate::core::commands::execute_voice_text(
            user_id,
            config.root_user == user_id,
            &recognized,
            &config,
        )
        .await?;

        if matches!(execution, CommandExecution::NotACommand) {
            anyhow::bail!("команда не распознана: {}", recognized);
        }

        apply_command_execution(bot, chat_id, user_id, execution, config.clone()).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    let _ = tokio::fs::remove_file(&path).await;
    result
}

fn temp_voice_path(user_id: u64) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "telegram_ha_bot_voice_{}_{}.oga",
        user_id,
        Utc::now().timestamp_millis()
    ))
}

async fn handle_text_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    text: &str,
    config: Arc<AppConfig>,
) -> Result<bool> {
    let execution = crate::core::commands::execute_text(
        user_id,
        config.root_user == user_id,
        CommandSource::Text,
        text,
        &config,
    )
    .await?;

    if matches!(execution, CommandExecution::NotACommand) {
        return Ok(false);
    }

    apply_command_execution(bot, chat_id, user_id, execution, config).await?;
    Ok(true)
}

async fn apply_command_execution(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    execution: CommandExecution,
    config: Arc<AppConfig>,
) -> Result<()> {
    match execution {
        CommandExecution::Done { message } => {
            let sent = bot.send_message(chat_id, message).await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 8);
        }
        CommandExecution::NeedsConfirmation {
            pending_id,
            message,
        } => {
            let kb = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback(
                    "✅ Подтвердить",
                    Payload::Command(CommandPayload::Confirm { id: pending_id }).to_string(),
                ),
                InlineKeyboardButton::callback(
                    "✖️ Отмена",
                    Payload::Command(CommandPayload::Cancel { id: pending_id }).to_string(),
                ),
            ]]);
            bot.send_message(chat_id, message).reply_markup(kb).await?;
        }
        CommandExecution::OpenCameraArchive { camera_id } => {
            let view = router(
                Payload::Camera(CameraPayload::RecordingArchive { camera: camera_id }),
                user_id,
                config.clone(),
            )
            .await?;
            let message_id = config
                .sessions
                .get(&user_id)
                .map(|session| MessageId(session.last_menu_id));
            if let Some(message_id) = message_id {
                update_view(bot, chat_id, message_id, user_id, view, config.clone()).await?;
            } else {
                send_new_view(bot, chat_id, user_id, view, config.clone()).await?;
            }
        }
        CommandExecution::SendCameraSnapshot { camera_id } => {
            spawn_camera_snapshot(bot.clone(), chat_id, user_id, camera_id, config);
        }
        CommandExecution::SendCameraClip { camera_id, seconds } => {
            spawn_camera_clip(bot.clone(), chat_id, user_id, camera_id, seconds, config);
        }
        CommandExecution::NotACommand => {}
    }

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

    if let Payload::Command(command_payload) = payload {
        handle_command_callback(
            &bot,
            msg.chat().id,
            msg.id(),
            user_id,
            command_payload,
            config,
        )
        .await?;
        return Ok(());
    }

    let payload = handle_camera_media_action(&bot, msg.chat().id, user_id, payload, &config)?;

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

async fn handle_command_callback(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    payload: CommandPayload,
    config: Arc<AppConfig>,
) -> Result<()> {
    match payload {
        CommandPayload::Confirm { id } => {
            let execution = crate::core::commands::confirm_pending(
                user_id,
                config.root_user == user_id,
                id,
                &config,
            )
            .await;

            let _ = bot.delete_message(chat_id, message_id).await;

            match execution {
                Ok(execution) => {
                    apply_command_execution(bot, chat_id, user_id, execution, config).await?;
                }
                Err(error) => {
                    let sent = bot
                        .send_message(chat_id, format!("Не удалось выполнить команду: {}", error))
                        .await?;
                    crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 10);
                }
            }
        }
        CommandPayload::Cancel { id } => {
            let _ = crate::core::commands::cancel_pending(user_id, id, &config).await;
            let _ = bot.delete_message(chat_id, message_id).await;
            let sent = bot.send_message(chat_id, "Команда отменена").await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), chat_id, sent.id, 5);
        }
    }

    Ok(())
}

fn handle_camera_media_action(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    payload: Payload,
    config: &Arc<AppConfig>,
) -> Result<Payload> {
    match payload {
        Payload::Camera(CameraPayload::Snapshot { id }) => {
            spawn_camera_snapshot(bot.clone(), chat_id, user_id, id, config.clone());
            Ok(Payload::Camera(CameraPayload::CameraDetail { id }))
        }
        Payload::Camera(CameraPayload::Clip { id, seconds }) => {
            spawn_camera_clip(bot.clone(), chat_id, user_id, id, seconds, config.clone());
            Ok(Payload::Camera(CameraPayload::CameraDetail { id }))
        }
        Payload::Camera(CameraPayload::SendRecordingSegment {
            camera,
            session,
            segment,
        }) => {
            spawn_recording_segment(
                bot.clone(),
                chat_id,
                user_id,
                camera,
                session,
                segment,
                config.clone(),
            );
            Ok(Payload::Camera(CameraPayload::RecordingSession {
                camera,
                session,
            }))
        }
        Payload::Camera(CameraPayload::SendRecordingAll { camera, session }) => {
            if config.start_recording_send(user_id, session) {
                spawn_recording_all(
                    bot.clone(),
                    chat_id,
                    user_id,
                    camera,
                    session,
                    config.clone(),
                );
            }
            Ok(Payload::Camera(CameraPayload::RecordingSession {
                camera,
                session,
            }))
        }
        Payload::Admin(AdminPayload::RoomCameraSnapshot { room, camera }) => {
            if user_id == config.root_user {
                spawn_camera_snapshot(bot.clone(), chat_id, user_id, camera, config.clone());
            }
            Ok(Payload::Admin(AdminPayload::RoomCameraDetail {
                room,
                camera,
            }))
        }
        Payload::Admin(AdminPayload::RoomCameraClip {
            room,
            camera,
            seconds,
        }) => {
            if user_id == config.root_user {
                spawn_camera_clip(
                    bot.clone(),
                    chat_id,
                    user_id,
                    camera,
                    seconds,
                    config.clone(),
                );
            }
            Ok(Payload::Admin(AdminPayload::RoomCameraDetail {
                room,
                camera,
            }))
        }
        payload => Ok(payload),
    }
}

fn spawn_camera_snapshot(
    bot: Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    config: Arc<AppConfig>,
) {
    tokio::spawn(async move {
        if let Err(error) = send_camera_snapshot(&bot, chat_id, user_id, camera_id, &config).await {
            log::warn!(
                "Failed to send camera snapshot for user {} camera {}: {}",
                user_id,
                camera_id,
                error
            );
        }
    });
}

fn spawn_camera_clip(
    bot: Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    seconds: u32,
    config: Arc<AppConfig>,
) {
    tokio::spawn(async move {
        if let Err(error) =
            send_camera_clip(&bot, chat_id, user_id, camera_id, seconds, &config).await
        {
            log::warn!(
                "Failed to send camera clip for user {} camera {}: {}",
                user_id,
                camera_id,
                error
            );
        }
    });
}

pub fn spawn_camera_health_check(user_id: u64, camera_id: i64, config: Arc<AppConfig>) {
    tokio::spawn(async move {
        let Some(camera) = (match db::cameras::get_camera(camera_id, &config.db).await {
            Ok(camera) => camera,
            Err(error) => {
                log::warn!(
                    "Failed to load camera {} for health check: {}",
                    camera_id,
                    error
                );
                return;
            }
        }) else {
            return;
        };

        match crate::core::cameras::capture_snapshot(&camera).await {
            Ok(bytes) => {
                let _ = db::camera_health::mark_check_ok(camera.id, bytes.len() as i64, &config.db)
                    .await;
                let camera_id = camera.id.to_string();
                let _ = log_activity(
                    &config,
                    Some(user_id),
                    "camera",
                    "camera",
                    Some(&camera_id),
                    "health_check",
                    "ok",
                    Some(&camera.name),
                )
                .await;
            }
            Err(error) => {
                let error = error.to_string();
                let _ = db::camera_health::mark_error(camera.id, &error, &config.db).await;
                let camera_id = camera.id.to_string();
                let _ = log_activity(
                    &config,
                    Some(user_id),
                    "camera",
                    "camera",
                    Some(&camera_id),
                    "health_check",
                    "error",
                    Some(&error),
                )
                .await;
            }
        }
    });
}

fn spawn_recording_segment(
    bot: Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    session_id: i64,
    segment_id: i64,
    config: Arc<AppConfig>,
) {
    tokio::spawn(async move {
        if let Err(error) = send_recording_segment(
            &bot, chat_id, user_id, camera_id, session_id, segment_id, &config,
        )
        .await
        {
            log::warn!(
                "Failed to send recording segment {} for user {}: {}",
                segment_id,
                user_id,
                error
            );
        }
    });
}

fn spawn_recording_all(
    bot: Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    session_id: i64,
    config: Arc<AppConfig>,
) {
    tokio::spawn(async move {
        let result =
            send_recording_all(&bot, chat_id, user_id, camera_id, session_id, &config).await;
        config.finish_recording_send(user_id, session_id);

        if let Err(error) = result {
            log::warn!(
                "Failed to send recording session {} for user {}: {}",
                session_id,
                user_id,
                error
            );
        }

        refresh_recording_session_view(&bot, chat_id, user_id, camera_id, session_id, &config)
            .await;
    });
}

async fn refresh_recording_session_view(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    session_id: i64,
    config: &Arc<AppConfig>,
) {
    let context = Payload::Camera(CameraPayload::RecordingSession {
        camera: camera_id,
        session: session_id,
    })
    .to_string();
    let message_id = config
        .sessions
        .get(&user_id)
        .filter(|session| session.current_context == context)
        .map(|session| MessageId(session.last_menu_id));

    let Some(message_id) = message_id else {
        return;
    };

    if let Err(error) =
        refresh_current_view(bot, config, user_id, chat_id, message_id, &context).await
    {
        log::warn!(
            "Failed to refresh recording session {} after send for user {}: {}",
            session_id,
            user_id,
            error
        );
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
        Ok(bytes) => {
            let _ = db::camera_health::mark_snapshot_ok(camera.id, bytes.len() as i64, &config.db)
                .await;
            let camera_id = camera.id.to_string();
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "snapshot_capture",
                "ok",
                Some(&camera.name),
            )
            .await;
            bytes
        }
        Err(error) => {
            let error = error.to_string();
            let _ = db::camera_health::mark_error(camera.id, &error, &config.db).await;
            let camera_id = camera.id.to_string();
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "snapshot",
                "error",
                Some(&error),
            )
            .await;
            bot.send_message(chat_id, format!("Не удалось получить снимок: {}", error))
                .await?;
            return Ok(());
        }
    };

    match bot
        .send_photo(chat_id, InputFile::memory(bytes))
        .caption(format!("📸 {}", camera.name))
        .await
    {
        Ok(_) => {
            let camera_id = camera.id.to_string();
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "snapshot_send",
                "ok",
                Some(&camera.name),
            )
            .await;
        }
        Err(error) => {
            let camera_id = camera.id.to_string();
            let error_text = error.to_string();
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "snapshot_send",
                "error",
                Some(&error_text),
            )
            .await;
            return Err(error.into());
        }
    }

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
        Ok(bytes) => {
            let _ =
                db::camera_health::mark_clip_ok(camera.id, bytes.len() as i64, &config.db).await;
            let camera_id = camera.id.to_string();
            let message = format!("{}с · {}", seconds, camera.name);
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "clip_capture",
                "ok",
                Some(&message),
            )
            .await;
            bytes
        }
        Err(error) => {
            let error = error.to_string();
            let _ = db::camera_health::mark_error(camera.id, &error, &config.db).await;
            let camera_id = camera.id.to_string();
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "clip",
                "error",
                Some(&error),
            )
            .await;
            bot.edit_message_text(
                chat_id,
                status.id,
                format!("Не удалось записать видео: {}", error),
            )
            .await?;
            return Ok(());
        }
    };

    let send_result = bot
        .send_video(chat_id, InputFile::memory(bytes))
        .caption(format!("🎞 {} · {}с", camera.name, seconds))
        .await;
    let _ = bot.delete_message(chat_id, status.id).await;

    match send_result {
        Ok(_) => {
            let camera_id = camera.id.to_string();
            let message = format!("{}с · {}", seconds, camera.name);
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "clip_send",
                "ok",
                Some(&message),
            )
            .await;
        }
        Err(error) => {
            let camera_id = camera.id.to_string();
            let error_text = error.to_string();
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "clip_send",
                "error",
                Some(&error_text),
            )
            .await;
            return Err(error.into());
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn log_activity(
    config: &Arc<AppConfig>,
    user_id: Option<u64>,
    kind: &str,
    entity_type: &str,
    entity_id: Option<&str>,
    action: &str,
    status: &str,
    message: Option<&str>,
) -> Result<()> {
    db::activity_log::log(
        db::activity_log::NewActivity {
            user_id,
            kind,
            entity_type,
            entity_id,
            action,
            status,
            message,
        },
        &config.db,
    )
    .await?;
    Ok(())
}

async fn send_recording_segment(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    session_id: i64,
    segment_id: i64,
    config: &Arc<AppConfig>,
) -> Result<()> {
    ensure_recording_access(bot, chat_id, user_id, camera_id, config).await?;

    let Some(segment) = db::camera_recording_segments::get_segment(segment_id, &config.db).await?
    else {
        bot.send_message(chat_id, "Файл записи не найден.").await?;
        return Ok(());
    };

    if segment.session_id != session_id
        || segment.camera_id != camera_id
        || segment.status != "ready"
    {
        bot.send_message(chat_id, "Эта часть записи еще не готова.")
            .await?;
        return Ok(());
    }

    let Some(path) = segment.file_path else {
        bot.send_message(chat_id, "Файл записи отсутствует.")
            .await?;
        return Ok(());
    };

    let (full_path, _) = match crate::core::camera_recording::recording_file_info(config, &path)
        .await
    {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "Recording segment file is unavailable before send: user={}, camera={}, session={}, segment={}, path={}, error={:#}",
                user_id,
                camera_id,
                session_id,
                segment_id,
                path,
                error
            );
            bot.send_message(
                chat_id,
                "Файл записи недоступен на диске. Проверьте папку хранения записей и логи сервиса.",
            )
            .await?;
            return Ok(());
        }
    };
    send_recording_video_file(
        bot,
        chat_id,
        &full_path,
        &format!("🎞 Часть {}", segment.segment_index),
    )
    .await?;

    Ok(())
}

async fn send_recording_all(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    session_id: i64,
    config: &Arc<AppConfig>,
) -> Result<()> {
    ensure_recording_access(bot, chat_id, user_id, camera_id, config).await?;

    let Some(session) = db::camera_recording_sessions::get_session(session_id, &config.db).await?
    else {
        bot.send_message(chat_id, "Запись не найдена.").await?;
        return Ok(());
    };
    if session.camera_id != camera_id {
        bot.send_message(chat_id, "Недостаточно прав для просмотра записи.")
            .await?;
        return Ok(());
    }

    let segments =
        db::camera_recording_segments::list_ready_segments(session_id, &config.db).await?;
    if segments.is_empty() {
        bot.send_message(chat_id, "Готовых частей записи нет.")
            .await?;
        return Ok(());
    }

    let mut files = Vec::new();
    let mut missing_files = 0usize;
    for segment in segments {
        if let Some(path) = segment.file_path {
            match crate::core::camera_recording::recording_file_info(config, &path).await {
                Ok((full_path, _)) => files.push((segment.segment_index, full_path)),
                Err(error) => {
                    missing_files += 1;
                    log::warn!(
                        "Recording segment file is unavailable before send-all: user={}, camera={}, session={}, segment={}, path={}, error={:#}",
                        user_id,
                        camera_id,
                        session_id,
                        segment.id,
                        path,
                        error
                    );
                }
            }
        }
    }

    if files.is_empty() {
        bot.send_message(
            chat_id,
            "Файлы записи недоступны на диске. Проверьте папку хранения записей и логи сервиса.",
        )
        .await?;
        return Ok(());
    }

    if missing_files > 0 {
        bot.send_message(
            chat_id,
            format!(
                "Часть файлов записи недоступна на диске: {}. Отправляю доступные части.",
                missing_files
            ),
        )
        .await?;
    }

    for (segment_index, full_path) in files {
        send_recording_video_file(
            bot,
            chat_id,
            &full_path,
            &format!("🎞 Часть {}", segment_index),
        )
        .await?;
    }

    Ok(())
}

async fn send_recording_video_file(
    bot: &Bot,
    chat_id: ChatId,
    path: &Path,
    caption: &str,
) -> Result<()> {
    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!(
                "Recording file disappeared before Telegram send: path={}, error={}",
                path.display(),
                error
            );
            bot.send_message(
                chat_id,
                "Файл записи недоступен на диске. Проверьте папку хранения записей и логи сервиса.",
            )
            .await?;
            return Ok(());
        }
    };
    if !metadata.is_file() {
        log::warn!("Recording send path is not a file: path={}", path.display());
        bot.send_message(
            chat_id,
            "Файл записи недоступен на диске. Проверьте папку хранения записей и логи сервиса.",
        )
        .await?;
        return Ok(());
    }

    let size = metadata.len();
    if size == 0 {
        log::warn!("Recording send file is empty: path={}", path.display());
        bot.send_message(
            chat_id,
            "Файл записи пустой. Проверьте поток камеры и логи сервиса.",
        )
        .await?;
        return Ok(());
    }
    let (send_path, send_size, compressed) = if size > TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES {
        match prepare_telegram_compressed_video(path).await {
            Ok(Some((path, size))) => (path, size, true),
            Ok(None) => {
                bot.send_message(
                    chat_id,
                    format!(
                        "Не могу отправить {}: файл {} больше лимита Telegram Bot API 50 MB, а сжатая копия тоже не поместилась.\n\nПуть: `{}`\n\nМожно уменьшить Max segment seconds или использовать локальный Telegram Bot API server.",
                        caption,
                        crate::bot::format::decimal_mb(size),
                        path.display()
                    ),
                )
                .await?;
                return Ok(());
            }
            Err(error) => {
                bot.send_message(
                    chat_id,
                    format!(
                        "Не удалось сжать {} для Telegram: {}\n\nИсходный файл: `{}`",
                        caption,
                        error,
                        path.display()
                    ),
                )
                .await?;
                return Ok(());
            }
        }
    } else {
        (path.to_path_buf(), size, false)
    };

    if send_size > TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES {
        bot.send_message(
            chat_id,
            format!(
                "Не могу отправить {}: файл {} больше лимита Telegram Bot API 50 MB.\n\nПуть: `{}`",
                caption,
                crate::bot::format::decimal_mb(send_size),
                send_path.display()
            ),
        )
        .await?;
        return Ok(());
    }

    let caption = if compressed {
        format!(
            "{} · сжато до {}",
            caption,
            crate::bot::format::decimal_mb(send_size)
        )
    } else {
        caption.to_string()
    };
    let sent_path = send_path.clone();
    let sent = bot
        .send_video(chat_id, InputFile::file(send_path))
        .caption(caption)
        .await?;
    if compressed {
        let _ = tokio::fs::remove_file(sent_path).await;
    }
    crate::bot::utils::spawn_delayed_delete(
        bot.clone(),
        chat_id,
        sent.id,
        RECORDING_VIDEO_MESSAGE_TTL_SECONDS,
    );

    Ok(())
}

async fn prepare_telegram_compressed_video(
    path: &Path,
) -> Result<Option<(std::path::PathBuf, u64)>> {
    let compressed_path = telegram_compressed_path(path);
    if let Ok(metadata) = tokio::fs::metadata(&compressed_path).await {
        if is_usable_telegram_compressed_video(metadata.len()) {
            return Ok(Some((compressed_path, metadata.len())));
        }
        let _ = tokio::fs::remove_file(&compressed_path).await;
    }

    crate::core::cameras::compress_clip_for_telegram(
        path.to_path_buf(),
        compressed_path.clone(),
        TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES,
    )
    .await?;
    let metadata = tokio::fs::metadata(&compressed_path).await?;
    if is_usable_telegram_compressed_video(metadata.len()) {
        Ok(Some((compressed_path, metadata.len())))
    } else if metadata.len() < TELEGRAM_MIN_COMPRESSED_VIDEO_BYTES {
        let _ = tokio::fs::remove_file(&compressed_path).await;
        anyhow::bail!(
            "сжатая копия получилась подозрительно маленькой: {}. Попробуйте отправить еще раз после обновления профиля сжатия.",
            crate::bot::format::decimal_mb(metadata.len())
        );
    } else {
        Ok(None)
    }
}

fn is_usable_telegram_compressed_video(bytes: u64) -> bool {
    (TELEGRAM_MIN_COMPRESSED_VIDEO_BYTES..=TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES).contains(&bytes)
}

fn telegram_compressed_path(path: &Path) -> std::path::PathBuf {
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    path.with_file_name(format!("{}.telegram.v4.mp4", stem))
}

async fn ensure_recording_access(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    config: &Arc<AppConfig>,
) -> Result<()> {
    if db::cameras::get_accessible_camera(
        user_id,
        config.root_user == user_id,
        camera_id,
        &config.db,
    )
    .await?
    .is_none()
    {
        bot.send_message(chat_id, "Недостаточно прав для просмотра записи.")
            .await?;
        anyhow::bail!("recording access denied");
    }

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

    match edit_existing_view(bot, config, user_id, chat_id, message_id, &view, &text).await {
        Ok(EditViewResult::Updated(mode)) => {
            crate::core::update_user_state_with_mode(
                config,
                user_id,
                message_id.0,
                &payload_str,
                mode,
            )
            .await;
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
            reanchor_view(bot, chat_id, message_id, user_id, view, config.clone()).await
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

    match edit_existing_view(bot, &config, user_id, chat_id, message_id, &view, &text).await {
        Ok(EditViewResult::Updated(mode)) => {
            crate::core::update_user_state_with_mode(
                &config,
                user_id,
                message_id.0,
                &payload_str,
                mode,
            )
            .await;
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
    config: &Arc<AppConfig>,
    user_id: u64,
    chat_id: ChatId,
    message_id: MessageId,
    view: &View,
    text: &str,
) -> std::result::Result<EditViewResult, RequestError> {
    let kb = view.kb.clone();
    let edit_mode = edit_view_mode(config, user_id, text);
    if edit_mode == UiMessageMode::Text {
        log::debug!(
            "UI view for user {} payload {} uses text mode: {} chars, caption limit {}, current message mode {:?}",
            user_id,
            view.payload.to_string(),
            text.chars().count(),
            TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS,
            config.sessions.get(&user_id).map(|session| session.ui_message_mode)
        );
        let text = telegram_text_message(&view.get_plain_text());
        let res = bot
            .edit_message_text(chat_id, message_id, text)
            .reply_markup(kb)
            .await;

        return match res {
            Ok(_) => Ok(EditViewResult::Updated(UiMessageMode::Text)),
            Err(RequestError::Api(teloxide::ApiError::MessageNotModified)) => {
                Ok(EditViewResult::NotModified)
            }
            Err(e) if is_missing_message_error(&e) => Ok(EditViewResult::MessageMissing),
            Err(e) => Err(e),
        };
    }

    let input_file = InputFile::memory(resolve_view_image(view, user_id, config).await);

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
        Ok(_) => Ok(EditViewResult::Updated(UiMessageMode::Photo)),
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

async fn resolve_view_image(view: &View, user_id: u64, config: &Arc<AppConfig>) -> Vec<u8> {
    if let Some(image) = &view.image {
        return non_empty_image_or_placeholder(Some(image.as_slice()), "view image");
    }

    let background = crate::core::ui_background::resolve(config.clone(), user_id).await;
    non_empty_image_or_placeholder(background.as_deref(), "UI background")
}

fn non_empty_image_or_placeholder(image: Option<&[u8]>, source: &str) -> Vec<u8> {
    match image {
        Some(bytes) if !bytes.is_empty() => bytes.to_vec(),
        Some(_) => {
            log::warn!("{} is empty; using UI placeholder", source);
            crate::bot::utils::UI_PLACEHOLDER_BYTES.to_vec()
        }
        None => crate::bot::utils::UI_PLACEHOLDER_BYTES.to_vec(),
    }
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

    if should_send_view_as_text(&text) {
        log::debug!(
            "New UI view for user {} payload {} uses text mode: {} chars exceed Telegram photo caption limit {}",
            user_id,
            view.payload.to_string(),
            text.chars().count(),
            TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS
        );
        let sent = bot
            .send_message(chat_id, telegram_text_message(&view.get_plain_text()))
            .reply_markup(view.kb)
            .await?;

        crate::core::update_user_state_with_mode(
            &config,
            user_id,
            sent.id.0,
            &payload_str,
            UiMessageMode::Text,
        )
        .await;
        return Ok(());
    }

    // Исправлено: send_photo принимает InputFile, а не InputMedia
    let image = resolve_view_image(&view, user_id, &config).await;
    let input_file = InputFile::memory(image);

    let sent = bot
        .send_photo(chat_id, input_file)
        .caption(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(view.kb)
        .await?;

    // Критическая правка: сохраняем ID СООБЩЕНИЯ БОТА (sent.id), а не входящего апдейта
    crate::core::update_user_state_with_mode(
        &config,
        user_id,
        sent.id.0,
        &payload_str,
        UiMessageMode::Photo,
    )
    .await;
    Ok(())
}

fn edit_view_mode(config: &Arc<AppConfig>, user_id: u64, text: &str) -> UiMessageMode {
    let current_is_text = config
        .sessions
        .get(&user_id)
        .is_some_and(|session| session.ui_message_mode == UiMessageMode::Text);

    if current_is_text || should_send_view_as_text(text) {
        UiMessageMode::Text
    } else {
        UiMessageMode::Photo
    }
}

fn should_send_view_as_text(text: &str) -> bool {
    text.chars().count() > TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS
}

fn telegram_text_message(text: &str) -> String {
    if text.chars().count() <= TELEGRAM_TEXT_MESSAGE_LIMIT_CHARS {
        return text.to_string();
    }

    let marker = "\n\nСообщение сокращено: экран слишком длинный для Telegram.";
    let keep_chars = TELEGRAM_TEXT_MESSAGE_LIMIT_CHARS.saturating_sub(marker.chars().count());
    let mut shortened = text.chars().take(keep_chars).collect::<String>();
    while shortened.ends_with('\\') {
        shortened.pop();
    }
    log::warn!(
        "Telegram text view truncated from {} to <= {} chars",
        text.chars().count(),
        TELEGRAM_TEXT_MESSAGE_LIMIT_CHARS
    );
    shortened.push_str(marker);
    shortened
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

    keep_dialogue_with_error(
        &bot,
        &dialogue,
        &msg,
        State::WaitingForGraphInterval { device_id, room_id },
        "⚠️ Ошибка: введите целое число часов.".to_string(),
    )
    .await
}

pub async fn handle_state_alias_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (device_id, original_state, room_id): (i64, String, i64),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let alias = msg.text().unwrap_or("").trim();
    if alias.is_empty() {
        let err_msg = bot
            .send_message(msg.chat.id, "Ошибка: алиас не должен быть пустым.")
            .await?;
        crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);
        return finalize_dialogue(bot, dialogue, msg, config, None).await;
    }

    if alias.chars().count() > 40 {
        let err_msg = bot
            .send_message(
                msg.chat.id,
                "Ошибка: алиас должен быть не длиннее 40 символов.",
            )
            .await?;
        crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);
        return finalize_dialogue(bot, dialogue, msg, config, None).await;
    }

    let Some(device) = crate::db::devices::get_device_by_id(device_id, &config.db).await? else {
        let err_msg = bot
            .send_message(msg.chat.id, "Устройство не найдено.")
            .await?;
        crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 5);
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    };

    crate::db::devices::set_state_alias(&device.entity_id, &original_state, alias, &config.db)
        .await?;
    config.set_state_alias_cache(&device.entity_id, &original_state, alias.to_string());

    let target_payload = settings_payload_for_current_session(
        &config,
        msg.from.as_ref().map(|user| user.id.0),
        crate::bot::router::SettingsPayload::StateAliases {
            room: room_id,
            device: device_id,
        },
    );

    finalize_dialogue(bot, dialogue, msg, config, Some(target_payload)).await
}

fn settings_payload_for_current_session(
    config: &Arc<AppConfig>,
    user_id: Option<u64>,
    payload: crate::bot::router::SettingsPayload,
) -> Payload {
    let Some(user_id) = user_id else {
        return Payload::Settings(payload);
    };

    let from_admin_settings = config.sessions.get(&user_id).is_some_and(|session| {
        matches!(
            Payload::from_string(&session.current_context),
            Ok(Payload::AdminSettings(_))
        )
    });

    if from_admin_settings {
        Payload::AdminSettings(payload)
    } else {
        Payload::Settings(payload)
    }
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

pub async fn handle_add_recording_rule_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    room_id: i64,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    match parse_recording_rule_input(
        msg.text().unwrap_or(""),
        config.camera_recording_max_tail_seconds,
        config.camera_recording_max_segment_seconds,
    ) {
        Ok(input) => {
            let cameras = crate::db::cameras::list_room_cameras(room_id, &config.db).await?;
            if !cameras.iter().any(|camera| camera.id == input.camera_id) {
                let err_msg = bot
                    .send_message(
                        msg.chat.id,
                        "Ошибка: камера не принадлежит выбранной комнате.",
                    )
                    .await?;
                crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 8);
                return finalize_dialogue(bot, dialogue, msg, config, None).await;
            }

            let rule_id = crate::db::camera_recording_rules::create_rule(
                crate::db::camera_recording_rules::NewRecordingRule {
                    name: &input.name,
                    camera_id: input.camera_id,
                    condition_logic: input.logic,
                    tail_seconds: i64::from(input.tail_seconds),
                    max_segment_seconds: i64::from(input.max_segment_seconds),
                    cooldown_s: i64::from(input.cooldown_s),
                    retention_days: i64::from(input.retention_days),
                },
                &config.db,
            )
            .await?;

            for condition in &input.conditions {
                crate::db::camera_recording_rules::add_condition(
                    crate::db::camera_recording_rules::NewRecordingCondition {
                        rule_id,
                        entity_id: &condition.entity_id,
                        operator: condition.operator,
                        from_state: condition.from_state.as_deref(),
                        to_state: condition.to_state.as_deref(),
                        value: condition.value.as_deref(),
                    },
                    &config.db,
                )
                .await?;
            }

            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(
                    crate::bot::router::AdminPayload::RecordingRules { room: room_id },
                )),
            )
            .await
        }
        Err(text) => {
            let err_msg = bot.send_message(msg.chat.id, text).await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 10);
            finalize_dialogue(bot, dialogue, msg, config, None).await
        }
    }
}

pub async fn handle_add_recording_rule_group_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    room_id: Option<i64>,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let name = match parse_recording_rule_group_name(msg.text().unwrap_or("")) {
        Ok(name) => name,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::AddRecordingRuleGroup { room_id },
                text,
            )
            .await?;
            return Ok(());
        }
    };

    let group_id = crate::db::camera_recording_rule_groups::create_group(&name, &config.db).await?;
    let group_id_text = group_id.to_string();
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: msg.from.as_ref().map(|user| user.id.0),
            kind: "recording",
            entity_type: "rule_group",
            entity_id: Some(&group_id_text),
            action: "rule_group.create",
            status: "ok",
            message: Some(&name),
        },
        &config.db,
    )
    .await;
    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(recording_rule_group_detail_payload(room_id, group_id)),
    )
    .await
}

pub async fn handle_add_recording_rule_group_for_wizard_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let user_id = msg.from.as_ref().context("User context missing")?.id.0;
    let has_wizard = config
        .sessions
        .get(&user_id)
        .is_some_and(|session| session.recording_rule_wizard.is_some());
    if !has_wizard {
        return finish_stale_wizard_input(&bot, &dialogue, &msg).await;
    }

    let name = match parse_recording_rule_group_name(msg.text().unwrap_or("")) {
        Ok(name) => name,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::AddRecordingRuleGroupForWizard,
                text,
            )
            .await?;
            return Ok(());
        }
    };

    let group_id = crate::db::camera_recording_rule_groups::create_group(&name, &config.db).await?;
    if let Some(mut session) = config.sessions.get_mut(&user_id) {
        if let Some(wizard) = session.recording_rule_wizard.as_mut() {
            if !wizard.group_ids.contains(&group_id) {
                wizard.group_ids.push(group_id);
                wizard.group_ids.sort_unstable();
            }
        }
    }

    let group_id_text = group_id.to_string();
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: msg.from.as_ref().map(|user| user.id.0),
            kind: "recording",
            entity_type: "rule_group",
            entity_id: Some(&group_id_text),
            action: "rule_group.create",
            status: "ok",
            message: Some(&name),
        },
        &config.db,
    )
    .await;

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(
            crate::bot::router::AdminPayload::WizardGroups,
        )),
    )
    .await
}

pub async fn handle_add_recording_rule_group_for_edit_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (room_id, rule_id): (i64, i64),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    if reject_recording_rule_room_mismatch(&bot, &msg, &config, room_id, rule_id).await? {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(
                crate::bot::router::AdminPayload::RecordingRules { room: room_id },
            )),
        )
        .await;
    }

    let name = match parse_recording_rule_group_name(msg.text().unwrap_or("")) {
        Ok(name) => name,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::AddRecordingRuleGroupForEdit { room_id, rule_id },
                text,
            )
            .await?;
            return Ok(());
        }
    };

    let group_id = crate::db::camera_recording_rule_groups::create_group(&name, &config.db).await?;
    let added =
        crate::db::camera_recording_rule_groups::add_rule_to_group(rule_id, group_id, &config.db)
            .await?;
    let group_id_text = group_id.to_string();
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: msg.from.as_ref().map(|user| user.id.0),
            kind: "recording",
            entity_type: "rule_group",
            entity_id: Some(&group_id_text),
            action: "rule_group.create",
            status: "ok",
            message: Some(&name),
        },
        &config.db,
    )
    .await;
    if added {
        let activity_message = format!("rule {}", rule_id);
        let _ = crate::db::activity_log::log(
            crate::db::activity_log::NewActivity {
                user_id: msg.from.as_ref().map(|user| user.id.0),
                kind: "recording",
                entity_type: "rule_group",
                entity_id: Some(&group_id_text),
                action: "rule_group.add_rule",
                status: "ok",
                message: Some(&activity_message),
            },
            &config.db,
        )
        .await;
    }

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(
            crate::bot::router::AdminPayload::RecordingRuleEditGroups {
                room: room_id,
                rule: rule_id,
            },
        )),
    )
    .await
}

pub async fn handle_rename_recording_rule_group_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (room_id, group_id): (Option<i64>, i64),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let name = match parse_recording_rule_group_name(msg.text().unwrap_or("")) {
        Ok(name) => name,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::RenameRecordingRuleGroup { room_id, group_id },
                text,
            )
            .await?;
            return Ok(());
        }
    };

    if let Err(error) =
        crate::db::camera_recording_rule_groups::rename_group(group_id, &name, &config.db).await
    {
        keep_dialogue_with_error(
            &bot,
            &dialogue,
            &msg,
            State::RenameRecordingRuleGroup { room_id, group_id },
            recording_rule_group_error_text(&error),
        )
        .await?;
        return Ok(());
    }

    let group_id_text = group_id.to_string();
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: msg.from.as_ref().map(|user| user.id.0),
            kind: "recording",
            entity_type: "rule_group",
            entity_id: Some(&group_id_text),
            action: "rule_group.rename",
            status: "ok",
            message: Some(&name),
        },
        &config.db,
    )
    .await;

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(recording_rule_group_detail_payload(room_id, group_id)),
    )
    .await
}

pub async fn handle_add_action_group_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let name = match parse_action_group_name(msg.text().unwrap_or("")) {
        Ok(name) => name,
        Err(text) => {
            keep_dialogue_with_error(&bot, &dialogue, &msg, State::AddActionGroup, text).await?;
            return Ok(());
        }
    };

    match crate::db::action_groups::create_action_group(&name, &config.db).await {
        Ok(group_id) => {
            let group_id_text = group_id.to_string();
            let _ = crate::db::activity_log::log(
                crate::db::activity_log::NewActivity {
                    user_id: msg.from.as_ref().map(|user| user.id.0),
                    kind: "action_group",
                    entity_type: "action_group",
                    entity_id: Some(&group_id_text),
                    action: "action_group.create",
                    status: "ok",
                    message: Some(&name),
                },
                &config.db,
            )
            .await;
            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(AdminPayload::ActionGroupDetail {
                    group: group_id,
                })),
            )
            .await
        }
        Err(error) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::AddActionGroup,
                action_group_error_text(&error),
            )
            .await
        }
    }
}

pub async fn handle_rename_action_group_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    group_id: i64,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let name = match parse_action_group_name(msg.text().unwrap_or("")) {
        Ok(name) => name,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::RenameActionGroup { group_id },
                text,
            )
            .await?;
            return Ok(());
        }
    };

    match crate::db::action_groups::rename_action_group(group_id, &name, &config.db).await {
        Ok(()) => {
            let group_id_text = group_id.to_string();
            let _ = crate::db::activity_log::log(
                crate::db::activity_log::NewActivity {
                    user_id: msg.from.as_ref().map(|user| user.id.0),
                    kind: "action_group",
                    entity_type: "action_group",
                    entity_id: Some(&group_id_text),
                    action: "action_group.rename",
                    status: "ok",
                    message: Some(&name),
                },
                &config.db,
            )
            .await;
            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(AdminPayload::ActionGroupDetail {
                    group: group_id,
                })),
            )
            .await
        }
        Err(error) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::RenameActionGroup { group_id },
                action_group_error_text(&error),
            )
            .await
        }
    }
}

pub async fn handle_ha_native_alias_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    target_id: i64,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let name = match parse_action_group_name(msg.text().unwrap_or("")) {
        Ok(name) => name,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::RenameHaNativeTarget { target_id },
                text,
            )
            .await?;
            return Ok(());
        }
    };

    crate::db::action_groups::set_ha_native_target_alias(target_id, &name, &config.db).await?;
    let target_id_text = target_id.to_string();
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: msg.from.as_ref().map(|user| user.id.0),
            kind: "ha_native_target",
            entity_type: "ha_native_target",
            entity_id: Some(&target_id_text),
            action: "ha_native_target.alias_update",
            status: "ok",
            message: Some(&name),
        },
        &config.db,
    )
    .await;

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(AdminPayload::HaNativeActionDetail {
            action: target_id,
        })),
    )
    .await
}

pub async fn handle_add_action_schedule_time_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (target, command): (
        crate::db::action_groups::ActionTargetRef,
        crate::db::action_groups::ActionScheduleCommand,
    ),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let time_minute = match crate::db::action_groups::parse_time_minute(msg.text().unwrap_or("")) {
        Ok(value) => value,
        Err(error) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::AddActionScheduleTime { target, command },
                error.to_string(),
            )
            .await?;
            return Ok(());
        }
    };

    match crate::db::action_groups::create_action_schedule(
        target,
        command,
        time_minute,
        crate::db::action_groups::ALL_DAYS_MASK,
        &config.db,
    )
    .await
    {
        Ok(schedule_id) => {
            let schedule_id_text = schedule_id.to_string();
            let _ = crate::db::activity_log::log(
                crate::db::activity_log::NewActivity {
                    user_id: msg.from.as_ref().map(|user| user.id.0),
                    kind: "action_group",
                    entity_type: "action_schedule",
                    entity_id: Some(&schedule_id_text),
                    action: "action_group.schedule_create",
                    status: "ok",
                    message: Some(&crate::db::action_groups::format_time_minute(time_minute)),
                },
                &config.db,
            )
            .await;
            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(AdminPayload::ActionScheduleDetail {
                    target,
                    schedule: schedule_id,
                })),
            )
            .await
        }
        Err(error) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::AddActionScheduleTime { target, command },
                error.to_string(),
            )
            .await
        }
    }
}

pub async fn handle_edit_action_schedule_time_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (target, schedule_id): (crate::db::action_groups::ActionTargetRef, i64),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let time_minute = match crate::db::action_groups::parse_time_minute(msg.text().unwrap_or("")) {
        Ok(value) => value,
        Err(error) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::EditActionScheduleTime {
                    target,
                    schedule_id,
                },
                error.to_string(),
            )
            .await?;
            return Ok(());
        }
    };

    let schedule_matches = crate::db::action_groups::get_action_schedule(schedule_id, &config.db)
        .await?
        .map(|schedule| schedule.target_ref())
        .transpose()?
        .is_some_and(|schedule_target| schedule_target == target);
    if !schedule_matches {
        let user_id = msg.from.as_ref().map(|user| user.id.0);
        let lang = match user_id {
            Some(user_id) => crate::db::get_user_language(user_id, &config.db)
                .await?
                .unwrap_or(config.default_language),
            None => config.default_language,
        };
        let notice = bot
            .send_message(
                msg.chat.id,
                crate::i18n::t(lang, "action_groups.schedule_not_found"),
            )
            .await?;
        crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, notice.id, 8);
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(AdminPayload::ActionSchedules { target })),
        )
        .await;
    }

    crate::db::action_groups::update_action_schedule_time(schedule_id, time_minute, &config.db)
        .await?;
    let schedule_id_text = schedule_id.to_string();
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: msg.from.as_ref().map(|user| user.id.0),
            kind: "action_group",
            entity_type: "action_schedule",
            entity_id: Some(&schedule_id_text),
            action: "action_group.schedule_time_update",
            status: "ok",
            message: Some(&crate::db::action_groups::format_time_minute(time_minute)),
        },
        &config.db,
    )
    .await;
    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(AdminPayload::ActionScheduleDetail {
            target,
            schedule: schedule_id,
        })),
    )
    .await
}

pub async fn handle_recording_rule_wizard_source_value_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    mode: crate::bot::recording_rule_wizard::WizardTriggerMode,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let value =
        match crate::bot::recording_rule_wizard::source_threshold_value(msg.text().unwrap_or("")) {
            Ok(value) => value,
            Err(text) => {
                keep_dialogue_with_error(
                    &bot,
                    &dialogue,
                    &msg,
                    State::RecordingRuleWizardSourceValue { mode },
                    text,
                )
                .await?;
                return Ok(());
            }
        };

    let user_id = msg.from.as_ref().context("User context missing")?.id.0;
    let updated = {
        if let Some(mut session) = config.sessions.get_mut(&user_id) {
            if let Some(wizard) = session.recording_rule_wizard.as_mut() {
                wizard.set_source_mode(mode, Some(value));
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if !updated {
        return finish_stale_wizard_input(&bot, &dialogue, &msg).await;
    }

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(
            crate::bot::router::AdminPayload::WizardConditions,
        )),
    )
    .await
}

pub async fn handle_recording_rule_wizard_active_time_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let active_time =
        match crate::db::camera_recording_rules::parse_active_time_window(msg.text().unwrap_or(""))
        {
            Ok(active_time) => active_time,
            Err(text) => {
                keep_dialogue_with_error(
                    &bot,
                    &dialogue,
                    &msg,
                    State::RecordingRuleWizardActiveTimeValue,
                    text,
                )
                .await?;
                return Ok(());
            }
        };

    let user_id = msg.from.as_ref().context("User context missing")?.id.0;
    let updated = {
        if let Some(mut session) = config.sessions.get_mut(&user_id) {
            if let Some(wizard) = session.recording_rule_wizard.as_mut() {
                wizard.active_time = active_time;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if !updated {
        return finish_stale_wizard_input(&bot, &dialogue, &msg).await;
    }

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(AdminPayload::WizardGroups)),
    )
    .await
}

pub async fn handle_recording_rule_wizard_condition_value_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    let user_id = msg.from.as_ref().context("User context missing")?.id.0;
    let pending = config
        .sessions
        .get(&user_id)
        .and_then(|session| session.recording_rule_wizard.clone())
        .and_then(|wizard| wizard.pending_condition);
    let Some(pending) = pending else {
        return finish_stale_wizard_input(&bot, &dialogue, &msg).await;
    };
    let Some(operator) = pending.operator else {
        return finish_stale_wizard_input(&bot, &dialogue, &msg).await;
    };
    let Some(candidate) =
        crate::db::devices::get_recording_wizard_candidate(pending.device_id, &config.db).await?
    else {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(
                crate::bot::router::AdminPayload::WizardConditions,
            )),
        )
        .await;
    };

    let condition = match crate::bot::recording_rule_wizard::condition_from_input(
        &candidate.entity_id,
        operator,
        msg.text().unwrap_or(""),
    ) {
        Ok(condition) => condition,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::RecordingRuleWizardConditionValue,
                text,
            )
            .await?;
            return Ok(());
        }
    };

    let updated = {
        if let Some(mut session) = config.sessions.get_mut(&user_id) {
            if let Some(wizard) = session.recording_rule_wizard.as_mut() {
                wizard.add_extra_condition(condition);
                wizard.pending_condition = None;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if !updated {
        return finish_stale_wizard_input(&bot, &dialogue, &msg).await;
    }

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(
            crate::bot::router::AdminPayload::WizardConditions,
        )),
    )
    .await
}

pub async fn handle_edit_recording_rule_number_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (room_id, rule_id, field): (i64, i64, RecordingRuleEditField),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    if reject_recording_rule_room_mismatch(&bot, &msg, &config, room_id, rule_id).await? {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(
                crate::bot::router::AdminPayload::RecordingRules { room: room_id },
            )),
        )
        .await;
    }

    let (min, max) = recording_rule_edit_field_bounds(&config, field);
    let value = match parse_range(msg.text().unwrap_or("").trim(), field.title(), min, max) {
        Ok(value) => i64::from(value),
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::EditRecordingRuleNumber {
                    room_id,
                    rule_id,
                    field,
                },
                text,
            )
            .await?;
            return Ok(());
        }
    };

    let (tail_seconds, max_segment_seconds, cooldown_s, retention_days) = match field {
        RecordingRuleEditField::TailSeconds => (Some(value), None, None, None),
        RecordingRuleEditField::MaxSegmentSeconds => (None, Some(value), None, None),
        RecordingRuleEditField::CooldownSeconds => (None, None, Some(value), None),
        RecordingRuleEditField::RetentionDays => (None, None, None, Some(value)),
    };

    crate::db::camera_recording_rules::update_rule_recording_options(
        rule_id,
        tail_seconds,
        max_segment_seconds,
        cooldown_s,
        retention_days,
        &config.db,
    )
    .await?;

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(
            crate::bot::router::AdminPayload::RecordingRuleEditMenu {
                room: room_id,
                rule: rule_id,
            },
        )),
    )
    .await
}

pub async fn handle_edit_recording_rule_active_time_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (room_id, rule_id): (i64, i64),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    if reject_recording_rule_room_mismatch(&bot, &msg, &config, room_id, rule_id).await? {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(AdminPayload::RecordingRules {
                room: room_id,
            })),
        )
        .await;
    }

    let mut active_time =
        match crate::db::camera_recording_rules::parse_active_time_window(msg.text().unwrap_or(""))
        {
            Ok(active_time) => active_time,
            Err(text) => {
                keep_dialogue_with_error(
                    &bot,
                    &dialogue,
                    &msg,
                    State::EditRecordingRuleActiveTime { room_id, rule_id },
                    text,
                )
                .await?;
                return Ok(());
            }
        };

    if let Some(rule) = crate::db::camera_recording_rules::get_rule(rule_id, &config.db).await? {
        let current = rule.active_time();
        if current.enabled && crate::db::camera_recording_rules::active_time_is_valid(current) {
            active_time.days_mask = current.days_mask;
        }
    }

    crate::db::camera_recording_rules::update_rule_active_time(rule_id, active_time, &config.db)
        .await?;

    let rule_id_text = rule_id.to_string();
    let message = format!(
        "{}-{} mask={}",
        crate::db::camera_recording_rules::format_minute(active_time.from_minute),
        crate::db::camera_recording_rules::format_minute(active_time.to_minute),
        active_time.days_mask
    );
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: msg.from.as_ref().map(|user| user.id.0),
            kind: "recording",
            entity_type: "recording_rule",
            entity_id: Some(&rule_id_text),
            action: "recording_rule.active_time_update",
            status: "ok",
            message: Some(&message),
        },
        &config.db,
    )
    .await;

    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(AdminPayload::RecordingRuleEditActiveTime {
            room: room_id,
            rule: rule_id,
        })),
    )
    .await
}

pub async fn handle_edit_recording_rule_condition_value_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (room_id, rule_id, device_id, operator): (
        i64,
        i64,
        i64,
        crate::db::camera_recording_rules::ConditionOperator,
    ),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    if reject_recording_rule_room_mismatch(&bot, &msg, &config, room_id, rule_id).await? {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(
                crate::bot::router::AdminPayload::RecordingRules { room: room_id },
            )),
        )
        .await;
    }

    let Some(candidate) =
        crate::db::devices::get_recording_wizard_candidate(device_id, &config.db).await?
    else {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(
                crate::bot::router::AdminPayload::RecordingRuleEditSensors {
                    room: room_id,
                    rule: rule_id,
                },
            )),
        )
        .await;
    };

    let condition = match crate::bot::recording_rule_wizard::condition_from_input(
        &candidate.entity_id,
        operator,
        msg.text().unwrap_or(""),
    ) {
        Ok(condition) => condition,
        Err(text) => {
            keep_dialogue_with_error(
                &bot,
                &dialogue,
                &msg,
                State::EditRecordingRuleConditionValue {
                    room_id,
                    rule_id,
                    device_id,
                    operator,
                },
                text,
            )
            .await?;
            return Ok(());
        }
    };

    crate::db::camera_recording_rules::add_condition(
        crate::db::camera_recording_rules::NewRecordingCondition {
            rule_id,
            entity_id: &condition.entity_id,
            operator: condition.operator,
            from_state: condition.from_state.as_deref(),
            to_state: condition.to_state.as_deref(),
            value: condition.value.as_deref(),
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
            crate::bot::router::AdminPayload::RecordingRuleEditSensors {
                room: room_id,
                rule: rule_id,
            },
        )),
    )
    .await
}

pub async fn handle_edit_recording_rule_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (room_id, rule_id): (i64, i64),
) -> Result<()> {
    if msg.from.as_ref().map(|u| u.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }

    if reject_recording_rule_room_mismatch(&bot, &msg, &config, room_id, rule_id).await? {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(
                crate::bot::router::AdminPayload::RecordingRules { room: room_id },
            )),
        )
        .await;
    }

    match parse_recording_rule_input(
        msg.text().unwrap_or(""),
        config.camera_recording_max_tail_seconds,
        config.camera_recording_max_segment_seconds,
    ) {
        Ok(input) => {
            let cameras = crate::db::cameras::list_room_cameras(room_id, &config.db).await?;
            if !cameras.iter().any(|camera| camera.id == input.camera_id) {
                let err_msg = bot
                    .send_message(
                        msg.chat.id,
                        "Ошибка: камера не принадлежит выбранной комнате.",
                    )
                    .await?;
                crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 8);
                return finalize_dialogue(bot, dialogue, msg, config, None).await;
            }

            let conditions = input
                .conditions
                .iter()
                .map(
                    |condition| crate::db::camera_recording_rules::NewRecordingCondition {
                        rule_id,
                        entity_id: &condition.entity_id,
                        operator: condition.operator,
                        from_state: condition.from_state.as_deref(),
                        to_state: condition.to_state.as_deref(),
                        value: condition.value.as_deref(),
                    },
                )
                .collect::<Vec<_>>();

            crate::db::camera_recording_rules::update_rule_replace_conditions(
                rule_id,
                crate::db::camera_recording_rules::NewRecordingRule {
                    name: &input.name,
                    camera_id: input.camera_id,
                    condition_logic: input.logic,
                    tail_seconds: i64::from(input.tail_seconds),
                    max_segment_seconds: i64::from(input.max_segment_seconds),
                    cooldown_s: i64::from(input.cooldown_s),
                    retention_days: i64::from(input.retention_days),
                },
                &conditions,
                &config.db,
            )
            .await?;

            finalize_dialogue(
                bot,
                dialogue,
                msg,
                config,
                Some(Payload::Admin(
                    crate::bot::router::AdminPayload::RecordingRuleDetail {
                        room: room_id,
                        rule: rule_id,
                    },
                )),
            )
            .await
        }
        Err(text) => {
            let err_msg = bot.send_message(msg.chat.id, text).await?;
            crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 10);
            finalize_dialogue(bot, dialogue, msg, config, None).await
        }
    }
}

async fn reject_recording_rule_room_mismatch(
    bot: &Bot,
    msg: &Message,
    config: &Arc<AppConfig>,
    room_id: i64,
    rule_id: i64,
) -> Result<bool> {
    let exists = crate::db::camera_recording_rules::get_rule_for_room(rule_id, room_id, &config.db)
        .await?
        .is_some();
    if exists {
        return Ok(false);
    }

    let err_msg = bot
        .send_message(
            msg.chat.id,
            "Ошибка: правило не найдено или не принадлежит выбранной комнате.",
        )
        .await?;
    crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 8);
    Ok(true)
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

fn parse_recording_rule_group_name(text: &str) -> std::result::Result<String, String> {
    let name = text.trim();
    if name.is_empty() {
        return Err("Ошибка: название группы не должно быть пустым.".to_string());
    }
    if name.chars().count() > 40 {
        return Err("Ошибка: название группы должно быть не длиннее 40 символов.".to_string());
    }
    Ok(name.to_string())
}

fn parse_action_group_name(text: &str) -> std::result::Result<String, String> {
    let name = text.trim();
    if name.is_empty() {
        return Err("Ошибка: название не должно быть пустым.".to_string());
    }
    if name.chars().count() > 80 {
        return Err("Ошибка: название должно быть не длиннее 80 символов.".to_string());
    }
    Ok(name.to_string())
}

fn action_group_error_text(error: &anyhow::Error) -> String {
    let text = error.to_string();
    if text.contains("UNIQUE constraint failed") || text.contains("already exists") {
        "Ошибка: группа с таким названием уже есть.".to_string()
    } else {
        format!("Ошибка: не удалось сохранить: {}", text)
    }
}

fn recording_rule_group_error_text(error: &anyhow::Error) -> String {
    let text = error.to_string();
    if text.contains("already exists") || text.contains("UNIQUE constraint failed") {
        "Ошибка: группа с таким названием уже есть.".to_string()
    } else if text.contains("not found") {
        "Ошибка: группа не найдена.".to_string()
    } else {
        format!("Ошибка: не удалось сохранить группу: {}", text)
    }
}

fn recording_rule_group_detail_payload(room_id: Option<i64>, group_id: i64) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::RecordingRuleGroupDetailForRoom {
            room,
            group: group_id,
        },
        None => AdminPayload::RecordingRuleGroupDetail { group: group_id },
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

#[derive(Debug)]
struct RecordingRuleInput {
    name: String,
    camera_id: i64,
    logic: crate::db::camera_recording_rules::ConditionLogic,
    conditions: Vec<RecordingConditionInput>,
    tail_seconds: u32,
    max_segment_seconds: u32,
    cooldown_s: u32,
    retention_days: u32,
}

#[derive(Debug)]
struct RecordingConditionInput {
    entity_id: String,
    operator: crate::db::camera_recording_rules::ConditionOperator,
    from_state: Option<String>,
    to_state: Option<String>,
    value: Option<String>,
}

fn parse_recording_rule_input(
    text: &str,
    max_tail_seconds: u32,
    max_segment_seconds_limit: u32,
) -> std::result::Result<RecordingRuleInput, String> {
    let normalized = extract_recording_rule_block(text);
    let lines = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if lines.len() < 8 {
        return Err(recording_rule_input_help());
    }

    let numbers_start = lines.len() - 4;
    let name = lines[0].clone();
    let camera_id = lines[1]
        .parse::<i64>()
        .map_err(|_| "ID камеры должен быть числом.".to_string())?;
    let logic = match lines[2].to_lowercase().as_str() {
        "all" | "и" => crate::db::camera_recording_rules::ConditionLogic::All,
        "any" | "или" => crate::db::camera_recording_rules::ConditionLogic::Any,
        _ => return Err("Логика должна быть any/all или И/ИЛИ.".to_string()),
    };

    let mut conditions = Vec::new();
    for line in &lines[3..numbers_start] {
        conditions.push(parse_recording_condition(line)?);
    }

    if conditions.is_empty() {
        return Err("Нужно добавить хотя бы одно условие.".to_string());
    }

    let tail_seconds = parse_range(&lines[numbers_start], "Tail seconds", 5, max_tail_seconds)?;
    let max_segment_seconds = parse_range(
        &lines[numbers_start + 1],
        "Max segment seconds",
        30,
        max_segment_seconds_limit,
    )?;
    let cooldown_s = parse_range(&lines[numbers_start + 2], "Cooldown", 0, 86400)?;
    let retention_days = parse_range(&lines[numbers_start + 3], "Retention days", 1, 365)?;

    Ok(RecordingRuleInput {
        name,
        camera_id,
        logic,
        conditions,
        tail_seconds,
        max_segment_seconds,
        cooldown_s,
        retention_days,
    })
}

fn extract_recording_rule_block(text: &str) -> String {
    let mut use_tail = false;
    let mut lines = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.contains("Текст правила")
            || trimmed.contains("Отправьте исправленный блок")
            || trimmed.contains("Скопируйте блок")
        {
            use_tail = true;
            lines.clear();
            continue;
        }

        if use_tail {
            if trimmed.starts_with("Чтобы изменить")
                || trimmed.starts_with("────────────────")
                || trimmed.starts_with("Обновлено:")
            {
                break;
            }
            lines.push(line);
        }
    }

    if use_tail {
        lines.join("\n")
    } else {
        text.to_string()
    }
}

fn parse_recording_condition(line: &str) -> std::result::Result<RecordingConditionInput, String> {
    let parts = line.split(';').map(str::trim).collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err(format!("Некорректное условие: {}", line));
    }

    let operator = match parts[1] {
        "changed_to" => crate::db::camera_recording_rules::ConditionOperator::ChangedTo,
        "changed_from_to" => crate::db::camera_recording_rules::ConditionOperator::ChangedFromTo,
        "is" => crate::db::camera_recording_rules::ConditionOperator::Is,
        "is_not" => crate::db::camera_recording_rules::ConditionOperator::IsNot,
        "contains" => crate::db::camera_recording_rules::ConditionOperator::Contains,
        "above" => crate::db::camera_recording_rules::ConditionOperator::Above,
        "below" => crate::db::camera_recording_rules::ConditionOperator::Below,
        other => return Err(format!("Неизвестный оператор: {}", other)),
    };

    Ok(RecordingConditionInput {
        entity_id: parts[0].to_string(),
        operator,
        from_state: non_empty_part(parts.get(2).copied()),
        to_state: non_empty_part(parts.get(3).copied()),
        value: non_empty_part(parts.get(4).copied()),
    })
}

fn non_empty_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "*")
        .map(ToOwned::to_owned)
}

fn parse_range(value: &str, label: &str, min: u32, max: u32) -> std::result::Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("{} должен быть числом.", label))?;
    if !(min..=max).contains(&parsed) {
        return Err(format!("{} должен быть от {} до {}.", label, min, max));
    }
    Ok(parsed)
}

fn recording_rule_edit_field_bounds(
    config: &AppConfig,
    field: RecordingRuleEditField,
) -> (u32, u32) {
    match field {
        RecordingRuleEditField::TailSeconds => (5, config.camera_recording_max_tail_seconds),
        RecordingRuleEditField::MaxSegmentSeconds => {
            (30, config.camera_recording_max_segment_seconds)
        }
        RecordingRuleEditField::CooldownSeconds => (0, 86_400),
        RecordingRuleEditField::RetentionDays => (1, 365),
    }
}

fn recording_rule_input_help() -> String {
    "Ошибка: заполните правило в формате:\nНазвание\nID камеры\nany/all\nentity_id;operator;from;to;value\nTail seconds\nMax segment seconds\nCooldown seconds\nRetention days".to_string()
}

async fn keep_dialogue_with_error(
    bot: &Bot,
    dialogue: &MyDialogue,
    msg: &Message,
    state: State,
    text: String,
) -> Result<()> {
    dialogue.update(state).await?;
    let err_msg = bot.send_message(msg.chat.id, text).await?;
    crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 8);
    let _ = bot.delete_message(msg.chat.id, msg.id).await;
    Ok(())
}

async fn finish_stale_wizard_input(bot: &Bot, dialogue: &MyDialogue, msg: &Message) -> Result<()> {
    dialogue.exit().await?;
    let _ = bot.delete_message(msg.chat.id, msg.id).await;
    let notice = bot
        .send_message(
            msg.chat.id,
            "Сессия мастера устарела. Откройте мастер заново.",
        )
        .await?;
    crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, notice.id, 8);
    Ok(())
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
    use crate::bot::text_commands;

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

    #[test]
    fn video_text_command_requires_seconds_suffix_for_duration() {
        assert_eq!(
            text_commands::split_camera_video_command("камера 3"),
            ("камера 3", None)
        );
        assert_eq!(
            text_commands::split_camera_video_command("камера 3 10с"),
            ("камера 3", Some(10))
        );
        assert_eq!(
            text_commands::split_camera_video_command("#3 15sec"),
            ("#3", Some(15))
        );
    }

    #[test]
    fn device_text_match_allows_words_in_different_order() {
        let haystack = text_commands::normalize_command_text("Коридор Свет light.corridor");

        assert!(text_commands::device_text_matches(
            "свет коридор",
            &haystack
        ));
        assert!(text_commands::device_text_matches(
            "коридор свет",
            &haystack
        ));
        assert!(!text_commands::device_text_matches("свет кухня", &haystack));
    }

    #[test]
    fn device_text_command_accepts_natural_prefix_form() {
        let normalized = text_commands::normalize_command_text("Включи свет в коридоре");
        let (query, action) = text_commands::parse_device_text_command(&normalized)
            .expect("natural command should parse");

        assert_eq!(query, "свет в коридоре");
        assert!(matches!(action, crate::core::devices::DeviceAction::TurnOn));
    }

    #[test]
    fn device_text_match_allows_simple_russian_location_suffixes() {
        let haystack = text_commands::normalize_command_text("Коридор Свет light.corridor");

        assert!(text_commands::device_text_matches(
            "свет в коридоре",
            &haystack
        ));
    }

    #[test]
    fn device_text_match_tolerates_small_stt_mistakes() {
        let haystack = text_commands::normalize_command_text("Коридор Свет light.corridor");

        assert!(text_commands::device_text_matches(
            "свет каридор",
            &haystack
        ));
        assert!(text_commands::device_text_matches(
            "света коридор",
            &haystack
        ));
        assert!(!text_commands::device_text_matches(
            "свет спальня",
            &haystack
        ));
    }

    #[test]
    fn text_command_detector_accepts_supported_commands() {
        assert!(looks_like_text_command("свет коридор вкл"));
        assert!(looks_like_text_command("включи свет в коридоре"));
        assert!(looks_like_text_command("снимок вход"));
        assert!(!looks_like_text_command("привет"));
    }

    #[test]
    fn recording_rule_input_extracts_copy_block_from_detail_text() {
        let input = parse_recording_rule_input(
            "1. switch.vykliuchatel_0;changed_from_to;off;on;\n2. switch.vykliuchatel_0;changed_from_to;on;off;\n\nТекст правила для копирования и изменения:\nТест: свет в коридоре\n3\nany\nswitch.vykliuchatel_0;changed_from_to;off;on;\nswitch.vykliuchatel_0;changed_from_to;on;off;\n60\n30\n0\n15\n\nЧтобы изменить правило: нажмите ✏️ Изменить.",
            300,
            300,
        )
        .expect("copy block should parse");

        assert_eq!(input.name, "Тест: свет в коридоре");
        assert_eq!(input.camera_id, 3);
        assert_eq!(input.conditions.len(), 2);
        assert_eq!(input.retention_days, 15);
    }

    #[test]
    fn recording_rule_input_extracts_wizard_advanced_block() {
        let input = parse_recording_rule_input(
            "Скопируйте блок ниже, измените если нужно и отправьте сообщением:\n\nЗамок Дверь: открытие или закрытие\n3\nany\nbinary_sensor.zamok_contact;changed_from_to;off;on;\nbinary_sensor.zamok_contact;changed_from_to;on;off;\n60\n300\n0\n30\n\n────────────────────\nОбновлено: 12:00:00",
            300,
            300,
        )
        .expect("wizard advanced block should parse");

        assert_eq!(input.name, "Замок Дверь: открытие или закрытие");
        assert_eq!(input.camera_id, 3);
        assert_eq!(input.conditions.len(), 2);
        assert_eq!(input.retention_days, 30);
    }

    #[test]
    fn empty_view_image_falls_back_to_placeholder() {
        let image = non_empty_image_or_placeholder(Some(&[]), "test image");

        assert_eq!(image, crate::bot::utils::UI_PLACEHOLDER_BYTES);
    }

    #[test]
    fn long_view_text_uses_text_message_mode() {
        let text = "x".repeat(TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS + 1);

        assert!(should_send_view_as_text(&text));
    }

    #[test]
    fn telegram_text_message_is_truncated_to_limit() {
        let text = "x".repeat(TELEGRAM_TEXT_MESSAGE_LIMIT_CHARS + 100);
        let shortened = telegram_text_message(&text);

        assert!(shortened.chars().count() <= TELEGRAM_TEXT_MESSAGE_LIMIT_CHARS);
        assert!(shortened.contains("Сообщение сокращено"));
    }

    #[test]
    fn file_size_is_formatted_in_decimal_mb() {
        assert_eq!(crate::bot::format::decimal_mb(58_858_228), "58.9 MB");
    }

    #[test]
    fn telegram_compressed_path_uses_sidecar_mp4() {
        let path = std::path::Path::new("data/recordings/a/b/camera_1_segment_1.mp4");

        assert_eq!(
            telegram_compressed_path(path),
            std::path::PathBuf::from("data/recordings/a/b/camera_1_segment_1.telegram.v4.mp4")
        );
    }

    #[test]
    fn tiny_cached_compressed_video_is_treated_as_stale() {
        assert!(!is_usable_telegram_compressed_video(136_508));
        assert!(is_usable_telegram_compressed_video(5_000_000));
        assert!(!is_usable_telegram_compressed_video(51_000_000));
    }
}
