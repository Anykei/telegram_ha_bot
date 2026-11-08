use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration as StdDuration;

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

use super::html_ui::media_pending::{
    Outcome as VideoOutcome, Target as VideoTarget, UploadUncertain,
};
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
const TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditViewResult {
    Updated(UiMessageMode),
    NotModified,
    MessageMissing,
    ModeChanged,
}

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Доступные команды:")]
pub enum Command {
    #[command(description = "Показать главное меню")]
    Start,
    #[command(description = "Помощь / Help")]
    Help,
    #[command(description = "Отменить ввод / Cancel input")]
    Cancel,
    #[command(description = "Выбрать вид интерфейса / Interface")]
    Ui,
    #[command(description = "Кнопки Telegram / Classic interface")]
    Classic,
    #[command(description = "HTML-интерфейс / HTML interface")]
    Html,
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
    config.cancel_view_update(user_id);

    log::info!("Processing command {:?} for user {}", cmd, user_id);

    match cmd {
        Command::Ui | Command::Classic | Command::Html => {
            let mut payload = Payload::Personal(super::personal::PersonalPayload::Interface);
            let unavailable = matches!(cmd, Command::Html) && !config.html_ui.enabled;
            if !matches!(cmd, Command::Ui) && !unavailable {
                let mode = if matches!(cmd, Command::Html) {
                    db::interface::InterfaceMode::Html
                } else {
                    db::interface::InterfaceMode::Classic
                };
                db::interface::set(user_id, mode, &config.db).await?;
                payload = config
                    .sessions
                    .get(&user_id)
                    .and_then(|s| Payload::from_string(&s.current_context).ok())
                    .unwrap_or(Payload::Home)
                    .for_refresh();
                // Calendar drafts belong to the HTML message. A mode change
                // returns to the last applied graph rather than applying a draft.
                if let Payload::Control(crate::bot::router::ControlPayload::QuickAction {
                    room,
                    device,
                    cmd: crate::bot::router::DeviceCmd::EditChartRange { .. },
                }) = payload
                {
                    payload = Payload::Control(crate::bot::router::ControlPayload::DeviceControl {
                        room,
                        device,
                    });
                }
            } else {
                dialogue.exit().await?;
            }
            let mut view = router(payload, user_id, config.clone()).await?;
            if unavailable {
                view.alert = Some(
                    super::personal::words(
                        view.lang,
                        "HTML-интерфейс ещё не включён владельцем бота.",
                        "HTML interface has not been enabled by the bot owner.",
                    )
                    .into(),
                );
            }
            let message = config
                .sessions
                .get(&user_id)
                .map(|s| MessageId(s.last_menu_id));
            if let Some(message) = message {
                update_view(&bot, chat_id, message, user_id, view, config.clone()).await?;
            } else {
                send_new_view(&bot, chat_id, user_id, view, config.clone()).await?;
            }
        }
        Command::Help => {
            let view = router(Payload::Help, user_id, config.clone()).await?;
            let message_id = config
                .sessions
                .get(&user_id)
                .map(|s| MessageId(s.last_menu_id));
            if let Some(message_id) = message_id {
                update_view(&bot, chat_id, message_id, user_id, view, config.clone()).await?;
            } else {
                send_new_view(&bot, chat_id, user_id, view, config.clone()).await?;
            }
        }
        Command::Cancel => {
            let state = dialogue.get().await?.unwrap_or_default();
            let payload = cancel_input_payload(&state, user_id, &config).await?;
            db::command_choices::cancel_all(user_id, &config.db).await?;
            dialogue.exit().await?;
            let mut view = router(payload, user_id, config.clone()).await?;
            view.next_state = None;
            let message_id = config
                .sessions
                .get(&user_id)
                .map(|s| MessageId(s.last_menu_id));
            if let Some(message_id) = message_id {
                update_view(&bot, chat_id, message_id, user_id, view, config.clone()).await?;
            } else {
                send_new_view(&bot, chat_id, user_id, view, config.clone()).await?;
            }
        }
        Command::Start => {
            db::command_choices::cancel_all(user_id, &config.db).await?;
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
    handle_text_command_message(bot, msg, config).await
}

pub async fn handle_text_command_message(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
) -> Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let text = msg.text().unwrap_or("").trim();
    if text.is_empty() {
        return Ok(());
    }
    config.cancel_view_update(user.id.0);
    match handle_text_command(&bot, msg.chat.id, user.id.0, text, config.clone()).await {
        Ok(true) => {
            let _ = bot.delete_message(msg.chat.id, msg.id).await;
        }
        Ok(false) => send_command_help(&bot, msg.chat.id, user.id.0, &config, None).await?,
        Err(error) => {
            // User input and transport details can contain credentials; do not persist them.
            let message = if error.to_string().contains("not found")
                || error.to_string().contains("не найден")
            {
                Some("Target not found. Choose a device / Цель не найдена. Выберите устройство.")
            } else {
                Some("Command could not be completed. Check states / Не удалось выполнить команду. Проверьте состояния.")
            };
            send_command_help(&bot, msg.chat.id, user.id.0, &config, message).await?;
        }
    }
    Ok(())
}

async fn send_command_help(
    bot: &Bot,
    chat: ChatId,
    user: u64,
    config: &Arc<AppConfig>,
    notice: Option<&str>,
) -> Result<()> {
    let lang = db::get_user_language(user, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let en = lang == crate::i18n::Language::En;
    let text = notice.unwrap_or(if en {
        "I did not understand the command. For example: turn on kitchen light or snapshot entrance."
    } else {
        "Не понял команду. Например: включи свет в кухне или снимок вход"
    });
    let text = text
        .split_once(" / ")
        .map(|(english, russian)| if en { english } else { russian })
        .unwrap_or(text);
    let kb = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            if en {
                "Choose a device"
            } else {
                "Выбрать устройство"
            },
            Payload::Control(crate::bot::router::ControlPayload::ListRooms).to_string(),
        )],
        vec![
            InlineKeyboardButton::callback(
                if en { "Help" } else { "Помощь" },
                Payload::Help.to_string(),
            ),
            InlineKeyboardButton::callback(
                if en { "Main menu" } else { "В меню" },
                Payload::Home.to_string(),
            ),
        ],
    ]);
    bot.send_message(chat, text).reply_markup(kb).await?;
    Ok(())
}

pub(crate) fn looks_like_text_command(text: &str) -> bool {
    let normalized = crate::bot::text_commands::normalize_command_text(text);
    normalized.starts_with("снимок ")
        || normalized.starts_with("видео ")
        || normalized.starts_with("архив ")
        || normalized.starts_with("snapshot ")
        || normalized.starts_with("video ")
        || normalized.starts_with("archive ")
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
    let token = config.begin_view_update(user_id);
    crate::db::user::spawn_with_admission(async move {
        let result =
            handle_voice_message_inner(&bot, &msg, user_id, chat_id, config.clone(), &token).await;
        if let Err(error) = result {
            if !token.is_cancelled() {
                handle_voice_command_error(&bot, chat_id, user_id, error, &config).await;
            }
        }
        config.finish_view_update(user_id, &token);
    });
}

