use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use super::models::View;
use super::router::{AdminPayload, ControlPayload, DeviceCmd, Payload, RenderContext};
use crate::core::action_results::{ActionResult, Outcome, TargetKind, TargetResult};
use crate::db::{self, action_groups::ActionGroupCommand, ui_confirmations as pending};
use crate::i18n::t;

#[derive(Clone, Serialize, Deserialize)]
enum Action {
    Native {
        target: i64,
    },
    Device {
        device: i64,
        entity_id: String,
        cmd: DeviceCmd,
    },
    Group {
        group: i64,
        command: ActionGroupCommand,
        targets: Vec<i64>,
    },
}

#[derive(Serialize, Deserialize)]
struct Proposal {
    action: Action,
    description: String,
    back: Payload,
}

pub async fn is_critical_device(device: i64, config: &crate::models::AppConfig) -> Result<bool> {
    let device = db::devices::get_device_by_id(device, &config.db)
        .await?
        .context("Device missing")?;
    db::devices::is_device_critical(&device.entity_id, &config.db).await
}

/// Intercept writes, including old callback buttons. Review callbacks never recreate a proposal.
async fn proposal(ctx: &RenderContext, payload: &Payload) -> Result<Option<(Proposal, bool)>> {
    let mut auto_execute = false;
    let proposal = match payload {
        Payload::Control(ControlPayload::QuickAction { room, device, cmd })
            if super::router::action_requires_control(cmd) =>
        {
            if !is_critical_device(*device, &ctx.config).await? {
                return Ok(None);
            }
            ensure!(
                db::access::can_control_device(ctx.user_id, ctx.is_admin, *device, &ctx.config.db)
                    .await?,
                "Access denied"
            );
            let dev = db::devices::get_device_by_id(*device, &ctx.config.db)
                .await?
                .context("Device missing")?;
            let entity = ctx
                .config
                .ha_client
                .fetch_states_by_ids(std::slice::from_ref(&dev.entity_id))
                .await?
                .into_iter()
                .next()
                .context("Device unavailable")?;
            let domain = dev.entity_id.split('.').next().unwrap_or("");
            let command = match cmd {
                DeviceCmd::Toggle => {
                    ensure!(matches!(domain, "light" | "switch"), "Unsupported command");
                    match entity.state.as_str() {
                        "on" => DeviceCmd::TurnOff,
                        "off" => DeviceCmd::TurnOn,
                        _ => anyhow::bail!("Device state unavailable"),
                    }
                }
                DeviceCmd::AdjustTemp(direction) => {
                    ensure!(matches!(direction, -1 | 1), "Invalid temperature step");
                    let base = entity.attributes.temperature.context("Target missing")?;
                    ensure!(
                        entity.temperature_target_is_valid(base),
                        "Temperature unavailable"
                    );
                    let step = entity
                        .attributes
                        .target_temp_step
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .unwrap_or(0.5);
                    let target =
                        (((base + f32::from(*direction) * step) * 1000.0).round() / 1000.0).clamp(
                            entity.attributes.min_temp.unwrap(),
                            entity.attributes.max_temp.unwrap(),
                        );
                    DeviceCmd::SetTemp(target)
                }
                other => other.clone(),
            };
            let inverted = db::devices::is_state_inverted(&dev.entity_id, &ctx.config.db).await?;
            let verb = match &command {
                DeviceCmd::TurnOn | DeviceCmd::TurnOff => {
                    ensure!(matches!(domain, "light" | "switch"), "Unsupported command");
                    let turn_on = matches!(command, DeviceCmd::TurnOn) != inverted;
                    t(
                        ctx.lang,
                        if turn_on {
                            "control.turn_on"
                        } else {
                            "control.turn_off"
                        },
                    )
                    .to_string()
                }
                DeviceCmd::SetTemp(target) => {
                    ensure!(
                        domain == "climate" && entity.temperature_target_is_valid(*target),
                        "Invalid temperature"
                    );
                    format!(
                        "{}: {}{}",
                        t(ctx.lang, "climate.target"),
                        target,
                        entity
                            .attributes
                            .unit_of_measurement
                            .as_deref()
                            .unwrap_or("°")
                    )
                }
                DeviceCmd::SetLevel(level) => {
                    ensure!(domain == "light", "Unsupported command");
                    format!(
                        "{}: {:.1}%",
                        t(ctx.lang, "confirm.brightness"),
                        f32::from(*level) * 100.0 / 255.0
                    )
                }
                _ => anyhow::bail!("Unsupported command"),
            };
            let actual_room = db::devices::get_room_id_by_entity(&dev.entity_id, &ctx.config.db)
                .await?
                .unwrap_or(*room);
            let room = &actual_room;
            let room_row = db::rooms::get_room_by_id(*room, &ctx.config.db).await?;
            let devices = db::devices::get_devices_by_room_for_user(
                ctx.user_id,
                ctx.is_admin,
                *room,
                &ctx.config.db,
            )
            .await?;
            let mut visible = Vec::new();
            for device in devices {
                if !db::subscriptions::is_hidden(&device.entity_id, &ctx.config.db).await? {
                    visible.push(device);
                }
            }
            let ids = visible
                .iter()
                .map(|d| d.entity_id.clone())
                .collect::<Vec<_>>();
            let states = ctx.config.ha_client.fetch_states_by_ids(&ids).await?;
            let names = super::screens::control::labels::room_names(&visible, &states, ctx.lang);
            let name = names.get(device).cloned().unwrap_or_else(|| {
                super::screens::control::labels::device_name(&dev, &entity, ctx.lang)
            });
            let location = room_row
                .map(|r| r.display_name().to_string())
                .unwrap_or_default();
            Proposal {
                action: Action::Device {
                    device: *device,
                    entity_id: dev.entity_id.clone(),
                    cmd: command,
                },
                description: format!("{location} · {name}\n{verb}"),
                back: if domain == "climate" {
                    Payload::Control(ControlPayload::DeviceControl {
                        room: *room,
                        device: *device,
                    })
                } else {
                    Payload::Control(ControlPayload::RoomDetail { room: *room })
                },
            }
        }
        Payload::Control(ControlPayload::ExecuteActionGroup { group, command })
        | Payload::Admin(AdminPayload::ExecuteActionGroup { group, command }) => {
            ensure!(
                !matches!(payload, Payload::Admin(_)) || ctx.is_admin,
                "Access denied"
            );
            let group_row = db::action_groups::get_action_group(*group, &ctx.config.db)
                .await?
                .context("Group missing")?;
            ensure!(
                group_row.is_enabled() && (ctx.is_admin || group_row.is_visible_to_all_users()),
                "Group unavailable"
            );
            let mut targets = Vec::new();
            let mut critical = false;
            let mut critical_names = Vec::new();
            for item in db::action_groups::list_action_group_items(*group, &ctx.config.db).await? {
                if item.is_archived() || !matches!(item.device_domain.as_str(), "light" | "switch")
                {
                    continue;
                }
                if db::access::can_control_device(
                    ctx.user_id,
                    ctx.is_admin,
                    item.device_id,
                    &ctx.config.db,
                )
                .await?
                {
                    let is_critical =
                        db::devices::is_device_critical(&item.entity_id, &ctx.config.db).await?;
                    critical |= is_critical;
                    if is_critical {
                        let device = db::devices::get_device_by_id(item.device_id, &ctx.config.db)
                            .await?
                            .context("Device missing")?;
                        critical_names.push(device.alias.unwrap_or(device.entity_id));
                    }
                    targets.push(item.device_id);
                }
            }
            auto_execute = !critical;
            targets.sort_unstable();
            let command = if *command == ActionGroupCommand::Toggle {
                crate::core::action_groups::aggregate_group_state(
                    *group,
                    ctx.user_id,
                    ctx.is_admin,
                    &ctx.config,
                )
                .await?
                .dynamic_command()
            } else {
                *command
            };
            let verb = t(
                ctx.lang,
                if command == ActionGroupCommand::TurnOn {
                    "control.turn_on"
                } else {
                    "control.turn_off"
                },
            );
            Proposal {
                description: format!(
                    "{}\n{} · {} {}\n{}",
                    group_row.name,
                    verb,
                    targets.len(),
                    t(ctx.lang, "confirm.devices"),
                    critical_names.join("\n")
                ),
                action: Action::Group {
                    group: *group,
                    command,
                    targets,
                },
                back: if matches!(payload, Payload::Admin(_)) {
                    Payload::Admin(AdminPayload::ActionGroupDetail { group: *group })
                } else {
                    Payload::Control(ControlPayload::ActionGroupDetail { group: *group })
                },
            }
        }
        Payload::Control(ControlPayload::ExecuteHaNativeAction { action })
        | Payload::Admin(AdminPayload::ExecuteHaNativeAction { action }) => {
            ensure!(
                !matches!(payload, Payload::Admin(_)) || ctx.is_admin,
                "Access denied"
            );
            let target = db::action_groups::get_ha_native_target(*action, &ctx.config.db)
                .await?
                .context("Target missing")?;
            ensure!(
                !target.is_archived()
                    && target.is_enabled()
                    && (ctx.is_admin || target.is_visible_to_all_users()),
                "Target unavailable"
            );
            auto_execute = true;
            Proposal {
                action: Action::Native { target: *action },
                description: target.display_name().to_string(),
                back: if matches!(payload, Payload::Admin(_)) {
                    Payload::Admin(AdminPayload::HaNativeActionDetail { action: *action })
                } else {
                    Payload::Control(ControlPayload::HaNativeActionDetail { action: *action })
                },
            }
        }
        _ => return Ok(None),
    };
    Ok(Some((proposal, auto_execute)))
}

