use crate::bot::models::View;
use crate::bot::router::{CameraPayload, Payload, RenderContext};
use crate::i18n::{t, Language};
use anyhow::{Context, Result};
use chrono::Utc;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render_list(ctx: RenderContext) -> Result<View> {
    render_list_page(ctx, None).await
}

pub async fn render_list_page(ctx: RenderContext, requested: Option<u16>) -> Result<View> {
    use super::common;
    let cameras =
        crate::db::cameras::list_accessible_cameras(ctx.user_id, ctx.is_admin, &ctx.config.db)
            .await?;
    let cameras_empty = cameras.is_empty();
    let (page, pages) = common::list_page(
        &ctx,
        &Payload::Camera(CameraPayload::ListCameras),
        requested,
        cameras.len(),
    )
    .await?;
    let mut rows = Vec::new();

    for camera in cameras
        .into_iter()
        .skip(page * common::LIST_PAGE_SIZE)
        .take(common::LIST_PAGE_SIZE)
    {
        ctx.track_camera(camera.id);
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

    let pager = common::page_row(page, pages, |page| {
        Payload::Camera(CameraPayload::ListCamerasPage { page })
    });
    if !pager.is_empty() {
        rows.push(pager);
    }
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
    render_recording_group_modes_page(ctx, None).await
}

pub async fn render_recording_group_modes_page(
    ctx: RenderContext,
    requested: Option<u16>,
) -> Result<View> {
    use super::common;
    let groups = crate::db::camera_recording_rule_groups::list_groups_visible_to_user(
        ctx.user_id,
        ctx.config.root_user,
        &ctx.config.db,
    )
    .await?;
    for group in &groups {
        ctx.track_recording_group(group.id);
    }
    let (page, pages) = common::list_page(
        &ctx,
        &Payload::Camera(CameraPayload::RecordingGroupModes),
        requested,
        groups.len(),
    )
    .await?;
    let mut rows = Vec::new();
    for group in groups
        .iter()
        .skip(page * common::LIST_PAGE_SIZE)
        .take(common::LIST_PAGE_SIZE)
    {
        let icon = if group.is_enabled() { "✅" } else { "⏸" };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} {} · {} правил", icon, group.name, group.rules_count),
            Payload::Camera(CameraPayload::RecordingGroupModeDetail { group: group.id })
                .to_string(),
        )]);
    }
    let pager = common::page_row(page, pages, |page| {
        Payload::Camera(CameraPayload::RecordingGroupModesPage { page })
    });
    if !pager.is_empty() {
        rows.push(pager);
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

    ctx.track_recording_group(group.id);
    let cameras = crate::db::camera_recording_rule_groups::list_group_cameras_visible_to_user(
        ctx.user_id,
        ctx.is_admin,
        group.id,
        &ctx.config.db,
    )
    .await?;
    for camera in &cameras {
        ctx.track_camera(camera.camera_id);
    }
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
    ctx.track_camera(camera.id);

    let mut rows = vec![vec![InlineKeyboardButton::callback(
        t(ctx.lang, "camera.snapshot"),
        Payload::Camera(CameraPayload::Snapshot { id: camera.id }).to_string(),
    )]];

    let mut interval_row = Vec::new();
    let mut html_button_labels = std::collections::HashMap::new();
    for seconds in &ctx.config.camera_clip_intervals_s {
        html_button_labels.insert(
            Payload::Camera(CameraPayload::Clip {
                id: camera.id,
                seconds: *seconds,
            })
            .to_string(),
            format!(
                "{} · {seconds} {}",
                localized(ctx.lang, "Видео", "Video"),
                localized(ctx.lang, "с", "s")
            ),
        );
        interval_row.push(InlineKeyboardButton::callback(
            format!("🎞 {}с", seconds),
            Payload::Camera(CameraPayload::Clip {
                id: camera.id,
                seconds: *seconds,
            })
            .to_string(),
        ));

        if interval_row.len() == 2 {
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

    rows.push(vec![
        InlineKeyboardButton::callback(
            t(ctx.lang, "camera.archive.button"),
            Payload::Camera(CameraPayload::RecordingArchive { camera: camera.id }).to_string(),
        ),
        InlineKeyboardButton::callback(
            localized(ctx.lang, "Подробности", "Details"),
            Payload::Camera(CameraPayload::CameraDetails { id: camera.id }).to_string(),
        ),
    ]);
    html_button_labels.insert(
        Payload::Camera(CameraPayload::RecordingArchive { camera: camera.id }).to_string(),
        localized(ctx.lang, "Архив", "Archive").into(),
    );
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

    let mut text = format!(
        "{}{}\n\n{}: {}с.",
        camera.name,
        recording_text,
        t(ctx.lang, "camera.detail.help"),
        camera.clip_seconds
    );
    let mut room_context = localized(ctx.lang, "Камеры", "Cameras").to_owned();
    if let Some(room_id) = camera.room_id {
        if let Some(room) = crate::db::rooms::get_room_by_id(room_id, &ctx.config.db).await? {
            ctx.track_room(room.id);
            room_context = room.display_name();
            text.push_str(&format!(
                "\n{}: {}",
                localized(ctx.lang, "Комната", "Room"),
                room.display_name()
            ));
        }
    }
    let (image, preview_text) =
        camera_snapshot_preview(ctx.config.clone(), &camera, ctx.lang).await;
    text.push_str(&format!("\n\n{}", preview_text));

    Ok(View {
        header: Some(t(ctx.lang, "camera.detail.header").to_string()),
        html_title: Some(camera.name.clone()),
        html_context: Some(room_context),
        html_summary: Some(format!(
            "{}{}",
            media_delivery_hint(ctx.lang),
            if active_session.is_some() {
                format!("\n{}", localized(ctx.lang, "● Идёт запись", "● Recording"))
            } else {
                String::new()
            }
        )),
        html_button_labels,
        html_media_controls: true,
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::CameraDetail { id: camera.id }),
        image,
        lang: ctx.lang,
        ..Default::default()
    })
}

pub async fn render_details(ctx: RenderContext, camera_id: i64) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;
    ctx.track_camera(camera.id);
    let mut text = format!(
        "{}\nID: {}\n{}: {} s",
        camera.name,
        camera.id,
        t(ctx.lang, "camera.detail.help"),
        camera.clip_seconds
    );
    if let Some(health) = crate::db::camera_health::get(camera_id, &ctx.config.db)
        .await?
        .filter(|_| ctx.is_admin)
    {
        text.push_str(&format!(
            "\n\n{}: {}",
            localized(ctx.lang, "Диагностика", "Diagnostics"),
            health.health_state
        ));
        if let Some(last_check) = health.last_check_at {
            text.push_str(&format!(
                "\n{}: {}",
                localized(ctx.lang, "Последняя проверка", "Last check"),
                crate::bot::format::datetime(last_check)
            ));
        }
        if let Some(error) = health.last_error {
            text.push_str(&format!(
                "\n{}: {}",
                localized(ctx.lang, "Ошибка", "Error"),
                error
            ));
        }
    }
    Ok(View {
        header: Some(localized(ctx.lang, "Подробности камеры", "Camera details").into()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(vec![vec![super::common::back_button_lang(
            ctx.lang,
            Payload::Camera(CameraPayload::CameraDetail { id: camera_id }),
        )]]),
        payload: Payload::Camera(CameraPayload::CameraDetails { id: camera_id }),
        lang: ctx.lang,
        ..Default::default()
    })
}

pub async fn render_recording_archive(ctx: RenderContext, camera_id: i64) -> Result<View> {
    let key = format!("ui:archive:{}:{camera_id}", ctx.user_id);
    let saved = crate::db::settings::get_string(&key, &ctx.config.db)
        .await?
        .and_then(|value| Payload::from_string(&value).ok());
    if let Some(Payload::Camera(CameraPayload::RecordingArchivePage {
        camera,
        filter,
        page,
    })) = saved
    {
        if camera == camera_id {
            return render_recording_archive_page(ctx, camera_id, &filter, page).await;
        }
    }
    render_recording_archive_filtered(ctx, camera_id, "all").await
}

pub async fn render_recording_archive_filtered(
    ctx: RenderContext,
    camera_id: i64,
    filter: &str,
) -> Result<View> {
    render_recording_archive_page(ctx, camera_id, filter, 0).await
}

pub async fn render_recording_archive_page(
    ctx: RenderContext,
    camera_id: i64,
    filter: &str,
    page: u16,
) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;
    ctx.track_camera(camera.id);
    let rules = camera_archive_rules(camera_id, &ctx.config.db).await?;
    let filter_missing = filter.strip_prefix("rule:").is_some_and(|id| {
        id.parse::<i64>()
            .ok()
            .is_none_or(|id| !rules.iter().any(|rule| rule.id == id))
    });
    let filter = if filter_missing { "all" } else { filter };
    let archive_filter = parse_archive_filter(filter);
    let mut sessions = crate::db::camera_recording_sessions::list_camera_sessions_filtered(
        camera_id,
        archive_filter,
        11,
        i64::from(page) * 10,
        &ctx.config.db,
    )
    .await?;
    let page = if sessions.is_empty() && page > 0 {
        sessions = crate::db::camera_recording_sessions::list_camera_sessions_filtered(
            camera_id,
            parse_archive_filter(filter),
            11,
            0,
            &ctx.config.db,
        )
        .await?;
        0
    } else {
        page
    };
    let has_next = sessions.len() > 10;
    sessions.truncate(10);
    let current = Payload::Camera(CameraPayload::RecordingArchivePage {
        camera: camera_id,
        filter: filter.into(),
        page,
    });
    crate::db::settings::set_string(
        &format!("ui:archive:{}:{camera_id}", ctx.user_id),
        &current.to_string(),
        &ctx.config.db,
    )
    .await?;
    let mut rows = archive_filter_rows(camera_id, filter, ctx.lang);

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

    let mut pager = Vec::new();
    if page > 0 {
        pager.push(InlineKeyboardButton::callback(
            "◀",
            Payload::Camera(CameraPayload::RecordingArchivePage {
                camera: camera_id,
                filter: filter.into(),
                page: page - 1,
            })
            .to_string(),
        ));
    }
    pager.push(InlineKeyboardButton::callback(
        format!("{} {}", t(ctx.lang, "action_groups.page"), page + 1),
        current.to_string(),
    ));
    if has_next && page < u16::MAX {
        pager.push(InlineKeyboardButton::callback(
            "▶",
            Payload::Camera(CameraPayload::RecordingArchivePage {
                camera: camera_id,
                filter: filter.into(),
                page: page + 1,
            })
            .to_string(),
        ));
    }
    rows.push(pager);
    rows.extend(crate::bot::screens::common::navigation_rows(
        ctx.lang,
        current.clone(),
        Payload::Camera(CameraPayload::CameraDetail { id: camera_id }),
    ));

    let mut text = format!(
        "{} · {}\n{}: {}\n\n{}: {}",
        camera.name,
        t(ctx.lang, "camera.archive.header"),
        localized(ctx.lang, "Фильтр", "Filter"),
        archive_filter_name(filter, &rules, ctx.lang),
        localized(ctx.lang, "Показано", "Shown"),
        sessions.len(),
    );
    if sessions.is_empty() {
        text.push_str(&format!("\n{}", t(ctx.lang, "camera.archive.empty")));
        rows.insert(
            1,
            vec![InlineKeyboardButton::callback(
                localized(ctx.lang, "Показать все", "Show all"),
                Payload::Camera(CameraPayload::RecordingArchiveFiltered {
                    camera: camera_id,
                    filter: "all".into(),
                })
                .to_string(),
            )],
        );
    }
    let image = recording_archive_preview(ctx.config.clone(), &sessions).await;

    Ok(View {
        header: Some(t(ctx.lang, "camera.archive.header").to_string()),
        html_context: Some(camera.name.clone()),
        html_media_controls: true,
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        alert: filter_missing.then(|| {
            localized(
                ctx.lang,
                "Правило больше недоступно. Показаны все записи.",
                "This rule is no longer available. Showing all recordings.",
            )
            .into()
        }),
        lang: ctx.lang,
        payload: current,
        image,
        ..Default::default()
    })
}

