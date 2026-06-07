use crate::bot::models::View;
use crate::bot::recording_rule_wizard::{
    self, RecordingRuleWizard, WizardPendingCondition, WizardTriggerMode,
};
use crate::bot::router::{
    ActivityLogFilter, AdminPayload, Payload, RecordingRuleActiveTimePreset,
    RecordingRuleEditField, RecordingRuleGroupRulesFilter, RenderContext, SettingsPayload, State,
};
use crate::db::camera_recording_rule_groups::RecordingRuleGroup;
use crate::db::camera_recording_rules::{ConditionLogic, ConditionOperator};
use crate::i18n::t;
use crate::models::{UiMessageMode, UserSession};

use anyhow::Result;
use chrono::{DateTime, Duration, Local, Utc};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext) -> Result<View> {
    let text = admin_menu_text(ctx.lang);
    let kb = make_keyboard(ctx.lang);

    Ok(View {
        notifications: ctx.notifications.clone(),
        text,
        kb,
        payload: Payload::Admin(AdminPayload::ListActions),
        ..Default::default()
    })
}

fn admin_menu_text(lang: crate::i18n::Language) -> String {
    format!(
        "{}\n{}: v{}",
        t(lang, "admin.menu"),
        t(lang, "admin.version"),
        env!("CARGO_PKG_VERSION")
    )
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
    let voice_allowed =
        crate::db::access::can_use_voice(user_id, user_id == ctx.config.root_user, &ctx.config.db)
            .await?;
    let voice_engine = crate::db::access::get_user_voice_command_engine(
        user_id,
        ctx.config.voice_command_engine,
        &ctx.config.db,
    )
    .await?;
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
        "{}\n\nID: {}\n{}: {}\n{}: {}\n🎙 Голос: {}\n🎙 Engine: {}\n{}\n\n{}: ✅ {} / 👁 {} / 🚫 {}\n{}: ✅ {} / 👁 {} / 🚫 {}\n{}: {}{}{}",
        t(ctx.lang, "admin.user_profile.title"),
        user_id,
        t(ctx.lang, "admin.role"),
        role,
        t(ctx.lang, "admin.language"),
        user_lang.label(),
        if voice_allowed { "ВКЛ" } else { "ВЫКЛ" },
        voice_engine.label(),
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
        rows.push(vec![InlineKeyboardButton::callback(
            if voice_allowed {
                "🎙 Голос: ВКЛ"
            } else {
                "🎙 Голос: ВЫКЛ"
            },
            Payload::Admin(AdminPayload::ToggleUserVoice { id: user_id }).to_string(),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        format!("🎙 Engine: {}", voice_engine.label()),
        Payload::Admin(AdminPayload::CycleUserVoiceEngine { id: user_id }).to_string(),
    )]);

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
            "➕ Мастер",
            Payload::Admin(AdminPayload::StartRecordingRuleWizard { room: room_id }).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "⌨️ Расширенно",
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
    let display_context = recording_condition_display_context(&ctx).await?;
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
    let pause_reason = recording_rule_pause_reason(&rule, &groups, &rule_group_ids);
    let status = if pause_reason.is_some() {
        "на паузе"
    } else {
        "включено"
    };
    let pause_reason_text = pause_reason
        .as_deref()
        .map(|reason| format!("\nПричина паузы: {}", reason))
        .unwrap_or_default();
    let toggle_label = if rule.is_enabled() {
        "⏸ Пауза"
    } else {
        "▶️ Включить"
    };
    let conditions_text =
        format_recording_conditions_summary(&conditions, &display_context, &ctx.config);
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
    let groups_text = selected_group_names_text(&groups, &rule_group_ids);
    let active_time_text = format_rule_active_time(&rule);
    let short_segment_warning = if rule.max_segment_seconds < 60 {
        "\n\n⚠️ Длина файла меньше 60с. Для теста нормально, но в рабочем режиме будет много мелких файлов."
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
                Payload::Admin(AdminPayload::RecordingRuleEditMenu {
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
        "Правило записи\n\nID правила: {}\nНазвание: {}\nСтатус: {}{}\nУведомления: {}\nШумодав: {}\nКомната: {}\nКамера: {} ({})\nЛогика: {}\nУсловий: {}\nАктивно: {}\nПисать после события: {}с\nДлина файла: {}с\nПауза после записи: {}с\nХранение: {}д\nГруппы: {}\n\nИстория:\n{}\n\nУсловия:\n{}\n\nЧтобы изменить правило: нажмите ✏️ Изменить. Текстовый блок доступен в расширенном режиме.{}",
        rule.id,
        rule.name,
        status,
        pause_reason_text,
        notify_text,
        noise_text,
        room.display_name(),
        camera.name,
        camera.id,
        condition_logic_label(rule.logic()),
        conditions.len(),
        active_time_text,
        rule.tail_seconds,
        rule.max_segment_seconds,
        rule.cooldown_s,
        rule.retention_days,
        groups_text,
        history_text,
        conditions_text,
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

pub async fn render_recording_rule_edit_menu(
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
    let rule_group_ids =
        crate::db::camera_recording_rule_groups::list_rule_group_ids(rule.id, &ctx.config.db)
            .await?;
    let group_button_label = if rule_group_ids.is_empty() {
        "👥 Группы: не выбраны".to_string()
    } else {
        format!("👥 Группы: {}", rule_group_ids.len())
    };

    let rows = vec![
        vec![InlineKeyboardButton::callback(
            "📡 Сенсоры",
            Payload::Admin(AdminPayload::RecordingRuleEditSensors {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            group_button_label,
            Payload::Admin(AdminPayload::RecordingRuleEditGroups {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!("🕒 Активно: {}", format_rule_active_time(&rule)),
            Payload::Admin(AdminPayload::RecordingRuleEditActiveTime {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!("⏱ Писать после события: {}с", rule.tail_seconds),
            Payload::Admin(AdminPayload::PromptEditRecordingRuleNumber {
                room: room_id,
                rule: rule.id,
                field: RecordingRuleEditField::TailSeconds,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!("📦 Длина файла: {}с", rule.max_segment_seconds),
            Payload::Admin(AdminPayload::PromptEditRecordingRuleNumber {
                room: room_id,
                rule: rule.id,
                field: RecordingRuleEditField::MaxSegmentSeconds,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!("⏳ Пауза после записи: {}с", rule.cooldown_s),
            Payload::Admin(AdminPayload::PromptEditRecordingRuleNumber {
                room: room_id,
                rule: rule.id,
                field: RecordingRuleEditField::CooldownSeconds,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!("🗓 Хранение: {}д", rule.retention_days),
            Payload::Admin(AdminPayload::PromptEditRecordingRuleNumber {
                room: room_id,
                rule: rule.id,
                field: RecordingRuleEditField::RetentionDays,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "⌨️ Расширенно",
            Payload::Admin(AdminPayload::PromptEditRecordingRule {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RecordingRuleDetail {
                room: room_id,
                rule: rule.id,
            }),
        )],
    ];

    let text = format!(
        "Изменение правила записи\n\nПравило: {}\nКомната: {}\nКамера: {} ({})\n\nЧто изменить?",
        rule.name,
        room.display_name(),
        camera.name,
        camera.id,
    );

    Ok(View {
        header: Some("✏️ Изменение правила".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleEditMenu {
            room: room_id,
            rule: rule_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_edit_groups(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };
    let groups = crate::db::camera_recording_rule_groups::list_groups(&ctx.config.db).await?;
    let rule_group_ids =
        crate::db::camera_recording_rule_groups::list_rule_group_ids(rule.id, &ctx.config.db)
            .await?;

    let mut rows = vec![vec![
        InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.create_custom"),
            Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroupForEdit {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        ),
        InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.create_defaults"),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForEdit {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        ),
    ]];
    for group in &groups {
        let checked = if rule_group_ids.contains(&group.id) {
            "☑"
        } else {
            "☐"
        };
        let state = if group.is_enabled() { "" } else { " ⏸" };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} {}{}", checked, group.name, state),
            Payload::Admin(AdminPayload::ToggleRecordingRuleEditGroupItem {
                room: room_id,
                rule: rule.id,
                group: group.id,
            })
            .to_string(),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "Готово",
        Payload::Admin(AdminPayload::RecordingRuleEditMenu {
            room: room_id,
            rule: rule.id,
        })
        .to_string(),
    )]);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RecordingRuleEditMenu {
            room: room_id,
            rule: rule.id,
        }),
    )]);

    let selected_text = selected_group_names_text(&groups, &rule_group_ids);
    let text = if groups.is_empty() {
        format!(
            "Группы правила\n\nПравило: {}\n\nГруппы еще не созданы. Создайте свою группу, создайте базовые группы или вернитесь к изменению правила.",
            rule.name
        )
    } else {
        format!(
            "Группы правила\n\nПравило: {}\nВыбрано: {}\n\nПравило работает, когда включены все выбранные группы. Отметьте нужные группы галочкой. Можно ничего не выбирать.",
            rule.name, selected_text
        )
    };

    Ok(View {
        header: Some("👥 Группы правила".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleEditGroups {
            room: room_id,
            rule: rule_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_edit_sensors(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };
    let conditions =
        crate::db::camera_recording_rules::list_conditions(rule.id, &ctx.config.db).await?;
    let display_context = recording_condition_display_context(&ctx).await?;
    let conditions_text =
        format_recording_conditions_summary(&conditions, &display_context, &ctx.config);
    let logic_text = condition_logic_explanation(rule.logic());

    let mut rows = vec![
        vec![InlineKeyboardButton::callback(
            "➕ Добавить сенсор",
            Payload::Admin(AdminPayload::RecordingRuleEditSensorPage {
                room: room_id,
                rule: rule.id,
                page: 0,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!("🔀 Логика: {}", condition_logic_short_label(rule.logic())),
            Payload::Admin(AdminPayload::CycleRecordingRuleLogic {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )],
    ];

    for (index, condition) in conditions.iter().enumerate() {
        rows.push(vec![InlineKeyboardButton::callback(
            shorten_button_label(&format!(
                "🗑 {}. {}",
                index + 1,
                format_recording_condition_human(condition, &display_context, &ctx.config)
            )),
            Payload::Admin(AdminPayload::DeleteRecordingRuleCondition {
                room: room_id,
                rule: rule.id,
                condition: condition.id,
            })
            .to_string(),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "⌨️ Изменить условия текстом",
        Payload::Admin(AdminPayload::PromptEditRecordingRule {
            room: room_id,
            rule: rule.id,
        })
        .to_string(),
    )]);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RecordingRuleEditMenu {
            room: room_id,
            rule: rule.id,
        }),
    )]);

    let text = format!(
        "Сенсоры правила\n\nПравило: {}\nЛогика: {}\n\nПодключенные сенсоры:\n{}\n\nНажмите на сенсор с 🗑, чтобы удалить его из правила.",
        rule.name, logic_text, conditions_text
    );

    Ok(View {
        header: Some("📡 Сенсоры правила".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleEditSensors {
            room: room_id,
            rule: rule_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_edit_sensor_page(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
    page: u16,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };

    let candidates =
        crate::db::devices::list_recording_wizard_candidates(room_id, &ctx.config.db).await?;
    let page = bounded_page(page, candidates.len(), WIZARD_ENTITY_PAGE_SIZE);
    let total_pages = total_pages(candidates.len(), WIZARD_ENTITY_PAGE_SIZE);
    let visible = page_slice(&candidates, page, WIZARD_ENTITY_PAGE_SIZE);

    let mut rows = Vec::new();
    for candidate in visible {
        rows.push(vec![InlineKeyboardButton::callback(
            candidate_button_label(candidate),
            Payload::Admin(AdminPayload::RecordingRuleEditPickSensor {
                room: room_id,
                rule: rule.id,
                device: candidate.device_id,
            })
            .to_string(),
        )]);
    }
    add_entity_pagination_rows(&mut rows, page, total_pages, |target_page| {
        Payload::Admin(AdminPayload::RecordingRuleEditSensorPage {
            room: room_id,
            rule: rule.id,
            page: target_page,
        })
    });
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RecordingRuleEditSensors {
            room: room_id,
            rule: rule.id,
        }),
    )]);

    Ok(View {
        header: Some("➕ Сенсор правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Добавление сенсора\n\nПравило: {}\nВыберите датчик.\nСтраница {}/{} · датчиков: {}",
            rule.name,
            usize::from(page) + 1,
            total_pages.max(1),
            candidates.len()
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleEditSensorPage {
            room: room_id,
            rule: rule_id,
            page,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_edit_sensor_operators(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
    device_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };
    let Some(candidate) =
        crate::db::devices::get_recording_wizard_candidate(device_id, &ctx.config.db).await?
    else {
        let mut view = render_recording_rule_edit_sensor_page(ctx, room_id, rule_id, 0).await?;
        view.alert = Some("Датчик не найден".to_string());
        return Ok(view);
    };

    let mut rows = Vec::new();
    for operator in condition_operators_for_candidate(&candidate) {
        rows.push(vec![InlineKeyboardButton::callback(
            recording_rule_wizard::condition_operator_label(operator, ctx.lang),
            Payload::Admin(AdminPayload::RecordingRuleEditPickSensorOperator {
                room: room_id,
                rule: rule.id,
                device: device_id,
                operator,
            })
            .to_string(),
        )]);
    }
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RecordingRuleEditSensorPage {
            room: room_id,
            rule: rule.id,
            page: 0,
        }),
    )]);

    Ok(View {
        header: Some("➕ Сенсор правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Добавление сенсора\n\nПравило: {}\nДатчик: {}\nКомната: {}\nТип: {}\n\nВыберите условие.",
            rule.name,
            candidate.display_name,
            candidate.room_name,
            candidate_kind_label(&candidate)
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleEditPickSensor {
            room: room_id,
            rule: rule_id,
            device: device_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_edit_sensor_value_input(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
    device_id: i64,
    operator: ConditionOperator,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };
    let Some(candidate) =
        crate::db::devices::get_recording_wizard_candidate(device_id, &ctx.config.db).await?
    else {
        let mut view = render_recording_rule_edit_sensor_page(ctx, room_id, rule_id, 0).await?;
        view.alert = Some("Датчик не найден".to_string());
        return Ok(view);
    };

    let help = match operator {
        ConditionOperator::ChangedFromTo => "Введите два состояния в формате `from;to`.",
        ConditionOperator::Above | ConditionOperator::Below => "Введите числовое значение.",
        ConditionOperator::ChangedTo => "Введите новое состояние, например `on` или `off`.",
        ConditionOperator::Is | ConditionOperator::IsNot | ConditionOperator::Contains => {
            "Введите значение условия."
        }
    };

    Ok(View {
        header: Some("➕ Сенсор правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Добавление сенсора\n\nПравило: {}\nДатчик: {}\nУсловие: {}\n\n{}",
            rule.name,
            candidate.display_name,
            recording_rule_wizard::condition_operator_label(operator, ctx.lang),
            help
        ),
        kb: InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            t(ctx.lang, "common.cancel"),
            Payload::Admin(AdminPayload::RecordingRuleEditSensors {
                room: room_id,
                rule: rule.id,
            })
            .to_string(),
        )]]),
        payload: Payload::Admin(AdminPayload::RecordingRuleEditPickSensorOperator {
            room: room_id,
            rule: rule.id,
            device: device_id,
            operator,
        }),
        next_state: Some(State::EditRecordingRuleConditionValue {
            room_id,
            rule_id: rule.id,
            device_id,
            operator,
        }),
        ..Default::default()
    })
}

pub async fn render_edit_recording_rule_input(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
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
        kb: InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            t(ctx.lang, "common.cancel"),
            Payload::Admin(AdminPayload::RecordingRuleEditMenu {
                room: room_id,
                rule: rule_id,
            })
            .to_string(),
        )]]),
        payload: Payload::Admin(AdminPayload::PromptEditRecordingRule {
            room: room_id,
            rule: rule_id,
        }),
        next_state: Some(State::EditRecordingRule { room_id, rule_id }),
        ..Default::default()
    })
}

pub async fn render_edit_recording_rule_number_input(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
    field: RecordingRuleEditField,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };

    let current = recording_rule_field_value(&rule, field);
    let (min, max) = recording_rule_field_bounds(&ctx, field);
    let text = format!(
        "Изменение: {}\n\nПравило: {}\nТекущее значение: {}{}\nДиапазон: {}-{}{}\n\nВведите новое число.",
        field.title(),
        rule.name,
        current,
        field.unit(),
        min,
        max,
        field.unit(),
    );

    Ok(View {
        header: Some(format!("✏️ {}", field.title())),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            t(ctx.lang, "common.cancel"),
            Payload::Admin(AdminPayload::RecordingRuleEditMenu {
                room: room_id,
                rule: rule_id,
            })
            .to_string(),
        )]]),
        payload: Payload::Admin(AdminPayload::PromptEditRecordingRuleNumber {
            room: room_id,
            rule: rule_id,
            field,
        }),
        next_state: Some(State::EditRecordingRuleNumber {
            room_id,
            rule_id,
            field,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_active_time(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };

    let rows = vec![
        vec![InlineKeyboardButton::callback(
            "Всегда",
            Payload::Admin(AdminPayload::SetRecordingRuleActiveTimePreset {
                room: room_id,
                rule: rule_id,
                preset: RecordingRuleActiveTimePreset::Always,
            })
            .to_string(),
        )],
        vec![
            InlineKeyboardButton::callback(
                "Ночь 22:00-07:00",
                Payload::Admin(AdminPayload::SetRecordingRuleActiveTimePreset {
                    room: room_id,
                    rule: rule_id,
                    preset: RecordingRuleActiveTimePreset::Night,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                "День 08:00-20:00",
                Payload::Admin(AdminPayload::SetRecordingRuleActiveTimePreset {
                    room: room_id,
                    rule: rule_id,
                    preset: RecordingRuleActiveTimePreset::Day,
                })
                .to_string(),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            "Свое время",
            Payload::Admin(AdminPayload::PromptRecordingRuleActiveTimeCustom {
                room: room_id,
                rule: rule_id,
            })
            .to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "Дни недели",
            Payload::Admin(AdminPayload::RecordingRuleActiveDays {
                room: room_id,
                rule: rule_id,
            })
            .to_string(),
        )],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::RecordingRuleEditMenu {
                room: room_id,
                rule: rule_id,
            }),
        )],
    ];

    Ok(View {
        header: Some("🕒 Время активности".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Время активности\n\nПравило: {}\nСейчас: {}\nВремя сервера: {}\n\nВыберите, когда правило может запускать запись.",
            rule.name,
            format_rule_active_time(&rule),
            server_timezone_label()
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleEditActiveTime {
            room: room_id,
            rule: rule_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_active_time_custom_input(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };

    Ok(render_user_input(
        ctx,
        State::EditRecordingRuleActiveTime { room_id, rule_id },
        "Свое время",
        &format!(
            "Правило: {}\nТекущее значение: {}\n\nВведите время в формате HH:MM-HH:MM.\nНапример: 22:00-07:00",
            rule.name,
            format_rule_active_time(&rule)
        ),
        Payload::Admin(AdminPayload::PromptRecordingRuleActiveTimeCustom {
            room: room_id,
            rule: rule_id,
        }),
        Payload::Admin(AdminPayload::RecordingRuleEditActiveTime {
            room: room_id,
            rule: rule_id,
        }),
    ))
}

pub async fn render_recording_rule_active_days(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };
    let active_time = rule.active_time();
    if !active_time.enabled || !crate::db::camera_recording_rules::active_time_is_valid(active_time)
    {
        let mut view = render_recording_rule_active_time(ctx, room_id, rule_id).await?;
        view.alert = Some("Сначала выберите корректное окно времени.".to_string());
        return Ok(view);
    }

    let names = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
    let mask = active_time.days_mask;
    let mut rows = Vec::new();
    for chunk in [0_usize..3, 3..5, 5..7] {
        rows.push(
            chunk
                .map(|index| {
                    let checked = if mask & (1_i64 << index) != 0 {
                        "☑"
                    } else {
                        "☐"
                    };
                    InlineKeyboardButton::callback(
                        format!("{} {}", checked, names[index]),
                        Payload::Admin(AdminPayload::ToggleRecordingRuleActiveDay {
                            room: room_id,
                            rule: rule_id,
                            day: index as u8,
                        })
                        .to_string(),
                    )
                })
                .collect::<Vec<_>>(),
        );
    }
    rows.push(vec![InlineKeyboardButton::callback(
        "Готово",
        Payload::Admin(AdminPayload::RecordingRuleEditActiveTime {
            room: room_id,
            rule: rule_id,
        })
        .to_string(),
    )]);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RecordingRuleEditActiveTime {
            room: room_id,
            rule: rule_id,
        }),
    )]);

    Ok(View {
        header: Some("🗓 Дни активности".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Дни активности\n\nПравило: {}\nОкно: {}\nДни: {}\nВремя сервера: {}",
            rule.name,
            format_rule_active_time(&rule),
            format_active_days_mask(mask),
            server_timezone_label()
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::RecordingRuleActiveDays {
            room: room_id,
            rule: rule_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_test(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    let Some(rule) = recording_rule_for_room(&ctx, room_id, rule_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    };
    let conditions =
        crate::db::camera_recording_rules::list_conditions(rule.id, &ctx.config.db).await?;
    let display_context = recording_condition_display_context(&ctx).await?;
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
            let state_text = current
                .map(|state| {
                    format_recording_state(
                        &condition.entity_id,
                        state,
                        &display_context,
                        &ctx.config,
                    )
                })
                .unwrap_or_else(|| "не найдено".to_string());
            lines.push(format!(
                "🕓 {} · сейчас: {} · ждет переход",
                format_recording_condition_human(condition, &display_context, &ctx.config),
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
        let state_text = current
            .map(|state| {
                format_recording_state(&condition.entity_id, state, &display_context, &ctx.config)
            })
            .unwrap_or_else(|| "не найдено".to_string());
        lines.push(format!(
            "{} {} · сейчас: {}",
            icon,
            format_recording_condition_human(condition, &display_context, &ctx.config),
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
            condition_logic_label(rule.logic()),
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
        "Добавление правила записи\n\nТехнический режим: отправьте raw-блок правила.\n\nКамеры:\n{}\n\nФормат:\nНазвание\nID камеры\nЛогика: any или all\nУсловие: entity_id;operator;from;to;value\nМожно несколько строк условий\nTail seconds\nMax segment seconds\nCooldown seconds\nRetention days\n\nПример:\nЗамок Дверь: открытие или закрытие\n{}\nany\nbinary_sensor.zamok_contact;changed_from_to;off;on;\nbinary_sensor.zamok_contact;changed_from_to;on;off;\n60\n300\n0\n{}",
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
        kb: InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            t(ctx.lang, "common.cancel"),
            Payload::Admin(AdminPayload::RecordingRules { room: room_id }).to_string(),
        )]]),
        payload: Payload::Admin(AdminPayload::PromptAddRecordingRule { room: room_id }),
        next_state: Some(State::AddRecordingRule { room_id }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_start(ctx: RenderContext, room_id: i64) -> Result<View> {
    let room = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Room not found"))?;
    let cameras = crate::db::cameras::list_room_cameras(room_id, &ctx.config.db).await?;
    let cameras_empty = cameras.is_empty();

    let mut wizard = RecordingRuleWizard::new(room_id);
    if cameras.len() == 1 {
        wizard.camera_id = Some(cameras[0].id);
        store_wizard(&ctx, wizard)?;
        return render_recording_rule_wizard_entities(ctx, room_id, cameras[0].id).await;
    }
    store_wizard(&ctx, wizard)?;

    let mut rows = Vec::new();
    if cameras.is_empty() {
        rows.push(vec![InlineKeyboardButton::callback(
            "➕ Добавить камеру",
            Payload::Admin(AdminPayload::PromptAddCamera { room: room_id }).to_string(),
        )]);
    } else {
        for camera in cameras {
            rows.push(vec![InlineKeyboardButton::callback(
                format!("📹 {}", camera.name),
                Payload::Admin(AdminPayload::WizardPickCamera {
                    room: room_id,
                    camera: camera.id,
                })
                .to_string(),
            )]);
        }
    }

    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::RecordingRules { room: room_id }),
    )]);

    let text = if cameras_empty {
        format!(
            "Мастер правила записи\n\nКомната: {}\n\nВ этой комнате нет активных камер.",
            room.display_name()
        )
    } else {
        format!(
            "Мастер правила записи\n\nКомната: {}\n\nВыберите камеру.",
            room.display_name()
        )
    };

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::StartRecordingRuleWizard { room: room_id }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_entities(
    ctx: RenderContext,
    room_id: i64,
    camera_id: i64,
) -> Result<View> {
    let Some(_camera) = wizard_camera(&ctx, room_id, camera_id).await? else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Камера не найдена или не принадлежит комнате".to_string());
        return Ok(view);
    };

    update_wizard(&ctx, |wizard| {
        wizard.room_id = room_id;
        wizard.camera_id = Some(camera_id);
        wizard.source_device_id = None;
        wizard.source_mode = None;
        wizard.source_value = None;
        wizard.extra_conditions.clear();
        wizard.pending_condition = None;
    })?;

    render_recording_rule_wizard_source_page(ctx, 0).await
}

pub async fn render_recording_rule_wizard_source_page(
    ctx: RenderContext,
    page: u16,
) -> Result<View> {
    let Some(wizard) = wizard_state(&ctx) else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };
    let Some(camera_id) = wizard.camera_id else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не выбрана".to_string());
        return Ok(view);
    };
    let Some(camera) = wizard_camera(&ctx, wizard.room_id, camera_id).await? else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не найдена или не принадлежит комнате".to_string());
        return Ok(view);
    };

    let candidates =
        crate::db::devices::list_recording_wizard_candidates(wizard.room_id, &ctx.config.db)
            .await?;
    let page = bounded_page(page, candidates.len(), WIZARD_ENTITY_PAGE_SIZE);
    let total_pages = total_pages(candidates.len(), WIZARD_ENTITY_PAGE_SIZE);
    let visible = page_slice(&candidates, page, WIZARD_ENTITY_PAGE_SIZE);

    let mut rows = Vec::new();
    for candidate in visible {
        rows.push(vec![InlineKeyboardButton::callback(
            candidate_button_label(candidate),
            Payload::Admin(AdminPayload::WizardPickEntity {
                room: wizard.room_id,
                camera: camera.id,
                device: candidate.device_id,
            })
            .to_string(),
        )]);
    }

    add_entity_pagination_rows(&mut rows, page, total_pages, |target_page| {
        Payload::Admin(AdminPayload::WizardSourcePage { page: target_page })
    });

    rows.push(vec![InlineKeyboardButton::callback(
        "⌨️ Расширенно",
        Payload::Admin(AdminPayload::PromptAddRecordingRule {
            room: wizard.room_id,
        })
        .to_string(),
    )]);
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::StartRecordingRuleWizard {
            room: wizard.room_id,
        }),
    )]);

    let text = if candidates.is_empty() {
        format!(
            "Мастер правила записи\n\nКамера: {}\n\nВ доступных комнатах не найдено подходящих датчиков.",
            camera.name
        )
    } else {
        format!(
            "Мастер правила записи\n\nКамера: {}\n\nВыберите источник события.\nСтраница {}/{} · датчиков: {}",
            camera.name,
            usize::from(page) + 1,
            total_pages.max(1),
            candidates.len()
        )
    };

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardSourcePage { page }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_modes(
    ctx: RenderContext,
    room_id: i64,
    camera_id: i64,
    device_id: i64,
) -> Result<View> {
    let Some((camera, candidate)) =
        wizard_camera_and_candidate(&ctx, room_id, camera_id, device_id).await?
    else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Камера или датчик не найдены".to_string());
        return Ok(view);
    };

    update_wizard(&ctx, |wizard| {
        wizard.room_id = room_id;
        wizard.camera_id = Some(camera_id);
        wizard.reset_source(device_id);
    })?;

    let modes = source_modes_for_candidate(&candidate);
    let mut rows = modes
        .into_iter()
        .map(|mode| {
            vec![InlineKeyboardButton::callback(
                mode.button_label(ctx.lang),
                Payload::Admin(AdminPayload::WizardPickMode {
                    room: room_id,
                    camera: camera_id,
                    device: device_id,
                    mode,
                })
                .to_string(),
            )]
        })
        .collect::<Vec<_>>();
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardPickCamera {
            room: room_id,
            camera: camera_id,
        }),
    )]);

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Мастер правила записи\n\nКамера: {}\nИсточник: {}\nКомната датчика: {}\nТип: {}\n\nКогда запускать проверку?",
            camera.name,
            candidate.display_name,
            candidate.room_name,
            candidate_kind_label(&candidate)
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardPickEntity {
            room: room_id,
            camera: camera_id,
            device: device_id,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_tail(
    ctx: RenderContext,
    room_id: i64,
    camera_id: i64,
    device_id: i64,
    mode: WizardTriggerMode,
) -> Result<View> {
    if mode.needs_value() {
        return render_recording_rule_wizard_source_value_input(ctx, mode);
    }

    update_wizard(&ctx, |wizard| {
        wizard.room_id = room_id;
        wizard.camera_id = Some(camera_id);
        wizard.source_device_id = Some(device_id);
        wizard.set_source_mode(mode, None);
    })?;

    render_recording_rule_wizard_conditions(ctx).await
}

pub fn render_recording_rule_wizard_source_value_input(
    ctx: RenderContext,
    mode: WizardTriggerMode,
) -> Result<View> {
    let wizard = match wizard_state(&ctx) {
        Some(wizard) => wizard,
        None => {
            return Ok(View {
                header: Some("➕ Мастер правила".to_string()),
                notifications: ctx.notifications,
                text: "Сессия мастера устарела. Откройте мастер заново.".to_string(),
                payload: Payload::Admin(AdminPayload::CameraRooms),
                ..Default::default()
            });
        }
    };

    Ok(render_wizard_user_input(
        ctx,
        State::RecordingRuleWizardSourceValue { mode },
        "Порог условия",
        "Введите числовое значение порога.",
        Payload::Admin(AdminPayload::WizardPickMode {
            room: wizard.room_id,
            camera: wizard.camera_id.unwrap_or_default(),
            device: wizard.source_device_id.unwrap_or_default(),
            mode,
        }),
        Payload::Admin(AdminPayload::WizardPickEntity {
            room: wizard.room_id,
            camera: wizard.camera_id.unwrap_or_default(),
            device: wizard.source_device_id.unwrap_or_default(),
        }),
    ))
}

pub async fn render_recording_rule_wizard_conditions(ctx: RenderContext) -> Result<View> {
    let Some(wizard) = wizard_state(&ctx) else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };
    let Some(camera_id) = wizard.camera_id else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не выбрана".to_string());
        return Ok(view);
    };
    let Some(source_device_id) = wizard.source_device_id else {
        return render_recording_rule_wizard_entities(ctx, wizard.room_id, camera_id).await;
    };
    let Some(source_mode) = wizard.source_mode else {
        return render_recording_rule_wizard_modes(
            ctx,
            wizard.room_id,
            camera_id,
            source_device_id,
        )
        .await;
    };
    let Some(camera) = wizard_camera(&ctx, wizard.room_id, camera_id).await? else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не найдена".to_string());
        return Ok(view);
    };
    let Some(source) =
        crate::db::devices::get_recording_wizard_candidate(source_device_id, &ctx.config.db)
            .await?
    else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Источник не найден".to_string());
        return Ok(view);
    };

    let conditions = match recording_rule_wizard::build_final_conditions(
        &source.entity_id,
        source_mode,
        wizard.source_value.as_deref(),
        &wizard.extra_conditions,
    ) {
        Ok(conditions) => conditions,
        Err(error) => {
            let mut view = render_recording_rule_wizard_modes(
                ctx,
                wizard.room_id,
                camera_id,
                source_device_id,
            )
            .await?;
            view.alert = Some(error);
            return Ok(view);
        }
    };

    let display_context = recording_condition_display_context(&ctx).await?;
    let condition_text =
        format_wizard_conditions_summary(&conditions, &display_context, &ctx.config);

    let logic_label = format!("Логика: {}", condition_logic_label(wizard.condition_logic));

    let mut rows = Vec::new();
    rows.push(vec![InlineKeyboardButton::callback(
        "➕ Добавить условие",
        Payload::Admin(AdminPayload::WizardAddCondition).to_string(),
    )]);
    rows.push(vec![InlineKeyboardButton::callback(
        logic_label.clone(),
        Payload::Admin(AdminPayload::WizardToggleLogic).to_string(),
    )]);
    for (index, condition) in wizard.extra_conditions.iter().enumerate() {
        rows.push(vec![InlineKeyboardButton::callback(
            shorten_button_label(&format!(
                "🗑 Условие {} · {}",
                index + 1,
                format_wizard_condition_human(condition, &display_context, &ctx.config)
            )),
            Payload::Admin(AdminPayload::WizardRemoveCondition {
                index: u8::try_from(index).unwrap_or(u8::MAX),
            })
            .to_string(),
        )]);
    }
    rows.push(vec![InlineKeyboardButton::callback(
        "Далее",
        Payload::Admin(AdminPayload::WizardNextTail).to_string(),
    )]);
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardPickEntity {
            room: wizard.room_id,
            camera: camera_id,
            device: source_device_id,
        }),
    )]);

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Мастер правила записи\n\nКамера: {}\nИсточник: {} · {}\nСобытие: {}\n{}\n\nУсловия:\n{}",
            camera.name,
            source.display_name,
            source.room_name,
            source_mode.summary_label(ctx.lang),
            logic_label,
            condition_text
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardConditions),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_condition_entities(ctx: RenderContext) -> Result<View> {
    render_recording_rule_wizard_condition_entities_page(ctx, 0).await
}

