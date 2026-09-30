use crate::bot::models::View;
use crate::bot::router::{AdminPayload, CameraPayload, ControlPayload, Payload, RenderContext};
use crate::i18n::{t, Language};

use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext) -> Result<View> {
    let mut text = t(ctx.lang, "home.title").to_string();
    let mut kb = make_keyboard(ctx.is_admin, ctx.lang);
    let (favorites, _, _) = crate::bot::personal::favorite_buttons(&ctx, 0, 4).await?;
    if favorites.is_empty() {
        text.push_str(crate::bot::personal::words(
            ctx.lang,
            "\nДобавьте избранное из карточки устройства, камеры или сценария.",
            "\nAdd favorites from a device, camera or scenario card.",
        ));
    } else {
        let mut rows: Vec<_> = favorites.into_iter().map(|(_, b)| vec![b]).collect();
        rows.push(vec![InlineKeyboardButton::callback(
            crate::bot::personal::words(ctx.lang, "Всё избранное", "All favorites"),
            Payload::Personal(crate::bot::personal::PersonalPayload::Favorites { page: 0 })
                .to_string(),
        )]);
        rows.extend(kb.inline_keyboard);
        kb.inline_keyboard = rows;
    }

    Ok(View {
        compact: true,
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
        t(lang, "home.action_groups"),
        Payload::Control(ControlPayload::ActionGroups).to_string(),
    )]);

    rows.push(vec![InlineKeyboardButton::callback(
        t(lang, "home.cameras"),
        Payload::Camera(CameraPayload::ListCameras).to_string(),
    )]);

    rows.push(vec![InlineKeyboardButton::callback(
        crate::bot::personal::words(lang, "Мои настройки", "My settings"),
        Payload::Personal(crate::bot::personal::PersonalPayload::Settings).to_string(),
    )]);
    rows.push(vec![InlineKeyboardButton::callback(
        crate::bot::personal::words(lang, "Помощь", "Help"),
        Payload::Help.to_string(),
    )]);
    if root_admin {
        rows.push(vec![InlineKeyboardButton::callback(
            t(lang, "home.admin"),
            Payload::Admin(AdminPayload::ListActions).to_string(),
        )]);
    }
    InlineKeyboardMarkup::new(rows)
}
