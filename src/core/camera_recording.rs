use crate::bot::utils::md;
use crate::db;
use crate::models::{AppConfig, NotificationData};
use anyhow::{ensure, Context, Result};
use chrono::{Duration, Utc};
use log::{debug, error, info, warn};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;
use teloxide::Bot;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::{JoinError, JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

const RECORDING_SHUTDOWN_TIMEOUT: StdDuration = StdDuration::from_secs(20);

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
        if let Err(error) = recover_stale_recordings(&config).await {
            error!("Camera recording recovery failed: {}", error);
        }

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

    let Some(rule) = db::camera_recording_rules::get_rule(session.rule_id, &config.db).await?
    else {
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

    let mut segment_index = next_segment_index(session.id, &config).await?;

    loop {
        let Some(current) =
            db::camera_recording_sessions::get_session(session.id, &config.db).await?
        else {
            return Ok(());
        };

        if current.status == "failed" || current.status == "deleted" {
            return Ok(());
        }

        let now = Utc::now();
        let remaining = current
            .stop_after_at
            .signed_duration_since(now)
            .num_seconds();
        if remaining <= 0 {
            break;
        }

        let segment_duration = remaining
            .min(rule.max_segment_seconds)
            .max(1)
            .try_into()
            .unwrap_or(u32::MAX);
        let segment_id = db::camera_recording_segments::create_segment(
            session.id,
            session.camera_id,
            segment_index,
            i64::from(segment_duration),
            current.expires_at,
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
                let _ =
                    db::camera_health::mark_error(session.camera_id, &message, &config.db).await;
                db::camera_recording_segments::mark_failed(segment_id, &message, &config.db)
                    .await?;
                db::camera_recording_sessions::mark_failed(session.id, &message, &config.db)
                    .await?;
                return Ok(());
            }
        }

        segment_index += 1;
    }

    let completed_at = Utc::now();
    db::camera_recording_sessions::mark_ready(session.id, completed_at, &config.db).await?;
    db::camera_recording_rules::mark_rule_completed(rule.id, completed_at, &config.db).await?;
    send_ready_notification(bot, config, session.id).await?;

    Ok(())
}

async fn capture_and_store_segment(
    config: &Arc<AppConfig>,
    camera: &db::cameras::Camera,
    session_id: i64,
    segment_index: i64,
    segment_duration: u32,
) -> Result<(String, i64)> {
    let bytes = crate::core::cameras::capture_clip(camera, segment_duration).await?;
    ensure!(
        !bytes.is_empty(),
        "camera clip capture returned an empty recording"
    );
    let relative_path = segment_relative_path(camera.id, session_id, segment_index);
    let final_path = Path::new(&config.camera_recording_storage_root).join(&relative_path);
    let tmp_path = tmp_segment_path(&final_path);

    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!("failed to create recording directory {}", parent.display())
        })?;
    }

    tokio::fs::write(&tmp_path, &bytes)
        .await
        .with_context(|| format!("failed to write tmp recording {}", tmp_path.display()))?;
    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .with_context(|| format!("failed to publish recording {}", final_path.display()))?;

    let metadata = tokio::fs::metadata(&final_path).await.with_context(|| {
        format!(
            "failed to stat published recording {}",
            final_path.display()
        )
    })?;
    ensure!(
        metadata.is_file(),
        "published recording path is not a file: {}",
        final_path.display()
    );
    let size_bytes = metadata.len();
    if size_bytes == 0 {
        let _ = tokio::fs::remove_file(&final_path).await;
        anyhow::bail!(
            "published recording file is empty: {}",
            final_path.display()
        );
    }
    if size_bytes != bytes.len() as u64 {
        warn!(
            "Published recording size differs from captured bytes: session={}, camera={} {}, path={}, captured={} bytes, stored={} bytes",
            session_id,
            camera.id,
            camera.name,
            final_path.display(),
            bytes.len(),
            size_bytes
        );
    }
    info!(
        "Published recording file: session={}, camera={} {}, path={}, size={} bytes",
        session_id,
        camera.id,
        camera.name,
        final_path.display(),
        size_bytes
    );

    Ok((relative_path, i64::try_from(size_bytes).unwrap_or(i64::MAX)))
}