pub async fn intercept(ctx: &RenderContext, payload: &Payload) -> Result<Option<View>> {
    let Some((proposal, auto_execute)) = proposal(ctx, payload).await? else {
        return Ok(None);
    };
    if ctx.is_admin {
        db::add_user(ctx.user_id, &ctx.config.db).await?;
    }
    let id = pending::create(
        ctx.user_id,
        &serde_json::to_string(&proposal)?,
        &ctx.config.db,
    )
    .await?;
    Ok(Some(handle(ctx, id, auto_execute, false).await?))
}

/// Rendered mutation buttons carry a single-use server operation ID.
/// Rendering prepares a proposal, and never sends a command to HA.
pub async fn prepare_button(ctx: &RenderContext, payload: &Payload) -> Result<Payload> {
    if !matches!(
        payload,
        Payload::Control(
            ControlPayload::ExecuteActionGroup { .. }
                | ControlPayload::ExecuteHaNativeAction { .. }
        ) | Payload::Admin(
            AdminPayload::ExecuteActionGroup { .. } | AdminPayload::ExecuteHaNativeAction { .. }
        )
    ) {
        return Ok(payload.clone());
    }
    let Some((proposal, auto_execute)) = proposal(ctx, payload).await? else {
        return Ok(payload.clone());
    };
    if ctx.is_admin {
        db::add_user(ctx.user_id, &ctx.config.db).await?;
    }
    let id = pending::create_independent(
        ctx.user_id,
        &serde_json::to_string(&proposal)?,
        &ctx.config.db,
    )
    .await?;
    Ok(operation_payload(id, !auto_execute))
}