pub async fn render_recording_rule_wizard_condition_entities_page(
    ctx: RenderContext,
    page: u16,
) -> Result<View> {
    let Some(wizard) = wizard_state(&ctx) else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };
    let candidates =
        crate::db::devices::list_recording_wizard_candidates(wizard.room_id, &ctx.config.db)
            .await?;
    let page = bounded_page(page, candidates.len(), WIZARD_ENTITY_PAGE_SIZE);
    let total_pages = total_pages(candidates.len(), WIZARD_ENTITY_PAGE_SIZE);
    let visible = page_slice(&candidates, page, WIZARD_ENTITY_PAGE_SIZE);

    let mut rows = Vec::new();
    for candidate in visible {
        rows.push(vec![InlineKeyboardButton::callback(
            candidate_button_label(candidate),
            Payload::Admin(AdminPayload::WizardPickConditionEntity {
                device: candidate.device_id,
            })
            .to_string(),
        )]);
    }
    add_entity_pagination_rows(&mut rows, page, total_pages, |target_page| {
        Payload::Admin(AdminPayload::WizardConditionEntityPage { page: target_page })
    });
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardConditions),
    )]);

    Ok(View {
        header: Some("➕ Условие правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Выберите датчик для дополнительного условия.\nСтраница {}/{} · датчиков: {}",
            usize::from(page) + 1,
            total_pages.max(1),
            candidates.len()
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardConditionEntityPage { page }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_condition_operators(
    ctx: RenderContext,
    device_id: i64,
) -> Result<View> {
    let Some(candidate) =
        crate::db::devices::get_recording_wizard_candidate(device_id, &ctx.config.db).await?
    else {
        let mut view = render_recording_rule_wizard_conditions(ctx).await?;
        view.alert = Some("Датчик не найден".to_string());
        return Ok(view);
    };

    if !update_wizard(&ctx, |wizard| {
        wizard.pending_condition = Some(WizardPendingCondition {
            device_id,
            operator: None,
        });
    })? {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    }

    let mut rows = Vec::new();
    for operator in condition_operators_for_candidate(&candidate) {
        rows.push(vec![InlineKeyboardButton::callback(
            recording_rule_wizard::condition_operator_label(operator, ctx.lang),
            Payload::Admin(AdminPayload::WizardPickConditionOperator { operator }).to_string(),
        )]);
    }
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardAddCondition),
    )]);

    Ok(View {
        header: Some("➕ Условие правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Датчик: {}\nКомната: {}\nТип: {}\n\nВыберите оператор.",
            candidate.display_name,
            candidate.room_name,
            candidate_kind_label(&candidate)
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardPickConditionEntity { device: device_id }),
        ..Default::default()
    })
}

