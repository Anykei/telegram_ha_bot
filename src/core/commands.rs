use crate::core::action_results::{ActionResult, Outcome, TargetKind, TargetResult};
use crate::core::devices::{DeviceAction, InteractionResult};
use crate::db;
use crate::models::AppConfig;
use crate::options::VoiceCommandEngine;
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandSource {
    Text,
    Voice,
}

impl CommandSource {
    fn pending_source(self) -> db::pending_commands::PendingCommandSource {
        match self {
            Self::Text => db::pending_commands::PendingCommandSource::Text,
            Self::Voice => db::pending_commands::PendingCommandSource::Voice,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandIntent {
    DeviceAction {
        device_id: i64,
        action: SerializableDeviceAction,
    },
    AllLightsAction {
        action: SerializableDeviceAction,
    },
    MultiDeviceAction {
        device_ids: Vec<i64>,
        action: SerializableDeviceAction,
    },
    CameraSnapshot {
        camera_id: i64,
    },
    CameraClip {
        camera_id: i64,
        seconds: u32,
    },
    CameraArchive {
        camera_id: i64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SerializableDeviceAction {
    Toggle,
    TurnOn,
    TurnOff,
}

impl SerializableDeviceAction {
    fn to_device_action(self) -> DeviceAction {
        match self {
            Self::Toggle => DeviceAction::Toggle,
            Self::TurnOn => DeviceAction::TurnOn,
            Self::TurnOff => DeviceAction::TurnOff,
        }
    }
}

impl TryFrom<DeviceAction> for SerializableDeviceAction {
    type Error = anyhow::Error;

    fn try_from(value: DeviceAction) -> Result<Self> {
        match value {
            DeviceAction::Toggle => Ok(Self::Toggle),
            DeviceAction::TurnOn => Ok(Self::TurnOn),
            DeviceAction::TurnOff => Ok(Self::TurnOff),
            _ => anyhow::bail!("это действие пока не поддерживается текстовой командой"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedCommand {
    pub intent: CommandIntent,
    pub target_label: String,
}

#[derive(Debug, Clone)]
pub enum CommandExecution {
    Done { message: String },
    NeedsConfirmation { pending_id: i64, message: String },
    OpenCameraArchive { camera_id: i64 },
    SendCameraSnapshot { camera_id: i64 },
    SendCameraClip { camera_id: i64, seconds: u32 },
    NotACommand,
    NeedsChoice { choice_id: i64 },
    SavedResult { pending_id: i64 },
}

pub async fn execute_text(
    user_id: u64,
    is_admin: bool,
    source: CommandSource,
    text: &str,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    if source == CommandSource::Voice
        && !db::access::can_use_voice(user_id, is_admin, &config.db).await?
    {
        anyhow::bail!("голосовые команды выключены для пользователя");
    }

    if user_id == config.root_user {
        db::add_user(user_id, &config.db).await?;
    }
    db::command_choices::cancel_all(user_id, &config.db).await?;
    let parsed = match parse_text(user_id, is_admin, text, config).await {
        Ok(Some(parsed)) => parsed,
        Ok(None) => return Ok(CommandExecution::NotACommand),
        Err(error) => {
            let engine = if source == CommandSource::Voice {
                db::access::get_user_voice_command_engine(
                    user_id,
                    config.voice_command_engine,
                    &config.db,
                )
                .await?
            } else {
                VoiceCommandEngine::LocalParser
            };
            return handle_parse_error(error, user_id, source, text, engine.as_str(), config).await;
        }
    };
    execute_intent(user_id, is_admin, source, text, parsed, config).await
}

pub async fn execute_voice_text(
    user_id: u64,
    is_admin: bool,
    text: &str,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    if !db::access::can_use_voice(user_id, is_admin, &config.db).await? {
        anyhow::bail!("голосовые команды выключены для пользователя");
    }

    let engine =
        db::access::get_user_voice_command_engine(user_id, config.voice_command_engine, &config.db)
            .await?;

    if user_id == config.root_user {
        db::add_user(user_id, &config.db).await?;
    }
    db::command_choices::cancel_all(user_id, &config.db).await?;
    match engine {
        VoiceCommandEngine::LocalParser => {
            execute_text(user_id, is_admin, CommandSource::Voice, text, config).await
        }
        VoiceCommandEngine::HaConversationReadonly => {
            let parsed = match parse_bot_owned_text(user_id, text, config).await {
                Ok(parsed) => parsed,
                Err(error) => {
                    return handle_parse_error(
                        error,
                        user_id,
                        CommandSource::Voice,
                        text,
                        engine.as_str(),
                        config,
                    )
                    .await
                }
            };
            if let Some(parsed) = parsed {
                return execute_intent(
                    user_id,
                    is_admin,
                    CommandSource::Voice,
                    text,
                    parsed,
                    config,
                )
                .await;
            }

            let language = db::get_user_language(user_id, &config.db)
                .await?
                .unwrap_or(config.default_language);
            let client = crate::ha::intent_recognition::RecognitionClient::new(
                &config.ha_url,
                &config.ha_token,
                config.voice_stt_timeout_s,
            )?;
            let answer = crate::core::readonly::answer(
                &client,
                user_id,
                is_admin,
                text,
                language,
                &config.db,
                config.voice_stt_timeout_s,
            )
            .await?;
            db::activity_log::log(
                db::activity_log::NewActivity {
                    user_id: Some(user_id),
                    kind: "voice_command",
                    entity_type: "ha_readonly",
                    entity_id: None,
                    action: "readonly_query",
                    status: "ok",
                    message: None,
                },
                &config.db,
            )
            .await?;
            Ok(CommandExecution::Done { message: answer })
        }
        VoiceCommandEngine::HaConversationFull => {
            let parsed = match parse_bot_owned_text(user_id, text, config).await {
                Ok(parsed) => parsed,
                Err(error) => {
                    return handle_parse_error(
                        error,
                        user_id,
                        CommandSource::Voice,
                        text,
                        engine.as_str(),
                        config,
                    )
                    .await
                }
            };
            if let Some(parsed) = parsed {
                return execute_intent(
                    user_id,
                    is_admin,
                    CommandSource::Voice,
                    text,
                    parsed,
                    config,
                )
                .await;
            }

            match execute_ha_conversation(user_id, text, config).await {
                Ok(execution) => Ok(execution),
                Err(ha_error) if crate::core::action_results::definitely_not_sent(&ha_error) => {
                    execute_text(user_id, is_admin, CommandSource::Voice, text, config).await
                }
                Err(_) => Ok(CommandExecution::Done {
                    message: unknown_message(user_id, config).await?,
                }),
            }
        }
    }
}

pub async fn confirm_pending(
    user_id: u64,
    is_admin: bool,
    pending_id: i64,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    if !db::pending_commands::claim(pending_id, user_id, &config.db).await? {
        return Ok(CommandExecution::SavedResult { pending_id });
    }
    let pending = db::pending_commands::get_pending(pending_id, user_id, &config.db)
        .await?
        .context("Command missing")?;
    let result = match serde_json::from_str::<CommandIntent>(&pending.intent_json) {
        Ok(intent) => execute_device_intent(user_id, is_admin, &intent, config).await,
        Err(error) => Err(error.into()),
    };
    let result = result.unwrap_or_else(|_| {
        ActionResult::new(vec![TargetResult::new(
            TargetKind::Device,
            0,
            Outcome::Failed,
            Some("Validation failed / Проверка не пройдена"),
        )])
    });
    let language = db::get_user_language(user_id, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let message = result.display(user_id, language, config).await?;
    db::pending_commands::finish(pending_id, &result, &message, &config.db).await?;
    let correlation = pending_id.to_string();
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(user_id),
            kind: "command_result",
            entity_type: "pending_command",
            entity_id: Some(&correlation),
            action: "finish",
            status: match result.outcome {
                Outcome::Sent => "sent",
                Outcome::Partial => "partial",
                Outcome::Failed => "failed",
                Outcome::Unknown => "unknown",
            },
            message: Some(&result.summary(language)),
        },
        &config.db,
    )
    .await;

    Ok(CommandExecution::SavedResult { pending_id })
}

pub async fn pending_result_text(user: u64, id: i64, config: &Arc<AppConfig>) -> Result<String> {
    let lang = db::get_user_language(user, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let Some(pending) = db::pending_commands::get_pending(id, user, &config.db).await? else {
        return Ok(if lang == crate::i18n::Language::En {
            "Command expired or unavailable."
        } else {
            "Команда истекла или недоступна."
        }
        .into());
    };
    if let Some(json) = pending.result_json {
        return serde_json::from_str::<ActionResult>(&json)?
            .display(user, lang, config)
            .await;
    }
    if pending.status == "confirmed" {
        return unknown_message(user, config).await;
    }
    if pending.status == "pending" && pending.expires_at > Utc::now() {
        let intent: CommandIntent = serde_json::from_str(&pending.intent_json)?;
        return confirmation_description(
            user,
            user == config.root_user,
            &ParsedCommand {
                intent,
                target_label: String::new(),
            },
            config,
        )
        .await;
    }
    Ok(pending.result_text.unwrap_or_else(|| {
        match pending.status.as_str() {
            "cancelled" => {
                if lang == crate::i18n::Language::En {
                    "Command cancelled."
                } else {
                    "Команда отменена."
                }
            }
            "pending" if pending.expires_at > Utc::now() => {
                if lang == crate::i18n::Language::En {
                    "Awaiting confirmation."
                } else {
                    "Ожидается подтверждение."
                }
            }
            _ => {
                if lang == crate::i18n::Language::En {
                    "Command expired or unavailable."
                } else {
                    "Команда истекла или недоступна."
                }
            }
        }
        .to_string()
    }))
}

async fn unknown_message(user: u64, config: &Arc<AppConfig>) -> Result<String> {
    let lang = db::get_user_language(user, &config.db)
        .await?
        .unwrap_or(config.default_language);
    Ok(if lang == crate::i18n::Language::En { "The send outcome could not be determined. Check states first; the command will not be retried automatically." } else { "Результат отправки не удалось определить. Сначала проверьте состояния; автоматического повтора не будет." }.into())
}

pub async fn cancel_pending(user_id: u64, pending_id: i64, config: &Arc<AppConfig>) -> Result<()> {
    let pending = db::pending_commands::get_pending(pending_id, user_id, &config.db)
        .await?
        .context("команда не найдена или уже обработана")?;
    db::pending_commands::mark_cancelled(pending.id, &config.db).await
}

async fn parse_text(
    user_id: u64,
    is_admin: bool,
    text: &str,
    config: &Arc<AppConfig>,
) -> Result<Option<ParsedCommand>> {
    let normalized = crate::bot::text_commands::normalize_command_text(text);

    if let Some(query) = normalized
        .strip_prefix("снимок ")
        .or_else(|| normalized.strip_prefix("snapshot "))
    {
        let camera = resolve_camera(user_id, query, ChoiceAction::Snapshot, config).await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraSnapshot {
                camera_id: camera.id,
            },
        }));
    }

    if let Some(rest) = normalized
        .strip_prefix("видео ")
        .or_else(|| normalized.strip_prefix("video "))
    {
        let (query, seconds) = crate::bot::text_commands::split_camera_video_command(rest);
        let camera = resolve_camera(
            user_id,
            query,
            ChoiceAction::Clip {
                seconds: seconds.unwrap_or(config.camera_default_clip_s),
            },
            config,
        )
        .await?;
        let seconds = seconds.unwrap_or(config.camera_default_clip_s);
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraClip {
                camera_id: camera.id,
                seconds,
            },
        }));
    }

    if let Some(query) = normalized
        .strip_prefix("архив ")
        .or_else(|| normalized.strip_prefix("archive "))
    {
        let camera = resolve_camera(user_id, query, ChoiceAction::Archive, config).await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraArchive {
                camera_id: camera.id,
            },
        }));
    }