fn parse_archive_filter(
    filter: &str,
) -> crate::db::camera_recording_sessions::RecordingArchiveFilter {
    match filter {
        "today" => crate::db::camera_recording_sessions::RecordingArchiveFilter::Today,
        "week" => crate::db::camera_recording_sessions::RecordingArchiveFilter::Week,
        "ready" => crate::db::camera_recording_sessions::RecordingArchiveFilter::Ready,
        "failed" => crate::db::camera_recording_sessions::RecordingArchiveFilter::Failed,
        "pinned" => crate::db::camera_recording_sessions::RecordingArchiveFilter::Pinned,
        value if value.starts_with("rule:") => value
            .trim_start_matches("rule:")
            .parse::<i64>()
            .map(crate::db::camera_recording_sessions::RecordingArchiveFilter::Rule)
            .unwrap_or(crate::db::camera_recording_sessions::RecordingArchiveFilter::All),
        _ => crate::db::camera_recording_sessions::RecordingArchiveFilter::All,
    }
}

fn localized<'a>(lang: Language, ru: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Ru => ru,
        Language::En => en,
    }
}

fn archive_filter_rows(
    camera_id: i64,
    active: &str,
    lang: Language,
) -> Vec<Vec<InlineKeyboardButton>> {
    let mut row = ["today", "all"]
        .into_iter()
        .map(|filter| {
            let label = archive_filter_name(filter, &[], lang);
            InlineKeyboardButton::callback(
                if filter == active {
                    format!("• {label}")
                } else {
                    label
                },
                Payload::Camera(CameraPayload::RecordingArchiveFiltered {
                    camera: camera_id,
                    filter: filter.into(),
                })
                .to_string(),
            )
        })
        .collect::<Vec<_>>();
    row.push(InlineKeyboardButton::callback(
        localized(lang, "Фильтры", "Filters"),
        Payload::Camera(CameraPayload::RecordingArchiveFilters { camera: camera_id }).to_string(),
    ));
    vec![row]
}