pub fn render_recording_rule_wizard_condition_value_input(
    ctx: RenderContext,
    operator: ConditionOperator,
) -> Result<View> {
    let Some(wizard) = wizard_state(&ctx) else {
        return Ok(wizard_expired_view(ctx));
    };
    if wizard.pending_condition.is_none() {
        return Ok(wizard_pending_condition_missing_view(ctx));
    }

    if !update_wizard(&ctx, |wizard| {
        if let Some(pending) = wizard.pending_condition.as_mut() {
            pending.operator = Some(operator);
        }
    })? {
        return Ok(wizard_expired_view(ctx));
    }

    let help = match operator {
        ConditionOperator::ChangedFromTo => "Введите два состояния в формате `from;to`.",
        ConditionOperator::Above | ConditionOperator::Below => "Введите числовое значение.",
        ConditionOperator::ChangedTo => "Введите новое состояние, например `on` или `off`.",
        ConditionOperator::Is | ConditionOperator::IsNot | ConditionOperator::Contains => {
            "Введите значение условия."
        }
    };

    Ok(render_wizard_user_input(
        ctx,
        State::RecordingRuleWizardConditionValue,
        "Значение условия",
        help,
        Payload::Admin(AdminPayload::WizardPickConditionOperator { operator }),
        Payload::Admin(AdminPayload::WizardAddCondition),
    ))
}