    if let Some(action) = parse_all_lights_action(&normalized) {
        return Ok(Some(ParsedCommand {
            target_label: "весь свет".to_string(),
            intent: CommandIntent::AllLightsAction { action },
        }));
    }

    if let Some((query, action)) = crate::bot::text_commands::parse_device_text_command(&normalized)
    {
        if let Some(parsed) =
            parse_multi_device_action(user_id, is_admin, query, &action, config).await?
        {
            return Ok(Some(parsed));
        }

        let device = resolve_device(
            user_id,
            is_admin,
            query,
            SerializableDeviceAction::try_from(action.clone())?,
            config,
        )
        .await?;
        let action = SerializableDeviceAction::try_from(action)?;
        let label = device
            .alias
            .clone()
            .unwrap_or_else(|| device.entity_id.clone());
        return Ok(Some(ParsedCommand {
            target_label: label,
            intent: CommandIntent::DeviceAction {
                device_id: device.id,
                action,
            },
        }));
    }

    Ok(None)
}

async fn parse_bot_owned_text(
    user_id: u64,
    text: &str,
    config: &Arc<AppConfig>,
) -> Result<Option<ParsedCommand>> {
    let normalized = crate::bot::text_commands::normalize_command_text(text);

    if let Some(query) = normalized
        .strip_prefix("снимок ")
        .or_else(|| normalized.strip_prefix("snapshot "))
    {
        let camera = resolve_camera(user_id, query, ChoiceAction::Snapshot, config).await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraSnapshot {
                camera_id: camera.id,
            },
        }));
    }

    if let Some(rest) = normalized
        .strip_prefix("видео ")
        .or_else(|| normalized.strip_prefix("video "))
    {
        let (query, seconds) = crate::bot::text_commands::split_camera_video_command(rest);
        let camera = resolve_camera(
            user_id,
            query,
            ChoiceAction::Clip {
                seconds: seconds.unwrap_or(config.camera_default_clip_s),
            },
            config,
        )
        .await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraClip {
                camera_id: camera.id,
                seconds: seconds.unwrap_or(config.camera_default_clip_s),
            },
        }));
    }

    if let Some(query) = normalized
        .strip_prefix("архив ")
        .or_else(|| normalized.strip_prefix("archive "))
    {
        let camera = resolve_camera(user_id, query, ChoiceAction::Archive, config).await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraArchive {
                camera_id: camera.id,
            },
        }));
    }

    Ok(None)
}