fn operation_payload(id: i64, review: bool) -> Payload {
    Payload::Control(if review {
        ControlPayload::ReviewAction { id }
    } else {
        ControlPayload::ConfirmAction { id }
    })
}

/// Adapt already delivered legacy buttons. Their message/payload identity survives restart.
pub async fn prepare_legacy_callback(
    payload: Payload,
    user: u64,
    chat: i64,
    message: i32,
    config: std::sync::Arc<crate::models::AppConfig>,
) -> Result<Payload> {
    if !matches!(
        &payload,
        Payload::Control(
            ControlPayload::ExecuteActionGroup { .. }
                | ControlPayload::ExecuteHaNativeAction { .. }
        ) | Payload::Admin(
            AdminPayload::ExecuteActionGroup { .. } | AdminPayload::ExecuteHaNativeAction { .. }
        )
    ) {
        return Ok(payload);
    }
    let key = format!("{chat}:{message}:{}", payload.to_string());
    if let Some((id, review)) = pending::callback_operation(user, &key, &config.db).await? {
        return Ok(operation_payload(id, review));
    }
    let ctx = RenderContext {
        user_id: user,
        is_admin: user == config.root_user,
        lang: db::get_user_language(user, &config.db)
            .await?
            .unwrap_or(config.default_language),
        notifications: Vec::new(),
        config: config.clone(),
        settings_origin: super::router::SettingsOrigin::Home,
    };
    let Some((proposal, auto_execute)) = proposal(&ctx, &payload).await? else {
        return Ok(payload);
    };
    if ctx.is_admin {
        db::add_user(user, &config.db).await?;
    }
    let (id, review) = pending::bind_callback(
        user,
        &key,
        &serde_json::to_string(&proposal)?,
        !auto_execute,
        &config.db,
    )
    .await?;
    Ok(operation_payload(id, review))
}

