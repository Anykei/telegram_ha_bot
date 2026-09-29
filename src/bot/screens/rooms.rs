use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::bot::models::View;
use crate::bot::router::{ControlPayload, Payload, RenderContext, SettingsPayload};
pub(crate) use crate::core::types::RoomViewMode;
use crate::db;

pub async fn render(ctx: RenderContext, mode: RoomViewMode) -> Result<View> {
    render_page(ctx, mode, None).await
}

pub async fn render_page(
    ctx: RenderContext,
    mode: RoomViewMode,
    requested: Option<u16>,
) -> Result<View> {
    use super::common;
    let rooms = db::rooms::get_rooms_for_user(ctx.user_id, ctx.is_admin, &ctx.config.db).await?;

    let text = crate::core::presentation::StateFormatter::get_rooms_header(&mode);

    let current_payload = match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::ListRooms),
        RoomViewMode::Settings => ctx.settings_payload(SettingsPayload::ListRooms),
    };
    let (page, pages) = common::list_page(&ctx, &current_payload, requested, rooms.len()).await?;
    let empty = rooms.is_empty();
    let mut rows = vec![];
    for room in rooms
        .into_iter()
        .skip(page * common::LIST_PAGE_SIZE)
        .take(common::LIST_PAGE_SIZE)
    {
        let callback_payload = match mode {
            RoomViewMode::Control => Payload::Control(ControlPayload::RoomDetail { room: room.id }),
            RoomViewMode::Settings => {
                ctx.settings_payload(SettingsPayload::RoomDetail { room: room.id })
            }
        };

        rows.push(vec![InlineKeyboardButton::callback(
            format!("🏠 {}", room.display_name()),
            callback_payload.to_string(),
        )]);
    }

    let pager = common::page_row(page, pages, |page| match mode {
        RoomViewMode::Control => Payload::Control(ControlPayload::RoomsPage { page }),
        RoomViewMode::Settings => ctx.settings_payload(SettingsPayload::RoomsPage { page }),
    });
    if !pager.is_empty() {
        rows.push(pager);
    }
    rows.extend(common::navigation_rows(
        ctx.lang,
        current_payload.clone(),
        match mode {
            RoomViewMode::Control => Payload::Home,
            RoomViewMode::Settings => ctx.settings_root_back_payload(),
        },
    ));

    Ok(View {
        compact: mode == RoomViewMode::Control,
        notifications: ctx.notifications,
        header: Some(text.into()),
        text: if empty {
            crate::i18n::t(ctx.lang, "control.empty_rooms").into()
        } else {
            String::new()
        },
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_payload,
        ..Default::default()
    })
}