async fn execute_intent(
    user_id: u64,
    is_admin: bool,
    source: CommandSource,
    command_text: &str,
    parsed: ParsedCommand,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    let was_mass = matches!(parsed.intent, CommandIntent::AllLightsAction { .. });
    let mut parsed = parsed;
    if let CommandIntent::AllLightsAction { action } = parsed.intent {
        anyhow::ensure!(
            is_admin,
            "Bulk lights require root / Массовое управление доступно root"
        );
        let ids = db::devices::list_active_lights(&config.db)
            .await?
            .into_iter()
            .map(|d| d.id)
            .collect::<Vec<_>>();
        anyhow::ensure!(
            !ids.is_empty(),
            "No active lights / Нет активных светильников"
        );
        parsed.intent = CommandIntent::MultiDeviceAction {
            device_ids: ids,
            action,
        };
    }
    if (was_mass || requires_confirmation(&parsed.intent, config).await?)
        && config.voice_confirm_dangerous
    {
        let reason = "критичное действие".to_string();
        let expires_at = Utc::now() + Duration::seconds(config.voice_pending_ttl_s as i64);
        let intent_json = serde_json::to_string(&parsed.intent)?;
        let pending_id = db::pending_commands::create(
            user_id,
            source.pending_source(),
            command_text,
            &intent_json,
            Some(&reason),
            expires_at,
            &config.db,
        )
        .await?;

        return Ok(CommandExecution::NeedsConfirmation {
            pending_id,
            message: confirmation_description(user_id, is_admin, &parsed, config).await?,
        });
    }

    execute_intent_without_confirmation(user_id, is_admin, parsed, config).await
}