fn archive_filter_name(
    filter: &str,
    rules: &[crate::db::camera_recording_rules::RecordingRule],
    lang: Language,
) -> String {
    if let Some(rule) = filter
        .strip_prefix("rule:")
        .and_then(|id| id.parse::<i64>().ok())
    {
        return match rules.iter().find(|candidate| candidate.id == rule) {
            Some(rule) => format!(
                "{}: {} · #{}",
                localized(lang, "Правило", "Rule"),
                rule.name,
                rule.id
            ),
            None => format!(
                "{} #{}",
                localized(lang, "Правило недоступно", "Unavailable rule"),
                rule
            ),
        };
    }
    match filter {
        "today" => localized(lang, "Сегодня", "Today"),
        "week" => localized(lang, "7 дней", "7 days"),
        "ready" => localized(lang, "Готовые", "Ready"),
        "failed" => localized(lang, "Ошибки", "Failed"),
        "pinned" => localized(lang, "Закреплённые", "Pinned"),
        _ => localized(lang, "Все", "All"),
    }
    .into()
}

async fn camera_archive_rules(
    camera_id: i64,
    pool: &sqlx::SqlitePool,
) -> Result<Vec<crate::db::camera_recording_rules::RecordingRule>> {
    let mut rules = crate::db::camera_recording_rules::list_rules(pool)
        .await?
        .into_iter()
        .filter(|rule| rule.camera_id == camera_id)
        .collect::<Vec<_>>();
    rules.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(rules)
}

