use crate::bot::models::View;
use crate::bot::router::{
    ActivityLogFilter, AdminPayload, Payload, RenderContext, SettingsPayload, State,
};
use crate::i18n::t;

use anyhow::Result;
use chrono::{DateTime, Duration, Local, Utc};
use std::collections::HashMap;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext) -> Result<View> {
    let text = t(ctx.lang, "admin.menu").to_string();
    let kb = make_keyboard(ctx.lang);

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
            t(ctx.lang, "admin.add"),
            Payload::Admin(AdminPayload::PromptAddUser).to_string(),
        ),
        InlineKeyboardButton::callback(
            t(ctx.lang, "admin.delete"),
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

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::ListActions),
    )]);

    Ok(View {
        notifications: ctx.notifications,
        text: t(ctx.lang, "admin.users.title").to_string(),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::ListUsers),
        ..Default::default()
    })
}

pub async fn render_user_profile(ctx: RenderContext, user_id: u64) -> Result<View> {
    let role = crate::db::access::get_user_role(user_id, &ctx.config.db).await?;
    let user_lang = crate::db::get_user_language(user_id, &ctx.config.db)
        .await?
        .unwrap_or(ctx.config.default_language);
    let summary = crate::db::access::get_access_summary(
        user_id,
        user_id == ctx.config.root_user,
        &ctx.config.db,
    )
    .await?;
    let root_note = if user_id == ctx.config.root_user {
        t(ctx.lang, "admin.user_profile.root_note")
    } else {
        ""
    };

    let role_hint = if user_id == ctx.config.root_user {
        ""
    } else {
        t(ctx.lang, "admin.user_profile.role_hint")
    };

    let text = format!(
        "{}\n\nID: {}\n{}: {}\n{}: {}\n{}\n\n{}: ✅ {} / 👁 {} / 🚫 {}\n{}: ✅ {} / 👁 {} / 🚫 {}\n{}: {}{}{}",
        t(ctx.lang, "admin.user_profile.title"),
        user_id,
        t(ctx.lang, "admin.role"),
        role,
        t(ctx.lang, "admin.language"),
        user_lang.label(),
        root_note,
        t(ctx.lang, "admin.rooms.label"),
        summary.rooms_full,
        summary.rooms_view,
        summary.rooms_hidden,
        t(ctx.lang, "admin.devices.label"),
        summary.devices_full,
        summary.devices_view,
        summary.devices_hidden,
        t(ctx.lang, "admin.notifications_off"),
        summary.notifications_disabled,
        if role_hint.is_empty() { "" } else { "\n" },
        role_hint
    );

    let mut rows = Vec::new();

    if user_id != ctx.config.root_user {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "admin.change_role"),
            Payload::Admin(AdminPayload::CycleUserRole { id: user_id }).to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "admin.reset_access"),
            Payload::Admin(AdminPayload::ResetUserAccess { id: user_id }).to_string(),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        format!("{}: {}", t(ctx.lang, "admin.language"), user_lang.label()),
        Payload::Admin(AdminPayload::CycleUserLanguage { id: user_id }).to_string(),
    )]);

    rows.push(vec![InlineKeyboardButton::callback(
        t(ctx.lang, "admin.rooms"),
        Payload::Admin(AdminPayload::UserRooms { id: user_id }).to_string(),
    )]);

    if user_id != ctx.config.root_user {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "admin.delete_user"),
            Payload::Admin(AdminPayload::ConfirmDeleteUser { id: user_id }).to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
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

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
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
    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            "➕ Добавить камеру",
            Payload::Admin(AdminPayload::PromptAddCamera { room: room_id }).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "⚙️ Правила записи",
            Payload::Admin(AdminPayload::RecordingRules { room: room_id }).to_string(),
        )],
    ];

    for camera in &cameras {
        rows.push(vec![
            InlineKeyboardButton::callback(
                format!(
                    "📹 #{} {} · {}с",
                    camera.id, camera.name, camera.clip_seconds
                ),
                Payload::Admin(AdminPayload::RoomCameraDetail {
                    room: room_id,
                    camera: camera.id,
                })
                .to_string(),
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

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
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

pub async fn render_room_camera_detail(
    ctx: RenderContext,
    room_id: i64,
    camera_id: i64,
) -> Result<View> {
    let room = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Room not found"))?;
    let Some(camera) = crate::db::cameras::get_camera(camera_id, &ctx.config.db).await? else {
        let mut view = render_room_cameras(ctx, room_id).await?;
        view.alert = Some("Камера не найдена или уже удалена".to_string());
        return Ok(view);
    };
    if camera.room_id != Some(room_id) {
        let mut view = render_room_cameras(ctx, room_id).await?;
        view.alert = Some("Камера не принадлежит выбранной комнате".to_string());
        return Ok(view);
    }

    let mut rows = vec![vec![InlineKeyboardButton::callback(
        "📸 Снимок",
        Payload::Admin(AdminPayload::RoomCameraSnapshot {
            room: room_id,
            camera: camera.id,
        })
        .to_string(),
    )]];

    let mut interval_row = Vec::new();
    for seconds in &ctx.config.camera_clip_intervals_s {
        interval_row.push(InlineKeyboardButton::callback(
            format!("🎞 {}с", seconds),
            Payload::Admin(AdminPayload::RoomCameraClip {
                room: room_id,
                camera: camera.id,
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

    rows.push(vec![InlineKeyboardButton::callback(
        "🩺 Health",
        Payload::Admin(AdminPayload::CameraHealth {
            room: room_id,
            camera: camera.id,
        })
        .to_string(),
    )]);
    rows.push(vec![InlineKeyboardButton::callback(
        "🗑 Удалить камеру",
        Payload::Admin(AdminPayload::ConfirmDeleteCamera {
            room: room_id,
            camera: camera.id,
        })
        .to_string(),
    )]);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RoomCameras { room: room_id }),
    )]);

    let text = format!(
        "Камера комнаты\n\nКомната: {}\nКамера: {}\nID: `{}`\nКлип по умолчанию: {}с.",
        room.display_name(),
        camera.name,
        camera.id,
        camera.clip_seconds
    );

    Ok(View {
        header: Some("📹 Камера".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RoomCameraDetail {
            room: room_id,
            camera: camera_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rules(ctx: RenderContext, room_id: i64) -> Result<View> {
    let room = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Room not found"))?;
    let cameras = crate::db::cameras::list_room_cameras(room_id, &ctx.config.db).await?;
    let camera_ids = cameras.iter().map(|camera| camera.id).collect::<Vec<_>>();
    let rules = crate::db::camera_recording_rules::list_rules(&ctx.config.db).await?;
    let default_retention = crate::db::settings::get_i64(
        crate::db::settings::CAMERA_RECORDING_DEFAULT_RETENTION_DAYS,
        &ctx.config.db,
    )
    .await?
    .unwrap_or(30);
    let quota = crate::db::settings::get_i64(
        crate::db::settings::CAMERA_RECORDING_MAX_STORAGE_MB,
        &ctx.config.db,
    )
    .await?
    .unwrap_or(0);

    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            "➕ Добавить правило",
            Payload::Admin(AdminPayload::PromptAddRecordingRule { room: room_id }).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "🧩 Группы правил",
            Payload::Admin(AdminPayload::RecordingRuleGroupsForRoom { room: room_id }).to_string(),
        )],
        vec![
            InlineKeyboardButton::callback(
                format!("🗓 Хранить: {}д", default_retention),
                Payload::Admin(AdminPayload::CycleRecordingDefaultRetention { room: room_id })
                    .to_string(),
            ),
            InlineKeyboardButton::callback(
                format!("💽 Квота: {}", crate::bot::format::quota(quota)),
                Payload::Admin(AdminPayload::CycleRecordingStorageQuota { room: room_id })
                    .to_string(),
            ),
        ],
    ];

    let mut shown = 0;
    for rule in rules
        .iter()
        .filter(|rule| camera_ids.contains(&rule.camera_id))
    {
        shown += 1;
        let icon = if rule.enabled != 0 { "✅" } else { "⏸" };
        rows.push(vec![
            InlineKeyboardButton::callback(
                format!(
                    "📄 {} · cam {} · {}с",
                    rule.name, rule.camera_id, rule.tail_seconds
                ),
                Payload::Admin(AdminPayload::RecordingRuleDetail {
                    room: room_id,
                    rule: rule.id,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                icon,
                Payload::Admin(AdminPayload::ToggleRecordingRule {
                    room: room_id,
                    rule: rule.id,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                "🗑",
                Payload::Admin(AdminPayload::ConfirmDeleteRecordingRule {
                    room: room_id,
                    rule: rule.id,
                })
                .to_string(),
            ),
        ]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RoomCameras { room: room_id }),
    )]);

    let cameras_text = cameras
        .iter()
        .map(|camera| format!("{}: {}", camera.id, camera.name))
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "Правила записи\n\nКомната: {}\nПравил: {}\n\nКамеры:\n{}",
        room.display_name(),
        shown,
        if cameras_text.is_empty() {
            "нет камер".to_string()
        } else {
            cameras_text
        }
    );

    Ok(View {
        header: Some("⚙️ Правила записи".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRules { room: room_id }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_detail(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let room = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Room not found"))?;
    let cameras = crate::db::cameras::list_room_cameras(room_id, &ctx.config.db).await?;
    let Some(rule) = crate::db::camera_recording_rules::get_rule(rule_id, &ctx.config.db).await?
    else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или уже удалено".to_string());
        return Ok(view);
    };
    let Some(camera) = cameras.iter().find(|camera| camera.id == rule.camera_id) else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };
    let conditions =
        crate::db::camera_recording_rules::list_conditions(rule.id, &ctx.config.db).await?;
    let display_names = device_display_names(&ctx).await?;
    let last_session =
        crate::db::camera_recording_sessions::get_last_rule_session(rule.id, &ctx.config.db)
            .await?;
    let groups = crate::db::camera_recording_rule_groups::list_groups(&ctx.config.db).await?;
    let rule_group_ids =
        crate::db::camera_recording_rule_groups::list_rule_group_ids(rule.id, &ctx.config.db)
            .await?;
    let sessions_24h = crate::db::camera_recording_sessions::count_rule_sessions_since(
        rule.id,
        Utc::now() - Duration::hours(24),
        &ctx.config.db,
    )
    .await?;
    let status = if rule.is_enabled() {
        "включено"
    } else {
        "на паузе"
    };
    let toggle_label = if rule.is_enabled() {
        "⏸ Пауза"
    } else {
        "▶️ Включить"
    };
    let rule_text = format_recording_rule_text(&rule, &conditions);
    let conditions_text = format_recording_conditions_summary(&conditions, &display_names);
    let history_text = format_rule_history(last_session.as_ref(), sessions_24h);
    let notify_text = if rule.notifications_enabled() {
        "включены"
    } else {
        "выключены"
    };
    let noise_text = if rule.noise_enabled() {
        "включен"
    } else {
        "выключен"
    };
    let short_segment_warning = if rule.max_segment_seconds < 60 {
        "\n\n⚠️ Max segment меньше 60с. Для теста нормально, но в рабочем режиме будет много мелких файлов."
    } else {
        ""
    };

    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            toggle_label,
            Payload::Admin(AdminPayload::ToggleRecordingRuleDetail {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
        vec![
            InlineKeyboardButton::callback(
                "✏️ Изменить",
                Payload::Admin(AdminPayload::PromptEditRecordingRule {
                    room: room_id,
                    rule: rule.id,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                "📋 Дублировать",
                Payload::Admin(AdminPayload::DuplicateRecordingRule {
                    room: room_id,
                    rule: rule.id,
                })
                .to_string(),
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                "🧪 Проверить",
                Payload::Admin(AdminPayload::TestRecordingRule {
                    room: room_id,
                    rule: rule.id,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                if rule.notifications_enabled() {
                    "🔔 Увед."
                } else {
                    "🔕 Увед."
                },
                Payload::Admin(AdminPayload::ToggleRecordingRuleNotify {
                    room: room_id,
                    rule: rule.id,
                })
                .to_string(),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            if rule.noise_enabled() {
                "🔇 Шумодав"
            } else {
                "🔈 Шумодав"
            },
            Payload::Admin(AdminPayload::ToggleRecordingRuleNoise {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
    ];

    for group in &groups {
        let marker = if rule_group_ids.contains(&group.id) {
            "✅"
        } else {
            "➕"
        };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} группа: {}", marker, group.name),
            Payload::Admin(AdminPayload::ToggleRuleGroupItem {
                room: room_id,
                rule: rule.id,
                group: group.id,
            })
            .to_string(),
        )]);
    }

    rows.extend(vec![
        vec![InlineKeyboardButton::callback(
            "🗑 Удалить правило",
            Payload::Admin(AdminPayload::ConfirmDeleteRecordingRule {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RecordingRules { room: room_id }),
        )],
    ]);

    let text = format!(
        "Правило записи\n\nID правила: {}\nНазвание: {}\nСтатус: {}\nУведомления: {}\nШумодав: {}\nКомната: {}\nКамера: {} ({})\nЛогика: {}\nУсловий: {}\nTail: {}с\nMax segment: {}с\nCooldown: {}с\nХранение: {}д\n\nИстория:\n{}\n\nУсловия:\n{}\n\nТекст правила для копирования и изменения:\n{}\n\nЧтобы изменить правило: нажмите ✏️ Изменить и отправьте исправленный блок.{}",
        rule.id,
        rule.name,
        status,
        notify_text,
        noise_text,
        room.display_name(),
        camera.name,
        camera.id,
        rule.condition_logic,
        conditions.len(),
        rule.tail_seconds,
        rule.max_segment_seconds,
        rule.cooldown_s,
        rule.retention_days,
        history_text,
        conditions_text,
        rule_text,
        short_segment_warning,
    );

    Ok(View {
        header: Some("⚙️ Правило записи".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleDetail {
            room: room_id,
            rule: rule_id,
        }),
        ..Default::default()
    })
}

pub async fn render_edit_recording_rule_input(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = crate::db::camera_recording_rules::get_rule(rule_id, &ctx.config.db).await?
    else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или уже удалено".to_string());
        return Ok(view);
    };
    let conditions =
        crate::db::camera_recording_rules::list_conditions(rule.id, &ctx.config.db).await?;
    let rule_text = format_recording_rule_text(&rule, &conditions);

    Ok(View {
        header: Some("✏️ Изменение правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Изменение правила записи\n\nОтправьте исправленный блок ниже:\n\n{}",
            rule_text
        ),
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RecordingRuleDetail {
                room: room_id,
                rule: rule_id,
            }),
        )]]),
        payload: Payload::Admin(AdminPayload::RecordingRuleDetail {
            room: room_id,
            rule: rule_id,
        }),
        next_state: Some(State::EditRecordingRule { room_id, rule_id }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_test(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = crate::db::camera_recording_rules::get_rule(rule_id, &ctx.config.db).await?
    else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или уже удалено".to_string());
        return Ok(view);
    };
    let conditions =
        crate::db::camera_recording_rules::list_conditions(rule.id, &ctx.config.db).await?;
    let display_names = device_display_names(&ctx).await?;
    let entity_ids = conditions
        .iter()
        .map(|condition| condition.entity_id.clone())
        .collect::<Vec<_>>();
    let states = ctx
        .config
        .ha_client
        .fetch_states_by_ids(&entity_ids)
        .await?;
    let state_map = states
        .into_iter()
        .map(|entity| (entity.entity_id, entity.state))
        .collect::<HashMap<_, _>>();

    let mut passed_current = 0;
    let mut current_checks = 0;
    let mut event_checks = 0;
    let mut lines = Vec::new();
    for condition in &conditions {
        let current = state_map.get(&condition.entity_id).map(String::as_str);
        if condition.operator().is_event_operator() {
            event_checks += 1;
            let state_text = current.unwrap_or("не найдено");
            lines.push(format!(
                "🕓 {} · сейчас: {} · ждет переход",
                format_recording_condition_human(condition, &display_names),
                state_text
            ));
            continue;
        }

        current_checks += 1;
        let matches = current
            .map(|state| condition_matches_current(condition, state))
            .unwrap_or(false);
        let icon = if matches { "✅" } else { "❌" };
        if matches {
            passed_current += 1;
        }
        let state_text = current.unwrap_or("не найдено");
        lines.push(format!(
            "{} {} · сейчас: {}",
            icon,
            format_recording_condition_human(condition, &display_names),
            state_text
        ));
    }

    let status =
        format_rule_test_status(rule.logic(), current_checks, passed_current, event_checks);

    Ok(View {
        header: Some("🧪 Проверка правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Проверка правила\n\nПравило: {}\nЛогика: {}\nИтог: {}\n\n{}",
            rule.name,
            rule.condition_logic,
            status,
            if lines.is_empty() {
                "нет условий".to_string()
            } else {
                lines.join("\n")
            }
        ),
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RecordingRuleDetail {
                room: room_id,
                rule: rule_id,
            }),
        )]]),
        payload: Payload::Admin(AdminPayload::RecordingRuleDetail {
            room: room_id,
            rule: rule_id,
        }),
        ..Default::default()
    })
}

pub async fn render_add_recording_rule_input(ctx: RenderContext, room_id: i64) -> Result<View> {
    let cameras = crate::db::cameras::list_room_cameras(room_id, &ctx.config.db).await?;
    let default_retention = crate::db::settings::get_i64(
        crate::db::settings::CAMERA_RECORDING_DEFAULT_RETENTION_DAYS,
        &ctx.config.db,
    )
    .await?
    .unwrap_or(30);
    let cameras_text = cameras
        .iter()
        .map(|camera| format!("{}: {}", camera.id, camera.name))
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "Добавление правила записи\n\nКамеры:\n{}\n\nФормат:\nНазвание\nID камеры\nЛогика: any или all\nУсловие: entity_id;operator;from;to;value\nМожно несколько строк условий\nTail seconds\nMax segment seconds\nCooldown seconds\nRetention days\n\nПример:\nДверь открыта\n{}\nany\nbinary_sensor.front_door;changed_from_to;off;on;\n60\n300\n0\n{}",
        if cameras_text.is_empty() {
            "нет камер".to_string()
        } else {
            cameras_text
        },
        cameras.first().map(|camera| camera.id).unwrap_or(0),
        default_retention
    );

    Ok(View {
        header: Some("🛠 Правило записи".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RecordingRules { room: room_id }),
        )]]),
        payload: Payload::Admin(AdminPayload::RecordingRules { room: room_id }),
        next_state: Some(State::AddRecordingRule { room_id }),
        ..Default::default()
    })
}

fn format_recording_rule_text(
    rule: &crate::db::camera_recording_rules::RecordingRule,
    conditions: &[crate::db::camera_recording_rules::RecordingRuleCondition],
) -> String {
    let mut lines = vec![
        rule.name.clone(),
        rule.camera_id.to_string(),
        rule.condition_logic.clone(),
    ];

    lines.extend(conditions.iter().map(format_recording_condition_line));
    lines.push(rule.tail_seconds.to_string());
    lines.push(rule.max_segment_seconds.to_string());
    lines.push(rule.cooldown_s.to_string());
    lines.push(rule.retention_days.to_string());
    lines.join("\n")
}

fn format_recording_condition_line(
    condition: &crate::db::camera_recording_rules::RecordingRuleCondition,
) -> String {
    format!(
        "{};{};{};{};{}",
        condition.entity_id,
        condition.operator,
        condition.from_state.as_deref().unwrap_or(""),
        condition.to_state.as_deref().unwrap_or(""),
        condition.value.as_deref().unwrap_or("")
    )
}

fn format_recording_conditions_summary(
    conditions: &[crate::db::camera_recording_rules::RecordingRuleCondition],
    display_names: &HashMap<String, String>,
) -> String {
    if conditions.is_empty() {
        return "нет условий".to_string();
    }

    conditions
        .iter()
        .enumerate()
        .map(|(index, condition)| {
            format!(
                "{}. {}",
                index + 1,
                format_recording_condition_human(condition, display_names)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn device_display_names(ctx: &RenderContext) -> Result<HashMap<String, String>> {
    Ok(crate::db::devices::get_all_display_names(&ctx.config.db)
        .await?
        .into_iter()
        .collect())
}

fn format_recording_condition_human(
    condition: &crate::db::camera_recording_rules::RecordingRuleCondition,
    display_names: &HashMap<String, String>,
) -> String {
    let name = display_names
        .get(&condition.entity_id)
        .map(String::as_str)
        .unwrap_or(&condition.entity_id);
    let target = condition
        .value
        .as_deref()
        .or(condition.to_state.as_deref())
        .unwrap_or("*");

    match condition.operator() {
        crate::db::camera_recording_rules::ConditionOperator::ChangedTo => {
            format!(
                "{}: меняется на {}",
                name,
                condition.to_state.as_deref().unwrap_or("*")
            )
        }
        crate::db::camera_recording_rules::ConditionOperator::ChangedFromTo => format!(
            "{}: {} -> {}",
            name,
            condition.from_state.as_deref().unwrap_or("*"),
            condition.to_state.as_deref().unwrap_or("*")
        ),
        crate::db::camera_recording_rules::ConditionOperator::Is => {
            format!("{}: равно {}", name, target)
        }
        crate::db::camera_recording_rules::ConditionOperator::IsNot => {
            format!("{}: не равно {}", name, target)
        }
        crate::db::camera_recording_rules::ConditionOperator::Contains => {
            format!("{}: содержит {}", name, target)
        }
        crate::db::camera_recording_rules::ConditionOperator::Above => {
            format!("{}: выше {}", name, target)
        }
        crate::db::camera_recording_rules::ConditionOperator::Below => {
            format!("{}: ниже {}", name, target)
        }
    }
}

fn format_rule_history(
    last_session: Option<&crate::db::camera_recording_sessions::RecordingSession>,
    sessions_24h: i64,
) -> String {
    let last = last_session
        .map(|session| {
            let status = match session.status.as_str() {
                "ready" => "готова",
                "recording" | "queued" => "идет",
                "failed" => "ошибка",
                _ => session.status.as_str(),
            };
            format!(
                "последнее: {} · {}",
                crate::bot::format::datetime(session.created_at),
                status
            )
        })
        .unwrap_or_else(|| "последнее: нет".to_string());

    let error = last_session
        .and_then(|session| session.error.as_deref())
        .map(|error| format!("\nпоследняя ошибка: {}", error))
        .unwrap_or_else(|| "\nпоследняя ошибка: нет".to_string());

    format!("{}\nза 24ч: {}{}", last, sessions_24h, error)
}

fn format_rule_test_status(
    logic: crate::db::camera_recording_rules::ConditionLogic,
    current_checks: usize,
    passed_current: usize,
    event_checks: usize,
) -> &'static str {
    match (logic, current_checks, event_checks) {
        (_, 0, 0) => "нет условий для проверки",
        (_, 0, _) => "событийное правило: ждет переход",
        (crate::db::camera_recording_rules::ConditionLogic::Any, _, 0) => {
            if passed_current > 0 {
                "текущее состояние подходит"
            } else {
                "текущее состояние не подходит"
            }
        }
        (crate::db::camera_recording_rules::ConditionLogic::Any, _, _) => {
            if passed_current > 0 {
                "текущее состояние подходит для части условий; событийные ждут переход"
            } else {
                "текущие условия не подходят; событийные ждут переход"
            }
        }
        (crate::db::camera_recording_rules::ConditionLogic::All, _, 0) => {
            if passed_current == current_checks {
                "текущее состояние подходит"
            } else {
                "текущее состояние не подходит"
            }
        }
        (crate::db::camera_recording_rules::ConditionLogic::All, _, _) => {
            if passed_current == current_checks {
                "контекст подходит; событийные условия ждут переход"
            } else {
                "контекст не подходит; событийные условия ждут переход"
            }
        }
    }
}

fn condition_matches_current(
    condition: &crate::db::camera_recording_rules::RecordingRuleCondition,
    current_state: &str,
) -> bool {
    let expected = condition
        .value
        .as_deref()
        .or(condition.to_state.as_deref())
        .unwrap_or("");

    match condition.operator() {
        crate::db::camera_recording_rules::ConditionOperator::ChangedTo
        | crate::db::camera_recording_rules::ConditionOperator::ChangedFromTo
        | crate::db::camera_recording_rules::ConditionOperator::Is => current_state == expected,
        crate::db::camera_recording_rules::ConditionOperator::IsNot => current_state != expected,
        crate::db::camera_recording_rules::ConditionOperator::Contains => {
            current_state.contains(expected)
        }
        crate::db::camera_recording_rules::ConditionOperator::Above => {
            compare_numeric(current_state, expected, |left, right| left > right)
        }
        crate::db::camera_recording_rules::ConditionOperator::Below => {
            compare_numeric(current_state, expected, |left, right| left < right)
        }
    }
}

fn compare_numeric<F>(current: &str, expected: &str, compare: F) -> bool
where
    F: Fn(f64, f64) -> bool,
{
    let Ok(left) = current.parse::<f64>() else {
        return false;
    };
    let Ok(right) = expected.parse::<f64>() else {
        return false;
    };
    compare(left, right)
}

pub async fn render_ui_background(ctx: RenderContext) -> Result<View> {
    let cameras =
        crate::db::cameras::list_accessible_cameras(ctx.user_id, true, &ctx.config.db).await?;
    let selected = crate::core::ui_background::selected_camera_id(&ctx.config).await;
    let interval = crate::core::ui_background::refresh_interval_s(&ctx.config).await;
    let selected_name = selected
        .and_then(|id| cameras.iter().find(|camera| camera.id == id))
        .map(|camera| camera.name.clone())
        .unwrap_or_else(|| t(ctx.lang, "admin.ui_background.stock").to_string());

    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            format!(
                "{}: {}с",
                t(ctx.lang, "admin.ui_background.interval"),
                interval
            ),
            Payload::Admin(AdminPayload::CycleUiBackgroundInterval).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(ctx.lang, "admin.ui_background.stock_button"),
            Payload::Admin(AdminPayload::SetUiBackgroundCamera { camera: None }).to_string(),
        )],
    ];

    for camera in cameras {
        let marker = if Some(camera.id) == selected {
            "✅"
        } else {
            "📹"
        };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} {}", marker, camera.name),
            Payload::Admin(AdminPayload::SetUiBackgroundCamera {
                camera: Some(camera.id),
            })
            .to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::ListActions),
    )]);

    Ok(View {
        header: Some(t(ctx.lang, "admin.ui_background").to_string()),
        notifications: ctx.notifications,
        text: format!(
            "{}\n\n{}: {}\n{}: {}с\n\n{}",
            t(ctx.lang, "admin.ui_background.title"),
            t(ctx.lang, "admin.ui_background.current"),
            selected_name,
            t(ctx.lang, "admin.ui_background.interval"),
            interval,
            t(ctx.lang, "admin.ui_background.hint")
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::UiBackground),
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
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RoomCameras { room: room_id }),
        )]]),
        payload: Payload::Admin(AdminPayload::RoomCameras { room: room_id }),
        next_state: Some(State::AddCamera { room_id }),
        ..Default::default()
    })
}

