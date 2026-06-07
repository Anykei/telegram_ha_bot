use crate::bot::models::View;
use crate::bot::router::{AdminPayload, ControlPayload, Payload, RenderContext, State};
use crate::db;
use crate::i18n::{t, Language};
use anyhow::Result;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

const ITEMS_PAGE_SIZE: usize = 8;

pub async fn render_user_list(ctx: RenderContext) -> Result<View> {
    let mut sync_alert = None;
    if let Err(error) = crate::core::action_groups::sync_ha_native_targets(&ctx.config).await {
        sync_alert = Some(format!(
            "{}: {}",
            t(ctx.lang, "action_groups.sync_failed"),
            error
        ));
    }

    let groups =
        db::action_groups::list_visible_action_groups(ctx.is_admin, &ctx.config.db).await?;
    let targets =
        db::action_groups::list_visible_ha_native_targets(ctx.is_admin, &ctx.config.db).await?;

    let mut rows = Vec::new();
    for group in &groups {
        let state = crate::core::action_groups::aggregate_group_state(
            group.id,
            ctx.user_id,
            ctx.is_admin,
            &ctx.config,
        )
        .await
        .unwrap_or(crate::core::action_groups::AggregateState::Unknown);
        rows.push(vec![InlineKeyboardButton::callback(
            format!("💡 {} · {}", group.name, aggregate_label(ctx.lang, state)),
            Payload::Control(ControlPayload::ActionGroupDetail { group: group.id }).to_string(),
        )]);
    }

    for target in &targets {
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {} · {}",
                ha_icon(&target.domain),
                target.display_name(),
                target.entity_id
            ),
            Payload::Control(ControlPayload::HaNativeActionDetail { action: target.id })
                .to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Home,
    )]);

    let text = if groups.is_empty() && targets.is_empty() {
        format!(
            "{}\n\n{}",
            t(ctx.lang, "action_groups.title_plain"),
            t(ctx.lang, "action_groups.empty")
        )
    } else {
        format!(
            "{}\n\n{}: {}\nHome Assistant: {}",
            t(ctx.lang, "action_groups.title_plain"),
            t(ctx.lang, "action_groups.bot_groups"),
            groups.len(),
            targets.len()
        )
    };

    Ok(View {
        header: Some(t(ctx.lang, "action_groups.title").to_string()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Control(ControlPayload::ActionGroups),
        alert: sync_alert,
        ..Default::default()
    })
}

pub async fn render_admin_list(ctx: RenderContext) -> Result<View> {
    let mut sync_alert = None;
    if let Err(error) = crate::core::action_groups::sync_ha_native_targets(&ctx.config).await {
        sync_alert = Some(format!(
            "{}: {}",
            t(ctx.lang, "action_groups.sync_failed"),
            error
        ));
    }

    let groups = db::action_groups::list_action_groups(&ctx.config.db).await?;
    let targets = db::action_groups::list_ha_native_targets(&ctx.config.db).await?;

    let mut rows = vec![vec![InlineKeyboardButton::callback(
        t(ctx.lang, "action_groups.create_group"),
        Payload::Admin(AdminPayload::PromptCreateActionGroup).to_string(),
    )]];

    for group in &groups {
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "💡 {} · {} {} · {}",
                group.name,
                group.items_count,
                t(ctx.lang, "action_groups.devices_count"),
                access_label(ctx.lang, &group.access_scope)
            ),
            Payload::Admin(AdminPayload::ActionGroupDetail { group: group.id }).to_string(),
        )]);
    }

    for target in &targets {
        let archived = if target.is_archived() {
            t(ctx.lang, "action_groups.not_found_suffix")
        } else {
            ""
        };
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {} · {} · {}{}",
                ha_icon(&target.domain),
                target.display_name(),
                target.entity_id,
                access_label(ctx.lang, &target.access_scope),
                archived
            ),
            Payload::Admin(AdminPayload::HaNativeActionDetail { action: target.id }).to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::ListActions),
    )]);

    Ok(View {
        header: Some(t(ctx.lang, "action_groups.title").to_string()),
        notifications: ctx.notifications,
        text: format!(
            "{}\n\n{}: {}\nHome Assistant: {}",
            t(ctx.lang, "action_groups.title_plain"),
            t(ctx.lang, "action_groups.bot_groups"),
            groups.len(),
            targets.len()
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::ActionGroups),
        alert: sync_alert,
        ..Default::default()
    })
}

