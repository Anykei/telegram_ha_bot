// use crate::models::{RenderContext, View, Device, Payload, ControlPayload, DeviceCmd, HeaderItem};
use crate::ha::models::Entity;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use crate::bot::models::View;
use crate::bot::router::{ControlPayload, DeviceCmd, Payload, RenderContext, SettingsPayload};
use crate::core::HeaderItem;
use crate::core::types::{Device, RoomViewMode};

pub async fn render(ctx: RenderContext, room_id: i64, dev: Device, entity: Entity) -> anyhow::Result<View> {
    // Извлекаем атрибуты климата
    // let cur_temp = entity.attributes.get("current_temperature").and_then(|v| v.as_f64()).unwrap_or(0.0);
    // let target_temp = entity.attributes.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let target_temp = 15.0f32;
    // Шапка пульта
    // let text = vec![HeaderItem {
    //     icon: "🌡".into(),
    //     label: dev.alias.as_deref().unwrap_or("").to_string(),
    //     value: "".to_string(),//format!("*{}°C* → 🎯 *{}°C*", cur_temp, target_temp),
    //     last_update: chrono::Utc::now(),
    // }];

    let text = format!("❄️ *Управление климатом*\nРежим: `{}`", entity.state.to_uppercase());

    let mut rows = vec![];

    // Кнопки температуры
    rows.push(vec![
        InlineKeyboardButton::callback("➖ 0.5°", Payload::Control(ControlPayload::QuickAction {
            room: room_id, device: dev.id, cmd: DeviceCmd::SetTemp((target_temp - 0.5) as f32)
        }).to_string()),
        InlineKeyboardButton::callback("➕ 0.5°", Payload::Control(ControlPayload::QuickAction {
            room: room_id, device: dev.id, cmd: DeviceCmd::SetTemp((target_temp + 0.5) as f32)
        }).to_string()),
    ]);

    // Кнопка назад
    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Control(ControlPayload::RoomDetail { room: room_id })
    )]);

    Ok(View {
        header: Some("🌡 Термостат".into()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Control(ControlPayload::QuickAction {room: room_id, device: dev.id, cmd: DeviceCmd::Toggle}),
        ..Default::default()
    })

    // ctx.notifications;
    // Ok(View {
    //     // header: Some("🌡 Термостат".into()),
    //     ctx.notifications,
    //     text,
    //     kb: InlineKeyboardMarkup::new(rows),
    //     payload: Payload::Control(ControlPayload::DeviceControl { room: room_id, device: dev.id }),
    //     ..Default::default()
    // })
}