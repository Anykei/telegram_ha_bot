use crate::bot::utils::md;
use crate::db;
use crate::models::{AppConfig, NotificationData};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Duration, Utc};
use image::codecs::jpeg::JpegEncoder;
use image::ColorType;
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;
use teloxide::Bot;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::{JoinError, JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

tokio::task_local! {
    static FILE_WRITER: WriterLease;
}

#[derive(Clone)]
pub(crate) struct WriterLease(Arc<WriterLeaseInner>);

struct WriterLeaseInner {
    id: i64,
    pool: sqlx::SqlitePool,
    runtime: tokio::runtime::Handle,
    released: std::sync::atomic::AtomicBool,
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

static ACTIVE_WRITER_LEASES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn active_writer_leases() -> usize {
    ACTIVE_WRITER_LEASES.load(std::sync::atomic::Ordering::Acquire)
}

static WRITER_CAPACITY: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

enum WriterAuthorization {
    Segment(i64),
    Derivative {
        source_path: String,
        paths: Vec<String>,
    },
}

impl WriterLease {
    async fn begin(segment: i64, kind: &str, pool: &sqlx::SqlitePool) -> Result<Self> {
        Self::begin_authorized(WriterAuthorization::Segment(segment), kind, pool).await
    }

    async fn begin_derivative(
        source_path: &str,
        kind: &str,
        pool: &sqlx::SqlitePool,
    ) -> Result<Self> {
        Self::begin_authorized(
            WriterAuthorization::Derivative {
                source_path: source_path.to_owned(),
                paths: owned_recording_paths(source_path),
            },
            kind,
            pool,
        )
        .await
    }

    async fn begin_authorized(
        owner: WriterAuthorization,
        kind: &str,
        pool: &sqlx::SqlitePool,
    ) -> Result<Self> {
        let permit = WRITER_CAPACITY
            .get_or_init(|| Arc::new(Semaphore::new(64)))
            .clone()
            .try_acquire_owned()
            .context("Recording writer capacity exhausted")?;
        let pool = pool.clone();
        let kind = kind.to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        // Cancellation during SQLite INSERT must not lose the generated writer ID.
        crate::core::cameras::spawn_media_task(async move {
            let outcome = async {
                let id = match owner {
                    WriterAuthorization::Segment(segment) => {
                        db::camera_recording_segments::begin_writer(segment, &kind, &pool).await?
                    }
                    WriterAuthorization::Derivative { source_path, paths } => {
                        db::camera_recording_segments::begin_derivative_writer(
                            &source_path,
                            &kind,
                            &paths,
                            &pool,
                        )
                        .await?
                    }
                };
                ACTIVE_WRITER_LEASES.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                Ok(Self(Arc::new(WriterLeaseInner {
                    id,
                    pool,
                    runtime: tokio::runtime::Handle::current(),
                    released: std::sync::atomic::AtomicBool::new(false),
                    _permit: Some(permit),
                })))
            }
            .await;
            let _ = tx.send(outcome);
        })?;
        rx.await
            .context("Recording writer registration interrupted")?
    }

    async fn finish(&self) -> Result<()> {
        db::camera_recording_segments::finish_writer(self.0.id, &self.0.pool).await?;
        self.0
            .released
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}

impl Drop for WriterLeaseInner {
    fn drop(&mut self) {
        struct DecrementLease;
        impl Drop for DecrementLease {
            fn drop(&mut self) {
                ACTIVE_WRITER_LEASES.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            }
        }
        let _decrement = DecrementLease;
        if self.released.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let pool = self.pool.clone();
        let id = self.id;
        let permit = self._permit.take();
        // The last lease can be dropped by the native thread. The runtime stays
        // alive until this tracked database release completes at shutdown.
        crate::core::cameras::track_media_task(self.runtime.spawn(async move {
            let _permit = permit;
            if let Err(error) = db::camera_recording_segments::finish_writer(id, &pool).await {
                error!(
                    "Recording writer release failed; recovery will retry: {}",
                    error
                );
            }
        }));
    }
}

pub(crate) fn current_writer_lease() -> Option<WriterLease> {
    FILE_WRITER.try_with(Clone::clone).ok()
}

const RECORDING_SHUTDOWN_TIMEOUT: StdDuration = StdDuration::from_secs(20);
const TELEGRAM_THUMBNAIL_MAX_BYTES: usize = 200_000;
const TELEGRAM_THUMBNAIL_MAX_SIDE: u32 = 320;
const RECORDING_DERIVATIVE_FAILURE_BACKOFF_S: i64 = 300;
const RECORDING_DERIVATIVE_FAILURE_RETENTION_S: i64 = 3600;
const TMP_RECORDING_MAX_AGE: StdDuration = StdDuration::from_secs(30 * 60);

static RECORDING_DERIVATIVE_JOBS: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();
static RECORDING_DERIVATIVE_FAILURES: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, chrono::DateTime<Utc>>>,
> = std::sync::OnceLock::new();

#[derive(Debug, Clone)]
pub struct RecordingJob {
    pub session_id: i64,
}

pub fn spawn_recording_worker(
    mut rx: mpsc::Receiver<RecordingJob>,
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        crate::core::cameras::configure_recording_pool(config.camera_recording_max_parallel_jobs);
        let semaphore = Arc::new(Semaphore::new(config.camera_recording_max_parallel_jobs));
        let mut active_jobs = JoinSet::new();
        info!(
            "Core: Camera recording worker started, parallel jobs={}",
            config.camera_recording_max_parallel_jobs
        );

        let mut accepting_jobs = true;
        loop {
            tokio::select! {
                job = rx.recv(), if accepting_jobs => {
                    match job {
                        Some(job) => {
                            let semaphore = semaphore.clone();
                            let config = config.clone();
                            let bot = bot.clone();
                            let permit = tokio::select! {
                                permit = semaphore.acquire_owned() => {
                                    match permit {
                                        Ok(permit) => permit,
                                        Err(error) => {
                                            error!("Camera recording semaphore closed: {}", error);
                                            continue;
                                        }
                                    }
                                }
                                _ = cancel_token.cancelled() => {
                                    info!("Core: Camera recording worker stopping");
                                    break;
                                }
                            };

                            active_jobs.spawn(async move {
                                let _permit = permit;
                                if let Err(error) = process_recording_job(bot, config, job).await {
                                    error!("Camera recording job failed: {}", error);
                                }
                            });
                        }
                        None => accepting_jobs = false,
                    }
                }
                result = active_jobs.join_next(), if !active_jobs.is_empty() => {
                    if let Some(result) = result {
                        log_recording_task_result(result);
                    }
                }
                _ = cancel_token.cancelled() => {
                    info!("Core: Camera recording worker stopping");
                    break;
                }
                else => {
                    if !accepting_jobs && active_jobs.is_empty() {
                        break;
                    }
                }
            }
        }

        wait_for_recording_jobs(&mut active_jobs).await;
        info!("Core: Camera recording worker stopped");
    })
}

pub(crate) async fn initialize_recording_storage(config: &Arc<AppConfig>) -> Result<()> {
    prepare_recording_storage(Path::new(&config.camera_recording_storage_root)).await?;
    let legacy: Vec<(i64, String)> = sqlx::query_as("SELECT s.id,s.file_path FROM camera_recording_segments s JOIN camera_recording_sessions r ON r.id=s.session_id WHERE s.file_path IS NOT NULL AND s.deleted_at IS NULL AND r.deleted_at IS NULL")
        .fetch_all(&config.db).await?;
    for (segment, path) in legacy {
        register_recording_paths(segment, &path, &config.db).await?;
    }
    // The migration cannot stat legacy previews and compressed sidecars. Reconcile
    // their sizes before quota sees the catalog, in finite database batches.
    let mut cursor = String::new();
    loop {
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM recording_files WHERE path>? ORDER BY path LIMIT 256",
        )
        .bind(&cursor)
        .fetch_all(&config.db)
        .await?;
        if paths.is_empty() {
            break;
        }
        for path in paths {
            if db::camera_recording_segments::safe_relative_path(&path) {
                let size = match tokio::fs::symlink_metadata(
                    Path::new(&config.camera_recording_storage_root).join(&path),
                )
                .await
                {
                    Ok(metadata) => Some(i64::try_from(metadata.len()).unwrap_or(i64::MAX)),
                    Err(error) if error.kind() == ErrorKind::NotFound => Some(0),
                    Err(error) => {
                        warn!("Cannot account recording file {}: {}", path, error);
                        None
                    }
                };
                if let Some(size) = size {
                    sqlx::query("UPDATE recording_files SET size_bytes=? WHERE path=?")
                        .bind(size)
                        .bind(&path)
                        .execute(&config.db)
                        .await?;
                }
            }
            cursor = path;
        }
    }
    recover_stale_recordings(config).await
}

async fn prepare_recording_storage(root: &Path) -> Result<PathBuf> {
    use tokio::io::AsyncWriteExt;
    let absolute = if root.is_absolute() {
        root.to_owned()
    } else {
        std::env::current_dir()?.join(root)
    };
    info!(
        "Camera recording storage resolved to {}",
        absolute.display()
    );
    tokio::fs::create_dir_all(&absolute)
        .await
        .with_context(|| format!("Cannot create recording archive {}", absolute.display()))?;
    let probe = absolute.join(format!(
        ".storage-probe-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .await
        .with_context(|| format!("Recording archive is not writable: {}", absolute.display()))?;
    let result = async {
        file.write_all(b"storage probe").await?;
        file.sync_all().await
    }
    .await;
    drop(file);
    let cleanup = tokio::fs::remove_file(&probe).await;
    result.context("Recording archive write check failed")?;
    cleanup.context("Recording archive delete check failed")?;
    Ok(absolute)
}

async fn wait_for_recording_jobs(active_jobs: &mut JoinSet<()>) {
    if active_jobs.is_empty() {
        return;
    }

    let timeout = tokio::time::sleep(RECORDING_SHUTDOWN_TIMEOUT);
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            result = active_jobs.join_next(), if !active_jobs.is_empty() => {
                if let Some(result) = result {
                    log_recording_task_result(result);
                }

                if active_jobs.is_empty() {
                    return;
                }
            }
            _ = &mut timeout => {
                warn!(
                    "Camera recording worker still has {} active job(s) after {:?}; aborting them before database shutdown",
                    active_jobs.len(),
                    RECORDING_SHUTDOWN_TIMEOUT
                );
                active_jobs.abort_all();

                while let Some(result) = active_jobs.join_next().await {
                    log_recording_task_result(result);
                }
                return;
            }
        }
    }
}

fn log_recording_task_result(result: Result<(), JoinError>) {
    if let Err(error) = result {
        if error.is_cancelled() {
            warn!("Camera recording job was aborted during shutdown");
        } else {
            error!("Camera recording task join failed: {}", error);
        }
    }
}

