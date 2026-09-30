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

pub fn back_button_lang(lang: crate::i18n::Language, to: Payload) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(t(lang, "common.back"), to.to_string())
}

pub fn main_menu_button_lang(lang: crate::i18n::Language) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(t(lang, "common.main_menu"), Payload::Home {}.to_string())
}

pub const LIST_PAGE_SIZE: usize = 10;

/// Scope by user and list (including settings origin), so Back restores its page.
pub async fn list_page(
    ctx: &RenderContext,
    list: &Payload,
    requested: Option<u16>,
    count: usize,
) -> Result<(usize, usize)> {
    let key = format!("ui:list:{}:{}", ctx.user_id, list.to_string());
    let saved = crate::db::settings::get_i64(&key, &ctx.config.db)
        .await?
        .unwrap_or(0);
    let pages = count.div_ceil(LIST_PAGE_SIZE).max(1);
    let page = requested
        .map(usize::from)
        .unwrap_or(saved.max(0) as usize)
        .min(pages - 1);
    if requested.is_some() {
        crate::db::settings::set_i64(&key, page as i64, &ctx.config.db).await?;
    }
    Ok((page, pages))
}

pub fn page_row(
    page: usize,
    pages: usize,
    payload: impl Fn(u16) -> Payload,
) -> Vec<InlineKeyboardButton> {
    if pages <= 1 {
        return vec![];
    }
    let mut row = Vec::new();
    if page > 0 {
        row.push(InlineKeyboardButton::callback(
            "◀",
            payload((page - 1) as u16).to_string(),
        ));
    }
    row.push(InlineKeyboardButton::callback(
        format!("{} / {}", page + 1, pages),
        payload(page as u16).to_string(),
    ));
    if page + 1 < pages {
        row.push(InlineKeyboardButton::callback(
            "▶",
            payload((page + 1) as u16).to_string(),
        ));
    }
    row
}

pub fn navigation_rows(
    lang: crate::i18n::Language,
    refresh: Payload,
    back: Payload,
) -> Vec<Vec<InlineKeyboardButton>> {
    vec![
        vec![InlineKeyboardButton::callback(
            t(lang, "control.refresh"),
            refresh.to_string(),
        )],
        vec![back_button_lang(lang, back), main_menu_button_lang(lang)],
    ]
}