async fn handle_voice_command_error(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    error: anyhow::Error,
    config: &Arc<AppConfig>,
) {
    let _ = error;
    let error = "Voice command failed / Не удалось обработать голосовую команду".to_string();
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
    let lang = db::get_user_language(user_id, &config.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(config.default_language);
    let _ = bot
        .send_message(
            chat_id,
            if lang == crate::i18n::Language::En {
                "Could not process the voice command. Please try again."
            } else {
                "Не удалось обработать голосовую команду. Повторите ввод."
            },
        )
        .await;
}

async fn handle_voice_message_inner(
    bot: &Bot,
    msg: &Message,
    user_id: u64,
    chat_id: ChatId,
    config: Arc<AppConfig>,
    token: &tokio_util::sync::CancellationToken,
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

    let (voice_file, mut dst) = crate::core::voice::TemporaryVoice::create(user_id).await?;
    bot.download_file(&file.path, &mut dst).await?;
    tokio::io::AsyncWriteExt::flush(&mut dst).await?;
    drop(dst);

    let lang = db::get_user_language(user_id, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let progress = bot
        .send_message(
            chat_id,
            if lang == crate::i18n::Language::En {
                format!(
                    "⏳ Recognizing voice command… timeout {}s",
                    config.voice_stt_timeout_s
                )
            } else {
                format!(
                    "⏳ Распознаю голосовую команду… таймаут {}с",
                    config.voice_stt_timeout_s
                )
            },
        )
        .await?;
    crate::bot::utils::spawn_delayed_delete(
        bot.clone(),
        chat_id,
        progress.id,
        config.voice_stt_timeout_s + 10,
    );

    let result = async {
        let metadata = tokio::fs::metadata(voice_file.path())
            .await
            .context("Не удалось проверить скачанный voice-файл")?;
        if metadata.len() > max_bytes {
            anyhow::bail!(
                "voice слишком большой: максимум {} MB",
                config.voice_max_audio_size_mb
            );
        }

        let pcm = crate::core::voice::decode_audio_file_to_pcm_mono(
            voice_file.clone(),
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

        if token.is_cancelled() {
            return Ok(());
        }
        let lang = db::get_user_language(user_id, &config.db)
            .await?
            .unwrap_or(config.default_language);
        let _ = bot
            .edit_message_text(
                chat_id,
                progress.id,
                if lang == crate::i18n::Language::En {
                    "Processing…"
                } else {
                    "Обрабатываю…"
                },
            )
            .await;
        if matches!(
            response_format,
            crate::options::VoiceResponseFormat::Voice | crate::options::VoiceResponseFormat::Both
        ) {
            log::debug!("Voice/TTS response format is configured; MVP sends text responses");
        }

        if config.voice_show_recognized_text {
            let sent = bot
                .send_message(
                    chat_id,
                    format!(
                        "{}: {}",
                        if lang == crate::i18n::Language::En {
                            "Recognized"
                        } else {
                            "Распознано"
                        },
                        recognized
                    ),
                )
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
            send_command_help(bot, chat_id, user_id, &config, None).await?;
            return Ok(());
        }

        if !token.is_cancelled() {
            finish_voice_execution(
                bot,
                chat_id,
                user_id,
                progress.id,
                execution,
                config.clone(),
                token,
            )
            .await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    let _ = bot.delete_message(chat_id, progress.id).await;
    result
}

async fn finish_voice_execution(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    progress_id: MessageId,
    execution: CommandExecution,
    config: Arc<AppConfig>,
    token: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let _ = bot.delete_message(chat_id, progress_id).await;
    let allowed = tokio::select! {
        biased;
        _ = token.cancelled() => return Ok(()),
        result = db::access::can_use_voice(user_id, user_id == config.root_user, &config.db) => result?,
    };
    if allowed {
        apply_command_execution_cancellable(bot, chat_id, user_id, execution, config, Some(token))
            .await?;
    }
    Ok(())
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
    apply_command_execution_cancellable(bot, chat_id, user_id, execution, config, None).await
}

/// The action has already completed. Cancellation here only suppresses its
/// obsolete presentation; never cancel or replay the accepted HA operation.
async fn apply_command_execution_cancellable(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    execution: CommandExecution,
    config: Arc<AppConfig>,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Result<()> {
    if cancellation.is_some_and(|token| token.is_cancelled()) {
        return Ok(());
    }
    match execution {
        CommandExecution::Done {
            message,
            displayed_resources,
            admission_generation,
        } => {
            if chat_id.0 == user_id as i64
                && super::view_access::sources_allowed(&displayed_resources, user_id, &config)
                    .await?
                && db::user::admission_is_current(user_id, &admission_generation, &config.db)
                    .await?
                && !cancellation.is_some_and(|token| token.is_cancelled())
            {
                bot.send_message(chat_id, message).await?;
            }
        }
        CommandExecution::NeedsConfirmation { pending_id, .. } => {
            let view = router(
                Payload::Command(CommandPayload::Result { id: pending_id }),
                user_id,
                config.clone(),
            )
            .await?;
            send_new_view_cancellable(bot, chat_id, user_id, view, config, cancellation).await?;
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
                update_view_cancellable(
                    bot,
                    chat_id,
                    message_id,
                    user_id,
                    view,
                    config.clone(),
                    cancellation,
                )
                .await?;
            } else {
                send_new_view_cancellable(
                    bot,
                    chat_id,
                    user_id,
                    view,
                    config.clone(),
                    cancellation,
                )
                .await?;
            }
        }
        CommandExecution::SendCameraSnapshot { camera_id } => {
            spawn_camera_snapshot(bot.clone(), chat_id, user_id, camera_id, config);
        }
        CommandExecution::SendCameraClip { camera_id, seconds } => {
            spawn_video_delivery(
                bot.clone(),
                chat_id,
                user_id,
                VideoTarget::Clip {
                    camera: camera_id,
                    seconds,
                },
                config,
            )
            .await?;
        }
        CommandExecution::NeedsChoice { choice_id } => {
            let view = router(
                Payload::Command(CommandPayload::Choice {
                    id: choice_id,
                    page: 0,
                }),
                user_id,
                config.clone(),
            )
            .await?;
            send_new_view_cancellable(bot, chat_id, user_id, view, config, cancellation).await?;
        }
        CommandExecution::SavedResult { pending_id } => {
            let view = router(
                Payload::Command(CommandPayload::Result { id: pending_id }),
                user_id,
                config.clone(),
            )
            .await?;
            send_new_view_cancellable(bot, chat_id, user_id, view, config, cancellation).await?;
        }
        CommandExecution::NotACommand => {}
    }

    Ok(())
}

/// Основной диспетчер нажатий на кнопки.
pub async fn handle_callback(
    bot: Bot,
    mut q: CallbackQuery,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
) -> Result<()> {
    if super::html_ui::is_widget(&q) {
        let Some(data) = super::html_ui::resolve(&bot, &q, &config).await? else {
            return Ok(());
        };
        q.data = Some(data);
    }
    let data = q.data.as_ref().context("No callback data")?;
    let user_id = q.from.id.0;
    let msg = q.message.as_ref().context("Message missing")?;

    // 2. Декодирование (Infallible logic)
    let payload = match Payload::from_string(data) {
        Ok(payload) => payload,
        Err(_) => {
            if !super::html_ui::is_widget(&q) {
                let _ = super::html_ui::answer(
                    &bot,
                    &q,
                    "Кнопка устарела. Откройте меню командой /start.",
                )
                .await;
            }
            return Ok(());
        }
    };
    if matches!(
        &payload,
        Payload::Control(crate::bot::router::ControlPayload::QuickAction {
            cmd: crate::bot::router::DeviceCmd::ExportChart,
            ..
        })
    ) {
        return super::html_ui::chart_export::send(&bot, &q, &config, &payload).await;
    }
    let is_chart = payload.is_chart_navigation();
    let lang = db::get_user_language(user_id, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let temperature = payload.temperature_command();
    // Critical targets go through the router's persisted confirmation screen.
    // Fail closed on a DB error; the router will render a recoverable error.
    let temperature = if let Some((_, device, _)) = temperature {
        if super::confirmations::is_critical_device(device, &config)
            .await
            .unwrap_or(true)
        {
            None
        } else {
            temperature
        }
    } else {
        None
    };
    let temperature_request = if let Some((room, device, command)) = temperature {
        match config
            .climate_controls
            .enqueue(config.clone(), user_id, device, command)
            .await
        {
            Ok(reply) => Some((room, device, reply)),
            Err(_) => {
                if !super::html_ui::is_widget(&q) {
                    let _ = super::html_ui::answer(&bot, &q, crate::i18n::t(lang, "climate.busy"))
                        .await;
                }
                return Ok(());
            }
        }
    } else {
        None
    };
    config.cancel_view_update(user_id);
    let is_background_view = is_chart
        || payload.is_device_view()
        || payload.is_navigation_view()
        || temperature_request.is_some();
    let answer_text = if temperature_request.is_some() {
        crate::i18n::t(lang, "climate.queued")
    } else if is_chart {
        crate::i18n::t(lang, "sensor.loading")
    } else if is_background_view {
        crate::i18n::t(lang, "control.loading")
    } else if matches!(&payload, Payload::Control(crate::bot::router::ControlPayload::QuickAction { cmd, .. }) if crate::bot::router::action_requires_control(cmd))
    {
        crate::i18n::t(lang, "control.accepted")
    } else {
        ""
    };
    if !super::html_ui::is_widget(&q) {
        let _ = super::html_ui::answer(&bot, &q, answer_text).await;
    }

    if is_background_view {
        dialogue.exit().await?;
        let token = config.begin_view_update(user_id);
        let chat_id = msg.chat().id;
        let message_id = msg.id();
        crate::db::user::spawn_with_admission(async move {
            let result = async {
                // Accepted temperature commands execute once in device order.
                // Cancellation only prevents their old screen from replacing
                // newer navigation; it never interrupts an in-flight HA call.
                let (payload, temperature_result) =
                    if let Some((room, device, reply)) = temperature_request {
                        let outcome = reply.await.context("Climate command worker stopped")?;
                        (
                            Payload::Control(crate::bot::router::ControlPayload::DeviceControl {
                                room,
                                device,
                            }),
                            Some(outcome),
                        )
                    } else {
                        (payload, None)
                    };
                if let Some(Err(error)) = &temperature_result {
                    log::warn!("Temperature command failed for user {}: {}", user_id, error);
                }
                let mut view = tokio::select! {
                    biased;
                    _ = token.cancelled() => return Ok(()),
                    result = router(payload, user_id, config.clone()) => result?,
                };
                if let Some(outcome) = temperature_result {
                    match outcome {
                        Ok(result) => {
                            view.notice =
                                Some(crate::bot::screens::control::temperature_command_notice(
                                    lang, &result,
                                ))
                        }
                        Err(_) => {
                            view.alert = Some(crate::i18n::t(lang, "control.action_failed").into());
                        }
                    }
                }
                update_view_cancellable(
                    &bot,
                    chat_id,
                    message_id,
                    user_id,
                    view,
                    config.clone(),
                    Some(&token),
                )
                .await
            }
            .await;
            config.finish_view_update(user_id, &token);
            if let Err(error) = result {
                log::warn!("Device view update failed for user {}: {}", user_id, error);
                if !token.is_cancelled() {
                    let _ = bot
                        .send_message(
                            chat_id,
                            crate::i18n::t(
                                lang,
                                if is_chart {
                                    "sensor.load_failed"
                                } else {
                                    "control.load_failed"
                                },
                            ),
                        )
                        .await;
                }
            }
        });
        return Ok(());
    }

    if let Payload::Command(command_payload) = payload {
        let token = config.begin_view_update(user_id);
        let chat_id = msg.chat().id;
        let message_id = msg.id();
        crate::db::user::spawn_with_admission(async move {
            let result = handle_command_callback(
                &bot,
                chat_id,
                message_id,
                user_id,
                command_payload,
                config.clone(),
                &token,
            )
            .await;
            if result.is_err() && !token.is_cancelled() {
                let _ = send_command_help(&bot, chat_id, user_id, &config, Some("Command unavailable. Check states / Команда недоступна. Проверьте состояния.")).await;
            }
            config.finish_view_update(user_id, &token);
        });
        return Ok(());
    }

    // Once accepted, a write finishes independently of navigation. Only its
    // obsolete screen is cancelled; cancelling the HA future could lose a receipt.
    let accepted_write = matches!(
        &payload,
        Payload::Control(
            crate::bot::router::ControlPayload::ConfirmAction { .. }
                | crate::bot::router::ControlPayload::ExecuteActionGroup { .. }
                | crate::bot::router::ControlPayload::ExecuteHaNativeAction { .. }
        ) | Payload::Admin(
            AdminPayload::ExecuteActionGroup { .. } | AdminPayload::ExecuteHaNativeAction { .. }
        )
    ) || matches!(&payload, Payload::Control(crate::bot::router::ControlPayload::QuickAction { cmd, .. }) if crate::bot::router::action_requires_control(cmd));
    if accepted_write {
        dialogue.exit().await?;
        let token = config.begin_view_update(user_id);
        let chat = msg.chat().id;
        let message = msg.id();
        crate::db::user::spawn_with_admission(async move {
            let result = async {
                let payload = super::confirmations::prepare_legacy_callback(
                    payload,
                    user_id,
                    chat.0,
                    message.0,
                    config.clone(),
                )
                .await?;
                router(payload, user_id, config.clone()).await
            }
            .await;
            match result {
                Ok(view) => {
                    if let Err(error) = update_view_cancellable(
                        &bot,
                        chat,
                        message,
                        user_id,
                        view,
                        config.clone(),
                        Some(&token),
                    )
                    .await
                    {
                        log::warn!("Accepted action result delivery failed for {user_id}: {error}");
                    }
                }
                Err(error) => {
                    log::warn!("Accepted action failed for {user_id}: {error}");
                    if !token.is_cancelled() {
                        let _ = bot
                            .send_message(chat, crate::i18n::t(lang, "control.load_failed"))
                            .await;
                    }
                }
            }
            config.finish_view_update(user_id, &token);
        });
        return Ok(());
    }
    let payload =
        handle_camera_media_action(&bot, msg.chat().id, user_id, payload, &config).await?;
    let token = config.begin_view_update(user_id);
    let chat_id = msg.chat().id;
    let message_id = msg.id();
    crate::db::user::spawn_with_admission(async move {
        let result = async {
            let view = router(payload, user_id, config.clone()).await?;
            if token.is_cancelled() {
                return Ok(());
            }
            apply_view(
                &bot, &config, &dialogue, chat_id, message_id, user_id, view, &token,
            )
            .await
        }
        .await;
        config.finish_view_update(user_id, &token);
        if let Err(error) = result {
            log::warn!("UI callback failed for {user_id}: {error}");
            if !token.is_cancelled() {
                let _ = bot
                    .send_message(chat_id, crate::i18n::t(lang, "control.load_failed"))
                    .await;
            }
        }
    });
    Ok(())
}

async fn handle_command_callback(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    payload: CommandPayload,
    config: Arc<AppConfig>,
    token: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    match payload {
        CommandPayload::Confirm { id } => {
            let execution = crate::core::commands::confirm_pending(
                user_id,
                user_id == config.root_user,
                id,
                &config,
            )
            .await?;
            {
                // Keep a delayed DELETE from removing a newer screen edited
                // into the same anchor while this result was being delivered.
                let ui_lock = config.ui_lock_for(user_id);
                let _guard = tokio::select! {
                    biased;
                    _ = token.cancelled() => return Ok(()),
                    guard = ui_lock.lock() => guard,
                };
                if token.is_cancelled() {
                    return Ok(());
                }
                let _ = bot.delete_message(chat_id, message_id).await;
            }
            apply_command_execution_cancellable(
                bot,
                chat_id,
                user_id,
                execution,
                config,
                Some(token),
            )
            .await?;
        }
        CommandPayload::Cancel { id } => {
            crate::core::commands::cancel_pending(user_id, id, &config).await?;
            let view = router(
                Payload::Command(CommandPayload::Result { id }),
                user_id,
                config.clone(),
            )
            .await?;
            update_view_cancellable(bot, chat_id, message_id, user_id, view, config, Some(token))
                .await?;
        }
        CommandPayload::Choose { id, target } => {
            match crate::core::commands::choose_target(user_id, id, target, &config).await {
                Ok(execution) => {
                    if !token.is_cancelled() {
                        apply_command_execution_cancellable(
                            bot,
                            chat_id,
                            user_id,
                            execution,
                            config,
                            Some(token),
                        )
                        .await?;
                    }
                }
                Err(_) if !token.is_cancelled() => {
                    send_command_help(bot, chat_id, user_id, &config, Some("Choice unavailable or expired. Repeat the command / Выбор недоступен или истёк. Повторите команду.")).await?;
                }
                Err(_) => {}
            }
        }
        CommandPayload::CancelChoice { id } => {
            let back = db::command_choices::get(id, user_id, &config.db)
                .await?
                .and_then(|r| {
                    serde_json::from_str::<crate::core::commands::ChoiceContext>(&r.context_json)
                        .ok()
                })
                .map(|c| c.back)
                .unwrap_or(Payload::Home);
            db::command_choices::cancel(id, user_id, &config.db).await?;
            let view = router(back.for_refresh(), user_id, config.clone()).await?;
            update_view_cancellable(bot, chat_id, message_id, user_id, view, config, Some(token))
                .await?;
        }
        read @ (CommandPayload::Choice { .. } | CommandPayload::Result { .. }) => {
            let view = router(Payload::Command(read), user_id, config.clone()).await?;
            update_view_cancellable(bot, chat_id, message_id, user_id, view, config, Some(token))
                .await?;
        }
    }

    Ok(())
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
            spawn_camera_snapshot(bot.clone(), chat_id, user_id, id, config.clone());
            Ok(Payload::Camera(CameraPayload::CameraDetail { id }))
        }
        Payload::Camera(CameraPayload::Clip { id, seconds }) => {
            spawn_video_delivery(
                bot.clone(),
                chat_id,
                user_id,
                VideoTarget::Clip {
                    camera: id,
                    seconds,
                },
                config.clone(),
            )
            .await?;
            Ok(Payload::Camera(CameraPayload::CameraDetail { id }))
        }
        Payload::Camera(CameraPayload::SendRecordingSegment {
            camera,
            session,
            segment,
        }) => {
            spawn_video_delivery(
                bot.clone(),
                chat_id,
                user_id,
                VideoTarget::Segment {
                    camera,
                    session,
                    segment,
                },
                config.clone(),
            )
            .await?;
            Ok(Payload::Camera(CameraPayload::RecordingSession {
                camera,
                session,
            }))
        }
        Payload::Camera(CameraPayload::SendRecordingAll { camera, session }) => {
            spawn_video_delivery(
                bot.clone(),
                chat_id,
                user_id,
                VideoTarget::All { camera, session },
                config.clone(),
            )
            .await?;
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
                spawn_video_delivery(
                    bot.clone(),
                    chat_id,
                    user_id,
                    VideoTarget::Clip { camera, seconds },
                    config.clone(),
                )
                .await?;
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
    crate::db::user::spawn_with_admission(async move {
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

async fn spawn_video_delivery(
    bot: Bot,
    chat_id: ChatId,
    user_id: u64,
    target: VideoTarget,
    config: Arc<AppConfig>,
) -> Result<()> {
    // Acquire admission and the overlap guard before the callback renders its receipt.
    let access = CameraDeliveryAccess::new(chat_id, user_id, target.camera(), &config).await?;
    let Some(operation) =
        config
            .html_ui
            .media
            .begin(&config, user_id, &access.generation, target.clone())
    else {
        return Ok(());
    };
    crate::db::user::spawn_with_admission(async move {
        let result = match target {
            VideoTarget::Clip { camera, seconds } => {
                send_camera_clip(&bot, chat_id, user_id, camera, seconds, &config, &access).await
            }
            VideoTarget::Segment {
                camera,
                session,
                segment,
            } => {
                send_recording_segment(
                    &bot, chat_id, user_id, camera, session, segment, &config, &access,
                )
                .await
            }
            VideoTarget::All { camera, session } => {
                send_recording_all(&bot, chat_id, user_id, camera, session, &config, &access).await
            }
        };
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                // Do not print a transport error: it may contain the bot token.
                log::warn!("operation=video_delivery user={} result=failed", user_id);
                VideoOutcome::from_error(&error)
            }
        };
        operation.complete(outcome).await;
    });
    Ok(())
}

pub fn spawn_camera_health_check(user_id: u64, camera_id: i64, config: Arc<AppConfig>) {
    crate::db::user::spawn_with_admission(async move {
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
                let busy = crate::core::cameras::is_capacity_error(&error);
                let error = error.to_string();
                if !busy {
                    let _ = db::camera_health::mark_error(camera.id, &error, &config.db).await;
                }
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

struct CameraDeliveryAccess {
    user_id: u64,
    camera_id: i64,
    generation: String,
    config: Arc<AppConfig>,
}

impl CameraDeliveryAccess {
    async fn new(
        chat_id: ChatId,
        user_id: u64,
        camera_id: i64,
        config: &Arc<AppConfig>,
    ) -> Result<Self> {
        anyhow::ensure!(
            chat_id.0 == user_id as i64,
            "Camera delivery requires a private chat"
        );
        let generation = db::admission_generation(user_id, &config.db)
            .await?
            .context("User admission revoked")?;
        let access = Self {
            user_id,
            camera_id,
            generation,
            config: config.clone(),
        };
        access.check().await?;
        Ok(access)
    }

    async fn check(&self) -> Result<()> {
        let allowed = db::cameras::get_accessible_camera(
            self.user_id,
            self.user_id == self.config.root_user,
            self.camera_id,
            &self.config.db,
        )
        .await?
        .is_some();
        anyhow::ensure!(
            allowed
                && db::admission_is_current(self.user_id, &self.generation, &self.config.db)
                    .await?,
            "Camera delivery access revoked"
        );
        Ok(())
    }
}

async fn send_camera_snapshot(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    config: &Arc<AppConfig>,
) -> Result<()> {
    let access = CameraDeliveryAccess::new(chat_id, user_id, camera_id, config).await?;
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

    access.check().await?;
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
            let busy = crate::core::cameras::is_capacity_error(&error);
            let error = error.to_string();
            if !busy {
                let _ = db::camera_health::mark_error_kind(
                    camera.id,
                    db::camera_health::CameraHealthOperation::Snapshot,
                    &error,
                    &config.db,
                )
                .await;
            }
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
            access.check().await?;
            bot.send_message(chat_id, format!("Не удалось получить снимок: {}", error))
                .await?;
            return Ok(());
        }
    };

    access.check().await?;
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
    access: &CameraDeliveryAccess,
) -> Result<VideoOutcome> {
    access.check().await?;
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
        return Ok(VideoOutcome::failed());
    };

    let seconds = seconds.clamp(1, 120);
    let status = bot
        .send_message(
            chat_id,
            format!("Готовлю видео {}с с камеры «{}»...", seconds, camera.name),
        )
        .await?;

    access.check().await?;
    let capture_result = match capture_clip_from_pre_roll(config, camera.id, seconds).await {
        Some(Ok(bytes)) => Ok(bytes),
        Some(Err(error)) => {
            log::debug!(
                "Pre-roll clip fast path failed, falling back to live capture: camera={}, seconds={}, error={:#}",
                camera.id,
                seconds,
                error
            );
            access.check().await?;
            crate::core::cameras::capture_clip(&camera, seconds).await
        }
        None => crate::core::cameras::capture_clip(&camera, seconds).await,
    };

    let bytes = match capture_result {
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
            let busy = crate::core::cameras::is_capacity_error(&error);
            let error = error.to_string();
            if !busy {
                let _ = db::camera_health::mark_error_kind(
                    camera.id,
                    db::camera_health::CameraHealthOperation::Clip,
                    &error,
                    &config.db,
                )
                .await;
            }
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
            access.check().await?;
            bot.edit_message_text(
                chat_id,
                status.id,
                format!("Не удалось записать видео: {}", error),
            )
            .await?;
            return Ok(VideoOutcome::failed());
        }
    };

    access.check().await?;
    let send_result = send_manual_camera_video(
        bot,
        chat_id,
        bytes,
        &format!("🎞 {} · {}с", camera.name, seconds),
        &access,
    )
    .await;
    let _ = bot.delete_message(chat_id, status.id).await;

    let outcome = match send_result {
        Ok(true) => {
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
            VideoOutcome::sent()
        }
        Ok(false) => {
            let camera_id = camera.id.to_string();
            let _ = log_activity(
                config,
                Some(user_id),
                "camera",
                "camera",
                Some(&camera_id),
                "clip_send",
                "error",
                Some("Video could not fit the Telegram upload limit"),
            )
            .await;
            VideoOutcome::failed()
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
            return Err(error);
        }
    };

    Ok(outcome)
}

async fn send_manual_camera_video(
    bot: &Bot,
    chat_id: ChatId,
    bytes: Vec<u8>,
    caption: &str,
    access: &CameraDeliveryAccess,
) -> Result<bool> {
    access.check().await?;
    let owner = crate::core::cameras::TemporaryMedia::new()?;
    // Cancellation of the awaiting handler must not remove a directory while
    // the blocking writer is still creating or writing its input file.
    let writer_owner = owner.clone();
    tokio::task::spawn_blocking(move || std::fs::write(writer_owner.path(), bytes)).await??;
    send_camera_video_file(
        bot,
        chat_id,
        &owner.path(),
        caption,
        None,
        access,
        Some(owner.clone()),
    )
    .await
}

async fn capture_clip_from_pre_roll(
    config: &Arc<AppConfig>,
    camera_id: i64,
    seconds: u32,
) -> Option<Result<Vec<u8>>> {
    let window_seconds = config
        .camera_pre_roll_registry
        .ready_window_seconds(camera_id)?;
    if seconds == 0 || seconds > window_seconds {
        return None;
    }

    tokio::time::sleep(StdDuration::from_secs(u64::from(seconds))).await;
    let end_at = Utc::now();
    let owner = match crate::core::cameras::TemporaryMedia::new() {
        Ok(owner) => owner,
        Err(error) => return Some(Err(error)),
    };
    let tmp_path = owner.path();
    let result = async {
        config
            .camera_pre_roll_registry
            .write_to_file(
                camera_id,
                seconds,
                end_at,
                end_at,
                tmp_path.clone(),
                Some(owner.clone()),
            )
            .await?;
        let bytes = tokio::fs::read(&tmp_path)
            .await
            .with_context(|| format!("failed to read pre-roll clip {}", tmp_path.display()))?;
        anyhow::ensure!(!bytes.is_empty(), "pre-roll clip is empty");
        Ok(bytes)
    }
    .await;
    drop(owner);

    Some(result)
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
    access: &CameraDeliveryAccess,
) -> Result<VideoOutcome> {
    access.check().await?;
    ensure_recording_access(bot, chat_id, user_id, camera_id, config).await?;

    let Some(segment) = db::camera_recording_segments::get_segment(segment_id, &config.db).await?
    else {
        bot.send_message(chat_id, "Файл записи не найден.").await?;
        return Ok(VideoOutcome::failed());
    };

    if segment.session_id != session_id
        || segment.camera_id != camera_id
        || segment.status != "ready"
    {
        bot.send_message(chat_id, "Эта часть записи еще не готова.")
            .await?;
        return Ok(VideoOutcome::failed());
    }

    let Some(path) = segment.file_path else {
        bot.send_message(chat_id, "Файл записи отсутствует.")
            .await?;
        return Ok(VideoOutcome::failed());
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
            return Ok(VideoOutcome::failed());
        }
    };
    let thumbnail = recording_video_thumbnail(config, &path).await;
    let sent = send_camera_video_file(
        bot,
        chat_id,
        &full_path,
        &format!("🎞 Часть {}", segment.segment_index),
        thumbnail,
        &access,
        None,
    )
    .await?;

    Ok(if sent {
        VideoOutcome::sent()
    } else {
        VideoOutcome::failed()
    })
}

async fn send_recording_all(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    camera_id: i64,
    session_id: i64,
    config: &Arc<AppConfig>,
    access: &CameraDeliveryAccess,
) -> Result<VideoOutcome> {
    access.check().await?;
    ensure_recording_access(bot, chat_id, user_id, camera_id, config).await?;

    let Some(session) = db::camera_recording_sessions::get_session(session_id, &config.db).await?
    else {
        bot.send_message(chat_id, "Запись не найдена.").await?;
        return Ok(VideoOutcome::failed());
    };
    if session.camera_id != camera_id {
        bot.send_message(chat_id, "Недостаточно прав для просмотра записи.")
            .await?;
        return Ok(VideoOutcome::failed());
    }

    let segments =
        db::camera_recording_segments::list_ready_segments(session_id, &config.db).await?;
    if segments.is_empty() {
        bot.send_message(chat_id, "Готовых частей записи нет.")
            .await?;
        return Ok(VideoOutcome::failed());
    }

    let mut files = Vec::new();
    let mut missing_files = 0usize;
    for segment in segments {
        if let Some(path) = segment.file_path {
            match crate::core::camera_recording::recording_file_info(config, &path).await {
                Ok((full_path, _)) => files.push((segment.segment_index, full_path, path)),
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
        } else {
            missing_files += 1;
        }
    }

    if files.is_empty() {
        bot.send_message(
            chat_id,
            "Файлы записи недоступны на диске. Проверьте папку хранения записей и логи сервиса.",
        )
        .await?;
        return Ok(VideoOutcome::failed());
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

    let mut outcome = VideoOutcome {
        failed: missing_files,
        ..Default::default()
    };
    let file_count = files.len();
    for (index, (segment_index, full_path, relative_path)) in files.into_iter().enumerate() {
        if access.check().await.is_err() {
            outcome.failed += file_count - index;
            return Ok(outcome);
        }
        let thumbnail = recording_video_thumbnail(config, &relative_path).await;
        let result = send_camera_video_file(
            bot,
            chat_id,
            &full_path,
            &format!("🎞 Часть {}", segment_index),
            thumbnail,
            &access,
            None,
        )
        .await;
        match result {
            Ok(true) => outcome.sent += 1,
            Ok(false) => outcome.failed += 1,
            Err(error) => {
                outcome.failed += file_count - index;
                outcome.uncertain = error.is::<UploadUncertain>();
                return Ok(outcome);
            }
        }
    }

    Ok(outcome)
}

async fn send_camera_video_file(
    bot: &Bot,
    chat_id: ChatId,
    path: &Path,
    caption: &str,
    thumbnail: Option<Vec<u8>>,
    access: &CameraDeliveryAccess,
    input_owner: Option<crate::core::cameras::TemporaryMedia>,
) -> Result<bool> {
    access.check().await?;
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
            return Ok(false);
        }
    };
    if !metadata.is_file() {
        log::warn!("Recording send path is not a file: path={}", path.display());
        bot.send_message(
            chat_id,
            "Файл записи недоступен на диске. Проверьте папку хранения записей и логи сервиса.",
        )
        .await?;
        return Ok(false);
    }

    let size = metadata.len();
    if size == 0 {
        log::warn!("Recording send file is empty: path={}", path.display());
        bot.send_message(
            chat_id,
            "Файл записи пустой. Проверьте поток камеры и логи сервиса.",
        )
        .await?;
        return Ok(false);
    }
    let (send_path, send_size, media_owner) = if size > TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES {
        match prepare_telegram_compressed_video(path, input_owner.clone()).await {
            Ok(Some((owner, size))) => (owner.path(), size, Some(owner)),
            Ok(None) => {
                access.check().await?;
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
                return Ok(false);
            }
            Err(error) => {
                access.check().await?;
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
                return Ok(false);
            }
        }
    } else {
        (path.to_path_buf(), size, None)
    };

    if send_size > TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES {
        access.check().await?;
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
        return Ok(false);
    }

    let caption = if media_owner.is_some() {
        format!(
            "{} · сжато до {}",
            caption,
            crate::bot::format::decimal_mb(send_size)
        )
    } else {
        caption.to_string()
    };
    let request = bot
        .send_video(chat_id, InputFile::file(send_path))
        .caption(caption)
        .supports_streaming(true);
    let request = if let Some(thumbnail) = thumbnail {
        request.thumbnail(InputFile::memory(thumbnail))
    } else {
        request
    };
    access.check().await?;
    let sent = request.await.map_err(|error| match error {
        RequestError::Network(_) | RequestError::InvalidJson { .. } | RequestError::Io(_) => {
            anyhow::Error::new(UploadUncertain)
        }
        other => anyhow::Error::new(other),
    })?;
    super::utils::spawn_delayed_delete(
        bot.clone(),
        chat_id,
        sent.id,
        access.config.delete_notification_messages_timeout_s,
    );
    drop(media_owner);

    Ok(true)
}

async fn recording_video_thumbnail(config: &Arc<AppConfig>, file_path: &str) -> Option<Vec<u8>> {
    match crate::core::camera_recording::recording_thumbnail(config, file_path).await {
        Ok(bytes) if crate::core::camera_recording::is_usable_telegram_thumbnail(&bytes) => {
            Some(bytes)
        }
        Ok(bytes) => {
            log::debug!(
                "Recording thumbnail skipped: path={}, size={} bytes",
                file_path,
                bytes.len()
            );
            None
        }
        Err(error) => {
            log::debug!(
                "Recording thumbnail unavailable: path={}, error={:#}",
                file_path,
                error
            );
            None
        }
    }
}

async fn prepare_telegram_compressed_video(
    path: &Path,
    input_owner: Option<crate::core::cameras::TemporaryMedia>,
) -> Result<Option<(crate::core::cameras::TemporaryMedia, u64)>> {
    let owner = crate::core::cameras::TemporaryMedia::new()?;
    crate::core::cameras::compress_clip_for_telegram(
        path.to_path_buf(),
        input_owner,
        owner.clone(),
        TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES,
    )
    .await?;
    let size = tokio::fs::metadata(owner.path()).await?.len();
    if is_usable_telegram_compressed_video(size) {
        Ok(Some((owner, size)))
    } else if size < TELEGRAM_MIN_COMPRESSED_VIDEO_BYTES {
        anyhow::bail!(
            "Сжатая копия получилась слишком маленькой: {}",
            crate::bot::format::decimal_mb(size)
        );
    } else {
        Ok(None)
    }
}

fn is_usable_telegram_compressed_video(bytes: u64) -> bool {
    (TELEGRAM_MIN_COMPRESSED_VIDEO_BYTES..=TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES).contains(&bytes)
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

/// Перерисовка после завершения ввода; защита открытой формы относится
/// к фоновому refresh_current_view, а не к переходу после dialog.exit().
pub async fn render_current_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user_id: u64,
    chat_id: ChatId,
    message_id: MessageId,
    context: &str,
) -> Result<()> {
    let decoded = Payload::from_string(context).context("Context decoding failed")?;
    if matches!(
        decoded,
        Payload::Settings(crate::bot::router::SettingsPayload::EditName { .. })
            | Payload::AdminSettings(crate::bot::router::SettingsPayload::EditName { .. })
    ) {
        return Ok(());
    }
    let payload = decoded.for_refresh();
    let token = config.begin_view_update(user_id);
    let result = async {
        let view = tokio::select! {
            biased;
            _ = token.cancelled() => return Ok(()),
            result = router(payload, user_id, config.clone()) => result?,
        };
        update_view_cancellable(
            bot,
            chat_id,
            message_id,
            user_id,
            view,
            config.clone(),
            Some(&token),
        )
        .await
    }
    .await;
    config.finish_view_update(user_id, &token);
    result
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
    let Some(token) = config.try_begin_view_update(user_id) else {
        return Ok(());
    };
    let result =
        refresh_current_view_inner(bot, config, user_id, chat_id, message_id, context, &token)
            .await;
    config.finish_view_update(user_id, &token);
    result
}

async fn refresh_current_view_inner(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user_id: u64,
    chat_id: ChatId,
    message_id: MessageId,
    context: &str,
    token: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    if config.html_ui.input_is_open(user_id, message_id.0) {
        return Ok(());
    }
    if !is_current_session(config, user_id, message_id, context) {
        log::debug!(
            "Skip stale refresh before render for user {} context {}",
            user_id,
            context
        );
        return Ok(());
    }

    let decoded = Payload::from_string(context).context("Context decoding failed")?;
    if matches!(
        decoded,
        Payload::Settings(crate::bot::router::SettingsPayload::EditName { .. })
            | Payload::AdminSettings(crate::bot::router::SettingsPayload::EditName { .. })
    ) {
        return Ok(());
    }
    let payload = decoded.for_refresh();
    // Graphs are refreshed by their controls. Re-uploading them on every heartbeat
    // competes with navigation for Telegram's edit quota.
    if payload.is_chart_navigation() || matches!(payload, Payload::TextPage { .. }) {
        return Ok(());
    }
    let view = tokio::select! {
        biased;
        _ = token.cancelled() => return Ok(()),
        result = router(payload, user_id, config.clone()) => result?,
    };
    let Some(generation) = view_admission(&view, config, user_id, chat_id).await? else {
        return Ok(());
    };
    let payload_str = view.payload.to_string();
    let ui_lock = config.ui_lock_for(user_id);
    let _guard = ui_lock.lock().await;

    if token.is_cancelled() || !is_current_session(config, user_id, message_id, context) {
        log::debug!(
            "Skip stale refresh before edit for user {} context {}",
            user_id,
            context
        );
        return Ok(());
    }

    let result = edit_existing_view(
        bot,
        config,
        user_id,
        chat_id,
        message_id,
        &view,
        Some(token),
    )
    .await;
    if token.is_cancelled() {
        // Telegram may already have applied the edit; the UI lock keeps a newer
        // screen from being sent until this response is resolved. Never commit
        // the old context or recreate a missing message after cancellation.
        if let Err(RequestError::RetryAfter(retry_after)) = &result {
            block_user_ui_refresh(config, user_id, retry_after.chrono_duration());
        }
        return Ok(());
    }
    match result {
        Ok(EditViewResult::Updated(mode)) => {
            crate::core::update_user_state_for_admission(
                config,
                user_id,
                message_id.0,
                &payload_str,
                mode,
                &generation,
            )
            .await;
            Ok(())
        }
        Ok(EditViewResult::NotModified) => Ok(()),
        Ok(EditViewResult::ModeChanged) => {
            reanchor_view_cancellable(
                bot,
                chat_id,
                message_id,
                user_id,
                view,
                config.clone(),
                Some(token),
            )
            .await
        }
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
            reanchor_view_cancellable(
                bot,
                chat_id,
                message_id,
                user_id,
                view,
                config.clone(),
                Some(token),
            )
            .await
        }
    }
}

/// Атомарно применяет изменения стейта и обновляет сообщение.
#[allow(clippy::too_many_arguments)]
async fn apply_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    dialogue: &MyDialogue,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    view: View,
    token: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    {
        let _lifecycle = config.admission_lifecycle.lock().await;
        if token.is_cancelled()
            || view_admission(&view, config, user_id, chat_id)
                .await?
                .is_none()
        {
            return Ok(());
        }
        if let Some(new_state) = view.next_state.clone() {
            dialogue.update(new_state).await?;
        } else {
            dialogue.exit().await?;
        }
    }

    update_view_cancellable(
        bot,
        chat_id,
        message_id,
        user_id,
        view,
        config.clone(),
        Some(token),
    )
    .await
}

pub async fn update_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
) -> anyhow::Result<()> {
    let token = config.begin_view_update(user_id);
    let result = update_view_cancellable(
        bot,
        chat_id,
        message_id,
        user_id,
        view,
        config.clone(),
        Some(&token),
    )
    .await;
    config.finish_view_update(user_id, &token);
    result
}

pub(crate) async fn update_view_cancellable(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> anyhow::Result<()> {
    let Some(generation) = view_admission(&view, &config, user_id, chat_id).await? else {
        return Ok(());
    };
    let mut view = view;
    view.admission_generation = Some(generation.clone());
    super::text_pages::prepare(&mut view, user_id, &config).await?;
    let ui_lock = config.ui_lock_for(user_id);
    let payload_str = view.payload.to_string();
    let mut retries = 0;

    loop {
        let guard = ui_lock.lock().await;
        if cancellation.is_some_and(|token| token.is_cancelled())
            || !db::user::admission_is_current(user_id, &generation, &config.db).await?
        {
            return Ok(());
        }
        let delay = config.sessions.get(&user_id).and_then(|session| {
            session.ui_refresh_blocked_until.and_then(|until| {
                (until - Utc::now())
                    .to_std()
                    .ok()
                    .filter(|delay| !delay.is_zero())
            })
        });
        if let Some(delay) = delay {
            drop(guard);
            if let Some(token) = cancellation {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(delay) => {}
                }
            } else {
                tokio::time::sleep(delay).await;
            }
            continue;
        }
        let message_id = config
            .sessions
            .get(&user_id)
            .map(|session| MessageId(session.last_menu_id))
            .unwrap_or(message_id);
        let result = edit_existing_view(
            bot,
            &config,
            user_id,
            chat_id,
            message_id,
            &view,
            cancellation,
        )
        .await;
        if cancellation.is_some_and(|token| token.is_cancelled()) {
            if let Err(RequestError::RetryAfter(retry_after)) = &result {
                block_user_ui_refresh(&config, user_id, retry_after.chrono_duration());
            }
            return Ok(());
        }
        match result {
            Ok(EditViewResult::Updated(mode)) => {
                crate::core::update_user_state_for_admission(
                    &config,
                    user_id,
                    message_id.0,
                    &payload_str,
                    mode,
                    &generation,
                )
                .await;
                return Ok(());
            }
            Ok(EditViewResult::NotModified) => return Ok(()),
            Ok(EditViewResult::MessageMissing | EditViewResult::ModeChanged) => {
                match reanchor_view_cancellable(
                    bot,
                    chat_id,
                    message_id,
                    user_id,
                    view.clone(),
                    config.clone(),
                    cancellation,
                )
                .await
                {
                    Err(error)
                        if matches!(
                            error.downcast_ref::<RequestError>(),
                            Some(RequestError::RetryAfter(_))
                        ) =>
                    {
                        let RequestError::RetryAfter(retry_after) =
                            error.downcast_ref::<RequestError>().unwrap()
                        else {
                            unreachable!()
                        };
                        let until =
                            block_user_ui_refresh(&config, user_id, retry_after.chrono_duration());
                        log_telegram_retry_after(
                            "UI mode change",
                            user_id,
                            retry_after.seconds(),
                            until,
                        );
                        if retries >= 2 {
                            return Err(error);
                        }
                        retries += 1;
                        // The next iteration waits without holding the UI lock.
                        // Retry only the new message; the action has already completed.
                    }
                    outcome => return outcome,
                }
            }
            Err(RequestError::RetryAfter(retry_after)) => {
                let blocked_until =
                    block_user_ui_refresh(&config, user_id, retry_after.chrono_duration());
                log_telegram_retry_after(
                    "UI update",
                    user_id,
                    retry_after.seconds(),
                    blocked_until,
                );
                if retries >= 2 {
                    return Err(RequestError::RetryAfter(retry_after).into());
                }
                retries += 1;
                // Release the UI lock while waiting so a newer graph request or Back
                // can supersede this update. Only message edits are retried.
                drop(guard);
                let delay = (blocked_until - Utc::now()).to_std().unwrap_or_default();
                if let Some(token) = cancellation {
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(delay) => {}
                    }
                } else {
                    tokio::time::sleep(delay).await;
                }
            }
            Err(e) => {
                log::info!("UI Mode transition for user {}: {}", user_id, e);
                return reanchor_view_cancellable(
                    bot,
                    chat_id,
                    message_id,
                    user_id,
                    view,
                    config,
                    cancellation,
                )
                .await;
            }
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
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> std::result::Result<EditViewResult, RequestError> {
    if view_admission(view, config, user_id, chat_id)
        .await
        .ok()
        .flatten()
        .is_none()
        || cancellation.is_some_and(|token| token.is_cancelled())
    {
        return Ok(EditViewResult::NotModified);
    }
    let text = view.get_text();
    let kb = view.kb.clone();
    let current_mode = config.sessions.get(&user_id).map(|s| s.ui_message_mode);
    if super::html_ui::preferred(config, user_id).await && super::html_ui::eligible(view) {
        if current_mode != Some(UiMessageMode::Html)
            || !super::html_ui::renderer_matches(config, user_id, view)
        {
            return Ok(EditViewResult::ModeChanged);
        }
        return match super::html_ui::deliver(
            bot,
            config,
            user_id,
            chat_id,
            Some(message_id),
            view,
            cancellation,
        )
        .await
        {
            Ok(Some(_)) => Ok(EditViewResult::Updated(UiMessageMode::Html)),
            Ok(None) => Ok(EditViewResult::NotModified),
            Err(error) => Err(error
                .downcast_ref::<RequestError>()
                .cloned()
                .unwrap_or_else(|| {
                    RequestError::Api(teloxide::ApiError::Unknown(error.to_string()))
                })),
        };
    }
    let edit_mode = if view.text_page.is_some() && view.image.is_none() {
        UiMessageMode::Text
    } else {
        view_mode(&view.get_plain_text())
    };
    if current_mode.is_some_and(|mode| mode != edit_mode) {
        return Ok(EditViewResult::ModeChanged);
    }
    if edit_mode == UiMessageMode::Text {
        log::debug!(
            "UI view for user {} payload {} uses text mode: {} chars, caption limit {}, current message mode {:?}",
            user_id,
            view.payload.to_string(),
            text.chars().count(),
            TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS,
            config.sessions.get(&user_id).map(|session| session.ui_message_mode)
        );
        let text = view.get_plain_text();
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

    let mut delivery_view = view.clone();
    let input_file =
        InputFile::memory(resolve_view_image(&mut delivery_view, user_id, config).await);
    if view_admission(&delivery_view, config, user_id, chat_id)
        .await
        .ok()
        .flatten()
        .is_none()
        || cancellation.is_some_and(|token| token.is_cancelled())
    {
        return Ok(EditViewResult::NotModified);
    }

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

async fn reanchor_view_cancellable(
    bot: &Bot,
    chat_id: ChatId,
    old_message_id: MessageId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Result<()> {
    if !send_new_view_inner(bot, chat_id, user_id, view, config, cancellation).await? {
        return Ok(());
    }

    let b = bot.clone();
    crate::db::user::spawn_with_admission(async move {
        let _ = b.delete_message(chat_id, old_message_id).await;
    });

    Ok(())
}

pub(super) async fn view_admission(
    view: &View,
    config: &AppConfig,
    user_id: u64,
    chat_id: ChatId,
) -> Result<Option<String>> {
    if chat_id.0 != user_id as i64 {
        return Ok(None);
    }
    let current = db::user::admission_generation(user_id, &config.db).await?;
    if view
        .admission_generation
        .as_ref()
        .is_some_and(|expected| current.as_ref() != Some(expected))
    {
        return Ok(None);
    }
    let Some(generation) = current else {
        return Ok(None);
    };
    // Validate identities captured when values/names were read, not targets
    // inferred from today's room membership or from action-only callbacks.
    let sources: Vec<_> = view
        .displayed_resources
        .iter()
        .chain(
            view.notifications
                .iter()
                .flat_map(|item| item.source.iter().chain(item.additional_sources.iter())),
        )
        .cloned()
        .collect();
    if !super::view_access::sources_allowed(&sources, user_id, config).await? {
        return Ok(None);
    }
    for notification in &view.notifications {
        if let Some(crate::core::types::DisplaySource::Device { entity_id, .. }) =
            &notification.source
        {
            if !db::access::can_notify_entity(user_id, entity_id, &config.db).await? {
                return Ok(None);
            }
        }
    }
    // Rendered data can wait behind the UI lock, media generation, or a retry.
    // Recheck the source screen, including owner-bound text pages, at delivery.
    use crate::bot::router::{ControlPayload, SettingsPayload};
    let source = super::text_pages::source_payload(view, user_id, config).await?;
    let device = match source {
        Payload::DeviceDetails { device, .. }
        | Payload::Control(
            ControlPayload::DeviceControl { device, .. }
            | ControlPayload::QuickAction { device, .. },
        ) => Some((device, false)),
        Payload::TemperatureInput { device, .. } => Some((device, true)),
        Payload::Settings(settings) | Payload::AdminSettings(settings) => match settings {
            SettingsPayload::DeviceDetail { device, .. }
            | SettingsPayload::ToggleNotify { device, .. }
            | SettingsPayload::ToggleHide { device, .. }
            | SettingsPayload::EditName { device, .. }
            | SettingsPayload::StateAliases { device, .. }
            | SettingsPayload::EditStateAlias { device, .. }
            | SettingsPayload::StateAliasEditContext { device, .. }
            | SettingsPayload::StateAliasResetContext { device, .. }
            | SettingsPayload::ResetStateAlias { device, .. }
            | SettingsPayload::ToggleStateInversion { device, .. }
            | SettingsPayload::ToggleCritical { device, .. } => Some((device, false)),
            _ => None,
        },
        Payload::TextPage { .. } => return Ok(None),
        _ => None,
    };
    if let Some((device, control)) = device {
        let access = db::access::get_effective_device_access(
            user_id,
            user_id == config.root_user,
            device,
            &config.db,
        )
        .await?;
        if !access.can_view || (control && !access.can_control) {
            return Ok(None);
        }
    }
    if !db::admission_is_current(user_id, &generation, &config.db).await? {
        return Ok(None);
    }
    Ok(Some(generation))
}

async fn clear_ghost_session(config: &Arc<AppConfig>, user_id: u64) {
    config.sessions.remove(&user_id);

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

async fn resolve_view_image(view: &mut View, user_id: u64, config: &Arc<AppConfig>) -> Vec<u8> {
    if let Some(image) = &view.image {
        return non_empty_image_or_placeholder(Some(image.as_slice()), "view image");
    }

    let background = crate::core::ui_background::resolve(config.clone(), user_id).await;
    if let Some((id, bytes)) = background {
        view.displayed_resources
            .push(crate::core::types::DisplaySource::Camera { id });
        non_empty_image_or_placeholder(Some(&bytes), "UI background")
    } else {
        non_empty_image_or_placeholder(None, "UI background")
    }
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
    send_new_view_cancellable(bot, chat_id, user_id, view, config, None).await
}

async fn send_new_view_cancellable(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Result<()> {
    // Serialize new messages and session publication with edits/reanchors. An
    // in-flight Telegram request must finish before the newer screen is sent.
    let ui_lock = config.ui_lock_for(user_id);
    let _guard = if let Some(token) = cancellation {
        tokio::select! {
            biased;
            _ = token.cancelled() => return Ok(()),
            guard = ui_lock.lock() => guard,
        }
    } else {
        ui_lock.lock().await
    };
    send_new_view_inner(bot, chat_id, user_id, view, config, cancellation)
        .await
        .map(|_| ())
}

pub(crate) async fn send_new_view_inner(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Result<bool> {
    if cancellation.is_some_and(|token| token.is_cancelled()) {
        return Ok(false);
    }
    let Some(generation) = view_admission(&view, &config, user_id, chat_id).await? else {
        return Ok(false);
    };
    let mut view = view;
    view.admission_generation = Some(generation.clone());
    super::text_pages::prepare(&mut view, user_id, &config).await?;
    let payload_str = view.payload.to_string();

    if super::html_ui::preferred(&config, user_id).await {
        match super::html_ui::deliver(bot, &config, user_id, chat_id, None, &view, cancellation)
            .await
        {
            Ok(Some(message)) => {
                crate::core::update_user_state_for_admission(
                    &config,
                    user_id,
                    message.0,
                    &payload_str,
                    UiMessageMode::Html,
                    &generation,
                )
                .await;
                return Ok(true);
            }
            Ok(None) => {}
            Err(error) if error.downcast_ref::<RequestError>().is_some() => return Err(error),
            Err(error) => log::warn!("HTML delivery failed; using classic view: {error}"),
        }
        if view_admission(&view, &config, user_id, chat_id)
            .await?
            .is_none()
            || cancellation.is_some_and(|token| token.is_cancelled())
        {
            return Ok(false);
        }
        view.notice.get_or_insert_with(|| {
            super::personal::words(
                view.lang,
                "Этот экран открыт обычным видом. Выбор HTML сохранён.",
                "This screen uses the classic view. Your HTML preference is saved.",
            )
            .into()
        });
        // A failed HTML send must still fit native Telegram limits. The earlier
        // preparation deliberately skipped the classic pager for HTML content.
        super::text_pages::prepare_classic(&mut view, user_id, &config).await?;
    }
    let payload_str = view.payload.to_string();
    let text = view.get_text();

    if (view.text_page.is_some() && view.image.is_none())
        || view_mode(&view.get_plain_text()) == UiMessageMode::Text
    {
        log::debug!(
            "New UI view for user {} payload {} uses text mode: {} chars exceed Telegram photo caption limit {}",
            user_id,
            view.payload.to_string(),
            text.chars().count(),
            TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS
        );
        if view_admission(&view, &config, user_id, chat_id)
            .await?
            .is_none()
            || cancellation.is_some_and(|token| token.is_cancelled())
        {
            return Ok(false);
        }
        let sent = bot
            .send_message(chat_id, view.get_plain_text())
            .reply_markup(view.kb)
            .await?;

        if cancellation.is_some_and(|token| token.is_cancelled()) {
            let _ = bot.delete_message(chat_id, sent.id).await;
            return Ok(false);
        }
        crate::core::update_user_state_for_admission(
            &config,
            user_id,
            sent.id.0,
            &payload_str,
            UiMessageMode::Text,
            &generation,
        )
        .await;
        config.html_ui.clear(user_id);
        return Ok(true);
    }

    // Исправлено: send_photo принимает InputFile, а не InputMedia
    let image = resolve_view_image(&mut view, user_id, &config).await;
    let input_file = InputFile::memory(image);
    if view_admission(&view, &config, user_id, chat_id)
        .await?
        .is_none()
        || cancellation.is_some_and(|token| token.is_cancelled())
    {
        return Ok(false);
    }

    let sent = bot
        .send_photo(chat_id, input_file)
        .caption(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(view.kb)
        .await?;

    if cancellation.is_some_and(|token| token.is_cancelled()) {
        let _ = bot.delete_message(chat_id, sent.id).await;
        return Ok(false);
    }
    // Критическая правка: сохраняем ID СООБЩЕНИЯ БОТА (sent.id), а не входящего апдейта
    crate::core::update_user_state_for_admission(
        &config,
        user_id,
        sent.id.0,
        &payload_str,
        UiMessageMode::Photo,
        &generation,
    )
    .await;
    config.html_ui.clear(user_id);
    Ok(true)
}

fn view_mode(text: &str) -> UiMessageMode {
    // Compact formatting must not disable the configured background. Text is
    // only a fallback for captions that exceed Telegram's photo limit.
    if should_send_view_as_text(text) {
        UiMessageMode::Text
    } else {
        UiMessageMode::Photo
    }
}

fn should_send_view_as_text(text: &str) -> bool {
    super::text_pages::text_len(text) > TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS
}

// --- ОБРАБОТЧИКИ ДИАЛОГОВ ---

pub(crate) async fn cancel_input_payload(
    state: &State,
    user: u64,
    config: &AppConfig,
) -> Result<Payload> {
    use crate::bot::router::{ControlPayload, SettingsPayload};
    if let State::WaitingForNameDraft { draft_id, .. } = state {
        sqlx::query("UPDATE ui_drafts SET status='cancelled',revision=revision+1 WHERE id=? AND owner_id=? AND kind='device_name' AND status='editing'")
            .bind(draft_id).bind(user as i64).execute(&config.db).await?;
    }
    Ok(match state {
        State::EditUserAlias { user_id } => {
            Payload::Admin(AdminPayload::UserProfile { id: *user_id })
        }
        State::ScheduleDraftTime { id, .. } => {
            Payload::Schedule(super::schedule_drafts::SchedulePayload::Review { id: *id })
        }
        State::PersonalQuietHours => Payload::Personal(super::personal::PersonalPayload::Settings),
        State::ExactTemperature { room_id, device_id }
        | State::WaitingForGraphInterval { room_id, device_id } => {
            Payload::Control(ControlPayload::DeviceControl {
                room: *room_id,
                device: *device_id,
            })
        }
        State::WaitingForName { room_id, device_id }
        | State::WaitingForNameDraft {
            room_id, device_id, ..
        }
        | State::WaitingForStateAlias {
            room_id, device_id, ..
        } => {
            let back = SettingsPayload::DeviceDetail {
                room: *room_id,
                device: *device_id,
            };
            if matches!(
                current_input_payload(config, user).await?,
                Some(Payload::AdminSettings(_))
            ) {
                Payload::AdminSettings(back)
            } else {
                settings_payload_for_current_session(config, Some(user), back).await?
            }
        }
        State::AddActionScheduleTime { target, .. }
        | State::EditActionScheduleTime { target, .. } => {
            Payload::Admin(AdminPayload::ActionSchedules { target: *target })
        }
        State::EditRecordingRule { room_id, rule_id }
        | State::EditRecordingRuleNumber {
            room_id, rule_id, ..
        }
        | State::EditRecordingRuleActiveTime { room_id, rule_id }
        | State::EditRecordingRuleConditionValue {
            room_id, rule_id, ..
        }
        | State::AddRecordingRuleGroupForEdit { room_id, rule_id } => {
            Payload::Admin(AdminPayload::RecordingRuleDetail {
                room: *room_id,
                rule: *rule_id,
            })
        }
        State::RecordingRuleWizardSourceValue { .. }
        | State::RecordingRuleWizardActiveTimeValue
        | State::RecordingRuleWizardConditionValue
        | State::AddRecordingRuleGroupForWizard => {
            let current_id = match current_input_payload(config, user).await? {
                Some(Payload::Admin(
                    AdminPayload::WizardResume { id } | AdminPayload::WizardDraft { id, .. },
                )) => Some(id),
                _ => config.sessions.get(&user).and_then(|session| {
                    session
                        .recording_rule_wizard
                        .as_ref()
                        .and_then(|wizard| wizard.draft_id)
                }),
            };
            let draft = match current_id {
                Some(id) => db::ui_drafts::load(id, user, "recording_rule", &config.db).await?,
                None => db::ui_drafts::latest(user, "recording_rule", &config.db).await?,
            };
            draft
                .map(|draft| Payload::Admin(AdminPayload::WizardResume { id: draft.id }))
                .unwrap_or(Payload::Home)
        }
        State::AddRecordingRule { room_id } => {
            match recording_text_draft(config, user, *room_id).await {
                Ok(Some(wizard)) => wizard
                    .draft_id
                    .map(|id| Payload::Admin(AdminPayload::WizardResume { id }))
                    .unwrap_or(Payload::Home),
                _ => Payload::Admin(AdminPayload::RecordingRules { room: *room_id }),
            }
        }
        State::AddCamera { room_id } => {
            Payload::Admin(AdminPayload::RoomCameras { room: *room_id })
        }
        State::RenameActionGroup { group_id } => {
            Payload::Admin(AdminPayload::ActionGroupDetail { group: *group_id })
        }
        State::RenameHaNativeTarget { target_id } => {
            Payload::Admin(AdminPayload::HaNativeActionDetail { action: *target_id })
        }
        State::Idle => match current_input_payload(config, user).await? {
            Some(Payload::Command(
                CommandPayload::Choice { id, .. }
                | CommandPayload::Choose { id, .. }
                | CommandPayload::CancelChoice { id },
            )) => db::command_choices::get(id, user, &config.db)
                .await?
                .and_then(|record| {
                    serde_json::from_str::<crate::core::commands::ChoiceContext>(
                        &record.context_json,
                    )
                    .ok()
                })
                .map(|context| context.back.for_refresh())
                .unwrap_or(Payload::Home),
            _ => Payload::Home,
        },
        _ => Payload::Home,
    })
}

pub async fn handle_custom_interval(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (device_id, room_id): (i64, i64),
) -> Result<()> {
    let text = msg.text().unwrap_or("").trim();

    // Пытаемся распарсить ввод
    if let Some(hours) = text.parse::<u32>().ok().filter(|hours| {
        (1..=crate::bot::screens::control::sensor_view::MAX_PERIOD_HOURS).contains(hours)
    }) {
        let user_id = msg.from.as_ref().context("User context missing")?.id.0;
        let mut new_payload = crate::bot::screens::control::sensor_view::saved_chart_payload(
            user_id, room_id, device_id, &config,
        )
        .await?;
        if let Payload::Control(crate::bot::router::ControlPayload::QuickAction { cmd, .. }) =
            &mut new_payload
        {
            match cmd {
                crate::bot::router::DeviceCmd::ShowChart { h, .. } => *h = hours,
                crate::bot::router::DeviceCmd::ShowChartRange { start, end } => {
                    *start = *end - i64::from(hours) * 3600
                }
                _ => {}
            }
        }

        // Завершаем диалог с ПЕРЕХОДОМ на новый график
        return finalize_dialogue(bot, dialogue, msg, config, Some(new_payload)).await;
    }

    keep_dialogue_with_error(
        &bot,
        &dialogue,
        &msg,
        State::WaitingForGraphInterval { device_id, room_id },
        "⚠️ Ошибка: введите целое число часов от 1 до 240.".to_string(),
    )
    .await
}

pub async fn handle_device_name_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    (device_id, room_id, draft_id): (i64, i64, Option<i64>),
) -> Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let user_id = user.id.0;
    if !msg.chat.is_private() || msg.chat.id.0 != user_id as i64 || user_id != config.root_user {
        return Ok(());
    }
    let lang = db::get_user_language(user_id, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let Some(draft_id) = draft_id else {
        bot.send_message(
            msg.chat.id,
            super::personal::words(
                lang,
                "Форма устарела. Нажмите «Изменить имя» снова.",
                "This form expired. Open Rename again.",
            ),
        )
        .await?;
        return Ok(());
    };
    let Some(raw) = msg.text() else {
        return Ok(());
    };
    if raw.starts_with('/') {
        return Ok(());
    }
    let Ok(name) = db::devices::validate_device_name(raw) else {
        bot.send_message(
            msg.chat.id,
            super::personal::words(
                lang,
                "Введите имя длиной 1–128 символов без переносов строки и управляющих символов.",
                "Enter a name of 1–128 characters without line breaks or control characters.",
            ),
        )
        .await?;
        return Ok(());
    };
    let back = settings_payload_for_current_session(
        &config,
        Some(user_id),
        crate::bot::router::SettingsPayload::DeviceDetail {
            room: room_id,
            device: device_id,
        },
    )
    .await?;
    {
        let _lifecycle = config.admission_lifecycle.lock().await;
        if !db::user_exists(user_id, &config.db).await {
            return Ok(());
        }
        let current = dialogue.get().await?.unwrap_or_default();
        if !matches!(current, State::WaitingForNameDraft { device_id: d, room_id: r, draft_id: draft } if d == device_id && r == room_id && draft == draft_id)
        {
            return Ok(());
        }
        let expected = crate::bot::router::SettingsPayload::EditName {
            room: room_id,
            device: device_id,
        };
        if !matches!(current_input_payload(&config, user_id).await?,
            Some(Payload::Settings(payload) | Payload::AdminSettings(payload)) if payload == expected)
        {
            return Ok(());
        }
        let Some(device) = db::devices::get_device_by_id(device_id, &config.db).await? else {
            return Ok(());
        };
        if !db::devices::commit_name_draft(user_id, device_id, draft_id, name, &config.db).await? {
            return Ok(());
        }
        config
            .name_aliases
            .insert(device.entity_id, name.to_owned());
    }
    finalize_dialogue(bot, dialogue, msg, config, Some(back)).await
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
    )
    .await?;

    finalize_dialogue(bot, dialogue, msg, config, Some(target_payload)).await
}

async fn settings_payload_for_current_session(
    config: &AppConfig,
    user_id: Option<u64>,
    payload: crate::bot::router::SettingsPayload,
) -> Result<Payload> {
    let from_admin_settings = if let Some(user) = user_id {
        matches!(
            current_input_payload(config, user).await?,
            Some(Payload::AdminSettings(_))
        )
    } else {
        false
    };
    Ok(if from_admin_settings {
        Payload::AdminSettings(payload)
    } else {
        Payload::Settings(payload)
    })
}

pub async fn handle_user_alias_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: MyDialogue,
    user_id: u64,
) -> Result<()> {
    if msg.from.as_ref().map(|user| user.id.0) != Some(config.root_user) {
        return finalize_dialogue(bot, dialogue, msg, config, Some(Payload::Home)).await;
    }
    if user_id != config.root_user && !crate::db::user_exists(user_id, &config.db).await {
        return finalize_dialogue(
            bot,
            dialogue,
            msg,
            config,
            Some(Payload::Admin(crate::bot::router::AdminPayload::ListUsers)),
        )
        .await;
    }
    let lang = crate::db::get_user_language(config.root_user, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let input = msg.text().unwrap_or("").trim();
    if input.is_empty() || input.chars().count() > 80 {
        bot.send_message(
            msg.chat.id,
            if lang == crate::i18n::Language::Ru {
                "Введите псевдоним длиной от 1 до 80 символов или - для сброса."
            } else {
                "Enter an alias between 1 and 80 characters, or - to reset."
            },
        )
        .await?;
        return Ok(());
    }
    crate::db::user_display::set_alias(user_id, (input != "-").then_some(input), &config.db)
        .await?;
    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Admin(
            crate::bot::router::AdminPayload::UserProfile { id: user_id },
        )),
    )
    .await
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

async fn current_input_payload(config: &AppConfig, owner: u64) -> Result<Option<Payload>> {
    let mut payload = config
        .sessions
        .get(&owner)
        .and_then(|session| Payload::from_string(&session.current_context).ok());
    if let Some(Payload::TextPage { screen, .. }) = payload {
        payload = db::ui_text_pages::context(screen, owner, &config.db)
            .await?
            .and_then(|context| serde_json::from_str::<serde_json::Value>(&context).ok())
            .and_then(|context| {
                context
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|source| Payload::from_string(source).ok())
            });
    }
    Ok(payload)
}

pub(crate) async fn recording_text_draft(
    config: &AppConfig,
    owner: u64,
    room: i64,
) -> Result<Option<crate::bot::recording_rule_wizard::RecordingRuleWizard>> {
    let context = current_input_payload(config, owner).await?;
    let current = config
        .sessions
        .get(&owner)
        .and_then(|session| session.recording_rule_wizard.clone());
    // The independent legacy form must not accidentally edit an unrelated saved draft.
    if matches!(
        context,
        Some(Payload::Admin(AdminPayload::PromptAddRecordingRule { .. }))
    ) {
        return Ok(None);
    }
    let context_id = match context {
        Some(Payload::Admin(
            AdminPayload::WizardResume { id } | AdminPayload::WizardDraft { id, .. },
        )) => Some(id),
        _ => None,
    };
    let from_wizard =
        context_id.is_some() || current.as_ref().is_some_and(|wizard| wizard.editing_text);
    if !from_wizard {
        return Ok(None);
    }
    let id = context_id.or_else(|| current.as_ref().and_then(|wizard| wizard.draft_id));
    let wizard = if id.is_some() {
        crate::bot::recording_rule_wizard::restore_draft(config, owner, id).await?
    } else {
        current
    };
    let wizard = wizard
        .filter(|wizard| {
            wizard.editing_text && wizard.room_id == room && wizard.committed_rule_id.is_none()
        })
        .context("Recording draft is expired or no longer accepting text")?;
    if let Some(mut session) = config.sessions.get_mut(&owner) {
        session.recording_rule_wizard = Some(wizard.clone());
    }
    Ok(Some(wizard))
}

async fn restore_recording_wizard_input(config: &AppConfig, owner: u64) -> Result<()> {
    let current = config
        .sessions
        .get(&owner)
        .and_then(|session| session.recording_rule_wizard.clone());
    let context = current_input_payload(config, owner).await?;
    let id = match context {
        Some(Payload::Admin(
            AdminPayload::WizardResume { id } | AdminPayload::WizardDraft { id, .. },
        )) => Some(id),
        _ => current.as_ref().and_then(|wizard| wizard.draft_id),
    };
    if current.is_some() && id.is_none() {
        return Ok(());
    }
    let saved = crate::bot::recording_rule_wizard::restore_draft(config, owner, id).await?;
    if let Some(mut session) = config.sessions.get_mut(&owner) {
        session.recording_rule_wizard = saved;
    }
    Ok(())
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

    let user_id = msg.from.as_ref().context("User context missing")?.id.0;
    let editing_wizard = match recording_text_draft(&config, user_id, room_id).await {
        Ok(wizard) => wizard,
        Err(_) => return finish_stale_wizard_input(&bot, &dialogue, &msg).await,
    };
    if editing_wizard.is_some() {
        if let Some(mut session) = config.sessions.get_mut(&user_id) {
            if let Some(wizard) = session.recording_rule_wizard.as_mut() {
                wizard.raw_text = Some(msg.text().unwrap_or("").to_owned());
            }
        }
        crate::bot::recording_rule_wizard::persist_draft(&config, user_id).await?;
    }

    match parse_recording_rule_input(
        msg.text().unwrap_or(""),
        config.camera_recording_max_tail_seconds,
        config.camera_recording_max_segment_seconds,
    ) {
        Ok(input) => {
            let cameras = crate::db::cameras::list_room_cameras(room_id, &config.db).await?;
            if !cameras.iter().any(|camera| camera.id == input.camera_id) {
                if editing_wizard.is_some() {
                    return keep_dialogue_with_error(
                        &bot,
                        &dialogue,
                        &msg,
                        State::AddRecordingRule { room_id },
                        "Строка 2: камера не принадлежит выбранной комнате.".into(),
                    )
                    .await;
                }
                let err_msg = bot
                    .send_message(
                        msg.chat.id,
                        "Ошибка: камера не принадлежит выбранной комнате.",
                    )
                    .await?;
                crate::bot::utils::spawn_delayed_delete(bot.clone(), msg.chat.id, err_msg.id, 8);
                return finalize_dialogue(bot, dialogue, msg, config, None).await;
            }

            if let Some(mut wizard) = editing_wizard {
                // Parse first; assign the complete legacy block only after all checks pass.
                // Fields absent from that block (groups/window/pre-roll) remain unchanged.
                let conditions = input
                    .conditions
                    .iter()
                    .map(
                        |condition| crate::bot::recording_rule_wizard::WizardCondition {
                            entity_id: condition.entity_id.clone(),
                            operator: condition.operator,
                            from_state: condition.from_state.clone(),
                            to_state: condition.to_state.clone(),
                            value: condition.value.clone(),
                        },
                    )
                    .collect::<Vec<_>>();
                let source_id: Option<i64> =
                    sqlx::query_scalar("SELECT id FROM devices WHERE entity_id=? AND archived=0")
                        .bind(&conditions[0].entity_id)
                        .fetch_optional(&config.db)
                        .await?;
                if wizard.source_device_id.is_none() {
                    wizard.source_device_id = source_id;
                }
                if wizard.source_mode.is_none() {
                    wizard.source_mode =
                        Some(crate::bot::recording_rule_wizard::WizardTriggerMode::AnyChange);
                }
                // Use the revision after retaining the user's raw text.
                if let Some(current) = config
                    .sessions
                    .get(&user_id)
                    .and_then(|session| session.recording_rule_wizard.clone())
                {
                    wizard.revision = current.revision;
                }
                apply_recording_rule_input_to_wizard(&mut wizard, input, conditions);
                if let Some(mut session) = config.sessions.get_mut(&user_id) {
                    session.recording_rule_wizard = Some(wizard);
                }
                crate::bot::recording_rule_wizard::persist_draft(&config, user_id).await?;
                return finalize_dialogue(
                    bot,
                    dialogue,
                    msg,
                    config,
                    Some(Payload::Admin(AdminPayload::WizardConfirmGroups)),
                )
                .await;
            }

            let pre_roll_enabled = crate::db::settings::get_i64(
                crate::db::settings::CAMERA_PRE_ROLL_ENABLED,
                &config.db,
            )
            .await?
            .unwrap_or(0)
                != 0;

            let conditions = input
                .conditions
                .iter()
                .map(
                    |condition| crate::db::camera_recording_rules::NewRecordingConditionDraft {
                        entity_id: &condition.entity_id,
                        operator: condition.operator,
                        from_state: condition.from_state.as_deref(),
                        to_state: condition.to_state.as_deref(),
                        value: condition.value.as_deref(),
                    },
                )
                .collect::<Vec<_>>();
            crate::db::camera_recording_rules::create_rule_with_conditions_and_groups(
                crate::db::camera_recording_rules::NewRecordingRule {
                    name: &input.name,
                    camera_id: input.camera_id,
                    condition_logic: input.logic,
                    tail_seconds: i64::from(input.tail_seconds),
                    max_segment_seconds: i64::from(input.max_segment_seconds),
                    cooldown_s: i64::from(input.cooldown_s),
                    retention_days: i64::from(input.retention_days),
                    pre_roll_enabled,
                    pre_roll_seconds: crate::db::camera_recording_rules::DEFAULT_PRE_ROLL_SECONDS,
                },
                &conditions,
                &[],
                &config.db,
            )
            .await?;

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
            if editing_wizard.is_some() {
                return keep_dialogue_with_error(
                    &bot,
                    &dialogue,
                    &msg,
                    State::AddRecordingRule { room_id },
                    text,
                )
                .await;
            }
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
    let draft_owner = msg.from.as_ref().context("User context missing")?.id.0;
    restore_recording_wizard_input(&config, draft_owner).await?;

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

    crate::bot::recording_rule_wizard::persist_draft(&config, draft_owner).await?;

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

    let data = db::schedule_drafts::ScheduleDraft {
        target,
        command,
        time_minute: Some(time_minute),
        days_mask: 127,
        enabled: false,
        existing: None,
    };
    let draft = db::ui_drafts::create(
        config.root_user,
        db::schedule_drafts::KIND,
        &serde_json::to_string(&data)?,
        config.session_ttl_hours,
        &config.db,
    )
    .await?;
    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Schedule(
            super::schedule_drafts::SchedulePayload::Review { id: draft.id },
        )),
    )
    .await
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

    let time_minute = match db::action_groups::parse_time_minute(msg.text().unwrap_or("")) {
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
    let mut data =
        db::schedule_drafts::ScheduleDraft::from_existing(schedule_id, target, &config.db).await?;
    data.time_minute = Some(time_minute);
    let draft = db::ui_drafts::create(
        config.root_user,
        db::schedule_drafts::KIND,
        &serde_json::to_string(&data)?,
        config.session_ttl_hours,
        &config.db,
    )
    .await?;
    finalize_dialogue(
        bot,
        dialogue,
        msg,
        config,
        Some(Payload::Schedule(
            super::schedule_drafts::SchedulePayload::Review { id: draft.id },
        )),
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
    let draft_owner = msg.from.as_ref().context("User context missing")?.id.0;
    restore_recording_wizard_input(&config, draft_owner).await?;
    if let Some(mut session) = config.sessions.get_mut(&draft_owner) {
        if let Some(wizard) = session.recording_rule_wizard.as_mut() {
            wizard.pending_input = Some(msg.text().unwrap_or("").to_owned());
        }
    }
    crate::bot::recording_rule_wizard::persist_draft(&config, draft_owner).await?;

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

    if let Some(mut session) = config.sessions.get_mut(&draft_owner) {
        if let Some(wizard) = session.recording_rule_wizard.as_mut() {
            wizard.pending_input = None;
        }
    }
    crate::bot::recording_rule_wizard::persist_draft(&config, draft_owner).await?;

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
    let draft_owner = msg.from.as_ref().context("User context missing")?.id.0;
    restore_recording_wizard_input(&config, draft_owner).await?;
    if let Some(mut session) = config.sessions.get_mut(&draft_owner) {
        if let Some(wizard) = session.recording_rule_wizard.as_mut() {
            wizard.pending_input = Some(msg.text().unwrap_or("").to_owned());
        }
    }
    crate::bot::recording_rule_wizard::persist_draft(&config, draft_owner).await?;

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
                wizard.active_time = crate::db::camera_recording_rules::RecordingRuleActiveTime {
                    days_mask: wizard.active_time.days_mask,
                    ..active_time
                };
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

    if let Some(mut session) = config.sessions.get_mut(&draft_owner) {
        if let Some(wizard) = session.recording_rule_wizard.as_mut() {
            wizard.pending_input = None;
        }
    }
    crate::bot::recording_rule_wizard::persist_draft(&config, draft_owner).await?;

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
    let draft_owner = msg.from.as_ref().context("User context missing")?.id.0;
    restore_recording_wizard_input(&config, draft_owner).await?;
    if let Some(mut session) = config.sessions.get_mut(&draft_owner) {
        if let Some(wizard) = session.recording_rule_wizard.as_mut() {
            wizard.pending_input = Some(msg.text().unwrap_or("").to_owned());
        }
    }
    crate::bot::recording_rule_wizard::persist_draft(&config, draft_owner).await?;

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

    if let Some(mut session) = config.sessions.get_mut(&draft_owner) {
        if let Some(wizard) = session.recording_rule_wizard.as_mut() {
            wizard.pending_input = None;
        }
    }
    crate::bot::recording_rule_wizard::persist_draft(&config, draft_owner).await?;

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
            let existing_rule = crate::db::camera_recording_rules::get_rule(rule_id, &config.db)
                .await?
                .context("Recording rule not found")?;

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
                    pre_roll_enabled: existing_rule.pre_roll_enabled(),
                    pre_roll_seconds: existing_rule.normalized_pre_roll_seconds(),
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

fn apply_recording_rule_input_to_wizard(
    wizard: &mut crate::bot::recording_rule_wizard::RecordingRuleWizard,
    input: RecordingRuleInput,
    conditions: Vec<crate::bot::recording_rule_wizard::WizardCondition>,
) {
    wizard.name_touched |= wizard.name.as_deref() != Some(input.name.as_str());
    wizard.name = Some(input.name);
    wizard.camera_id = Some(input.camera_id);
    wizard.logic_touched |= wizard.condition_logic != input.logic;
    wizard.condition_logic = input.logic;
    wizard.custom_conditions = Some(conditions);
    wizard.tail_seconds = Some(input.tail_seconds);
    wizard.max_segment_seconds = Some(input.max_segment_seconds);
    wizard.cooldown_touched |= wizard.cooldown_s != Some(input.cooldown_s);
    wizard.cooldown_s = Some(input.cooldown_s);
    wizard.retention_days = Some(input.retention_days);
    wizard.raw_text = None;
    wizard.editing_text = false;
    wizard.step = Payload::Admin(AdminPayload::WizardConfirmGroups).to_string();
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
    for (index, line) in lines[3..numbers_start].iter().enumerate() {
        conditions.push(
            parse_recording_condition(line)
                .map_err(|error| format!("Строка {}: {}", index + 4, error))?,
        );
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
            || trimmed.contains("Текст правила: скопируйте блок")
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
#[path = "video_delivery_tests.rs"]
mod video_delivery_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn camera_background_keeps_its_source_until_delivery() -> Result<()> {
        let config = crate::test_support::config().await;
        db::add_user(42, &config.db).await?;
        db::rooms::sync_rooms_from_ha("test", "Test", &config.db).await?;
        sqlx::query("INSERT INTO cameras(id,key,name,room_id,stream_url) VALUES(1,'test','Test',1,'rtsp://unused.invalid')").execute(&config.db).await?;
        db::settings::set_i64(db::settings::UI_BACKGROUND_CAMERA_ID, 1, &config.db).await?;
        *config.ui_background_cache.lock().await = Some(crate::models::UiBackgroundCache {
            camera_id: 1,
            bytes: Some(vec![1, 2, 3]),
            captured_at: Some(Utc::now()),
            failed_at: None,
            last_warned_at: None,
            last_error: None,
            refreshing: false,
        });
        let mut view = View::default();
        assert_eq!(
            resolve_view_image(&mut view, 42, &config).await,
            vec![1, 2, 3]
        );
        assert_eq!(
            view.displayed_resources,
            vec![crate::core::types::DisplaySource::Camera { id: 1 }]
        );
        assert!(view_admission(&view, &config, 42, ChatId(42))
            .await?
            .is_some());
        db::access::set_room_access(42, 1, false, false, &config.db).await?;
        assert!(view_admission(&view, &config, 42, ChatId(42))
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn manual_video_shares_upload_limit_and_reports_failed_compression() -> Result<()> {
        let config = crate::test_support::config().await;
        db::add_user(42, &config.db).await?;
        db::rooms::sync_rooms_from_ha("test", "Test", &config.db).await?;
        sqlx::query("INSERT INTO cameras(id,key,name,room_id,stream_url) VALUES(1,'test','Test',1,'rtsp://unused.invalid')").execute(&config.db).await?;
        let access = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
        let message = serde_json::json!({"ok":true,"result":{"message_id":7,"date":1_700_000_000,"chat":{"id":42,"type":"private"},"text":"ok"}});
        let (bot, server) = crate::bot::router::control_tests::mock_telegram_edits(vec![
            (200, message.clone()),
            (200, message),
        ])
        .await?;
        assert!(
            send_manual_camera_video(&bot, ChatId(42), vec![1; 1024], "Manual", &access).await?
        );
        // Invalid oversized input must enter compression and produce a readable
        // error instead of sending a video that Telegram would reject.
        assert!(
            !send_manual_camera_video(
                &bot,
                ChatId(42),
                vec![0; TELEGRAM_STANDARD_UPLOAD_LIMIT_BYTES as usize + 1],
                "Manual 120s",
                &access
            )
            .await?
        );
        let requests = tokio::time::timeout(StdDuration::from_secs(5), server).await???;
        assert!(requests[0]["method"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("sendvideo"));
        assert!(requests[1]["method"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("sendmessage"));
        assert!(requests[1]["text"]
            .as_str()
            .unwrap()
            .contains("Не удалось сжать"));
        Ok(())
    }

    #[tokio::test]
    async fn delayed_camera_delivery_rejects_revoked_readded_user_and_changed_acl() -> Result<()> {
        let config = crate::test_support::config().await;
        db::add_user(42, &config.db).await?;
        db::rooms::sync_rooms_from_ha("test", "Test", &config.db).await?;
        let room: i64 = sqlx::query_scalar("SELECT id FROM rooms WHERE area='test'")
            .fetch_one(&config.db)
            .await?;
        sqlx::query("INSERT INTO cameras(id,key,name,room_id,stream_url) VALUES(1,'test','Test',?,'rtsp://unused.invalid')")
            .bind(room).execute(&config.db).await?;
        let old = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
        db::delete_user(42, &config.db).await?;
        db::add_user(42, &config.db).await?;
        assert!(old.check().await.is_err());
        let current = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
        db::access::set_room_access(42, room, false, false, &config.db).await?;
        assert!(current.check().await.is_err());
        assert!(CameraDeliveryAccess::new(ChatId(-42), 42, 1, &config)
            .await
            .is_err());
        Ok(())
    }
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
    fn recording_draft_text_roundtrip_preserves_fields_absent_from_legacy_format() {
        use crate::bot::recording_rule_wizard::{
            build_advanced_rule_text_with_logic, build_source_conditions, RecordingRuleWizard,
            WizardTriggerMode,
        };
        use crate::db::camera_recording_rules::{ConditionLogic, RecordingRuleActiveTime};
        let mut wizard = RecordingRuleWizard::new(1);
        wizard.name = Some("Door".into());
        wizard.camera_id = Some(3);
        wizard.source_device_id = Some(4);
        wizard.source_mode = Some(WizardTriggerMode::AnyChange);
        wizard.cooldown_s = Some(30);
        wizard.active_time = RecordingRuleActiveTime::window(1320, 420, 31).unwrap();
        wizard.group_ids = vec![2, 9];
        wizard.pre_roll_enabled = Some(true);
        wizard.pre_roll_seconds = 23;
        let conditions = build_source_conditions(
            "binary_sensor.door",
            WizardTriggerMode::AnyChange,
            None,
            false,
        )
        .unwrap();
        let text = build_advanced_rule_text_with_logic(
            "Door",
            3,
            ConditionLogic::Any,
            &conditions,
            120,
            240,
            30,
            90,
        );
        let input = parse_recording_rule_input(&text, 300, 300).unwrap();
        apply_recording_rule_input_to_wizard(&mut wizard, input, conditions);
        assert_eq!(
            wizard.active_time,
            RecordingRuleActiveTime::window(1320, 420, 31).unwrap()
        );
        assert_eq!(wizard.group_ids, vec![2, 9]);
        assert_eq!(wizard.pre_roll_enabled, Some(true));
        assert_eq!(wizard.pre_roll_seconds, 23);
        assert!(!wizard.name_touched);
        assert!(!wizard.cooldown_touched);
        assert_eq!(wizard.custom_conditions.as_ref().unwrap().len(), 1);
        assert_eq!(wizard.tail_seconds, Some(120));
        assert_eq!(wizard.max_segment_seconds, Some(240));
        assert!(parse_recording_rule_input(
            "Bad\n3\nany\nsensor.a;bogus;;;4\n60\n240\n10\n30",
            300,
            300
        )
        .unwrap_err()
        .contains("Строка 4"));
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
    fn compact_menus_keep_background_and_images_with_text_fallback_for_long_captions() {
        let mut view = View {
            compact: true,
            text: "Кухня".into(),
            ..Default::default()
        };
        assert_eq!(view_mode(&view.get_text()), UiMessageMode::Photo);
        view.image = Some(vec![1, 2, 3]);
        assert_eq!(view_mode(&view.get_text()), UiMessageMode::Photo);
        view.compact = false;
        assert_eq!(view_mode(&view.get_text()), UiMessageMode::Photo);
        view.text = "x".repeat(TELEGRAM_PHOTO_CAPTION_LIMIT_CHARS + 1);
        assert_eq!(view_mode(&view.get_text()), UiMessageMode::Text);
    }

    #[test]
    fn file_size_is_formatted_in_decimal_mb() {
        assert_eq!(crate::bot::format::decimal_mb(58_858_228), "58.9 MB");
    }

    #[test]
    fn tiny_cached_compressed_video_is_treated_as_stale() {
        assert!(!is_usable_telegram_compressed_video(136_508));
        assert!(is_usable_telegram_compressed_video(5_000_000));
        assert!(!is_usable_telegram_compressed_video(51_000_000));
    }
}

#[cfg(test)]
#[path = "voice_delivery_tests.rs"]
mod voice_delivery_tests;

#[cfg(test)]
#[path = "command_delivery_tests.rs"]
mod command_delivery_tests;