async fn process_recording_job(bot: Bot, config: Arc<AppConfig>, job: RecordingJob) -> Result<()> {
    let Some(session) =
        db::camera_recording_sessions::get_session(job.session_id, &config.db).await?
    else {
        return Ok(());
    };

    if session.status != "queued" && session.status != "recording" {
        return Ok(());
    }

    let Some(rule_id) = session.rule_id else {
        return Ok(());
    };
    let Some(rule) = db::camera_recording_rules::get_rule(rule_id, &config.db).await? else {
        db::camera_recording_sessions::mark_failed(
            session.id,
            "recording rule not found",
            &config.db,
        )
        .await?;
        return Ok(());
    };

    let Some(camera) = db::cameras::get_camera(session.camera_id, &config.db).await? else {
        db::camera_recording_sessions::mark_failed(session.id, "camera not found", &config.db)
            .await?;
        return Ok(());
    };

    info!(
        "Camera recording session {} started: camera={} {}, rule={}, trigger={}",
        session.id, camera.id, camera.name, rule.id, session.trigger_summary
    );
    db::camera_recording_sessions::mark_recording(session.id, &config.db).await?;

    let mut pre_roll_task = Some(spawn_pre_roll_attachment(
        config.clone(),
        camera.clone(),
        session.id,
    )?);
    // Part zero is reserved for a head requested by any participating rule,
    // including one that joins after live recording has already begun.
    let mut segment_index = next_segment_index(session.id, &config).await?;

    loop {
        let Some(current) =
            db::camera_recording_sessions::get_session(session.id, &config.db).await?
        else {
            finish_pre_roll_task(pre_roll_task.take()).await?;
            return Ok(());
        };

        if current.status != "recording" {
            finish_pre_roll_task(pre_roll_task.take()).await?;
            return Ok(());
        }

        if pre_roll_task.as_ref().is_some_and(PreRollTask::is_finished) {
            finish_pre_roll_task(pre_roll_task.take()).await?;
        }

        let now = Utc::now();
        if current.stop_after_at <= now {
            // The transaction also checks that the newest pre-roll request
            // has completed an attempt and no native writer remains active.
            if db::camera_recording_sessions::mark_ready_if_due(session.id, Utc::now(), &config.db)
                .await?
            {
                break;
            }
            tokio::time::sleep(StdDuration::from_millis(100)).await;
            continue;
        }

        let segment_duration = current
            .stop_after_at
            .signed_duration_since(now)
            .num_seconds()
            .min(rule.max_segment_seconds)
            .max(1)
            .try_into()
            .unwrap_or(u32::MAX);
        let segment_id = db::camera_recording_segments::create_segment(
            session.id,
            session.camera_id,
            segment_index,
            i64::from(segment_duration),
            &config.db,
        )
        .await?;

        info!(
            "Camera recording segment {} started: session={}, camera={} {}, index={}, duration={}s",
            segment_id, session.id, camera.id, camera.name, segment_index, segment_duration
        );
        match capture_and_store_segment(
            &config,
            &camera,
            segment_id,
            session.id,
            segment_index,
            segment_duration,
        )
        .await
        {
            Ok((relative_path, size_bytes)) => {
                db::camera_recording_segments::mark_ready(
                    segment_id,
                    &relative_path,
                    size_bytes,
                    Utc::now(),
                    &config.db,
                )
                .await?;
                spawn_recording_derivatives(config.clone(), relative_path.clone());
                let _ =
                    db::camera_health::mark_recording_ok(session.camera_id, size_bytes, &config.db)
                        .await;
                info!(
                    "Camera recording segment {} ready: session={}, camera={} {}, size={} bytes",
                    segment_id, session.id, camera.id, camera.name, size_bytes
                );
            }
            Err(error) => {
                let message = crate::core::cameras::sanitize_camera_error(&error);
                error!(
                    "Camera recording segment {} failed: session={}, camera={} {}, duration={}s, error={}",
                    segment_id,
                    session.id,
                    camera.id,
                    camera.name,
                    segment_duration,
                    message
                );
                let _ = db::camera_health::mark_error_kind(
                    session.camera_id,
                    db::camera_health::CameraHealthOperation::Recording,
                    &message,
                    &config.db,
                )
                .await;
                db::camera_recording_segments::mark_failed(segment_id, &message, &config.db)
                    .await?;
                db::camera_recording_sessions::mark_failed(session.id, &message, &config.db)
                    .await?;
                finish_pre_roll_task(pre_roll_task.take()).await?;
                return Ok(());
            }
        }

        segment_index += 1;
    }

    finish_pre_roll_task(pre_roll_task.take()).await?;
    send_ready_notification(bot, config, session.id).await?;

    Ok(())
}

struct PreRollTask {
    done: Arc<std::sync::atomic::AtomicBool>,
    completion: Option<tokio::sync::oneshot::Receiver<Result<()>>>,
    cancel: CancellationToken,
}
impl PreRollTask {
    fn is_finished(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::Acquire)
    }
}
impl Drop for PreRollTask {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn spawn_pre_roll_attachment(
    config: Arc<AppConfig>,
    camera: db::cameras::Camera,
    session_id: i64,
) -> Result<PreRollTask> {
    let recording_start_at = Utc::now();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let task_done = done.clone();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let (tx, completion) = tokio::sync::oneshot::channel();
    crate::core::cameras::spawn_media_task(async move {
        let work = async {
            loop {
                let Some(session) =
                    db::camera_recording_sessions::get_session(session_id, &config.db).await?
                else {
                    return Ok(());
                };
                if session.status != "recording" {
                    return Ok(());
                }
                if requested_pre_roll_seconds(&session) > session.pre_roll_attempted_seconds {
                    if let Err(error) =
                        attach_pre_roll_segment(&config, &camera, &session, recording_start_at)
                            .await
                    {
                        let message = crate::core::cameras::sanitize_camera_error(&error);
                        warn!(
                            "Pre-roll attempt failed: session={}, requested={}s, error={}",
                            session_id, session.pre_roll_seconds, message
                        );
                        db::camera_recording_sessions::mark_pre_roll_failure(
                            session_id,
                            requested_pre_roll_seconds(&session),
                            &message,
                            &config.db,
                        )
                        .await?;
                    }
                    // Re-read immediately once an attempt finishes: a merge may
                    // have increased the request while its native mux was busy.
                    continue;
                }
                // Keep watching during the live capture. A missing historical
                // interval is attempted once per larger request, never retried
                // on every poll.
                tokio::time::sleep(StdDuration::from_millis(100)).await;
            }
        };
        let result = tokio::select! {
            biased;
            _ = task_cancel.cancelled() => Ok(()),
            result = work => result,
        };
        task_done.store(true, std::sync::atomic::Ordering::Release);
        let _ = tx.send(result);
    })?;
    Ok(PreRollTask {
        done,
        completion: Some(completion),
        cancel,
    })
}

async fn finish_pre_roll_task(task: Option<PreRollTask>) -> Result<()> {
    if let Some(mut task) = task {
        task.cancel.cancel();
        task.completion
            .take()
            .unwrap()
            .await
            .context("Pre-roll coordinator stopped before completion")??;
    }
    Ok(())
}

async fn capture_and_store_segment(
    config: &Arc<AppConfig>,
    camera: &db::cameras::Camera,
    segment_id: i64,
    session_id: i64,
    segment_index: i64,
    segment_duration: u32,
) -> Result<(String, i64)> {
    let relative_path = segment_relative_path(camera.id, session_id, segment_index);
    register_recording_paths(segment_id, &relative_path, &config.db).await?;
    let writer = WriterLease::begin(segment_id, "capture", &config.db).await?;
    let result = FILE_WRITER
        .scope(writer.clone(), async {
            let final_path = Path::new(&config.camera_recording_storage_root).join(&relative_path);
            let tmp_path = tmp_segment_path(&final_path);

            if let Some(parent) = final_path.parent() {
                tokio::fs::create_dir_all(parent).await.with_context(|| {
                    format!("failed to create recording directory {}", parent.display())
                })?;
            }

            let captured_size_bytes = match crate::core::cameras::capture_clip_to_file(
                camera,
                segment_duration,
                tmp_path.clone(),
            )
            .await
            {
                Ok(size_bytes) => size_bytes,
                Err(error) => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Err(error);
                }
            };
            publish_segment_file(
                camera,
                session_id,
                relative_path,
                tmp_path,
                final_path,
                Some(captured_size_bytes),
                false,
            )
            .await
        })
        .await;
    writer.finish().await?;
    result
}

async fn attach_pre_roll_segment(
    config: &Arc<AppConfig>,
    camera: &db::cameras::Camera,
    session: &db::camera_recording_sessions::RecordingSession,
    recording_start_at: DateTime<Utc>,
) -> Result<()> {
    let requested_seconds = requested_pre_roll_seconds(session);
    if requested_seconds <= session.pre_roll_attempted_seconds {
        return Ok(());
    }
    let segment_id =
        db::camera_recording_segments::prepare_pre_roll_segment(session.id, &config.db).await?;
    // Never overwrite a published head or its derivatives. The row keeps index
    // zero; each larger request owns separate files until its atomic swap.
    let relative_path = segment_relative_path(camera.id, session.id, 0)
        .replace(".mp4", &format!(".pre_roll_{requested_seconds}.mp4"));
    let paths = owned_recording_paths(&relative_path);
    let writer = WriterLease::begin(segment_id, "pre_roll", &config.db).await?;
    db::camera_recording_segments::register_pre_roll_candidate(segment_id, &paths, &config.db)
        .await?;
    let outcome = FILE_WRITER
        .scope(writer.clone(), async {
            let final_path = Path::new(&config.camera_recording_storage_root).join(&relative_path);
            let tmp_path = tmp_segment_path(&final_path);
            if let Some(parent) = final_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let result = config
                .camera_pre_roll_registry
                .write_archive_to_file(
                    camera.id,
                    requested_seconds as u32,
                    session.first_event_at,
                    recording_start_at,
                    tmp_path.clone(),
                )
                .await?;
            let (published_path, size_bytes) = publish_segment_file(
                camera,
                session.id,
                relative_path.clone(),
                tmp_path,
                final_path,
                None,
                true,
            )
            .await?;
            let actual_seconds = (session.first_event_at - result.from)
                .num_seconds()
                .max(0)
                .min(result.duration_s);
            let partial = result.partial || actual_seconds < requested_seconds;
            let warning = result.warning.as_deref().or(if partial {
                Some("Requested pre-roll history is only partially available")
            } else {
                None
            });
            let replaced = db::camera_recording_segments::publish_pre_roll(
                segment_id,
                db::camera_recording_segments::PreRollPublication {
                    path: &published_path,
                    paths: &paths,
                    size_bytes,
                    duration_s: result.duration_s,
                    from: result.from,
                    requested_seconds,
                    actual_seconds,
                    partial,
                    warning,
                },
                &config.db,
            )
            .await?;
            if replaced {
                spawn_recording_derivatives(config.clone(), published_path);
            }
            Ok(())
        })
        .await;
    if let Err(error) = &outcome {
        let message = crate::core::cameras::sanitize_camera_error(error);
        db::camera_recording_segments::discard_pre_roll_candidate(segment_id, &message, &config.db)
            .await?;
    }
    writer.finish().await?;
    outcome
}

fn requested_pre_roll_seconds(session: &db::camera_recording_sessions::RecordingSession) -> i64 {
    session
        .pre_roll_seconds
        .clamp(0, db::camera_recording_rules::MAX_PRE_ROLL_SECONDS)
}

