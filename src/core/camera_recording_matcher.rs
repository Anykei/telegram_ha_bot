use crate::db;
use crate::db::camera_recording_rules::{
    ConditionLogic, ConditionOperator, RecordingRuleCondition,
};
use crate::ha::NotifyEvent;
use crate::models::AppConfig;
use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError;

pub async fn process_event(config: Arc<AppConfig>, event: &NotifyEvent) -> Result<()> {
    if event.old_state == event.new_state {
        return Ok(());
    }

    let candidates =
        db::camera_recording_rules::find_candidate_rules_by_entity(&event.entity_id, &config.db)
            .await?;
    if candidates.is_empty() {
        return Ok(());
    }

    let event_group_id = format!(
        "{}:{}:{}",
        Utc::now().timestamp_millis(),
        event.entity_id,
        event.new_state
    );

    for (rule, conditions) in candidates {
        if !rule.is_enabled() || conditions.is_empty() {
            continue;
        }

        if !db::camera_recording_rule_groups::rule_groups_enabled(rule.id, &config.db).await? {
            continue;
        }

        if !matches_rule(&config, event, &conditions, rule.logic()).await? {
            continue;
        }

        let now = Utc::now();
        let trigger_summary = format!(
            "{} {} -> {}",
            event.entity_id, event.old_state, event.new_state
        );

        match db::camera_recording_sessions::start_or_extend_session(
            &rule,
            &event_group_id,
            &trigger_summary,
            now,
            &config.db,
        )
        .await?
        {
            db::camera_recording_sessions::SessionAction::Created(session_id) => {
                let rule_id = rule.id.to_string();
                let _ = db::activity_log::log(
                    db::activity_log::NewActivity {
                        user_id: None,
                        kind: "recording",
                        entity_type: "rule",
                        entity_id: Some(&rule_id),
                        action: "recording_created",
                        status: "ok",
                        message: Some(&trigger_summary),
                    },
                    &config.db,
                )
                .await;
                let job = crate::core::camera_recording::RecordingJob { session_id };
                match config.camera_recording_tx.try_send(job) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        db::camera_recording_sessions::mark_failed(
                            session_id,
                            "recording queue is full",
                            &config.db,
                        )
                        .await?;
                        log::warn!(
                            "Camera recording queue is full, session {} failed",
                            session_id
                        );
                    }
                    Err(TrySendError::Closed(_)) => {
                        db::camera_recording_sessions::mark_failed(
                            session_id,
                            "recording queue is closed",
                            &config.db,
                        )
                        .await?;
                    }
                }
            }
            db::camera_recording_sessions::SessionAction::Extended(session_id) => {
                let rule_id = rule.id.to_string();
                let _ = db::activity_log::log(
                    db::activity_log::NewActivity {
                        user_id: None,
                        kind: "recording",
                        entity_type: "rule",
                        entity_id: Some(&rule_id),
                        action: "recording_extended",
                        status: "ok",
                        message: Some(&trigger_summary),
                    },
                    &config.db,
                )
                .await;
                log::debug!("Camera recording session {} extended", session_id);
            }
            db::camera_recording_sessions::SessionAction::SkippedCooldown => {}
        }
    }

    Ok(())
}

