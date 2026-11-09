//! Inline thermostat control. Dragging is local; Apply uses the normal
//! temperature command queue, access checks and HA-state acknowledgement.
use crate::bot::{
    models::View,
    personal::words,
    router::{ControlPayload, DeviceCmd, Payload},
};
use crate::ha::models::Entity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use teloxide::types::{InlineKeyboardButton as Button, InlineKeyboardButtonKind};

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

#[cfg(test)]
pub fn route(room: i64, device: i64) -> Payload {
    Payload::Control(ControlPayload::DeviceControl { room, device })
}

/// Replace the HTML entry buttons with the control itself. Classic views retain
/// their existing keyboard; details, Home, favorites and subscriptions are added
/// by the ordinary device router, exactly as for the other device cards.
pub fn attach(view: &mut View, entity: &Entity) {
    let Some(slider) = Slider::from_entity(entity) else {
        return;
    };
    if let Some(current) = view.html_metrics.get(1) {
        view.html_summary = Some(format!("{}: {}\n{}", current[0], current[1], current[2]));
    }
    view.html_metrics.clear();
    view.html_kind = Some("temperature".into());
    view.html_temperature = Some(slider);
}

/// Only the HTML projection changes its controls; failed widget delivery must
/// still have the complete classic keyboard available.
pub(super) fn controls(view: &mut View) {
    let Some(slider) = view.html_temperature.as_ref() else {
        return;
    };
    let Payload::Control(ControlPayload::DeviceControl { room, device }) = view.payload else {
        return;
    };
    view.kb.inline_keyboard.retain(|row| {
        !row.iter().any(|b| match &b.kind {
            InlineKeyboardButtonKind::CallbackData(data) => matches!(
                Payload::from_string(data),
                Ok(Payload::TemperatureInput { .. }
                    | Payload::Control(ControlPayload::QuickAction {
                        cmd: DeviceCmd::AdjustTemp(_),
                        ..
                    }))
            ),
            _ => false,
        })
    });
    let apply = Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::SetTemp(slider.value as f32 / 1000.0),
    });
    view.kb.inline_keyboard.insert(
        0,
        vec![Button::callback(
            words(view.lang, "Применить", "Apply"),
            apply.to_string(),
        )],
    );
    // Apply and power share a control row; changing mode never applies a draft.
    if let Some(index) = view.kb.inline_keyboard.iter().position(|row| row.len() == 1 &&
        matches!(&row[0].kind, InlineKeyboardButtonKind::CallbackData(data)
            if matches!(Payload::from_string(data), Ok(Payload::Control(ControlPayload::QuickAction {
                cmd: DeviceCmd::TurnOn | DeviceCmd::TurnOff, ..
            }))))) {
        let power = view.kb.inline_keyboard.remove(index);
        view.kb.inline_keyboard[0].extend(power);
    }
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
    let Payload::Control(
        ControlPayload::DeviceControl { room, device }
        | ControlPayload::QuickAction {
            room,
            device,
            cmd: DeviceCmd::EditTemperature,
        },
    ) = view.payload
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