async fn publish_segment_file(
    camera: &db::cameras::Camera,
    session_id: i64,
    relative_path: String,
    tmp_path: PathBuf,
    final_path: PathBuf,
    captured_size_bytes: Option<u64>,
    archive_pre_roll: bool,
) -> Result<(String, i64)> {
    let native_final_path = final_path.clone();
    let publish = move || {
        crate::core::cameras::check_native_cancelled()?;
        let metadata = std::fs::metadata(&tmp_path)?;
        ensure!(
            metadata.is_file() && metadata.len() > 0,
            "Recording output is empty or not a regular file"
        );
        std::fs::rename(&tmp_path, &native_final_path).with_context(|| {
            format!(
                "failed to publish recording {}",
                native_final_path.display()
            )
        })?;
        Ok(metadata.len())
    };
    let stored_size_bytes = if archive_pre_roll {
        crate::core::cameras::run_archive_pre_roll_job(publish).await?
    } else {
        crate::core::cameras::run_recording_io_job(publish).await?
    };
    if captured_size_bytes.is_some_and(|size_bytes| size_bytes != stored_size_bytes) {
        warn!(
            "Published recording size differs from captured file: session={}, camera={} {}, path={}, captured={} bytes, stored={} bytes",
            session_id,
            camera.id,
            camera.name,
            final_path.display(),
            captured_size_bytes.unwrap_or_default(),
            stored_size_bytes
        );
    }
    info!(
        "Published recording file: session={}, camera={} {}, path={}, size={} bytes",
        session_id,
        camera.id,
        camera.name,
        final_path.display(),
        stored_size_bytes
    );

    Ok((
        relative_path,
        i64::try_from(stored_size_bytes).unwrap_or(i64::MAX),
    ))
}

fn segment_relative_path(camera_id: i64, session_id: i64, segment_index: i64) -> String {
    let date = Utc::now().format("%Y/%m/%d");
    format!(
        "{}/camera_{}_session_{}_segment_{}.mp4",
        date, camera_id, session_id, segment_index
    )
}

/// Save the captured source once, before Telegram compression. The tracked task
/// owns its temporary input and completes even if the requesting widget closes.
pub(crate) async fn archive_manual_clip(
    config: Arc<AppConfig>,
    camera_id: i64,
    user_id: u64,
    seconds: u32,
    source: crate::core::cameras::TemporaryMedia,
) -> Result<Option<i64>> {
    if !db::cameras::archives_manual_clips(camera_id, &config.db).await? {
        return Ok(None);
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    crate::core::cameras::spawn_media_task(async move {
        let result = store_manual_clip(&config, camera_id, user_id, seconds, source).await;
        let _ = tx.send(result);
    })?;
    rx.await.context("Manual archive task stopped")?
}

async fn store_manual_clip(
    config: &Arc<AppConfig>,
    camera_id: i64,
    user_id: u64,
    seconds: u32,
    source: crate::core::cameras::TemporaryMedia,
) -> Result<Option<i64>> {
    let retention = db::settings::get_u32_or(
        db::settings::CAMERA_RECORDING_DEFAULT_RETENTION_DAYS,
        30,
        &config.db,
    )
    .await;
    let Some(session) = db::camera_recording_sessions::create_manual_session(
        camera_id, user_id, seconds, retention, &config.db,
    )
    .await?
    else {
        return Ok(None);
    };
    let result = async {
        let segment = db::camera_recording_segments::create_segment(
            session,
            camera_id,
            1,
            i64::from(seconds),
            &config.db,
        )
        .await?;
        let relative = segment_relative_path(camera_id, session, 1);
        register_recording_paths(segment, &relative, &config.db).await?;
        let writer = WriterLease::begin(segment, "manual_import", &config.db).await?;
        let result = FILE_WRITER
            .scope(writer.clone(), async {
                let final_path = Path::new(&config.camera_recording_storage_root).join(&relative);
                let tmp_path = tmp_segment_path(&final_path);
                let size = crate::core::cameras::run_recording_io_job(move || {
                    crate::core::cameras::check_native_cancelled()?;
                    std::fs::create_dir_all(
                        final_path.parent().context("Archive directory missing")?,
                    )?;
                    let size = std::fs::copy(source.path(), &tmp_path)?;
                    ensure!(size > 0, "Manual recording is empty");
                    std::fs::rename(&tmp_path, &final_path)?;
                    Ok(i64::try_from(size)?)
                })
                .await?;
                db::camera_recording_segments::mark_ready(
                    segment,
                    &relative,
                    size,
                    Utc::now(),
                    &config.db,
                )
                .await
            })
            .await;
        writer.finish().await?;
        result?;
        ensure!(
            db::camera_recording_sessions::mark_ready_if_due(session, Utc::now(), &config.db)
                .await?,
            "Manual recording was removed before completion"
        );
        Ok(Some(session))
    }
    .await;
    if let Err(ref error) = result {
        let _ = db::camera_recording_sessions::mark_failed(session, &error.to_string(), &config.db)
            .await;
    }
    result
}

fn tmp_segment_path(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let stem = final_path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or(file_name);
    let extension = final_path
        .extension()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();

    if extension.is_empty() {
        final_path.with_file_name(format!(".{}.tmp", stem))
    } else {
        final_path.with_file_name(format!(".{}.tmp.{}", stem, extension))
    }
}

pub(crate) fn recording_file_path(config: &AppConfig, file_path: &str) -> PathBuf {
    Path::new(&config.camera_recording_storage_root).join(file_path)
}

pub(crate) async fn recording_file_info(
    config: &AppConfig,
    file_path: &str,
) -> Result<(PathBuf, u64)> {
    let full_path = recording_file_path(config, file_path);
    let metadata = tokio::fs::metadata(&full_path)
        .await
        .with_context(|| format!("recording file is missing: {}", full_path.display()))?;
    ensure!(
        metadata.is_file(),
        "recording path is not a file: {}",
        full_path.display()
    );
    let size_bytes = metadata.len();
    ensure!(
        size_bytes > 0,
        "recording file is empty: {}",
        full_path.display()
    );

    Ok((full_path, size_bytes))
}

pub(crate) async fn recording_preview(config: &AppConfig, file_path: &str) -> Result<Vec<u8>> {
    let preview_path = recording_preview_path(config, file_path);
    if let Ok(bytes) = tokio::fs::read(&preview_path).await {
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }

    let writer = WriterLease::begin_derivative(file_path, "preview", &config.db).await?;
    let result = FILE_WRITER
        .scope(writer.clone(), async {
            let (video_path, _) = recording_file_info(config, file_path).await?;
            let bytes = crate::core::cameras::extract_video_preview(video_path.clone())
                .await
                .with_context(|| {
                    format!("failed to create preview for {}", video_path.display())
                })?;
            ensure!(
                !bytes.is_empty(),
                "video preview is empty: {}",
                video_path.display()
            );

            publish_derivative_file(preview_path.clone(), bytes.clone()).await?;

            sqlx::query("UPDATE recording_files SET size_bytes=? WHERE path=?")
                .bind(bytes.len() as i64)
                .bind(
                    preview_path
                        .strip_prefix(&config.camera_recording_storage_root)?
                        .to_string_lossy()
                        .as_ref(),
                )
                .execute(&config.db)
                .await?;
            Ok(bytes)
        })
        .await;
    writer.finish().await?;
    result
}

async fn publish_derivative_file(path: PathBuf, bytes: Vec<u8>) -> Result<()> {
    crate::core::cameras::run_local_media_job(move || {
        crate::core::cameras::check_native_cancelled()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = tmp_preview_path(&path);
        std::fs::write(&tmp, bytes)?;
        crate::core::cameras::check_native_cancelled()?;
        std::fs::rename(tmp, path)?;
        Ok(())
    })
    .await
}

pub(crate) async fn cached_recording_preview(
    config: &AppConfig,
    file_path: &str,
) -> Result<Option<Vec<u8>>> {
    let preview_path = recording_preview_path(config, file_path);
    match tokio::fs::read(&preview_path).await {
        Ok(bytes) if !bytes.is_empty() => Ok(Some(bytes)),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to read preview {}", preview_path.display()))
        }
    }
}

pub(crate) async fn recording_thumbnail(config: &AppConfig, file_path: &str) -> Result<Vec<u8>> {
    let thumbnail_path = recording_thumbnail_path(config, file_path);
    if let Ok(bytes) = tokio::fs::read(&thumbnail_path).await {
        if is_usable_telegram_thumbnail(&bytes) {
            return Ok(bytes);
        }
    }

    let preview = recording_preview(config, file_path).await?;
    let writer = WriterLease::begin_derivative(file_path, "thumbnail", &config.db).await?;
    let result = FILE_WRITER
        .scope(writer.clone(), async {
            let thumbnail = crate::core::cameras::run_local_media_job(move || {
                encode_telegram_thumbnail(&preview)
            })
            .await
            .context("thumbnail encoder task failed")?;

            publish_derivative_file(thumbnail_path.clone(), thumbnail.clone()).await?;

            sqlx::query("UPDATE recording_files SET size_bytes=? WHERE path=?")
                .bind(thumbnail.len() as i64)
                .bind(
                    thumbnail_path
                        .strip_prefix(&config.camera_recording_storage_root)?
                        .to_string_lossy()
                        .as_ref(),
                )
                .execute(&config.db)
                .await?;
            Ok(thumbnail)
        })
        .await;
    writer.finish().await?;
    result
}

pub(crate) async fn first_cached_recording_preview(
    config: Arc<AppConfig>,
    segments: &[db::camera_recording_segments::RecordingSegment],
) -> Option<Vec<u8>> {
    match first_cached_recording_preview_lookup(&config, segments).await {
        CachedRecordingPreview::Ready(bytes) => Some(bytes),
        CachedRecordingPreview::Missing(file_path) => {
            spawn_recording_derivatives(config, file_path);
            None
        }
        CachedRecordingPreview::None => None,
    }
}

pub(crate) enum CachedRecordingPreview {
    Ready(Vec<u8>),
    Missing(String),
    None,
}

pub(crate) async fn first_cached_recording_preview_lookup(
    config: &AppConfig,
    segments: &[db::camera_recording_segments::RecordingSegment],
) -> CachedRecordingPreview {
    let mut first_missing_path = None;

    for segment in segments {
        if segment.status != "ready" {
            continue;
        }

        let Some(file_path) = segment.file_path.as_deref() else {
            continue;
        };

        match cached_recording_preview(config, file_path).await {
            Ok(Some(bytes)) => return CachedRecordingPreview::Ready(bytes),
            Ok(None) => {
                if first_missing_path.is_none() {
                    first_missing_path = Some(file_path.to_string());
                }
            }
            Err(error) => {
                debug!(
                    "Recording preview unavailable: session={}, segment={}, path={}, error={:#}",
                    segment.session_id, segment.id, file_path, error
                );
            }
        }
    }

    if let Some(file_path) = first_missing_path {
        return CachedRecordingPreview::Missing(file_path);
    }

    CachedRecordingPreview::None
}

