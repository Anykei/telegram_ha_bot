use anyhow::Context;
use chrono::Local;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::bot::models::View;
use crate::bot::router::{ControlPayload, DeviceCmd, Payload, RenderContext};
use crate::bot::State;
use crate::core::devices::{ChartParams, SmartDevice};
use crate::core::types::Device;
use crate::i18n::t;

// Константа глубины истории в HA (обычно 10 дней)
const MAX_HISTORY_DAYS: i32 = 10;
const MAX_BACK_HOURS: i32 = MAX_HISTORY_DAYS * 24;
pub(crate) const MAX_PERIOD_HOURS: u32 = MAX_BACK_HOURS as u32;

pub async fn render(
    ctx: RenderContext,
    room_id: i64,
    dev: Device,
    entity: crate::ha::models::Entity,
    cmd: DeviceCmd,
) -> anyhow::Result<View> {
    // 1. Извлекаем параметры из команды или ставим дефолт
    let params = match cmd {
        DeviceCmd::ShowChart { h, o } => ChartParams {
            period_hours: h,
            offset_hours: o,
        },
        _ => ChartParams {
            period_hours: 24,
            offset_hours: 0,
        },
    };

    anyhow::ensure!(
        (1..=MAX_PERIOD_HOURS).contains(&params.period_hours)
            && (-MAX_BACK_HOURS..=0).contains(&params.offset_hours),
        "Invalid graph interval"
    );

    crate::db::settings::set_string(
        &format!("ui:chart:{}:{}", ctx.user_id, dev.id),
        &Payload::Control(ControlPayload::QuickAction {
            room: room_id,
            device: dev.id,
            cmd: DeviceCmd::ShowChart {
                h: params.period_hours,
                o: params.offset_hours,
            },
        })
        .to_string(),
        &ctx.config.db,
    )
    .await?;

    // 2. Получаем историю
    let history = ctx
        .config
        .ha_client
        .fetch_history(&entity.entity_id, params.period_hours, params.offset_hours)
        .await?;

    // Период относится к графику, а не к уведомлениям о состоянии системы.
    let start_local = history.start_time.with_timezone(&Local);
    let end_local = history.end_time.with_timezone(&Local);

    // 4. Отрисовка графика
    let style = match SmartDevice::new(entity.clone()) {
        SmartDevice::Sensor(_) => crate::charts::ChartStyle::Numeric,
        SmartDevice::BinarySensor(_) => crate::charts::ChartStyle::Binary,
        _ => return Err(anyhow::anyhow!("Тип устройства не поддерживает графики")),
    };

    let device_name = super::labels::device_name(&dev, &entity, ctx.lang);
    let metric_name = if entity.entity_id.starts_with("sensor.")
        && entity.device_class.as_deref() == Some("battery")
    {
        format!("{} — {}", t(ctx.lang, "sensor.battery"), device_name)
    } else {
        device_name
    };
    let chart_title = match entity.attributes.unit_of_measurement.as_deref() {
        Some(unit) if !unit.trim().is_empty() => format!("{} ({})", metric_name, unit),
        _ => metric_name,
    };
    let no_data = !history.points.iter().any(|(_, state)| match style {
        crate::charts::ChartStyle::Numeric => state.parse::<f64>().is_ok_and(f64::is_finite),
        crate::charts::ChartStyle::Binary => matches!(state.as_str(), "on" | "off"),
    });
    let image_title = chart_title.clone();
    let image = if no_data {
        None
    } else {
        Some(
            tokio::task::spawn_blocking(move || {
                crate::charts::draw_ha_chart(
                    &history.points,
                    &image_title,
                    history.start_time,
                    history.end_time,
                    style,
                )
            })
            .await
            .context("Chart worker failed")?
            .context("Drawing failed")?,
        )
    };

    // 5. Логика кнопок навигации
    let mut nav_row = vec![];

    // Кнопка НАЗАД (только если не превысили лимит хранения)
    if (params.offset_hours.abs() + 24) <= MAX_BACK_HOURS {
        nav_row.push(InlineKeyboardButton::callback(
            "⏪ -24ч",
            Payload::Control(ControlPayload::QuickAction {
                room: room_id,
                device: dev.id,
                cmd: DeviceCmd::ShowChart {
                    h: params.period_hours,
                    o: params.offset_hours - 24,
                },
            })
            .to_string(),
        ));
    }

    nav_row.push(InlineKeyboardButton::callback(
        "🔄 Текущее",
        Payload::Control(ControlPayload::QuickAction {
            room: room_id,
            device: dev.id,
            cmd: DeviceCmd::ShowChart {
                h: params.period_hours,
                o: 0,
            },
        })
        .to_string(),
    ));

    // Кнопка ВПЕРЕД (только если мы в прошлом)
    if params.offset_hours < 0 {
        nav_row.push(InlineKeyboardButton::callback(
            "⏩ +24ч",
            Payload::Control(ControlPayload::QuickAction {
                room: room_id,
                device: dev.id,
                cmd: DeviceCmd::ShowChart {
                    h: params.period_hours,
                    o: (params.offset_hours + 24).min(0),
                },
            })
            .to_string(),
        ));
    }

    let mut rows = vec![nav_row];

    // 6. Интервалы (Пресеты)
    let intervals = [
        ("🕒 12ч", 12),
        ("🕒 24ч", 24),
        ("📅 3д", 72),
        ("📅 7д", 168),
    ];
    rows.push(
        intervals
            .iter()
            .map(|(label, h)| {
                InlineKeyboardButton::callback(
                    if *h == params.period_hours {
                        format!("✓ {}", label)
                    } else {
                        label.to_string()
                    },
                    Payload::Control(ControlPayload::QuickAction {
                        room: room_id,
                        device: dev.id,
                        cmd: DeviceCmd::ShowChart {
                            h: *h,
                            o: params.offset_hours,
                        },
                    })
                    .to_string(),
                )
            })
            .collect(),
    );

    // 7. Утилиты и Навигация
    rows.push(vec![InlineKeyboardButton::callback(
        "⌨️ Свой интервал",
        Payload::Control(ControlPayload::QuickAction {
            room: room_id,
            device: dev.id,
            cmd: DeviceCmd::EnterManualInput,
        })
        .to_string(),
    )]);

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Control(ControlPayload::RoomDetail { room: room_id }),
    )]);

    // 8. Текст описания
    let time_desc = if params.offset_hours == 0 {
        format!("за последние {}ч", params.period_hours)
    } else {
        let days_ago = params.offset_hours.abs() / 24;
        if days_ago > 0 {
            format!("за {}ч ({} дн. назад)", params.period_hours, days_ago)
        } else {
            format!(
                "за {}ч (сдвиг {}ч)",
                params.period_hours, params.offset_hours
            )
        }
    };

    let mut text = format!(
        "📊 {}\nПериод: {} — {}\nОтображение: {}\n\nВыберите масштаб или используйте навигацию:",
        chart_title,
        start_local.format("%d.%m.%Y %H:%M"),
        end_local.format("%d.%m.%Y %H:%M"),
        time_desc
    );

    if no_data {
        text.push_str(&format!("\n\n{}", t(ctx.lang, "sensor.empty")));
    }
    let inverted = crate::db::devices::is_state_inverted(&dev.entity_id, &ctx.config.db).await?;
    let alias = ctx
        .config
        .state_alias_for_display(&dev.entity_id, &entity.state, inverted);
    text.push_str(&format!(
        "\n{}: {}",
        t(ctx.lang, "control.details"),
        super::labels::state(&entity, ctx.lang, inverted, alias.as_deref())
    ));

    let current_state_payload = Payload::Control(ControlPayload::QuickAction {
        room: room_id,
        device: dev.id,
        cmd: DeviceCmd::ShowChart {
            h: params.period_hours,
            o: params.offset_hours,
        },
    });

    rows.insert(
        rows.len() - 1,
        vec![InlineKeyboardButton::callback(
            t(ctx.lang, "control.refresh"),
            current_state_payload.to_string(),
        )],
    );
    Ok(View {
        compact: true,
        image,
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_state_payload,
        lang: ctx.lang,
        ..Default::default()
    })
}

