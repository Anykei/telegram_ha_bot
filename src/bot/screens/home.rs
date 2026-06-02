use crate::bot::models::View;
use crate::bot::router::{
    AdminPayload, CameraPayload, ControlPayload, Payload, RenderContext, SettingsPayload,
};
use crate::i18n::{t, Language};

use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext) -> Result<View> {
    let text = t(ctx.lang, "home.title").to_string();
    let kb = make_keyboard(ctx.is_admin, ctx.lang);

    Ok(View {
        notifications: ctx.notifications.clone(),
        text,
        kb,
        payload: Payload::Home {},
        ..Default::default()
    })
}

pub fn make_keyboard(root_admin: bool, lang: Language) -> InlineKeyboardMarkup {
    let mut rows = vec![vec![InlineKeyboardButton::callback(
        t(lang, "home.control"),
        Payload::Control(ControlPayload::ListRooms).to_string(),
    )]];

    rows.push(vec![InlineKeyboardButton::callback(
        t(lang, "home.cameras"),
        Payload::Camera(CameraPayload::ListCameras).to_string(),
    )]);

    if root_admin {
        rows.push(vec![InlineKeyboardButton::callback(
            t(lang, "home.settings"),
            Payload::Settings(SettingsPayload::ListRooms).to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            t(lang, "home.admin"),
            Payload::Admin(AdminPayload::ListActions).to_string(),
        )]);
    }
    InlineKeyboardMarkup::new(rows)
}
