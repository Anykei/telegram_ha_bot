//! Interpret a closed set of read operations; never dispatch HA intents/services.
use crate::{
    db,
    ha::intent_recognition::{ReadState, ReadonlyHa, Recognition},
    i18n::{t, Language},
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use sqlx::SqlitePool;
use std::time::Duration;

#[derive(Debug, PartialEq)]
enum ReadOperation {
    State,
    Attribute(String),
    Predicate(Vec<String>),
    Temperature,
    Weather,
}

fn operation(recognition: &Recognition) -> Result<ReadOperation> {
    ensure!(
        !recognition.details.contains_key("floor") || recognition.floor_targets_supported,
        "HA version does not provide verified floor targets"
    );
    ensure!(
        recognition.matched
            && matches!(
                recognition.source.as_deref(),
                None | Some("builtin" | "custom")
            ),
        "Unsupported recognition source"
    );
    ensure!(
        recognition.details.keys().all(|key| matches!(
            key.as_str(),
            "name" | "area" | "floor" | "domain" | "device_class" | "state" | "attribute"
        )),
        "Unsupported query parameters"
    );
    match recognition.intent.name.as_str() {
        "HassGetState" => {
            ensure!(
                !(recognition.details.contains_key("state")
                    && recognition.details.contains_key("attribute")),
                "Ambiguous query"
            );
            if let Some(slot) = recognition.details.get("state") {
                let values = match &slot.value {
                    Value::String(value) => vec![value.clone()],
                    Value::Array(values) => values
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .context("Invalid state predicate")
                        })
                        .collect::<Result<Vec<_>>>()?,
                    _ => bail!("Invalid state predicate"),
                };
                ensure!(
                    !values.is_empty() && values.iter().all(|v| !v.is_empty()),
                    "Empty predicate"
                );
                Ok(ReadOperation::Predicate(values))
            } else if let Some(slot) = recognition.details.get("attribute") {
                let attribute = slot.value.as_str().context("Invalid attribute")?;
                ensure!(
                    matches!(
                        attribute,
                        "current_temperature"
                            | "temperature"
                            | "humidity"
                            | "current_humidity"
                            | "brightness"
                            | "battery_level"
                            | "current_position"
                    ),
                    "Unsupported attribute"
                );
                Ok(ReadOperation::Attribute(attribute.to_owned()))
            } else {
                Ok(ReadOperation::State)
            }
        }
        "HassClimateGetTemperature" => Ok(ReadOperation::Temperature),
        "HassGetWeather" => Ok(ReadOperation::Weather),
        _ => bail!("Unsupported readonly intent"),
    }
}

async fn allowed(
    user_id: u64,
    root: bool,
    entity_id: &str,
    pool: &SqlitePool,
) -> Result<Option<String>> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, COALESCE(NULLIF(alias, ''), NULLIF(ha_name, ''), entity_id) FROM devices WHERE entity_id = ?")
        .bind(entity_id).fetch_optional(pool).await?;
    if let Some((id, name)) = row {
        if db::access::can_view_device(user_id, root, id, pool).await? {
            return Ok(Some(name));
        }
    } else if root {
        return Ok(Some(entity_id.to_owned()));
    }
    Ok(None)
}

pub async fn answer(
    ha: &dyn ReadonlyHa,
    user_id: u64,
    root: bool,
    text: &str,
    language: Language,
    pool: &SqlitePool,
    timeout_s: u64,
) -> Result<String> {
    match tokio::time::timeout(
        Duration::from_secs(timeout_s.max(1)),
        answer_inner(ha, user_id, root, text, language, pool),
    )
    .await
    {
        Ok(Ok(answer)) => Ok(answer),
        // Never include HA payloads, target names or credentials in errors/logs.
        _ => bail!("{}", t(language, "readonly.unavailable")),
    }
}