pub async fn render_recording_rule_wizard_tail_from_state(ctx: RenderContext) -> Result<View> {
    let Some(wizard) = wizard_state(&ctx) else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };
    let Some(camera_id) = wizard.camera_id else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не выбрана".to_string());
        return Ok(view);
    };
    let Some(camera) = wizard_camera(&ctx, wizard.room_id, camera_id).await? else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не найдена".to_string());
        return Ok(view);
    };

    let max_tail = ctx.config.camera_recording_max_tail_seconds;
    let mut presets = [30, 60, 120, 300]
        .into_iter()
        .filter(|seconds| (5..=max_tail).contains(seconds))
        .collect::<Vec<_>>();
    if presets.is_empty() {
        presets.push(max_tail.max(5));
    }

    let mut rows = presets
        .chunks(2)
        .map(|chunk| {
            chunk
                .iter()
                .map(|tail| {
                    InlineKeyboardButton::callback(
                        format!("{}с", tail),
                        Payload::Admin(AdminPayload::WizardPickWizardTail { tail: *tail })
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardConditions),
    )]);

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Мастер правила записи\n\nКамера: {}\n\nСколько писать после события?",
            camera.name
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardNextTail),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_retention_from_state(
    ctx: RenderContext,
    tail: u32,
) -> Result<View> {
    update_wizard(&ctx, |wizard| {
        wizard.tail_seconds = Some(tail);
    })?;

    let Some(wizard) = wizard_state(&ctx) else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };
    let Some(camera_id) = wizard.camera_id else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не выбрана".to_string());
        return Ok(view);
    };
    let Some(camera) = wizard_camera(&ctx, wizard.room_id, camera_id).await? else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не найдена".to_string());
        return Ok(view);
    };

    let mut rows = [7, 15, 30, 90]
        .chunks(2)
        .map(|chunk| {
            chunk
                .iter()
                .map(|retention| {
                    InlineKeyboardButton::callback(
                        format!("{}д", retention),
                        Payload::Admin(AdminPayload::WizardPickWizardRetention {
                            retention: *retention,
                        })
                        .to_string(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardNextTail),
    )]);

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Мастер правила записи\n\nКамера: {}\nЗапись: {}с\n\nСколько хранить записи?",
            camera.name, tail
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardPickWizardTail { tail }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_retention(
    ctx: RenderContext,
    room_id: i64,
    camera_id: i64,
    device_id: i64,
    mode: WizardTriggerMode,
    tail: u32,
) -> Result<View> {
    let Some((camera, candidate)) =
        wizard_camera_and_candidate(&ctx, room_id, camera_id, device_id).await?
    else {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Камера или датчик не найдены".to_string());
        return Ok(view);
    };

    let mut rows = [7, 15, 30, 90]
        .chunks(2)
        .map(|chunk| {
            chunk
                .iter()
                .map(|retention| {
                    InlineKeyboardButton::callback(
                        format!("{}д", retention),
                        Payload::Admin(AdminPayload::WizardPickRetention {
                            room: room_id,
                            camera: camera_id,
                            device: device_id,
                            mode,
                            tail,
                            retention: *retention,
                        })
                        .to_string(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardPickMode {
            room: room_id,
            camera: camera_id,
            device: device_id,
            mode,
        }),
    )]);

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Мастер правила записи\n\nКамера: {}\nДатчик: {}\nТип: {}\nСобытие: {}\nЗапись: {}с\n\nСколько хранить записи?",
            camera.name,
            candidate.display_name,
            candidate_kind_label(&candidate),
            mode.summary_label(ctx.lang),
            tail
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardPickTail {
            room: room_id,
            camera: camera_id,
            device: device_id,
            mode,
            tail,
        }),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_active_time_from_state(
    ctx: RenderContext,
    retention: Option<u32>,
) -> Result<View> {
    if let Some(retention) = retention {
        update_wizard(&ctx, |wizard| {
            wizard.retention_days = Some(retention);
        })?;
    }

    let Some(wizard) = wizard_state(&ctx) else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };
    let Some(camera_id) = wizard.camera_id else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не выбрана".to_string());
        return Ok(view);
    };
    let Some(camera) = wizard_camera(&ctx, wizard.room_id, camera_id).await? else {
        let mut view = render_recording_rules(ctx, wizard.room_id).await?;
        view.alert = Some("Камера не найдена".to_string());
        return Ok(view);
    };

    let rows = vec![
        vec![InlineKeyboardButton::callback(
            "Всегда",
            Payload::Admin(AdminPayload::WizardPickActiveTimePreset {
                preset: RecordingRuleActiveTimePreset::Always,
            })
            .to_string(),
        )],
        vec![
            InlineKeyboardButton::callback(
                "Ночь 22:00-07:00",
                Payload::Admin(AdminPayload::WizardPickActiveTimePreset {
                    preset: RecordingRuleActiveTimePreset::Night,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                "День 08:00-20:00",
                Payload::Admin(AdminPayload::WizardPickActiveTimePreset {
                    preset: RecordingRuleActiveTimePreset::Day,
                })
                .to_string(),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            "Свое время",
            Payload::Admin(AdminPayload::WizardPromptActiveTimeCustom).to_string(),
        )],
        vec![wizard_cancel_button()],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::WizardPickWizardTail {
                tail: wizard.tail_seconds.unwrap_or(60),
            }),
        )],
    ];

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Когда правило активно?\n\nКамера: {}\nХранить: {}д\nСейчас: {}\nВремя сервера: {}",
            camera.name,
            wizard.retention_days.unwrap_or(30),
            format_wizard_active_time(wizard.active_time),
            server_timezone_label()
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardActiveTime),
        ..Default::default()
    })
}

pub fn render_recording_rule_wizard_active_time_custom_input(ctx: RenderContext) -> Result<View> {
    Ok(render_wizard_user_input(
        ctx,
        State::RecordingRuleWizardActiveTimeValue,
        "Время активности",
        "Введите время в формате HH:MM-HH:MM.\nНапример: 22:00-07:00",
        Payload::Admin(AdminPayload::WizardPromptActiveTimeCustom),
        Payload::Admin(AdminPayload::WizardActiveTime),
    ))
}