async fn matches_rule(
    config: &Arc<AppConfig>,
    event: &NotifyEvent,
    conditions: &[RecordingRuleCondition],
    logic: ConditionLogic,
) -> Result<bool> {
    let event_matches = conditions
        .iter()
        .any(|condition| condition_matches_event(condition, event));

    if !event_matches {
        return Ok(false);
    }

    match logic {
        ConditionLogic::Any => Ok(true),
        ConditionLogic::All => {
            let context_states = fetch_context_states(config, event, conditions).await?;
            for condition in conditions {
                if condition.entity_id == event.entity_id {
                    if !condition_matches_event_or_current(condition, event, &event.new_state) {
                        return Ok(false);
                    }
                } else {
                    let Some(state) = context_states.get(&condition.entity_id) else {
                        return Ok(false);
                    };
                    if !condition_matches_current(condition, state) {
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        }
    }
}

async fn fetch_context_states(
    config: &Arc<AppConfig>,
    event: &NotifyEvent,
    conditions: &[RecordingRuleCondition],
) -> Result<HashMap<String, String>> {
    let entity_ids = conditions
        .iter()
        .filter(|condition| condition.entity_id != event.entity_id)
        .map(|condition| condition.entity_id.clone())
        .collect::<Vec<_>>();

    if entity_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let states = config.ha_client.fetch_states_by_ids(&entity_ids).await?;
    Ok(states
        .into_iter()
        .map(|entity| (entity.entity_id, entity.state))
        .collect())
}

fn condition_matches_event(condition: &RecordingRuleCondition, event: &NotifyEvent) -> bool {
    if condition.entity_id != event.entity_id {
        return false;
    }

    match condition.operator() {
        ConditionOperator::ChangedTo => {
            optional_eq(condition.to_state.as_deref(), &event.new_state)
        }
        ConditionOperator::ChangedFromTo => {
            optional_eq(condition.from_state.as_deref(), &event.old_state)
                && optional_eq(condition.to_state.as_deref(), &event.new_state)
        }
        ConditionOperator::Is
        | ConditionOperator::IsNot
        | ConditionOperator::Contains
        | ConditionOperator::Above
        | ConditionOperator::Below => condition_matches_current(condition, &event.new_state),
    }
}

fn condition_matches_event_or_current(
    condition: &RecordingRuleCondition,
    event: &NotifyEvent,
    current_state: &str,
) -> bool {
    if condition.operator().is_event_operator() {
        condition_matches_event(condition, event)
    } else {
        condition_matches_current(condition, current_state)
    }
}

fn condition_matches_current(condition: &RecordingRuleCondition, current_state: &str) -> bool {
    let expected = condition
        .value
        .as_deref()
        .or(condition.to_state.as_deref())
        .unwrap_or("");

    match condition.operator() {
        ConditionOperator::ChangedTo => optional_eq(condition.to_state.as_deref(), current_state),
        ConditionOperator::ChangedFromTo => {
            optional_eq(condition.to_state.as_deref(), current_state)
        }
        ConditionOperator::Is => current_state == expected,
        ConditionOperator::IsNot => current_state != expected,
        ConditionOperator::Contains => current_state.contains(expected),
        ConditionOperator::Above => {
            compare_numeric(current_state, expected, |left, right| left > right)
        }
        ConditionOperator::Below => {
            compare_numeric(current_state, expected, |left, right| left < right)
        }
    }
}

fn optional_eq(expected: Option<&str>, actual: &str) -> bool {
    expected.map(|value| value == actual).unwrap_or(true)
}

fn compare_numeric<F>(current: &str, expected: &str, compare: F) -> bool
where
    F: Fn(f64, f64) -> bool,
{
    let Ok(left) = current.parse::<f64>() else {
        return false;
    };
    let Ok(right) = expected.parse::<f64>() else {
        return false;
    };
    compare(left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(
        operator: ConditionOperator,
        from: Option<&str>,
        to: Option<&str>,
        value: Option<&str>,
    ) -> RecordingRuleCondition {
        RecordingRuleCondition {
            id: 1,
            rule_id: 1,
            entity_id: "binary_sensor.door".to_string(),
            operator: operator.as_str().to_string(),
            from_state: from.map(str::to_string),
            to_state: to.map(str::to_string),
            value: value.map(str::to_string),
        }
    }

    fn event(old_state: &str, new_state: &str) -> NotifyEvent {
        NotifyEvent {
            entity_id: "binary_sensor.door".to_string(),
            old_state: old_state.to_string(),
            new_state: new_state.to_string(),
            friendly_name: "Door".to_string(),
            device_class: None,
        }
    }

    #[test]
    fn changed_from_to_requires_old_state() {
        let condition = condition(
            ConditionOperator::ChangedFromTo,
            Some("off"),
            Some("on"),
            None,
        );

        assert!(condition_matches_event(&condition, &event("off", "on")));
        assert!(!condition_matches_event(
            &condition,
            &event("unavailable", "on")
        ));
    }

    #[test]
    fn numeric_operators_compare_current_state() {
        let above = condition(ConditionOperator::Above, None, None, Some("28"));
        let below = condition(ConditionOperator::Below, None, None, Some("20"));

        assert!(condition_matches_current(&above, "29.5"));
        assert!(condition_matches_current(&below, "19"));
        assert!(!condition_matches_current(&above, "unknown"));
    }
}