pub async fn render_group_detail(ctx: RenderContext, group_id: i64, admin: bool) -> Result<View> {
    let lang = ctx.lang;
    let Some(group) = db::action_groups::get_action_group(group_id, &ctx.config.db).await? else {
        let mut view = if admin {
            render_admin_list(ctx).await?
        } else {
            render_user_list(ctx).await?
        };
        view.alert = Some(t(lang, "action_groups.group_not_found").to_string());
        return Ok(view);
    };
    if !ctx.is_admin && !group.is_visible_to_all_users() {
        let mut view = render_user_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.group_no_access").to_string());
        return Ok(view);
    }
    if !ctx.is_admin && !group.is_enabled() {
        let mut view = render_user_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.group_paused").to_string());
        return Ok(view);
    }

    let items = db::action_groups::list_action_group_items(group.id, &ctx.config.db).await?;
    let mut visible_items = Vec::new();
    for item in items {
        if !ctx.is_admin
            && !db::access::can_control_device(
                ctx.user_id,
                ctx.is_admin,
                item.device_id,
                &ctx.config.db,
            )
            .await?
        {
            continue;
        }
        visible_items.push(item);
    }
    let executable_items_count = visible_items
        .iter()
        .filter(|item| !item.is_archived())
        .count();

    let entity_ids = visible_items
        .iter()
        .filter(|item| !item.is_archived())
        .map(|item| item.entity_id.clone())
        .collect::<Vec<_>>();
    let states = ctx
        .config
        .ha_client
        .fetch_states_by_ids(&entity_ids)
        .await
        .unwrap_or_default();
    let state_map = crate::core::action_groups::state_map_by_entity(states);
    let aggregate = crate::core::action_groups::aggregate_states(
        visible_items
            .iter()
            .filter_map(|item| state_map.get(&item.entity_id).map(String::as_str)),
    );

    let mut rows = Vec::new();
    if group.is_enabled() && executable_items_count > 0 {
        let dynamic_command = aggregate.dynamic_command();
        rows.push(vec![InlineKeyboardButton::callback(
            dynamic_button_label(ctx.lang, dynamic_command),
            execute_group_payload(admin, group.id, dynamic_command).to_string(),
        )]);
        rows.push(vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "action_groups.turn_on_all"),
                execute_group_payload(
                    admin,
                    group.id,
                    db::action_groups::ActionGroupCommand::TurnOn,
                )
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "action_groups.turn_off_all"),
                execute_group_payload(
                    admin,
                    group.id,
                    db::action_groups::ActionGroupCommand::TurnOff,
                )
                .to_string(),
            ),
        ]);
    }

    if admin {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "action_groups.devices_button"),
            Payload::Admin(AdminPayload::ActionGroupItems {
                group: group.id,
                page: 0,
                filter: db::action_groups::ActionGroupItemsFilter::All,
            })
            .to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "action_groups.schedules_button"),
            Payload::Admin(AdminPayload::ActionSchedules {
                target: db::action_groups::ActionTargetRef::BotGroup(group.id),
            })
            .to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{}: {}",
                t(ctx.lang, "action_groups.access_button"),
                access_label(ctx.lang, &group.access_scope)
            ),
            Payload::Admin(AdminPayload::ToggleActionGroupAccess { group: group.id }).to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            if group.is_enabled() {
                t(ctx.lang, "action_groups.pause_group")
            } else {
                t(ctx.lang, "action_groups.resume")
            },
            Payload::Admin(AdminPayload::ToggleActionGroupEnabled { group: group.id }).to_string(),
        )]);
        rows.push(vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "action_groups.rename"),
                Payload::Admin(AdminPayload::PromptRenameActionGroup { group: group.id })
                    .to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "action_groups.delete"),
                Payload::Admin(AdminPayload::ConfirmDeleteActionGroup { group: group.id })
                    .to_string(),
            ),
        ]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        if admin {
            Payload::Admin(AdminPayload::ActionGroups)
        } else {
            Payload::Control(ControlPayload::ActionGroups)
        },
    )]);

    let mut lines = vec![
        format!("⚡ {}", group.name),
        String::new(),
        format!(
            "{}: {}",
            t(ctx.lang, "action_groups.state"),
            aggregate_label(ctx.lang, aggregate)
        ),
        format!(
            "{}: {}",
            t(ctx.lang, "action_groups.group_status"),
            enabled_label(ctx.lang, group.is_enabled())
        ),
        format!(
            "{}: {}",
            t(ctx.lang, "action_groups.access"),
            access_label(ctx.lang, &group.access_scope)
        ),
        format!(
            "{}: {}",
            t(ctx.lang, "action_groups.devices"),
            group.items_count
        ),
        format!(
            "{}: {}",
            t(ctx.lang, "action_groups.available_to_you"),
            executable_items_count
        ),
    ];
    if !visible_items.is_empty() {
        lines.push(String::new());
    }
    for item in visible_items.iter().take(12) {
        let state = if item.is_archived() {
            t(ctx.lang, "action_groups.archived").to_string()
        } else {
            state_map
                .get(&item.entity_id)
                .map(|state| state_alias(ctx.lang, state))
                .unwrap_or_else(|| t(ctx.lang, "action_groups.state_unknown").to_string())
        };
        lines.push(format!("{} · {}", item.display_name, state));
    }
    if visible_items.len() > 12 {
        lines.push(format!(
            "{} {}",
            t(ctx.lang, "action_groups.and_more"),
            visible_items.len() - 12
        ));
    }

    Ok(View {
        header: Some(format!("⚡ {}", group.name)),
        notifications: ctx.notifications,
        text: lines.join("\n"),
        kb: InlineKeyboardMarkup::new(rows),
        payload: if admin {
            Payload::Admin(AdminPayload::ActionGroupDetail { group: group.id })
        } else {
            Payload::Control(ControlPayload::ActionGroupDetail { group: group.id })
        },
        ..Default::default()
    })
}

