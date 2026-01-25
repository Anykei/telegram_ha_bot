use crate::bot::models::View;
use crate::bot::router::{ControlPayload, DeviceCmd, Payload, RenderContext, SettingsPayload};

use crate::core::devices::{SmartDevice, SmartEntity};
use crate::bot::screens::common;
use crate::db;
use anyhow::{Context, Result};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use crate::core::types::RoomViewMode;

pub async fn render(ctx: RenderContext, room_id: i64, mode: RoomViewMode) -> Result<View> {
    let db = &ctx.config.db;

    let db_devices = db::devices::get_devices_by_room(room_id, db).await?;
    let room = db::rooms::get_room_by_id(room_id, db).await?
        .context("Room not found")?;

    let room_display = room.display_name();

    let header_label = match mode {
        RoomViewMode::Control => "📱 Управление",
        RoomViewMode::Settings => "⚙️ Настройки",
    };

    let entity_ids: Vec<String> = db_devices.iter().map(|d| d.entity_id.clone()).collect();
    let ha_entities = ctx.config.ha_client.fetch_states_by_ids(&entity_ids).await?;

    let mut rows = vec![];

    for db_dev in db_devices {
        if db::subscriptions::is_hidden(db_dev.entity_id.as_str(), &ctx.config.db).await{
            continue;
        }

        if let Some(ha_ent) = ha_entities.iter().find(|e| e.entity_id == db_dev.entity_id) {

            let smart_dev = SmartDevice::new(ha_ent.clone());
            let alias = db_dev.alias.as_deref().unwrap_or(&db_dev.entity_id);

            let text = match mode {
                RoomViewMode::Control => smart_dev.render_button_text_with_state(alias),
                RoomViewMode::Settings => smart_dev.render_button_text(alias)
            };

            let payload = match mode {
                RoomViewMode::Control => {
                    let domain = db_dev.entity_id.split('.').next().unwrap_or("");
                    if domain == "climate" {
                        Payload::Control(ControlPayload::DeviceControl { room: room_id, device: db_dev.id })
                    } else {
                        Payload::Control(ControlPayload::QuickAction {
                            room: room_id,
                            device: db_dev.id,
                            cmd: DeviceCmd::Toggle
                        })
                    }
                }
                RoomViewMode::Settings => {
                    Payload::Settings(SettingsPayload::DeviceDetail { room: room_id, device: db_dev.id })
                }
            };

            rows.push(vec![InlineKeyboardButton::callback(text, payload.to_string())]);
        }
    }

    let back_payload = match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::ListRooms),
        RoomViewMode::Settings => Payload::Settings(SettingsPayload::ListRooms),
    };
    rows.push(vec![common::back_button(back_payload)]);

    Ok(View {
        notifications: ctx.notifications,
        text: format!("{} {}", header_label, room_display),
        kb: InlineKeyboardMarkup::new(rows),
        payload: match mode {
            RoomViewMode::Control => Payload::Control(ControlPayload::RoomDetail { room: room_id }),
            RoomViewMode::Settings => Payload::Settings(SettingsPayload::RoomDetail { room: room_id }),
        },
        ..Default::default()
    })
}