pub async fn saved_chart_payload(
    ctx: &RenderContext,
    room: i64,
    device: i64,
) -> anyhow::Result<Payload> {
    let saved = crate::db::settings::get_string(
        &format!("ui:chart:{}:{device}", ctx.user_id),
        &ctx.config.db,
    )
    .await?
    .and_then(|value| Payload::from_string(&value).ok());
    if let Some(Payload::Control(ControlPayload::QuickAction {
        device: saved_device,
        cmd: DeviceCmd::ShowChart { h, o },
        ..
    })) = saved
    {
        if saved_device == device
            && (1..=MAX_PERIOD_HOURS).contains(&h)
            && (-MAX_BACK_HOURS..=0).contains(&o)
        {
            return Ok(Payload::Control(ControlPayload::QuickAction {
                room,
                device,
                cmd: DeviceCmd::ShowChart { h, o },
            }));
        }
    }
    Ok(Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::ShowChart { h: 24, o: 0 },
    }))
}

pub fn render_manual_input(room_id: i64, device_id: i64, state: State) -> View {
    let input_payload = Payload::Control(ControlPayload::QuickAction {
        room: room_id,
        device: device_id,
        cmd: DeviceCmd::EnterManualInput,
    });
    let cancel_payload = Payload::Control(ControlPayload::QuickAction {
        room: room_id,
        device: device_id,
        cmd: DeviceCmd::ShowChart { h: 24, o: 0 },
    });

    View {
        compact: true,
        header: Some("⌨️ Ввод данных".into()),
        text: "Введите количество часов для отображения на графике: целое число от 1 до 240.\n\n\
               Например: 48 для двух суток."
            .into(),
        kb: InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            "❌ Отмена",
            cancel_payload.to_string(),
        )]]),
        payload: input_payload,
        next_state: Some(state),
        ..View::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_input_screen_keeps_stable_prompt_payload() {
        let view = render_manual_input(
            10,
            20,
            State::WaitingForGraphInterval {
                device_id: 20,
                room_id: 10,
            },
        );

        assert_eq!(
            view.payload,
            Payload::Control(ControlPayload::QuickAction {
                room: 10,
                device: 20,
                cmd: DeviceCmd::EnterManualInput,
            })
        );
        assert!(matches!(
            view.next_state,
            Some(State::WaitingForGraphInterval {
                device_id: 20,
                room_id: 10
            })
        ));
    }
}
