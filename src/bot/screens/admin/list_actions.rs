use crate::bot::models::View;
use crate::bot::router::{
    AdminPayload, CameraPayload, Payload, RenderContext, SettingsPayload, State,
};

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext) -> Result<View> {
    let text = "Админ меню".to_string();
    let kb = make_keyboard();

    Ok(View {
        notifications: ctx.notifications.clone(),
        text,
        kb,
        payload: Payload::Admin(AdminPayload::ListActions),
        ..Default::default()
    })
}

pub async fn render_users(ctx: RenderContext) -> Result<View> {
    let users = crate::db::list_users(&ctx.config.db).await?;
    let mut rows = vec![vec![
        InlineKeyboardButton::callback(
            "➕ Добавить",
            Payload::Admin(AdminPayload::PromptAddUser).to_string(),
        ),
        InlineKeyboardButton::callback(
            "➖ Удалить",
            Payload::Admin(AdminPayload::PromptDeleteUser).to_string(),
        ),
    ]];

    for user in users {
        if user == ctx.config.root_user {
            rows.push(vec![InlineKeyboardButton::callback(
                format!("👑 {} root", user),
                Payload::Admin(AdminPayload::UserProfile { id: user }).to_string(),
            )]);
        } else {
            rows.push(vec![InlineKeyboardButton::callback(
                format!("👤 {}", user),
                Payload::Admin(AdminPayload::UserProfile { id: user }).to_string(),
            )]);
        }
    }

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Admin(AdminPayload::ListActions),
    )]);

    Ok(View {
        notifications: ctx.notifications,
        text: "Пользователи с доступом".to_string(),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::ListUsers),
        ..Default::default()
    })
}

pub async fn render_user_profile(ctx: RenderContext, user_id: u64) -> Result<View> {
    let role = crate::db::access::get_user_role(user_id, &ctx.config.db).await?;
    let summary = crate::db::access::get_access_summary(
        user_id,
        user_id == ctx.config.root_user,
        &ctx.config.db,
    )
    .await?;
    let root_note = if user_id == ctx.config.root_user {
        "\nRoot имеет полный доступ. Ограничения профиля не применяются."
    } else {
        ""
    };

    let role_hint = if user_id == ctx.config.root_user {
        ""
    } else {
        "\nРоли переключаются по кругу: user → child → guest. Для child/guest комнаты закрываются, нужные откройте вручную."
    };

    let text = format!(
        "Профиль пользователя\n\nID: {}\nРоль: {}{}\n\nКомнаты: ✅ {} / 👁 {} / 🚫 {}\nУстройства: ✅ {} / 👁 {} / 🚫 {}\nУведомления выключены: {}{}",
        user_id,
        role,
        root_note,
        summary.rooms_full,
        summary.rooms_view,
        summary.rooms_hidden,
        summary.devices_full,
        summary.devices_view,
        summary.devices_hidden,
        summary.notifications_disabled,
        role_hint
    );

    let mut rows = Vec::new();

    if user_id != ctx.config.root_user {
        rows.push(vec![InlineKeyboardButton::callback(
            "🔁 Сменить роль",
            Payload::Admin(AdminPayload::CycleUserRole { id: user_id }).to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            "♻️ Сбросить доступы",
            Payload::Admin(AdminPayload::ResetUserAccess { id: user_id }).to_string(),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "🏠 Комнаты",
        Payload::Admin(AdminPayload::UserRooms { id: user_id }).to_string(),
    )]);

    if user_id != ctx.config.root_user {
        rows.push(vec![InlineKeyboardButton::callback(
            "🗑 Удалить пользователя",
            Payload::Admin(AdminPayload::ConfirmDeleteUser { id: user_id }).to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Admin(AdminPayload::ListUsers),
    )]);

    Ok(View {
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::UserProfile { id: user_id }),
        ..Default::default()
    })
}

pub async fn render_camera_rooms(ctx: RenderContext) -> Result<View> {
    let rooms = crate::db::rooms::get_rooms(&ctx.config.db).await?;
    let mut rows = Vec::new();

    for room in rooms {
        let cameras_count = crate::db::cameras::count_room_cameras(room.id, &ctx.config.db).await?;
        rows.push(vec![InlineKeyboardButton::callback(
            format!("📹 {} · {}", room.display_name(), cameras_count),
            Payload::Admin(AdminPayload::RoomCameras { room: room.id }).to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Admin(AdminPayload::ListActions),
    )]);

    Ok(View {
        notifications: ctx.notifications,
        text: "Камеры по комнатам\n\nВыберите комнату, чтобы добавить или удалить камеру."
            .to_string(),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::CameraRooms),
        ..Default::default()
    })
}

pub async fn render_room_cameras(ctx: RenderContext, room_id: i64) -> Result<View> {
    let room = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Room not found"))?;
    let cameras = crate::db::cameras::list_room_cameras(room_id, &ctx.config.db).await?;
    let mut rows = vec![vec![InlineKeyboardButton::callback(
        "➕ Добавить камеру",
        Payload::Admin(AdminPayload::PromptAddCamera { room: room_id }).to_string(),
    )]];

    for camera in &cameras {
        rows.push(vec![
            InlineKeyboardButton::callback(
                format!("📹 {} · {}с", camera.name, camera.clip_seconds),
                Payload::Camera(CameraPayload::CameraDetail { id: camera.id }).to_string(),
            ),
            InlineKeyboardButton::callback(
                "🗑",
                Payload::Admin(AdminPayload::ConfirmDeleteCamera {
                    room: room_id,
                    camera: camera.id,
                })
                .to_string(),
            ),
        ]);
    }

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Admin(AdminPayload::CameraRooms),
    )]);

    let text = if cameras.is_empty() {
        format!(
            "Камеры комнаты\n\nКомната: {}\nКамер пока нет.",
            room.display_name()
        )
    } else {
        format!(
            "Камеры комнаты\n\nКомната: {}\nКамер: {}",
            room.display_name(),
            cameras.len()
        )
    };

    Ok(View {
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RoomCameras { room: room_id }),
        ..Default::default()
    })
}