async fn saved_archive_payload(ctx: &RenderContext, camera_id: i64) -> Result<Payload> {
    Ok(crate::db::settings::get_string(&format!("ui:archive:{}:{camera_id}", ctx.user_id), &ctx.config.db)
        .await?.and_then(|value| Payload::from_string(&value).ok())
        .filter(|payload| matches!(payload, Payload::Camera(CameraPayload::RecordingArchivePage { camera, .. }) if *camera == camera_id))
        .unwrap_or(Payload::Camera(CameraPayload::RecordingArchivePage { camera: camera_id, filter: "all".into(), page: 0 })))
}

pub async fn render_recording_archive_filters(ctx: RenderContext, camera_id: i64) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;
    ctx.track_camera(camera.id);
    let back = saved_archive_payload(&ctx, camera_id).await?;
    let active = match &back {
        Payload::Camera(CameraPayload::RecordingArchivePage { filter, .. }) => filter.as_str(),
        _ => "all",
    };
    let rules = camera_archive_rules(camera_id, &ctx.config.db).await?;
    let mut rows = ["all", "today", "week", "ready", "failed", "pinned"]
        .into_iter()
        .map(|filter| {
            let name = archive_filter_name(filter, &[], ctx.lang);
            vec![InlineKeyboardButton::callback(
                if active == filter {
                    format!("• {name}")
                } else {
                    name
                },
                Payload::Camera(CameraPayload::RecordingArchiveFiltered {
                    camera: camera_id,
                    filter: filter.into(),
                })
                .to_string(),
            )]
        })
        .collect::<Vec<_>>();
    rows.push(vec![InlineKeyboardButton::callback(
        localized(ctx.lang, "По правилу", "By rule"),
        Payload::Camera(CameraPayload::RecordingArchiveRules {
            camera: camera_id,
            page: 0,
        })
        .to_string(),
    )]);
    rows.push(vec![super::common::back_button_lang(
        ctx.lang,
        back.clone(),
    )]);
    Ok(View {
        header: Some(localized(ctx.lang, "Фильтры архива", "Archive filters").into()),
        text: format!(
            "{}\n{}: {}",
            camera.name,
            localized(ctx.lang, "Текущий фильтр", "Current filter"),
            archive_filter_name(active, &rules, ctx.lang)
        ),
        notifications: ctx.notifications,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingArchiveFilters { camera: camera_id }),
        lang: ctx.lang,
        ..Default::default()
    })
}

