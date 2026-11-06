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

/// Two authored lines avoid hiding the current state behind a long device name.
pub fn room_html_label(base: &str, unique: &str, entity_id: &str, state: &str) -> String {
    let single_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let qualifier = unique.strip_prefix(&format!("{base} · "));
    let detail = if let Some(qualifier) = qualifier {
        let qualifier = if qualifier == entity_id {
            entity_id.split_once('.').map_or(entity_id, |(_, id)| id)
        } else {
            qualifier
        };
        format!("{state} · {qualifier}")
    } else {
        state.to_owned()
    };
    format!("{}\n{}", single_line(base), single_line(&detail))
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
        return match (
            domain,
            entity.device_class.as_deref().unwrap_or(""),
            logical.as_str(),
        ) {
            ("binary_sensor", "door" | "window" | "opening" | "garage_door", "on") => "Open",
            ("binary_sensor", "door" | "window" | "opening" | "garage_door", "off") => "Closed",
            ("binary_sensor", "motion" | "occupancy" | "presence", "on") => "Detected",
            ("binary_sensor", "motion" | "occupancy" | "presence", "off") => "Clear",
            ("binary_sensor", _, "on") => "Active",
            ("binary_sensor", _, "off") => "Inactive",
            _ => &logical,
        }
        .into();
    }
    StateFormatter::translate_state_for(
        domain,
        entity.device_class.as_deref().unwrap_or(""),
        &logical,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_labels_keep_state_on_second_line_and_disambiguate_duplicates() {
        assert_eq!(
            room_html_label("Люстра", "Люстра", "light.ceiling", "ВЫКЛ"),
            "Люстра\nВЫКЛ"
        );
        for id in ["switch.vykliuchatel_3_right", "switch.rele_0"] {
            let label = room_html_label("Подсветка", &format!("Подсветка · {id}"), id, "ВКЛ");
            assert_eq!(
                label,
                format!("Подсветка\nВКЛ · {}", id.split_once('.').unwrap().1)
            );
        }
        assert_eq!(
            room_html_label(
                "Подсветка",
                "Подсветка · Столешница",
                "switch.worktop",
                "ВКЛ"
            ),
            "Подсветка\nВКЛ · Столешница"
        );
        // User-authored separators and whitespace must never create a third line
        // or get confused with the structured name/state boundary.
        let alias = "Очень длинное\nимя · 🪔 светильника\r\nс пробелами";
        let label = room_html_label(alias, alias, "light.test", "Работает · ярко\n\nна 50%");
        assert_eq!(
            label,
            "Очень длинное имя · 🪔 светильника с пробелами\nРаботает · ярко на 50%"
        );
    }

    #[test]
    fn english_binary_labels_use_classes_inversion_and_aliases_consistently() {
        let mut entity = Entity {
            entity_id: "binary_sensor.test".into(),
            name: "Test".into(),
            state: "on".into(),
            device_class: None,
            attributes: Default::default(),
        };
        for (class, on, off) in [
            ("door", "Open", "Closed"),
            ("window", "Open", "Closed"),
            ("motion", "Detected", "Clear"),
            ("occupancy", "Detected", "Clear"),
            ("", "Active", "Inactive"),
        ] {
            entity.device_class = Some(class.into());
            entity.state = "on".into();
            assert_eq!(state(&entity, Language::En, false, None), on);
            assert_eq!(state(&entity, Language::En, true, None), off);
            entity.state = "off".into();
            assert_eq!(state(&entity, Language::En, false, None), off);
            assert_eq!(state(&entity, Language::En, true, None), on);
            assert_eq!(
                state(&entity, Language::En, true, Some("Custom state")),
                "Custom state"
            );
        }
        for unknown in ["unknown", "unavailable"] {
            entity.state = unknown.into();
            assert_eq!(
                state(&entity, Language::En, false, None),
                state(&entity, Language::En, true, None)
            );
            assert!(!matches!(
                state(&entity, Language::En, true, None).as_str(),
                "Active" | "Inactive"
            ));
        }
    }
}