async fn execute_intent_without_confirmation(
    user_id: u64,
    is_admin: bool,
    parsed: ParsedCommand,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    match parsed.intent {
        intent @ (CommandIntent::AllLightsAction { .. }
        | CommandIntent::MultiDeviceAction { .. }
        | CommandIntent::DeviceAction { .. }) => {
            let result = execute_device_intent(user_id, is_admin, &intent, config).await?;
            let lang = db::get_user_language(user_id, &config.db)
                .await?
                .unwrap_or(config.default_language);
            Ok(CommandExecution::Done {
                message: result.display(user_id, lang, config).await?,
            })
        }
        CommandIntent::CameraSnapshot { camera_id } => {
            ensure_camera_access(user_id, is_admin, camera_id, config).await?;
            Ok(CommandExecution::SendCameraSnapshot { camera_id })
        }
        CommandIntent::CameraClip { camera_id, seconds } => {
            ensure_camera_access(user_id, is_admin, camera_id, config).await?;
            Ok(CommandExecution::SendCameraClip { camera_id, seconds })
        }
        CommandIntent::CameraArchive { camera_id } => {
            ensure_camera_access(user_id, is_admin, camera_id, config).await?;
            Ok(CommandExecution::OpenCameraArchive { camera_id })
        }
    }
}

