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
    ctx.track_device(dev_db.id, &dev_db.entity_id);
    let read_started = std::time::Instant::now();
    let ha_ent = ctx
        .config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&dev_db.entity_id))
        .await?
        .into_iter()
        .next()
        .context("HA state missing")?;
    ctx.config.html_ui.pending.observe_states(
        ctx.user_id,
        &ctx.admission_generation,
        read_started,
        std::slice::from_ref(&ha_ent),
    );

    let room_id = crate::db::devices::get_room_id_by_entity(&dev_db.entity_id, db)
        .await?
        .unwrap_or(room_id);
    let room_name = crate::db::rooms::get_room_by_id(room_id, db)
        .await?
        .map(|r| {
            ctx.track_room(r.id);
            r.display_name().to_string()
        })
        .unwrap_or_default();
    let lang = ctx.lang;
    let device_name = super::labels::device_name(&dev_db, &ha_ent, ctx.lang);
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
            let mut rows = vec![];
            if can_control
                && matches!(domain, "light" | "switch")
                && matches!(ha_ent.state.as_str(), "on" | "off")
            {
                let logical_on = (ha_ent.state == "on") != inverted;
                rows.push(vec![InlineKeyboardButton::callback(
                    t(
                        ctx.lang,
                        if logical_on {
                            "control.turn_off"
                        } else {
                            "control.turn_on"
                        },
                    ),
                    Payload::Control(ControlPayload::QuickAction {
                        room: room_id,
                        device: device_id,
                        cmd: if ha_ent.state == "on" {
                            DeviceCmd::TurnOff
                        } else {
                            DeviceCmd::TurnOn
                        },
                    })
                    .to_string(),
                )]);
            }
            rows.push(vec![InlineKeyboardButton::callback(
                t(ctx.lang, "control.refresh"),
                payload.to_string(),
            )]);
            rows.push(vec![crate::bot::screens::common::back_button_lang(
                ctx.lang,
                Payload::Control(ControlPayload::RoomDetail { room: room_id }),
            )]);
            let text = format!(
                "{}: {}\n\n{}",
                crate::bot::personal::words(ctx.lang, "Состояние", "State"),
                super::labels::state(&ha_ent, ctx.lang, inverted, state_alias.as_deref()),
                detail
            );
            Ok(View {
                compact: true,
                header: Some(device_name.clone()),
                html_summary: Some(text.trim().to_owned()),
                text,
                notifications: ctx.notifications,
                kb: InlineKeyboardMarkup::new(rows),
                payload,
                lang: ctx.lang,
                ..Default::default()
            })
        }
    }?;
    view.html_title.get_or_insert(device_name);
    view.html_context = Some(room_name.clone());
    view.header = Some(match view.header {
        Some(title) => format!("{room_name} · {title}"),
        None => room_name,
    });
    let details_index = if view.html_chart.is_some() {
        view.kb.inline_keyboard.len().saturating_sub(1)
    } else {
        usize::from(view.kb.inline_keyboard.first().is_some_and(|row| row.iter().any(|button| {
        matches!(&button.kind, teloxide::types::InlineKeyboardButtonKind::CallbackData(data) if matches!(Payload::from_string(data), Ok(Payload::Control(crate::bot::router::ControlPayload::QuickAction {..}))))
    })))
    };
    view.kb.inline_keyboard.insert(
        details_index,
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
