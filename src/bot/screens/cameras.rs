use crate::bot::models::View;
use crate::bot::router::{CameraPayload, Payload, RenderContext};
use anyhow::{Context, Result};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render_list(ctx: RenderContext) -> Result<View> {
    let cameras =
        crate::db::cameras::list_accessible_cameras(ctx.user_id, ctx.is_admin, &ctx.config.db)
            .await?;
    let mut rows = Vec::new();

    for camera in cameras {
        rows.push(vec![InlineKeyboardButton::callback(
            format!("📹 {}", camera.name),
            Payload::Camera(CameraPayload::CameraDetail { id: camera.id }).to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Home,
    )]);

    let text = if rows.len() == 1 {
        "Камеры\n\nНет доступных камер.".to_string()
    } else {
        "Камеры\n\nВыберите камеру.".to_string()
    };

    Ok(View {
        header: Some("📹 Камеры".to_string()),
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
        "📸 Снимок",
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

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Camera(CameraPayload::ListCameras),
    )]);

    let text = format!(
        "{}\n\nСнимок отправляется сразу. Видео можно выбрать по длительности.\nДлительность по умолчанию для камеры: {}с.",
        camera.name, camera.clip_seconds
    );

    Ok(View {
        header: Some("📹 Камера".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::CameraDetail { id: camera.id }),
        ..Default::default()
    })
}