pub async fn render_camera_health(
    ctx: RenderContext,
    room_id: i64,
    camera_id: i64,
) -> Result<View> {
    let lang = ctx.lang;
    let Some(camera) = crate::db::cameras::get_camera(camera_id, &ctx.config.db).await? else {
        let mut view = render_room_cameras(ctx, room_id).await?;
        view.alert = Some(t(lang, "admin.camera.health.not_found").to_string());
        return Ok(view);
    };
    let health = crate::db::camera_health::get(camera_id, &ctx.config.db).await?;
    let text = format_camera_health_text(ctx.lang, &camera.name, health.as_ref());

    let rows = vec![
        vec![InlineKeyboardButton::callback(
            t(ctx.lang, "admin.camera.health.check_now"),
            Payload::Admin(AdminPayload::CheckCameraHealth {
                room: room_id,
                camera: camera_id,
            })
            .to_string(),
        )],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RoomCameraDetail {
                room: room_id,
                camera: camera_id,
            }),
        )],
    ];

    Ok(View {
        header: Some(t(ctx.lang, "admin.camera.health").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::CameraHealth {
            room: room_id,
            camera: camera_id,
        }),
        ..Default::default()
    })
}

pub async fn render_activity_log(ctx: RenderContext, filter: ActivityLogFilter) -> Result<View> {
    let (kind, only_errors, title) = match filter {
        ActivityLogFilter::All => (None, false, t(ctx.lang, "admin.activity_log.all")),
        ActivityLogFilter::Errors => (None, true, t(ctx.lang, "admin.activity_log.errors")),
        ActivityLogFilter::Cameras => (
            Some("camera"),
            false,
            t(ctx.lang, "admin.activity_log.cameras"),
        ),
        ActivityLogFilter::Devices => (Some("device"), false, t(ctx.lang, "admin.devices.label")),
        ActivityLogFilter::Recording => (
            Some("recording"),
            false,
            t(ctx.lang, "admin.activity_log.recording"),
        ),
    };
    let entries =
        crate::db::activity_log::list_recent(kind, only_errors, 30, &ctx.config.db).await?;
    let rows = vec![
        vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "admin.activity_log.all"),
                Payload::Admin(AdminPayload::ActivityLog {
                    filter: ActivityLogFilter::All,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "admin.activity_log.errors"),
                Payload::Admin(AdminPayload::ActivityLog {
                    filter: ActivityLogFilter::Errors,
                })
                .to_string(),
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "admin.activity_log.cameras"),
                Payload::Admin(AdminPayload::ActivityLog {
                    filter: ActivityLogFilter::Cameras,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "admin.activity_log.recording"),
                Payload::Admin(AdminPayload::ActivityLog {
                    filter: ActivityLogFilter::Recording,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "admin.activity_log.devices_short"),
                Payload::Admin(AdminPayload::ActivityLog {
                    filter: ActivityLogFilter::Devices,
                })
                .to_string(),
            ),
        ],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::ListActions),
        )],
    ];

    let lines = entries
        .iter()
        .map(format_activity_log_entry)
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "{}\n\n{}: {}\n\n{}",
        t(ctx.lang, "admin.activity_log.title"),
        t(ctx.lang, "admin.activity_log.filter"),
        title,
        if lines.is_empty() {
            t(ctx.lang, "admin.activity_log.empty").to_string()
        } else {
            lines
        }
    );

    Ok(View {
        header: Some(t(ctx.lang, "admin.activity_log").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::ActivityLog { filter }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_groups(ctx: RenderContext) -> Result<View> {
    render_recording_rule_groups_with_back(ctx, None).await
}

pub async fn render_recording_rule_groups_for_room(
    ctx: RenderContext,
    room_id: i64,
) -> Result<View> {
    render_recording_rule_groups_with_back(ctx, Some(room_id)).await
}

async fn render_recording_rule_groups_with_back(
    ctx: RenderContext,
    room_id: Option<i64>,
) -> Result<View> {
    let groups = crate::db::camera_recording_rule_groups::list_groups(&ctx.config.db).await?;
    let back_payload = room_id
        .map(|room| Payload::Admin(AdminPayload::RecordingRules { room }))
        .unwrap_or(Payload::Admin(AdminPayload::ListActions));
    let current_payload = room_id
        .map(|room| Payload::Admin(AdminPayload::RecordingRuleGroupsForRoom { room }))
        .unwrap_or(Payload::Admin(AdminPayload::RecordingRuleGroups));
    let ensure_payload = room_id
        .map(|room| Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForRoom { room }))
        .unwrap_or(Payload::Admin(AdminPayload::EnsureDefaultRuleGroups));
    let mut rows = Vec::new();
    rows.push(vec![InlineKeyboardButton::callback(
        t(ctx.lang, "admin.rule_groups.create_defaults"),
        ensure_payload.to_string(),
    )]);
    for group in &groups {
        let icon = if group.is_enabled() { "✅" } else { "⏸" };
        let toggle_payload = room_id
            .map(|room| {
                Payload::Admin(AdminPayload::ToggleRecordingRuleGroupForRoom {
                    room,
                    group: group.id,
                })
            })
            .unwrap_or(Payload::Admin(AdminPayload::ToggleRecordingRuleGroup {
                group: group.id,
            }));
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {} · {} {}",
                icon,
                group.name,
                t(ctx.lang, "admin.rule_groups.rules_count"),
                group.rules_count
            ),
            toggle_payload.to_string(),
        )]);
    }
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        back_payload,
    )]);

    let text = if groups.is_empty() {
        t(ctx.lang, "admin.rule_groups.empty").to_string()
    } else {
        t(ctx.lang, "admin.rule_groups.hint").to_string()
    };

    Ok(View {
        header: Some(t(ctx.lang, "admin.rule_groups").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_payload,
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

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
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
                format!("{} #{} {} · {}", icon, device.id, name, label),
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

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
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

fn format_camera_health_text(
    lang: crate::i18n::Language,
    camera_name: &str,
    health: Option<&crate::db::camera_health::CameraHealth>,
) -> String {
    let Some(health) = health else {
        return format!(
            "{} · Health\n\n{}",
            camera_name,
            t(lang, "admin.camera.health.no_checks")
        );
    };

    format!(
        "{} · Health\n\nSnapshot OK: {}\nClip OK: {}\nRecording OK: {}\n{}: {}\n{}: {}\n{}: {}",
        camera_name,
        crate::bot::format::optional_datetime(health.last_snapshot_ok_at),
        crate::bot::format::optional_datetime(health.last_clip_ok_at),
        crate::bot::format::optional_datetime(health.last_recording_ok_at),
        t(lang, "admin.camera.health.last_check"),
        crate::bot::format::optional_datetime(health.last_check_at),
        t(lang, "admin.camera.health.last_size"),
        health
            .last_file_size
            .map(|size| crate::bot::format::bytes(size as u64))
            .unwrap_or_else(|| "нет".to_string()),
        t(lang, "admin.camera.health.last_error"),
        health.last_error.as_deref().unwrap_or("нет")
    )
}

fn format_activity_log_entry(entry: &crate::db::activity_log::ActivityLogEntry) -> String {
    let icon = match entry.status.as_str() {
        "ok" => "✅",
        "error" => "⚠️",
        _ => "•",
    };
    let user = entry
        .user_id
        .map(|id| format!("u{}", id))
        .unwrap_or_else(|| "system".to_string());
    let entity = entry.entity_id.as_deref().unwrap_or("-");
    let message = entry
        .message
        .as_deref()
        .map(|message| format!(" · {}", message))
        .unwrap_or_default();

    format!(
        "{} {} · {} · {}:{} · {}{}",
        icon,
        crate::bot::format::datetime(entry.created_at),
        user,
        entry.entity_type,
        entity,
        entry.action,
        message
    )
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
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::ListActions),
        )],
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
        InlineKeyboardButton::callback(t(ctx.lang, "common.confirm"), confirm_payload.to_string()),
        InlineKeyboardButton::callback(t(ctx.lang, "common.cancel"), cancel_payload.to_string()),
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

pub fn render_user_input(
    ctx: RenderContext,
    next_state: State,
    title: &str,
    text: &str,
    back_payload: Payload,
) -> View {
    View {
        header: Some(format!("🛠 {}", title)),
        notifications: ctx.notifications,
        text: text.to_string(),
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            back_payload.clone(),
        )]]),
        payload: back_payload,
        next_state: Some(next_state),
        ..Default::default()
    }
}