pub async fn handle(ctx: &RenderContext, id: i64, execute: bool, cancel: bool) -> Result<View> {
    if cancel {
        pending::cancel(id, ctx.user_id, &ctx.config.db).await?;
    }
    if execute && pending::claim(id, ctx.user_id, &ctx.config.db).await? {
        let record = pending::get(id, ctx.user_id, &ctx.config.db)
            .await?
            .context("Confirmation missing")?;
        // A claimed action stays consumed even if parsing or the service request fails.
        let result = async {
            let proposal: Proposal = serde_json::from_str(&record.action_json)?;
            let targets = match &proposal.action {
                Action::Device { device, .. } => vec![(TargetKind::Device, *device)],
                Action::Group { targets, .. } => {
                    targets.iter().map(|id| (TargetKind::Device, *id)).collect()
                }
                Action::Native { target } => vec![(TargetKind::HaNative, *target)],
            };
            Ok::<_, anyhow::Error>(perform(ctx, proposal.action).await.unwrap_or_else(|_| {
                ActionResult::new(
                    targets
                        .into_iter()
                        .map(|(kind, id)| {
                            TargetResult::new(
                                kind,
                                id,
                                Outcome::Failed,
                                Some("Validation failed / Проверка не пройдена"),
                            )
                        })
                        .collect(),
                )
            }))
        }
        .await;
        let result = result.unwrap_or_else(|_| {
            ActionResult::new(vec![TargetResult::new(
                TargetKind::Device,
                0,
                Outcome::Failed,
                Some("Validation failed / Проверка не пройдена"),
            )])
        });
        let message = result.display(ctx.user_id, ctx.lang, &ctx.config).await?;
        pending::finish(id, &result, &message, &ctx.config.db).await?;
        let correlation = id.to_string();
        let _ = db::activity_log::log(
            db::activity_log::NewActivity {
                user_id: Some(ctx.user_id),
                kind: "command_result",
                entity_type: "ui_confirmation",
                entity_id: Some(&correlation),
                action: "finish",
                status: match result.outcome {
                    Outcome::Sent => "sent",
                    Outcome::Partial => "partial",
                    Outcome::Failed => "failed",
                    Outcome::Unknown => "unknown",
                },
                message: Some(&result.summary(ctx.lang)),
            },
            &ctx.config.db,
        )
        .await;
    }
    render(ctx, id).await
}

async fn perform(ctx: &RenderContext, action: Action) -> Result<ActionResult> {
    match action {
        Action::Native { target } => Ok(crate::core::action_groups::execute_ha_native_target(
            target,
            crate::core::action_groups::ActionActor::User(ctx.user_id),
            ctx.config.clone(),
        )
        .await?
        .result),
        Action::Device {
            device,
            entity_id,
            cmd,
        } => {
            ensure!(
                db::access::can_control_device(ctx.user_id, ctx.is_admin, device, &ctx.config.db)
                    .await?,
                "Access revoked"
            );
            let current = db::devices::get_device_by_id(device, &ctx.config.db)
                .await?
                .context("Device removed")?;
            ensure!(current.entity_id == entity_id, "Device changed");
            let outcome = if let DeviceCmd::SetTemp(target) = cmd {
                match ctx
                    .config
                    .climate_controls
                    .enqueue_confirmed(ctx.config.clone(), ctx.user_id, device, target)?
                    .await
                {
                    Ok(Ok(_)) => Outcome::Sent,
                    _ => Outcome::Unknown,
                }
            } else {
                match crate::core::devices::handle_device_interaction(
                    &ctx.config,
                    device,
                    cmd.into(),
                )
                .await?
                {
                    crate::core::devices::InteractionResult::Processed => Outcome::Sent,
                    crate::core::devices::InteractionResult::Error { .. } => Outcome::Unknown,
                    _ => Outcome::Failed,
                }
            };
            Ok(ActionResult::new(vec![TargetResult::new(
                TargetKind::Device,
                device,
                outcome,
                None,
            )]))
        }
        Action::Group {
            group,
            command,
            targets,
        } => {
            let result = crate::core::action_groups::execute_action_group_with_targets(
                group,
                command,
                crate::core::action_groups::ActionActor::User(ctx.user_id),
                ctx.config.clone(),
                Some(&targets),
            )
            .await?;
            Ok(result.result)
        }
    }
}