pub(crate) fn spawn_recording_derivatives(config: Arc<AppConfig>, file_path: String) {
    if !mark_derivative_job_started(&file_path) {
        debug!(
            "Recording derivative prebuild skipped by dedupe/backoff: path={}",
            file_path
        );
        return;
    }

    let cleanup_path = file_path.clone();
    let result = crate::core::cameras::spawn_media_task(async move {
        let job_path = file_path.clone();
        match recording_preview(&config, &file_path).await {
            Ok(_) => {
                if recording_file_info(&config, &file_path).await.is_err() {
                    let _ = retry_pending_recording_deletes(&config).await;
                    clear_recording_derivative_failure(&job_path);
                    clear_recording_derivative_job(&job_path);
                    return;
                }

                if let Err(error) = recording_thumbnail(&config, &file_path).await {
                    debug!(
                        "Recording thumbnail prebuild unavailable: path={}, error={:#}",
                        file_path, error
                    );
                }
                if recording_file_info(&config, &file_path).await.is_err() {
                    let _ = retry_pending_recording_deletes(&config).await;
                }
                clear_recording_derivative_failure(&job_path);
            }
            Err(error) => {
                debug!(
                    "Recording preview prebuild unavailable: path={}, error={:#}",
                    file_path, error
                );
                mark_recording_derivative_failure(&job_path);
            }
        }
        clear_recording_derivative_job(&job_path);
    });
    if result.is_err() {
        clear_recording_derivative_job(&cleanup_path);
    }
}

fn mark_derivative_job_started(file_path: &str) -> bool {
    if recording_derivative_failure_backoff_active(file_path) {
        return false;
    }

    let jobs = RECORDING_DERIVATIVE_JOBS.get_or_init(|| std::sync::Mutex::new(HashSet::new()));
    let Ok(mut jobs) = jobs.lock() else {
        return false;
    };
    jobs.len() < 16 && jobs.insert(file_path.to_string())
}

fn clear_recording_derivative_job(file_path: &str) {
    if let Some(jobs) = RECORDING_DERIVATIVE_JOBS.get() {
        if let Ok(mut jobs) = jobs.lock() {
            jobs.remove(file_path);
        }
    }
}

fn recording_derivative_failure_backoff_active(file_path: &str) -> bool {
    let failures =
        RECORDING_DERIVATIVE_FAILURES.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let Ok(mut failures) = failures.lock() else {
        return false;
    };

    prune_recording_derivative_failures(&mut failures);
    failures.get(file_path).is_some_and(|failed_at| {
        Utc::now().signed_duration_since(*failed_at)
            < Duration::seconds(RECORDING_DERIVATIVE_FAILURE_BACKOFF_S)
    })
}

fn mark_recording_derivative_failure(file_path: &str) {
    let failures =
        RECORDING_DERIVATIVE_FAILURES.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut failures) = failures.lock() {
        prune_recording_derivative_failures(&mut failures);
        failures.insert(file_path.to_string(), Utc::now());
    }
}

fn clear_recording_derivative_failure(file_path: &str) {
    if let Some(failures) = RECORDING_DERIVATIVE_FAILURES.get() {
        if let Ok(mut failures) = failures.lock() {
            failures.remove(file_path);
        }
    }
}

fn prune_recording_derivative_failures(failures: &mut HashMap<String, chrono::DateTime<Utc>>) {
    let now = Utc::now();
    failures.retain(|_, failed_at| {
        now.signed_duration_since(*failed_at)
            < Duration::seconds(RECORDING_DERIVATIVE_FAILURE_RETENTION_S)
    });
}

fn recording_preview_path(config: &AppConfig, file_path: &str) -> PathBuf {
    preview_path_for_video(&recording_file_path(config, file_path))
}

fn recording_thumbnail_path(config: &AppConfig, file_path: &str) -> PathBuf {
    thumbnail_path_for_video(&recording_file_path(config, file_path))
}

fn preview_path_for_video(video_path: &Path) -> PathBuf {
    let stem = video_path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    video_path.with_file_name(format!("{}.preview.jpg", stem))
}

fn thumbnail_path_for_video(video_path: &Path) -> PathBuf {
    let stem = video_path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    video_path.with_file_name(format!("{}.thumb.jpg", stem))
}

fn tmp_preview_path(preview_path: &Path) -> PathBuf {
    let stem = preview_path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    preview_path.with_file_name(format!(".{}.tmp.jpg", stem))
}

fn encode_telegram_thumbnail(preview: &[u8]) -> Result<Vec<u8>> {
    let image = image::load_from_memory(preview).context("failed to decode preview image")?;
    let thumbnail = image
        .thumbnail(TELEGRAM_THUMBNAIL_MAX_SIDE, TELEGRAM_THUMBNAIL_MAX_SIDE)
        .to_rgb8();
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 75);
    encoder
        .encode(
            &thumbnail,
            thumbnail.width(),
            thumbnail.height(),
            ColorType::Rgb8.into(),
        )
        .context("failed to encode Telegram thumbnail")?;
    ensure!(
        is_usable_telegram_thumbnail(&bytes),
        "Telegram thumbnail is too large: {} bytes",
        bytes.len()
    );
    Ok(bytes)
}

pub(crate) fn is_usable_telegram_thumbnail(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len() <= TELEGRAM_THUMBNAIL_MAX_BYTES
}

async fn next_segment_index(session_id: i64, config: &Arc<AppConfig>) -> Result<i64> {
    let segments =
        db::camera_recording_segments::list_session_segments(session_id, &config.db).await?;
    Ok(segments
        .iter()
        .map(|segment| segment.segment_index)
        .max()
        .unwrap_or(0)
        + 1)
}

async fn send_ready_notification(bot: Bot, config: Arc<AppConfig>, session_id: i64) -> Result<()> {
    let Some(session) = db::camera_recording_sessions::get_session(session_id, &config.db).await?
    else {
        return Ok(());
    };

    if session.notification_sent_at.is_some() {
        return Ok(());
    }

    let Some(rule_id) = session.rule_id else {
        return Ok(());
    };
    let Some(rule) = db::camera_recording_rules::get_rule(rule_id, &config.db).await? else {
        return Ok(());
    };
    if !rule.notifications_enabled() {
        db::camera_recording_sessions::mark_notification_sent(session.id, &config.db).await?;
        return Ok(());
    }

    let segments =
        db::camera_recording_segments::list_ready_segments(session_id, &config.db).await?;
    if segments.is_empty() {
        return Ok(());
    }

    let Some(camera) = db::cameras::get_camera(session.camera_id, &config.db).await? else {
        return Ok(());
    };

    let mut recipients = std::collections::HashSet::new();
    recipients.insert(config.root_user as i64);

    let trigger_entities = trigger_entities(&session.trigger_summary);
    for entity_id in &trigger_entities {
        for user_id in db::subscriptions::get_subscribers(entity_id, &config.db)
            .await
            .unwrap_or_default()
        {
            recipients.insert(user_id);
        }
    }

    if recipients.is_empty() {
        return Ok(());
    }

    let critical = recording_notification_critical(&trigger_entities, &config.db).await;
    if !critical
        && should_suppress_recording_notification(&rule, config.clone(), &recipients, bot.clone())
            .await?
    {
        db::camera_recording_sessions::mark_notification_sent(session_id, &config.db).await?;
        return Ok(());
    }

    let total_duration: i64 = segments.iter().map(|segment| segment.duration_s).sum();
    let completion = if session.error.is_some() {
        "доступна частично"
    } else {
        "готова"
    };
    let text = format!(
        "🎞 Запись с камеры {} {} · {} · {}",
        md::bold(&camera.name),
        completion,
        format_recording_duration(total_duration),
        format_parts(segments.len())
    );

    crate::bot::notification::send_notification(
        bot,
        config.clone(),
        NotificationData {
            scope: crate::models::NotificationScope::Camera(camera.id),
            human_state: text,
            recipients: recipients.into_iter().collect(),
            critical,
        },
    )
    .await?;
    db::camera_recording_sessions::mark_notification_sent(session_id, &config.db).await?;

    Ok(())
}

async fn should_suppress_recording_notification(
    rule: &db::camera_recording_rules::RecordingRule,
    config: Arc<AppConfig>,
    recipients: &std::collections::HashSet<i64>,
    bot: Bot,
) -> Result<bool> {
    if !rule.noise_enabled() {
        return Ok(false);
    }

    let window_s = db::settings::get_i64(db::settings::NOTIFICATION_NOISE_WINDOW_S, &config.db)
        .await?
        .unwrap_or(300);
    let threshold = db::settings::get_i64(db::settings::NOTIFICATION_NOISE_THRESHOLD, &config.db)
        .await?
        .unwrap_or(10);
    let cooldown_s =
        db::settings::get_i64(db::settings::NOTIFICATION_SUMMARY_COOLDOWN_S, &config.db)
            .await?
            .unwrap_or(300);

    if threshold <= 0 || window_s <= 0 {
        return Ok(false);
    }

    let now = Utc::now();
    let since = now - Duration::seconds(window_s);
    let count =
        db::activity_log::count_recording_rule_events_since(rule.id, since, &config.db).await?;
    if count < threshold {
        return Ok(false);
    }

    let cooldown_allows_summary = rule
        .noise_summary_sent_at
        .map(|sent_at| sent_at + Duration::seconds(cooldown_s) <= now)
        .unwrap_or(true);
    if cooldown_allows_summary {
        let text = format!(
            "🔕 Правило {} сработало {} раз за {}. Уведомления временно свернуты.",
            md::bold(&rule.name),
            count,
            format_recording_duration(window_s)
        );
        crate::bot::notification::send_notification(
            bot,
            config.clone(),
            NotificationData {
                scope: crate::models::NotificationScope::Camera(rule.camera_id),
                human_state: text,
                recipients: recipients.iter().copied().collect(),
                critical: false,
            },
        )
        .await?;
        db::camera_recording_rules::mark_noise_summary_sent(rule.id, now, &config.db).await?;
    }

    Ok(true)
}

fn trigger_entities(trigger_summary: &str) -> Vec<String> {
    trigger_summary
        .split(';')
        .filter_map(|part| part.split_whitespace().next())
        .filter(|value| value.contains('.'))
        .map(str::to_string)
        .collect()
}

