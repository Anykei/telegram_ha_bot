use crate::bot::models::View;
use crate::bot::router::{ControlPayload, DeviceCmd, Payload, RenderContext};
use crate::core::types::Device;
use crate::ha::models::Entity;
use crate::i18n::{t, Language};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(
    ctx: RenderContext,
    room_id: i64,
    dev: Device,
    entity: Entity,
) -> anyhow::Result<View> {
    ctx.track_device(dev.id, &dev.entity_id);
    let can_control =
        crate::db::access::can_control_device(ctx.user_id, ctx.is_admin, dev.id, &ctx.config.db)
            .await?;
    let mut view = render_view(
        ctx.lang,
        ctx.notifications,
        room_id,
        dev,
        entity.clone(),
        can_control,
    );
    if can_control && crate::bot::html_ui::preferred(&ctx.config, ctx.user_id).await {
        crate::bot::html_ui::temperature::attach(&mut view, &entity);
    }
    Ok(view)
}

fn render_view(
    lang: Language,
    notifications: Vec<crate::core::HeaderItem>,
    room_id: i64,
    dev: Device,
    entity: Entity,
    can_control: bool,
) -> View {
    let unit = entity
        .attributes
        .unit_of_measurement
        .as_deref()
        .unwrap_or("°");
    let temperature = |value: Option<f32>| {
        value
            .filter(|v| v.is_finite())
            .map(|v| format!("{}{}", display_number(v), unit))
            .unwrap_or_else(|| t(lang, "climate.no_data").to_string())
    };
    let target_text = if let (Some(low), Some(high)) = (
        entity.attributes.target_temp_low,
        entity.attributes.target_temp_high,
    ) {
        if entity.state == "heat_cool" || entity.attributes.temperature.is_none() {
            format!("{} – {}", temperature(Some(low)), temperature(Some(high)))
        } else {
            temperature(entity.attributes.temperature)
        }
    } else {
        temperature(entity.attributes.temperature)
    };
    let summary = format!(
        "{}: {}\n{}: {}\n{}: {}",
        t(lang, "climate.current"),
        temperature(entity.attributes.current_temperature),
        t(lang, "climate.target"),
        target_text,
        t(lang, "climate.mode"),
        mode_label(lang, &entity.state),
    );
    let mut text = format!(
        "{}\n\n{summary}",
        super::labels::device_name(&dev, &entity, lang)
    );
    let mut limits = String::new();
    if let (Some(min), Some(max)) = (entity.attributes.min_temp, entity.attributes.max_temp) {
        if min.is_finite() && max.is_finite() {
            limits = format!("{} – {}", temperature(Some(min)), temperature(Some(max)));
            text.push_str(&format!(
                "\n{}: {} – {}",
                t(lang, "climate.limits"),
                temperature(Some(min)),
                temperature(Some(max))
            ));
        }
    }
    let step = entity
        .attributes
        .target_temp_step
        .filter(|step| step.is_finite() && *step > 0.0)
        .unwrap_or(0.5);
    text.push_str(&format!(
        "\n{}: {}{}",
        t(lang, "climate.step"),
        display_number(step),
        unit
    ));
    let mut rows = Vec::new();
    let mut html_button_labels = std::collections::HashMap::new();
    let mut html_summary = String::new();
    if can_control {
        let step = entity
            .attributes
            .target_temp_step
            .filter(|step| step.is_finite() && *step > 0.0)
            .unwrap_or(0.5);
        let mut controls = Vec::new();
        if let Some(current_target) = entity.attributes.temperature {
            for (icon, direction) in [("➖", -1), ("➕", 1)] {
                let target =
                    ((current_target + f32::from(direction) * step) * 1000.0).round() / 1000.0;
                if entity.temperature_target_is_valid(target) && target != current_target {
                    let callback = Payload::Control(ControlPayload::QuickAction {
                        room: room_id,
                        device: dev.id,
                        cmd: DeviceCmd::AdjustTemp(direction),
                    })
                    .to_string();
                    html_button_labels.insert(
                        callback.clone(),
                        format!(
                            "{} {}{}",
                            if direction < 0 { "−" } else { "+" },
                            display_number(step),
                            unit
                        ),
                    );
                    controls.push(InlineKeyboardButton::callback(
                        format!("{} {}{}", icon, display_number(step), unit),
                        callback,
                    ));
                }
            }
        }
        if controls.is_empty() {
            text.push_str(&format!("\n\n{}", t(lang, "climate.adjust_unavailable")));
            html_summary = crate::bot::personal::words(
                lang,
                "Изменение температуры недоступно. Обновите данные или откройте подробности.",
                "Temperature adjustment unavailable. Refresh or open details.",
            )
            .into();
        } else {
            text.push_str(&format!("\n\n{}", t(lang, "climate.choose_target")));
            rows.push(controls);
        }
    } else {
        text.push_str(&format!("\n\n{}", t(lang, "control.view_only")));
        html_summary = t(lang, "control.view_only").into();
    }
    if can_control
        && entity
            .attributes
            .temperature
            .is_some_and(|v| entity.temperature_target_is_valid(v))
    {
        rows.push(vec![InlineKeyboardButton::callback(
            crate::bot::personal::words(lang, "Задать температуру", "Set temperature"),
            Payload::TemperatureInput {
                room: room_id,
                device: dev.id,
            }
            .to_string(),
        )]);
    }
    let payload = Payload::Control(ControlPayload::DeviceControl {
        room: room_id,
        device: dev.id,
    });
    let turn_on = entity.state == "off";
    if can_control && entity.supports_hvac_mode(if turn_on { "heat" } else { "off" }) {
        rows.push(vec![InlineKeyboardButton::callback(
            crate::bot::personal::words(
                lang,
                if turn_on {
                    "Включить отопление"
                } else {
                    "Выключить отопление"
                },
                if turn_on {
                    "Enable heating"
                } else {
                    "Turn off heating"
                },
            ),
            Payload::Control(ControlPayload::QuickAction {
                room: room_id,
                device: dev.id,
                cmd: if turn_on {
                    DeviceCmd::TurnOn
                } else {
                    DeviceCmd::TurnOff
                },
            })
            .to_string(),
        )]);
    }
    rows.push(vec![InlineKeyboardButton::callback(
        t(lang, "control.refresh"),
        payload.to_string(),
    )]);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        lang,
        Payload::Control(ControlPayload::RoomDetail { room: room_id }),
    )]);
    View {
        compact: true,
        html_kind: Some("climate".into()),
        html_button_labels,
        html_summary: Some(html_summary),
        html_metrics: vec![
            [
                crate::bot::personal::words(lang, "Задано", "Target").into(),
                target_text,
                format!(
                    "{}{}{}: {}{}",
                    limits,
                    if limits.is_empty() { "" } else { " · " },
                    t(lang, "climate.step"),
                    display_number(step),
                    unit,
                ),
            ],
            [
                crate::bot::personal::words(lang, "Сейчас", "Current").into(),
                temperature(
                    if matches!(entity.state.as_str(), "unavailable" | "unknown") {
                        None
                    } else {
                        entity.attributes.current_temperature
                    },
                ),
                format!(
                    "{}: {}",
                    t(lang, "climate.mode"),
                    mode_label(lang, &entity.state)
                ),
            ],
        ],
        header: Some(t(lang, "climate.title").into()),
        notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload,
        lang,
        ..Default::default()
    }
}

fn display_number(value: f32) -> String {
    format!("{value:.3}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

pub fn command_notice(
    lang: Language,
    result: &crate::core::climate_control::TemperatureResult,
) -> String {
    format!(
        "{}: {}{}",
        t(lang, "climate.requested"),
        display_number(result.target),
        result.unit
    )
}

pub(super) fn mode_label(lang: Language, state: &str) -> &str {
    let key = match state {
        "heat" => "climate.heat",
        "cool" => "climate.cool",
        "heat_cool" => "climate.heat_cool",
        "auto" => "climate.auto",
        "off" => "climate.off",
        "dry" => "climate.dry",
        "fan_only" => "climate.fan",
        "unknown" | "unavailable" => "climate.no_data",
        _ => return state,
    };
    t(lang, key)
}