async fn requires_confirmation(intent: &CommandIntent, config: &Arc<AppConfig>) -> Result<bool> {
    match intent {
        CommandIntent::AllLightsAction { .. } => Ok(true),
        CommandIntent::MultiDeviceAction { device_ids, .. } => {
            if device_ids.len() > 1 {
                return Ok(true);
            }
            for id in device_ids {
                if let Some(device) = db::devices::get_device_by_id(*id, &config.db).await? {
                    if db::devices::is_device_critical(&device.entity_id, &config.db).await? {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        CommandIntent::DeviceAction { device_id, .. } => {
            let device = db::devices::get_device_by_id(*device_id, &config.db)
                .await?
                .context("устройство не найдено")?;
            db::devices::is_device_critical(&device.entity_id, &config.db).await
        }
        _ => Ok(false),
    }
}

async fn ensure_camera_access(
    user_id: u64,
    is_admin: bool,
    camera_id: i64,
    config: &Arc<AppConfig>,
) -> Result<()> {
    if db::cameras::get_accessible_camera(user_id, is_admin, camera_id, &config.db)
        .await?
        .is_some()
    {
        Ok(())
    } else {
        anyhow::bail!("недостаточно прав для просмотра камеры")
    }
}

fn parse_all_lights_action(text: &str) -> Option<SerializableDeviceAction> {
    let text = text
        .trim()
        .trim_end_matches(['.', '!', '?', ',', '…'])
        .trim();

    let (rest, action) = [
        ("включи ", SerializableDeviceAction::TurnOn),
        ("включить ", SerializableDeviceAction::TurnOn),
        ("вкл ", SerializableDeviceAction::TurnOn),
        ("выключи ", SerializableDeviceAction::TurnOff),
        ("выключить ", SerializableDeviceAction::TurnOff),
        ("выкл ", SerializableDeviceAction::TurnOff),
        ("turn on ", SerializableDeviceAction::TurnOn),
        ("turn off ", SerializableDeviceAction::TurnOff),
    ]
    .into_iter()
    .find_map(|(prefix, action)| text.strip_prefix(prefix).map(|rest| (rest.trim(), action)))?;

    let rest = rest
        .replace("ё", "е")
        .replace("во всех", "всех")
        .replace("во всем", "всем");
    let tokens = rest
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    let mentions_light = tokens
        .iter()
        .any(|token| matches!(*token, "свет" | "света" | "lights" | "light"));
    let mentions_all = tokens.iter().any(|token| {
        matches!(
            *token,
            "весь" | "все" | "всех" | "везде" | "all" | "everywhere"
        )
    });
    let mentions_rooms = tokens
        .iter()
        .any(|token| matches!(*token, "комнатах" | "комнаты" | "комнат" | "rooms"));

    if mentions_light && (mentions_all || mentions_rooms) {
        Some(action)
    } else {
        None
    }
}

#[derive(Debug)]
struct DeviceCommandCandidate {
    device: crate::core::types::Device,
    room_area: Option<String>,
    room_alias: Option<String>,
}

async fn parse_multi_device_action(
    user_id: u64,
    is_admin: bool,
    query: &str,
    action: &DeviceAction,
    config: &Arc<AppConfig>,
) -> Result<Option<ParsedCommand>> {
    let Some(parts) = parse_multi_device_query(query) else {
        return Ok(None);
    };

    let action = SerializableDeviceAction::try_from(action.clone())?;
    let candidates = load_command_device_candidates(user_id, is_admin, config).await?;
    if candidates.is_empty() {
        anyhow::bail!("доступные устройства не найдены");
    }

    let room = find_room_in_query(parts.normalized_query, &candidates);
    let room_tokens = room
        .as_ref()
        .map(|room| command_tokens(room))
        .unwrap_or_default();

    let mut device_ids = Vec::new();
    let mut labels = Vec::new();
    let mut unresolved = Vec::new();
    for raw_name in parts.names {
        let name = clean_multi_device_name(raw_name, &room_tokens);
        if name.is_empty() {
            continue;
        }

        let mut matches = candidates
            .iter()
            .filter(|candidate| {
                room.as_deref()
                    .is_none_or(|room| candidate_matches_room(candidate, room))
            })
            .filter(|candidate| candidate_matches_device_name(candidate, &name))
            .collect::<Vec<_>>();
        matches.dedup_by_key(|candidate| candidate.device.id);

        match matches.len() {
            1 => {
                let candidate = matches[0];
                if !device_ids.contains(&candidate.device.id) {
                    device_ids.push(candidate.device.id);
                    labels.push(
                        candidate
                            .device
                            .alias
                            .clone()
                            .unwrap_or_else(|| candidate.device.entity_id.clone()),
                    );
                }
            }
            0 => {
                let location = room
                    .as_deref()
                    .map(|room| format!(" в комнате `{room}`"))
                    .unwrap_or_default();
                anyhow::bail!("устройство `{}`{} не найдено", name, location);
            }
            _ => {
                unresolved.push(
                    matches
                        .iter()
                        .map(|candidate| candidate.device.id)
                        .collect::<Vec<_>>(),
                );
            }
        }
    }

    if !unresolved.is_empty() {
        let candidates = unresolved.remove(0);
        return Err(ChoiceRequest {
            action: ChoiceAction::Device {
                action,
                resolved: device_ids,
                remaining: unresolved,
            },
            candidates,
        }
        .into());
    }
    if device_ids.len() < 2 {
        return Ok(None);
    }

    let target_label = match room {
        Some(room) => format!("{} · {}", labels.join(", "), room),
        None => labels.join(", "),
    };

    Ok(Some(ParsedCommand {
        target_label,
        intent: CommandIntent::MultiDeviceAction { device_ids, action },
    }))
}

struct MultiDeviceQuery<'a> {
    normalized_query: &'a str,
    names: Vec<&'a str>,
}

fn parse_multi_device_query(query: &str) -> Option<MultiDeviceQuery<'_>> {
    let normalized_query = query
        .trim()
        .trim_end_matches(['.', '!', '?', ',', '…'])
        .trim();

    if !normalized_query.contains(" и ") && !normalized_query.contains(',') {
        return None;
    }

    let names = normalized_query
        .split([','])
        .flat_map(|part| part.split(" и "))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if names.len() < 2 {
        return None;
    }

    Some(MultiDeviceQuery {
        normalized_query,
        names,
    })
}

async fn load_command_device_candidates(
    user_id: u64,
    is_admin: bool,
    config: &Arc<AppConfig>,
) -> Result<Vec<DeviceCommandCandidate>> {
    let rows = sqlx::query(
        r#"
        SELECT d.id, d.entity_id, d.alias, r.area AS room_area, r.alias AS room_alias
        FROM devices d
        LEFT JOIN rooms r ON r.id = d.room_id
        WHERE d.archived = 0
          AND (
              d.entity_id LIKE 'light.%'
              OR d.entity_id LIKE 'switch.%'
          )
        ORDER BY d.alias, d.entity_id
        "#,
    )
    .fetch_all(&config.db)
    .await?;

    let mut candidates = Vec::new();
    for row in rows {
        let device = crate::core::types::Device {
            id: row.get("id"),
            entity_id: row.get("entity_id"),
            alias: row.get("alias"),
        };
        if !db::access::can_view_device(user_id, is_admin, device.id, &config.db).await?
            || !db::access::can_control_device(user_id, is_admin, device.id, &config.db).await?
        {
            continue;
        }

        candidates.push(DeviceCommandCandidate {
            device,
            room_area: row.get("room_area"),
            room_alias: row.get("room_alias"),
        });
    }

    Ok(candidates)
}

fn find_room_in_query(query: &str, candidates: &[DeviceCommandCandidate]) -> Option<String> {
    let mut rooms = Vec::new();
    for candidate in candidates {
        if let Some(room) = candidate
            .room_alias
            .as_deref()
            .or(candidate.room_area.as_deref())
        {
            let room = crate::bot::text_commands::normalize_command_text(room);
            if !room.is_empty() && !rooms.contains(&room) {
                rooms.push(room);
            }
        }
    }

    rooms
        .into_iter()
        .filter(|room| phrase_tokens_match(room, query))
        .max_by_key(|room| room.chars().count())
}

fn candidate_matches_room(candidate: &DeviceCommandCandidate, room: &str) -> bool {
    let haystack = crate::bot::text_commands::normalize_command_text(&format!(
        "{} {}",
        candidate.room_area.as_deref().unwrap_or_default(),
        candidate.room_alias.as_deref().unwrap_or_default()
    ));
    phrase_tokens_match(room, &haystack)
}

fn candidate_matches_device_name(candidate: &DeviceCommandCandidate, name: &str) -> bool {
    let haystack = crate::bot::text_commands::normalize_command_text(&format!(
        "{} {}",
        candidate
            .device
            .alias
            .as_deref()
            .unwrap_or(&candidate.device.entity_id),
        candidate.device.entity_id
    ));
    crate::bot::text_commands::device_text_matches(name, &haystack)
}

fn clean_multi_device_name(name: &str, room_tokens: &[String]) -> String {
    command_tokens(name)
        .into_iter()
        .filter(|token| !matches!(token.as_str(), "в" | "во" | "на"))
        .filter(|token| {
            !room_tokens
                .iter()
                .any(|room_token| command_tokens_match(token, room_token))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn phrase_tokens_match(needle: &str, haystack: &str) -> bool {
    let haystack_tokens = command_tokens(haystack);
    let needle_tokens = command_tokens(needle);
    !needle_tokens.is_empty()
        && needle_tokens.iter().all(|needle| {
            haystack_tokens
                .iter()
                .any(|haystack| command_tokens_match(needle, haystack))
        })
}

fn command_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(crate::bot::text_commands::normalize_command_text)
        .collect()
}

fn command_tokens_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }

    command_token_stem(left) == command_token_stem(right)
        || strsim::levenshtein(left, right) <= 1
        || strsim::jaro_winkler(left, right) >= 0.9
}

fn command_token_stem(token: &str) -> String {
    for suffix in [
        "ыми", "ими", "ого", "ему", "ыми", "ами", "ями", "ой", "ом", "ей", "ым", "им", "ах", "ях",
        "е", "и", "ы", "у", "ю", "я", "а",
    ] {
        if let Some(stem) = token.strip_suffix(suffix) {
            if stem.chars().count() >= 3 {
                return stem.to_string();
            }
        }
    }
    token.to_string()
}

async fn execute_ha_conversation(
    user_id: u64,
    text: &str,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    let language = db::get_user_language(user_id, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let answer = crate::ha::conversation::process(
        &config.ha_url,
        &config.ha_token,
        text,
        language.code(),
        config.voice_stt_timeout_s,
    )
    .await?;

    db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(user_id),
            kind: "voice_command",
            entity_type: "ha_conversation",
            entity_id: None,
            action: "conversation",
            status: "ok",
            message: None,
        },
        &config.db,
    )
    .await?;

    Ok(CommandExecution::Done {
        message: format!("HA Assist: {}", answer),
    })
}

#[cfg(test)]
mod voice_engine_tests {
    use super::*;

    #[test]
    fn parses_all_lights_commands_with_stt_punctuation() {
        assert_eq!(
            parse_all_lights_action("включи свет во всех комнатах."),
            Some(SerializableDeviceAction::TurnOn)
        );
        assert_eq!(
            parse_all_lights_action("выключи весь свет"),
            Some(SerializableDeviceAction::TurnOff)
        );
        assert_eq!(
            parse_all_lights_action("turn on all lights"),
            Some(SerializableDeviceAction::TurnOn)
        );
        assert_eq!(parse_all_lights_action("включи свет в коридоре"), None);
    }

    #[test]
    fn parses_multi_device_query_in_both_word_orders() {
        let first = parse_multi_device_query("на кухне люстру и подсветку.").unwrap();
        assert_eq!(first.names, vec!["на кухне люстру", "подсветку"]);

        let second = parse_multi_device_query("люстру и подсветку на кухне").unwrap();
        assert_eq!(second.names, vec!["люстру", "подсветку на кухне"]);

        assert!(parse_multi_device_query("люстру на кухне").is_none());
    }

    #[test]
    fn cleans_room_words_from_multi_device_parts() {
        let room_tokens = command_tokens("кухня");

        assert_eq!(
            clean_multi_device_name("на кухне люстру", &room_tokens),
            "люстру"
        );
        assert_eq!(
            clean_multi_device_name("подсветку на кухне", &room_tokens),
            "подсветку"
        );
    }

    #[test]
    fn matches_inflected_room_tokens() {
        assert!(phrase_tokens_match("кухня", "включи на кухне люстру"));
        assert!(phrase_tokens_match("коридор", "свет в коридоре"));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChoiceAction {
    Device {
        action: SerializableDeviceAction,
        resolved: Vec<i64>,
        remaining: Vec<Vec<i64>>,
    },
    Snapshot,
    Clip {
        seconds: u32,
    },
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceContext {
    pub action: ChoiceAction,
    pub candidates: Vec<i64>,
    pub text: String,
    pub source: CommandSource,
    pub engine: String,
    pub back: crate::bot::router::Payload,
}

#[derive(Debug)]
struct ChoiceRequest {
    action: ChoiceAction,
    candidates: Vec<i64>,
}
impl std::fmt::Display for ChoiceRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Choose a target / Уточните цель")
    }
}
impl std::error::Error for ChoiceRequest {}

async fn handle_parse_error(
    error: anyhow::Error,
    user: u64,
    source: CommandSource,
    text: &str,
    engine: &str,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    let request = match error.downcast::<ChoiceRequest>() {
        Ok(request) => request,
        Err(error) => return Err(error),
    };
    let back = config
        .sessions
        .get(&user)
        .and_then(|session| crate::bot::router::Payload::from_string(&session.current_context).ok())
        .map(|p| p.for_refresh())
        .unwrap_or(crate::bot::router::Payload::Home);
    let context = ChoiceContext {
        action: request.action,
        candidates: request.candidates,
        text: text.to_string(),
        source,
        engine: engine.to_string(),
        back,
    };
    let choice_id =
        db::command_choices::create(user, &serde_json::to_string(&context)?, &config.db).await?;
    Ok(CommandExecution::NeedsChoice { choice_id })
}

async fn resolve_camera(
    user: u64,
    query: &str,
    action: ChoiceAction,
    config: &Arc<AppConfig>,
) -> Result<db::cameras::Camera> {
    let mut cameras = crate::bot::text_commands::camera_candidates(user, query, config).await?;
    match cameras.len() {
        0 => anyhow::bail!("Camera not found / Камера не найдена"),
        1 => Ok(cameras.remove(0)),
        _ => Err(ChoiceRequest {
            action,
            candidates: cameras.iter().map(|camera| camera.id).collect(),
        }
        .into()),
    }
}
async fn resolve_device(
    user: u64,
    admin: bool,
    query: &str,
    action: SerializableDeviceAction,
    config: &Arc<AppConfig>,
) -> Result<crate::core::types::Device> {
    let mut devices =
        crate::bot::text_commands::device_candidates(user, admin, query, config).await?;
    match devices.len() {
        0 => anyhow::bail!("Device not found / Устройство не найдено"),
        1 => Ok(devices.remove(0)),
        _ => Err(ChoiceRequest {
            action: ChoiceAction::Device {
                action,
                resolved: vec![],
                remaining: vec![],
            },
            candidates: devices.iter().map(|device| device.id).collect(),
        }
        .into()),
    }
}

pub async fn choose_target(
    user: u64,
    id: i64,
    target: i64,
    config: &Arc<AppConfig>,
) -> Result<CommandExecution> {
    let record = db::command_choices::get(id, user, &config.db)
        .await?
        .context("Choice unavailable / Выбор недоступен")?;
    let mut context: ChoiceContext = serde_json::from_str(&record.context_json)?;
    anyhow::ensure!(
        context.candidates.contains(&target),
        "Target unavailable / Цель недоступна"
    );
    anyhow::ensure!(db::command_choices::claim(id, user, &config.db).await?, "Choice expired or completed. Repeat the command / Выбор истёк или завершён. Повторите команду");
    let admin = user == config.root_user;
    if context.source == CommandSource::Voice {
        anyhow::ensure!(
            db::access::can_use_voice(user, admin, &config.db).await?,
            "Voice access revoked / Голосовой доступ отозван"
        );
    }
    let intent = match &mut context.action {
        ChoiceAction::Device {
            action,
            resolved,
            remaining,
        } => {
            anyhow::ensure!(
                db::access::can_view_device(user, admin, target, &config.db).await?
                    && db::access::can_control_device(user, admin, target, &config.db).await?,
                "Access revoked / Доступ отозван"
            );
            if !resolved.contains(&target) {
                resolved.push(target);
            }
            if !remaining.is_empty() {
                context.candidates = remaining.remove(0);
                // A second ambiguous part shares the initial deadline; choosing does not extend it.
                let next = db::command_choices::create_until(
                    user,
                    &serde_json::to_string(&context)?,
                    record.expires_at,
                    &config.db,
                )
                .await?;
                return Ok(CommandExecution::NeedsChoice { choice_id: next });
            }
            if resolved.len() == 1 {
                CommandIntent::DeviceAction {
                    device_id: target,
                    action: *action,
                }
            } else {
                CommandIntent::MultiDeviceAction {
                    device_ids: resolved.clone(),
                    action: *action,
                }
            }
        }
        ChoiceAction::Snapshot => CommandIntent::CameraSnapshot { camera_id: target },
        ChoiceAction::Clip { seconds } => CommandIntent::CameraClip {
            camera_id: target,
            seconds: *seconds,
        },
        ChoiceAction::Archive => CommandIntent::CameraArchive { camera_id: target },
    };
    let label = match &intent {
        CommandIntent::DeviceAction { device_id, .. } => {
            db::devices::get_device_by_id(*device_id, &config.db)
                .await?
                .context("Device removed")?
                .alias
                .unwrap_or_else(|| format!("#{device_id}"))
        }
        CommandIntent::MultiDeviceAction { device_ids, .. } => {
            format!("{} devices / устройств", device_ids.len())
        }
        _ => {
            db::cameras::get_accessible_camera(user, admin, target, &config.db)
                .await?
                .context("Camera access revoked")?
                .name
        }
    };
    let correlation = id.to_string();
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(user),
            kind: "command_choice",
            entity_type: "command_choice",
            entity_id: Some(&correlation),
            action: "select",
            status: "ok",
            message: None,
        },
        &config.db,
    )
    .await;
    execute_intent(
        user,
        admin,
        context.source,
        &context.text,
        ParsedCommand {
            intent,
            target_label: label,
        },
        config,
    )
    .await
}

