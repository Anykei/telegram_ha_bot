use crate::bot::models::View;
use crate::bot::router::{CameraPayload, Payload, RenderContext};
use crate::i18n::t;
use anyhow::{Context, Result};
use chrono::Utc;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render_list(ctx: RenderContext) -> Result<View> {
    let cameras =
        crate::db::cameras::list_accessible_cameras(ctx.user_id, ctx.is_admin, &ctx.config.db)
            .await?;
    let cameras_empty = cameras.is_empty();
    let mut rows = Vec::new();

    for camera in cameras {
        let label = if ctx.is_admin {
            format!("📹 #{} {}", camera.id, camera.name)
        } else {
            format!("📹 {}", camera.name)
        };
        rows.push(vec![InlineKeyboardButton::callback(
            label,
            Payload::Camera(CameraPayload::CameraDetail { id: camera.id }).to_string(),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "🎛 Режимы записи",
        Payload::Camera(CameraPayload::RecordingGroupModes).to_string(),
    )]);

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Home,
    )]);

    let text = if cameras_empty {
        t(ctx.lang, "camera.list.empty").to_string()
    } else {
        t(ctx.lang, "camera.list.pick").to_string()
    };

    Ok(View {
        header: Some(t(ctx.lang, "camera.title").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::ListCameras),
        ..Default::default()
    })
}

pub async fn render_recording_group_modes(ctx: RenderContext) -> Result<View> {
    let groups = crate::db::camera_recording_rule_groups::list_groups_visible_to_user(
        ctx.user_id,
        ctx.config.root_user,
        &ctx.config.db,
    )
    .await?;
    let mut rows = Vec::new();
    for group in &groups {
        let icon = if group.is_enabled() { "✅" } else { "⏸" };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} {} · {} правил", icon, group.name, group.rules_count),
            Payload::Camera(CameraPayload::RecordingGroupModeDetail { group: group.id })
                .to_string(),
        )]);
    }
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Camera(CameraPayload::ListCameras),
    )]);

    let text = if groups.is_empty() {
        "Режимы записи\n\nДоступных режимов пока нет.".to_string()
    } else {
        "Режимы записи\n\nВключенная группа разрешает связанные правила. Выключенная группа ставит их на паузу."
            .to_string()
    };

    Ok(View {
        header: Some("🎛 Режимы записи".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingGroupModes),
        ..Default::default()
    })
}

pub async fn render_recording_group_mode_detail(ctx: RenderContext, group_id: i64) -> Result<View> {
    let Some(group) =
        crate::db::camera_recording_rule_groups::get_group(group_id, &ctx.config.db).await?
    else {
        let mut view = render_recording_group_modes(ctx).await?;
        view.alert = Some("Группа не найдена".to_string());
        return Ok(view);
    };
    if !ctx.is_admin && !group.is_visible_to_all_users() {
        let mut view = render_recording_group_modes(ctx).await?;
        view.alert = Some("Группа недоступна".to_string());
        return Ok(view);
    }

    let cameras = crate::db::camera_recording_rule_groups::list_group_cameras_visible_to_user(
        ctx.user_id,
        ctx.is_admin,
        group.id,
        &ctx.config.db,
    )
    .await?;
    let cameras_text = if cameras.is_empty() {
        "нет камер".to_string()
    } else {
        cameras
            .iter()
            .map(|camera| camera.camera_name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let status = if group.is_enabled() {
        "включена"
    } else {
        "на паузе"
    };
    let toggle_label = if group.is_enabled() {
        "⏸ Поставить на паузу"
    } else {
        "▶️ Включить"
    };

    let rows = vec![
        vec![InlineKeyboardButton::callback(
            toggle_label,
            Payload::Camera(CameraPayload::ToggleRecordingGroupMode { group: group.id })
                .to_string(),
        )],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Camera(CameraPayload::RecordingGroupModes),
        )],
    ];

    Ok(View {
        header: Some("🎛 Режим записи".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "{}\n\nСтатус: {}\nПравил: {}\nКамеры: {}",
            group.name, status, group.rules_count, cameras_text
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingGroupModeDetail { group: group.id }),
        ..Default::default()
    })
}

