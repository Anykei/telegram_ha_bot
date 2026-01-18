use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use anyhow::Result;

use crate::bot::models::View;
use crate::bot::router::{ControlPayload, Payload, RenderContext, SettingsPayload};
pub(crate) use crate::core::types::RoomViewMode;
use crate::db;



pub async fn render(ctx: RenderContext, mode: RoomViewMode) -> Result<View> {
    let rooms = db::rooms::get_rooms(&ctx.config.db).await.unwrap_or_else(|_| Vec::new());

    let text = crate::core::presentation::StateFormatter::get_rooms_header(&mode);

    let mut rows = vec![];
    for room in rooms {
        let callback_payload = match mode {
            RoomViewMode::Control =>
                Payload::Control(ControlPayload::RoomDetail { room: room.id }),
            RoomViewMode::Settings =>
                Payload::Settings(SettingsPayload::RoomDetail { room: room.id }),
        };

        rows.push(vec![InlineKeyboardButton::callback(
            room.display_name(),
            callback_payload.to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button(Payload::Home)]);

    let current_payload = match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::ListRooms),
        RoomViewMode::Settings => Payload::Settings(SettingsPayload::ListRooms),
    };

    Ok(View {
        notifications: ctx.notifications,
        text: text.to_string(),
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_payload,
        ..Default::default()
    })
}