async fn execute_device_intent(
    user: u64,
    admin: bool,
    intent: &CommandIntent,
    config: &Arc<AppConfig>,
) -> Result<ActionResult> {
    let (ids, action) = match intent {
        CommandIntent::DeviceAction { device_id, action } => (vec![*device_id], *action),
        CommandIntent::MultiDeviceAction { device_ids, action } => (device_ids.clone(), *action),
        CommandIntent::AllLightsAction { action } => {
            anyhow::ensure!(
                admin,
                "Bulk lights require root / Массовое управление доступно root"
            );
            (
                db::devices::list_active_lights(&config.db)
                    .await?
                    .into_iter()
                    .map(|d| d.id)
                    .collect(),
                *action,
            )
        }
        _ => anyhow::bail!("Unsupported confirmed command"),
    };
    anyhow::ensure!(!ids.is_empty(), "No targets / Нет устройств");
    let mut targets = Vec::new();
    for id in ids {
        let result = async {
            if !db::access::can_control_device(user, admin, id, &config.db).await? {
                return Ok(TargetResult::new(
                    TargetKind::Device,
                    id,
                    Outcome::Failed,
                    Some("Access revoked / Доступ отозван"),
                ));
            }
            if db::devices::get_device_by_id(id, &config.db)
                .await?
                .is_none()
            {
                return Ok(TargetResult::new(
                    TargetKind::Device,
                    id,
                    Outcome::Failed,
                    Some("Device removed / Устройство удалено"),
                ));
            }
            match crate::core::devices::handle_device_interaction(
                config,
                id,
                action.to_device_action(),
            )
            .await
            {
                Ok(InteractionResult::Processed) => Ok(TargetResult::new(
                    TargetKind::Device,
                    id,
                    Outcome::Sent,
                    None,
                )),
                Ok(InteractionResult::Error { .. }) => Ok(TargetResult::new(
                    TargetKind::Device,
                    id,
                    Outcome::Unknown,
                    Some("HA response unavailable / Нет подтверждения HA"),
                )),
                Ok(_) => Ok(TargetResult::new(
                    TargetKind::Device,
                    id,
                    Outcome::Failed,
                    Some("Unsupported action / Действие недоступно"),
                )),
                // handle_device_interaction only returns Err before dispatch (lookup/state read).
                Err(_) => Ok(TargetResult::new(
                    TargetKind::Device,
                    id,
                    Outcome::Failed,
                    Some("Device state unavailable / Состояние недоступно"),
                )),
            }
        }
        .await;
        targets.push(result.unwrap_or_else(|_: anyhow::Error| {
            TargetResult::new(
                TargetKind::Device,
                id,
                Outcome::Failed,
                Some("Validation failed / Проверка не пройдена"),
            )
        }));
    }
    let result = ActionResult::new(targets);
    let summary = result.summary(config.default_language);
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(user),
            kind: "device",
            entity_type: "command",
            entity_id: None,
            action: "command",
            status: match result.outcome {
                Outcome::Sent => "ok",
                Outcome::Partial => "partial",
                _ => "error",
            },
            message: Some(&summary),
        },
        &config.db,
    )
    .await;
    Ok(result)
}

