static RECORDING_QUEUE_OVERFLOW: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn recording_queue_overflow() -> u64 {
    RECORDING_QUEUE_OVERFLOW.load(std::sync::atomic::Ordering::Relaxed)
}

use crate::db;
use crate::db::camera_recording_rules::{
    ConditionLogic, ConditionOperator, RecordingRuleCondition,
};
use crate::ha::NotifyEvent;
use crate::models::AppConfig;
use anyhow::Result;
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
    log::debug!(
        "Camera recording matcher: event {} {} -> {}, candidate_rules={}",
        event.entity_id,
        event.old_state,
        event.new_state,
        candidates.len()
    );

    let event_group_id = format!(
        "{}:{}:{}",
        event.received_at.timestamp_micros(),
        event.entity_id,
        event.new_state
    );

    for (rule, conditions) in candidates {
        if let Err(error) = process_rule(&config, event, &event_group_id, &rule, &conditions).await
        {
            config.runtime_status.write().await.ha_rule_failures += 1;
            log::warn!(
                "operation=recording_rule rule={} camera={} result=failed error={:#}",
                rule.id,
                rule.camera_id,
                error
            );
            let rule_id = rule.id.to_string();
            let message = format!("{error:#}");
            let _ = db::activity_log::log(
                db::activity_log::NewActivity {
                    user_id: None,
                    kind: "recording",
                    entity_type: "rule",
                    entity_id: Some(&rule_id),
                    action: "recording_rule_error",
                    status: "error",
                    message: Some(&message),
                },
                &config.db,
            )
            .await;
        }
    }

    Ok(())
}