pub async fn render_recording_rule_wizard_groups_from_state(ctx: RenderContext) -> Result<View> {
    let Some(wizard) = wizard_state(&ctx) else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };

    let groups = crate::db::camera_recording_rule_groups::list_groups(&ctx.config.db).await?;
    let mut rows = vec![vec![
        InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.create_custom"),
            Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroupForWizard).to_string(),
        ),
        InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.create_defaults"),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForWizard).to_string(),
        ),
    ]];
    if groups.is_empty() {
        rows.push(vec![InlineKeyboardButton::callback(
            "Готово",
            Payload::Admin(AdminPayload::WizardConfirmGroups).to_string(),
        )]);
        rows.push(vec![wizard_cancel_button()]);
        rows.push(vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::WizardConfirmGroups),
        )]);

        return Ok(View {
            header: Some("➕ Мастер правила".to_string()),
            notifications: ctx.notifications,
            text: "Группы правила\n\nГруппы еще не созданы. Можно создать базовые группы или продолжить без групп."
                .to_string(),
            kb: InlineKeyboardMarkup::new(rows),
            payload: Payload::Admin(AdminPayload::WizardGroups),
            ..Default::default()
        });
    }

    for group in groups {
        let checked = if wizard.group_ids.contains(&group.id) {
            "☑"
        } else {
            "☐"
        };
        let state = if group.is_enabled() { "" } else { " ⏸" };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} {}{}", checked, group.name, state),
            Payload::Admin(AdminPayload::WizardToggleGroup { group: group.id }).to_string(),
        )]);
    }
    rows.push(vec![InlineKeyboardButton::callback(
        "Готово",
        Payload::Admin(AdminPayload::WizardConfirmGroups).to_string(),
    )]);
    push_wizard_cancel(&mut rows);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::WizardConfirmGroups),
    )]);

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: "Группы правила\n\nОтметьте нужные группы галочкой. Можно ничего не выбирать."
            .to_string(),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardGroups),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_confirm(ctx: RenderContext) -> Result<View> {
    let Some((wizard, camera, source, source_mode, conditions)) =
        wizard_summary_parts(&ctx).await?
    else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера неполная. Откройте мастер заново.".to_string());
        return Ok(view);
    };

    let group_names = wizard_group_names(&ctx, &wizard.group_ids).await?;
    let display_context = recording_condition_display_context(&ctx).await?;
    let condition_text =
        format_wizard_conditions_summary(&conditions, &display_context, &ctx.config);
    let logic_label = condition_logic_label(wizard.condition_logic);
    let cooldown_s = recording_rule_wizard::default_cooldown_s(source_mode);
    let group_button_label = if group_names.is_empty() {
        "👥 Группы: не выбраны".to_string()
    } else {
        format!("👥 Группы: {}", group_names.len())
    };

    let rows = vec![
        vec![InlineKeyboardButton::callback(
            "✅ Создать",
            Payload::Admin(AdminPayload::WizardCreateCurrentRule).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            group_button_label,
            Payload::Admin(AdminPayload::WizardGroups).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!(
                "🕒 Активно: {}",
                format_wizard_active_time(wizard.active_time)
            ),
            Payload::Admin(AdminPayload::WizardActiveTime).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            "✏️ Расширенно",
            Payload::Admin(AdminPayload::WizardAdvancedCurrentText).to_string(),
        )],
        vec![wizard_cancel_button()],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            Payload::Admin(AdminPayload::WizardPickWizardRetention {
                retention: wizard.retention_days.unwrap_or(30),
            }),
        )],
    ];

    Ok(View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Создать правило записи?\n\nКамера: {}\nИсточник: {} · {}\nСобытие: {}\nЛогика: {}\nАктивно: {}\nПисать после события: {}с\nДлина файла: {}с\nПауза после записи: {}с\nХранить: {}д\nГруппы: {}\n\nБудут созданы условия:\n{}",
            camera.name,
            source.display_name,
            source.room_name,
            source_mode.summary_label(ctx.lang),
            logic_label,
            format_wizard_active_time(wizard.active_time),
            wizard.tail_seconds.unwrap_or(60),
            ctx.config.camera_recording_max_segment_seconds,
            cooldown_s,
            wizard.retention_days.unwrap_or(30),
            if group_names.is_empty() { "нет".to_string() } else { group_names.join(", ") },
            condition_text
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::WizardConfirmGroups),
        ..Default::default()
    })
}

pub async fn render_recording_rule_wizard_advanced(ctx: RenderContext) -> Result<View> {
    let Some((wizard, camera, source, source_mode, conditions)) =
        wizard_summary_parts(&ctx).await?
    else {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера неполная. Откройте мастер заново.".to_string());
        return Ok(view);
    };

    let name = recording_rule_wizard::build_rule_name(
        ctx.lang,
        &source.display_name,
        source_mode,
        wizard.source_value.as_deref(),
    );
    let text_block = recording_rule_wizard::build_advanced_rule_text_with_logic(
        &name,
        camera.id,
        wizard.condition_logic,
        &conditions,
        wizard.tail_seconds.unwrap_or(60),
        ctx.config.camera_recording_max_segment_seconds,
        recording_rule_wizard::default_cooldown_s(source_mode),
        wizard.retention_days.unwrap_or(30),
    );

    Ok(View {
        header: Some("⌨️ Расширенное правило".to_string()),
        notifications: ctx.notifications,
        text: format!(
            "Технический режим: скопируйте raw-блок ниже, измените если нужно и отправьте сообщением:\n\n{}",
            text_block
        ),
        kb: InlineKeyboardMarkup::new(vec![
            vec![wizard_cancel_button()],
            vec![crate::bot::screens::common::back_button_lang(
                ctx.lang,
                Payload::Admin(AdminPayload::WizardConfirmGroups),
            )],
        ]),
        payload: Payload::Admin(AdminPayload::WizardAdvancedCurrentText),
        next_state: Some(State::AddRecordingRule {
            room_id: wizard.room_id,
        }),
        ..Default::default()
    })
}

fn store_wizard(ctx: &RenderContext, wizard: RecordingRuleWizard) -> Result<()> {
    if ctx.config.sessions.get(&ctx.user_id).is_none() {
        let now = Utc::now();
        let context = Payload::Admin(AdminPayload::StartRecordingRuleWizard {
            room: wizard.room_id,
        })
        .to_string();
        ctx.config.sessions.insert(
            ctx.user_id,
            UserSession {
                last_menu_id: 0,
                current_context: context,
                ui_message_mode: UiMessageMode::Photo,
                header_entities: HashSet::new(),
                recording_rule_wizard: None,
                last_ui_refresh_at: None,
                ui_refresh_blocked_until: None,
                last_seen_at: now,
            },
        );
    }

    let Some(mut session) = ctx.config.sessions.get_mut(&ctx.user_id) else {
        return Err(anyhow::anyhow!("User session not found"));
    };
    session.recording_rule_wizard = Some(wizard);
    Ok(())
}

fn update_wizard<F>(ctx: &RenderContext, update: F) -> Result<bool>
where
    F: FnOnce(&mut RecordingRuleWizard),
{
    let mut wizard = wizard_state(ctx).unwrap_or_else(|| RecordingRuleWizard::new(0));
    update(&mut wizard);
    if wizard.room_id == 0 {
        log::warn!(
            "Recording rule wizard update ignored for user {}: session is missing",
            ctx.user_id
        );
        return Ok(false);
    }
    store_wizard(ctx, wizard).map(|_| true)
}

fn wizard_state(ctx: &RenderContext) -> Option<RecordingRuleWizard> {
    ctx.config
        .sessions
        .get(&ctx.user_id)
        .and_then(|session| session.recording_rule_wizard.clone())
}

fn wizard_expired_view(ctx: RenderContext) -> View {
    View {
        header: Some("➕ Мастер правила".to_string()),
        notifications: ctx.notifications,
        text: "Сессия мастера устарела. Откройте мастер заново.".to_string(),
        payload: Payload::Admin(AdminPayload::CameraRooms),
        ..Default::default()
    }
}

fn wizard_pending_condition_missing_view(ctx: RenderContext) -> View {
    View {
        header: Some("➕ Условие правила".to_string()),
        notifications: ctx.notifications,
        text: "Условие уже не выбрано. Вернитесь к списку условий и добавьте его заново."
            .to_string(),
        kb: InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            "К условиям",
            Payload::Admin(AdminPayload::WizardConditions).to_string(),
        )]]),
        payload: Payload::Admin(AdminPayload::WizardConditions),
        ..Default::default()
    }
}

async fn wizard_summary_parts(
    ctx: &RenderContext,
) -> Result<
    Option<(
        RecordingRuleWizard,
        crate::db::cameras::Camera,
        crate::db::devices::RecordingTriggerCandidate,
        WizardTriggerMode,
        Vec<recording_rule_wizard::WizardCondition>,
    )>,
> {
    let Some(wizard) = wizard_state(ctx) else {
        return Ok(None);
    };
    let Some(camera_id) = wizard.camera_id else {
        return Ok(None);
    };
    let Some(source_device_id) = wizard.source_device_id else {
        return Ok(None);
    };
    let Some(source_mode) = wizard.source_mode else {
        return Ok(None);
    };
    let Some(camera) = wizard_camera(ctx, wizard.room_id, camera_id).await? else {
        return Ok(None);
    };
    let Some(source) =
        crate::db::devices::get_recording_wizard_candidate(source_device_id, &ctx.config.db)
            .await?
    else {
        return Ok(None);
    };
    let conditions = recording_rule_wizard::build_final_conditions(
        &source.entity_id,
        source_mode,
        wizard.source_value.as_deref(),
        &wizard.extra_conditions,
    )
    .map_err(anyhow::Error::msg)?;

    Ok(Some((wizard, camera, source, source_mode, conditions)))
}

async fn wizard_group_names(ctx: &RenderContext, group_ids: &[i64]) -> Result<Vec<String>> {
    if group_ids.is_empty() {
        return Ok(Vec::new());
    }

    let groups = crate::db::camera_recording_rule_groups::list_groups(&ctx.config.db).await?;
    Ok(groups
        .into_iter()
        .filter(|group| group_ids.contains(&group.id))
        .map(|group| group.name)
        .collect())
}

fn selected_group_names_text(groups: &[RecordingRuleGroup], group_ids: &[i64]) -> String {
    let names = groups
        .iter()
        .filter(|group| group_ids.contains(&group.id))
        .map(|group| {
            if group.is_enabled() {
                group.name.clone()
            } else {
                format!("{} ⏸", group.name)
            }
        })
        .collect::<Vec<_>>();

    if names.is_empty() {
        "не выбраны".to_string()
    } else {
        names.join(", ")
    }
}

fn recording_rule_pause_reason(
    rule: &crate::db::camera_recording_rules::RecordingRule,
    groups: &[RecordingRuleGroup],
    group_ids: &[i64],
) -> Option<String> {
    let mut reasons = Vec::new();
    if !rule.is_enabled() {
        reasons.push("правило выключено".to_string());
    }

    let disabled_groups = groups
        .iter()
        .filter(|group| group_ids.contains(&group.id) && !group.is_enabled())
        .map(|group| group.name.clone())
        .collect::<Vec<_>>();

    match disabled_groups.as_slice() {
        [] => {}
        [name] => reasons.push(format!("выключенная группа: {}", name)),
        names => reasons.push(format!("выключенные группы: {}", names.join(", "))),
    }

    let active_time = rule.active_time();
    if active_time.enabled {
        if !crate::db::camera_recording_rules::active_time_is_valid(active_time) {
            reasons.push("ошибка настройки времени активности".to_string());
        } else if !crate::db::camera_recording_rules::active_time_matches_now(rule) {
            reasons.push("сейчас правило на паузе по времени".to_string());
        }
    }

    if reasons.is_empty() {
        None
    } else {
        Some(reasons.join("; "))
    }
}

fn format_rule_active_time(rule: &crate::db::camera_recording_rules::RecordingRule) -> String {
    format_wizard_active_time(rule.active_time())
}

fn format_wizard_active_time(
    active_time: crate::db::camera_recording_rules::RecordingRuleActiveTime,
) -> String {
    if !active_time.enabled {
        return "всегда".to_string();
    }
    if !crate::db::camera_recording_rules::active_time_is_valid(active_time) {
        return "ошибка настройки времени".to_string();
    }

    format!(
        "{}-{} · {} · {}",
        crate::db::camera_recording_rules::format_minute(active_time.from_minute),
        crate::db::camera_recording_rules::format_minute(active_time.to_minute),
        format_active_days_mask(active_time.days_mask),
        server_timezone_label()
    )
}