pub async fn render_recording_archive_rules(
    ctx: RenderContext,
    camera_id: i64,
    requested_page: u16,
) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;
    ctx.track_camera(camera.id);
    let rules = camera_archive_rules(camera_id, &ctx.config.db).await?;
    let pages = rules.len().div_ceil(super::common::LIST_PAGE_SIZE).max(1);
    let page = usize::from(requested_page).min(pages - 1);
    let mut rows = Vec::new();
    let mut text = format!(
        "{}\n\n{}",
        camera.name,
        localized(
            ctx.lang,
            "Выберите правило. Полное название можно проверить перед применением.",
            "Choose a rule. Review its full name before applying."
        )
    );
    for rule in rules
        .iter()
        .skip(page * super::common::LIST_PAGE_SIZE)
        .take(super::common::LIST_PAGE_SIZE)
    {
        text.push_str(&format!("\n#{} · {}", rule.id, rule.name));
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} · #{}", archive_rule_label(&rule.name), rule.id),
            Payload::Camera(CameraPayload::RecordingArchiveRule {
                camera: camera_id,
                rule: rule.id,
                page: page as u16,
            })
            .to_string(),
        )]);
    }
    if rules.is_empty() {
        text.push_str(&format!(
            "\n{}",
            localized(ctx.lang, "Нет доступных правил", "No available rules")
        ));
    }
    let pager = super::common::page_row(page, pages, |page| {
        Payload::Camera(CameraPayload::RecordingArchiveRules {
            camera: camera_id,
            page,
        })
    });
    if !pager.is_empty() {
        rows.push(pager);
    }
    rows.push(vec![super::common::back_button_lang(
        ctx.lang,
        Payload::Camera(CameraPayload::RecordingArchiveFilters { camera: camera_id }),
    )]);
    Ok(View {
        header: Some(localized(ctx.lang, "Фильтр по правилу", "Filter by rule").into()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingArchiveRules {
            camera: camera_id,
            page: page as u16,
        }),
        lang: ctx.lang,
        ..Default::default()
    })
}

