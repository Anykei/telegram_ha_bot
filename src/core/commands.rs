use crate::core::devices::{DeviceAction, InteractionResult};
use crate::db;
use crate::models::AppConfig;
use crate::options::VoiceCommandEngine;
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    let Some(parsed) = parse_text(user_id, is_admin, text, config).await? else {
        return Ok(CommandExecution::NotACommand);
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

    match engine {
        VoiceCommandEngine::LocalParser => {
            execute_text(user_id, is_admin, CommandSource::Voice, text, config).await
        }
        VoiceCommandEngine::HaConversationReadonly => {
            if let Some(parsed) = parse_bot_owned_text(user_id, text, config).await? {
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

            if !looks_like_readonly_question(text) {
                anyhow::bail!(
                    "HA Conversation readonly разрешает только вопросы. Для управления используйте локальный voice engine или выдайте HA full."
                );
            }
            execute_ha_conversation(user_id, text, config).await
        }
        VoiceCommandEngine::HaConversationFull => {
            if let Some(parsed) = parse_bot_owned_text(user_id, text, config).await? {
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
                Err(ha_error) => {
                    if let Some(parsed) = parse_text(user_id, is_admin, text, config).await? {
                        return execute_intent(
                            user_id,
                            is_admin,
                            CommandSource::Voice,
                            text,
                            parsed,
                            config,
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "HA Assist не выполнил команду: {}. Локальный fallback тоже не сработал",
                                ha_error
                            )
                        });
                    }

                    Err(ha_error)
                }
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
    let pending = db::pending_commands::get_pending(pending_id, user_id, &config.db)
        .await?
        .context("команда не найдена или уже обработана")?;

    if pending.expires_at <= Utc::now() {
        db::pending_commands::expire_old(&config.db).await?;
        anyhow::bail!("время подтверждения истекло");
    }

    let intent: CommandIntent = serde_json::from_str(&pending.intent_json)?;
    let label = intent_label(&intent, config).await?;
    let parsed = ParsedCommand {
        intent,
        target_label: label,
    };

    let result = execute_intent_without_confirmation(user_id, is_admin, parsed, config).await?;
    db::pending_commands::mark_executed(pending.id, &config.db).await?;
    Ok(result)
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

    if let Some(query) = normalized.strip_prefix("снимок ") {
        let camera =
            crate::bot::text_commands::find_camera_for_text(user_id, query, config).await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraSnapshot {
                camera_id: camera.id,
            },
        }));
    }

    if let Some(rest) = normalized.strip_prefix("видео ") {
        let (query, seconds) = crate::bot::text_commands::split_camera_video_command(rest);
        let camera =
            crate::bot::text_commands::find_camera_for_text(user_id, query, config).await?;
        let seconds = seconds.unwrap_or(config.camera_default_clip_s);
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraClip {
                camera_id: camera.id,
                seconds,
            },
        }));
    }

    if let Some(query) = normalized.strip_prefix("архив ") {
        let camera =
            crate::bot::text_commands::find_camera_for_text(user_id, query, config).await?;
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

        let device =
            crate::bot::text_commands::find_device_for_text(user_id, is_admin, query, config)
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

    if let Some(query) = normalized.strip_prefix("снимок ") {
        let camera =
            crate::bot::text_commands::find_camera_for_text(user_id, query, config).await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraSnapshot {
                camera_id: camera.id,
            },
        }));
    }

    if let Some(rest) = normalized.strip_prefix("видео ") {
        let (query, seconds) = crate::bot::text_commands::split_camera_video_command(rest);
        let camera =
            crate::bot::text_commands::find_camera_for_text(user_id, query, config).await?;
        return Ok(Some(ParsedCommand {
            target_label: camera.name,
            intent: CommandIntent::CameraClip {
                camera_id: camera.id,
                seconds: seconds.unwrap_or(config.camera_default_clip_s),
            },
        }));
    }

    if let Some(query) = normalized.strip_prefix("архив ") {
        let camera =
            crate::bot::text_commands::find_camera_for_text(user_id, query, config).await?;
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
    if requires_confirmation(&parsed.intent, config).await? && config.voice_confirm_dangerous {
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
            message: format!(
                "Распознано: `{}`\n\nЭто критичное действие для `{}`.\nПодтвердить выполнение?",
                command_text, parsed.target_label
            ),
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
        CommandIntent::AllLightsAction { action } => {
            if !is_admin {
                anyhow::bail!("массовое управление светом доступно только root пользователю");
            }

            let service = match action {
                SerializableDeviceAction::TurnOn => "turn_on",
                SerializableDeviceAction::TurnOff => "turn_off",
                SerializableDeviceAction::Toggle => "toggle",
            };
            let lights = db::devices::list_active_lights(&config.db).await?;
            if lights.is_empty() {
                anyhow::bail!("в базе нет активных светильников light.*");
            }

            for light in &lights {
                config
                    .ha_client
                    .call_service("light", service, &light.entity_id)
                    .await
                    .with_context(|| format!("не удалось отправить команду {}", light.entity_id))?;
            }

            db::activity_log::log(
                db::activity_log::NewActivity {
                    user_id: Some(user_id),
                    kind: "device",
                    entity_type: "light",
                    entity_id: Some("bulk"),
                    action: service,
                    status: "ok",
                    message: Some(&parsed.target_label),
                },
                &config.db,
            )
            .await?;

            Ok(CommandExecution::Done {
                message: format!(
                    "Команда отправлена: {} · {} устройств",
                    parsed.target_label,
                    lights.len()
                ),
            })
        }
        CommandIntent::MultiDeviceAction { device_ids, action } => {
            if device_ids.is_empty() {
                anyhow::bail!("список устройств пуст");
            }

            let mut applied = 0usize;
            let mut errors = Vec::new();
            for device_id in device_ids {
                let device = db::devices::get_device_by_id(device_id, &config.db)
                    .await?
                    .context("устройство не найдено")?;
                let allowed =
                    db::access::can_control_device(user_id, is_admin, device_id, &config.db)
                        .await?;
                if !allowed {
                    anyhow::bail!("недостаточно прав для управления {}", device.entity_id);
                }

                let result = crate::core::devices::handle_device_interaction(
                    config,
                    device_id,
                    action.to_device_action(),
                )
                .await?;
                let label = device.alias.as_deref().unwrap_or(&device.entity_id);
                match processed_device_result(result, label) {
                    Ok(()) => applied += 1,
                    Err(error) => errors.push(error.to_string()),
                }
            }

            if !errors.is_empty() {
                anyhow::bail!(
                    "не все устройства выполнены: {}. Успешно: {}",
                    errors.join("; "),
                    applied
                );
            }

            Ok(CommandExecution::Done {
                message: format!(
                    "Команда отправлена: {} · {} устройств",
                    parsed.target_label, applied
                ),
            })
        }
        CommandIntent::DeviceAction { device_id, action } => {
            let device = db::devices::get_device_by_id(device_id, &config.db)
                .await?
                .context("устройство не найдено")?;
            let allowed =
                db::access::can_control_device(user_id, is_admin, device_id, &config.db).await?;
            if !allowed {
                anyhow::bail!("недостаточно прав для управления устройством");
            }

            let result = crate::core::devices::handle_device_interaction(
                config,
                device_id,
                action.to_device_action(),
            )
            .await?;
            let command_result = processed_device_result(
                result,
                device.alias.as_deref().unwrap_or(&device.entity_id),
            );
            let status = if command_result.is_ok() {
                "ok"
            } else {
                "error"
            };
            db::activity_log::log(
                db::activity_log::NewActivity {
                    user_id: Some(user_id),
                    kind: "device",
                    entity_type: "device",
                    entity_id: Some(&device.entity_id),
                    action: "command",
                    status,
                    message: Some(&parsed.target_label),
                },
                &config.db,
            )
            .await?;

            command_result?;

            Ok(CommandExecution::Done {
                message: format!("Команда отправлена: {}", parsed.target_label),
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

fn processed_device_result(result: InteractionResult, label: &str) -> Result<()> {
    match result {
        InteractionResult::Processed => Ok(()),
        InteractionResult::Error { error } => {
            anyhow::bail!("{}: {}", label, error)
        }
        InteractionResult::RequiresDetail | InteractionResult::RequiresInput(_) => {
            anyhow::bail!(
                "{}: действие недоступно из текстовой/голосовой команды",
                label
            )
        }
    }
}

async fn requires_confirmation(intent: &CommandIntent, config: &Arc<AppConfig>) -> Result<bool> {
    match intent {
        CommandIntent::AllLightsAction { .. } => Ok(true),
        CommandIntent::MultiDeviceAction { device_ids, .. } => Ok(device_ids.len() > 1),
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

async fn intent_label(intent: &CommandIntent, config: &Arc<AppConfig>) -> Result<String> {
    match intent {
        CommandIntent::AllLightsAction { .. } => Ok("весь свет".to_string()),
        CommandIntent::MultiDeviceAction { device_ids, .. } => {
            Ok(format!("{} устройств", device_ids.len()))
        }
        CommandIntent::DeviceAction { device_id, .. } => {
            let device = db::devices::get_device_by_id(*device_id, &config.db)
                .await?
                .context("устройство не найдено")?;
            Ok(device.alias.unwrap_or(device.entity_id))
        }
        CommandIntent::CameraSnapshot { camera_id }
        | CommandIntent::CameraClip { camera_id, .. }
        | CommandIntent::CameraArchive { camera_id } => {
            let camera = db::cameras::get_camera(*camera_id, &config.db)
                .await?
                .context("камера не найдена")?;
            Ok(camera.name)
        }
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
                anyhow::bail!(
                    "найдено несколько устройств для `{}`, уточните название",
                    name
                );
            }
        }
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
            message: Some(text),
        },
        &config.db,
    )
    .await?;

    Ok(CommandExecution::Done {
        message: format!("HA Assist: {}", answer),
    })
}

fn looks_like_readonly_question(text: &str) -> bool {
    let normalized = crate::bot::text_commands::normalize_command_text(text);

    if [
        "включи",
        "включить",
        "выключи",
        "выключить",
        "вкл",
        "выкл",
        "открой",
        "открыть",
        "закрой",
        "закрыть",
        "поставь",
        "установи",
        "измени",
        "переключи",
        "turn on",
        "turn off",
        "open",
        "close",
        "set ",
    ]
    .iter()
    .any(|word| normalized.contains(word))
    {
        return false;
    }

    [
        "какой",
        "какая",
        "какое",
        "какие",
        "сколько",
        "что",
        "где",
        "когда",
        "почему",
        "покажи",
        "скажи",
        "статус",
        "состояние",
        "температура",
        "погода",
        "what",
        "where",
        "when",
        "why",
        "how",
        "show",
        "tell",
        "status",
        "state",
        "temperature",
        "weather",
    ]
    .iter()
    .any(|word| normalized.contains(word))
}

#[cfg(test)]
mod voice_engine_tests {
    use super::*;

    #[test]
    fn readonly_questions_reject_control_phrases() {
        assert!(looks_like_readonly_question("какая температура дома"));
        assert!(looks_like_readonly_question("покажи статус света"));
        assert!(!looks_like_readonly_question("включи свет в коридоре"));
        assert!(!looks_like_readonly_question("turn on kitchen light"));
    }

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
