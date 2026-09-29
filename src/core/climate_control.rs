use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, oneshot};

use crate::models::AppConfig;

const QUEUE_CAPACITY: usize = 32;
const STATE_SETTLE_TIME: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub enum TemperatureCommand {
    Set(f32),
    Step(i8),
}

pub struct TemperatureResult {
    pub target: f32,
    pub unit: String,
}

pub type TemperatureReply = oneshot::Receiver<Result<TemperatureResult>>;

struct Request {
    user_id: u64,
    confirmed: bool,
    command: TemperatureCommand,
    reply: oneshot::Sender<Result<TemperatureResult>>,
}

#[derive(Default)]
pub struct ClimateControls {
    workers: Mutex<HashMap<i64, mpsc::Sender<Request>>>,
}

impl ClimateControls {
    /// Enqueue before spawning UI work so opposite button presses keep arrival order.
    pub fn enqueue(
        &self,
        config: Arc<AppConfig>,
        user_id: u64,
        device_id: i64,
        command: TemperatureCommand,
    ) -> Result<TemperatureReply> {
        self.enqueue_inner(config, user_id, device_id, command, false)
    }

    /// Only the server-side confirmation flow may call this after claiming its record.
    pub(crate) fn enqueue_confirmed(
        &self,
        config: Arc<AppConfig>,
        user_id: u64,
        device_id: i64,
        target: f32,
    ) -> Result<TemperatureReply> {
        self.enqueue_inner(
            config,
            user_id,
            device_id,
            TemperatureCommand::Set(target),
            true,
        )
    }

    fn enqueue_inner(
        &self,
        config: Arc<AppConfig>,
        user_id: u64,
        device_id: i64,
        command: TemperatureCommand,
        confirmed: bool,
    ) -> Result<TemperatureReply> {
        let mut workers = self.workers.lock().unwrap();
        let sender = workers.entry(device_id).or_insert_with(|| {
            let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
            tokio::spawn(run_worker(config, device_id, receiver));
            sender
        });
        let (reply, receiver) = oneshot::channel();
        sender
            .try_send(Request {
                user_id,
                confirmed,
                command,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("Temperature command queue is full"))?;
        Ok(receiver)
    }
}

struct RecentTargets {
    values: VecDeque<f32>,
    sent_at: Instant,
}

async fn run_worker(config: Arc<AppConfig>, device_id: i64, mut receiver: mpsc::Receiver<Request>) {
    let mut recent = None;
    loop {
        let request = match tokio::time::timeout(STATE_SETTLE_TIME, receiver.recv()).await {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(_) => {
                // Enqueue and idle cleanup share the lock; a newly queued command
                // must not be abandoned when retiring an idle worker.
                let mut workers = config.climate_controls.workers.lock().unwrap();
                match receiver.try_recv() {
                    Ok(request) => request,
                    Err(_) => {
                        workers.remove(&device_id);
                        break;
                    }
                }
            }
        };
        let mut uncertain = false;
        let result = execute(&config, device_id, &request, &mut recent, &mut uncertain).await;
        if uncertain {
            recent = None;
            // A failed service request may have reached the device. Do not retry
            // it or apply queued relative steps against an uncertain target.
            while let Ok(queued) = receiver.try_recv() {
                let _ = queued
                    .reply
                    .send(Err(anyhow::anyhow!("Previous temperature command failed")));
            }
        }
        let _ = request.reply.send(result);
    }
}

async fn execute(
    config: &Arc<AppConfig>,
    device_id: i64,
    request: &Request,
    recent: &mut Option<RecentTargets>,
    uncertain: &mut bool,
) -> Result<TemperatureResult> {
    anyhow::ensure!(
        crate::db::access::can_control_device(
            request.user_id,
            request.user_id == config.root_user,
            device_id,
            &config.db,
        )
        .await?,
        "Temperature control is not allowed"
    );
    let device = crate::db::devices::get_device_by_id(device_id, &config.db)
        .await?
        .context("Device not found")?;
    anyhow::ensure!(
        request.confirmed
            || !crate::db::devices::is_device_critical(&device.entity_id, &config.db).await?,
        "Critical device requires confirmation"
    );
    anyhow::ensure!(
        device.entity_id.starts_with("climate."),
        "Not a climate device"
    );
    let entity = config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&device.entity_id))
        .await?
        .into_iter()
        .next()
        .context("Climate state missing")?;
    let current = entity
        .attributes
        .temperature
        .context("Temperature target missing")?;
    anyhow::ensure!(
        entity.temperature_target_is_valid(current),
        "Temperature control unavailable"
    );

    // HA integrations can return the previous target just after accepting a
    // command. Preserve accepted steps briefly, but adopt unrelated HA changes.
    if recent
        .as_ref()
        .is_some_and(|r| r.sent_at.elapsed() >= STATE_SETTLE_TIME || !r.values.contains(&current))
    {
        *recent = None;
    }
    let base = recent
        .as_ref()
        .and_then(|r| r.values.back().copied())
        .unwrap_or(current);
    let target = match request.command {
        TemperatureCommand::Set(target) => target,
        TemperatureCommand::Step(direction) => {
            anyhow::ensure!(matches!(direction, -1 | 1), "Invalid temperature step");
            let step = entity
                .attributes
                .target_temp_step
                .filter(|step| step.is_finite() && *step > 0.0)
                .unwrap_or(0.5);
            let target = ((base + f32::from(direction) * step) * 1000.0).round() / 1000.0;
            target.clamp(
                entity.attributes.min_temp.unwrap(),
                entity.attributes.max_temp.unwrap(),
            )
        }
    };
    anyhow::ensure!(
        entity.temperature_target_is_valid(target),
        "Temperature outside device limits"
    );
    if target != base {
        *uncertain = true;
        config
            .ha_client
            .call_service_with_data(
                "climate",
                "set_temperature",
                &device.entity_id,
                serde_json::json!({"temperature": target}),
            )
            .await?;
        *uncertain = false;
        let remembered = recent.get_or_insert_with(|| RecentTargets {
            values: VecDeque::from([current]),
            sent_at: Instant::now(),
        });
        remembered.values.push_back(target);
        remembered.sent_at = Instant::now();
        while remembered.values.len() > QUEUE_CAPACITY + 1 {
            // Keep the original observed target while HA is still reporting it.
            remembered.values.remove(1);
        }
    }
    Ok(TemperatureResult {
        target,
        unit: entity
            .attributes
            .unit_of_measurement
            .unwrap_or_else(|| "°".into()),
    })
}
