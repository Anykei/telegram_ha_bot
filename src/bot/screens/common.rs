use crate::bot::models::View;
use crate::bot::router::{Payload, RenderContext};
use crate::i18n::t;
use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn in_dev_menu(ctx: RenderContext, back: Payload) -> Result<View> {
    let rows = vec![
        vec![back_button_lang(ctx.lang, back)],
        vec![main_menu_button_lang(ctx.lang)],
    ];
    let kb = InlineKeyboardMarkup::new(rows);
    let text = t(ctx.lang, "common.in_development").to_string();

    Ok(View {
        notifications: ctx.notifications.clone(),
        text,
        kb,
        payload: Payload::InDev {},
        ..Default::default()
    })
}

pub fn back_button(to: Payload) -> InlineKeyboardButton {
    back_button_lang(crate::i18n::Language::Ru, to)
}

pub fn back_button_lang(lang: crate::i18n::Language, to: Payload) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(t(lang, "common.back"), to.to_string())
}

pub fn main_menu_button_lang(lang: crate::i18n::Language) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(t(lang, "common.main_menu"), Payload::Home {}.to_string())
}