async fn answer_inner(
    ha: &dyn ReadonlyHa,
    user_id: u64,
    root: bool,
    text: &str,
    language: Language,
    pool: &SqlitePool,
) -> Result<String> {
    let mut recognized = ha.recognize(text, language.code()).await?;
    let op = operation(&recognized)?;
    if op == ReadOperation::Weather
        && recognized.targets.is_empty()
        && recognized.details.is_empty()
    {
        for id in ha.weather_entities().await? {
            recognized
                .targets
                .insert(id, crate::ha::intent_recognition::Target { matched: true });
        }
    }
    if recognized.targets.is_empty() {
        return Ok(t(language, "readonly.no_targets").to_owned());
    }
    let explicit = recognized.details.contains_key("name");
    let mut readable = Vec::new();
    let mut restricted = false;
    for entity_id in recognized.targets.keys() {
        if allowed(user_id, root, entity_id, pool).await?.is_none() {
            if explicit {
                return Ok(t(language, "readonly.no_access").to_owned());
            }
            restricted = true;
            continue;
        }
        match &op {
            ReadOperation::Temperature => ensure!(
                entity_id.starts_with("climate.") || entity_id.starts_with("sensor."),
                "Unexpected temperature target"
            ),
            ReadOperation::Weather => ensure!(
                entity_id.starts_with("weather."),
                "Unexpected weather target"
            ),
            _ => {}
        }
        readable.push(entity_id.clone());
    }
    let mut states = Vec::new();
    for entity_id in readable {
        let state = ha.read_state(&entity_id).await?;
        if let Some(state) = &state {
            ensure!(state.entity_id == entity_id, "Mismatched state entity");
            if op == ReadOperation::Temperature && entity_id.starts_with("sensor.") {
                ensure!(
                    state.attributes.get("device_class").and_then(Value::as_str)
                        == Some("temperature"),
                    "Not a temperature sensor"
                );
            }
        }
        states.push((entity_id, state));
    }
    let needs_temperature_unit = matches!(&op, ReadOperation::Temperature)
        || matches!(&op, ReadOperation::Attribute(key) if key == "temperature" || key == "current_temperature");
    let temperature_unit =
        if needs_temperature_unit && states.iter().any(|(id, _)| id.starts_with("climate.")) {
            ha.temperature_unit().await?
        } else {
            None
        };
    let mut visible = Vec::new();
    // Check again after ALL network waits, before computing counts or formatting.
    for (id, state) in states {
        if let Some(name) = allowed(user_id, root, &id, pool).await? {
            visible.push((name, state));
        } else {
            if explicit {
                return Ok(t(language, "readonly.no_access").to_owned());
            }
            restricted = true;
        }
    }
    if visible.is_empty() {
        return Ok(t(language, "readonly.no_access").to_owned());
    }
    let mut lines = Vec::new();
    if restricted {
        lines.push(t(language, "readonly.scope").to_owned());
    }
    if let ReadOperation::Predicate(expected) = &op {
        let count = visible
            .iter()
            .filter(|(_, s)| {
                s.as_ref()
                    .is_some_and(|s| known(s) && expected.contains(&s.state))
            })
            .count();
        let unknown = visible
            .iter()
            .filter(|(_, s)| s.as_ref().is_none_or(|s| !known(s)))
            .count();
        let all = if count == visible.len() {
            t(language, "readonly.yes")
        } else if count + unknown == visible.len() {
            t(language, "readonly.unknown")
        } else {
            t(language, "readonly.no")
        };
        let any = if count > 0 {
            t(language, "readonly.yes")
        } else if unknown > 0 {
            t(language, "readonly.unknown")
        } else {
            t(language, "readonly.no")
        };
        lines.push(format!(
            "{}: {}/{}. {}: {}. {}: {}. {}: {}.",
            t(language, "readonly.matches"),
            count,
            visible.len(),
            t(language, "readonly.any"),
            any,
            t(language, "readonly.all"),
            all,
            t(language, "readonly.missing"),
            unknown
        ));
    }
    for (name, state) in &visible {
        let value = state
            .as_ref()
            .filter(|s| known(s))
            .map(|s| format_value(s, &op, temperature_unit.as_deref(), language))
            .unwrap_or_else(|| t(language, "readonly.unknown").to_owned());
        lines.push(format!("{name}: {value}"));
    }
    // Telegram accepts 4096 characters; keep complete rows and report omission.
    let mut result = String::new();
    for line in lines {
        if result.chars().count() + line.chars().count() > 3500 {
            result.push_str(t(language, "readonly.more"));
            break;
        }
        result.push_str(&line);
        result.push('\n');
    }
    Ok(result.trim_end().to_owned())
}