async fn render(ctx: &RenderContext, id: i64) -> Result<View> {
    let payload = Payload::Control(ControlPayload::ReviewAction { id });
    let Some(record) = pending::get(id, ctx.user_id, &ctx.config.db).await? else {
        return Ok(View {
            compact: true,
            text: t(ctx.lang, "confirm.expired").into(),
            payload,
            kb: InlineKeyboardMarkup::new(vec![vec![
                super::screens::common::main_menu_button_lang(ctx.lang),
            ]]),
            lang: ctx.lang,
            ..Default::default()
        });
    };
    let proposal: Proposal = serde_json::from_str(&record.action_json)?;
    let allowed = proposal_visible(ctx, &proposal.action).await?;
    let active =
        allowed && record.status == "pending" && record.expires_at > chrono::Utc::now().timestamp();
    let mut rows = Vec::new();
    let saved = match record.result_json.as_deref() {
        Some(json) => Some(serde_json::from_str::<ActionResult>(json)?),
        None => None,
    };
    let saved_text = match &saved {
        Some(result) => Some(result.display(ctx.user_id, ctx.lang, &ctx.config).await?),
        None => None,
    };
    let status = if active {
        rows.push(vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "confirm.yes"),
                Payload::Control(ControlPayload::ConfirmAction { id }).to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "confirm.cancel"),
                Payload::Control(ControlPayload::CancelAction { id }).to_string(),
            ),
        ]);
        t(ctx.lang, "confirm.prompt")
    } else {
        match record.status.as_str() {
            "executed" | "failed" => saved_text
                .as_deref()
                .or(record.result_text.as_deref())
                .unwrap_or(t(ctx.lang, "confirm.failed")),
            "executing" => {
                if ctx.lang == crate::i18n::Language::En {
                    "Send outcome unknown. Check states first."
                } else {
                    "Результат отправки не удалось определить. Сначала проверьте состояния."
                }
            }
            "cancelled" => t(ctx.lang, "confirm.cancelled"),
            _ => t(ctx.lang, "confirm.expired"),
        }
    };
    if let Some(result) = &saved {
        for target in &result.targets {
            if target.kind == TargetKind::HaNative {
                if let Some(name) =
                    crate::core::action_results::target_label(target, ctx.user_id, &ctx.config)
                        .await?
                {
                    rows.push(vec![InlineKeyboardButton::callback(
                        name,
                        Payload::Control(ControlPayload::HaNativeActionDetail {
                            action: target.id,
                        })
                        .to_string(),
                    )]);
                }
                continue;
            }
            let Some(name) =
                crate::core::action_results::target_label(target, ctx.user_id, &ctx.config).await?
            else {
                continue;
            };
            let Some(device) = db::devices::get_device_by_id(target.id, &ctx.config.db).await?
            else {
                continue;
            };
            let Some(room) =
                db::devices::get_room_id_by_entity(&device.entity_id, &ctx.config.db).await?
            else {
                continue;
            };
            rows.push(vec![InlineKeyboardButton::callback(
                name,
                Payload::Control(ControlPayload::DeviceControl {
                    room,
                    device: target.id,
                })
                .to_string(),
            )]);
        }
    }
    rows.push(vec![super::screens::common::back_button_lang(
        ctx.lang,
        if allowed {
            proposal.back
        } else {
            Payload::Home
        },
    )]);
    Ok(View {
        compact: true,
        header: Some(t(ctx.lang, "confirm.title").into()),
        text: if active {
            format!("{}\n\n{}", proposal.description, status)
        } else {
            status.to_string()
        },
        notifications: ctx.notifications.clone(),
        kb: InlineKeyboardMarkup::new(rows),
        payload,
        lang: ctx.lang,
        ..Default::default()
    })
}

async fn proposal_visible(ctx: &RenderContext, action: &Action) -> Result<bool> {
    match action {
        Action::Device { device, .. } => {
            db::access::can_control_device(ctx.user_id, ctx.is_admin, *device, &ctx.config.db).await
        }
        Action::Native { target } => Ok(db::action_groups::get_ha_native_target(
            *target,
            &ctx.config.db,
        )
        .await?
        .is_some_and(|t| !t.is_archived() && (ctx.is_admin || t.is_visible_to_all_users()))),
        Action::Group { group, targets, .. } => {
            let group = db::action_groups::get_action_group(*group, &ctx.config.db).await?;
            if !group.is_some_and(|g| ctx.is_admin || g.is_visible_to_all_users()) {
                return Ok(false);
            }
            for device in targets {
                if !db::access::can_control_device(
                    ctx.user_id,
                    ctx.is_admin,
                    *device,
                    &ctx.config.db,
                )
                .await?
                {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}