pub async fn render_ha_native_detail(
    ctx: RenderContext,
    target_id: i64,
    admin: bool,
) -> Result<View> {
    let lang = ctx.lang;
    let Some(target) = db::action_groups::get_ha_native_target(target_id, &ctx.config.db).await?
    else {
        let mut view = if admin {
            render_admin_list(ctx).await?
        } else {
            render_user_list(ctx).await?
        };
        view.alert = Some(t(lang, "action_groups.action_not_found").to_string());
        return Ok(view);
    };
    if !ctx.is_admin && (!target.is_visible_to_all_users() || target.is_archived()) {
        let mut view = render_user_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.action_no_access").to_string());
        return Ok(view);
    }
    if !ctx.is_admin && !target.is_enabled() {
        let mut view = render_user_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.action_paused").to_string());
        return Ok(view);
    }

    let mut rows = Vec::new();
    if target.is_enabled() && !target.is_archived() {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "action_groups.execute"),
            if admin {
                Payload::Admin(AdminPayload::ExecuteHaNativeAction { action: target.id })
            } else {
                Payload::Control(ControlPayload::ExecuteHaNativeAction { action: target.id })
            }
            .to_string(),
        )]);
    }

    if admin {
        rows.push(vec![InlineKeyboardButton::callback(
            t(ctx.lang, "action_groups.schedules_button"),
            Payload::Admin(AdminPayload::ActionSchedules {
                target: db::action_groups::ActionTargetRef::HaNative(target.id),
            })
            .to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{}: {}",
                t(ctx.lang, "action_groups.access_button"),
                access_label(ctx.lang, &target.access_scope)
            ),
            Payload::Admin(AdminPayload::ToggleHaNativeActionAccess { action: target.id })
                .to_string(),
        )]);
        rows.push(vec![InlineKeyboardButton::callback(
            if target.is_enabled() {
                t(ctx.lang, "action_groups.pause_action")
            } else {
                t(ctx.lang, "action_groups.resume")
            },
            Payload::Admin(AdminPayload::ToggleHaNativeActionEnabled { action: target.id })
                .to_string(),
        )]);
        rows.push(vec![
            InlineKeyboardButton::callback(
                t(ctx.lang, "action_groups.alias"),
                Payload::Admin(AdminPayload::PromptHaNativeActionAlias { action: target.id })
                    .to_string(),
            ),
            InlineKeyboardButton::callback(
                t(ctx.lang, "action_groups.reset_alias"),
                Payload::Admin(AdminPayload::ResetHaNativeActionAlias { action: target.id })
                    .to_string(),
            ),
        ]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        if admin {
            Payload::Admin(AdminPayload::ActionGroups)
        } else {
            Payload::Control(ControlPayload::ActionGroups)
        },
    )]);

    let text = format!(
        "{} {}\n\n{}: Home Assistant\nEntity: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}",
        ha_icon(&target.domain),
        target.display_name(),
        t(ctx.lang, "action_groups.source"),
        target.entity_id,
        t(ctx.lang, "action_groups.type"),
        target.domain,
        t(ctx.lang, "action_groups.ha_name"),
        target.ha_name,
        t(ctx.lang, "action_groups.bot_alias"),
        target
            .display_name
            .as_deref()
            .unwrap_or_else(|| t(ctx.lang, "common.no")),
        t(ctx.lang, "action_groups.access"),
        access_label(ctx.lang, &target.access_scope),
        t(ctx.lang, "action_groups.status"),
        enabled_label(ctx.lang, target.is_enabled()),
        t(ctx.lang, "action_groups.note"),
        if target.is_archived() {
            t(ctx.lang, "action_groups.not_found_in_ha")
        } else {
            t(ctx.lang, "action_groups.managed_in_ha")
        }
    );

    Ok(View {
        header: Some(format!(
            "{} {}",
            ha_icon(&target.domain),
            target.display_name()
        )),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: if admin {
            Payload::Admin(AdminPayload::HaNativeActionDetail { action: target.id })
        } else {
            Payload::Control(ControlPayload::HaNativeActionDetail { action: target.id })
        },
        ..Default::default()
    })
}

