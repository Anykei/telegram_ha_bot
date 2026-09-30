use crate::bot::models::View;
use crate::bot::router::{DeviceCmd, Payload, RenderContext};
use anyhow::Context;

use crate::core::devices::SmartDevice;

pub async fn render(
    ctx: RenderContext,
    room_id: i64,
    device_id: i64,
    cmd: DeviceCmd, // Команда пробрасывается из роутера
) -> anyhow::Result<View> {
    let db = &ctx.config.db;

    // 1. Загружаем данные (как ты и делал)
    let dev_db = crate::db::devices::get_device_by_id(device_id, db)
        .await?
        .context("Device not found")?;
    let ha_ent = ctx
        .config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&dev_db.entity_id))
        .await?
        .into_iter()
        .next()
        .context("HA state missing")?;

    let room_id = crate::db::devices::get_room_id_by_entity(&dev_db.entity_id, db)
        .await?
        .unwrap_or(room_id);
    let room_name = crate::db::rooms::get_room_by_id(room_id, db)
        .await?
        .map(|r| r.display_name().to_string())
        .unwrap_or_default();
    let lang = ctx.lang;
    let smart_obj = SmartDevice::new(ha_ent.clone());

    // 2. Диспетчеризация отрисовки
    let mut view = match smart_obj {
        SmartDevice::Sensor(e) | SmartDevice::BinarySensor(e) => {
            super::sensor_view::render(ctx, room_id, dev_db, e, cmd).await
        }
        SmartDevice::Climate(e) => super::climate_view::render(ctx, room_id, dev_db, e).await,
        _ => {
            use crate::bot::router::ControlPayload;
            use crate::i18n::t;
            use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
            let inverted = crate::db::devices::is_state_inverted(&dev_db.entity_id, db).await?;
            let state_alias =
                ctx.config
                    .state_alias_for_display(&dev_db.entity_id, &ha_ent.state, inverted);
            let can_control =
                crate::db::access::can_control_device(ctx.user_id, ctx.is_admin, device_id, db)
                    .await?;
            let domain = dev_db.entity_id.split('.').next().unwrap_or("");
            let detail = if !can_control {
                t(ctx.lang, "control.view_only")
            } else if matches!(domain, "light" | "switch") {
                ""
            } else {
                t(ctx.lang, "control.unsupported")
            };
            let payload = Payload::Control(ControlPayload::DeviceControl {
                room: room_id,
                device: device_id,
            });
            Ok(View {
                compact: true,
                text: format!(
                    "{}\n{}: {}\n\n{}",
                    super::labels::device_name(&dev_db, &ha_ent, ctx.lang),
                    t(ctx.lang, "control.details"),
                    super::labels::state(&ha_ent, ctx.lang, inverted, state_alias.as_deref()),
                    detail
                ),
                notifications: ctx.notifications,
                kb: InlineKeyboardMarkup::new(vec![
                    vec![InlineKeyboardButton::callback(
                        t(ctx.lang, "control.refresh"),
                        payload.to_string(),
                    )],
                    vec![crate::bot::screens::common::back_button_lang(
                        ctx.lang,
                        Payload::Control(ControlPayload::RoomDetail { room: room_id }),
                    )],
                ]),
                payload,
                lang: ctx.lang,
                ..Default::default()
            })
        }
    }?;
    view.header = Some(match view.header {
        Some(title) => format!("{room_name} · {title}"),
        None => room_name,
    });
    view.kb.inline_keyboard.insert(
        0,
        vec![teloxide::types::InlineKeyboardButton::callback(
            crate::bot::personal::words(lang, "Подробности", "Details"),
            Payload::DeviceDetails {
                room: room_id,
                device: device_id,
            }
            .to_string(),
        )],
    );
    if !view.kb.inline_keyboard.iter().flatten().any(|button| matches!(&button.kind,
        teloxide::types::InlineKeyboardButtonKind::CallbackData(data) if data == &Payload::Home.to_string())) {
        if let Some(row) = view.kb.inline_keyboard.last_mut() {
            row.push(crate::bot::screens::common::main_menu_button_lang(lang));
        }
    }
    Ok(view)
}
