use crate::bot::models::{HtmlSection, View};
use crate::bot::personal::{words, PersonalPayload};
use crate::bot::router::{AdminPayload, CameraPayload, ControlPayload, Payload, RenderContext};
use crate::i18n::{t, Language};

use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext) -> Result<View> {
    let title = words(ctx.lang, "Мой дом", "My home").to_string();
    let mut kb = make_keyboard(ctx.is_admin, ctx.lang);
    let mut html_sections = Vec::new();
    let (favorites, _, _) = crate::bot::personal::favorite_buttons(&ctx, 0, 4).await?;
    if !favorites.is_empty() {
        html_sections.push(HtmlSection {
            start_row: kb.inline_keyboard.len(),
            title: words(ctx.lang, "Избранное", "Favorites").into(),
        });
        kb.inline_keyboard
            .extend(favorites.into_iter().map(|(_, button)| vec![button]));
    }

    Ok(View {
        compact: true,
        header: Some(title.clone()),
        html_title: Some(title),
        html_summary: Some(String::new()),
        html_sections,
        html_details_title: Some(words(ctx.lang, "Сводка дома", "Home overview").into()),
        notifications: ctx.notifications.clone(),
        kb,
        payload: Payload::Home {},
        ..Default::default()
    })
}

pub fn make_keyboard(root_admin: bool, lang: Language) -> InlineKeyboardMarkup {
    let mut rows = vec![
        vec![
            InlineKeyboardButton::callback(
                t(lang, "home.control"),
                Payload::Control(ControlPayload::ListRooms).to_string(),
            ),
            InlineKeyboardButton::callback(
                t(lang, "home.action_groups"),
                Payload::Control(ControlPayload::ActionGroups).to_string(),
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                t(lang, "home.cameras"),
                Payload::Camera(CameraPayload::ListCameras).to_string(),
            ),
            InlineKeyboardButton::callback(
                words(lang, "Избранное", "Favorites"),
                Payload::Personal(PersonalPayload::Favorites { page: 0 }).to_string(),
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                words(lang, "Мои настройки", "My settings"),
                Payload::Personal(PersonalPayload::Settings).to_string(),
            ),
            InlineKeyboardButton::callback(
                words(lang, "Помощь", "Help"),
                Payload::Help.to_string(),
            ),
        ],
    ];
    if root_admin {
        rows.push(vec![InlineKeyboardButton::callback(
            t(lang, "home.admin"),
            Payload::Admin(AdminPayload::ListActions).to_string(),
        )]);
    }
    InlineKeyboardMarkup::new(rows)
}