pub async fn render_group_items(
    ctx: RenderContext,
    group_id: i64,
    page: u16,
    filter: db::action_groups::ActionGroupItemsFilter,
) -> Result<View> {
    let lang = ctx.lang;
    let Some(group) = db::action_groups::get_action_group(group_id, &ctx.config.db).await? else {
        let mut view = render_admin_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.group_not_found").to_string());
        return Ok(view);
    };
    let candidates =
        db::action_groups::list_action_group_device_candidates(group_id, &ctx.config.db).await?;
    let candidates_count = candidates.len();
    let selected_count = candidates
        .iter()
        .filter(|candidate| candidate.is_selected())
        .count();
    let filtered = candidates
        .into_iter()
        .filter(|candidate| match filter {
            db::action_groups::ActionGroupItemsFilter::All => true,
            db::action_groups::ActionGroupItemsFilter::Selected => candidate.is_selected(),
            db::action_groups::ActionGroupItemsFilter::Unselected => !candidate.is_selected(),
        })
        .collect::<Vec<_>>();

    let total_pages = filtered.len().div_ceil(ITEMS_PAGE_SIZE).max(1);
    let page = usize::from(page).min(total_pages.saturating_sub(1));
    let slice = filtered
        .iter()
        .skip(page * ITEMS_PAGE_SIZE)
        .take(ITEMS_PAGE_SIZE)
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    for item in &slice {
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {} · {}{}",
                if item.is_selected() { "☑" } else { "☐" },
                item.room_name,
                item.display_name,
                if item.is_archived() {
                    t(ctx.lang, "action_groups.archived_suffix")
                } else {
                    ""
                }
            ),
            Payload::Admin(AdminPayload::ToggleActionGroupItem {
                group: group.id,
                device: item.device_id,
                page: page as u16,
                filter,
            })
            .to_string(),
        )]);
    }

    rows.push(vec![
        filter_button(
            group.id,
            page as u16,
            filter,
            db::action_groups::ActionGroupItemsFilter::All,
            t(ctx.lang, "action_groups.filter_all"),
        ),
        filter_button(
            group.id,
            page as u16,
            filter,
            db::action_groups::ActionGroupItemsFilter::Selected,
            t(ctx.lang, "action_groups.filter_selected"),
        ),
        filter_button(
            group.id,
            page as u16,
            filter,
            db::action_groups::ActionGroupItemsFilter::Unselected,
            t(ctx.lang, "action_groups.filter_unselected"),
        ),
    ]);
    if total_pages > 1 {
        rows.push(vec![
            InlineKeyboardButton::callback(
                "◀️",
                Payload::Admin(AdminPayload::ActionGroupItems {
                    group: group.id,
                    page: page.saturating_sub(1) as u16,
                    filter,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                format!("{}/{}", page + 1, total_pages),
                Payload::Admin(AdminPayload::ActionGroupItems {
                    group: group.id,
                    page: page as u16,
                    filter,
                })
                .to_string(),
            ),
            InlineKeyboardButton::callback(
                "▶️",
                Payload::Admin(AdminPayload::ActionGroupItems {
                    group: group.id,
                    page: (page + 1).min(total_pages - 1) as u16,
                    filter,
                })
                .to_string(),
            ),
        ]);
    }
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::ActionGroupDetail { group: group.id }),
    )]);

    Ok(View {
        header: Some(t(ctx.lang, "action_groups.devices_title").to_string()),
        notifications: ctx.notifications,
        text: format!(
            "{}: {}\n\n{}: {}\n{}: {} / {}\n{}: {} / {}",
            t(ctx.lang, "action_groups.devices_group"),
            group.name,
            t(ctx.lang, "action_groups.filter"),
            filter_label(ctx.lang, filter),
            t(ctx.lang, "action_groups.selected"),
            selected_count,
            candidates_count,
            t(ctx.lang, "action_groups.page"),
            page + 1,
            total_pages
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::ActionGroupItems {
            group: group.id,
            page: page as u16,
            filter,
        }),
        ..Default::default()
    })
}