fn known(state: &ReadState) -> bool {
    !matches!(state.state.as_str(), "unknown" | "unavailable" | "")
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn format_value(
    state: &ReadState,
    op: &ReadOperation,
    temperature_unit: Option<&str>,
    lang: Language,
) -> String {
    let attr = |key: &str| state.attributes.get(key).and_then(scalar);
    let (value, unit) = match op {
        ReadOperation::Temperature if state.entity_id.starts_with("sensor.") => {
            (Some(state.state.clone()), attr("unit_of_measurement"))
        }
        ReadOperation::Temperature => (
            attr("current_temperature"),
            attr("temperature_unit").or_else(|| temperature_unit.map(str::to_owned)),
        ),
        ReadOperation::Attribute(key) => (
            attr(key),
            match key.as_str() {
                "temperature" | "current_temperature" => {
                    attr("temperature_unit").or_else(|| temperature_unit.map(str::to_owned))
                }
                "humidity" | "current_humidity" | "battery_level" | "current_position" => {
                    Some("%".into())
                }
                _ => None,
            },
        ),
        ReadOperation::Weather => {
            let mut value = state.state.clone();
            if let Some(temperature) = attr("temperature") {
                value.push_str(&format!(
                    ", {} {}",
                    temperature,
                    attr("temperature_unit").unwrap_or_default()
                ));
            }
            return value;
        }
        _ => (Some(state.state.clone()), attr("unit_of_measurement")),
    };
    value
        .map(|v| format!("{}{}", v, unit.map(|u| format!(" {u}")).unwrap_or_default()))
        .unwrap_or_else(|| t(lang, "readonly.unknown").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    struct Fake {
        recognition: Recognition,
        reads: Mutex<Vec<String>>,
        revoke: Option<SqlitePool>,
        state_override: Option<Option<ReadState>>,
    }
    #[async_trait::async_trait]
    impl ReadonlyHa for Fake {
        async fn recognize(&self, _: &str, _: &str) -> Result<Recognition> {
            Ok(self.recognition.clone())
        }
        async fn read_state(&self, id: &str) -> Result<Option<ReadState>> {
            self.reads.lock().unwrap().push(id.to_owned());
            if let Some(pool) = &self.revoke {
                db::access::set_room_access(1, 1, false, false, pool).await?;
            }
            if let Some(state) = &self.state_override {
                return Ok(state.clone());
            }
            Ok(Some(serde_json::from_value(
                json!({"entity_id":id,"state":"on","attributes":{"unit_of_measurement":"°C"}}),
            )?))
        }
        async fn temperature_unit(&self) -> Result<Option<String>> {
            Ok(Some("°C".into()))
        }
        async fn weather_entities(&self) -> Result<Vec<String>> {
            Ok(vec!["weather.test".into()])
        }
    }
    fn fake() -> Fake {
        Fake { recognition: serde_json::from_value(json!({"match":true,"intent":{"name":"HassGetState"},
            "details":{"state":{"value":"on"}},"targets":{"light.allowed":{"matched":false},"light.secret":{"matched":true}},"source":"builtin"})).unwrap(),
            reads: Mutex::new(Vec::new()), revoke: None, state_override: None }
    }
    async fn pool() -> Result<SqlitePool> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        sqlx::query("INSERT INTO users (id) VALUES (1)")
            .execute(&pool)
            .await?;
        db::access::set_user_role(1, "guest", &pool).await?;
        db::rooms::sync_rooms_from_ha("room", "Room", &pool).await?;
        sqlx::query("INSERT INTO devices (room_id, entity_id, alias) VALUES (1, 'light.allowed', 'Allowed')").execute(&pool).await?;
        db::access::set_room_access(1, 1, true, false, &pool).await?;
        Ok(pool)
    }
    #[tokio::test]
    async fn filters_before_reading_and_aggregates_fresh_states() -> Result<()> {
        let pool = pool().await?;
        let ha = fake();
        for language in [Language::Ru, Language::En] {
            let answer = answer(&ha, 1, false, "are all lights on", language, &pool, 2).await?;
            assert!(answer.contains("1/1"));
            assert!(answer.contains("Allowed: on °C"));
            assert!(!answer.contains("secret"));
        }
        assert_eq!(
            *ha.reads.lock().unwrap(),
            vec!["light.allowed", "light.allowed"]
        );
        Ok(())
    }
    #[tokio::test]
    async fn denied_named_target_and_revoked_access_reveal_nothing() -> Result<()> {
        let pool = pool().await?;
        let mut ha = fake();
        ha.recognition.details.insert(
            "name".into(),
            crate::ha::intent_recognition::Slot {
                value: json!("secret"),
            },
        );
        assert_eq!(
            answer(&ha, 1, false, "state", Language::En, &pool, 2).await?,
            t(Language::En, "readonly.no_access")
        );
        assert!(ha.reads.lock().unwrap().is_empty());
        ha.recognition.details.remove("name");
        ha.revoke = Some(pool.clone());
        assert_eq!(
            answer(&ha, 1, false, "state", Language::En, &pool, 2).await?,
            t(Language::En, "readonly.no_access")
        );
        Ok(())
    }
    #[tokio::test]
    async fn actions_and_custom_triggers_never_read_or_execute() -> Result<()> {
        let pool = pool().await?;
        for intent in [
            "HassTurnOn",
            "HassTurnOff",
            "HassActivateScene",
            "CustomAction",
        ] {
            let mut ha = fake();
            ha.recognition.intent.name = intent.into();
            assert!(answer(
                &ha,
                1,
                false,
                "покажи статус и активируй сцену ночь\n",
                Language::Ru,
                &pool,
                2
            )
            .await
            .is_err());
            assert!(ha.reads.lock().unwrap().is_empty());
        }
        Ok(())
    }
    #[test]
    fn temperature_and_missing_attributes_preserve_units_and_unknown() {
        let state: ReadState = serde_json::from_value(json!({"entity_id":"climate.test","state":"heat","attributes":{"current_temperature":21.5}})).unwrap();
        assert_eq!(
            format_value(
                &state,
                &ReadOperation::Temperature,
                Some("°C"),
                Language::En
            ),
            "21.5 °C"
        );
        assert_eq!(
            format_value(
                &state,
                &ReadOperation::Attribute("humidity".into()),
                None,
                Language::En
            ),
            "unknown"
        );
    }

    #[tokio::test]
    async fn missing_and_unknown_states_do_not_turn_into_zero_or_negative_answers() -> Result<()> {
        let pool = pool().await?;
        for state in [
            None,
            Some(serde_json::from_value(
                json!({"entity_id":"light.allowed", "state":"unavailable", "attributes":{}}),
            )?),
        ] {
            let mut ha = fake();
            ha.state_override = Some(state);
            let result = answer(&ha, 1, false, "are all lights on", Language::En, &pool, 2).await?;
            assert!(result.contains("All match: unknown"));
            assert!(result.contains("Any matches: unknown"));
            assert!(result.contains("Unavailable: 1"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn weather_catalog_empty_targets_and_unverified_floor_are_safe() -> Result<()> {
        let pool = pool().await?;
        let mut ha = fake();
        ha.recognition.targets.clear();
        assert_eq!(
            answer(&ha, 1, false, "question", Language::Ru, &pool, 2).await?,
            t(Language::Ru, "readonly.no_targets")
        );
        ha.recognition.intent.name = "HassGetWeather".into();
        ha.recognition.details.clear();
        assert_eq!(
            answer(&ha, 1, false, "weather", Language::En, &pool, 2).await?,
            t(Language::En, "readonly.no_access")
        );
        assert!(ha.reads.lock().unwrap().is_empty());
        ha.recognition.details.insert(
            "floor".into(),
            crate::ha::intent_recognition::Slot {
                value: json!("upstairs"),
            },
        );
        assert!(operation(&ha.recognition).is_err());
        Ok(())
    }
}
