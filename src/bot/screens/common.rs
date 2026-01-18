use crate::bot::models::View;
use crate::bot::router::{Payload, RenderContext};
use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn default_menu(ctx: RenderContext) -> Result<View> {
    let rows = vec![
        vec![main_menu_button()]
    ];

    let kb = InlineKeyboardMarkup::new(rows);
    let text= "В разработке".to_string();

    Ok(View {
        notifications:ctx.notifications.clone(),
        text,
        kb,
        payload: Payload::Home {},
        ..Default::default()
    })
}

pub fn back_button(to: Payload) -> InlineKeyboardButton {
    InlineKeyboardButton::callback("⬅️ Назад", to.to_string())
}

pub fn close_button() -> InlineKeyboardButton {
    InlineKeyboardButton::callback("❌ Закрыть", "del_msg")
}

pub fn main_menu_button() -> InlineKeyboardButton {
    InlineKeyboardButton::callback("🏠 В главное меню", Payload::Home {}.to_string())
}