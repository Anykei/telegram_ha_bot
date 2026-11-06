use crate::bot::models::View;
use crate::bot::router::{ControlPayload, DeviceCmd, Payload, RenderContext, SettingsPayload};

use super::control::labels;
use crate::bot::screens::common;
use crate::core::devices::{SmartDevice, SmartEntity};
use crate::core::types::RoomViewMode;
use crate::db;
use crate::i18n::t;
use anyhow::{Context, Result};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext, room_id: i64, mode: RoomViewMode) -> Result<View> {
    render_page(ctx, room_id, mode, None).await
}

pub async fn render_page(
    ctx: RenderContext,
    room_id: i64,
    mode: RoomViewMode,
    requested: Option<u16>,
) -> Result<View> {
    let db = &ctx.config.db;

    let mut db_devices =
        db::devices::get_devices_by_room_for_user(ctx.user_id, ctx.is_admin, room_id, db).await?;
    let room = db::rooms::get_room_by_id(room_id, db)
        .await?
        .context("Room not found")?;

    ctx.track_room(room.id);
    let room_display = room.display_name();

    if mode == RoomViewMode::Control {
        let mut visible = Vec::new();
        for device in db_devices {
            if !db::subscriptions::is_hidden(&device.entity_id, db).await? {
                visible.push(device);
            }
        }
        db_devices = visible;
    }

    let header_label = match mode {
        RoomViewMode::Control => "📱 Управление",
        RoomViewMode::Settings => "⚙️ Настройки",
    };

    // Keep the original contributors: a fresh ACL-filtered query at delivery
    // would omit revoked devices while their old values remain in the keyboard.
    for device in &db_devices {
        ctx.track_device(device.id, &device.entity_id);
    }
    let entity_ids: Vec<String> = db_devices.iter().map(|d| d.entity_id.clone()).collect();
    let read_started = std::time::Instant::now();
    let ha_result = ctx.config.ha_client.fetch_states_by_ids(&entity_ids).await;
    let failed = ha_result.is_err();
    let mut ha_entities = ha_result.unwrap_or_default();
    ctx.config.html_ui.pending.observe_states(
        ctx.user_id,
        &ctx.admission_generation,
        read_started,
        &ha_entities,
    );
    // Missing HA entities remain visible with a read-only, recoverable state.
    for device in &db_devices {
        if !ha_entities.iter().any(|e| e.entity_id == device.entity_id) {
            ha_entities.push(crate::ha::models::Entity {
                entity_id: device.entity_id.clone(),
                name: device.entity_id.clone(),
                state: "unavailable".into(),
                device_class: None,
                attributes: Default::default(),
            });
        }
    }
    let current_payload = match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::RoomDetail { room: room_id }),
        RoomViewMode::Settings => {
            ctx.settings_payload(SettingsPayload::RoomDetail { room: room_id })
        }
    };
    let (page, pages) =
        common::list_page(&ctx, &current_payload, requested, db_devices.len()).await?;
    let empty = db_devices.is_empty();
    let mut rows = vec![];
    let names = labels::room_names(&db_devices, &ha_entities, ctx.lang);

    for db_dev in db_devices
        .into_iter()
        .skip(page * common::LIST_PAGE_SIZE)
        .take(common::LIST_PAGE_SIZE)
    {
        if let Some(ha_ent) = ha_entities.iter().find(|e| e.entity_id == db_dev.entity_id) {
            let smart_dev = SmartDevice::new(ha_ent.clone());
            let alias = db_dev.alias.as_deref().unwrap_or(&db_dev.entity_id);
            let domain = db_dev.entity_id.split('.').next().unwrap_or("");
            let can_control = mode == RoomViewMode::Control
                && db::access::can_control_device(ctx.user_id, ctx.is_admin, db_dev.id, db).await?;
            let quick_action = can_control
                && matches!(domain, "light" | "switch")
                && matches!(ha_ent.state.as_str(), "on" | "off");

            let mut quick_label = None;
            let text = match mode {
                RoomViewMode::Control => {
                    let inverted =
                        db::devices::is_state_inverted(&db_dev.entity_id, &ctx.config.db)
                            .await
                            .unwrap_or(false);
                    let state_alias = ctx.config.state_alias_for_display(
                        &db_dev.entity_id,
                        &ha_ent.state,
                        inverted,
                    );

                    let name = names.get(&db_dev.id).map(String::as_str).unwrap_or(alias);
                    let state = labels::state(ha_ent, ctx.lang, inverted, state_alias.as_deref());
                    let action = if quick_action {
                        let logical_on = (ha_ent.state == "on") != inverted;
                        format!(
                            "💡 {}",
                            t(
                                ctx.lang,
                                if logical_on {
                                    "control.turn_off"
                                } else {
                                    "control.turn_on"
                                }
                            )
                        )
                    } else {
                        match domain {
                            "climate" => t(ctx.lang, "climate.title").into(),
                            "sensor" | "binary_sensor" => {
                                format!("📊 {}", t(ctx.lang, "control.history"))
                            }
                            _ => format!("ℹ️ {}", t(ctx.lang, "control.details")),
                        }
                    };
                    if quick_action {
                        quick_label = Some(action);
                        format!("{name} · {state}")
                    } else {
                        format!("{action}: {name} · {state}")
                    }
                }
                RoomViewMode::Settings if ctx.is_admin => {
                    format!("#{} {}", db_dev.id, smart_dev.render_button_text(alias))
                }
                RoomViewMode::Settings => smart_dev.render_button_text(alias),
            };

            let payload = match mode {
                RoomViewMode::Control => {
                    if !quick_action {
                        Payload::Control(ControlPayload::DeviceControl {
                            room: room_id,
                            device: db_dev.id,
                        })
                    } else {
                        Payload::Control(ControlPayload::QuickAction {
                            room: room_id,
                            device: db_dev.id,
                            cmd: match (domain, ha_ent.state.as_str()) {
                                ("light" | "switch", "on") => DeviceCmd::TurnOff,
                                ("light" | "switch", "off") => DeviceCmd::TurnOn,
                                _ => DeviceCmd::Toggle,
                            },
                        })
                    }
                }
                RoomViewMode::Settings => ctx.settings_payload(SettingsPayload::DeviceDetail {
                    room: room_id,
                    device: db_dev.id,
                }),
            };

            if let Some(action) = quick_label {
                rows.push(vec![
                    InlineKeyboardButton::callback(
                        text,
                        Payload::Control(ControlPayload::DeviceControl {
                            room: room_id,
                            device: db_dev.id,
                        })
                        .to_string(),
                    ),
                    InlineKeyboardButton::callback(action, payload.to_string()),
                ]);
            } else {
                rows.push(vec![InlineKeyboardButton::callback(
                    text,
                    payload.to_string(),
                )]);
            }
        }
    }

    let back_payload = match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::ListRooms),
        RoomViewMode::Settings => ctx.settings_payload(SettingsPayload::ListRooms),
    };
    let pager = common::page_row(page, pages, |page| match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::RoomPage {
            room: room_id,
            page,
        }),
        RoomViewMode::Settings => ctx.settings_payload(SettingsPayload::RoomPage {
            room: room_id,
            page,
        }),
    });
    if !pager.is_empty() {
        rows.push(pager);
    }
    rows.extend(common::navigation_rows(
        ctx.lang,
        current_payload.clone(),
        back_payload,
    ));

    Ok(View {
        compact: mode == RoomViewMode::Control,
        notifications: ctx.notifications,
        header: Some(format!("{header_label} · {room_display}")),
        html_title: Some(room_display),
        html_context: Some(crate::bot::personal::words(ctx.lang, "Комнаты", "Rooms").into()),
        html_summary: Some(String::new()),
        alert: failed.then(|| t(ctx.lang, "control.ha_unavailable").into()),
        text: if empty {
            t(ctx.lang, "control.empty_room").into()
        } else {
            String::new()
        },
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_payload,
        ..Default::default()
    })
}