pub async fn render_schedules(
    ctx: RenderContext,
    target: db::action_groups::ActionTargetRef,
) -> Result<View> {
    let lang = ctx.lang;
    let Some(title) = target_title(&ctx, target).await? else {
        let mut view = render_admin_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.schedule_target_not_found").to_string());
        return Ok(view);
    };
    let can_create_schedule = match target {
        db::action_groups::ActionTargetRef::BotGroup(_) => true,
        db::action_groups::ActionTargetRef::HaNative(target_id) => {
            db::action_groups::get_ha_native_target(target_id, &ctx.config.db)
                .await?
                .is_some_and(|target| !target.is_archived())
        }
    };
    let schedules = db::action_groups::list_action_schedules(target, &ctx.config.db).await?;
    let mut rows = Vec::new();
    if can_create_schedule {
        match target {
            db::action_groups::ActionTargetRef::BotGroup(_) => {
                rows.push(vec![
                    InlineKeyboardButton::callback(
                        t(ctx.lang, "action_groups.add_turn_on_schedule"),
                        Payload::Admin(AdminPayload::PromptCreateActionScheduleTime {
                            target,
                            command: db::action_groups::ActionScheduleCommand::TurnOn,
                        })
                        .to_string(),
                    ),
                    InlineKeyboardButton::callback(
                        t(ctx.lang, "action_groups.add_turn_off_schedule"),
                        Payload::Admin(AdminPayload::PromptCreateActionScheduleTime {
                            target,
                            command: db::action_groups::ActionScheduleCommand::TurnOff,
                        })
                        .to_string(),
                    ),
                ]);
            }
            db::action_groups::ActionTargetRef::HaNative(_) => {
                rows.push(vec![InlineKeyboardButton::callback(
                    t(ctx.lang, "action_groups.add_schedule"),
                    Payload::Admin(AdminPayload::PromptCreateActionScheduleTime {
                        target,
                        command: db::action_groups::ActionScheduleCommand::Execute,
                    })
                    .to_string(),
                )]);
            }
        }
    }

    for schedule in &schedules {
        let command = schedule.command()?;
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{} {} · {} · {}",
                if schedule.is_enabled() { "✅" } else { "⏸" },
                db::action_groups::format_time_minute(schedule.time_minute),
                format_days_mask_lang(ctx.lang, schedule.days_mask),
                schedule_command_label(ctx.lang, command)
            ),
            Payload::Admin(AdminPayload::ActionScheduleDetail {
                target,
                schedule: schedule.id,
            })
            .to_string(),
        )]);
    }

    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        target_detail_payload(target),
    )]);

    Ok(View {
        header: Some(t(ctx.lang, "action_groups.schedules_title").to_string()),
        notifications: ctx.notifications,
        text: format!(
            "{}: {}\n\n{}: {}",
            t(ctx.lang, "action_groups.schedules_plain"),
            title,
            t(ctx.lang, "action_groups.total"),
            schedules.len()
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::ActionSchedules { target }),
        ..Default::default()
    })
}

pub async fn render_schedule_detail(
    ctx: RenderContext,
    target: db::action_groups::ActionTargetRef,
    schedule_id: i64,
) -> Result<View> {
    let lang = ctx.lang;
    let Some(title) = target_title(&ctx, target).await? else {
        let mut view = render_admin_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.schedule_target_not_found").to_string());
        return Ok(view);
    };
    let Some(schedule) =
        db::action_groups::get_action_schedule(schedule_id, &ctx.config.db).await?
    else {
        let mut view = render_schedules(ctx, target).await?;
        view.alert = Some(t(lang, "action_groups.schedule_not_found").to_string());
        return Ok(view);
    };
    if schedule.target_ref()? != target {
        let mut view = render_schedules(ctx, target).await?;
        view.alert = Some(t(lang, "action_groups.schedule_not_found").to_string());
        return Ok(view);
    };
    let command = schedule.command()?;
    let mut rows = vec![vec![InlineKeyboardButton::callback(
        if schedule.is_enabled() {
            t(ctx.lang, "action_groups.disable_schedule")
        } else {
            t(ctx.lang, "action_groups.enable_schedule")
        },
        Payload::Admin(AdminPayload::ToggleActionScheduleEnabled {
            target,
            schedule: schedule.id,
        })
        .to_string(),
    )]];
    rows.push(vec![InlineKeyboardButton::callback(
        t(ctx.lang, "action_groups.edit_time"),
        Payload::Admin(AdminPayload::PromptEditActionScheduleTime {
            target,
            schedule: schedule.id,
        })
        .to_string(),
    )]);
    if matches!(target, db::action_groups::ActionTargetRef::BotGroup(_)) {
        rows.push(vec![InlineKeyboardButton::callback(
            format!(
                "{}: {}",
                t(ctx.lang, "action_groups.command_button"),
                schedule_command_label(ctx.lang, command)
            ),
            Payload::Admin(AdminPayload::CycleActionScheduleCommand {
                target,
                schedule: schedule.id,
            })
            .to_string(),
        )]);
    }
    rows.extend(day_rows(ctx.lang, target, schedule.id, schedule.days_mask));
    rows.push(vec![InlineKeyboardButton::callback(
        t(ctx.lang, "action_groups.delete"),
        Payload::Admin(AdminPayload::ConfirmDeleteActionSchedule {
            target,
            schedule: schedule.id,
        })
        .to_string(),
    )]);
    rows.push(vec![crate::bot::screens::common::back_button_lang(
        ctx.lang,
        Payload::Admin(AdminPayload::ActionSchedules { target }),
    )]);

    Ok(View {
        header: Some(t(ctx.lang, "action_groups.schedule_title").to_string()),
        notifications: ctx.notifications,
        text: format!(
            "{}\n\n{}: {}\n{}: {}\n{}: {}\n{}: {}",
            title,
            t(ctx.lang, "action_groups.command"),
            schedule_command_label(ctx.lang, command),
            t(ctx.lang, "action_groups.time"),
            db::action_groups::format_time_minute(schedule.time_minute),
            t(ctx.lang, "action_groups.days"),
            format_days_mask_lang(ctx.lang, schedule.days_mask),
            t(ctx.lang, "action_groups.status"),
            schedule_enabled_label(ctx.lang, schedule.is_enabled())
        ),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Admin(AdminPayload::ActionScheduleDetail {
            target,
            schedule: schedule.id,
        }),
        ..Default::default()
    })
}