async fn confirmation_description(
    user: u64,
    admin: bool,
    parsed: &ParsedCommand,
    config: &Arc<AppConfig>,
) -> Result<String> {
    let lang = db::get_user_language(user, &config.db)
        .await?
        .unwrap_or(config.default_language);
    let en = lang == crate::i18n::Language::En;
    let (ids, action) = match &parsed.intent {
        CommandIntent::DeviceAction { device_id, action } => (vec![*device_id], *action),
        CommandIntent::MultiDeviceAction { device_ids, action } => (device_ids.clone(), *action),
        _ => return Ok(parsed.target_label.clone()),
    };
    let verb = match (action, en) {
        (SerializableDeviceAction::TurnOn, true) => "Turn on",
        (SerializableDeviceAction::TurnOn, false) => "Включить",
        (SerializableDeviceAction::TurnOff, true) => "Turn off",
        (SerializableDeviceAction::TurnOff, false) => "Выключить",
        (SerializableDeviceAction::Toggle, true) => "Toggle",
        (SerializableDeviceAction::Toggle, false) => "Переключить",
    };
    let mut text = if en {
        format!("{verb} · {} devices. Confirm sending?", ids.len())
    } else {
        format!("{verb} · {} устройств. Подтвердить отправку?", ids.len())
    };
    for id in ids {
        if !db::access::can_view_device(user, admin, id, &config.db).await? {
            continue;
        }
        if let Some(device) = db::devices::get_device_by_id(id, &config.db).await? {
            if db::devices::is_device_critical(&device.entity_id, &config.db).await? {
                text.push_str(&format!(
                    "\n⚠ {}",
                    device.alias.as_deref().unwrap_or(&device.entity_id)
                ));
            }
        }
    }
    Ok(text)
}
