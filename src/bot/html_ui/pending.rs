//! Confirmation of a physical state, independent of widget revisions and navigation.
use super::HtmlUi;
use crate::{core::devices::DeviceAction, db, ha::models::Entity, models::AppConfig};
use anyhow::{ensure, Result};
use dashmap::{mapref::entry::Entry, DashMap};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const SENDING: u8 = 0;
const WAITING: u8 = 1;
const UNCONFIRMED: u8 = 2;
const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(30);

enum Expected {
    State(&'static str),
    HvacMode(&'static str),
    Temperature(f32),
}

impl Expected {
    fn matches(&self, entity: &Entity) -> bool {
        match self {
            Self::State(state) | Self::HvacMode(state) => entity.state == *state,
            Self::Temperature(target) => {
                !matches!(entity.state.as_str(), "unknown" | "unavailable")
                    && entity
                        .attributes
                        .temperature
                        .is_some_and(|value| (value - target).abs() < 0.001)
            }
        }
    }

    fn known(&self, entity: &Entity) -> bool {
        match self {
            Self::State(_) => matches!(entity.state.as_str(), "on" | "off"),
            Self::HvacMode(_) => entity.supports_hvac_mode(&entity.state),
            Self::Temperature(_) => entity
                .attributes
                .temperature
                .is_some_and(|value| entity.temperature_target_is_valid(value)),
        }
    }
}

struct Operation {
    user: u64,
    device: i64,
    generation: String,
    entity: String,
    expected: Expected,
    started: Instant,
    phase: AtomicU8,
    cancel: CancellationToken,
}

#[derive(Default)]
pub(crate) struct PendingCommands {
    operations: DashMap<(u64, i64), Arc<Operation>>,
}

impl PendingCommands {
    /// Only a fresh HA snapshot containing the expected physical state releases
    /// controls. A new revision, an unrelated event or an old render cannot do so.
    pub fn observe_states(
        &self,
        user: u64,
        generation: &str,
        read_started: Instant,
        states: &[Entity],
    ) {
        let completed: Vec<_> = self
            .operations
            .iter()
            .filter(|op| {
                op.user == user
                    && op.generation == generation
                    && op.started <= read_started
                    && op.phase.load(Ordering::Acquire) != SENDING
                    && states
                        .iter()
                        .any(|e| e.entity_id == op.entity && op.expected.matches(e))
            })
            .map(|op| op.value().clone())
            .collect();
        for op in completed {
            self.remove(&op);
        }
    }
    pub fn clear(&self, user: u64) {
        self.operations.retain(|&(owner, _), op| {
            if owner == user {
                op.cancel.cancel();
            }
            owner != user
        });
    }

    fn remove(&self, op: &Arc<Operation>) {
        self.operations
            .remove_if(&(op.user, op.device), |_, current| Arc::ptr_eq(current, op));
        op.cancel.cancel();
    }

    pub fn blocks(&self, user: u64, device: i64, generation: &str) -> bool {
        self.operations
            .get(&(user, device))
            .is_some_and(|op| op.generation == generation)
    }

    pub fn kinds(&self, user: u64, generation: &str) -> HashMap<i64, &'static str> {
        self.operations
            .iter()
            .filter(|op| op.user == user && op.generation == generation)
            .map(|op| {
                (
                    op.device,
                    if op.phase.load(Ordering::Acquire) == UNCONFIRMED {
                        "uncertain"
                    } else {
                        "wait"
                    },
                )
            })
            .collect()
    }

    /// After a timeout/error, an explicit read can reconcile a known state.
    /// It never sends the command again and cannot release an in-flight write.
    pub async fn reconcile(&self, config: &AppConfig, user: u64, generation: &str) {
        let operations: Vec<_> = self
            .operations
            .iter()
            .filter(|op| {
                op.user == user
                    && op.generation == generation
                    && op.phase.load(Ordering::Acquire) == UNCONFIRMED
            })
            .map(|op| op.value().clone())
            .collect();
        let mut valid = Vec::new();
        for op in operations {
            if valid_target(config, &op).await {
                valid.push(op);
            }
        }
        if valid.is_empty() {
            return;
        }
        let ids: Vec<_> = valid.iter().map(|op| op.entity.clone()).collect();
        if let Ok(Ok(states)) = tokio::time::timeout(
            Duration::from_secs(3),
            config.ha_client.fetch_states_by_ids(&ids),
        )
        .await
        {
            for op in valid {
                if states
                    .iter()
                    .any(|e| e.entity_id == op.entity && op.expected.known(e))
                {
                    self.remove(&op);
                }
            }
        }
    }
}

pub(crate) struct PendingCommand(Arc<Operation>);

impl Drop for PendingCommand {
    fn drop(&mut self) {
        // A cancelled/failed dispatch has an unknown outcome until an explicit read.
        let _ = self.0.phase.compare_exchange(
            SENDING,
            UNCONFIRMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

pub(crate) fn begin(
    config: &AppConfig,
    user: u64,
    generation: &str,
    device: i64,
    state: &Entity,
    action: &DeviceAction,
) -> Result<Option<PendingCommand>> {
    let HtmlUi {
        enabled,
        sessions,
        pending,
        ..
    } = &config.html_ui;
    if !enabled
        || sessions
            .get(&user)
            .is_none_or(|s| s.generation != generation)
    {
        return Ok(None);
    }
    let expected = match (state.entity_id.split('.').next(), action) {
        (Some("climate"), DeviceAction::TurnOn) if state.supports_hvac_mode("heat") => {
            Expected::HvacMode("heat")
        }
        (Some("climate"), DeviceAction::TurnOff) if state.supports_hvac_mode("off") => {
            Expected::HvacMode("off")
        }
        (Some("light" | "switch"), DeviceAction::TurnOn) => Expected::State("on"),
        (Some("light" | "switch"), DeviceAction::TurnOff) => Expected::State("off"),
        (Some("light" | "switch"), DeviceAction::Toggle) if state.state == "off" => {
            Expected::State("on")
        }
        (Some("light" | "switch"), DeviceAction::Toggle) if state.state == "on" => {
            Expected::State("off")
        }
        (Some("climate"), DeviceAction::SetTemperature(target))
            if state.temperature_target_is_valid(*target) =>
        {
            Expected::Temperature(*target)
        }
        _ => return Ok(None),
    };
    let op = Arc::new(Operation {
        user,
        device,
        generation: generation.into(),
        entity: state.entity_id.clone(),
        expected,
        started: Instant::now(),
        phase: AtomicU8::new(SENDING),
        cancel: CancellationToken::new(),
    });
    match pending.operations.entry((user, device)) {
        Entry::Occupied(mut entry) => {
            let old = entry.get();
            ensure!(
                old.generation != generation || old.entity != state.entity_id,
                "Device command is still awaiting state confirmation"
            );
            old.cancel.cancel();
            entry.insert(op.clone());
        }
        Entry::Vacant(entry) => {
            entry.insert(op.clone());
        }
    }
    Ok(Some(PendingCommand(op)))
}

impl PendingCommand {
    pub fn finish(self, config: &Arc<AppConfig>, sent: bool) {
        self.finish_after(config, sent, CONFIRMATION_TIMEOUT);
    }

    pub(super) fn finish_after(self, config: &Arc<AppConfig>, sent: bool, timeout: Duration) {
        self.0
            .phase
            .store(if sent { WAITING } else { UNCONFIRMED }, Ordering::Release);
        let op = self.0.clone();
        let config = config.clone();
        db::user::spawn_with_admission(async move {
            let refresh = !sent || observe(&config, &op, timeout).await;
            if refresh
                && !op.cancel.is_cancelled()
                && db::user::admission_is_current(op.user, &op.generation, &config.db)
                    .await
                    .unwrap_or(false)
            {
                let _ = crate::core::enqueue_current_refresh(&config, op.user).await;
            }
        });
    }
}

async fn valid_target(config: &AppConfig, op: &Operation) -> bool {
    if !db::user::admission_is_current(op.user, &op.generation, &config.db)
        .await
        .unwrap_or(false)
    {
        return false;
    }
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM devices WHERE id=? AND entity_id=? AND archived=0)",
    )
    .bind(op.device)
    .bind(&op.entity)
    .fetch_one(&config.db)
    .await
    .unwrap_or(false);
    active
        && db::access::can_view_device(op.user, op.user == config.root_user, op.device, &config.db)
            .await
            .unwrap_or(false)
}

async fn observe(config: &AppConfig, op: &Arc<Operation>, timeout: Duration) -> bool {
    let observe = async {
        loop {
            if !valid_target(config, op).await
                || config
                    .runtime_status
                    .read()
                    .await
                    .shutdown_requested_at
                    .is_some()
            {
                config.html_ui.pending.remove(op);
                return false;
            }
            if let Ok(states) = config
                .ha_client
                .fetch_states_by_ids(std::slice::from_ref(&op.entity))
                .await
            {
                if states
                    .iter()
                    .any(|e| e.entity_id == op.entity && op.expected.matches(e))
                {
                    // A refresh can be skipped while an accepted navigation or
                    // command receipt owns the UI. Keep requesting a coalesced
                    // read until a fresh rendered snapshot releases the button.
                    if !op.cancel.is_cancelled() && valid_target(config, op).await {
                        let _ = crate::core::enqueue_current_refresh(config, op.user).await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };
    tokio::select! {
        biased;
        _ = op.cancel.cancelled() => false,
        result = tokio::time::timeout(timeout, observe) => {
            match result {
                Ok(refresh) => refresh,
                Err(_) => { op.phase.store(UNCONFIRMED, Ordering::Release); true }
            }
        }
    }
}