pub fn render_create_group_input(ctx: RenderContext) -> View {
    let lang = ctx.lang;
    crate::bot::screens::admin::list_actions::render_user_input(
        ctx,
        State::AddActionGroup,
        t(lang, "action_groups.create_title"),
        t(lang, "action_groups.create_prompt"),
        Payload::Admin(AdminPayload::PromptCreateActionGroup),
        Payload::Admin(AdminPayload::ActionGroups),
    )
}

pub fn render_rename_group_input(ctx: RenderContext, group_id: i64) -> View {
    let lang = ctx.lang;
    crate::bot::screens::admin::list_actions::render_user_input(
        ctx,
        State::RenameActionGroup { group_id },
        t(lang, "action_groups.rename_title"),
        t(lang, "action_groups.rename_prompt"),
        Payload::Admin(AdminPayload::PromptRenameActionGroup { group: group_id }),
        Payload::Admin(AdminPayload::ActionGroupDetail { group: group_id }),
    )
}

pub fn render_ha_alias_input(ctx: RenderContext, target_id: i64) -> View {
    let lang = ctx.lang;
    crate::bot::screens::admin::list_actions::render_user_input(
        ctx,
        State::RenameHaNativeTarget { target_id },
        t(lang, "action_groups.alias_title"),
        t(lang, "action_groups.alias_prompt"),
        Payload::Admin(AdminPayload::PromptHaNativeActionAlias { action: target_id }),
        Payload::Admin(AdminPayload::HaNativeActionDetail { action: target_id }),
    )
}

pub async fn render_schedule_time_input(
    ctx: RenderContext,
    target: db::action_groups::ActionTargetRef,
    command: db::action_groups::ActionScheduleCommand,
) -> Result<View> {
    let lang = ctx.lang;
    if target_title(&ctx, target).await?.is_none() {
        let mut view = render_admin_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.schedule_target_not_found").to_string());
        return Ok(view);
    }
    if let db::action_groups::ActionTargetRef::HaNative(target_id) = target {
        let Some(native_target) =
            db::action_groups::get_ha_native_target(target_id, &ctx.config.db).await?
        else {
            let mut view = render_admin_list(ctx).await?;
            view.alert = Some(t(lang, "action_groups.schedule_target_not_found").to_string());
            return Ok(view);
        };
        if native_target.is_archived() {
            let mut view = render_schedules(ctx, target).await?;
            view.alert = Some(t(lang, "action_groups.not_found_in_ha").to_string());
            return Ok(view);
        }
    }
    Ok(crate::bot::screens::admin::list_actions::render_user_input(
        ctx,
        State::AddActionScheduleTime { target, command },
        t(lang, "action_groups.schedule_time_title"),
        t(lang, "action_groups.schedule_time_prompt"),
        Payload::Admin(AdminPayload::PromptCreateActionScheduleTime { target, command }),
        Payload::Admin(AdminPayload::ActionSchedules { target }),
    ))
}

