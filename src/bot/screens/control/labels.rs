use crate::core::{presentation::StateFormatter, types::Device};
use crate::ha::models::Entity;
use crate::i18n::{t, Language};
use std::collections::HashMap;

pub fn device_name(device: &Device, entity: &Entity, lang: Language) -> String {
    let name = device
        .alias
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if entity.name.trim().is_empty() {
                &device.entity_id
            } else {
                &entity.name
            }
        });
    if entity.device_class.as_deref() == Some("battery") {
        format!("{name} · {}", t(lang, "control.battery"))
    } else {
        name.to_string()
    }
}

/// Disambiguate only visible devices; never replace the user's saved aliases.
pub fn room_names(devices: &[Device], entities: &[Entity], lang: Language) -> HashMap<i64, String> {
    let mut names = HashMap::new();
    for device in devices {
        if let Some(entity) = entities.iter().find(|e| e.entity_id == device.entity_id) {
            names.insert(device.id, device_name(device, entity, lang));
        }
    }
    let original = names.clone();
    for device in devices {
        let Some(name) = original.get(&device.id) else {
            continue;
        };
        if original.values().filter(|n| *n == name).count() > 1 {
            if let Some(entity) = entities.iter().find(|e| e.entity_id == device.entity_id) {
                if !entity.name.trim().is_empty()
                    && entity.name != device.entity_id
                    && entity.name != *name
                {
                    names.insert(device.id, format!("{name} · {}", entity.name));
                }
            }
        }
    }
    let resolved = names.clone();
    for (&id, name) in &resolved {
        if resolved.values().filter(|other| *other == name).count() > 1 {
            if let Some(device) = devices.iter().find(|d| d.id == id) {
                names.insert(id, format!("{name} · {}", device.entity_id));
            }
        }
    }
    names
}

pub fn state(entity: &Entity, lang: Language, inverted: bool, alias: Option<&str>) -> String {
    if let Some(alias) = alias {
        return alias.into();
    }
    if matches!(entity.state.as_str(), "unavailable" | "unknown") {
        return t(
            lang,
            if entity.state == "unavailable" {
                "control.unavailable"
            } else {
                "climate.no_data"
            },
        )
        .into();
    }
    let domain = entity.entity_id.split('.').next().unwrap_or("");
    if domain == "climate" {
        return super::climate_view::mode_label(lang, &entity.state).into();
    }
    if let Ok(value) = entity.state.parse::<f64>() {
        if value.is_finite() {
            let number = value.to_string();
            let unit = entity.attributes.unit_of_measurement.as_deref().unwrap_or(
                match entity.device_class.as_deref() {
                    Some("battery" | "humidity") => "%",
                    Some("temperature") => "°C",
                    _ => "",
                },
            );
            return format!("{number}{unit}");
        }
    }
    let logical = StateFormatter::logical_state(&entity.state, inverted);
    if lang == Language::En {
        return logical;
    }
    StateFormatter::translate_state_for(
        domain,
        entity.device_class.as_deref().unwrap_or(""),
        &logical,
    )
    .into()
}
