use super::{
    models::View,
    router::{CommandPayload, Payload, RenderContext},
};
use crate::{
    core::commands::{ChoiceAction, ChoiceContext},
    db,
    i18n::Language,
};
use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: &RenderContext, id: i64, page: u32) -> Result<View> {
    let en = ctx.lang == Language::En;
    let payload = Payload::Command(CommandPayload::Choice { id, page });
    let record = db::command_choices::get(id, ctx.user_id, &ctx.config.db).await?;
    let mut rows = Vec::new();
    let mut text = if en {
        "Choice expired or completed. Repeat the command."
    } else {
        "Выбор истёк или завершён. Повторите команду."
    }
    .to_string();
    if let Some(record) =
        record.filter(|r| r.status == "pending" && r.expires_at > chrono::Utc::now().timestamp())
    {
        let context: ChoiceContext = serde_json::from_str(&record.context_json)?;
        let mut targets = Vec::new();
        for target in context.candidates {
            let (name, room) = if matches!(context.action, ChoiceAction::Device { .. }) {
                if !db::access::can_view_device(ctx.user_id, ctx.is_admin, target, &ctx.config.db)
                    .await?
                    || !db::access::can_control_device(
                        ctx.user_id,
                        ctx.is_admin,
                        target,
                        &ctx.config.db,
                    )
                    .await?
                {
                    continue;
                }
                let Some(device) = db::devices::get_device_by_id(target, &ctx.config.db).await?
                else {
                    continue;
                };
                ctx.track_device(device.id, &device.entity_id);
                let room =
                    db::devices::get_room_id_by_entity(&device.entity_id, &ctx.config.db).await?;
                (device.alias.unwrap_or(device.entity_id), room)
            } else {
                let Some(camera) = db::cameras::get_accessible_camera(
                    ctx.user_id,
                    ctx.is_admin,
                    target,
                    &ctx.config.db,
                )
                .await?
                else {
                    continue;
                };
                ctx.track_camera(camera.id);
                (camera.name, camera.room_id)
            };
            let room = if let Some(room) = room {
                db::rooms::get_room_by_id(room, &ctx.config.db)
                    .await?
                    .map(|r| {
                        ctx.track_room(r.id);
                        r.display_name().to_owned()
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };
            targets.push((target, format!("{room} · {name} · #{target}")));
        }
        let pages = targets.len().div_ceil(10).max(1);
        let page = (page as usize).min(pages - 1);
        text = if en {
            "Choose the intended target:"
        } else {
            "Уточните, какое устройство или камера нужны:"
        }
        .into();
        for (target, label) in targets.iter().skip(page * 10).take(10) {
            rows.push(vec![InlineKeyboardButton::callback(
                label,
                Payload::Command(CommandPayload::Choose {
                    id,
                    target: *target,
                })
                .to_string(),
            )]);
        }
        let mut pager = Vec::new();
        if page > 0 {
            pager.push(InlineKeyboardButton::callback(
                "‹",
                Payload::Command(CommandPayload::Choice {
                    id,
                    page: (page - 1) as u32,
                })
                .to_string(),
            ));
        }
        if page + 1 < pages {
            pager.push(InlineKeyboardButton::callback(
                "›",
                Payload::Command(CommandPayload::Choice {
                    id,
                    page: (page + 1) as u32,
                })
                .to_string(),
            ));
        }
        if !pager.is_empty() {
            rows.push(pager);
        }
        rows.push(vec![InlineKeyboardButton::callback(
            if en { "Cancel" } else { "Отмена" },
            Payload::Command(CommandPayload::CancelChoice { id }).to_string(),
        )]);
        rows.push(vec![super::screens::common::back_button_lang(
            ctx.lang,
            context.back,
        )]);
    }
    rows.push(vec![super::screens::common::main_menu_button_lang(
        ctx.lang,
    )]);
    Ok(View {
        compact: true,
        text,
        payload,
        kb: InlineKeyboardMarkup::new(rows),
        lang: ctx.lang,
        ..Default::default()
    })
}

pub async fn result(ctx: &RenderContext, id: i64) -> Result<View> {
    let text = crate::core::commands::pending_result_text_tracked(
        ctx.user_id,
        id,
        &ctx.config,
        |source| ctx.track_source(source),
    )
    .await?;
    let mut rows = Vec::new();
    let mut active = false;
    if let Some(pending) =
        db::pending_commands::get_pending(id, ctx.user_id, &ctx.config.db).await?
    {
        if pending.status == "pending" && pending.expires_at > chrono::Utc::now() {
            active = true;
            rows.push(vec![
                InlineKeyboardButton::callback(
                    crate::i18n::t(ctx.lang, "confirm.yes"),
                    Payload::Command(CommandPayload::Confirm { id }).to_string(),
                ),
                InlineKeyboardButton::callback(
                    crate::i18n::t(ctx.lang, "confirm.cancel"),
                    Payload::Command(CommandPayload::Cancel { id }).to_string(),
                ),
            ]);
        }
        if let Some(json) = pending.result_json {
            let result: crate::core::action_results::ActionResult = serde_json::from_str(&json)?;
            for target in result.targets {
                if target.kind != crate::core::action_results::TargetKind::Device {
                    continue;
                }
                let Some(name) = crate::core::action_results::target_label_tracked(
                    &target,
                    ctx.user_id,
                    &ctx.config,
                    |source| ctx.track_source(source),
                )
                .await?
                else {
                    continue;
                };
                let Some(device) = db::devices::get_device_by_id(target.id, &ctx.config.db).await?
                else {
                    continue;
                };
                ctx.track_device(device.id, &device.entity_id);
                let Some(room) =
                    db::devices::get_room_id_by_entity(&device.entity_id, &ctx.config.db).await?
                else {
                    continue;
                };
                rows.push(vec![InlineKeyboardButton::callback(
                    name,
                    Payload::Control(super::router::ControlPayload::DeviceControl {
                        room,
                        device: target.id,
                    })
                    .to_string(),
                )]);
            }
        }
    }
    rows.push(vec![InlineKeyboardButton::callback(
        if ctx.lang == Language::En {
            "Check states"
        } else {
            "Проверить состояния"
        },
        Payload::Control(super::router::ControlPayload::ListRooms).to_string(),
    )]);
    rows.push(vec![super::screens::common::main_menu_button_lang(
        ctx.lang,
    )]);
    Ok(View {
        compact: true,
        html_back: Some(if active {
            Payload::Command(CommandPayload::Cancel { id })
        } else {
            Payload::Home
        }),
        text,
        payload: Payload::Command(CommandPayload::Result { id }),
        kb: InlineKeyboardMarkup::new(rows),
        lang: ctx.lang,
        ..Default::default()
    })
}