pub async fn render_detail(ctx: RenderContext, camera_id: i64) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;

    let mut rows = vec![vec![InlineKeyboardButton::callback(
        t(ctx.lang, "camera.snapshot"),
        Payload::Camera(CameraPayload::Snapshot { id: camera.id }).to_string(),
    )]];

    let mut interval_row = Vec::new();
    for seconds in &ctx.config.camera_clip_intervals_s {
        interval_row.push(InlineKeyboardButton::callback(
            format!("🎞 {}с", seconds),
            Payload::Camera(CameraPayload::Clip {
                id: camera.id,
                seconds: *seconds,
            })
            .to_string(),
        ));

        if interval_row.len() == 3 {
            rows.push(std::mem::take(&mut interval_row));
        }
    }

    if !interval_row.is_empty() {
        rows.push(interval_row);
    }

    let active_session =
        crate::db::camera_recording_sessions::get_active_camera_session(camera.id, &ctx.config.db)
            .await?;
    if ctx.is_admin {
        if let Some(session) = &active_session {
            rows.push(vec![InlineKeyboardButton::callback(
                t(ctx.lang, "camera.stop_recording"),
                Payload::Camera(CameraPayload::StopRecording {
                    camera: camera.id,
                    session: session.id,
                })
                .to_string(),
            )]);
        }
    }

    rows.push(vec![InlineKeyboardButton::callback(
        t(ctx.lang, "camera.archive.button"),
        Payload::Camera(CameraPayload::RecordingArchive { camera: camera.id }).to_string(),
    )]);

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Camera(CameraPayload::ListCameras),
    )]);

    let recording_text = active_session
        .as_ref()
        .map(|session| {
            let remaining = session
                .stop_after_at
                .signed_duration_since(Utc::now())
                .num_seconds()
                .max(0);
            format!(
                "\n\nREC · {} #{} · {} {} · {}",
                t(ctx.lang, "camera.recording.rule"),
                session.rule_id,
                t(ctx.lang, "camera.recording.remaining"),
                crate::core::camera_recording::format_recording_duration(remaining),
                session.trigger_summary
            )
        })
        .unwrap_or_default();

    let text = format!(
        "{}\n\nID: `{}`{}\n\n{}: {}с.",
        camera.name,
        camera.id,
        recording_text,
        t(ctx.lang, "camera.detail.help"),
        camera.clip_seconds
    );
    let image = camera_snapshot_image(ctx.config.clone(), &camera).await;

    Ok(View {
        header: Some(t(ctx.lang, "camera.detail.header").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::CameraDetail { id: camera.id }),
        image,
        ..Default::default()
    })
}

pub async fn render_recording_archive(ctx: RenderContext, camera_id: i64) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;
    let sessions =
        crate::db::camera_recording_sessions::list_camera_sessions(camera_id, 30, &ctx.config.db)
            .await?;
    let mut rows = Vec::new();

    for session in &sessions {
        rows.push(vec![InlineKeyboardButton::callback(
            recording_session_label(session).await,
            Payload::Camera(CameraPayload::RecordingSession {
                camera: camera_id,
                session: session.id,
            })
            .to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Camera(CameraPayload::CameraDetail { id: camera_id }),
    )]);

    let text = if sessions.is_empty() {
        format!(
            "{} · {}\n\n{}",
            camera.name,
            t(ctx.lang, "camera.archive.header"),
            t(ctx.lang, "camera.archive.empty")
        )
    } else {
        format!(
            "{} · {}\n\n{}: {}",
            camera.name,
            t(ctx.lang, "camera.archive.header"),
            t(ctx.lang, "camera.archive.count"),
            sessions.len()
        )
    };
    let image = recording_archive_preview(ctx.config.clone(), &sessions).await;

    Ok(View {
        header: Some(t(ctx.lang, "camera.archive.header").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingArchive { camera: camera_id }),
        image,
        ..Default::default()
    })
}

