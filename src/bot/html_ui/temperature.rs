//! Experimental temperature editor. Dragging is local; Apply uses the normal
//! temperature command queue, access checks and HA-state acknowledgement.
use crate::bot::{
    models::View,
    personal::words,
    router::{ControlPayload, DeviceCmd, Payload, RenderContext},
};
use crate::{db, ha::models::Entity};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use teloxide::types::{InlineKeyboardButton as Button, InlineKeyboardMarkup};

/// Integer thousandths keep the wire value, snapping and command consistent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Slider {
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub value: i32,
    pub unit: String,
}
impl Slider {
    pub fn from_entity(entity: &Entity) -> Option<Self> {
        fn scaled(value: f32) -> Option<i32> {
            let value = (f64::from(value) * 1000.0).round();
            (value.is_finite() && value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX))
                .then_some(value as i32)
        }
        let a = &entity.attributes;
        let target = a.temperature?;
        if !entity.temperature_target_is_valid(target) {
            return None;
        }
        let slider = Self {
            min: scaled(a.min_temp?)?,
            max: scaled(a.max_temp?)?,
            step: scaled(
                a.target_temp_step
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .unwrap_or(0.5),
            )?,
            value: scaled(target)?,
            unit: a.unit_of_measurement.clone().unwrap_or("°".into()),
        };
        (slider.min < slider.max
            && slider.step > 0
            && i64::from(slider.step) <= i64::from(slider.max) - i64::from(slider.min))
        .then_some(slider)
    }
    fn accepts(&self, value: i32) -> bool {
        value >= self.min
            && value <= self.max
            && self.step > 0
            && (i64::from(value) - i64::from(self.min)) % i64::from(self.step) == 0
    }
}

pub fn route(room: i64, device: i64) -> Payload {
    Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::EditTemperature,
    })
}

pub(super) fn resolve(
    view: &View,
    actions: &HashMap<String, Payload>,
    key: &str,
) -> Option<Payload> {
    let slider = view.html_temperature.as_ref()?;
    let value: i32 = key.strip_prefix('s')?.parse().ok()?;
    if key != format!("s{value}") || !slider.accepts(value) {
        return None;
    }
    let Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::EditTemperature,
    }) = view.payload
    else {
        return None;
    };
    // No generic service parameters: only the same device's server-issued action.
    actions.values().find(|p| matches!(p,
        Payload::Control(ControlPayload::QuickAction {room: r, device: d, cmd: DeviceCmd::SetTemp(_)})
        if *r == room && *d == device))?;
    Some(Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::SetTemp(value as f32 / 1000.0),
    }))
}

pub async fn render(ctx: RenderContext, room: i64, device: i64) -> Result<View> {
    let dev = db::devices::get_device_by_id(device, &ctx.config.db)
        .await?
        .context("Device not found")?;
    ctx.track_device(dev.id, &dev.entity_id);
    let read_started = std::time::Instant::now();
    let entity = ctx
        .config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&dev.entity_id))
        .await?
        .into_iter()
        .next()
        .context("HA state missing")?;
    ctx.config.html_ui.pending.observe_states(
        ctx.user_id,
        &ctx.admission_generation,
        read_started,
        std::slice::from_ref(&entity),
    );
    let slider = Slider::from_entity(&entity);
    if slider.is_none()
        || !super::preferred(&ctx.config, ctx.user_id).await
        || !db::access::can_control_device(ctx.user_id, ctx.is_admin, device, &ctx.config.db)
            .await?
    {
        return crate::bot::screens::control::device_control::render(
            ctx,
            room,
            device,
            DeviceCmd::default(),
        )
        .await;
    }
    let slider = slider.unwrap();
    let room = db::devices::get_room_id_by_entity(&dev.entity_id, &ctx.config.db)
        .await?
        .unwrap_or(room);
    let room_name = db::rooms::get_room_by_id(room, &ctx.config.db)
        .await?
        .map(|r| {
            ctx.track_room(r.id);
            r.display_name().to_owned()
        })
        .unwrap_or_default();
    let parent = Payload::Control(ControlPayload::DeviceControl { room, device });
    let apply = Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::SetTemp(slider.value as f32 / 1000.0),
    });
    let name = crate::bot::screens::control::labels::device_name(&dev, &entity, ctx.lang);
    Ok(View {
        payload: route(room, device),
        lang: ctx.lang,
        compact: true,
        header: Some(name.clone()),
        html_title: Some(name),
        html_context: Some(room_name),
        html_kind: Some("temperature".into()),
        html_back: Some(parent.clone()),
        html_temperature: Some(slider),
        text: words(
            ctx.lang,
            "Выберите температуру и нажмите «Применить».",
            "Choose a temperature, then Apply.",
        )
        .into(),
        kb: InlineKeyboardMarkup::new(vec![
            vec![Button::callback(
                words(ctx.lang, "Применить", "Apply"),
                apply.to_string(),
            )],
            vec![Button::callback(
                words(ctx.lang, "Отмена", "Cancel"),
                parent.to_string(),
            )],
        ]),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slider_supports_fractional_and_negative_targets_and_rejects_invalid_ranges() {
        let mut entity = Entity {
            entity_id: "climate.test".into(),
            name: String::new(),
            state: "heat".into(),
            device_class: None,
            attributes: crate::ha::models::EntityAttributes {
                temperature: Some(-0.1),
                min_temp: Some(-5.0),
                max_temp: Some(5.0),
                target_temp_step: Some(0.1),
                ..Default::default()
            },
        };
        let slider = Slider::from_entity(&entity).unwrap();
        assert_eq!(slider.value, -100);
        for v in [-5000, -100, 0, 100, 5000] {
            assert!(slider.accepts(v));
        }
        for v in [-5100, -50, 50, 5100, i32::MIN, i32::MAX] {
            assert!(!slider.accepts(v));
        }
        for invalid in [f32::NAN, f32::INFINITY, -6.0, 6.0] {
            entity.attributes.temperature = Some(invalid);
            assert!(Slider::from_entity(&entity).is_none());
        }
        entity.attributes.temperature = Some(0.0);
        for state in ["unknown", "unavailable", "heat_cool"] {
            entity.state = state.into();
            assert!(Slider::from_entity(&entity).is_none());
        }
        entity.state = "heat".into();
        entity.attributes.min_temp = None;
        assert!(Slider::from_entity(&entity).is_none());
    }
}
