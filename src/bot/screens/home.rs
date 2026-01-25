use crate::bot::models::{View};
use crate::bot::router::{AdminPayload, ControlPayload, Payload, RenderContext, SettingsPayload};

use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};


pub async fn render(ctx: RenderContext) -> Result<View> {
    let text = "Главное меню".to_string();
    let kb = make_keyboard(ctx.is_admin);

    Ok(View {
        notifications: ctx.notifications.clone(),
        text,
        kb,
        payload: Payload::Home {},
        ..Default::default()
    })
}

pub fn make_keyboard(root_admin:bool) -> InlineKeyboardMarkup {
    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            "🏠 Управление",
            Payload::Control(ControlPayload::ListRooms).to_string()
        )],

        vec![InlineKeyboardButton::callback(
            "⚙️ Настройки",
            Payload::Settings(SettingsPayload::ListRooms).to_string()
        )],
    ];

    if root_admin {
        rows.push(vec![
            InlineKeyboardButton::callback(
                "🛠 Админка",
                Payload::Admin(AdminPayload::ListActions).to_string()
            )
        ]);
    }
    InlineKeyboardMarkup::new(rows)
}