use crate::bot::models::{View};
use crate::bot::router::{AdminPayload, ControlPayload, Payload, RenderContext, SettingsPayload};

use anyhow::{Context, Result};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use crate::db;

pub async fn render(ctx: RenderContext, room_id: i64, device_id: i64) -> Result<View> {
    let db = &ctx.config.db;

    let dev = db::devices::get_device_by_id(device_id, db).await?
        .context("Device not found")?;

    let subscribed = db::subscriptions::is_subscribed(ctx.user_id as i64, &dev.entity_id, db).await;
    let hidden = db::subscriptions::is_hidden(&dev.entity_id, db).await;

    let ha_ent = ctx.config.ha_client.fetch_states_by_ids(&[dev.entity_id.clone()]).await?
        .into_iter().next().context("HA offline")?;

    let domain = dev.entity_id.split('.').next().unwrap_or("");
    let status_text = crate::core::presentation::StateFormatter::translate_state(&ha_ent.state);

    let text = format!(
        "⚙️ Параметры\n\n\
        🛠 *Настройки устройства*\n\n\
        Имя: `{}`\n\
        ID: `{}`\n\
        Статус: {}\n\
        ────────────────────\n\
        Настройте поведение устройства в боте:",
        dev.alias.as_deref().unwrap_or(&dev.entity_id),
        dev.entity_id,
        status_text
    );

    let mut rows = vec![];

    let (sub_icon, sub_label) = if subscribed { ("🔔", "Уведомления: ВКЛ") } else { ("🔕", "Уведомления: ВЫКЛ") };
    rows.push(vec![InlineKeyboardButton::callback(
        format!("{} {}", sub_icon, sub_label),
        Payload::Settings(SettingsPayload::ToggleNotify { room: room_id, device: device_id }).to_string()
    )]);

    let (hide_icon, hide_label) = if hidden { ("👁", "Показать в управлении") } else { ("🚫", "Скрыть из управления") };
    rows.push(vec![InlineKeyboardButton::callback(
        format!("{} {}", hide_icon, hide_label),
        Payload::Settings(SettingsPayload::ToggleHide { room: room_id, device: device_id }).to_string()
    )]);

    rows.push(vec![InlineKeyboardButton::callback(
        "✏️ Изменить имя",
        Payload::Settings(SettingsPayload::EditName { room: room_id, device: device_id }).to_string()
    )]);

    // Кнопка "Назад"
    rows.push(vec![InlineKeyboardButton::callback(
        "⬅️ Назад к списку",
        Payload::Settings(SettingsPayload::RoomDetail { room: room_id }).to_string()
    )]);

    Ok(View {
        // header: Some("⚙️ Параметры".into()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Settings(SettingsPayload::DeviceDetail { room: room_id, device: device_id }),
        ..Default::default()
    })
}