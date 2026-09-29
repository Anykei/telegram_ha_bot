use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use super::models::View;
use super::router::{AdminPayload, ControlPayload, DeviceCmd, Payload, RenderContext};
use crate::db::{self, action_groups::ActionGroupCommand, ui_confirmations as pending};
use crate::i18n::t;

#[derive(Serialize, Deserialize)]
enum Action {
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
pub async fn intercept(ctx: &RenderContext, payload: &Payload) -> Result<Option<View>> {
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
                    critical |=
                        db::devices::is_device_critical(&item.entity_id, &ctx.config.db).await?;
                    targets.push(item.device_id);
                }
            }
            if !critical {
                return Ok(None);
            }
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
                    "{}\n{} · {} {}",
                    group_row.name,
                    verb,
                    targets.len(),
                    t(ctx.lang, "confirm.devices")
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
        _ => return Ok(None),
    };
    let id = pending::create(
        ctx.user_id,
        &serde_json::to_string(&proposal)?,
        &ctx.config.db,
    )
    .await?;
    Ok(Some(render(ctx, id).await?))
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
            perform(ctx, proposal.action).await
        }
        .await;
        let message = match &result {
            Ok(message) => message.clone(),
            Err(error) => {
                log::warn!("Confirmed UI action {id} failed: {error:#}");
                t(ctx.lang, "confirm.failed").to_string()
            }
        };
        pending::finish(id, result.is_ok(), &message, &ctx.config.db).await?;
    }
    render(ctx, id).await
}

async fn perform(ctx: &RenderContext, action: Action) -> Result<String> {
    match action {
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
            if let DeviceCmd::SetTemp(target) = cmd {
                let result = ctx
                    .config
                    .climate_controls
                    .enqueue_confirmed(ctx.config.clone(), ctx.user_id, device, target)?
                    .await
                    .context("Climate worker stopped")??;
                return Ok(super::screens::control::temperature_command_notice(
                    ctx.lang, &result,
                ));
            }
            let result =
                crate::core::devices::handle_device_interaction(&ctx.config, device, cmd.into())
                    .await?;
            ensure!(
                matches!(result, crate::core::devices::InteractionResult::Processed),
                "Command did not complete"
            );
            Ok(t(ctx.lang, "confirm.sent").into())
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
            ensure!(result.status() == "ok", "{}", result.user_message());
            Ok(result.user_message())
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
    let active = record.status == "pending" && record.expires_at > chrono::Utc::now().timestamp();
    let mut rows = Vec::new();
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
            "executed" | "failed" => record
                .result_text
                .as_deref()
                .unwrap_or(t(ctx.lang, "confirm.failed")),
            "executing" => t(ctx.lang, "confirm.executing"),
            "cancelled" => t(ctx.lang, "confirm.cancelled"),
            _ => t(ctx.lang, "confirm.expired"),
        }
    };
    rows.push(vec![super::screens::common::back_button_lang(
        ctx.lang,
        proposal.back,
    )]);
    Ok(View {
        compact: true,
        header: Some(t(ctx.lang, "confirm.title").into()),
        text: format!("{}\n\n{}", proposal.description, status),
        notifications: ctx.notifications.clone(),
        kb: InlineKeyboardMarkup::new(rows),
        payload,
        lang: ctx.lang,
        ..Default::default()
    })
}
