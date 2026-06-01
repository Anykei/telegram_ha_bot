use crate::bot::models::View;
use crate::bot::router::{Payload, RenderContext};
use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn in_dev_menu(ctx: RenderContext, back: Payload) -> Result<View> {
    let rows = vec![vec![back_button(back)], vec![main_menu_button()]];
    let kb = InlineKeyboardMarkup::new(rows);
    let text = "В разработке".to_string();

    Ok(View {
        notifications: ctx.notifications.clone(),
        text,
        kb,
        payload: Payload::InDev {},
        ..Default::default()
    })
}

pub fn back_button(to: Payload) -> InlineKeyboardButton {
    InlineKeyboardButton::callback("⬅️ Назад", to.to_string())
}

pub fn main_menu_button() -> InlineKeyboardButton {
    InlineKeyboardButton::callback("🏠 В главное меню", Payload::Home {}.to_string())
}