fn format_active_days_mask(days_mask: i64) -> String {
    match days_mask {
        crate::db::camera_recording_rules::ACTIVE_TIME_ALL_DAYS_MASK => "ежедневно".to_string(),
        0b001_1111 => "Пн-Пт".to_string(),
        0b110_0000 => "Сб-Вс".to_string(),
        _ => {
            let names = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
            names
                .iter()
                .enumerate()
                .filter(|(index, _)| days_mask & (1_i64 << index) != 0)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

fn server_timezone_label() -> String {
    std::env::var("TZ").unwrap_or_else(|_| Local::now().format("%:z").to_string())
}

async fn recording_rule_for_room(
    ctx: &RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<Option<crate::db::camera_recording_rules::RecordingRule>> {
    crate::db::camera_recording_rules::get_rule_for_room(rule_id, room_id, &ctx.config.db).await
}

fn push_wizard_cancel(rows: &mut Vec<Vec<InlineKeyboardButton>>) {
    rows.push(vec![wizard_cancel_button()]);
}

fn wizard_cancel_button() -> InlineKeyboardButton {
    InlineKeyboardButton::callback(
        "❌ Отменить создание",
        Payload::Admin(AdminPayload::WizardCancel).to_string(),
    )
}

fn render_wizard_user_input(
    ctx: RenderContext,
    next_state: State,
    title: &str,
    text: &str,
    current_payload: Payload,
    back_payload: Payload,
) -> View {
    let kb = InlineKeyboardMarkup::new(vec![
        vec![wizard_cancel_button()],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            back_payload,
        )],
    ]);

    View {
        header: Some(title.to_string()),
        notifications: ctx.notifications,
        text: text.to_string(),
        kb,
        payload: current_payload,
        next_state: Some(next_state),
        ..Default::default()
    }
}

const WIZARD_ENTITY_PAGE_SIZE: usize = 10;
const WIZARD_ENTITY_LABEL_LIMIT: usize = 46;

fn total_pages(total_items: usize, page_size: usize) -> usize {
    if total_items == 0 {
        1
    } else {
        total_items.div_ceil(page_size)
    }
}

fn bounded_page(page: u16, total_items: usize, page_size: usize) -> u16 {
    let max_page = total_pages(total_items, page_size).saturating_sub(1);
    usize::from(page).min(max_page) as u16
}

fn page_slice<T>(items: &[T], page: u16, page_size: usize) -> &[T] {
    let start = usize::from(page).saturating_mul(page_size);
    let end = (start + page_size).min(items.len());
    if start >= items.len() {
        &[]
    } else {
        &items[start..end]
    }
}

fn add_entity_pagination_rows<F>(
    rows: &mut Vec<Vec<InlineKeyboardButton>>,
    page: u16,
    total_pages: usize,
    payload_for_page: F,
) where
    F: Fn(u16) -> Payload,
{
    if total_pages <= 1 {
        return;
    }

    let mut nav = Vec::new();
    if page > 0 {
        nav.push(InlineKeyboardButton::callback(
            "←",
            payload_for_page(page - 1).to_string(),
        ));
    }
    nav.push(InlineKeyboardButton::callback(
        format!("{}/{}", usize::from(page) + 1, total_pages),
        payload_for_page(page).to_string(),
    ));
    if usize::from(page) + 1 < total_pages {
        nav.push(InlineKeyboardButton::callback(
            "→",
            payload_for_page(page + 1).to_string(),
        ));
    }
    rows.push(nav);
}

async fn wizard_camera(
    ctx: &RenderContext,
    room_id: i64,
    camera_id: i64,
) -> Result<Option<crate::db::cameras::Camera>> {
    let Some(camera) = crate::db::cameras::get_camera(camera_id, &ctx.config.db).await? else {
        return Ok(None);
    };
    if camera.room_id == Some(room_id) {
        Ok(Some(camera))
    } else {
        Ok(None)
    }
}

async fn wizard_camera_and_candidate(
    ctx: &RenderContext,
    room_id: i64,
    camera_id: i64,
    device_id: i64,
) -> Result<
    Option<(
        crate::db::cameras::Camera,
        crate::db::devices::RecordingTriggerCandidate,
    )>,
> {
    let Some(camera) = wizard_camera(ctx, room_id, camera_id).await? else {
        return Ok(None);
    };
    let Some(candidate) =
        crate::db::devices::get_recording_wizard_candidate(device_id, &ctx.config.db).await?
    else {
        return Ok(None);
    };
    Ok(Some((camera, candidate)))
}

fn candidate_button_label(candidate: &crate::db::devices::RecordingTriggerCandidate) -> String {
    shorten_button_label(&format!(
        "{} {} · {}",
        candidate_icon(candidate),
        candidate.display_name,
        candidate.room_name
    ))
}

fn shorten_button_label(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars().take(WIZARD_ENTITY_LABEL_LIMIT) {
        result.push(ch);
    }

    if value.chars().count() > WIZARD_ENTITY_LABEL_LIMIT {
        result.push('…');
    }

    result
}

fn candidate_kind_label(candidate: &crate::db::devices::RecordingTriggerCandidate) -> &'static str {
    match (
        candidate.device_domain.as_str(),
        candidate.device_class.as_str(),
    ) {
        ("binary_sensor", "door") => "дверь",
        ("binary_sensor", "window") => "окно",
        ("binary_sensor", "opening") => "открытие",
        ("binary_sensor", "garage_door") => "гаражная дверь",
        ("binary_sensor", "motion") => "движение",
        ("binary_sensor", "occupancy") => "присутствие",
        ("binary_sensor", "presence") => "присутствие",
        ("binary_sensor", _) => "binary_sensor",
        ("sensor", "temperature") => "температура",
        ("sensor", "humidity") => "влажность",
        ("sensor", "illuminance") => "освещенность",
        ("sensor", _) => "sensor",
        ("number", _) => "number",
        ("switch", _) => "switch",
        ("light", _) => "light",
        _ => "датчик",
    }
}

fn candidate_icon(candidate: &crate::db::devices::RecordingTriggerCandidate) -> &'static str {
    match (
        candidate.device_domain.as_str(),
        candidate.device_class.as_str(),
    ) {
        ("binary_sensor", "door" | "window" | "opening" | "garage_door") => "🚪",
        ("binary_sensor", "motion" | "occupancy" | "presence") => "🏃",
        ("binary_sensor", _) => "🔘",
        ("sensor", "temperature") => "🌡",
        ("sensor", _) => "📈",
        ("number", _) => "🔢",
        ("switch", _) => "⚡",
        ("light", _) => "💡",
        _ => "•",
    }
}

fn source_modes_for_candidate(
    candidate: &crate::db::devices::RecordingTriggerCandidate,
) -> Vec<WizardTriggerMode> {
    match (
        candidate.device_domain.as_str(),
        candidate.device_class.as_str(),
    ) {
        ("binary_sensor", "door" | "window" | "opening" | "garage_door") => vec![
            WizardTriggerMode::OpenAndClose,
            WizardTriggerMode::OpenOnly,
            WizardTriggerMode::CloseOnly,
            WizardTriggerMode::AnyChange,
        ],
        ("binary_sensor", "motion" | "occupancy" | "presence") => vec![
            WizardTriggerMode::Detected,
            WizardTriggerMode::Cleared,
            WizardTriggerMode::DetectedAndCleared,
            WizardTriggerMode::AnyChange,
        ],
        ("binary_sensor", _) | ("switch", _) | ("light", _) => vec![
            WizardTriggerMode::TurnedOn,
            WizardTriggerMode::TurnedOff,
            WizardTriggerMode::TurnedOnAndOff,
            WizardTriggerMode::AnyChange,
        ],
        ("sensor", _) | ("number", _) if is_numeric_candidate(candidate) => vec![
            WizardTriggerMode::AnyChange,
            WizardTriggerMode::NumericAbove,
            WizardTriggerMode::NumericBelow,
        ],
        ("sensor", _) | ("number", _) => vec![WizardTriggerMode::AnyChange],
        _ => vec![WizardTriggerMode::AnyChange],
    }
}

fn condition_operators_for_candidate(
    candidate: &crate::db::devices::RecordingTriggerCandidate,
) -> Vec<ConditionOperator> {
    let mut operators = vec![
        ConditionOperator::Is,
        ConditionOperator::IsNot,
        ConditionOperator::Contains,
        ConditionOperator::ChangedTo,
        ConditionOperator::ChangedFromTo,
    ];

    if is_numeric_candidate(candidate) {
        operators.insert(3, ConditionOperator::Above);
        operators.insert(4, ConditionOperator::Below);
    }

    operators
}

fn is_numeric_candidate(candidate: &crate::db::devices::RecordingTriggerCandidate) -> bool {
    if candidate.device_domain == "number" {
        return true;
    }

    matches!(
        candidate.device_class.as_str(),
        "temperature"
            | "humidity"
            | "illuminance"
            | "power"
            | "energy"
            | "battery"
            | "voltage"
            | "current"
            | "pressure"
    )
}

fn recording_rule_field_value(
    rule: &crate::db::camera_recording_rules::RecordingRule,
    field: RecordingRuleEditField,
) -> i64 {
    match field {
        RecordingRuleEditField::TailSeconds => rule.tail_seconds,
        RecordingRuleEditField::MaxSegmentSeconds => rule.max_segment_seconds,
        RecordingRuleEditField::CooldownSeconds => rule.cooldown_s,
        RecordingRuleEditField::RetentionDays => rule.retention_days,
    }
}

fn recording_rule_field_bounds(ctx: &RenderContext, field: RecordingRuleEditField) -> (u32, u32) {
    match field {
        RecordingRuleEditField::TailSeconds => (5, ctx.config.camera_recording_max_tail_seconds),
        RecordingRuleEditField::MaxSegmentSeconds => {
            (30, ctx.config.camera_recording_max_segment_seconds)
        }
        RecordingRuleEditField::CooldownSeconds => (0, 86_400),
        RecordingRuleEditField::RetentionDays => (1, 365),
    }
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

#[derive(Debug, Clone)]
struct RecordingConditionDisplay {
    name: String,
    domain: String,
    class: String,
}

fn format_recording_conditions_summary(
    conditions: &[crate::db::camera_recording_rules::RecordingRuleCondition],
    display_context: &HashMap<String, RecordingConditionDisplay>,
    config: &crate::models::AppConfig,
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
                format_recording_condition_human(condition, display_context, config)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_wizard_conditions_summary(
    conditions: &[recording_rule_wizard::WizardCondition],
    display_context: &HashMap<String, RecordingConditionDisplay>,
    config: &crate::models::AppConfig,
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
                format_wizard_condition_human(condition, display_context, config)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_wizard_condition_human(
    condition: &recording_rule_wizard::WizardCondition,
    display_context: &HashMap<String, RecordingConditionDisplay>,
    config: &crate::models::AppConfig,
) -> String {
    let condition = crate::db::camera_recording_rules::RecordingRuleCondition {
        id: 0,
        rule_id: 0,
        entity_id: condition.entity_id.clone(),
        operator: condition.operator.as_str().to_string(),
        from_state: condition.from_state.clone(),
        to_state: condition.to_state.clone(),
        value: condition.value.clone(),
    };
    format_recording_condition_human(&condition, display_context, config)
}

async fn recording_condition_display_context(
    ctx: &RenderContext,
) -> Result<HashMap<String, RecordingConditionDisplay>> {
    let rows = sqlx::query(
        r#"
        SELECT entity_id,
               COALESCE(NULLIF(TRIM(alias), ''), NULLIF(TRIM(ha_name), ''), entity_id) AS display_name,
               COALESCE(
                   NULLIF(TRIM(device_domain), ''),
                   CASE
                       WHEN instr(entity_id, '.') > 0
                       THEN substr(entity_id, 1, instr(entity_id, '.') - 1)
                       ELSE ''
                   END
               ) AS device_domain,
               COALESCE(NULLIF(TRIM(device_class), ''), '') AS device_class
        FROM devices
        WHERE COALESCE(archived, 0) = 0
        "#,
    )
    .fetch_all(&ctx.config.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("entity_id"),
                RecordingConditionDisplay {
                    name: row.get("display_name"),
                    domain: row.get("device_domain"),
                    class: row.get("device_class"),
                },
            )
        })
        .collect())
}