pub async fn render_add_camera_input(ctx: RenderContext, room_id: i64) -> Result<View> {
    let room = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Room not found"))?;
    let text = format!(
        "Добавление камеры\n\nКомната: {}\n\nВведите данные камеры строками:\nНазвание\nRTSP URL\nИнтервал видео в секундах\nSnapshot URL необязательно\n\nПример:\nВход\nrtsp://user:pass@192.168.1.50:554/stream1\n{}",
        room.display_name(),
        ctx.config.camera_default_clip_s
    );

    Ok(View {
        header: Some("🛠 Добавление камеры".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button(
            Payload::Admin(AdminPayload::RoomCameras { room: room_id }),
        )]]),
        payload: Payload::Admin(AdminPayload::RoomCameras { room: room_id }),
        next_state: Some(State::AddCamera { room_id }),
        ..Default::default()
    })
}

pub async fn render_user_rooms(ctx: RenderContext, user_id: u64) -> Result<View> {
    let rooms = crate::db::rooms::get_rooms(&ctx.config.db).await?;
    let mut rows = Vec::new();

    for room in rooms {
        let access = crate::db::access::get_room_access(user_id, room.id, &ctx.config.db).await?;
        let (icon, label) = if user_id == ctx.config.root_user {
            device_access_label(true, true)
        } else {
            device_access_label(access.can_view, access.can_control)
        };

        rows.push(vec![
            InlineKeyboardButton::callback(
                format!("{} {} · {}", icon, room.display_name(), label),
                Payload::Admin(AdminPayload::ToggleUserRoomAccess {
                    id: user_id,
                    room: room.id,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                "💡 Устр.",
                Payload::Admin(AdminPayload::UserRoomDevices {
                    id: user_id,
                    room: room.id,
                })
                .to_string(),
            ),
        ]);
    }

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Admin(AdminPayload::UserProfile { id: user_id }),
    )]);

    let text = if user_id == ctx.config.root_user {
        format!(
            "Доступ к комнатам\n\nПользователь: {}\nRoot всегда видит все комнаты.",
            user_id
        )
    } else {
        format!(
            "Доступ к комнатам\n\nПользователь: {}\nНажатие меняет режим: полный доступ, просмотр, скрыта.",
            user_id
        )
    };

    Ok(View {
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::UserRooms { id: user_id }),
        ..Default::default()
    })
}

