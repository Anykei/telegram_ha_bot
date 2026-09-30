use crate::db;
use crate::models::AppConfig;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use dashmap::DashMap;
use ffmpeg::{codec, encoder, format, media, Dictionary, Rational};
use ffmpeg_next as ffmpeg;
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

static OWNER_CAPACITY: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();
const MAX_PRE_ROLL_OWNERS: usize = 32;
const OWNER_RECONCILE_INTERVAL: Duration = Duration::from_secs(10);
const OWNER_RESTART_DELAY: Duration = Duration::from_secs(5);
const BUFFER_KEYFRAME_GRACE_S: u32 = 5;
const DEFAULT_MAX_BYTES_PER_CAMERA: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct CameraPreRollRegistry {
    buffers: Arc<DashMap<i64, Arc<Mutex<PreRollBufferInner>>>>,
    #[cfg(test)]
    archive_attempts: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CameraPreRollStats {
    pub buffers: usize,
    pub ready_buffers: usize,
    pub total_packets: usize,
    pub total_bytes: usize,
}

#[derive(Debug)]
pub struct PreRollWriteResult {
    pub from: DateTime<Utc>,
    pub duration_s: i64,
    pub partial: bool,
    pub warning: Option<String>,
}

struct PreRollBufferInner {
    active: bool,
    target_seconds: u32,
    max_bytes: usize,
    parameters: Option<codec::Parameters>,
    input_time_base: Option<Rational>,
    fallback_packet_duration: i64,
    packets: VecDeque<BufferedPacket>,
    bytes: usize,
    last_packet_at: Option<Instant>,
    last_error: Option<String>,
}

impl Default for PreRollBufferInner {
    fn default() -> Self {
        Self {
            target_seconds: db::camera_recording_rules::DEFAULT_PRE_ROLL_SECONDS as u32,
            active: false,
            max_bytes: 0,
            parameters: None,
            input_time_base: None,
            fallback_packet_duration: 1,
            packets: VecDeque::new(),
            bytes: 0,
            last_packet_at: None,
            last_error: None,
        }
    }
}

struct BufferedPacket {
    packet: ffmpeg::Packet,
    captured_at: Instant,
    captured_wall_at: DateTime<Utc>,
    size: usize,
}

fn retire_buffer(buffer: &mut PreRollBufferInner) {
    buffer.active = false;
    buffer.max_bytes = 0;
    buffer.packets.clear();
    buffer.bytes = 0;
    buffer.parameters = None;
}

#[derive(Clone)]
struct PreRollWriter {
    inner: Arc<Mutex<PreRollBufferInner>>,
}

impl PreRollWriter {
    fn limits(&self) -> (u32, usize) {
        self.inner
            .lock()
            .map(|b| (b.target_seconds, b.max_bytes))
            .unwrap_or_default()
    }

    fn configure(&self, parameters: codec::Parameters, time_base: Rational, duration: i64) {
        let Ok(mut buffer) = self.inner.lock() else {
            return;
        };
        if !buffer.active {
            return;
        }
        // A reconnect may change codec metadata, never the live quota/window.
        buffer.parameters = Some(parameters);
        buffer.input_time_base = Some(time_base);
        buffer.fallback_packet_duration = duration.max(1);
        buffer.last_error = None;
        // Packets from the previous RTSP connection may have different codec
        // parameters or reset timestamps; do not mix the two streams.
        buffer.packets.clear();
        buffer.bytes = 0;
        buffer.last_packet_at = None;
    }

    fn push_packet(&self, packet: ffmpeg::Packet) {
        let Ok(mut buffer) = self.inner.lock() else {
            return;
        };
        if !buffer.active || packet.size() > buffer.max_bytes {
            return;
        }
        let now = Instant::now();
        let size = packet.size();
        // Evict before retaining the incoming packet so retained bytes never
        // exceed this buffer's quota, even while another owner is pushing.
        while buffer.bytes > buffer.max_bytes - size {
            if let Some(old) = buffer.packets.pop_front() {
                buffer.bytes -= old.size;
            } else {
                break;
            }
        }
        buffer.bytes += size;
        buffer.packets.push_back(BufferedPacket {
            packet,
            captured_at: now,
            captured_wall_at: Utc::now(),
            size,
        });
        buffer.last_packet_at = Some(now);
        buffer.last_error = None;
        prune_buffer(&mut buffer, now);
    }

    fn mark_error(&self, error: &str) {
        let Ok(mut buffer) = self.inner.lock() else {
            return;
        };
        if buffer.active {
            buffer.last_error = Some(error.to_owned());
        }
    }
}

struct SelectedPacket {
    packet: ffmpeg::Packet,
    captured_at: Instant,
    captured_wall_at: DateTime<Utc>,
}

struct PreRollOwner {
    cancel: CancellationToken,
    thread_id: thread::ThreadId,
}

enum PreRollOutput {
    Interactive {
        path: PathBuf,
        owner: Option<crate::core::cameras::TemporaryMedia>,
    },
    Archive(PathBuf),
}

impl CameraPreRollRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn archive_attempts_for_test(&self) -> usize {
        self.archive_attempts
            .load(std::sync::atomic::Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn clear_history_for_test(&self) {
        self.buffers.clear();
    }

    pub fn stats(&self) -> CameraPreRollStats {
        let mut stats = CameraPreRollStats {
            buffers: self.buffers.len(),
            ..CameraPreRollStats::default()
        };

        for entry in self.buffers.iter() {
            let Ok(buffer) = entry.value().lock() else {
                continue;
            };
            stats.total_packets += buffer.packets.len();
            stats.total_bytes += buffer.bytes;
            if buffer.parameters.is_some()
                && !buffer.packets.is_empty()
                && buffer.last_error.is_none()
            {
                stats.ready_buffers += 1;
            }
        }

        stats
    }

    pub fn has_ready_buffer(&self, camera_id: i64) -> bool {
        self.ready_window_seconds(camera_id).is_some()
    }

    pub fn ready_window_seconds(&self, camera_id: i64) -> Option<u32> {
        let buffer = self.buffers.get(&camera_id)?;
        let Ok(buffer) = buffer.lock() else {
            return None;
        };

        if buffer.parameters.is_none() || buffer.packets.is_empty() || buffer.last_error.is_some() {
            return None;
        }

        Some(
            buffer
                .target_seconds
                .saturating_sub(BUFFER_KEYFRAME_GRACE_S),
        )
    }

    // Validate the entire candidate first. Shrink all existing buffers before
    // allowing any buffer to grow or a new owner to retain packets.
    fn apply_limits(&self, targets: &[(i64, u32)], total: usize, per_camera: usize) -> Result<()> {
        anyhow::ensure!(
            total > 0 && per_camera > 0,
            "Pre-roll budgets must be positive"
        );
        let quota = per_camera.min(total / targets.len().max(1));
        anyhow::ensure!(targets.is_empty() || quota > 0, "Pre-roll budget cannot give every camera a positive quota; keeping previous configuration");
        let active_ids = targets.iter().map(|(id, _)| *id).collect::<HashSet<_>>();
        anyhow::ensure!(
            active_ids.len() == targets.len(),
            "Duplicate pre-roll target"
        );
        self.retain_active_buffers(&active_ids);
        for entry in self.buffers.iter() {
            let mut buffer = entry
                .lock()
                .map_err(|_| anyhow!("Pre-roll buffer lock poisoned"))?;
            buffer.max_bytes = buffer.max_bytes.min(quota);
            prune_buffer(&mut buffer, Instant::now());
        }
        for &(id, seconds) in targets {
            let entry = self
                .buffers
                .entry(id)
                .or_insert_with(|| Arc::new(Mutex::new(PreRollBufferInner::default())));
            let mut buffer = entry
                .lock()
                .map_err(|_| anyhow!("Pre-roll buffer lock poisoned"))?;
            buffer.active = true;
            buffer.target_seconds = seconds.saturating_add(BUFFER_KEYFRAME_GRACE_S);
            buffer.max_bytes = quota;
            prune_buffer(&mut buffer, Instant::now());
        }
        Ok(())
    }

    // Each owner gets a separate generation. Detached/stale owners can only
    // access their retired allocation, which has a zero quota and rejects writes.
    fn new_writer(&self, camera_id: i64) -> Result<PreRollWriter> {
        let mut entry = self
            .buffers
            .get_mut(&camera_id)
            .context("Pre-roll camera not admitted")?;
        let replacement = {
            let mut previous = entry
                .lock()
                .map_err(|_| anyhow!("Pre-roll buffer lock poisoned"))?;
            let next = PreRollBufferInner {
                active: true,
                max_bytes: previous.max_bytes,
                target_seconds: previous.target_seconds,
                ..PreRollBufferInner::default()
            };
            retire_buffer(&mut previous);
            Arc::new(Mutex::new(next))
        };
        *entry = replacement.clone();
        Ok(PreRollWriter { inner: replacement })
    }

    fn retain_active_buffers(&self, active_ids: &HashSet<i64>) {
        let stale = self
            .buffers
            .iter()
            .filter(|entry| !active_ids.contains(entry.key()))
            .map(|entry| *entry.key())
            .collect::<Vec<_>>();

        for camera_id in stale {
            if let Some((_, buffer)) = self.buffers.remove(&camera_id) {
                if let Ok(mut buffer) = buffer.lock() {
                    retire_buffer(&mut buffer);
                }
            }
        }
    }

    pub async fn write_to_file(
        &self,
        camera_id: i64,
        seconds: u32,
        trigger_at: DateTime<Utc>,
        end_at: DateTime<Utc>,
        output_path: PathBuf,
        owner: Option<crate::core::cameras::TemporaryMedia>,
    ) -> Result<PreRollWriteResult> {
        self.write_selected_to_file(
            camera_id,
            seconds,
            trigger_at,
            end_at,
            PreRollOutput::Interactive {
                path: output_path,
                owner,
            },
        )
        .await
    }

    pub async fn write_archive_to_file(
        &self,
        camera_id: i64,
        seconds: u32,
        trigger_at: DateTime<Utc>,
        end_at: DateTime<Utc>,
        output_path: PathBuf,
    ) -> Result<PreRollWriteResult> {
        self.write_selected_to_file(
            camera_id,
            seconds,
            trigger_at,
            end_at,
            PreRollOutput::Archive(output_path),
        )
        .await
    }

    async fn write_selected_to_file(
        &self,
        camera_id: i64,
        seconds: u32,
        trigger_at: DateTime<Utc>,
        end_at: DateTime<Utc>,
        output: PreRollOutput,
    ) -> Result<PreRollWriteResult> {
        let (output_path, owner, archive) = match output {
            PreRollOutput::Interactive { path, owner } => (path, owner, false),
            PreRollOutput::Archive(path) => (path, None, true),
        };
        #[cfg(test)]
        if archive {
            self.archive_attempts
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
        // Clone the selected packet references before waiting for native capacity.
        // Buffer eviction or owner replacement cannot discard this accepted head.
        let (parameters, input_time_base, fallback_duration, mut packets, partial_from_cache) =
            self.select_packets(camera_id, seconds, trigger_at, end_at)?;
        let from = packets
            .first()
            .map(|packet| packet.captured_wall_at)
            .unwrap_or(trigger_at - ChronoDuration::seconds(i64::from(seconds)));

        let output_for_log = output_path.display().to_string();
        let write_job = move || {
            let _owner = owner;
            write_packets_to_mp4(
                &output_path,
                parameters,
                input_time_base,
                fallback_duration,
                &mut packets,
            )
        };
        let write_result = if archive {
            // The caller holds a writer lease before retaining these packets:
            // even detached error paths share the global 64-writer bound.
            // Dedicated mux capacity waits without expiring at the interactive
            // 2s threshold, and cannot compete with the main camera capture.
            crate::core::cameras::run_archive_pre_roll_job(write_job).await
        } else {
            crate::core::cameras::run_local_media_job(write_job).await
        }
        .context("pre-roll writer task failed")?;

        debug!(
            "Camera pre-roll segment written: camera={}, output={}, duration={}s, packets={}",
            camera_id, output_for_log, write_result.duration_s, write_result.packet_count
        );

        Ok(PreRollWriteResult {
            from,
            duration_s: write_result.duration_s,
            partial: partial_from_cache || write_result.partial,
            warning: write_result.warning,
        })
    }

    fn select_packets(
        &self,
        camera_id: i64,
        seconds: u32,
        trigger_at: DateTime<Utc>,
        end_at: DateTime<Utc>,
    ) -> Result<(codec::Parameters, Rational, i64, Vec<SelectedPacket>, bool)> {
        let buffer = self
            .buffers
            .get(&camera_id)
            .context("pre-roll camera is not active")?;
        let buffer = buffer
            .lock()
            .map_err(|_| anyhow!("pre-roll buffer lock poisoned"))?;

        if let Some(error) = buffer.last_error.as_deref() {
            return Err(anyhow!("pre-roll owner error: {}", error));
        }

        let parameters = buffer
            .parameters
            .clone()
            .ok_or_else(|| anyhow!("pre-roll owner is still warming up"))?;
        let input_time_base = buffer
            .input_time_base
            .ok_or_else(|| anyhow!("pre-roll input time base missing"))?;
        let fallback_duration = buffer.fallback_packet_duration;
        let requested_start_at = trigger_at - ChronoDuration::seconds(i64::from(seconds));
        let min_wall_at =
            requested_start_at - ChronoDuration::seconds(i64::from(BUFFER_KEYFRAME_GRACE_S));
        let mut packets = buffer
            .packets
            .iter()
            .filter(|packet| {
                packet.captured_wall_at >= min_wall_at && packet.captured_wall_at <= end_at
            })
            .map(|packet| SelectedPacket {
                packet: packet.packet.clone(),
                captured_at: packet.captured_at,
                captured_wall_at: packet.captured_wall_at,
            })
            .collect::<Vec<_>>();

        if packets.is_empty() {
            return Err(anyhow!("pre-roll buffer has no packets yet"));
        }

        let start_index = packets
            .iter()
            .enumerate()
            .filter(|(_, packet)| {
                packet.packet.is_key() && packet.captured_wall_at <= requested_start_at
            })
            .map(|(index, _)| index)
            .next_back()
            .or_else(|| packets.iter().position(|packet| packet.packet.is_key()))
            .ok_or_else(|| anyhow!("pre-roll buffer has no keyframe in requested window"))?;
        let partial = packets[start_index].captured_wall_at > requested_start_at;
        if start_index > 0 {
            packets.drain(..start_index);
        }

        Ok((
            parameters,
            input_time_base,
            fallback_duration,
            packets,
            partial,
        ))
    }
}

pub fn spawn_camera_pre_roll_worker(
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut owners: HashMap<i64, PreRollOwner> = HashMap::new();
        let mut interval = tokio::time::interval(OWNER_RECONCILE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        info!("Core: Camera pre-roll worker started");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = reconcile_pre_roll_owners(&config, &mut owners, &cancel_token).await {
                        error!("Camera pre-roll reconcile failed: {}", error);
                    }
                }
            }
        }

        for (camera_id, owner) in owners {
            stop_owner(camera_id, owner);
        }
        info!("Core: Camera pre-roll worker stopped");
    })
}

