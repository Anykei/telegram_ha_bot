use super::*;
use crate::ha::models::EntityAttributes;
use crate::i18n::Language;

#[tokio::test]
async fn climate_metrics_keep_controls_and_bounds_without_duplicate_pages() -> Result<()> {
    let (config, ha) = config().await?;
    db::devices::sync_device("climate.test", "living", "Термостат", "climate", &config.db).await?;
    sqlx::query("DELETE FROM hidden_entities WHERE entity_id='climate.test'")
        .execute(&config.db)
        .await?;
    let (room, device) = device(&config, "climate.test").await?;
    let route = Payload::Control(ControlPayload::DeviceControl { room, device });
    for lang in [Language::Ru, Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        for (name, state, target, current, step, unit, expected_steps, exact) in [
            (
                "normal",
                "heat",
                Some(22.0),
                Some(21.0),
                0.5,
                "°C",
                vec![-1, 1],
                true,
            ),
            (
                "min",
                "heat",
                Some(16.0),
                Some(21.0),
                1.0,
                "°C",
                vec![1],
                true,
            ),
            (
                "max",
                "cool",
                Some(30.0),
                Some(21.0),
                0.1,
                "°C",
                vec![-1],
                true,
            ),
            ("missing", "heat", None, None, 0.5, "°C", vec![], false),
            (
                "offline",
                "unavailable",
                Some(22.0),
                Some(21.0),
                0.5,
                "°C",
                vec![],
                false,
            ),
            (
                "range",
                "heat_cool",
                None,
                Some(21.0),
                0.5,
                "°C",
                vec![],
                false,
            ),
            (
                "fahrenheit",
                "off",
                Some(22.0),
                None,
                1.0,
                "°F",
                vec![-1, 1],
                true,
            ),
        ] {
            ha.states
                .lock()
                .unwrap()
                .insert("climate.test".into(), state.into());
            ha.attributes.lock().unwrap().insert(
                "climate.test".into(),
                EntityAttributes {
                    hvac_modes: Some(vec!["heat".into(), "cool".into(), "off".into()]),
                    temperature: target,
                    current_temperature: current,
                    target_temp_low: (state == "heat_cool").then_some(20.0),
                    target_temp_high: (state == "heat_cool").then_some(24.0),
                    target_temp_step: Some(step),
                    min_temp: Some(16.0),
                    max_temp: Some(30.0),
                    unit_of_measurement: Some(unit.into()),
                },
            );
            let view = router(route.clone(), 1, config.clone()).await?;
            let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
            assert_complete(&view, &prepared)?;
            assert_eq!(
                prepared.pages.len(),
                1,
                "{lang:?}/{name}: no duplicate continuation"
            );
            let page = &prepared.pages[0];
            if exact {
                let slider = page
                    .slider
                    .as_ref()
                    .expect("Control is inline on the device page");
                assert_eq!(slider.value, (target.unwrap() * 1000.0).round() as i32);
                assert_eq!(slider.min, 16000);
                assert_eq!(slider.max, 30000);
                assert_eq!(slider.step, (step * 1000.0).round() as i32);
                assert_eq!(slider.unit, unit);
                assert!(page.status.is_empty());
                assert!(page.body.contains(if lang == Language::Ru {
                    "Сейчас"
                } else {
                    "Current"
                }));
            } else {
                assert_eq!(page.status.len(), 2);
                if state == "unavailable" {
                    assert_eq!(page.status[1][1], crate::i18n::t(lang, "climate.no_data"));
                }
                if state == "heat_cool" {
                    assert_eq!(page.status[0][1], "20°C – 24°C");
                }
                assert!(view.text.contains(&page.status[0][1]));
            }
            let visible = page
                .rows
                .iter()
                .flatten()
                .map(|b| &prepared.actions[&b.key])
                .collect::<Vec<_>>();
            let steps = visible
                .iter()
                .filter_map(|p| match p {
                    Payload::Control(ControlPayload::QuickAction {
                        cmd: DeviceCmd::AdjustTemp(n),
                        ..
                    }) => Some(*n),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(steps, if exact { vec![] } else { expected_steps });
            assert!(!visible.contains(&&Payload::TemperatureInput { room, device }));
            assert!(visible.contains(&&Payload::DeviceDetails { room, device }));
            assert!(!visible.contains(&&Payload::Home));
            assert!(page.back.is_some());
            assert!(
                page.rows.iter().any(|row| {
                    let targets = row
                        .iter()
                        .map(|b| &prepared.actions[&b.key])
                        .collect::<Vec<_>>();
                    targets == vec![&Payload::DeviceDetails { room, device }]
                }),
                "Details use the row without a redundant Home shortcut"
            );
            // Export actual backend pages for the offline renderer review.
            if std::env::var_os("HA_CLIMATE_EXPORT").is_some() {
                println!(
                    "CLIMATE_FIXTURE={}",
                    serde_json::json!({
                        "id": format!("climate_actual_{name}{}", if lang == Language::En {"_en"} else {""}),
                        "label": format!("Thermostat / {name} / {lang:?}"), "page": page
                    })
                );
            } else {
                let mut fixtures: Vec<Value> = serde_json::from_str(include_str!(
                    "../../../ui/widgets/preview/release-fixtures.json"
                ))?;
                fixtures.extend(serde_json::from_str::<Vec<Value>>(include_str!(
                    "../../../ui/widgets/preview/temperature-fixtures.json"
                ))?);
                let id = format!(
                    "climate_actual_{name}{}",
                    if lang == Language::En { "_en" } else { "" }
                );
                let fixture = fixtures
                    .iter()
                    .find(|f| f["id"] == id)
                    .context("Missing climate fixture")?;
                assert_eq!(
                    fixture["page"],
                    serde_json::to_value(page)?,
                    "Browser uses the actual backend layout"
                );
            }
        }
    }
    assert!(
        ha.calls.lock().unwrap().is_empty(),
        "Viewing must never send commands"
    );
    Ok(())
}