pub async fn render_user_room_devices(
    ctx: RenderContext,
    user_id: u64,
    room_id: i64,
) -> Result<View> {
    let room = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Room not found"))?;
    let devices = crate::db::devices::get_devices_by_room(room_id, &ctx.config.db).await?;
    let mut rows = Vec::new();

    for device in devices {
        let access = crate::db::access::get_effective_device_access(
            user_id,
            user_id == ctx.config.root_user,
            device.id,
            &ctx.config.db,
        )
        .await?;
        let can_notify =
            crate::db::access::get_device_notify_access(user_id, &device.entity_id, &ctx.config.db)
                .await?;
        let (icon, label) = device_access_label(access.can_view, access.can_control);
        let notify_icon = if access.can_view && can_notify {
            "🔔"
        } else {
            "🔕"
        };
        let name = device.alias.as_deref().unwrap_or(&device.entity_id);

        rows.push(vec![
            InlineKeyboardButton::callback(
                format!("{} {} · {}", icon, name, label),
                Payload::Admin(AdminPayload::ToggleUserDeviceAccess {
                    id: user_id,
                    room: room_id,
                    device: device.id,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                notify_icon,
                Payload::Admin(AdminPayload::ToggleUserDeviceNotify {
                    id: user_id,
                    room: room_id,
                    device: device.id,
                })
                .to_string(),
            ),
        ]);
    }

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Admin(AdminPayload::UserRooms { id: user_id }),
    )]);

    let text = if user_id == ctx.config.root_user {
        format!(
            "Доступ к устройствам\n\nПользователь: {}\nКомната: {}\nRoot всегда имеет полный доступ.",
            user_id,
            room.display_name()
        )
    } else {
        format!(
            "Доступ к устройствам\n\nПользователь: {}\nКомната: {}\nНажатие меняет режим: полный доступ, просмотр, скрыто.",
            user_id,
            room.display_name()
        )
    };

    Ok(View {
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::UserRoomDevices {
            id: user_id,
            room: room_id,
        }),
        ..Default::default()
    })
}

fn device_access_label(can_view: bool, can_control: bool) -> (&'static str, &'static str) {
    match (can_view, can_control) {
        (true, true) => ("✅", "полный"),
        (true, false) => ("👁", "просмотр"),
        (false, _) => ("🚫", "скрыто"),
    }
}

pub async fn render_status(ctx: RenderContext) -> Result<View> {
    let stats = crate::db::get_system_stats(&ctx.config.db).await?;
    let ha_status = match ctx.config.ha_client.health_check().await {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("Ошибка HA: {}", e),
    };
    let (blocked_sessions, nearest_unblock) = summarize_ui_refresh_blocks(
        ctx.config
            .sessions
            .iter()
            .map(|entry| entry.value().ui_refresh_blocked_until),
        Utc::now(),
    );
    let runtime_status = ctx.config.runtime_status.read().await.clone();
    let last_heartbeat = format_optional_local_time(runtime_status.last_maintenance_tick_at);
    let ha_sync = format_ha_sync_status(
        runtime_status.last_ha_sync_at,
        runtime_status.last_ha_sync_error.as_deref(),
    );

    let text = format!(
        "Статус системы\n\nHA: {}\nПоследний heartbeat: {}\nHA sync: {}\nПользователей: {}\nКомнат: {}\nАктивных устройств: {}\nАрхивных устройств: {}\nПодписок: {}\nСобытий в журнале: {}\nАктивных сессий: {}\nUI refresh на паузе: {}\nБлижайшая разблокировка: {}",
        ha_status,
        last_heartbeat,
        ha_sync,
        stats.users,
        stats.rooms,
        stats.active_devices,
        stats.archived_devices,
        stats.subscriptions,
        stats.recent_events,
        ctx.config.sessions.len(),
        blocked_sessions,
        nearest_unblock,
    );

    let kb = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                "🔄 Обновить",
                Payload::Admin(AdminPayload::Status).to_string(),
            ),
            InlineKeyboardButton::callback(
                "💾 Backup DB",
                Payload::Admin(AdminPayload::ConfirmBackup).to_string(),
            ),
        ],
        vec![crate::bot::screens::common::back_button(Payload::Admin(
            AdminPayload::ListActions,
        ))],
    ]);

    Ok(View {
        notifications: ctx.notifications,
        text,
        kb,
        payload: Payload::Admin(AdminPayload::Status),
        ..Default::default()
    })
}