async fn process_rule(
    config: &Arc<AppConfig>,
    event: &NotifyEvent,
    event_group_id: &str,
    rule: &db::camera_recording_rules::RecordingRule,
    conditions: &[RecordingRuleCondition],
) -> Result<()> {
    if !rule.is_enabled() || conditions.is_empty() {
        log::debug!(
            "Camera recording rule {} skipped: enabled={}, conditions={}",
            rule.id,
            rule.is_enabled(),
            conditions.len()
        );
        return Ok(());
    }

    if !db::camera_recording_rule_groups::rule_groups_enabled(rule.id, &config.db).await? {
        log::debug!(
            "Camera recording rule {} skipped: all assigned groups are disabled",
            rule.id
        );
        return Ok(());
    }

    if !db::camera_recording_rules::active_time_matches_at(
        rule.active_time(),
        event.received_at.with_timezone(&chrono::Local),
    ) {
        log::debug!(
            "Camera recording rule {} skipped: inactive time window",
            rule.id
        );
        return Ok(());
    }

    if !matches_rule(config, event, conditions, rule.logic()).await? {
        log::debug!(
            "Camera recording rule {} did not match event {} {} -> {} with logic {:?}",
            rule.id,
            event.entity_id,
            event.old_state,
            event.new_state,
            rule.logic()
        );
        return Ok(());
    }

    let now = event.received_at;
    let trigger_summary = format!(
        "{} {} -> {}",
        event.entity_id, event.old_state, event.new_state
    );

    match db::camera_recording_sessions::start_or_extend_session(
        rule,
        event_group_id,
        &trigger_summary,
        now,
        &config.db,
    )
    .await?
    {
        db::camera_recording_sessions::SessionAction::Created(session_id) => {
            log::info!(
                "Camera recording rule {} matched: created session {}, camera={}, trigger={}",
                rule.id,
                session_id,
                rule.camera_id,
                trigger_summary
            );
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
                Ok(()) => {
                    log::info!("Camera recording session {} queued for worker", session_id);
                }
                Err(TrySendError::Full(_)) => {
                    RECORDING_QUEUE_OVERFLOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let id = session_id.to_string();
                    db::activity_log::log(
                        db::activity_log::NewActivity {
                            user_id: None,
                            kind: "recording",
                            entity_type: "session",
                            entity_id: Some(&id),
                            action: "recording_overflow",
                            status: "overloaded",
                            message: Some(
                                "Bounded recording queue is full; no recording was captured",
                            ),
                        },
                        &config.db,
                    )
                    .await?;
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
                    log::error!(
                        "Camera recording queue is closed, session {} failed",
                        session_id
                    );
                }
            }
        }
        db::camera_recording_sessions::SessionAction::Extended(session_id) => {
            log::info!(
                "Camera recording rule {} matched: extended session {}, camera={}, trigger={}",
                rule.id,
                session_id,
                rule.camera_id,
                trigger_summary
            );
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
        }
        db::camera_recording_sessions::SessionAction::SkippedCooldown => {
            log::info!(
                "Camera recording rule {} matched but skipped by cooldown, camera={}, trigger={}",
                rule.id,
                rule.camera_id,
                trigger_summary
            );
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
    if !event_matches_rule(conditions, event) {
        return Ok(false);
    }

    let context_states = if matches!(logic, ConditionLogic::All) {
        fetch_context_states(config, event, conditions).await?
    } else {
        HashMap::new()
    };

    Ok(matches_rule_with_context(
        event,
        conditions,
        logic,
        &context_states,
    ))
}

fn event_matches_rule(conditions: &[RecordingRuleCondition], event: &NotifyEvent) -> bool {
    let has_event_conditions = conditions
        .iter()
        .any(|condition| condition.operator().is_event_operator());

    conditions.iter().any(|condition| {
        if has_event_conditions && !condition.operator().is_event_operator() {
            return false;
        }

        condition_matches_event(condition, event)
    })
}

fn matches_rule_with_context(
    event: &NotifyEvent,
    conditions: &[RecordingRuleCondition],
    logic: ConditionLogic,
    context_states: &HashMap<String, String>,
) -> bool {
    match logic {
        ConditionLogic::Any => true,
        ConditionLogic::All => {
            for condition in conditions {
                if condition.entity_id == event.entity_id {
                    if !condition_matches_event_or_current(condition, event, &event.new_state) {
                        return false;
                    }
                } else {
                    let Some(state) = context_states.get(&condition.entity_id) else {
                        return false;
                    };
                    if !condition_matches_current(condition, state) {
                        return false;
                    }
                }
            }
            true
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
        ConditionOperator::ChangedTo | ConditionOperator::ChangedFromTo => condition
            .to_state
            .as_deref()
            .is_some_and(|expected| expected == current_state),
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
    use chrono::Utc;

    struct ContextHa {
        pause: bool,
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
        reads: std::sync::Mutex<Vec<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::ha::HomeAssistantClient for ContextHa {
        async fn health_check(&self) -> Result<()> {
            Ok(())
        }
        async fn fetch_rooms(&self) -> Result<Vec<crate::ha::Room>> {
            Ok(vec![])
        }
        async fn fetch_history(
            &self,
            _: &str,
            _: u32,
            _: i32,
        ) -> Result<crate::ha::client::HistoryResult> {
            anyhow::bail!("history was not expected")
        }
        async fn fetch_states_by_ids(
            &self,
            ids: &[String],
        ) -> Result<Vec<crate::ha::models::Entity>> {
            self.reads.lock().unwrap().push(ids.to_vec());
            if self.pause {
                self.started.notify_one();
                self.release.notified().await;
            }
            if ids.iter().any(|id| id == "binary_sensor.broken") {
                anyhow::bail!("test context unavailable")
            }
            Ok(ids
                .iter()
                .map(|id| crate::ha::models::Entity {
                    entity_id: id.clone(),
                    name: id.clone(),
                    state: "on".into(),
                    device_class: None,
                    attributes: Default::default(),
                })
                .collect())
        }
        async fn fetch_action_entities(&self) -> Result<Vec<crate::ha::models::Entity>> {
            Ok(vec![])
        }
        async fn call_service(&self, _: &str, _: &str, _: &str) -> Result<()> {
            anyhow::bail!("physical command was not expected")
        }
        async fn call_service_with_data(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: serde_json::Value,
        ) -> Result<()> {
            anyhow::bail!("physical command was not expected")
        }
    }

    async fn matcher_config(
        pause: bool,
    ) -> (
        Arc<AppConfig>,
        Arc<ContextHa>,
        tokio::sync::mpsc::Receiver<crate::core::camera_recording::RecordingJob>,
    ) {
        let mut config = crate::test_support::config().await;
        let ha = Arc::new(ContextHa {
            pause,
            started: Default::default(),
            release: Default::default(),
            reads: Default::default(),
        });
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let inner = Arc::get_mut(&mut config).unwrap();
        inner.ha_client = ha.clone();
        inner.camera_recording_tx = tx;
        (config, ha, rx)
    }

    async fn candidate(
        config: &AppConfig,
        name: &str,
        context: Option<&str>,
    ) -> Result<(i64, i64)> {
        use crate::db::camera_recording_rules as rules;
        db::rooms::sync_rooms_from_ha("room", "Room", &config.db).await?;
        let camera_id = db::cameras::add_manual_camera(
            db::cameras::NewCamera {
                name,
                room_id: 1,
                stream_url: "rtsp://unused",
                snapshot_url: None,
                clip_seconds: 5,
            },
            &config.db,
        )
        .await?;
        let rule_id = rules::create_rule(
            rules::NewRecordingRule {
                name,
                camera_id,
                condition_logic: if context.is_some() {
                    ConditionLogic::All
                } else {
                    ConditionLogic::Any
                },
                tail_seconds: 60,
                max_segment_seconds: 60,
                cooldown_s: 0,
                retention_days: 30,
                pre_roll_enabled: true,
                pre_roll_seconds: 15,
            },
            &config.db,
        )
        .await?;
        rules::add_condition(
            rules::NewRecordingCondition {
                rule_id,
                entity_id: "binary_sensor.door",
                operator: ConditionOperator::ChangedTo,
                from_state: None,
                to_state: Some("on"),
                value: None,
            },
            &config.db,
        )
        .await?;
        if let Some(entity_id) = context {
            rules::add_condition(
                rules::NewRecordingCondition {
                    rule_id,
                    entity_id,
                    operator: ConditionOperator::Is,
                    from_state: None,
                    to_state: None,
                    value: Some("on"),
                },
                &config.db,
            )
            .await?;
        }
        Ok((rule_id, camera_id))
    }

    #[tokio::test]
    async fn failed_rule_context_does_not_block_independent_camera() -> Result<()> {
        let (config, ha, mut jobs) = matcher_config(false).await;
        let (broken, broken_camera) =
            candidate(&config, "first", Some("binary_sensor.broken")).await?;
        let (independent, independent_camera) = candidate(&config, "second", None).await?;
        assert!(broken < independent); // The failing candidate is evaluated first.
        process_event(config.clone(), &event("off", "on")).await?;
        let job = jobs.try_recv()?;
        assert!(jobs.try_recv().is_err());
        let session = db::camera_recording_sessions::get_session(job.session_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(session.camera_id, independent_camera);
        assert_eq!(session.rule_id, Some(independent));
        assert_eq!(session.status, "queued");
        let broken_sessions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM camera_recording_sessions WHERE camera_id=?")
                .bind(broken_camera)
                .fetch_one(&config.db)
                .await?;
        assert_eq!(broken_sessions, 0);
        assert_eq!(config.runtime_status.read().await.ha_rule_failures, 1);
        let errors: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM activity_log WHERE action='recording_rule_error' AND entity_id=?",
        )
        .bind(broken.to_string())
        .fetch_one(&config.db)
        .await?;
        assert_eq!(errors, 1);
        assert_eq!(
            ha.reads.lock().unwrap().as_slice(),
            &[vec!["binary_sensor.broken".to_string()]]
        );
        Ok(())
    }

    #[tokio::test]
    async fn received_time_survives_delayed_context() -> Result<()> {
        let (config, ha, mut jobs) = matcher_config(true).await;
        candidate(&config, "delayed", Some("binary_sensor.context")).await?;
        let mut received = event("off", "on");
        received.received_at = Utc::now() - chrono::Duration::seconds(30);
        let origin = received.received_at;
        let matcher = process_event(config.clone(), &received);
        tokio::pin!(matcher);
        tokio::select! {
            result = &mut matcher => panic!("context barrier was bypassed: {result:?}"),
            _ = ha.started.notified() => {}
        }
        assert!(jobs.try_recv().is_err());
        let created: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM camera_recording_sessions")
            .fetch_one(&config.db)
            .await?;
        assert_eq!(created, 0);
        ha.release.notify_one();
        matcher.await?;
        let job = jobs.try_recv()?;
        let session = db::camera_recording_sessions::get_session(job.session_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(session.first_event_at, origin);
        assert_eq!(session.last_event_at, origin);
        assert_eq!(
            session.stop_after_at,
            origin + chrono::Duration::seconds(60)
        );
        assert_eq!(session.expires_at, origin + chrono::Duration::days(30));
        assert_eq!(session.pre_roll_seconds, 15);
        assert_eq!(
            session.first_event_at - chrono::Duration::seconds(session.pre_roll_seconds),
            origin - chrono::Duration::seconds(15)
        );
        assert!(session
            .event_group_id
            .starts_with(&origin.timestamp_micros().to_string()));
        assert_eq!(config.runtime_status.read().await.ha_rule_failures, 0);
        Ok(())
    }

    fn condition(
        operator: ConditionOperator,
        from: Option<&str>,
        to: Option<&str>,
        value: Option<&str>,
    ) -> RecordingRuleCondition {
        condition_for("binary_sensor.door", operator, from, to, value)
    }

    fn condition_for(
        entity_id: &str,
        operator: ConditionOperator,
        from: Option<&str>,
        to: Option<&str>,
        value: Option<&str>,
    ) -> RecordingRuleCondition {
        RecordingRuleCondition {
            id: 1,
            rule_id: 1,
            entity_id: entity_id.to_string(),
            operator: operator.as_str().to_string(),
            from_state: from.map(str::to_string),
            to_state: to.map(str::to_string),
            value: value.map(str::to_string),
        }
    }

    fn event(old_state: &str, new_state: &str) -> NotifyEvent {
        event_for("binary_sensor.door", old_state, new_state)
    }

    fn event_for(entity_id: &str, old_state: &str, new_state: &str) -> NotifyEvent {
        NotifyEvent {
            entity_id: entity_id.to_string(),
            old_state: old_state.to_string(),
            new_state: new_state.to_string(),
            friendly_name: "Door".to_string(),
            device_class: None,
            received_at: Utc::now(),
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

    #[test]
    fn state_context_condition_does_not_trigger_rule_with_event_source() {
        let conditions = vec![
            condition_for(
                "binary_sensor.door",
                ConditionOperator::ChangedTo,
                None,
                None,
                None,
            ),
            condition_for(
                "binary_sensor.alarm",
                ConditionOperator::Is,
                None,
                None,
                Some("on"),
            ),
        ];
        let context_states = HashMap::from([("binary_sensor.door".to_string(), "off".to_string())]);

        assert!(!matches_rule_with_context(
            &event_for("binary_sensor.alarm", "off", "on"),
            &conditions,
            ConditionLogic::All,
            &context_states,
        ));
    }

    #[test]
    fn event_source_triggers_when_state_context_matches() {
        let conditions = vec![
            condition_for(
                "binary_sensor.door",
                ConditionOperator::ChangedTo,
                None,
                None,
                None,
            ),
            condition_for(
                "binary_sensor.alarm",
                ConditionOperator::Is,
                None,
                None,
                Some("on"),
            ),
        ];
        let context_states = HashMap::from([("binary_sensor.alarm".to_string(), "on".to_string())]);

        assert!(matches_rule_with_context(
            &event_for("binary_sensor.door", "off", "on"),
            &conditions,
            ConditionLogic::All,
            &context_states,
        ));
    }
}