async fn recording_notification_critical(entity_ids: &[String], pool: &sqlx::SqlitePool) -> bool {
    for entity_id in entity_ids {
        if db::devices::is_device_critical(entity_id, pool)
            .await
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

pub(crate) fn format_recording_duration(seconds: i64) -> String {
    if seconds >= 60 {
        format!("{}м {}с", seconds / 60, seconds % 60)
    } else {
        format!("{}с", seconds)
    }
}

fn format_parts(count: usize) -> String {
    match count {
        1 => "1 файл".to_string(),
        n => format!("{} файла", n),
    }
}

async fn recover_stale_recordings(config: &Arc<AppConfig>) -> Result<()> {
    let recovered = db::camera_recording_sessions::recover_stale_sessions(&config.db).await?;
    if recovered > 0 {
        warn!(
            "Camera recording recovery: marked {} stale sessions failed",
            recovered
        );
    }

    sqlx::query("DELETE FROM recording_writers")
        .execute(&config.db)
        .await?;
    sqlx::query("UPDATE recording_files SET delete_pending=1 WHERE segment_id IN (SELECT id FROM camera_recording_segments WHERE status IN ('failed','deleted'))").execute(&config.db).await?;
    let _ = retry_pending_recording_deletes(config).await?;
    cleanup_recording_tmp_files(config).await;
    Ok(())
}

pub(crate) async fn cleanup_recording_tmp_files(config: &Arc<AppConfig>) {
    crate::core::cameras::cleanup_abandoned_media();
    cleanup_tmp_files(
        Path::new(&config.camera_recording_storage_root),
        TMP_RECORDING_MAX_AGE,
        &config.db,
    )
    .await;
}

fn owned_recording_paths(file_path: &str) -> Vec<String> {
    let video = Path::new(file_path);
    let preview = preview_path_for_video(video);
    let thumbnail = thumbnail_path_for_video(video);
    let tmp = tmp_segment_path(video);
    let native_tmp = crate::core::cameras::tmp_output_path(&tmp);
    let mut paths = vec![
        video.to_path_buf(),
        preview.clone(),
        thumbnail.clone(),
        tmp,
        native_tmp,
        tmp_preview_path(&preview),
        tmp_preview_path(&thumbnail),
    ];
    let stem = video.file_stem().unwrap_or_default().to_string_lossy();
    paths.push(video.with_file_name(format!("{}.telegram.mp4", stem)));
    for version in 1..=4 {
        paths.push(video.with_file_name(format!("{}.telegram.v{}.mp4", stem, version)));
    }
    paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

async fn register_recording_paths(
    segment_id: i64,
    file_path: &str,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    db::camera_recording_segments::register_paths(
        segment_id,
        &owned_recording_paths(file_path),
        pool,
    )
    .await
}

/// Returns true only after all owned paths have physically disappeared.
pub(crate) async fn delete_recording_session_files(
    config: &Arc<AppConfig>,
    session_id: i64,
) -> Result<bool> {
    delete_recording_session_files_inner(config, session_id, false).await
}

/// Cleanup may continue if deletion completed or the candidate became pinned;
/// false indicates physical deletion is still pending and cleanup must stop.
pub(crate) async fn delete_recording_session_files_if_unpinned(
    config: &Arc<AppConfig>,
    session_id: i64,
) -> Result<bool> {
    delete_recording_session_files_inner(config, session_id, true).await
}

async fn delete_recording_session_files_inner(
    config: &Arc<AppConfig>,
    session_id: i64,
    only_unpinned: bool,
) -> Result<bool> {
    // Legacy files are registered before hiding. Concurrent writers register first
    // and cannot start after the transaction marks the owner deleted.
    let segments =
        db::camera_recording_segments::list_session_segments(session_id, &config.db).await?;
    for segment in segments {
        if let Some(path) = segment.file_path {
            register_recording_paths(segment.id, &path, &config.db).await?;
        }
    }
    if only_unpinned {
        if !db::camera_recording_sessions::soft_delete_session_if_unpinned(session_id, &config.db)
            .await?
        {
            return Ok(true);
        }
    } else {
        db::camera_recording_sessions::soft_delete_session(session_id, &config.db).await?;
    }
    retry_pending_recording_deletes(config).await?;
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM recording_files WHERE session_id=?")
            .bind(session_id)
            .fetch_one(&config.db)
            .await?;
    Ok(pending == 0)
}

pub(crate) async fn retry_pending_recording_deletes(config: &AppConfig) -> Result<bool> {
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT path,attempts FROM recording_files f WHERE delete_pending=1 AND (retry_at IS NULL OR julianday(retry_at)<=julianday('now')) AND NOT EXISTS(SELECT 1 FROM recording_writers w WHERE w.session_id=f.session_id) LIMIT 128")
        .fetch_all(&config.db).await?;
    let root = tokio::fs::canonicalize(&config.camera_recording_storage_root).await?;
    for (path, attempts) in rows {
        if db::camera_recording_segments::safe_relative_path(&path) {
            if let Ok(metadata) = tokio::fs::symlink_metadata(root.join(&path)).await {
                sqlx::query("UPDATE recording_files SET size_bytes=? WHERE path=?")
                    .bind(i64::try_from(metadata.len()).unwrap_or(i64::MAX))
                    .bind(&path)
                    .execute(&config.db)
                    .await?;
            }
        }
        let outcome = remove_owned_recording_file(&root, &path).await;
        match outcome {
            Ok(()) => {
                sqlx::query("DELETE FROM recording_files WHERE path=? AND delete_pending=1")
                    .bind(&path)
                    .execute(&config.db)
                    .await?;
            }
            Err(error) => {
                let delay = (1_i64 << attempts.min(8)).min(300);
                sqlx::query("UPDATE recording_files SET attempts=attempts+1,retry_at=?,last_error=? WHERE path=?")
                    .bind(Utc::now()+Duration::seconds(delay)).bind(db::sanitize_error(&error.to_string())).bind(&path).execute(&config.db).await?;
                warn!("Recording unlink pending: path={}, error={}", path, error);
            }
        }
    }
    Ok(db::camera_recording_segments::pending_file_count(&config.db).await? == 0)
}

async fn remove_owned_recording_file(root: &Path, relative: &str) -> Result<()> {
    ensure!(
        db::camera_recording_segments::safe_relative_path(relative),
        "Refusing recording deletion outside storage root"
    );
    let path = root.join(relative);
    // Resolve the parent, not the file: unlinking a file symlink is safe, but
    // following a directory symlink outside storage is not.
    let parent = path.parent().context("Recording path has no parent")?;
    match tokio::fs::canonicalize(parent).await {
        Ok(parent) => ensure!(
            parent.starts_with(root),
            "Recording parent escapes storage root"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn cleanup_tmp_files(root: &Path, max_age: StdDuration, pool: &sqlx::SqlitePool) {
    let mut stack = vec![root.to_path_buf()];

    while let Some(path) = stack.pop() {
        let Ok(mut dir) = tokio::fs::read_dir(&path).await else {
            continue;
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let entry_path = entry.path();
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };

            if file_type.is_dir() {
                stack.push(entry_path);
            } else if is_tmp_recording_file(&entry_path)
                && is_stale_tmp_recording_file(&entry_path, max_age).await
            {
                let relative = entry_path
                    .strip_prefix(root)
                    .unwrap_or(&entry_path)
                    .to_string_lossy();
                let known: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM recording_files WHERE path=?")
                        .bind(relative.as_ref())
                        .fetch_one(pool)
                        .await
                        .unwrap_or(1);
                if known > 0 {
                    continue;
                }
                debug!(
                    "Unowned stale recording temporary file requires audit: {}",
                    entry_path.display()
                );
            }
        }
    }
}

async fn is_stale_tmp_recording_file(path: &Path, max_age: StdDuration) -> bool {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified_at) = metadata.modified() else {
        return false;
    };
    modified_at
        .elapsed()
        .map(|age| age >= max_age)
        .unwrap_or(false)
}

fn is_tmp_recording_file(path: &Path) -> bool {
    path.file_name()
        .map(|name| {
            let name = name.to_string_lossy();
            name.ends_with(".tmp") || name.contains(".tmp.")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn archive_storage_probe_creates_directory_and_detects_unusable_path() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "ha-archive-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let resolved = prepare_recording_storage(&root).await?;
        assert!(resolved.is_absolute());
        assert!(tokio::fs::read_dir(&root)
            .await?
            .next_entry()
            .await?
            .is_none());
        let file = root.join("file");
        tokio::fs::write(&file, b"not a directory").await?;
        assert!(prepare_recording_storage(&file.join("archive"))
            .await
            .is_err());
        tokio::fs::remove_dir_all(root).await?;
        Ok(())
    }

    #[test]
    fn tmp_recording_files_include_tmp_mp4_outputs() {
        assert!(is_tmp_recording_file(Path::new(
            ".camera_1_segment_1.telegram.tmp.mp4"
        )));
        assert!(is_tmp_recording_file(Path::new(
            ".camera_1_segment_1.preview.123.tmp.jpg"
        )));
        assert!(is_tmp_recording_file(Path::new("recording.tmp")));
        assert!(!is_tmp_recording_file(Path::new(
            "camera_1_segment_1.telegram.v4.mp4"
        )));
    }

    #[test]
    fn recording_derivative_paths_live_next_to_video() {
        let video = Path::new("/recordings/2026/06/29/camera_1_session_2_segment_3.mp4");

        assert_eq!(
            preview_path_for_video(video),
            PathBuf::from("/recordings/2026/06/29/camera_1_session_2_segment_3.preview.jpg")
        );
        assert_eq!(
            thumbnail_path_for_video(video),
            PathBuf::from("/recordings/2026/06/29/camera_1_session_2_segment_3.thumb.jpg")
        );
    }

    #[test]
    fn tmp_segment_path_keeps_mp4_extension_for_ffmpeg() {
        let video = Path::new("/recordings/2026/06/29/camera_1_session_2_segment_3.mp4");

        assert_eq!(
            tmp_segment_path(video),
            PathBuf::from("/recordings/2026/06/29/.camera_1_session_2_segment_3.tmp.mp4")
        );
    }

    #[test]
    fn recording_derivative_failure_backoff_blocks_until_cleared() {
        let file_path = format!(
            "tests/camera_1_session_2_segment_{}.mp4",
            Utc::now().timestamp_millis()
        );

        clear_recording_derivative_failure(&file_path);
        assert!(!recording_derivative_failure_backoff_active(&file_path));

        mark_recording_derivative_failure(&file_path);
        assert!(recording_derivative_failure_backoff_active(&file_path));

        clear_recording_derivative_failure(&file_path);
        assert!(!recording_derivative_failure_backoff_active(&file_path));
    }

    #[test]
    fn telegram_thumbnail_size_limits_are_enforced() {
        assert!(!is_usable_telegram_thumbnail(&[]));
        assert!(is_usable_telegram_thumbnail(&vec![
            1;
            TELEGRAM_THUMBNAIL_MAX_BYTES
        ]));
        assert!(!is_usable_telegram_thumbnail(&vec![
            1;
            TELEGRAM_THUMBNAIL_MAX_BYTES
                + 1
        ]));
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    async fn wait_for_pre_roll(
        config: &AppConfig,
        session_id: i64,
        requested: i64,
    ) -> Result<db::camera_recording_sessions::RecordingSession> {
        tokio::time::timeout(StdDuration::from_secs(3), async {
            loop {
                let session = db::camera_recording_sessions::get_session(session_id, &config.db)
                    .await?
                    .unwrap();
                let writers: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM recording_writers WHERE session_id=? AND kind='pre_roll'",
                )
                .bind(session_id)
                .fetch_one(&config.db)
                .await?;
                if session.pre_roll_attempted_seconds >= requested && writers == 0 {
                    return Ok(session);
                }
                tokio::time::sleep(StdDuration::from_millis(5)).await;
            }
        })
        .await?
    }

    async fn extend_pre_roll(config: &AppConfig, session_id: i64, seconds: i64) -> Result<()> {
        let session = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        let rule_id = db::camera_recording_rules::create_rule(
            db::camera_recording_rules::NewRecordingRule {
                name: "joined head",
                camera_id: session.camera_id,
                condition_logic: db::camera_recording_rules::ConditionLogic::Any,
                tail_seconds: 5,
                max_segment_seconds: 60,
                cooldown_s: 0,
                retention_days: 30,
                pre_roll_enabled: true,
                pre_roll_seconds: seconds,
            },
            &config.db,
        )
        .await?;
        let rule = db::camera_recording_rules::get_rule(rule_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(
            db::camera_recording_sessions::start_or_extend_session(
                &rule,
                "joined",
                "larger head",
                session.first_event_at,
                &config.db,
            )
            .await?,
            db::camera_recording_sessions::SessionAction::Extended(session_id)
        );
        Ok(())
    }

    async fn pre_roll_fixture(
        history_seconds: i64,
    ) -> Result<(Arc<AppConfig>, crate::core::cameras::TemporaryMedia, i64)> {
        let (mut config, owner, session, main) = fixture(false).await?;
        sqlx::query("UPDATE camera_recording_segments SET segment_index=1 WHERE id=?")
            .bind(main)
            .execute(&config.db)
            .await?;
        db::camera_recording_segments::mark_ready(
            main,
            "segment.mp4",
            4096,
            Utc::now(),
            &config.db,
        )
        .await?;
        sqlx::query("UPDATE camera_recording_rules SET notify_enabled=0 WHERE id=1")
            .execute(&config.db)
            .await?;
        let trigger = db::camera_recording_sessions::get_session(session, &config.db)
            .await?
            .unwrap()
            .first_event_at;
        Arc::get_mut(&mut config).unwrap().camera_pre_roll_registry = Arc::new(
            crate::core::camera_pre_roll::prepared_history_for_test(trigger, history_seconds)?,
        );
        extend_pre_roll(&config, session, 1).await?;
        Ok((config, owner, session))
    }

    #[tokio::test]
    async fn joined_long_head_replaces_inflight_short_mp4_before_finalization() -> Result<()> {
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let (config, _owner, session_id) = pre_roll_fixture(15).await?;
        // Even publication must use archival capacity: live captures may occupy
        // every recording slot for minutes while this head is being extended.
        let recording_capacity =
            crate::core::cameras::reserve_recording_capacity_for_test().await?;
        let origin = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap()
            .first_event_at;
        let capacity = crate::core::cameras::reserve_archive_pre_roll_capacity_for_test().await?;
        let job = tokio::spawn(process_recording_job(
            Bot::with_client(
                "123:test",
                teloxide::net::default_reqwest_settings()
                    .no_proxy()
                    .build()?,
            ),
            config.clone(),
            RecordingJob { session_id },
        ));
        // This counter is advanced synchronously with packet selection, before
        // the future waits on the two held native mux permits.
        tokio::time::timeout(StdDuration::from_secs(3), async {
            while config.camera_pre_roll_registry.archive_attempts_for_test() == 0 {
                tokio::time::sleep(StdDuration::from_millis(5)).await;
            }
        })
        .await?;
        extend_pre_roll(&config, session_id, 15).await?;
        let merged = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(merged.pre_roll_seconds, 15);
        assert_eq!(merged.pre_roll_attempted_seconds, 0);
        assert!(
            !db::camera_recording_sessions::mark_ready_if_due(session_id, Utc::now(), &config.db)
                .await?
        );
        drop(capacity);
        tokio::time::timeout(StdDuration::from_secs(5), job).await???;
        drop(recording_capacity);
        let completed = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(completed.status, "ready");
        assert_eq!(completed.pre_roll_seconds, 15);
        assert_eq!(completed.pre_roll_attempted_seconds, 15);
        assert_eq!(completed.pre_roll_actual_seconds, 15);
        assert_eq!(
            completed.pre_roll_from,
            Some(origin - Duration::seconds(15))
        );
        assert_eq!(completed.pre_roll_partial, 0);
        assert!(completed.pre_roll_warning.is_none());
        assert_eq!(
            config.camera_pre_roll_registry.archive_attempts_for_test(),
            2
        );
        let segments =
            db::camera_recording_segments::list_ready_segments(session_id, &config.db).await?;
        assert_eq!(segments.len(), 2);
        let head = segments
            .iter()
            .find(|part| part.segment_index == 0)
            .unwrap();
        let path = head.file_path.as_deref().unwrap();
        assert!(path.ends_with(".pre_roll_15.mp4"));
        let root = Path::new(&config.camera_recording_storage_root);
        let mut input = crate::core::cameras::open_camera_input(root.join(path).to_str().unwrap())?;
        let mut packets = 0;
        while crate::core::cameras::read_packet(&mut input)?.is_some() {
            packets += 1;
        }
        assert_eq!(
            packets, 16,
            "all historical seconds must be present in the real MP4"
        );
        drop(input);
        let short_path = path.replace(".pre_roll_15.mp4", ".pre_roll_1.mp4");
        wait_for_derivatives(&short_path).await?;
        wait_for_derivatives(path).await?;
        assert!(retry_pending_recording_deletes(&config).await?);
        assert!(!root.join(short_path).exists());
        assert!(root.join(path).exists());
        Ok(())
    }

    #[tokio::test]
    async fn partial_or_unavailable_larger_history_keeps_old_head_and_attempts_each_size_once(
    ) -> Result<()> {
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let (config, _owner, session_id) = pre_roll_fixture(1).await?;
        let initial = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        let camera = db::cameras::get_camera(initial.camera_id, &config.db)
            .await?
            .unwrap();
        attach_pre_roll_segment(&config, &camera, &initial, Utc::now()).await?;
        let old = db::camera_recording_segments::list_ready_segments(session_id, &config.db)
            .await?
            .into_iter()
            .find(|part| part.segment_index == 0)
            .unwrap();
        let first = wait_for_pre_roll(&config, session_id, 1).await?;
        assert_eq!(first.pre_roll_actual_seconds, 1);
        wait_for_derivatives(old.file_path.as_deref().unwrap()).await?;
        let coordinator = spawn_pre_roll_attachment(config.clone(), camera, session_id)?;
        // A due check based on the previous short request must not finalize a
        // request merged before the actual transaction acquires its write lock.
        let mut connection = config.db.acquire().await?;
        let finish =
            db::camera_recording_sessions::mark_ready_if_due(session_id, Utc::now(), &config.db);
        tokio::pin!(finish);
        assert!(futures_util::poll!(finish.as_mut()).is_pending());
        sqlx::query("UPDATE camera_recording_sessions SET pre_roll_seconds=15 WHERE id=?")
            .bind(session_id)
            .execute(&mut *connection)
            .await?;
        drop(connection);
        assert!(!finish.await?);
        let failed = wait_for_pre_roll(&config, session_id, 15).await?;
        assert_eq!(failed.pre_roll_seconds, 15);
        assert_eq!(failed.pre_roll_actual_seconds, 1);
        assert_eq!(failed.pre_roll_from, first.pre_roll_from);
        assert_eq!(failed.pre_roll_partial, 1);
        assert!(failed.pre_roll_warning.is_some());
        tokio::time::sleep(StdDuration::from_millis(250)).await;
        assert_eq!(
            config.camera_pre_roll_registry.archive_attempts_for_test(),
            2
        );
        config.camera_pre_roll_registry.clear_history_for_test();
        extend_pre_roll(&config, session_id, 30).await?;
        let failed_again = wait_for_pre_roll(&config, session_id, 30).await?;
        assert_eq!(failed_again.pre_roll_seconds, 30);
        assert_eq!(failed_again.pre_roll_actual_seconds, 1);
        tokio::time::sleep(StdDuration::from_millis(250)).await;
        assert_eq!(
            config.camera_pre_roll_registry.archive_attempts_for_test(),
            3
        );
        assert!(
            db::camera_recording_sessions::mark_ready_if_due(session_id, Utc::now(), &config.db)
                .await?
        );
        finish_pre_roll_task(Some(coordinator)).await?;
        let head = db::camera_recording_segments::get_segment(old.id, &config.db)
            .await?
            .unwrap();
        assert_eq!(head.status, "ready");
        assert_eq!(head.file_path, old.file_path);
        wait_for_derivatives(head.file_path.as_deref().unwrap()).await?;
        assert!(retry_pending_recording_deletes(&config).await?);
        assert!(Path::new(&config.camera_recording_storage_root)
            .join(head.file_path.unwrap())
            .exists());
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_ready_head_extension_keeps_candidate_pending_through_recovery() -> Result<()>
    {
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let (config, _owner, session_id) = pre_roll_fixture(15).await?;
        let short = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        let camera = db::cameras::get_camera(short.camera_id, &config.db)
            .await?
            .unwrap();
        attach_pre_roll_segment(&config, &camera, &short, Utc::now()).await?;
        let head = db::camera_recording_segments::list_ready_segments(session_id, &config.db)
            .await?
            .into_iter()
            .find(|part| part.segment_index == 0)
            .unwrap();
        let old_path = head.file_path.as_deref().unwrap();
        wait_for_derivatives(old_path).await?;
        extend_pre_roll(&config, session_id, 15).await?;
        let large = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        let capacity = crate::core::cameras::reserve_archive_pre_roll_capacity_for_test().await?;
        let pending = {
            let config = config.clone();
            tokio::spawn(async move {
                attach_pre_roll_segment(&config, &camera, &large, Utc::now()).await
            })
        };
        tokio::time::timeout(StdDuration::from_secs(3), async {
            while config.camera_pre_roll_registry.archive_attempts_for_test() < 2 {
                tokio::time::sleep(StdDuration::from_millis(5)).await;
            }
        })
        .await?;
        let candidate: String = sqlx::query_scalar("SELECT path FROM recording_files WHERE segment_id=? AND path LIKE '%.pre_roll_15.mp4' AND delete_pending=1")
            .bind(head.id).fetch_one(&config.db).await?;
        // Permit real mux + rename, but block the publication transaction after
        // the MP4 exists. Cancellation must need no compensating cleanup future.
        let connection = config.db.acquire().await?;
        drop(capacity);
        let root = Path::new(&config.camera_recording_storage_root);
        let candidate_path = root.join(&candidate);
        tokio::time::timeout(StdDuration::from_secs(3), async {
            while !candidate_path.exists() {
                tokio::time::sleep(StdDuration::from_millis(5)).await;
            }
        })
        .await?;
        assert!(tokio::fs::metadata(&candidate_path).await?.len() > 0);
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        drop(connection);
        tokio::time::timeout(StdDuration::from_secs(3), async {
            loop {
                let writers: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM recording_writers WHERE session_id=?")
                        .bind(session_id)
                        .fetch_one(&config.db)
                        .await?;
                if writers == 0 {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(StdDuration::from_millis(5)).await;
            }
        })
        .await??;
        assert_eq!(
            db::camera_recording_segments::get_segment(head.id, &config.db)
                .await?
                .unwrap()
                .file_path
                .as_deref(),
            Some(old_path)
        );
        let is_pending: bool =
            sqlx::query_scalar("SELECT delete_pending FROM recording_files WHERE path=?")
                .bind(&candidate)
                .fetch_one(&config.db)
                .await?;
        assert!(is_pending);
        recover_stale_recordings(&config).await?;
        assert!(!candidate_path.exists());
        assert!(root.join(old_path).exists());
        assert_eq!(
            db::camera_recording_segments::get_segment(head.id, &config.db)
                .await?
                .unwrap()
                .status,
            "ready"
        );
        assert_eq!(
            db::camera_recording_segments::pending_file_count(&config.db).await?,
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn delayed_derivative_cannot_resurrect_cleaned_up_pre_roll_paths() -> Result<()> {
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let (config, _owner, session_id) = pre_roll_fixture(15).await?;
        let short = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        let camera = db::cameras::get_camera(short.camera_id, &config.db)
            .await?
            .unwrap();
        attach_pre_roll_segment(&config, &camera, &short, Utc::now()).await?;
        let old = db::camera_recording_segments::list_ready_segments(session_id, &config.db)
            .await?
            .into_iter()
            .find(|part| part.segment_index == 0)
            .unwrap();
        let old_path = old.file_path.unwrap();
        wait_for_derivatives(&old_path).await?;
        let preview = recording_preview(&config, &old_path).await?;
        assert!(image::load_from_memory(&preview).is_ok());
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let delayed = {
            let config = config.clone();
            let source = old_path.clone();
            tokio::spawn(async move {
                // The job kept its old segment and image bytes while another
                // head was published and physical cleanup completed.
                resume_rx.await?;
                let lease = WriterLease::begin_derivative(&source, "preview", &config.db).await?;
                let result = FILE_WRITER
                    .scope(
                        lease.clone(),
                        publish_derivative_file(recording_preview_path(&config, &source), preview),
                    )
                    .await;
                lease.finish().await?;
                result
            })
        };
        extend_pre_roll(&config, session_id, 15).await?;
        let long = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        attach_pre_roll_segment(&config, &camera, &long, Utc::now()).await?;
        let current = db::camera_recording_segments::get_segment(old.id, &config.db)
            .await?
            .unwrap();
        let current_path = current.file_path.as_deref().unwrap();
        wait_for_derivatives(current_path).await?;
        assert_ne!(old_path, current_path);
        assert!(retry_pending_recording_deletes(&config).await?);
        let root = Path::new(&config.camera_recording_storage_root);
        for path in owned_recording_paths(&old_path) {
            assert!(!root.join(path).exists());
        }
        resume_tx.send(()).unwrap();
        assert!(
            delayed.await?.is_err(),
            "stale derivative must not acquire a writer by the reused segment id"
        );
        let restored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recording_files WHERE path=?")
            .bind(
                recording_preview_path(&config, &old_path)
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .as_ref(),
            )
            .fetch_one(&config.db)
            .await?;
        assert_eq!(restored, 0);
        for path in owned_recording_paths(&old_path) {
            assert!(!root.join(path).exists());
        }
        assert!(root.join(current_path).exists());
        assert!(!recording_thumbnail(&config, current_path).await?.is_empty());
        Ok(())
    }

    async fn wait_for_derivatives(file_path: &str) -> Result<()> {
        // Keep this test's storage alive until its optional native preview and
        // writer lease have completed.
        tokio::time::timeout(StdDuration::from_secs(2), async {
            loop {
                let pending = RECORDING_DERIVATIVE_JOBS
                    .get()
                    .is_some_and(|jobs| jobs.lock().unwrap().contains(file_path));
                if !pending {
                    break;
                }
                tokio::time::sleep(StdDuration::from_millis(5)).await;
            }
        })
        .await?;
        Ok(())
    }

    async fn fixture(
        terminal: bool,
    ) -> Result<(
        Arc<AppConfig>,
        crate::core::cameras::TemporaryMedia,
        i64,
        i64,
    )> {
        let owner = crate::core::cameras::TemporaryMedia::new()?;
        let mut config = crate::test_support::config().await;
        Arc::get_mut(&mut config)
            .unwrap()
            .camera_recording_storage_root = owner
            .path()
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        db::rooms::sync_rooms_from_ha("media_test", "Media", &config.db).await?;
        let camera = db::cameras::add_manual_camera(
            db::cameras::NewCamera {
                name: "camera",
                room_id: 1,
                stream_url: "rtsp://unused",
                snapshot_url: None,
                clip_seconds: 5,
            },
            &config.db,
        )
        .await?;
        let rule_id = db::camera_recording_rules::create_rule(
            db::camera_recording_rules::NewRecordingRule {
                name: "rule",
                camera_id: camera,
                condition_logic: db::camera_recording_rules::ConditionLogic::Any,
                tail_seconds: 5,
                max_segment_seconds: 60,
                cooldown_s: 0,
                retention_days: 1,
                pre_roll_enabled: false,
                pre_roll_seconds: 0,
            },
            &config.db,
        )
        .await?;
        let rule = db::camera_recording_rules::get_rule(rule_id, &config.db)
            .await?
            .unwrap();
        let db::camera_recording_sessions::SessionAction::Created(session) =
            db::camera_recording_sessions::start_or_extend_session(
                &rule,
                "test",
                "test",
                Utc::now() - Duration::seconds(10),
                &config.db,
            )
            .await?
        else {
            panic!("new session expected")
        };
        db::camera_recording_sessions::mark_recording(session, &config.db).await?;
        let segment =
            db::camera_recording_segments::create_segment(session, camera, 0, 5, &config.db)
                .await?;
        register_recording_paths(segment, "segment.mp4", &config.db).await?;
        tokio::fs::write(
            Path::new(&config.camera_recording_storage_root).join("segment.mp4"),
            vec![0u8; 4096],
        )
        .await?;
        if terminal {
            db::camera_recording_segments::mark_ready(
                segment,
                "segment.mp4",
                4096,
                Utc::now(),
                &config.db,
            )
            .await?;
            assert!(
                db::camera_recording_sessions::mark_ready_if_due(session, Utc::now(), &config.db)
                    .await?
            );
        }
        Ok((config, owner, session, segment))
    }

    #[tokio::test]
    async fn active_delete_is_rejected_and_late_publication_is_accounted() -> Result<()> {
        let (config, _owner, session, segment) = fixture(false).await?;
        assert!(delete_recording_session_files(&config, session)
            .await
            .is_err());
        assert!(
            db::camera_recording_sessions::soft_delete_session(session, &config.db)
                .await
                .is_err()
        );
        let path = Path::new(&config.camera_recording_storage_root).join("segment.mp4");
        assert!(path.exists());
        sqlx::query("UPDATE camera_recording_segments SET deleted_at=CURRENT_TIMESTAMP,status='deleted' WHERE id=?").bind(segment).execute(&config.db).await?;
        assert!(db::camera_recording_segments::mark_ready(
            segment,
            "segment.mp4",
            4096,
            Utc::now(),
            &config.db
        )
        .await
        .is_err());
        assert!(retry_pending_recording_deletes(&config).await?);
        assert!(!path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_candidate_pinned_after_selection_keeps_metadata_and_file() -> Result<()> {
        let (config, _owner, session, _) = fixture(true).await?;
        sqlx::query("UPDATE camera_recording_sessions SET expires_at=? WHERE id=?")
            .bind(Utc::now() - Duration::days(1))
            .bind(session)
            .execute(&config.db)
            .await?;
        let expired = db::camera_recording_sessions::find_expired_sessions(&config.db).await?;
        let quota =
            db::camera_recording_segments::list_deletable_sessions_for_quota(&config.db).await?;
        assert_eq!(expired[0].id, session);
        assert_eq!(quota[0].id, session);
        assert!(db::camera_recording_sessions::toggle_pin(session, 1, &config.db).await?);
        // Both cleanup lists are now stale. Neither path may mark files pending.
        for candidate in [expired[0].id, quota[0].id] {
            assert!(delete_recording_session_files_if_unpinned(&config, candidate).await?);
            assert_eq!(
                db::camera_recording_segments::pending_file_count(&config.db).await?,
                0
            );
            assert!(Path::new(&config.camera_recording_storage_root)
                .join("segment.mp4")
                .exists());
            let current = db::camera_recording_sessions::get_session(session, &config.db)
                .await?
                .unwrap();
            assert_eq!(current.status, "ready");
            assert!(current.pinned_at.is_some());
        }
        // An explicit user deletion retains its original pinned-file behavior.
        assert!(delete_recording_session_files(&config, session).await?);
        assert!(!Path::new(&config.camera_recording_storage_root)
            .join("segment.mp4")
            .exists());
        Ok(())
    }

    #[tokio::test]
    async fn registered_preview_writer_delays_final_delete_and_blocks_later_writers() -> Result<()>
    {
        let (config, _owner, session, segment) = fixture(true).await?;
        let writer =
            db::camera_recording_segments::begin_writer(segment, "preview", &config.db).await?;
        assert!(!delete_recording_session_files(&config, session).await?);
        assert!(
            db::camera_recording_segments::begin_writer(segment, "thumbnail", &config.db)
                .await
                .is_err()
        );
        let preview = Path::new(&config.camera_recording_storage_root).join("segment.preview.jpg");
        tokio::fs::write(&preview, b"late writer result").await?;
        assert!(!retry_pending_recording_deletes(&config).await?);
        assert!(preview.exists());
        db::camera_recording_segments::finish_writer(writer, &config.db).await?;
        assert!(retry_pending_recording_deletes(&config).await?);
        assert!(!preview.exists());
        assert!(delete_recording_session_files(&config, session).await?);
        Ok(())
    }

    #[tokio::test]
    async fn unlink_failure_survives_restart_and_keeps_bytes_until_retry() -> Result<()> {
        let (config, _owner, session, _) = fixture(true).await?;
        let video = Path::new(&config.camera_recording_storage_root).join("segment.mp4");
        // A directory at an owned file path provides a deterministic unlink
        // failure even when tests run with an account bypassing file modes.
        tokio::fs::remove_file(&video).await?;
        tokio::fs::create_dir(&video).await?;
        assert!(!delete_recording_session_files(&config, session).await?);
        let row: (i64, String) = sqlx::query_as(
            "SELECT attempts,last_error FROM recording_files WHERE path='segment.mp4'",
        )
        .fetch_one(&config.db)
        .await?;
        assert_eq!(row.0, 1);
        assert!(!row.1.is_empty());
        assert!(
            db::camera_recording_sessions::get_session(session, &config.db)
                .await?
                .is_none()
        );
        assert!(!retry_pending_recording_deletes(&config).await?);
        tokio::fs::remove_dir(&video).await?;
        sqlx::query("UPDATE recording_files SET retry_at=NULL")
            .execute(&config.db)
            .await?;
        recover_stale_recordings(&config).await?;
        assert_eq!(
            db::camera_recording_segments::pending_file_count(&config.db).await?,
            0
        );
        assert!(delete_recording_session_files(&config, session).await?);
        Ok(())
    }

    #[tokio::test]
    async fn crash_after_publication_before_database_ready_is_recovered() -> Result<()> {
        let (config, _owner, _session, segment) = fixture(false).await?;
        db::camera_recording_segments::begin_writer(segment, "capture", &config.db).await?;
        let video = Path::new(&config.camera_recording_storage_root).join("segment.mp4");
        assert!(video.exists()); // rename completed; mark_ready did not run.
        recover_stale_recordings(&config).await?;
        assert!(!video.exists());
        let writers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recording_writers")
            .fetch_one(&config.db)
            .await?;
        assert_eq!(writers, 0);
        Ok(())
    }

    #[tokio::test]
    async fn empty_recording_never_reports_ready_and_paths_cannot_escape_root() -> Result<()> {
        let (config, owner, session, _segment) = fixture(false).await?;
        assert!(
            db::camera_recording_sessions::mark_ready_if_due(session, Utc::now(), &config.db)
                .await?
        );
        assert_eq!(
            db::camera_recording_sessions::get_session(session, &config.db)
                .await?
                .unwrap()
                .status,
            "failed"
        );
        let root = owner.path().parent().unwrap().canonicalize()?;
        assert!(remove_owned_recording_file(&root, "../outside.mp4")
            .await
            .is_err());
        assert!(remove_owned_recording_file(&root, "/outside.mp4")
            .await
            .is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), root.join("escape"))?;
            assert!(remove_owned_recording_file(&root, "escape/anything.mp4")
                .await
                .is_err());
        }
        Ok(())
    }
    #[tokio::test]
    async fn cancelled_writer_releases_only_after_native_completion_without_restart() -> Result<()>
    {
        let (config, _owner, session, segment) = fixture(true).await?;
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let native_release = release.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task_config = config.clone();
        let task = tokio::spawn(async move {
            let writer = WriterLease::begin(segment, "cancelled_preview", &task_config.db).await?;
            let output =
                Path::new(&task_config.camera_recording_storage_root).join("segment.preview.jpg");
            FILE_WRITER
                .scope(writer, async move {
                    crate::core::cameras::run_local_media_job(move || {
                        let _ = started_tx.send(());
                        while !native_release.load(std::sync::atomic::Ordering::Acquire) {
                            std::thread::sleep(StdDuration::from_millis(1));
                        }
                        std::fs::write(output, b"late native write")?;
                        Ok(())
                    })
                    .await
                })
                .await
        });
        started_rx.await?;
        task.abort();
        let _ = task.await;
        assert!(!delete_recording_session_files(&config, session).await?);
        release.store(true, std::sync::atomic::Ordering::Release);
        tokio::time::timeout(StdDuration::from_secs(2), async {
            loop {
                let count: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM recording_writers WHERE segment_id=?")
                        .bind(segment)
                        .fetch_one(&config.db)
                        .await?;
                if count == 0 {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(StdDuration::from_millis(5)).await;
            }
        })
        .await??;
        assert!(retry_pending_recording_deletes(&config).await?);
        assert!(!Path::new(&config.camera_recording_storage_root)
            .join("segment.preview.jpg")
            .exists());
        Ok(())
    }

    #[tokio::test]
    async fn path_registration_rejects_foreign_owner_and_unregistered_publication() -> Result<()> {
        let (config, _owner, session, original) = fixture(false).await?;
        let camera: i64 =
            sqlx::query_scalar("SELECT camera_id FROM camera_recording_sessions WHERE id=?")
                .bind(session)
                .fetch_one(&config.db)
                .await?;
        let second =
            db::camera_recording_segments::create_segment(session, camera, 1, 5, &config.db)
                .await?;
        assert!(register_recording_paths(second, "segment.mp4", &config.db)
            .await
            .is_err());
        assert!(db::camera_recording_segments::mark_ready(
            second,
            "unknown.mp4",
            100,
            Utc::now(),
            &config.db
        )
        .await
        .is_err());
        let owner: i64 =
            sqlx::query_scalar("SELECT segment_id FROM recording_files WHERE path='segment.mp4'")
                .fetch_one(&config.db)
                .await?;
        assert_eq!(owner, original);
        Ok(())
    }

    #[tokio::test]
    async fn repeated_quota_cleanup_does_not_hide_more_archives_after_unlink_failure() -> Result<()>
    {
        let (config, _owner, first, _) = fixture(true).await?;
        let rule = db::camera_recording_rules::get_rule(1, &config.db)
            .await?
            .unwrap();
        let db::camera_recording_sessions::SessionAction::Created(second) =
            db::camera_recording_sessions::start_or_extend_session(
                &rule,
                "second",
                "second",
                Utc::now(),
                &config.db,
            )
            .await?
        else {
            panic!("new session expected")
        };
        db::camera_recording_sessions::mark_recording(second, &config.db).await?;
        let segment =
            db::camera_recording_segments::create_segment(second, rule.camera_id, 0, 5, &config.db)
                .await?;
        register_recording_paths(segment, "second.mp4", &config.db).await?;
        let root = Path::new(&config.camera_recording_storage_root);
        tokio::fs::write(root.join("second.mp4"), vec![0u8; 600_000]).await?;
        db::camera_recording_segments::mark_ready(
            segment,
            "second.mp4",
            600_000,
            Utc::now(),
            &config.db,
        )
        .await?;
        db::camera_recording_sessions::request_stop(second, &config.db).await?;
        db::camera_recording_sessions::mark_ready_if_due(second, Utc::now(), &config.db).await?;
        sqlx::query("UPDATE recording_files SET size_bytes=600000 WHERE path='segment.mp4'")
            .execute(&config.db)
            .await?;
        tokio::fs::remove_file(root.join("segment.mp4")).await?;
        tokio::fs::create_dir(root.join("segment.mp4")).await?;
        db::settings::set_i64(db::settings::CAMERA_RECORDING_MAX_STORAGE_MB, 1, &config.db).await?;
        for _ in 0..3 {
            crate::core::maintenance::cleanup_camera_recordings(&config).await;
            assert!(
                db::camera_recording_sessions::get_session(second, &config.db)
                    .await?
                    .is_some()
            );
            assert!(root.join("second.mp4").exists());
        }
        assert!(
            db::camera_recording_sessions::get_session(first, &config.db)
                .await?
                .is_none()
        );
        tokio::fs::remove_dir(root.join("segment.mp4")).await?;
        sqlx::query("UPDATE recording_files SET retry_at=NULL")
            .execute(&config.db)
            .await?;
        crate::core::maintenance::cleanup_camera_recordings(&config).await;
        assert_eq!(
            db::camera_recording_segments::pending_file_count(&config.db).await?,
            0
        );
        assert!(
            db::camera_recording_sessions::get_session(second, &config.db)
                .await?
                .is_some()
        );
        Ok(())
    }
    #[tokio::test]
    async fn archive_attachment_becomes_ready_while_both_compression_slots_are_busy() -> Result<()>
    {
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let (mut config, _owner, session_id, main_segment) = fixture(false).await?;
        Arc::get_mut(&mut config).unwrap().camera_pre_roll_registry =
            Arc::new(crate::core::camera_pre_roll::prepared_registry_for_test()?);
        sqlx::query("UPDATE camera_recording_segments SET segment_index=1 WHERE id=?")
            .bind(main_segment)
            .execute(&config.db)
            .await?;
        sqlx::query("UPDATE camera_recording_sessions SET pre_roll_seconds=1 WHERE id=?")
            .bind(session_id)
            .execute(&config.db)
            .await?;
        let session = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        let camera = db::cameras::get_camera(session.camera_id, &config.db)
            .await?
            .unwrap();
        let local_capacity = crate::core::cameras::reserve_local_media_capacity_for_test().await?;
        tokio::time::timeout(
            StdDuration::from_secs(1),
            attach_pre_roll_segment(&config, &camera, &session, Utc::now()),
        )
        .await??;
        let segments =
            db::camera_recording_segments::list_session_segments(session_id, &config.db).await?;
        let head = segments
            .iter()
            .find(|segment| segment.segment_index == 0)
            .unwrap();
        assert_eq!(head.status, "ready");
        assert!(head.error.is_none());
        let file_path = head.file_path.as_deref().unwrap();
        assert!(
            tokio::fs::metadata(Path::new(&config.camera_recording_storage_root).join(file_path))
                .await?
                .len()
                > 0
        );
        let updated = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        assert!(updated.pre_roll_from.is_some());
        assert_eq!(updated.pre_roll_seconds, 1);
        assert_eq!(updated.pre_roll_attempted_seconds, 1);
        assert_eq!(updated.pre_roll_actual_seconds, 0);
        assert!(updated.pre_roll_warning.is_some());
        drop(local_capacity);
        wait_for_derivatives(file_path).await?;
        Ok(())
    }

    #[tokio::test]
    async fn worker_attaches_merged_pre_roll_when_initial_rule_has_it_disabled() -> Result<()> {
        let _pool_test = crate::core::cameras::NATIVE_POOL_TEST_LOCK.lock().await;
        let (mut config, _owner, session_id, main_segment) = fixture(false).await?;
        Arc::get_mut(&mut config).unwrap().camera_pre_roll_registry =
            Arc::new(crate::core::camera_pre_roll::prepared_registry_for_test()?);
        sqlx::query("UPDATE camera_recording_segments SET segment_index=1 WHERE id=?")
            .bind(main_segment)
            .execute(&config.db)
            .await?;
        db::camera_recording_segments::mark_ready(
            main_segment,
            "segment.mp4",
            4096,
            Utc::now(),
            &config.db,
        )
        .await?;
        sqlx::query("UPDATE camera_recording_rules SET notify_enabled=0 WHERE id=1")
            .execute(&config.db)
            .await?;
        let initial = db::camera_recording_rules::get_rule(1, &config.db)
            .await?
            .unwrap();
        assert!(!initial.pre_roll_enabled());
        let joined_id = db::camera_recording_rules::create_rule(
            db::camera_recording_rules::NewRecordingRule {
                name: "with head",
                camera_id: initial.camera_id,
                condition_logic: db::camera_recording_rules::ConditionLogic::Any,
                tail_seconds: 5,
                max_segment_seconds: 60,
                cooldown_s: 0,
                retention_days: 30,
                pre_roll_enabled: true,
                pre_roll_seconds: 15,
            },
            &config.db,
        )
        .await?;
        let joined = db::camera_recording_rules::get_rule(joined_id, &config.db)
            .await?
            .unwrap();
        let before = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(before.pre_roll_seconds, 0);
        assert_eq!(
            db::camera_recording_sessions::start_or_extend_session(
                &joined,
                "joined",
                "joined pre-roll",
                before.first_event_at,
                &config.db,
            )
            .await?,
            db::camera_recording_sessions::SessionAction::Extended(session_id)
        );
        assert_eq!(
            db::camera_recording_sessions::get_session(session_id, &config.db)
                .await?
                .unwrap()
                .pre_roll_seconds,
            15
        );

        // The live tail already ended. The production worker must still attach
        // the merged head before finalization; it never opens the RTSP URL.
        tokio::time::timeout(
            StdDuration::from_secs(3),
            process_recording_job(
                Bot::with_client(
                    "123:test",
                    teloxide::net::default_reqwest_settings()
                        .no_proxy()
                        .build()?,
                ),
                config.clone(),
                RecordingJob { session_id },
            ),
        )
        .await??;
        let session = db::camera_recording_sessions::get_session(session_id, &config.db)
            .await?
            .unwrap();
        assert_eq!(session.status, "ready");
        assert!(session.pre_roll_from.is_some());
        let head = db::camera_recording_segments::list_ready_segments(session_id, &config.db)
            .await?
            .into_iter()
            .find(|part| part.segment_index == 0)
            .expect("joined pre-roll head");
        assert!(head.size_bytes.is_some_and(|bytes| bytes > 0));
        assert!(Path::new(&config.camera_recording_storage_root)
            .join(head.file_path.as_deref().unwrap())
            .exists());
        assert_eq!(head.expires_at, session.expires_at);
        wait_for_derivatives(head.file_path.as_deref().unwrap()).await?;
        Ok(())
    }
}
