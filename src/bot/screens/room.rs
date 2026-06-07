use crate::bot::models::View;
use crate::bot::router::{ControlPayload, DeviceCmd, Payload, RenderContext, SettingsPayload};

use crate::bot::screens::common;
use crate::core::devices::{SmartDevice, SmartEntity};
use crate::core::types::RoomViewMode;
use crate::db;
use anyhow::{Context, Result};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext, room_id: i64, mode: RoomViewMode) -> Result<View> {
    let db = &ctx.config.db;

    let db_devices =
        db::devices::get_devices_by_room_for_user(ctx.user_id, ctx.is_admin, room_id, db).await?;
    let room = db::rooms::get_room_by_id(room_id, db)
        .await?
        .context("Room not found")?;

    let room_display = room.display_name();

    let header_label = match mode {
        RoomViewMode::Control => "📱 Управление",
        RoomViewMode::Settings => "⚙️ Настройки",
    };

    let entity_ids: Vec<String> = db_devices.iter().map(|d| d.entity_id.clone()).collect();
    let ha_entities = ctx
        .config
        .ha_client
        .fetch_states_by_ids(&entity_ids)
        .await?;

    let mut rows = vec![];

    for db_dev in db_devices {
        if db::subscriptions::is_hidden(db_dev.entity_id.as_str(), &ctx.config.db)
            .await
            .unwrap_or(false)
            && mode == RoomViewMode::Control
        {
            continue;
        }

        if let Some(ha_ent) = ha_entities.iter().find(|e| e.entity_id == db_dev.entity_id) {
            let smart_dev = SmartDevice::new(ha_ent.clone());
            let alias = db_dev.alias.as_deref().unwrap_or(&db_dev.entity_id);

            let text = match mode {
                RoomViewMode::Control => {
                    let domain = db_dev.entity_id.split('.').next().unwrap_or("");
                    let class = ha_ent.device_class.as_deref().unwrap_or("");
                    let inverted =
                        db::devices::is_state_inverted(&db_dev.entity_id, &ctx.config.db)
                            .await
                            .unwrap_or(false);
                    let state_alias = ctx.config.state_alias_for_display(
                        &db_dev.entity_id,
                        &ha_ent.state,
                        inverted,
                    );

                    crate::core::presentation::StateFormatter::format_device_label_with_state_alias(
                        alias,
                        domain,
                        class,
                        &ha_ent.state,
                        inverted,
                        state_alias.as_deref(),
                    )
                }
                RoomViewMode::Settings if ctx.is_admin => {
                    format!("#{} {}", db_dev.id, smart_dev.render_button_text(alias))
                }
                RoomViewMode::Settings => smart_dev.render_button_text(alias),
            };

            let payload = match mode {
                RoomViewMode::Control => {
                    let domain = db_dev.entity_id.split('.').next().unwrap_or("");
                    if domain == "climate" {
                        Payload::Control(ControlPayload::DeviceControl {
                            room: room_id,
                            device: db_dev.id,
                        })
                    } else {
                        Payload::Control(ControlPayload::QuickAction {
                            room: room_id,
                            device: db_dev.id,
                            cmd: DeviceCmd::Toggle,
                        })
                    }
                }
                RoomViewMode::Settings => ctx.settings_payload(SettingsPayload::DeviceDetail {
                    room: room_id,
                    device: db_dev.id,
                }),
            };

            rows.push(vec![InlineKeyboardButton::callback(
                text,
                payload.to_string(),
            )]);
        }
    }

    let back_payload = match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::ListRooms),
        RoomViewMode::Settings => ctx.settings_payload(SettingsPayload::ListRooms),
    };
    rows.push(vec![common::back_button(back_payload)]);

    let current_payload = match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::RoomDetail { room: room_id }),
        RoomViewMode::Settings => {
            ctx.settings_payload(SettingsPayload::RoomDetail { room: room_id })
        }
    };

    Ok(View {
        notifications: ctx.notifications,
        text: format!("{} {}", header_label, room_display),
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_payload,
        ..Default::default()
    })
}