async fn reconcile_pre_roll_owners(
    config: &Arc<AppConfig>,
    owners: &mut HashMap<i64, PreRollOwner>,
    cancel_token: &CancellationToken,
) -> Result<()> {
    let globally_enabled = db::settings::get_i64(db::settings::CAMERA_PRE_ROLL_ENABLED, &config.db)
        .await?
        .unwrap_or(0)
        != 0;
    if !globally_enabled {
        stop_removed_owners(
            owners,
            &HashSet::new(),
            config.camera_pre_roll_registry.as_ref(),
        );
        return Ok(());
    }

    let targets = db::cameras::list_pre_roll_cameras(&config.db).await?;
    let active_ids = targets
        .iter()
        .map(|target| target.camera.id)
        .collect::<HashSet<_>>();
    let total_limit = read_buffer_limit(
        db::settings::CAMERA_PRE_ROLL_MAX_TOTAL_BUFFER_BYTES,
        DEFAULT_MAX_TOTAL_BYTES,
        &config.db,
    )
    .await?;
    let configured_per_camera = read_buffer_limit(
        db::settings::CAMERA_PRE_ROLL_MAX_BUFFER_BYTES_PER_CAMERA,
        DEFAULT_MAX_BYTES_PER_CAMERA,
        &config.db,
    )
    .await?;
    let windows = targets
        .iter()
        .map(|target| {
            (
                target.camera.id,
                target
                    .pre_roll_seconds
                    .clamp(1, db::camera_recording_rules::MAX_PRE_ROLL_SECONDS)
                    as u32,
            )
        })
        .collect::<Vec<_>>();
    config
        .camera_pre_roll_registry
        .apply_limits(&windows, total_limit, configured_per_camera)?;
    stop_removed_owners(
        owners,
        &active_ids,
        config.camera_pre_roll_registry.as_ref(),
    );

    for target in targets {
        let should_restart = owners
            .get(&target.camera.id)
            .is_some_and(|owner| crate::core::cameras::native_thread_finished(owner.thread_id));
        if should_restart {
            if let Some(owner) = owners.remove(&target.camera.id) {
                stop_owner(target.camera.id, owner);
            }
        }

        if owners.contains_key(&target.camera.id) {
            continue;
        }

        let capacity = OWNER_CAPACITY
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_PRE_ROLL_OWNERS)))
            .clone();
        let Ok(permit) = capacity.try_acquire_owned() else {
            warn!(
                "Pre-roll native owner capacity exhausted: max={}",
                MAX_PRE_ROLL_OWNERS
            );
            continue;
        };
        let owner_cancel = cancel_token.child_token();
        let writer = config
            .camera_pre_roll_registry
            .new_writer(target.camera.id)?;
        let camera = target.camera.clone();
        let thread_id = crate::core::cameras::spawn_native_thread(
            format!("telegram-ha-preroll-{}", target.camera.id),
            owner_cancel.clone(),
            {
                let owner_cancel = owner_cancel.clone();
                move || {
                    let _permit = permit;
                    crate::core::cameras::with_native_control(owner_cancel.clone(), None, || {
                        run_pre_roll_owner(camera, writer, owner_cancel)
                    })
                }
            },
        )?;
        owners.insert(
            target.camera.id,
            PreRollOwner {
                cancel: owner_cancel,
                thread_id,
            },
        );
    }

    Ok(())
}