fn summarize_ui_refresh_blocks<I>(blocked_until_values: I, now: DateTime<Utc>) -> (usize, String)
where
    I: IntoIterator<Item = Option<DateTime<Utc>>>,
{
    let active_blocks: Vec<DateTime<Utc>> = blocked_until_values
        .into_iter()
        .flatten()
        .filter(|blocked_until| *blocked_until > now)
        .collect();

    let nearest_unblock = active_blocks
        .iter()
        .min()
        .map(|dt| dt.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "нет".to_string());

    (active_blocks.len(), nearest_unblock)
}

fn format_optional_local_time(value: Option<DateTime<Utc>>) -> String {
    value
        .map(|dt| dt.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "нет".to_string())
}

fn format_ha_sync_status(last_sync_at: Option<DateTime<Utc>>, last_error: Option<&str>) -> String {
    if let Some(error) = last_error {
        return format!("Ошибка: {}", error);
    }

    last_sync_at
        .map(|dt| format!("OK {}", dt.with_timezone(&Local).format("%H:%M:%S")))
        .unwrap_or_else(|| "еще не выполнялась".to_string())
}

pub fn render_confirm_action(
    ctx: RenderContext,
    title: &str,
    text: &str,
    confirm_payload: Payload,
    cancel_payload: Payload,
) -> View {
    let kb = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("✅ Подтвердить", confirm_payload.to_string()),
        InlineKeyboardButton::callback("↩️ Отмена", cancel_payload.to_string()),
    ]]);

    View {
        header: Some(format!("🛠 {}", title)),
        notifications: ctx.notifications,
        text: text.to_string(),
        kb,
        payload: cancel_payload,
        ..Default::default()
    }
}

pub fn render_user_input(ctx: RenderContext, next_state: State, title: &str, text: &str) -> View {
    View {
        header: Some(format!("🛠 {}", title)),
        notifications: ctx.notifications,
        text: text.to_string(),
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button(
            Payload::Admin(AdminPayload::ListUsers),
        )]]),
        payload: Payload::Admin(AdminPayload::ListUsers),
        next_state: Some(next_state),
        ..Default::default()
    }
}

pub fn make_keyboard() -> InlineKeyboardMarkup {
    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            "👥 Список пользователей",
            Payload::Admin(AdminPayload::ListUsers).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "📊 Статус системы",
            Payload::Admin(AdminPayload::Status).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "📹 Камеры",
            Payload::Admin(AdminPayload::CameraRooms).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "⚙️ Настройки",
            Payload::Settings(SettingsPayload::ListRooms).to_string(),
        )],
    ];

    rows.push(vec![crate::bot::screens::common::back_button(
        Payload::Home,
    )]);
    InlineKeyboardMarkup::new(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_refresh_block_summary_ignores_expired_blocks() {
        let now = Utc::now();
        let (count, nearest) = summarize_ui_refresh_blocks(
            [
                Some(now - chrono::Duration::seconds(1)),
                Some(now + chrono::Duration::seconds(30)),
                Some(now + chrono::Duration::seconds(10)),
                None,
            ],
            now,
        );

        assert_eq!(count, 2);
        assert_ne!(nearest, "нет");
    }

    #[test]
    fn ui_refresh_block_summary_reports_empty_state() {
        let now = Utc::now();
        let (count, nearest) = summarize_ui_refresh_blocks([None], now);

        assert_eq!(count, 0);
        assert_eq!(nearest, "нет");
    }

    #[test]
    fn ha_sync_status_prefers_error_over_old_success() {
        let now = Utc::now();

        assert_eq!(
            format_ha_sync_status(Some(now), Some("network down")),
            "Ошибка: network down"
        );
    }

    #[test]
    fn ha_sync_status_reports_missing_sync() {
        assert_eq!(format_ha_sync_status(None, None), "еще не выполнялась");
    }

    #[test]
    fn device_access_label_matches_access_mode() {
        assert_eq!(device_access_label(true, true), ("✅", "полный"));
        assert_eq!(device_access_label(true, false), ("👁", "просмотр"));
        assert_eq!(device_access_label(false, false), ("🚫", "скрыто"));
    }
}