fn segment_relative_path(camera_id: i64, session_id: i64, segment_index: i64) -> String {
    let date = Utc::now().format("%Y/%m/%d");
    format!(
        "{}/camera_{}_session_{}_segment_{}.mp4",
        date, camera_id, session_id, segment_index
    )
}

fn tmp_segment_path(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    final_path.with_file_name(format!(".{}.tmp", file_name))
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

    let Some(rule) = db::camera_recording_rules::get_rule(session.rule_id, &config.db).await?
    else {
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

    for entity_id in trigger_entities(&session.trigger_summary) {
        for user_id in db::subscriptions::get_subscribers(&entity_id, &config.db)
            .await
            .unwrap_or_default()
        {
            recipients.insert(user_id);
        }
    }

    if recipients.is_empty() {
        return Ok(());
    }

    if should_suppress_recording_notification(&rule, config.clone(), &recipients, bot.clone())
        .await?
    {
        db::camera_recording_sessions::mark_notification_sent(session_id, &config.db).await?;
        return Ok(());
    }

    let total_duration: i64 = segments.iter().map(|segment| segment.duration_s).sum();
    let text = format!(
        "🎞 Запись с камеры {} готова · {} · {}",
        md::bold(&camera.name),
        format_recording_duration(total_duration),
        format_parts(segments.len())
    );

    crate::bot::notification::send_notification(
        bot,
        config.clone(),
        NotificationData {
            human_state: text,
            recipients: recipients.into_iter().collect(),
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
                human_state: text,
                recipients: recipients.iter().copied().collect(),
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

    cleanup_tmp_files(Path::new(&config.camera_recording_storage_root)).await;
    Ok(())
}

pub(crate) async fn delete_recording_session_files(
    config: &Arc<AppConfig>,
    session_id: i64,
) -> Result<()> {
    let paths =
        db::camera_recording_segments::soft_delete_segments_for_session(session_id, &config.db)
            .await?;

    for path in paths {
        let full_path = Path::new(&config.camera_recording_storage_root).join(path);
        delete_recording_file_and_derivatives(&full_path).await;
    }

    db::camera_recording_sessions::soft_delete_session(session_id, &config.db).await?;
    Ok(())
}

async fn delete_recording_file_and_derivatives(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await {
        debug!(
            "Failed to delete recording file {}: {}",
            path.display(),
            error
        );
    }

    let Some(parent) = path.parent() else {
        return;
    };
    let Some(stem) = path.file_stem().map(|value| value.to_string_lossy()) else {
        return;
    };
    let prefix = format!("{}.telegram", stem);

    let Ok(mut dir) = tokio::fs::read_dir(parent).await else {
        return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let entry_path = entry.path();
        let Some(name) = entry_path.file_name().map(|value| value.to_string_lossy()) else {
            continue;
        };
        if name.starts_with(&prefix) && name.ends_with(".mp4") {
            if let Err(error) = tokio::fs::remove_file(&entry_path).await {
                debug!(
                    "Failed to delete derived recording file {}: {}",
                    entry_path.display(),
                    error
                );
            }
        }
    }
}

async fn cleanup_tmp_files(root: &Path) {
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
            } else if is_tmp_recording_file(&entry_path) {
                if let Err(error) = tokio::fs::remove_file(&entry_path).await {
                    debug!(
                        "Failed to remove tmp recording {}: {}",
                        entry_path.display(),
                        error
                    );
                }
            }
        }
    }
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

    #[test]
    fn tmp_recording_files_include_tmp_mp4_outputs() {
        assert!(is_tmp_recording_file(Path::new(
            ".camera_1_segment_1.telegram.tmp.mp4"
        )));
        assert!(is_tmp_recording_file(Path::new("recording.tmp")));
        assert!(!is_tmp_recording_file(Path::new(
            "camera_1_segment_1.telegram.v4.mp4"
        )));
    }
}