pub async fn render_edit_schedule_time_input(
    ctx: RenderContext,
    target: db::action_groups::ActionTargetRef,
    schedule_id: i64,
) -> Result<View> {
    let lang = ctx.lang;
    if target_title(&ctx, target).await?.is_none() {
        let mut view = render_admin_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.schedule_target_not_found").to_string());
        return Ok(view);
    }
    let Some(schedule) =
        db::action_groups::get_action_schedule(schedule_id, &ctx.config.db).await?
    else {
        let mut view = render_schedules(ctx, target).await?;
        view.alert = Some(t(lang, "action_groups.schedule_not_found").to_string());
        return Ok(view);
    };
    if schedule.target_ref()? != target {
        let mut view = render_schedules(ctx, target).await?;
        view.alert = Some(t(lang, "action_groups.schedule_not_found").to_string());
        return Ok(view);
    }
    Ok(crate::bot::screens::admin::list_actions::render_user_input(
        ctx,
        State::EditActionScheduleTime {
            target,
            schedule_id,
        },
        t(lang, "action_groups.schedule_time_title"),
        t(lang, "action_groups.schedule_time_edit_prompt"),
        Payload::Admin(AdminPayload::PromptEditActionScheduleTime {
            target,
            schedule: schedule_id,
        }),
        Payload::Admin(AdminPayload::ActionScheduleDetail {
            target,
            schedule: schedule_id,
        }),
    ))
}

pub async fn render_confirm_delete_group(ctx: RenderContext, group_id: i64) -> Result<View> {
    let lang = ctx.lang;
    let Some(group) = db::action_groups::get_action_group(group_id, &ctx.config.db).await? else {
        let mut view = render_admin_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.group_not_found").to_string());
        return Ok(view);
    };
    Ok(
        crate::bot::screens::admin::list_actions::render_confirm_action(
            ctx,
            t(lang, "action_groups.delete_group_title"),
            &format!(
                "{} \"{}\"?",
                t(lang, "action_groups.delete_group_prompt"),
                group.name
            ),
            Payload::Admin(AdminPayload::DeleteActionGroup { group: group.id }),
            Payload::Admin(AdminPayload::ActionGroupDetail { group: group.id }),
        ),
    )
}

pub async fn render_confirm_delete_schedule(
    ctx: RenderContext,
    target: db::action_groups::ActionTargetRef,
    schedule_id: i64,
) -> Result<View> {
    let lang = ctx.lang;
    if target_title(&ctx, target).await?.is_none() {
        let mut view = render_admin_list(ctx).await?;
        view.alert = Some(t(lang, "action_groups.schedule_target_not_found").to_string());
        return Ok(view);
    }
    let Some(schedule) =
        db::action_groups::get_action_schedule(schedule_id, &ctx.config.db).await?
    else {
        let mut view = render_schedules(ctx, target).await?;
        view.alert = Some(t(lang, "action_groups.schedule_not_found").to_string());
        return Ok(view);
    };
    if schedule.target_ref()? != target {
        let mut view = render_schedules(ctx, target).await?;
        view.alert = Some(t(lang, "action_groups.schedule_not_found").to_string());
        return Ok(view);
    }
    Ok(
        crate::bot::screens::admin::list_actions::render_confirm_action(
            ctx,
            t(lang, "action_groups.delete_schedule_title"),
            t(lang, "action_groups.delete_schedule_prompt"),
            Payload::Admin(AdminPayload::DeleteActionSchedule {
                target,
                schedule: schedule_id,
            }),
            Payload::Admin(AdminPayload::ActionScheduleDetail {
                target,
                schedule: schedule_id,
            }),
        ),
    )
}

async fn target_title(
    ctx: &RenderContext,
    target: db::action_groups::ActionTargetRef,
) -> Result<Option<String>> {
    match target {
        db::action_groups::ActionTargetRef::BotGroup(group_id) => {
            let group = db::action_groups::get_action_group(group_id, &ctx.config.db).await?;
            Ok(group.map(|group| group.name))
        }
        db::action_groups::ActionTargetRef::HaNative(target_id) => {
            let target = db::action_groups::get_ha_native_target(target_id, &ctx.config.db).await?;
            Ok(target.map(|target| target.display_name().to_string()))
        }
    }
}

fn target_detail_payload(target: db::action_groups::ActionTargetRef) -> Payload {
    match target {
        db::action_groups::ActionTargetRef::BotGroup(group) => {
            Payload::Admin(AdminPayload::ActionGroupDetail { group })
        }
        db::action_groups::ActionTargetRef::HaNative(action) => {
            Payload::Admin(AdminPayload::HaNativeActionDetail { action })
        }
    }
}

fn execute_group_payload(
    admin: bool,
    group: i64,
    command: db::action_groups::ActionGroupCommand,
) -> Payload {
    if admin {
        Payload::Admin(AdminPayload::ExecuteActionGroup { group, command })
    } else {
        Payload::Control(ControlPayload::ExecuteActionGroup { group, command })
    }
}

fn dynamic_button_label(
    lang: Language,
    command: db::action_groups::ActionGroupCommand,
) -> &'static str {
    match command {
        db::action_groups::ActionGroupCommand::TurnOff => t(lang, "action_groups.dynamic_turn_off"),
        _ => t(lang, "action_groups.dynamic_turn_on"),
    }
}