pub async fn render_recording_session(
    ctx: RenderContext,
    camera_id: i64,
    session_id: i64,
) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;
    let session = crate::db::camera_recording_sessions::get_session(session_id, &ctx.config.db)
        .await?
        .context("Recording session not found")?;
    if session.camera_id != camera_id {
        anyhow::bail!("Recording session does not belong to camera");
    }
    let segments =
        crate::db::camera_recording_segments::list_session_segments(session_id, &ctx.config.db)
            .await?;
    let (sendable_segments, missing_ready_files) =
        collect_sendable_recording_segments(&ctx.config, session_id, &segments).await;
    let ready_segments = sendable_segments.len();
    let total_duration: i64 = segments.iter().map(|segment| segment.duration_s).sum();
    let completed = session.completed_at.unwrap_or_else(Utc::now);
    let partial = session.status == "failed" && ready_segments > 0;
    let sending_all = ctx
        .config
        .is_recording_send_in_progress(ctx.user_id, session_id);

    let mut rows = Vec::new();
    if ready_segments == 1 {
        if let Some(segment) = sendable_segments.first() {
            rows.push(vec![InlineKeyboardButton::callback(
                t(ctx.lang, "camera.recording.send_video"),
                Payload::Camera(CameraPayload::SendRecordingSegment {
                    camera: camera_id,
                    session: session_id,
                    segment: segment.id,
                })
                .to_string(),
            )]);
        }
    } else if ready_segments > 1 && !sending_all {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "camera.recording.send_all"),
            Payload::Camera(CameraPayload::SendRecordingAll {
                camera: camera_id,
                session: session_id,
            })
            .to_string(),
        )]);
    }

    if ctx.is_admin {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "camera.delete"),
            Payload::Camera(CameraPayload::ConfirmDeleteRecording {
                camera: camera_id,
                session: session_id,
            })
            .to_string(),
        )]);
    }
    if ctx.is_admin && (session.status == "recording" || session.status == "queued") {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "camera.stop_recording"),
            Payload::Camera(CameraPayload::StopRecording {
                camera: camera_id,
                session: session_id,
            })
            .to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Camera(CameraPayload::RecordingArchive { camera: camera_id }),
    )]);

    let warning = if partial {
        format!("\n\n{}", t(ctx.lang, "camera.recording.partial_warning"))
    } else if session.status == "failed" {
        format!("\n\n{}", t(ctx.lang, "camera.recording.failed_warning"))
    } else if session.status == "recording" || session.status == "queued" {
        format!("\n\n{}", t(ctx.lang, "camera.recording.active_warning"))
    } else if sending_all {
        format!("\n\n{}", t(ctx.lang, "camera.recording.sending_warning"))
    } else {
        String::new()
    };
    let disk_warning = recording_disk_warning(ctx.lang, missing_ready_files);
    let failure_details = if session.status == "failed" {
        recording_failure_reason(&session, &segments)
            .map(|reason| {
                format!(
                    "\n{}: {}",
                    t(ctx.lang, "camera.recording.failure_reason"),
                    shorten_recording_error(reason)
                )
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let image = crate::core::camera_recording::first_cached_recording_preview(
        ctx.config.clone(),
        &segments,
    )
    .await;

    let text = format!(
        "{}\n\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {} / {}\n{}: {}{}{}{}",
        t(ctx.lang, "camera.recording.header"),
        t(ctx.lang, "camera.recording.camera"),
        camera.name,
        t(ctx.lang, "camera.recording.event"),
        session.trigger_summary,
        t(ctx.lang, "camera.recording.start"),
        crate::bot::format::datetime(session.first_event_at),
        t(ctx.lang, "camera.recording.end"),
        crate::bot::format::datetime(completed),
        t(ctx.lang, "camera.recording.duration"),
        crate::core::camera_recording::format_recording_duration(total_duration),
        t(ctx.lang, "camera.recording.files"),
        ready_segments,
        segments.len(),
        t(ctx.lang, "camera.recording.keep_until"),
        crate::bot::format::datetime(session.expires_at),
        warning,
        disk_warning,
        failure_details
    );

    Ok(View {
        header: Some(t(ctx.lang, "camera.recording.header").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingSession {
            camera: camera_id,
            session: session_id,
        }),
        image,
        ..Default::default()
    })
}

pub(crate) async fn camera_snapshot_image(
    config: std::sync::Arc<crate::models::AppConfig>,
    camera: &crate::db::cameras::Camera,
) -> Option<Vec<u8>> {
    crate::core::camera_snapshots::resolve(config, camera.clone()).await
}

async fn recording_archive_preview(
    config: std::sync::Arc<crate::models::AppConfig>,
    sessions: &[crate::db::camera_recording_sessions::RecordingSession],
) -> Option<Vec<u8>> {
    let mut first_missing_preview = None;

    for session in sessions {
        let segments =
            match crate::db::camera_recording_segments::list_ready_segments(session.id, &config.db)
                .await
            {
                Ok(segments) => segments,
                Err(error) => {
                    log::debug!(
                        "Failed to load recording preview segments: session={}, error={}",
                        session.id,
                        error
                    );
                    continue;
                }
            };

        match crate::core::camera_recording::first_cached_recording_preview_lookup(
            &config, &segments,
        )
        .await
        {
            crate::core::camera_recording::CachedRecordingPreview::Ready(image) => {
                return Some(image);
            }
            crate::core::camera_recording::CachedRecordingPreview::Missing(file_path) => {
                if first_missing_preview.is_none() {
                    first_missing_preview = Some(file_path);
                }
            }
            crate::core::camera_recording::CachedRecordingPreview::None => {}
        }
    }

    if let Some(file_path) = first_missing_preview {
        crate::core::camera_recording::spawn_recording_derivatives(config, file_path);
    }

    None
}

async fn collect_sendable_recording_segments<'a>(
    config: &crate::models::AppConfig,
    session_id: i64,
    segments: &'a [crate::db::camera_recording_segments::RecordingSegment],
) -> (
    Vec<&'a crate::db::camera_recording_segments::RecordingSegment>,
    usize,
) {
    let mut sendable = Vec::new();
    let mut missing_ready_files = 0usize;

    for segment in segments {
        if segment.status != "ready" {
            continue;
        }

        let Some(file_path) = segment.file_path.as_deref() else {
            missing_ready_files += 1;
            continue;
        };

        match crate::core::camera_recording::recording_file_info(config, file_path).await {
            Ok(_) => sendable.push(segment),
            Err(error) => {
                missing_ready_files += 1;
                log::debug!(
                    "Recording segment is not sendable: session={}, segment={}, path={}, error={:#}",
                    session_id,
                    segment.id,
                    file_path,
                    error
                );
            }
        }
    }

    (sendable, missing_ready_files)
}

fn recording_disk_warning(lang: crate::i18n::Language, missing_ready_files: usize) -> String {
    if missing_ready_files == 0 {
        return String::new();
    }

    match lang {
        crate::i18n::Language::Ru => format!(
            "\n\n⚠️ Недоступных файлов на диске: {}. Проверьте папку хранения записей.",
            missing_ready_files
        ),
        crate::i18n::Language::En => format!(
            "\n\n⚠️ Files unavailable on disk: {}. Check the recordings storage folder.",
            missing_ready_files
        ),
    }
}

fn recording_failure_reason<'a>(
    session: &'a crate::db::camera_recording_sessions::RecordingSession,
    segments: &'a [crate::db::camera_recording_segments::RecordingSegment],
) -> Option<&'a str> {
    session
        .error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            segments
                .iter()
                .filter_map(|segment| segment.error.as_deref())
                .find(|value| !value.trim().is_empty())
        })
}

fn shorten_recording_error(error: &str) -> String {
    const LIMIT: usize = 280;
    let normalized = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= LIMIT {
        return normalized;
    }

    let mut shortened = normalized.chars().take(LIMIT).collect::<String>();
    shortened.push_str("...");
    shortened
}

async fn recording_session_label(
    session: &crate::db::camera_recording_sessions::RecordingSession,
) -> String {
    let status_icon = match session.status.as_str() {
        "ready" => "🎞",
        "recording" | "queued" => "⏳",
        "failed" => "⚠️",
        _ => "🎞",
    };
    let end = session.completed_at.unwrap_or(session.stop_after_at);
    let duration = end
        .signed_duration_since(session.first_event_at)
        .num_seconds()
        .max(0);

    format!(
        "{} {} · {}",
        status_icon,
        crate::bot::format::datetime(session.first_event_at),
        crate::core::camera_recording::format_recording_duration(duration)
    )
}