async fn read_buffer_limit(key: &str, default: usize, pool: &sqlx::SqlitePool) -> Result<usize> {
    let value = db::settings::get_i64(key, pool)
        .await?
        .unwrap_or(default as i64);
    anyhow::ensure!(
        value > 0,
        "Pre-roll setting {} must be positive; keeping previous configuration",
        key
    );
    usize::try_from(value).context("Pre-roll budget exceeds platform capacity")
}

fn stop_removed_owners(
    owners: &mut HashMap<i64, PreRollOwner>,
    active_ids: &HashSet<i64>,
    registry: &CameraPreRollRegistry,
) {
    let removed = owners
        .keys()
        .copied()
        .filter(|camera_id| !active_ids.contains(camera_id))
        .collect::<Vec<_>>();

    for camera_id in removed {
        if let Some(owner) = owners.remove(&camera_id) {
            stop_owner(camera_id, owner);
        }
    }
    registry.retain_active_buffers(active_ids);
}

fn stop_owner(camera_id: i64, owner: PreRollOwner) {
    owner.cancel.cancel();
    debug!(
        "Camera pre-roll owner cancellation requested: camera={}",
        camera_id
    );
}

fn run_pre_roll_owner(
    camera: db::cameras::Camera,
    writer: PreRollWriter,
    cancel_token: CancellationToken,
) {
    while !cancel_token.is_cancelled() {
        match run_pre_roll_owner_once(&camera, &writer, &cancel_token) {
            Ok(()) => {}
            Err(error) => {
                let message = crate::core::cameras::sanitize_camera_error(&error);
                warn!(
                    "Camera pre-roll owner failed: camera={} {}, error={}",
                    camera.id, camera.name, message
                );
                writer.mark_error(&message);
                let retry_at = Instant::now() + OWNER_RESTART_DELAY;
                while !cancel_token.is_cancelled() && Instant::now() < retry_at {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}

fn run_pre_roll_owner_once(
    camera: &db::cameras::Camera,
    writer: &PreRollWriter,
    cancel_token: &CancellationToken,
) -> Result<()> {
    crate::core::cameras::init_ffmpeg();

    let (target_seconds, max_bytes) = writer.limits();
    info!(
        "Camera pre-roll owner connecting: camera={} {}, window={}s, limit={} bytes, stream={}",
        camera.id,
        camera.name,
        target_seconds,
        max_bytes,
        crate::core::cameras::safe_stream_label(&camera.stream_url)
    );
    let mut input_ctx = crate::core::cameras::open_camera_input(&camera.stream_url)?;
    let input_stream = input_ctx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("В потоке камеры нет видео"))?;
    let input_stream_index = input_stream.index();
    let input_time_base = input_stream.time_base();
    let fallback_packet_duration =
        frame_duration_ticks(input_stream.avg_frame_rate(), input_time_base)
            .or_else(|| frame_duration_ticks(input_stream.rate(), input_time_base))
            .unwrap_or_else(|| {
                frame_duration_ticks(Rational::new(30, 1), input_time_base).unwrap_or(1)
            });
    let parameters = input_stream.parameters();

    writer.configure(parameters, input_time_base, fallback_packet_duration);

    while let Some(packet) = crate::core::cameras::read_packet(&mut input_ctx)? {
        if cancel_token.is_cancelled() {
            return Ok(());
        }
        if packet.stream() != input_stream_index {
            continue;
        }
        writer.push_packet(packet);
    }

    Err(anyhow!("pre-roll owner stream ended"))
}

struct PacketWriteResult {
    duration_s: i64,
    packet_count: usize,
    partial: bool,
    warning: Option<String>,
}

fn write_packets_to_mp4(
    output_path: &Path,
    parameters: codec::Parameters,
    input_time_base: Rational,
    fallback_packet_duration: i64,
    packets: &mut [SelectedPacket],
) -> Result<PacketWriteResult> {
    if packets.is_empty() {
        return Err(anyhow!("pre-roll packet list is empty"));
    }
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Не удалось создать папку {}", parent.display()))?;
    }

    let output_path_str = output_path.to_string_lossy().to_string();
    let mut output_ctx =
        format::output(&output_path_str).context("Не удалось открыть MP4 output")?;
    let output_stream_index = {
        let mut output_stream = output_ctx
            .add_stream(encoder::find(codec::Id::None))
            .context("Не удалось создать output stream")?;
        output_stream.set_parameters(parameters);
        unsafe {
            (*output_stream.parameters().as_mut_ptr()).codec_tag = 0;
        }
        output_stream.index()
    };

    let mut header_options = Dictionary::new();
    header_options.set("movflags", "+faststart");
    output_ctx
        .write_header_with(header_options)
        .context("Не удалось записать MP4 header")?;
    let output_time_base = output_ctx
        .stream(output_stream_index)
        .ok_or_else(|| anyhow!("Output stream missing"))?
        .time_base();

    let started_at = packets.first().map(|packet| packet.captured_at);
    let ended_at = packets.last().map(|packet| packet.captured_at);
    let base_ts = packets
        .iter()
        .find_map(|packet| packet.packet.dts().or_else(|| packet.packet.pts()))
        .unwrap_or(0);
    let mut next_synthetic_pts = 0i64;
    let mut wrote_packets = 0usize;

    for selected in packets {
        crate::core::cameras::check_native_cancelled()?;
        let mut packet = selected.packet.clone();
        if let Some(pts) = packet.pts() {
            packet.set_pts(Some(pts.saturating_sub(base_ts).max(0)));
        }
        if let Some(dts) = packet.dts() {
            packet.set_dts(Some(dts.saturating_sub(base_ts).max(0)));
        }
        normalize_packet_timestamps(
            &mut packet,
            &mut next_synthetic_pts,
            fallback_packet_duration,
        );

        packet.rescale_ts(input_time_base, output_time_base);
        packet.set_position(-1);
        packet.set_stream(output_stream_index);
        packet
            .write_interleaved(&mut output_ctx)
            .context("Не удалось записать pre-roll packet в MP4")?;
        wrote_packets += 1;
    }

    if wrote_packets == 0 {
        return Err(anyhow!("pre-roll writer did not write packets"));
    }
    output_ctx
        .write_trailer()
        .context("Не удалось записать MP4 trailer")?;

    let duration_s = match (started_at, ended_at) {
        (Some(started_at), Some(ended_at)) => {
            let millis = ended_at.saturating_duration_since(started_at).as_millis();
            i64::try_from((millis / 1000).max(1)).unwrap_or(i64::MAX)
        }
        _ => 1,
    };

    Ok(PacketWriteResult {
        duration_s,
        packet_count: wrote_packets,
        partial: false,
        warning: None,
    })
}

fn prune_buffer(buffer: &mut PreRollBufferInner, now: Instant) {
    let max_age = Duration::from_secs(u64::from(buffer.target_seconds.max(1)));
    while buffer
        .packets
        .front()
        .is_some_and(|packet| now.saturating_duration_since(packet.captured_at) > max_age)
    {
        if let Some(packet) = buffer.packets.pop_front() {
            buffer.bytes = buffer.bytes.saturating_sub(packet.size);
        }
    }

    while buffer.bytes > buffer.max_bytes {
        if let Some(packet) = buffer.packets.pop_front() {
            buffer.bytes = buffer.bytes.saturating_sub(packet.size);
        } else {
            break;
        }
    }
}

fn normalize_packet_timestamps(
    packet: &mut ffmpeg::Packet,
    next_synthetic_pts: &mut i64,
    fallback_duration: i64,
) {
    let duration = if packet.duration() > 0 {
        packet.duration()
    } else {
        fallback_duration
    };

    match (packet.pts(), packet.dts()) {
        (Some(pts), Some(dts)) => {
            *next_synthetic_pts = pts.max(dts).saturating_add(duration);
        }
        (Some(pts), None) => {
            packet.set_dts(Some(pts));
            *next_synthetic_pts = pts.saturating_add(duration);
        }
        (None, Some(dts)) => {
            packet.set_pts(Some(dts));
            *next_synthetic_pts = dts.saturating_add(duration);
        }
        (None, None) => {
            let pts = *next_synthetic_pts;
            packet.set_pts(Some(pts));
            packet.set_dts(Some(pts));
            *next_synthetic_pts = next_synthetic_pts.saturating_add(duration);
        }
    }

    if packet.duration() <= 0 {
        packet.set_duration(duration);
    }
}

fn frame_duration_ticks(frame_rate: Rational, time_base: Rational) -> Option<i64> {
    if frame_rate.numerator() <= 0
        || frame_rate.denominator() <= 0
        || time_base.numerator() <= 0
        || time_base.denominator() <= 0
    {
        return None;
    }

    let numerator = i64::from(frame_rate.denominator()) * i64::from(time_base.denominator());
    let denominator = i64::from(frame_rate.numerator()) * i64::from(time_base.numerator());

    Some((numerator / denominator).max(1))
}

#[cfg(test)]
pub(crate) fn prepared_registry_for_test() -> Result<CameraPreRollRegistry> {
    tests::buffered_jpeg_registry()
}

#[cfg(test)]
pub(crate) fn prepared_history_for_test(
    trigger: DateTime<Utc>,
    seconds: i64,
) -> Result<CameraPreRollRegistry> {
    let registry = tests::buffered_jpeg_registry()?;
    {
        let entry = registry.buffers.get(&1).unwrap();
        let mut buffer = entry.lock().unwrap();
        let source = buffer.packets.front().unwrap().packet.clone();
        buffer.packets.clear();
        buffer.bytes = 0;
        buffer.input_time_base = Some(Rational::new(1, 1));
        buffer.fallback_packet_duration = 1;
        for index in 0..=seconds {
            let mut packet = source.clone();
            packet.set_pts(Some(index));
            packet.set_dts(Some(index));
            packet.set_duration(1);
            let size = packet.size();
            buffer.packets.push_back(BufferedPacket {
                packet,
                size,
                captured_at: Instant::now() - Duration::from_secs((seconds - index) as u64),
                captured_wall_at: trigger - ChronoDuration::seconds(seconds - index),
            });
            buffer.bytes += size;
        }
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MIB: usize = 1024 * 1024;
    fn packet(size: usize) -> ffmpeg::Packet {
        ffmpeg::Packet::copy(&vec![0; size])
    }

    #[test]
    fn live_quotas_shrink_existing_buffers_without_replacing_owners() -> Result<()> {
        let registry = CameraPreRollRegistry::new();
        registry.apply_limits(&[(1, 15)], 8 * MIB, 8 * MIB)?;
        let one = registry.new_writer(1)?;
        for _ in 0..8 {
            one.push_packet(packet(MIB));
        }
        assert_eq!(registry.stats().total_bytes, 8 * MIB);
        registry.apply_limits(&[(1, 15), (2, 15)], 8 * MIB, 8 * MIB)?;
        assert_eq!(one.limits().1, 4 * MIB);
        let two = registry.new_writer(2)?;
        for _ in 0..8 {
            two.push_packet(packet(MIB));
        }
        assert_eq!(registry.stats().total_bytes, 8 * MIB);
        registry.apply_limits(&[(1, 30), (2, 30)], 4 * MIB, 8 * MIB)?;
        assert_eq!(registry.stats().total_bytes, 4 * MIB);
        assert_eq!(one.limits(), (35, 2 * MIB));
        one.configure(codec::Parameters::new(), Rational::new(1, 90000), 3000);
        assert_eq!(one.limits(), (35, 2 * MIB));
        one.push_packet(packet(3 * MIB));
        assert_eq!(one.inner.lock().unwrap().bytes, 0);
        one.push_packet(packet(MIB));
        assert_eq!(one.inner.lock().unwrap().bytes, MIB);
        Ok(())
    }

    #[test]
    fn invalid_candidate_preserves_configuration_and_small_budgets_cover_all_cameras() -> Result<()>
    {
        let registry = CameraPreRollRegistry::new();
        registry.apply_limits(&[(1, 15), (2, 15)], 2, 10)?;
        let one = registry.new_writer(1)?;
        let two = registry.new_writer(2)?;
        one.push_packet(packet(1));
        two.push_packet(packet(1));
        for total in [0, 1] {
            assert!(registry
                .apply_limits(&[(1, 15), (2, 15), (3, 15)], total, 10)
                .is_err());
            assert_eq!(registry.stats().buffers, 2);
            assert_eq!(registry.stats().total_bytes, 2);
        }
        assert!(registry.apply_limits(&[(1, 15)], 2, 0).is_err());
        Ok(())
    }

    #[test]
    fn retired_and_replaced_owners_cannot_resurrect_buffers() -> Result<()> {
        let registry = CameraPreRollRegistry::new();
        registry.apply_limits(&[(1, 15)], MIB, MIB)?;
        let old = registry.new_writer(1)?;
        old.push_packet(packet(MIB));
        let replacement = registry.new_writer(1)?;
        old.configure(codec::Parameters::new(), Rational::new(1, 1), 1);
        old.push_packet(packet(MIB));
        old.mark_error("old owner");
        assert_eq!(old.inner.lock().unwrap().bytes, 0);
        assert!(replacement.inner.lock().unwrap().last_error.is_none());
        registry.apply_limits(&[], MIB, MIB)?;
        replacement.push_packet(packet(MIB));
        assert_eq!(registry.stats().buffers, 0);
        assert_eq!(replacement.inner.lock().unwrap().bytes, 0);
        assert!(registry.new_writer(1).is_err());
        Ok(())
    }

    #[test]
    fn concurrent_packet_writes_obey_updated_budget() -> Result<()> {
        let registry = CameraPreRollRegistry::new();
        registry.apply_limits(&[(1, 15), (2, 15)], 8 * MIB, 8 * MIB)?;
        let writers = [registry.new_writer(1)?, registry.new_writer(2)?];
        let joins = writers
            .into_iter()
            .map(|writer| {
                thread::spawn(move || {
                    for _ in 0..1000 {
                        writer.push_packet(packet(4096));
                    }
                })
            })
            .collect::<Vec<_>>();
        for _ in 0..30 {
            registry.apply_limits(&[(1, 15), (2, 15)], MIB, 8 * MIB)?;
            assert!(registry.stats().total_bytes <= MIB);
        }
        for join in joins {
            join.join().unwrap();
        }
        assert!(registry.stats().total_bytes <= MIB);
        Ok(())
    }
    pub(super) fn buffered_jpeg_registry() -> Result<CameraPreRollRegistry> {
        crate::core::cameras::init_ffmpeg();
        let registry = CameraPreRollRegistry::new();
        registry.apply_limits(&[(1, 5)], MIB, MIB)?;
        let writer = registry.new_writer(1)?;
        let mut parameters = codec::Parameters::new();
        unsafe {
            let parameters = &mut *parameters.as_mut_ptr();
            parameters.codec_type = ffmpeg::ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
            parameters.codec_id = ffmpeg::ffi::AVCodecID::AV_CODEC_ID_MJPEG;
            parameters.width = 16;
            parameters.height = 16;
            parameters.format = ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_YUVJ420P as i32;
        }
        writer.configure(parameters, Rational::new(1, 10), 1);
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode(
            &vec![80_u8; 16 * 16 * 3],
            16,
            16,
            image::ColorType::Rgb8.into(),
        )?;
        for index in 0..3 {
            let mut packet = ffmpeg::Packet::copy(&jpeg);
            packet.set_flags(ffmpeg::packet::Flags::KEY);
            packet.set_pts(Some(index));
            packet.set_dts(Some(index));
            packet.set_duration(1);
            writer.push_packet(packet);
        }
        Ok(registry)
    }

    #[tokio::test]
    async fn archive_pre_roll_survives_busy_compression_and_retains_queued_packets() -> Result<()> {
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let registry = buffered_jpeg_registry()?;
        let output = crate::core::cameras::TemporaryMedia::new()?;
        let local_capacity = crate::core::cameras::reserve_local_media_capacity_for_test().await?;
        let now = Utc::now();
        let written = tokio::time::timeout(
            Duration::from_secs(1),
            registry.write_archive_to_file(1, 1, now, now, output.path()),
        )
        .await??;
        assert_eq!(written.duration_s, 1);
        assert!(std::fs::metadata(output.path())?.len() > 0);
        drop(local_capacity);

        // With both archival mux slots also busy, retain the selected packet
        // references beyond the old 2s timeout, even if the ring owner changes.
        let recording_capacity =
            crate::core::cameras::reserve_archive_pre_roll_capacity_for_test().await?;
        let queued_output = crate::core::cameras::TemporaryMedia::new()?;
        let pending = registry.write_archive_to_file(1, 1, now, now, queued_output.path());
        tokio::pin!(pending);
        tokio::select! {
            result = &mut pending => panic!("archive pre-roll unexpectedly finished without an archival mux permit: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(2100)) => {}
        }
        registry.new_writer(1)?;
        drop(recording_capacity);
        let written = tokio::time::timeout(Duration::from_secs(1), &mut pending).await??;
        assert_eq!(written.duration_s, 1);
        let mut input =
            crate::core::cameras::open_camera_input(queued_output.path().to_str().unwrap())?;
        let mut count = 0;
        while crate::core::cameras::read_packet(&mut input)?.is_some() {
            count += 1;
        }
        assert_eq!(
            count, 3,
            "queued packet references must survive buffer replacement"
        );
        Ok(())
    }
}