fn format_recording_condition_human(
    condition: &crate::db::camera_recording_rules::RecordingRuleCondition,
    display_context: &HashMap<String, RecordingConditionDisplay>,
    config: &crate::models::AppConfig,
) -> String {
    let display = display_context.get(&condition.entity_id);
    let name = display
        .map(|display| display.name.as_str())
        .unwrap_or(&condition.entity_id);
    let target = condition
        .value
        .as_deref()
        .or(condition.to_state.as_deref())
        .map(|state| format_recording_state(&condition.entity_id, state, display_context, config))
        .unwrap_or_else(|| "*".to_string());

    match condition.operator() {
        crate::db::camera_recording_rules::ConditionOperator::ChangedTo => {
            let to_state = condition
                .to_state
                .as_deref()
                .map(|state| {
                    format_recording_state(&condition.entity_id, state, display_context, config)
                })
                .unwrap_or_else(|| "*".to_string());
            format!("{}: меняется на {}", name, to_state)
        }
        crate::db::camera_recording_rules::ConditionOperator::ChangedFromTo => {
            let from_state = condition
                .from_state
                .as_deref()
                .map(|state| {
                    format_recording_state(&condition.entity_id, state, display_context, config)
                })
                .unwrap_or_else(|| "*".to_string());
            let to_state = condition
                .to_state
                .as_deref()
                .map(|state| {
                    format_recording_state(&condition.entity_id, state, display_context, config)
                })
                .unwrap_or_else(|| "*".to_string());
            format!("{}: {} -> {}", name, from_state, to_state)
        }
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

fn format_recording_state(
    entity_id: &str,
    state: &str,
    display_context: &HashMap<String, RecordingConditionDisplay>,
    config: &crate::models::AppConfig,
) -> String {
    let display = display_context.get(entity_id);
    let domain = display
        .map(|display| display.domain.as_str())
        .unwrap_or_else(|| {
            entity_id
                .split_once('.')
                .map(|(domain, _)| domain)
                .unwrap_or("")
        });
    let class = display.map(|display| display.class.as_str()).unwrap_or("");
    let alias = config.state_alias_for_display(entity_id, state, false);

    crate::core::presentation::StateFormatter::format_state_value_with_alias(
        domain,
        class,
        state,
        false,
        alias.as_deref(),
    )
}

fn condition_logic_label(logic: ConditionLogic) -> &'static str {
    match logic {
        ConditionLogic::All => "все условия сразу",
        ConditionLogic::Any => "любое из условий",
    }
}

fn condition_logic_short_label(logic: ConditionLogic) -> &'static str {
    match logic {
        ConditionLogic::All => "все сразу",
        ConditionLogic::Any => "любое из",
    }
}

fn condition_logic_explanation(logic: ConditionLogic) -> &'static str {
    match logic {
        ConditionLogic::All => "все условия сразу · должны выполниться все",
        ConditionLogic::Any => "любое из условий · достаточно одного совпадения",
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
        payload: Payload::Admin(AdminPayload::PromptAddCamera { room: room_id }),
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
    let back_payload = recording_rule_groups_back_payload(room_id);
    let current_payload = recording_rule_groups_payload(room_id);
    let ensure_payload = recording_rule_groups_defaults_payload(room_id);
    let create_payload = recording_rule_group_create_payload(room_id);
    let mut rows = Vec::new();
    rows.push(vec![
        InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.create_custom"),
            create_payload.to_string(),
        ),
        InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.create_defaults"),
            ensure_payload.to_string(),
        ),
    ]);
    for group in &groups {
        let icon = if group.is_enabled() { "✅" } else { "⏸" };
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {} · {} {}",
                icon,
                group.name,
                t(ctx.lang, "admin.rule_groups.rules_count"),
                group.rules_count
            ),
            recording_rule_group_detail_payload(room_id, group.id).to_string(),
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

pub async fn render_recording_rule_group_detail(ctx: RenderContext, group_id: i64) -> Result<View> {
    render_recording_rule_group_detail_with_back(ctx, None, group_id).await
}

pub async fn render_recording_rule_group_detail_for_room(
    ctx: RenderContext,
    room_id: i64,
    group_id: i64,
) -> Result<View> {
    render_recording_rule_group_detail_with_back(ctx, Some(room_id), group_id).await
}

async fn render_recording_rule_group_detail_with_back(
    ctx: RenderContext,
    room_id: Option<i64>,
    group_id: i64,
) -> Result<View> {
    let Some(group) =
        crate::db::camera_recording_rule_groups::get_group(group_id, &ctx.config.db).await?
    else {
        let lang = ctx.lang;
        let mut view = render_recording_rule_groups_with_back(ctx, room_id).await?;
        view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
        return Ok(view);
    };

    let status = if group.is_enabled() {
        t(ctx.lang, "admin.rule_groups.enabled")
    } else {
        t(ctx.lang, "admin.rule_groups.paused")
    };
    let toggle_label = if group.is_enabled() {
        t(ctx.lang, "admin.rule_groups.pause")
    } else {
        t(ctx.lang, "admin.rule_groups.enable")
    };

    let rows = vec![
        vec![InlineKeyboardButton::callback(
            toggle_label,
            recording_rule_group_toggle_detail_payload(room_id, group.id).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            format!(
                "🌐 Доступ: {}",
                if group.is_visible_to_all_users() {
                    "всем"
                } else {
                    "только администратору"
                }
            ),
            recording_rule_group_access_payload(room_id, group.id).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.rules"),
            recording_rule_group_rules_payload(
                room_id,
                group.id,
                0,
                RecordingRuleGroupRulesFilter::All,
            )
            .to_string(),
        )],
        vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "admin.rule_groups.rename"),
                recording_rule_group_rename_payload(room_id, group.id).to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "admin.rule_groups.delete"),
                recording_rule_group_confirm_delete_payload(room_id, group.id).to_string(),
            ),
        ],
        vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
            recording_rule_groups_payload(room_id),
        )],
    ];

    let text = format!(
        "{}\n\n{}: {}\n{}: {}\nДоступ: {}\n{}: {}\n\n{}",
        t(ctx.lang, "admin.rule_groups.detail"),
        t(ctx.lang, "admin.rule_groups.name"),
        group.name,
        t(ctx.lang, "admin.rule_groups.status"),
        status,
        group.access_scope_label(),
        t(ctx.lang, "admin.rule_groups.rules_count"),
        group.rules_count,
        t(ctx.lang, "admin.rule_groups.detail_hint"),
    );

    Ok(View {
        header: Some(t(ctx.lang, "admin.rule_groups").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: recording_rule_group_detail_payload(room_id, group.id),
        ..Default::default()
    })
}

pub async fn render_recording_rule_group_rules(
    ctx: RenderContext,
    group_id: i64,
    page: u16,
    filter: RecordingRuleGroupRulesFilter,
) -> Result<View> {
    render_recording_rule_group_rules_with_back(ctx, None, group_id, page, filter).await
}

pub async fn render_recording_rule_group_rules_for_room(
    ctx: RenderContext,
    room_id: i64,
    group_id: i64,
    page: u16,
    filter: RecordingRuleGroupRulesFilter,
) -> Result<View> {
    render_recording_rule_group_rules_with_back(ctx, Some(room_id), group_id, page, filter).await
}

async fn render_recording_rule_group_rules_with_back(
    ctx: RenderContext,
    room_id: Option<i64>,
    group_id: i64,
    page: u16,
    filter: RecordingRuleGroupRulesFilter,
) -> Result<View> {
    const PAGE_SIZE: usize = 8;

    let Some(group) =
        crate::db::camera_recording_rule_groups::get_group(group_id, &ctx.config.db).await?
    else {
        let lang = ctx.lang;
        let mut view = render_recording_rule_groups_with_back(ctx, room_id).await?;
        view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
        return Ok(view);
    };

    let all_rules =
        crate::db::camera_recording_rule_groups::list_group_rules(group_id, &ctx.config.db).await?;
    let selected_count = all_rules.iter().filter(|rule| rule.is_selected()).count();
    let filtered_rules = all_rules
        .iter()
        .filter(|rule| match filter {
            RecordingRuleGroupRulesFilter::All => true,
            RecordingRuleGroupRulesFilter::Selected => rule.is_selected(),
            RecordingRuleGroupRulesFilter::Unselected => !rule.is_selected(),
        })
        .collect::<Vec<_>>();
    let total_pages = filtered_rules.len().div_ceil(PAGE_SIZE).max(1);
    let page_index = usize::from(page).min(total_pages.saturating_sub(1));
    let page_rules = filtered_rules
        .iter()
        .skip(page_index * PAGE_SIZE)
        .take(PAGE_SIZE)
        .copied()
        .collect::<Vec<_>>();
    let current_payload =
        recording_rule_group_rules_payload(room_id, group_id, page_index as u16, filter);

    let mut rows = vec![vec![
        InlineKeyboardButton::callback(
            recording_rule_group_rules_filter_label(
                ctx.lang,
                RecordingRuleGroupRulesFilter::All,
                filter,
            ),
            recording_rule_group_rules_payload(
                room_id,
                group_id,
                0,
                RecordingRuleGroupRulesFilter::All,
            )
            .to_string(),
        ),
        InlineKeyboardButton::callback(
            recording_rule_group_rules_filter_label(
                ctx.lang,
                RecordingRuleGroupRulesFilter::Selected,
                filter,
            ),
            recording_rule_group_rules_payload(
                room_id,
                group_id,
                0,
                RecordingRuleGroupRulesFilter::Selected,
            )
            .to_string(),
        ),
        InlineKeyboardButton::callback(
            recording_rule_group_rules_filter_label(
                ctx.lang,
                RecordingRuleGroupRulesFilter::Unselected,
                filter,
            ),
            recording_rule_group_rules_payload(
                room_id,
                group_id,
                0,
                RecordingRuleGroupRulesFilter::Unselected,
            )
            .to_string(),
        ),
    ]];

    let page_rules_empty = page_rules.is_empty();
    let mut last_room_label = String::new();
    for rule in page_rules.iter().copied() {
        let room_label = recording_rule_group_rule_room_label(rule);
        if room_label != last_room_label {
            rows.push(vec![InlineKeyboardButton::callback(
                format!("— {} —", room_label),
                current_payload.to_string(),
            )]);
            last_room_label = room_label;
        }

        let checked = if rule.is_selected() { "☑" } else { "☐" };
        let paused = if rule.is_rule_enabled() { "" } else { " ⏸" };
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {} · {}{}",
                checked, rule.rule_name, rule.camera_name, paused
            ),
            recording_rule_group_toggle_rule_payload(
                room_id,
                group.id,
                rule.rule_id,
                page_index as u16,
                filter,
            )
            .to_string(),
        )]);
    }

    if page_rules_empty {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "admin.rule_groups.rules_empty"),
            current_payload.to_string(),
        )]);
    }

    if total_pages > 1 {
        let prev_page = page_index.saturating_sub(1) as u16;
        let next_page = (page_index + 1).min(total_pages - 1) as u16;
        let page_label = format!("{}/{}", page_index + 1, total_pages);
        rows.push(vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "common.prev"),
                recording_rule_group_rules_payload(room_id, group.id, prev_page, filter)
                    .to_string(),
            ),
            InlineKeyboardButton::callback(page_label, current_payload.to_string()),
            InlineKeyboardButton::callback(
                t(ctx.lang, "common.next"),
                recording_rule_group_rules_payload(room_id, group.id, next_page, filter)
                    .to_string(),
            ),
        ]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        t(ctx.lang, "common.done"),
        recording_rule_group_detail_payload(room_id, group.id).to_string(),
    )]);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        recording_rule_group_detail_payload(room_id, group.id),
    )]);

    let page_label = format!("{}/{}", page_index + 1, total_pages);
    let text = format!(
        "{}: {}\n\n{}: {}\n{}: {} / {}\n{}: {}\n\n{}",
        t(ctx.lang, "admin.rule_groups.rules_title"),
        group.name,
        t(ctx.lang, "admin.rule_groups.rules_filter"),
        recording_rule_group_rules_filter_name(ctx.lang, filter),
        t(ctx.lang, "admin.rule_groups.selected_count"),
        selected_count,
        all_rules.len(),
        t(ctx.lang, "admin.rule_groups.page"),
        page_label,
        t(ctx.lang, "admin.rule_groups.rules_hint"),
    );

    Ok(View {
        header: Some(t(ctx.lang, "admin.rule_groups").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: current_payload,
        ..Default::default()
    })
}

