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
use std::thread::{self, JoinHandle as ThreadJoinHandle};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const OWNER_RECONCILE_INTERVAL: Duration = Duration::from_secs(10);
const OWNER_RESTART_DELAY: Duration = Duration::from_secs(5);
const OWNER_STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(6);
const OWNER_JOIN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const BUFFER_KEYFRAME_GRACE_S: u32 = 5;
const DEFAULT_MAX_BYTES_PER_CAMERA: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct CameraPreRollRegistry {
    buffers: Arc<DashMap<i64, Arc<Mutex<PreRollBufferInner>>>>,
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
    join: ThreadJoinHandle<()>,
}

impl CameraPreRollRegistry {
    pub fn new() -> Self {
        Self::default()
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
    ) -> Result<PreRollWriteResult> {
        let (parameters, input_time_base, fallback_duration, mut packets, partial_from_cache) =
            self.select_packets(camera_id, seconds, trigger_at, end_at)?;
        let from = packets
            .first()
            .map(|packet| packet.captured_wall_at)
            .unwrap_or(trigger_at - ChronoDuration::seconds(i64::from(seconds)));

        let output_for_log = output_path.display().to_string();
        let write_result = tokio::task::spawn_blocking(move || {
            write_packets_to_mp4(
                &output_path,
                parameters,
                input_time_base,
                fallback_duration,
                &mut packets,
            )
        })
        .await
        .context("pre-roll writer task failed")??;

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
            .is_some_and(|owner| owner.join.is_finished());
        if should_restart {
            if let Some(owner) = owners.remove(&target.camera.id) {
                stop_owner(target.camera.id, owner);
            }
        }

        if owners.contains_key(&target.camera.id) {
            continue;
        }

        let owner_cancel = cancel_token.child_token();
        let writer = config
            .camera_pre_roll_registry
            .new_writer(target.camera.id)?;
        let camera = target.camera.clone();
        let join = thread::Builder::new()
            .name(format!("telegram-ha-preroll-{}", target.camera.id))
            .spawn({
                let owner_cancel = owner_cancel.clone();
                move || run_pre_roll_owner(camera, writer, owner_cancel)
            })
            .context("failed to start camera pre-roll owner thread")?;
        owners.insert(
            target.camera.id,
            PreRollOwner {
                cancel: owner_cancel,
                join,
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
    thread::spawn(move || {
        let deadline = Instant::now() + OWNER_STOP_JOIN_TIMEOUT;
        while !owner.join.is_finished() && Instant::now() < deadline {
            thread::sleep(OWNER_JOIN_POLL_INTERVAL);
        }

        if owner.join.is_finished() {
            if owner.join.join().is_err() {
                error!(
                    "Camera pre-roll owner thread panicked: camera={}",
                    camera_id
                );
            }
        } else {
            warn!(
                "Camera pre-roll owner did not stop within {:?}; detaching thread: camera={}",
                OWNER_STOP_JOIN_TIMEOUT, camera_id
            );
        }
    });
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
                std::thread::sleep(OWNER_RESTART_DELAY);
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

    for (stream, packet) in input_ctx.packets() {
        if cancel_token.is_cancelled() {
            return Ok(());
        }
        if stream.index() != input_stream_index {
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
}
