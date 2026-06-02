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

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Home,
    )]);

    let text = if rows.len() == 1 {
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

    Ok(View {
        header: Some(t(ctx.lang, "camera.detail.header").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::CameraDetail { id: camera.id }),
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

    Ok(View {
        header: Some(t(ctx.lang, "camera.archive.header").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingArchive { camera: camera_id }),
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
    let ready_segments = segments
        .iter()
        .filter(|segment| segment.status == "ready" && segment.file_path.is_some())
        .count();
    let total_duration: i64 = segments.iter().map(|segment| segment.duration_s).sum();
    let completed = session.completed_at.unwrap_or_else(Utc::now);
    let partial = session.status == "failed" && ready_segments > 0;
    let sending_all = ctx
        .config
        .is_recording_send_in_progress(ctx.user_id, session_id);

    let mut rows = Vec::new();
    if ready_segments == 1 {
        if let Some(segment) = segments
            .iter()
            .find(|segment| segment.status == "ready" && segment.file_path.is_some())
        {
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

    let text = format!(
        "{}\n\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {} / {}\n{}: {}{}",
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
        warning
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
        ..Default::default()
    })
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