pub fn make_keyboard(lang: crate::i18n::Language) -> InlineKeyboardMarkup {
    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.users"),
            Payload::Admin(AdminPayload::ListUsers).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.status"),
            Payload::Admin(AdminPayload::Status).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.cameras"),
            Payload::Admin(AdminPayload::CameraRooms).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.ui_background"),
            Payload::Admin(AdminPayload::UiBackground).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.activity_log"),
            Payload::Admin(AdminPayload::ActivityLog {
                filter: ActivityLogFilter::All,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.rule_groups"),
            Payload::Admin(AdminPayload::RecordingRuleGroups).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.settings"),
            Payload::Settings(SettingsPayload::ListRooms).to_string(),
        )],
    ];

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        lang,
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

    #[test]
    fn recording_rule_copy_text_matches_input_format() {
        let rule = crate::db::camera_recording_rules::RecordingRule {
            id: 10,
            name: "Коридор свет".to_string(),
            camera_id: 7,
            condition_logic: "any".to_string(),
            tail_seconds: 60,
            max_segment_seconds: 300,
            cooldown_s: 0,
            retention_days: 30,
            enabled: 1,
            notify_enabled: 1,
            noise_enabled: 1,
            noise_summary_sent_at: None,
            last_completed_at: None,
            deleted_at: None,
        };
        let conditions = vec![
            crate::db::camera_recording_rules::RecordingRuleCondition {
                id: 1,
                rule_id: 10,
                entity_id: "light.corridor".to_string(),
                operator: "changed_from_to".to_string(),
                from_state: Some("off".to_string()),
                to_state: Some("on".to_string()),
                value: None,
            },
            crate::db::camera_recording_rules::RecordingRuleCondition {
                id: 2,
                rule_id: 10,
                entity_id: "light.corridor".to_string(),
                operator: "changed_from_to".to_string(),
                from_state: Some("on".to_string()),
                to_state: Some("off".to_string()),
                value: None,
            },
        ];

        let text = format_recording_rule_text(&rule, &conditions);

        assert_eq!(
            text,
            "Коридор свет\n7\nany\nlight.corridor;changed_from_to;off;on;\nlight.corridor;changed_from_to;on;off;\n60\n300\n0\n30"
        );
    }

    #[test]
    fn recording_rule_test_status_marks_event_rules_as_waiting() {
        assert_eq!(
            format_rule_test_status(
                crate::db::camera_recording_rules::ConditionLogic::Any,
                0,
                0,
                2
            ),
            "событийное правило: ждет переход"
        );
        assert_eq!(
            format_rule_test_status(
                crate::db::camera_recording_rules::ConditionLogic::All,
                1,
                1,
                1
            ),
            "контекст подходит; событийные условия ждут переход"
        );
    }
}