pub async fn render_recording_archive_rule(
    ctx: RenderContext,
    camera_id: i64,
    rule_id: i64,
    page: u16,
) -> Result<View> {
    let camera = crate::db::cameras::get_accessible_camera(
        ctx.user_id,
        ctx.is_admin,
        camera_id,
        &ctx.config.db,
    )
    .await?
    .context("Camera not found or access denied")?;
    ctx.track_camera(camera.id);
    let rule = crate::db::camera_recording_rules::get_rule(rule_id, &ctx.config.db)
        .await?
        .filter(|rule| rule.camera_id == camera_id);
    let Some(rule) = rule else {
        let message = localized(
            ctx.lang,
            "Правило больше недоступно",
            "This rule is no longer available",
        )
        .to_string();
        let mut view = render_recording_archive_rules(ctx, camera_id, page).await?;
        view.alert = Some(message);
        return Ok(view);
    };
    Ok(View {
        header: Some(localized(ctx.lang, "Фильтр по правилу", "Filter by rule").into()),
        notifications: ctx.notifications,
        text: format!("{}\n\n{}\n#{}", camera.name, rule.name, rule.id),
        kb: InlineKeyboardMarkup::new(vec![
            vec![InlineKeyboardButton::callback(
                localized(ctx.lang, "Применить фильтр", "Apply filter"),
                Payload::Camera(CameraPayload::RecordingArchiveFiltered {
                    camera: camera_id,
                    filter: format!("rule:{rule_id}"),
                })
                .to_string(),
            )],
            vec![super::common::back_button_lang(
                ctx.lang,
                Payload::Camera(CameraPayload::RecordingArchiveRules {
                    camera: camera_id,
                    page,
                }),
            )],
        ]),
        payload: Payload::Camera(CameraPayload::RecordingArchiveRule {
            camera: camera_id,
            rule: rule_id,
            page,
        }),
        lang: ctx.lang,
        ..Default::default()
    })
}

fn archive_rule_label(name: &str) -> String {
    const LIMIT: usize = 18;
    let normalized = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= LIMIT {
        normalized
    } else {
        let mut label = normalized.chars().take(LIMIT).collect::<String>();
        label.push_str("...");
        label
    }
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
    ctx.track_camera(camera.id);
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
    let partial = (session.status == "failed" || session.error.is_some()) && ready_segments > 0;
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
    } else if ready_segments > 1
        && (!sending_all || crate::bot::html_ui::preferred(&ctx.config, ctx.user_id).await)
    {
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
        let pin_label = if session.pinned_at.is_some() {
            "📌 Открепить"
        } else {
            "📌 Закрепить"
        };
        rows.push(vec![InlineKeyboardButton::callback(
            pin_label,
            Payload::Camera(CameraPayload::ToggleRecordingPin {
                camera: camera_id,
                session: session_id,
            })
            .to_string(),
        )]);
        if !matches!(session.status.as_str(), "queued" | "recording") {
            rows.push(vec![InlineKeyboardButton::callback(
                t(ctx.lang, "camera.delete"),
                Payload::Camera(CameraPayload::ConfirmDeleteRecording {
                    camera: camera_id,
                    session: session_id,
                })
                .to_string(),
            )]);
        }
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

    let pinned_text = session
        .pinned_at
        .map(|pinned_at| {
            format!(
                "\n📌 Закреплена: {}",
                crate::bot::format::datetime(pinned_at)
            )
        })
        .unwrap_or_default();
    let pre_roll_text = recording_pre_roll_text(&session);

    let text = format!(
        "{}\n\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {} / {}\n{}: {}{}{}{}{}{}",
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
        pinned_text,
        pre_roll_text,
        warning,
        disk_warning,
        failure_details
    );

    Ok(View {
        header: Some(t(ctx.lang, "camera.recording.header").to_string()),
        html_context: Some(camera.name.clone()),
        html_media_controls: true,
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Camera(CameraPayload::RecordingSession {
            camera: camera_id,
            session: session_id,
        }),
        image,
        lang: ctx.lang,
        ..Default::default()
    })
}