fn day_rows(
    lang: Language,
    target: db::action_groups::ActionTargetRef,
    schedule_id: i64,
    days_mask: i64,
) -> Vec<Vec<InlineKeyboardButton>> {
    let labels = match lang {
        Language::Ru => ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"],
        Language::En => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    };
    labels
        .chunks(3)
        .enumerate()
        .map(|(chunk_index, chunk)| {
            chunk
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    let day = (chunk_index * 3 + index) as u8;
                    let checked = (days_mask & (1_i64 << day)) != 0;
                    InlineKeyboardButton::callback(
                        format!("{} {}", if checked { "☑" } else { "☐" }, label),
                        Payload::Admin(AdminPayload::ToggleActionScheduleDay {
                            target,
                            schedule: schedule_id,
                            day,
                        })
                        .to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

fn filter_button(
    group: i64,
    page: u16,
    current: db::action_groups::ActionGroupItemsFilter,
    filter: db::action_groups::ActionGroupItemsFilter,
    label: &str,
) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(
        if current == filter {
            format!("• {}", label)
        } else {
            label.to_string()
        },
        Payload::Admin(AdminPayload::ActionGroupItems {
            group,
            page,
            filter,
        })
        .to_string(),
    )
}

fn filter_label(lang: Language, filter: db::action_groups::ActionGroupItemsFilter) -> &'static str {
    match filter {
        db::action_groups::ActionGroupItemsFilter::All => t(lang, "action_groups.filter_all_plain"),
        db::action_groups::ActionGroupItemsFilter::Selected => {
            t(lang, "action_groups.filter_selected_plain")
        }
        db::action_groups::ActionGroupItemsFilter::Unselected => {
            t(lang, "action_groups.filter_unselected_plain")
        }
    }
}

fn access_label(lang: Language, access_scope: &str) -> &'static str {
    if access_scope == db::action_groups::ACCESS_ALL_USERS {
        t(lang, "action_groups.access_all")
    } else {
        t(lang, "action_groups.access_admin")
    }
}

fn aggregate_label(
    lang: Language,
    state: crate::core::action_groups::AggregateState,
) -> &'static str {
    match state {
        crate::core::action_groups::AggregateState::AllOn => {
            t(lang, "action_groups.aggregate_all_on")
        }
        crate::core::action_groups::AggregateState::AllOff => {
            t(lang, "action_groups.aggregate_all_off")
        }
        crate::core::action_groups::AggregateState::Mixed => {
            t(lang, "action_groups.aggregate_mixed")
        }
        crate::core::action_groups::AggregateState::Unknown => {
            t(lang, "action_groups.aggregate_unknown")
        }
    }
}

fn enabled_label(lang: Language, enabled: bool) -> &'static str {
    if enabled {
        t(lang, "action_groups.enabled")
    } else {
        t(lang, "action_groups.paused")
    }
}

fn schedule_enabled_label(lang: Language, enabled: bool) -> &'static str {
    if enabled {
        t(lang, "action_groups.schedule_enabled")
    } else {
        t(lang, "action_groups.schedule_disabled")
    }
}

fn schedule_command_label(
    lang: Language,
    command: db::action_groups::ActionScheduleCommand,
) -> &'static str {
    match command {
        db::action_groups::ActionScheduleCommand::TurnOn => {
            t(lang, "action_groups.command_turn_on")
        }
        db::action_groups::ActionScheduleCommand::TurnOff => {
            t(lang, "action_groups.command_turn_off")
        }
        db::action_groups::ActionScheduleCommand::Execute => {
            t(lang, "action_groups.command_execute")
        }
    }
}

fn format_days_mask_lang(lang: Language, mask: i64) -> &'static str {
    match (lang, mask) {
        (Language::Ru, db::action_groups::ALL_DAYS_MASK) => "ежедневно",
        (Language::En, db::action_groups::ALL_DAYS_MASK) => "daily",
        (Language::Ru, 0b001_1111) => "Пн-Пт",
        (Language::En, 0b001_1111) => "Mon-Fri",
        (Language::Ru, 0b110_0000) => "Сб-Вс",
        (Language::En, 0b110_0000) => "Sat-Sun",
        (Language::Ru, _) => "выбранные дни",
        (Language::En, _) => "selected days",
    }
}

fn ha_icon(domain: &str) -> &'static str {
    match domain {
        "scene" => "🎬",
        "script" => "▶️",
        _ => "⚡",
    }
}

fn state_alias(lang: Language, state: &str) -> String {
    match state {
        "on" => t(lang, "action_groups.state_on").to_string(),
        "off" => t(lang, "action_groups.state_off").to_string(),
        "unavailable" => t(lang, "action_groups.state_unavailable").to_string(),
        "unknown" => t(lang, "action_groups.state_unknown").to_string(),
        _ => state.to_string(),
    }
}
