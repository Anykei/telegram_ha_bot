use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::devices::{validate_critical_consent, CriticalConsent, DeviceAction};
use crate::models::AppConfig;

const QUEUE_CAPACITY: usize = 32;
const MAX_CLIMATE_WORKERS: usize = 32;
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
    admission_generation: String,
    entity_id: String,
    consent: Option<CriticalConsent>,
    command: TemperatureCommand,
    reply: oneshot::Sender<Result<TemperatureResult>>,
}

#[derive(Default)]
pub struct ClimateControls {
    workers: Mutex<Workers>,
    stopping: CancellationToken,
}

#[derive(Default)]
struct Workers {
    senders: HashMap<i64, mpsc::Sender<Request>>,
    tasks: Vec<JoinHandle<()>>,
}

// If the application's shared shutdown deadline expires, dropping the shutdown
// future aborts remaining async tasks instead of detaching them past DB close.
struct JoiningWorkers(Vec<JoinHandle<()>>);

impl Drop for JoiningWorkers {
    fn drop(&mut self) {
        for task in &self.0 {
            if !task.is_finished() {
                log::warn!("operation=climate_shutdown stage=deadline result=aborted outcome=possibly_unknown");
                task.abort();
            }
        }
    }
}

impl ClimateControls {
    /// Stop admission synchronously; requests already sent to HA keep awaiting
    /// their result until shutdown's caller exhausts its overall time budget.
    pub fn stop_accepting(&self) {
        let mut workers = self.workers.lock().unwrap();
        self.stopping.cancel();
        workers.senders.clear();
    }

    /// Join all workers. The caller supplies the application's shutdown timeout.
    pub async fn shutdown(&self) -> Result<()> {
        self.stop_accepting();
        let mut joining = JoiningWorkers({
            let mut workers = self.workers.lock().unwrap();
            std::mem::take(&mut workers.tasks)
        });
        let mut failure = None;
        for task in &mut joining.0 {
            if let Err(error) = task.await {
                log::warn!("operation=climate_shutdown stage=join result=failed reason={error}");
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            return Err(error.into());
        }
        Ok(())
    }

    /// Enqueue before spawning UI work so opposite button presses keep arrival order.
    pub async fn enqueue(
        &self,
        config: Arc<AppConfig>,
        user_id: u64,
        device_id: i64,
        command: TemperatureCommand,
    ) -> Result<TemperatureReply> {
        self.enqueue_inner(config, user_id, device_id, command, None)
            .await
    }

    /// Only the server-side confirmation flow may call this after claiming its record.
    pub(crate) async fn enqueue_confirmed(
        &self,
        config: Arc<AppConfig>,
        user_id: u64,
        device_id: i64,
        target: f32,
        consent: CriticalConsent,
    ) -> Result<TemperatureReply> {
        self.enqueue_inner(
            config,
            user_id,
            device_id,
            TemperatureCommand::Set(target),
            Some(consent),
        )
        .await
    }

    async fn enqueue_inner(
        &self,
        config: Arc<AppConfig>,
        user_id: u64,
        device_id: i64,
        command: TemperatureCommand,
        consent: Option<CriticalConsent>,
    ) -> Result<TemperatureReply> {
        let admission_generation = crate::db::user::admission_generation(user_id, &config.db)
            .await?
            .context("Access revoked / Доступ отозван")?;
        let entity_id = crate::db::devices::get_device_by_id(device_id, &config.db)
            .await?
            .context("Device missing")?
            .entity_id;
        let mut workers = self.workers.lock().unwrap();
        anyhow::ensure!(
            !self.stopping.is_cancelled(),
            "Application is stopping; temperature command was not queued"
        );
        // Retired workers cannot accumulate an unbounded history of handles.
        workers.tasks.retain(|task| !task.is_finished());
        anyhow::ensure!(
            workers.senders.contains_key(&device_id) || workers.senders.len() < MAX_CLIMATE_WORKERS,
            "Temperature worker capacity is full"
        );
        let Workers { senders, tasks } = &mut *workers;
        let sender = senders.entry(device_id).or_insert_with(|| {
            let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
            tasks.push(tokio::spawn(run_worker(config, device_id, receiver)));
            sender
        });
        let (reply, receiver) = oneshot::channel();
        sender
            .try_send(Request {
                user_id,
                admission_generation,
                entity_id,
                consent,
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
        let received = tokio::select! {
            biased;
            _ = config.climate_controls.stopping.cancelled() => {
                receiver.close();
                while let Some(queued) = receiver.recv().await {
                    let _ = queued.reply.send(Err(anyhow::anyhow!("Application stopped before temperature command was sent")));
                }
                break;
            }
            received = tokio::time::timeout(STATE_SETTLE_TIME, receiver.recv()) => received,
        };
        let request = match received {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(_) => {
                // Enqueue and idle cleanup share the lock; a newly queued command
                // must not be abandoned when retiring an idle worker.
                let mut workers = config.climate_controls.workers.lock().unwrap();
                match receiver.try_recv() {
                    Ok(request) => request,
                    Err(_) => {
                        workers.senders.remove(&device_id);
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
        !config.climate_controls.stopping.is_cancelled(),
        "Application is stopping; temperature command was not sent"
    );
    anyhow::ensure!(
        crate::db::user::admission_is_current(
            request.user_id,
            &request.admission_generation,
            &config.db
        )
        .await?,
        "Temperature request admission expired"
    );
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
        device.entity_id == request.entity_id,
        super::devices::DeviceTargetChanged
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
    anyhow::ensure!(
        entity.entity_id == request.entity_id,
        super::devices::DeviceTargetChanged
    );
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
    // Even a queued request that was ordinary when admitted may now require
    // consent. A confirmed temperature authorizes only its exact resolved target.
    validate_critical_consent(
        &config.db,
        device_id,
        &request.entity_id,
        &DeviceAction::SetTemperature(target),
        request.consent.as_ref(),
    )
    .await?;
    if target != base {
        // State reads may be slow. Revoke/readd and ACL changes during that read
        // must invalidate the queued request before the first external effect.
        anyhow::ensure!(
            crate::db::user::admission_is_current(
                request.user_id,
                &request.admission_generation,
                &config.db
            )
            .await?
                && crate::db::access::can_control_device(
                    request.user_id,
                    request.user_id == config.root_user,
                    device_id,
                    &config.db
                )
                .await?,
            "Temperature control access revoked"
        );
        anyhow::ensure!(
            config
                .runtime_status
                .read()
                .await
                .shutdown_requested_at
                .is_none(),
            "Application is stopping; temperature command was not sent"
        );
        anyhow::ensure!(
            !config.climate_controls.stopping.is_cancelled(),
            "Application is stopping; temperature command was not sent"
        );
        validate_critical_consent(
            &config.db,
            device_id,
            &request.entity_id,
            &DeviceAction::SetTemperature(target),
            request.consent.as_ref(),
        )
        .await?;
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