fn recording_pre_roll_text(
    session: &crate::db::camera_recording_sessions::RecordingSession,
) -> String {
    if session.pre_roll_seconds <= 0 {
        return String::new();
    }

    let mut text = format!(
        "\nPre-roll: сохранено {} с из {} с",
        session.pre_roll_actual_seconds, session.pre_roll_seconds
    );
    if let Some(from) = session.pre_roll_from {
        text.push_str(&format!(" с {}", crate::bot::format::datetime(from)));
    }
    if matches!(session.status.as_str(), "queued" | "recording")
        && session.pre_roll_attempted_seconds < session.pre_roll_seconds
    {
        text.push_str(", подготовка");
    } else if let Some(warning) = session.pre_roll_warning.as_deref() {
        text.push_str(&format!(
            ", частично/недоступен: {}",
            shorten_recording_error(warning)
        ));
    } else if session.pre_roll_partial != 0 {
        text.push_str(", частично");
    }
    text
}

pub(crate) async fn camera_snapshot_preview(
    config: std::sync::Arc<crate::models::AppConfig>,
    camera: &crate::db::cameras::Camera,
    lang: crate::i18n::Language,
) -> (Option<Vec<u8>>, String) {
    let preview = crate::core::camera_snapshots::resolve(config, camera.clone()).await;
    let text = snapshot_preview_text(&preview, lang, Utc::now());
    // A missing camera frame must not fall back to another camera's UI background.
    let image = preview
        .bytes
        .unwrap_or_else(|| crate::bot::utils::UI_PLACEHOLDER_BYTES.to_vec());
    (Some(image), text)
}

pub(crate) fn media_delivery_hint(lang: Language) -> &'static str {
    localized(
        lang,
        "Фото и видео придут в чат.",
        "Photos and videos arrive in chat.",
    )
}

fn snapshot_preview_text(
    preview: &crate::core::camera_snapshots::SnapshotPreview,
    lang: crate::i18n::Language,
    now: chrono::DateTime<Utc>,
) -> String {
    let Some(received_at) = preview.received_at.filter(|_| preview.bytes.is_some()) else {
        return t(
            lang,
            if preview.refreshing {
                "camera.preview.refreshing"
            } else {
                "camera.preview.unavailable"
            },
        )
        .to_string();
    };
    let mut text = format!(
        "{}: {}",
        t(lang, "camera.preview.received"),
        received_at
            .with_timezone(&chrono::Local)
            .format("%d.%m.%Y %H:%M:%S %:z")
    );
    if preview.failed {
        text.push_str(&format!("\n{}", t(lang, "camera.preview.failed")));
    } else if preview.is_stale(now) {
        text.push_str(&format!("\n{}", t(lang, "camera.preview.stale")));
    }
    if preview.refreshing {
        text.push_str(&format!("\n{}", t(lang, "camera.preview.refreshing")));
    }
    text
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
    let pin_icon = if session.pinned_at.is_some() {
        "📌 "
    } else {
        ""
    };
    let end = session.completed_at.unwrap_or(session.stop_after_at);
    let duration = end
        .signed_duration_since(session.first_event_at)
        .num_seconds()
        .max(0);

    format!(
        "{}{} {} · {}",
        pin_icon,
        status_icon,
        crate::bot::format::datetime(session.first_event_at),
        crate::core::camera_recording::format_recording_duration(duration)
    )
}
