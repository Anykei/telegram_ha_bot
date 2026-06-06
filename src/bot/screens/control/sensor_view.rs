use anyhow::Context;
use chrono::Local;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::bot::models::View;
use crate::bot::router::{ControlPayload, DeviceCmd, Payload, RenderContext};
use crate::bot::State;
use crate::core::devices::{ChartParams, SmartDevice};
use crate::core::types::Device;
use crate::core::HeaderItem;

// Константа глубины истории в HA (обычно 10 дней)
const MAX_HISTORY_DAYS: i32 = 10;
const MAX_BACK_HOURS: i32 = MAX_HISTORY_DAYS * 24;

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

    // 2. Получаем историю
    let history = ctx
        .config
        .ha_client
        .fetch_history(&entity.entity_id, params.period_hours, params.offset_hours)
        .await?;

    // 3. Формируем расширенную шапку (Status Bar)
    let mut notifications = ctx.notifications.clone();

    // Добавляем информацию о временном диапазоне в начало списка уведомлений
    let start_local = history.start_time.with_timezone(&Local);
    let end_local = history.end_time.with_timezone(&Local);

    notifications.insert(
        0,
        HeaderItem {
            icon: "📅".into(),
            label: "Период".into(),
            value: crate::bot::utils::md::code(&format!(
                "{} — {}",
                start_local.format("%d.%m %H:%M"),
                end_local.format("%H:%M")
            )),
            last_update: chrono::Utc::now(),
        },
    );

    // 4. Отрисовка графика
    let style = match SmartDevice::new(entity.clone()) {
        SmartDevice::Sensor(_) => crate::charts::ChartStyle::Numeric,
        SmartDevice::BinarySensor(_) => crate::charts::ChartStyle::Binary,
        _ => return Err(anyhow::anyhow!("Тип устройства не поддерживает графики")),
    };

    let device_name = dev.alias.as_deref().unwrap_or(&entity.entity_id);
    let image = crate::charts::draw_ha_chart(
        &history.points,
        device_name,
        history.start_time,
        history.end_time,
        style,
    )
    .context("Drawing failed")?;

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
            cmd: DeviceCmd::ShowChart { h: 24, o: 0 },
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
                    *label,
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

    let text = format!(
        "📊 *{}*\nОтображение: {}\n\nВыберите масштаб или используйте навигацию:",
        device_name, time_desc
    );

    let current_state_payload = Payload::Control(ControlPayload::QuickAction {
        room: room_id,
        device: dev.id,
        cmd: DeviceCmd::ShowChart {
            h: params.period_hours,
            o: params.offset_hours,
        },
    });

    Ok(View {
        image: Some(image),
        notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_state_payload,
        ..Default::default()
    })
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
        header: Some("⌨️ Ввод данных".into()),
        text: "Введите количество часов для отображения на графике (целое число).\n\n\
               Например: `48` для двух суток."
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