pub async fn render_create_recording_rule_group_input(
    ctx: RenderContext,
    room_id: Option<i64>,
) -> Result<View> {
    let lang = ctx.lang;
    Ok(render_user_input(
        ctx,
        State::AddRecordingRuleGroup { room_id },
        t(lang, "admin.rule_groups.create_title"),
        t(lang, "admin.rule_groups.create_prompt"),
        recording_rule_group_create_payload(room_id),
        recording_rule_groups_payload(room_id),
    ))
}

pub async fn render_create_recording_rule_group_for_edit_input(
    ctx: RenderContext,
    room_id: i64,
    rule_id: i64,
) -> Result<View> {
    if recording_rule_for_room(&ctx, room_id, rule_id)
        .await?
        .is_none()
    {
        let mut view = render_recording_rules(ctx, room_id).await?;
        view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
        return Ok(view);
    }

    let lang = ctx.lang;
    Ok(render_user_input(
        ctx,
        State::AddRecordingRuleGroupForEdit { room_id, rule_id },
        t(lang, "admin.rule_groups.create_title"),
        t(lang, "admin.rule_groups.create_prompt"),
        Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroupForEdit {
            room: room_id,
            rule: rule_id,
        }),
        Payload::Admin(AdminPayload::RecordingRuleEditGroups {
            room: room_id,
            rule: rule_id,
        }),
    ))
}

pub async fn render_create_recording_rule_group_for_wizard_input(
    ctx: RenderContext,
) -> Result<View> {
    if wizard_state(&ctx).is_none() {
        let mut view = render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    }

    let lang = ctx.lang;
    Ok(render_wizard_user_input(
        ctx,
        State::AddRecordingRuleGroupForWizard,
        t(lang, "admin.rule_groups.create_title"),
        t(lang, "admin.rule_groups.create_prompt"),
        Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroupForWizard),
        Payload::Admin(AdminPayload::WizardGroups),
    ))
}

pub async fn render_rename_recording_rule_group_input(
    ctx: RenderContext,
    room_id: Option<i64>,
    group_id: i64,
) -> Result<View> {
    let Some(group) =
        crate::db::camera_recording_rule_groups::get_group(group_id, &ctx.config.db).await?
    else {
        let lang = ctx.lang;
        let mut view = render_recording_rule_groups_with_back(ctx, room_id).await?;
        view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
        return Ok(view);
    };

    let lang = ctx.lang;
    Ok(render_user_input(
        ctx,
        State::RenameRecordingRuleGroup { room_id, group_id },
        t(lang, "admin.rule_groups.rename_title"),
        &format!(
            "{}\n\n{}: {}",
            t(lang, "admin.rule_groups.rename_prompt"),
            t(lang, "admin.rule_groups.current_name"),
            group.name
        ),
        recording_rule_group_rename_payload(room_id, group_id),
        recording_rule_group_detail_payload(room_id, group_id),
    ))
}

fn recording_rule_groups_back_payload(room_id: Option<i64>) -> Payload {
    room_id
        .map(|room| Payload::Admin(AdminPayload::RecordingRules { room }))
        .unwrap_or(Payload::Admin(AdminPayload::ListActions))
}

fn recording_rule_groups_payload(room_id: Option<i64>) -> Payload {
    room_id
        .map(|room| Payload::Admin(AdminPayload::RecordingRuleGroupsForRoom { room }))
        .unwrap_or(Payload::Admin(AdminPayload::RecordingRuleGroups))
}

fn recording_rule_groups_defaults_payload(room_id: Option<i64>) -> Payload {
    room_id
        .map(|room| Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForRoom { room }))
        .unwrap_or(Payload::Admin(AdminPayload::EnsureDefaultRuleGroups))
}

fn recording_rule_group_detail_payload(room_id: Option<i64>, group_id: i64) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::RecordingRuleGroupDetailForRoom {
            room,
            group: group_id,
        },
        None => AdminPayload::RecordingRuleGroupDetail { group: group_id },
    })
}

fn recording_rule_group_create_payload(room_id: Option<i64>) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::PromptCreateRecordingRuleGroupForRoom { room },
        None => AdminPayload::PromptCreateRecordingRuleGroup,
    })
}

fn recording_rule_group_rename_payload(room_id: Option<i64>, group_id: i64) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::PromptRenameRecordingRuleGroupForRoom {
            room,
            group: group_id,
        },
        None => AdminPayload::PromptRenameRecordingRuleGroup { group: group_id },
    })
}

fn recording_rule_group_toggle_detail_payload(room_id: Option<i64>, group_id: i64) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::ToggleRecordingRuleGroupDetailForRoom {
            room,
            group: group_id,
        },
        None => AdminPayload::ToggleRecordingRuleGroupDetail { group: group_id },
    })
}

fn recording_rule_group_confirm_delete_payload(room_id: Option<i64>, group_id: i64) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::ConfirmDeleteRecordingRuleGroupForRoom {
            room,
            group: group_id,
        },
        None => AdminPayload::ConfirmDeleteRecordingRuleGroup { group: group_id },
    })
}

fn recording_rule_group_access_payload(room_id: Option<i64>, group_id: i64) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::CycleRecordingRuleGroupAccessForRoom {
            room,
            group: group_id,
        },
        None => AdminPayload::CycleRecordingRuleGroupAccess { group: group_id },
    })
}

fn recording_rule_group_rules_payload(
    room_id: Option<i64>,
    group_id: i64,
    page: u16,
    filter: RecordingRuleGroupRulesFilter,
) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::RecordingRuleGroupRulesForRoom {
            room,
            group: group_id,
            page,
            filter,
        },
        None => AdminPayload::RecordingRuleGroupRules {
            group: group_id,
            page,
            filter,
        },
    })
}

fn recording_rule_group_toggle_rule_payload(
    room_id: Option<i64>,
    group_id: i64,
    rule_id: i64,
    page: u16,
    filter: RecordingRuleGroupRulesFilter,
) -> Payload {
    Payload::Admin(match room_id {
        Some(room) => AdminPayload::ToggleRecordingRuleGroupRuleForRoom {
            room,
            group: group_id,
            rule: rule_id,
            page,
            filter,
        },
        None => AdminPayload::ToggleRecordingRuleGroupRule {
            group: group_id,
            rule: rule_id,
            page,
            filter,
        },
    })
}

fn recording_rule_group_rules_filter_label(
    lang: crate::i18n::Language,
    candidate: RecordingRuleGroupRulesFilter,
    current: RecordingRuleGroupRulesFilter,
) -> String {
    let prefix = if candidate == current { "●" } else { "○" };
    format!(
        "{} {}",
        prefix,
        recording_rule_group_rules_filter_name(lang, candidate)
    )
}

fn recording_rule_group_rules_filter_name(
    lang: crate::i18n::Language,
    filter: RecordingRuleGroupRulesFilter,
) -> &'static str {
    match filter {
        RecordingRuleGroupRulesFilter::All => t(lang, "admin.rule_groups.filter_all"),
        RecordingRuleGroupRulesFilter::Selected => t(lang, "admin.rule_groups.filter_selected"),
        RecordingRuleGroupRulesFilter::Unselected => t(lang, "admin.rule_groups.filter_unselected"),
    }
}

fn recording_rule_group_rule_room_label(
    rule: &crate::db::camera_recording_rule_groups::RecordingRuleGroupRule,
) -> String {
    let name = rule
        .room_alias
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            rule.room_area
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("Без комнаты");

    format!("🏠 {}", name)
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
    let shutdown = format_shutdown_status(
        runtime_status.shutdown_requested_at,
        runtime_status.shutdown_reason.as_deref(),
    );

    let text = format!(
        "Статус системы\n\nHA: {}\nПоследний heartbeat: {}\nHA sync: {}\nShutdown: {}\nПользователей: {}\nКомнат: {}\nАктивных устройств: {}\nАрхивных устройств: {}\nПодписок: {}\nСобытий в журнале: {}\nАктивных сессий: {}\nUI refresh на паузе: {}\nБлижайшая разблокировка: {}",
        ha_status,
        last_heartbeat,
        ha_sync,
        shutdown,
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

fn format_shutdown_status(shutdown_at: Option<DateTime<Utc>>, reason: Option<&str>) -> String {
    shutdown_at
        .map(|dt| {
            format!(
                "{} {}",
                reason.unwrap_or("signal"),
                dt.with_timezone(&Local).format("%H:%M:%S")
            )
        })
        .unwrap_or_else(|| "нет".to_string())
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
    current_payload: Payload,
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
        payload: current_payload,
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
            t(lang, "admin.action_groups"),
            Payload::Admin(AdminPayload::ActionGroups).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.rule_groups"),
            Payload::Admin(AdminPayload::RecordingRuleGroups).to_string(),
        )],
        vec![InlineKeyboardButton::callback(
            t(lang, "admin.settings"),
            Payload::AdminSettings(SettingsPayload::ListRooms).to_string(),
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
    fn admin_menu_shows_package_version() {
        let text = admin_menu_text(crate::i18n::Language::Ru);

        assert!(text.contains("Версия бота"));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
    }

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
            active_time_enabled: 0,
            active_from_minute: None,
            active_to_minute: None,
            active_days_mask: crate::db::camera_recording_rules::ACTIVE_TIME_ALL_DAYS_MASK,
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
