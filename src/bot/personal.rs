use super::{
    models::View,
    router::{CameraPayload, ControlPayload, DeviceCmd, Payload, RenderContext, State},
};
use crate::{
    db,
    i18n::{t, Language},
    models::AppConfig,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton as Button, InlineKeyboardMarkup, MessageId},
};

pub fn words<'a>(lang: Language, ru: &'a str, en: &'a str) -> &'a str {
    if lang == Language::Ru {
        ru
    } else {
        en
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum FavoriteAction {
    Device { room: i64, device: i64 },
    DeviceOn { room: i64, device: i64 },
    DeviceOff { room: i64, device: i64 },
    Camera { id: i64 },
    Snapshot { id: i64 },
    GroupOn { id: i64 },
    GroupOff { id: i64 },
    Native { id: i64 },
}

impl FavoriteAction {
    fn payload(&self) -> Payload {
        match *self {
            Self::Device { room, device } => {
                Payload::Control(ControlPayload::DeviceControl { room, device })
            }
            Self::DeviceOn { room, device } => Payload::Control(ControlPayload::QuickAction {
                room,
                device,
                cmd: DeviceCmd::TurnOn,
            }),
            Self::DeviceOff { room, device } => Payload::Control(ControlPayload::QuickAction {
                room,
                device,
                cmd: DeviceCmd::TurnOff,
            }),
            Self::Camera { id } => Payload::Camera(CameraPayload::CameraDetail { id }),
            Self::Snapshot { id } => Payload::Camera(CameraPayload::Snapshot { id }),
            Self::GroupOn { id } => Payload::Control(ControlPayload::ExecuteActionGroup {
                group: id,
                command: db::action_groups::ActionGroupCommand::TurnOn,
            }),
            Self::GroupOff { id } => Payload::Control(ControlPayload::ExecuteActionGroup {
                group: id,
                command: db::action_groups::ActionGroupCommand::TurnOff,
            }),
            Self::Native { id } => {
                Payload::Control(ControlPayload::ExecuteHaNativeAction { action: id })
            }
        }
    }

    async fn canonical(&self, ctx: &RenderContext) -> Result<Self> {
        match *self {
            Self::Device { device, .. }
            | Self::DeviceOn { device, .. }
            | Self::DeviceOff { device, .. } => {
                let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                    .await?
                    .context("Device unavailable")?;
                let room = db::devices::get_room_id_by_entity(&dev.entity_id, &ctx.config.db)
                    .await?
                    .context("Room unavailable")?;
                Ok(match self {
                    Self::DeviceOn { .. } => Self::DeviceOn { room, device },
                    Self::DeviceOff { .. } => Self::DeviceOff { room, device },
                    _ => Self::Device { room, device },
                })
            }
            _ => Ok(self.clone()),
        }
    }

    async fn label(&self, ctx: &RenderContext) -> Result<Option<String>> {
        let pool = &ctx.config.db;
        Ok(match *self {
            Self::Device { device, .. }
            | Self::DeviceOn { device, .. }
            | Self::DeviceOff { device, .. } => {
                if !db::access::can_view_device(ctx.user_id, ctx.is_admin, device, pool).await? {
                    return Ok(None);
                }
                let row: Option<(String, String, String, i64)> = sqlx::query_as("SELECT COALESCE(NULLIF(d.alias, ''), NULLIF(d.ha_name, ''), d.entity_id), COALESCE(NULLIF(r.alias, ''), r.area), d.entity_id, r.id FROM devices d JOIN rooms r ON r.id = d.room_id WHERE d.id = ? AND d.archived = 0")
                    .bind(device).fetch_optional(pool).await?;
                let Some((name, room, entity, room_id)) = row else {
                    return Ok(None);
                };
                ctx.track_device(device, &entity);
                ctx.track_room(room_id);
                let action = match self {
                    Self::DeviceOn { .. } | Self::DeviceOff { .. } => {
                        if !matches!(entity.split('.').next(), Some("light" | "switch"))
                            || !db::access::can_control_device(
                                ctx.user_id,
                                ctx.is_admin,
                                device,
                                pool,
                            )
                            .await?
                        {
                            return Ok(None);
                        }
                        if matches!(self, Self::DeviceOn { .. }) {
                            words(ctx.lang, "Включить", "Turn on")
                        } else {
                            words(ctx.lang, "Выключить", "Turn off")
                        }
                    }
                    _ => words(ctx.lang, "Открыть", "Open"),
                };
                Some(format!("{room} · {name} · {action} #{device}"))
            }
            Self::Camera { id } | Self::Snapshot { id } => {
                match db::cameras::get_accessible_camera(ctx.user_id, ctx.is_admin, id, pool)
                    .await?
                {
                    Some(camera) => {
                        ctx.track_camera(camera.id);
                        let room = db::rooms::get_room_by_id(camera.room_id.unwrap_or(0), pool)
                            .await?
                            .map(|r| {
                                ctx.track_room(r.id);
                                r.display_name().to_owned()
                            })
                            .unwrap_or_default();
                        let action = if matches!(self, Self::Snapshot { .. }) {
                            words(ctx.lang, "Снимок", "Snapshot")
                        } else {
                            words(ctx.lang, "Открыть", "Open")
                        };
                        Some(format!("{room} · {} · {action} #{id}", camera.name))
                    }
                    None => None,
                }
            }
            Self::GroupOn { id } | Self::GroupOff { id } => {
                db::action_groups::get_action_group(id, pool)
                    .await?
                    .filter(|g| ctx.is_admin || (g.is_enabled() && g.is_visible_to_all_users()))
                    .map(|g| {
                        ctx.track_group(g.id);
                        format!(
                            "{} · {} #{id}",
                            g.name,
                            if matches!(self, Self::GroupOn { .. }) {
                                words(ctx.lang, "Включить", "Turn on")
                            } else {
                                words(ctx.lang, "Выключить", "Turn off")
                            }
                        )
                    })
            }
            Self::Native { id } => db::action_groups::get_ha_native_target(id, pool)
                .await?
                .filter(|g| {
                    !g.is_archived()
                        && (ctx.is_admin || (g.is_enabled() && g.is_visible_to_all_users()))
                })
                .map(|g| {
                    ctx.track_native(g.id, &g.entity_id);
                    format!(
                        "{} · {} #{id}",
                        g.display_name(),
                        words(ctx.lang, "Запустить", "Run")
                    )
                }),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PersonalPayload {
    Settings,
    Language { english: bool },
    Quiet { enabled: bool },
    QuietInput,
    Notifications { page: u16 },
    Subscription { device: i64 },
    SetSubscription { device: i64, enabled: bool },
    Favorites { page: u16 },
    FavoriteOptions { target: FavoriteAction },
    AddFavorite { action: FavoriteAction },
    RemoveFavorite { id: i64, page: u16 },
    FirstFavorite { id: i64, page: u16 },
    // Append only: postcard enum positions are encoded in existing Telegram buttons.
    Interface,
    SetInterface { mode: db::interface::InterfaceMode },
}
impl PersonalPayload {
    pub fn for_refresh(&self) -> Self {
        match self {
            Self::Language { .. } | Self::Quiet { .. } => Self::Settings,
            Self::SetInterface { .. } => Self::Interface,
            Self::SetSubscription { device, .. } => Self::Subscription { device: *device },
            Self::AddFavorite { .. } => Self::Favorites { page: 0 },
            Self::RemoveFavorite { page, .. } | Self::FirstFavorite { page, .. } => {
                Self::Favorites { page: *page }
            }
            _ => self.clone(),
        }
    }
}
fn button(label: impl Into<String>, payload: PersonalPayload) -> Button {
    Button::callback(label, Payload::Personal(payload).to_string())
}
fn html_label(
    labels: &mut std::collections::HashMap<String, String>,
    button: &Button,
    label: String,
) {
    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(callback) = &button.kind {
        labels.insert(callback.clone(), label);
    }
}
fn favorite_management_context(context: &str) -> String {
    // Two controls share one wire row. Reserve room for both localized verbs,
    // the page header and callback keys; full labels remain in the continuation.
    short_favorite_context(context, 100)
}
fn short_favorite_context(context: &str, bytes: usize) -> String {
    if context.len() <= bytes {
        return context.into();
    }
    let mut short = String::new();
    for ch in context.chars() {
        if short.len() + ch.len_utf8() > bytes.saturating_sub('…'.len_utf8()) {
            break;
        }
        short.push(ch);
    }
    short.push('…');
    short
}
fn favorite_verb(action: &FavoriteAction, button: &Button, lang: Language) -> &'static str {
    // An unavailable command is deliberately replaced with a safe detail link.
    // Confirmation callbacks, however, keep the original typed action's verb.
    if matches!(&button.kind,
        teloxide::types::InlineKeyboardButtonKind::CallbackData(data)
        if matches!(Payload::from_string(data), Ok(Payload::Control(
            ControlPayload::DeviceControl { .. } | ControlPayload::ActionGroupDetail { .. }
            | ControlPayload::HaNativeActionDetail { .. }
        ) | Payload::Camera(CameraPayload::CameraDetail { .. }))))
    {
        return words(lang, "Открыть", "Open");
    }
    match action {
        FavoriteAction::DeviceOn { .. } | FavoriteAction::GroupOn { .. } => {
            words(lang, "Включить", "Turn on")
        }
        FavoriteAction::DeviceOff { .. } | FavoriteAction::GroupOff { .. } => {
            words(lang, "Выключить", "Turn off")
        }
        FavoriteAction::Snapshot { .. } => words(lang, "Снимок", "Snapshot"),
        FavoriteAction::Native { .. } => words(lang, "Запустить", "Run"),
        _ => words(lang, "Открыть", "Open"),
    }
}
pub(crate) fn favorite_html_label(
    action: &FavoriteAction,
    button: &Button,
    lang: Language,
    number: usize,
) -> String {
    let object = button.text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{} · {number}\n{}",
        favorite_verb(action, button, lang),
        short_favorite_context(&object, 200)
    )
}
fn base(ctx: &RenderContext, text: String, payload: Payload, mut rows: Vec<Vec<Button>>) -> View {
    rows.push(vec![super::screens::common::main_menu_button_lang(
        ctx.lang,
    )]);
    View {
        text,
        payload,
        kb: InlineKeyboardMarkup::new(rows),
        lang: ctx.lang,
        compact: true,
        notifications: ctx.notifications.clone(),
        ..Default::default()
    }
}

/// Build operations only for the visible page, after ACL filtering and clamping.
pub async fn favorite_buttons(
    ctx: &RenderContext,
    requested_page: usize,
    page_size: usize,
) -> Result<(Vec<(i64, FavoriteAction, Button)>, usize, usize)> {
    let mut entries = Vec::new();
    for favorite in db::personal::favorites(ctx.user_id, &ctx.config.db).await? {
        let Ok(action) = serde_json::from_str::<FavoriteAction>(&favorite.action) else {
            continue;
        };
        if let Some(label) = action.label(ctx).await? {
            entries.push((favorite.id, action, label));
        }
    }
    let page_size = page_size.max(1);
    let pages = entries.len().div_ceil(page_size).max(1);
    let page = requested_page.min(pages - 1);
    let visible = entries
        .into_iter()
        .skip(page * page_size)
        .take(page_size)
        .collect::<Vec<_>>();
    let mut device_entities = std::collections::HashMap::new();
    for (_, action, _) in &visible {
        if let FavoriteAction::Device { device, .. }
        | FavoriteAction::DeviceOn { device, .. }
        | FavoriteAction::DeviceOff { device, .. } = action
        {
            if let Some(device_row) = db::devices::get_device_by_id(*device, &ctx.config.db).await?
            {
                ctx.track_device(device_row.id, &device_row.entity_id);
                device_entities.insert(*device, device_row.entity_id);
            }
        }
    }
    let states = if device_entities.is_empty() {
        Vec::new()
    } else {
        let entity_ids = device_entities.values().cloned().collect::<Vec<_>>();
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            ctx.config.ha_client.fetch_states_by_ids(&entity_ids),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
    };
    let available = states
        .into_iter()
        .filter(|state| !matches!(state.state.as_str(), "unknown" | "unavailable"))
        .map(|state| state.entity_id)
        .collect::<std::collections::HashSet<_>>();
    let mut buttons = Vec::new();
    for (id, action, mut label) in visible {
        let unavailable = match &action {
            FavoriteAction::Device { device, .. }
            | FavoriteAction::DeviceOn { device, .. }
            | FavoriteAction::DeviceOff { device, .. } => device_entities
                .get(device)
                .is_none_or(|entity| !available.contains(entity)),
            _ => false,
        };
        let favorite = action.canonical(ctx).await?;
        let action = favorite.payload();
        let payload = if unavailable {
            label.push_str(words(ctx.lang, " · HA недоступен", " · HA unavailable"));
            action.for_refresh()
        } else {
            match super::confirmations::prepare_button(ctx, &action).await {
                Ok(payload) => payload,
                Err(_) => {
                    label.push_str(words(ctx.lang, " · Недоступно", " · Unavailable"));
                    action.for_refresh()
                }
            }
        };
        buttons.push((id, favorite, Button::callback(label, payload.to_string())));
    }
    Ok((buttons, page, pages))
}

pub async fn render(ctx: RenderContext, request: PersonalPayload) -> Result<View> {
    let pool = &ctx.config.db;
    if ctx.is_admin {
        db::add_user(ctx.user_id, pool).await?;
    }
    let mut notice = None;
    let mut alert = None;
    {
        let _lifecycle = ctx.config.admission_lifecycle.lock().await;
        anyhow::ensure!(
            db::user_exists(ctx.user_id, pool).await,
            "User admission revoked"
        );
        match &request {
        PersonalPayload::Language { english } => db::set_user_language(ctx.user_id, if *english { Language::En } else { Language::Ru }, pool).await?,
        PersonalPayload::SetInterface { mode } => {
            if *mode == db::interface::InterfaceMode::Classic || ctx.config.html_ui.enabled {
                db::interface::set(ctx.user_id, *mode, pool).await?;
            } else {
                alert = Some(words(ctx.lang, "HTML-интерфейс ещё не включён владельцем бота.", "HTML interface has not been enabled by the bot owner.").into());
            }
        },
        PersonalPayload::Quiet { enabled } => db::personal::set_quiet(ctx.user_id, *enabled, pool).await?,
        PersonalPayload::SetSubscription { device, enabled } => {
            match db::personal::set_subscription(ctx.user_id, *device, *enabled, ctx.is_admin, pool).await {
                Ok(()) => notice = Some(words(ctx.lang, "Настройка уведомлений сохранена", "Notification preference saved").into()),
                Err(_) => alert = Some(words(ctx.lang, "Уведомления недоступны: проверьте права просмотра и разрешение администратора", "Notifications unavailable: check viewing and administrator permissions").into()),
            }
        },
        PersonalPayload::AddFavorite { action } => {
            if action.label(&ctx).await?.is_some() { db::personal::add_favorite(ctx.user_id, &serde_json::to_string(&action.canonical(&ctx).await?)?, pool).await?; }
            else { alert = Some(t(ctx.lang, "access.denied.text").into()); }
        },
        PersonalPayload::RemoveFavorite { id, .. } => db::personal::remove_favorite(ctx.user_id, *id, pool).await?,
        PersonalPayload::FirstFavorite { id, .. } => db::personal::move_favorite_first(ctx.user_id, *id, pool).await?,
        _ => {},
    }
    }
    let request = request.for_refresh();
    let mut ctx = ctx;
    ctx.lang = db::get_user_language(ctx.user_id, &ctx.config.db)
        .await?
        .unwrap_or(ctx.lang);
    let w = |ru, en| words(ctx.lang, ru, en);
    let mut rows = Vec::new();
    let mut html_button_labels = std::collections::HashMap::new();
    let mut favorite_descriptions = Vec::new();
    let mut next_state = None;
    let text = match &request {
        PersonalPayload::Interface => {
            let mode = db::interface::get(ctx.user_id, &ctx.config.db).await?;
            for (value, name) in [
                (
                    db::interface::InterfaceMode::Classic,
                    w("Обычный", "Classic"),
                ),
                (db::interface::InterfaceMode::Html, "HTML"),
            ] {
                if value == db::interface::InterfaceMode::Classic || ctx.config.html_ui.enabled {
                    rows.push(vec![button(
                        format!("{}{}", if mode == value { "✓ " } else { "" }, name),
                        PersonalPayload::SetInterface { mode: value },
                    )]);
                }
            }
            rows.push(vec![button(
                t(ctx.lang, "common.back"),
                PersonalPayload::Settings,
            )]);
            w("Вид интерфейса\n\nВыбор сохраняется для вашего аккаунта. Разделы, ещё не перенесённые в HTML, открываются обычным видом. /classic — вернуться к кнопкам Telegram.", "Interface\n\nYour preference is saved. Sections without HTML support open in the classic view. /classic returns to Telegram buttons.").into()
        }
        PersonalPayload::QuietInput => {
            let schedule =
                db::user_notification_schedule::get(ctx.user_id as i64, &ctx.config.db).await?;
            let from = schedule
                .as_ref()
                .and_then(|s| s.quiet_from.as_deref())
                .unwrap_or("23:00");
            let to = schedule
                .as_ref()
                .and_then(|s| s.quiet_to.as_deref())
                .unwrap_or("07:00");
            let zone = schedule
                .as_ref()
                .and_then(|s| s.timezone.as_deref())
                .unwrap_or(w("Время сервера", "Server time"));
            rows.push(vec![button(
                t(ctx.lang, "common.cancel"),
                PersonalPayload::Settings,
            )]);
            next_state = Some(State::PersonalQuietHours);
            format!(
                "{}: {from}–{to}\n{zone}\n\n{}",
                w("Текущий интервал", "Current interval"),
                w(
                    "Отправьте в чат HH:MM–HH:MM, например 23:00–07:00.\nСохранение интервала не включает тихие часы.",
                    "Send HH:MM–HH:MM in chat, e.g. 23:00–07:00.\nSaving an interval does not enable quiet hours."
                )
            )
        }
        PersonalPayload::Settings => {
            let schedule =
                db::user_notification_schedule::get(ctx.user_id as i64, &ctx.config.db).await?;
            let enabled = schedule.as_ref().is_some_and(|s| s.quiet_enabled != 0);
            let from = schedule
                .as_ref()
                .and_then(|s| s.quiet_from.as_deref())
                .unwrap_or("23:00");
            let to = schedule
                .as_ref()
                .and_then(|s| s.quiet_to.as_deref())
                .unwrap_or("07:00");
            let zone = schedule
                .as_ref()
                .and_then(|s| s.timezone.as_deref())
                .unwrap_or(w("Время сервера", "Server time"));
            rows.push(vec![
                button(
                    if ctx.lang == Language::Ru {
                        "✓ Русский"
                    } else {
                        "Русский"
                    },
                    PersonalPayload::Language { english: false },
                ),
                button(
                    if ctx.lang == Language::En {
                        "✓ English"
                    } else {
                        "English"
                    },
                    PersonalPayload::Language { english: true },
                ),
            ]);
            rows.insert(
                0,
                vec![button(
                    w("Вид интерфейса", "Interface"),
                    PersonalPayload::Interface,
                )],
            );
            rows.push(vec![button(
                if enabled {
                    w("Выключить тихие часы", "Disable quiet hours")
                } else {
                    w("Включить тихие часы", "Enable quiet hours")
                },
                PersonalPayload::Quiet { enabled: !enabled },
            )]);
            rows.push(vec![button(
                w("Изменить интервал", "Change interval"),
                PersonalPayload::QuietInput,
            )]);
            rows.push(vec![button(
                w("Мои уведомления", "My notifications"),
                PersonalPayload::Notifications { page: 0 },
            )]);
            rows.push(vec![button(
                w("Всё избранное", "All favorites"),
                PersonalPayload::Favorites { page: 0 },
            )]);
            format!(
                "{}\n\n{}: {}\n{from}–{to}\n{zone}\n{}",
                w("Мои настройки", "My settings"),
                w("Тихие часы", "Quiet hours"),
                if enabled {
                    w("Включены", "Enabled")
                } else {
                    w("Выключены", "Disabled")
                },
                w(
                    "Критические уведомления приходят и в тихие часы.",
                    "Critical notifications are delivered during quiet hours."
                )
            )
        }
        PersonalPayload::Notifications { page } => {
            let rooms =
                db::rooms::get_rooms_for_user(ctx.user_id, ctx.is_admin, &ctx.config.db).await?;
            let mut devices = Vec::new();
            for room in rooms {
                for device in db::devices::get_devices_by_room_for_user(
                    ctx.user_id,
                    ctx.is_admin,
                    room.id,
                    &ctx.config.db,
                )
                .await?
                {
                    ctx.track_device(device.id, &device.entity_id);
                    ctx.track_room(room.id);
                    devices.push((
                        device.id,
                        format!(
                            "{} · {} #{}",
                            room.display_name(),
                            device.alias.as_deref().unwrap_or(&device.entity_id),
                            device.id
                        ),
                    ));
                }
            }
            let pages = devices.len().div_ceil(10).max(1);
            let page = usize::from(*page).min(pages - 1);
            rows.extend(
                devices
                    .into_iter()
                    .skip(page * 10)
                    .take(10)
                    .map(|(device, label)| {
                        vec![button(label, PersonalPayload::Subscription { device })]
                    }),
            );
            rows.push(super::screens::common::page_row(page, pages, |page| {
                Payload::Personal(PersonalPayload::Notifications { page })
            }));
            rows.push(vec![button(
                t(ctx.lang, "common.back"),
                PersonalPayload::Settings,
            )]);
            w(
                "Мои уведомления · выберите устройство",
                "My notifications · choose a device",
            )
            .into()
        }
        PersonalPayload::Subscription { device } => {
            if !db::access::can_view_device(ctx.user_id, ctx.is_admin, *device, &ctx.config.db)
                .await?
            {
                return Ok(base(
                    &ctx,
                    w("Устройство недоступно", "Device unavailable").into(),
                    Payload::Home,
                    rows,
                ));
            }
            let dev = db::devices::get_device_by_id(*device, &ctx.config.db)
                .await?
                .context("Device unavailable")?;
            ctx.track_device(dev.id, &dev.entity_id);
            let subscribed = db::subscriptions::is_subscribed(
                ctx.user_id as i64,
                &dev.entity_id,
                &ctx.config.db,
            )
            .await?;
            let allowed = db::access::can_notify_entity_for_device_id(
                ctx.user_id,
                ctx.is_admin,
                *device,
                &ctx.config.db,
            )
            .await?;
            if subscribed || allowed {
                rows.push(vec![button(
                    if subscribed {
                        w("Выключить уведомления", "Disable notifications")
                    } else {
                        w("Включить уведомления", "Enable notifications")
                    },
                    PersonalPayload::SetSubscription {
                        device: *device,
                        enabled: !subscribed,
                    },
                )]);
            }
            rows.push(vec![button(
                t(ctx.lang, "common.back"),
                PersonalPayload::Notifications { page: 0 },
            )]);
            let state = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                ctx.config
                    .ha_client
                    .fetch_states_by_ids(std::slice::from_ref(&dev.entity_id)),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|states| states.into_iter().next());
            let status = if let Some(state) = state {
                let inverted =
                    db::devices::is_state_inverted(&dev.entity_id, &ctx.config.db).await?;
                let alias =
                    ctx.config
                        .state_alias_for_display(&dev.entity_id, &state.state, inverted);
                super::screens::control::labels::state(&state, ctx.lang, inverted, alias.as_deref())
            } else {
                w("Состояние недоступно", "State unavailable").into()
            };
            format!(
                "{}\n{}: {status}\n{}: {}\n{}",
                dev.alias.as_deref().unwrap_or(&dev.entity_id),
                w("Состояние устройства", "Device state"),
                w("Уведомления", "Notifications"),
                if subscribed {
                    w("Включены", "Enabled")
                } else {
                    w("Выключены", "Disabled")
                },
                if allowed {
                    w(
                        "Настройка сохраняется независимо от подключения к HA.",
                        "Preference is saved independently of the HA connection.",
                    )
                } else {
                    w(
                        "Администратор запретил уведомления.",
                        "Notifications disabled by administrator.",
                    )
                }
            )
        }
        PersonalPayload::Favorites { page } => {
            let (items, page, pages) = favorite_buttons(&ctx, usize::from(*page), 10).await?;
            for (index, (id, favorite, action)) in items.into_iter().enumerate() {
                // A byte/height page break can separate the management row from
                // its action. Repeat the identity with the same visible
                // position, keeping that position first when a label is clipped.
                let number = page * 10 + index + 1;
                let object = action.text.split_whitespace().collect::<Vec<_>>().join(" ");
                let context = format!("{number}. {object}");
                html_label(
                    &mut html_button_labels,
                    &action,
                    favorite_html_label(&favorite, &action, ctx.lang, number),
                );
                favorite_descriptions.push(format!("{number}. {}", action.text));
                let management_context = favorite_management_context(&context);
                rows.push(vec![action]);
                let controls = vec![
                    button(
                        w("На первое место", "Move first"),
                        PersonalPayload::FirstFavorite {
                            id,
                            page: page as u16,
                        },
                    ),
                    button(
                        w("Удалить из избранного", "Remove favorite"),
                        PersonalPayload::RemoveFavorite {
                            id,
                            page: page as u16,
                        },
                    ),
                ];
                for control in &controls {
                    html_label(
                        &mut html_button_labels,
                        control,
                        format!("{}\n{management_context}", control.text),
                    );
                }
                rows.push(controls);
            }
            rows.push(super::screens::common::page_row(page, pages, |page| {
                Payload::Personal(PersonalPayload::Favorites { page })
            }));
            w(
                "Избранное · добавляйте действия из карточек устройств, камер и сценариев.",
                "Favorites · add actions from device, camera and scenario cards.",
            )
            .into()
        }
        PersonalPayload::FavoriteOptions { target } => {
            if target.label(&ctx).await?.is_none() {
                return Ok(base(
                    &ctx,
                    w("Объект недоступен", "Target unavailable").into(),
                    Payload::Home,
                    rows,
                ));
            }
            let choices = match *target {
                FavoriteAction::Device { room, device } => vec![
                    target.clone(),
                    FavoriteAction::DeviceOn { room, device },
                    FavoriteAction::DeviceOff { room, device },
                ],
                FavoriteAction::Camera { id } => {
                    vec![target.clone(), FavoriteAction::Snapshot { id }]
                }
                FavoriteAction::GroupOn { id } => {
                    vec![target.clone(), FavoriteAction::GroupOff { id }]
                }
                _ => vec![target.clone()],
            };
            for action in choices {
                if let Some(label) = action.label(&ctx).await? {
                    let number = favorite_descriptions.len() + 1;
                    favorite_descriptions.push(format!("{number}. {label}"));
                    let choice = button(
                        label,
                        PersonalPayload::AddFavorite {
                            action: action.clone(),
                        },
                    );
                    html_label(
                        &mut html_button_labels,
                        &choice,
                        favorite_html_label(&action, &choice, ctx.lang, number),
                    );
                    rows.push(vec![choice]);
                }
            }
            w(
                "Добавить в избранное · выберите действие. Добавление не отправляет команду.",
                "Add to favorites · choose an action. Adding does not send a command.",
            )
            .into()
        }
        _ => unreachable!("mutations normalize to read routes"),
    };
    let quiet_input = matches!(request, PersonalPayload::QuietInput);
    let mut view = base(&ctx, text, Payload::Personal(request), rows);
    if !favorite_descriptions.is_empty() {
        view.html_summary = Some(view.text.clone());
        view.html_details_title = Some(
            if matches!(
                view.payload,
                Payload::Personal(PersonalPayload::FavoriteOptions { .. })
            ) {
                w("Полные названия действий", "Full action descriptions")
            } else {
                w("Полные названия избранного", "Full favorite descriptions")
            }
            .into(),
        );
        view.text
            .push_str(&format!("\n\n{}", favorite_descriptions.join("\n\n")));
    }
    if quiet_input {
        let title = w("Интервал тихих часов", "Quiet hours interval").to_owned();
        view.header = Some(title.clone());
        view.html_title = Some(title);
        view.html_context = Some(w("Мои настройки", "My settings").into());
        view.html_kind = Some("form".into());
        view.html_back = Some(Payload::Personal(PersonalPayload::Settings));
    }
    view.next_state = next_state;
    view.html_button_labels = html_button_labels;
    view.notice = notice;
    view.alert = alert;
    Ok(view)
}

pub async fn help(ctx: RenderContext) -> Result<View> {
    let voice = ctx.config.voice_enabled
        && db::access::can_use_voice(ctx.user_id, ctx.is_admin, &ctx.config.db).await?;
    let text = words(ctx.lang,
        "Помощь\n\nКомнаты — устройства и графики. Сценарии — группы действий и сценарии HA.\nТекст: «включи свет в кухне», «снимок вход».\nГолос: «выключи свет в спальне».\n/help — помощь; /start — главное меню; /cancel — отменить ввод текущего поля. Сохранённые поля черновика сохраняются. Принятая команда не отменяется.",
        "Help\n\nRooms — devices and charts. Scenarios — action groups and HA scripts/scenes.\nText: “turn on kitchen light”, “snapshot entrance”.\nVoice: “turn off bedroom light”.\n/help — help; /start — home; /cancel — cancel current input. Saved draft fields remain. Accepted commands cannot be cancelled.");
    let text = if voice {
        text.into()
    } else {
        format!(
            "{text}\n\n{}",
            words(
                ctx.lang,
                "Голосовой ввод отключён администратором.",
                "Voice input is disabled by your administrator."
            )
        )
    };
    Ok(base(
        &ctx,
        text,
        Payload::Help,
        vec![vec![Button::callback(
            t(ctx.lang, "home.control"),
            Payload::Control(ControlPayload::ListRooms).to_string(),
        )]],
    ))
}

pub async fn temperature_input(ctx: RenderContext, room: i64, device: i64) -> Result<View> {
    if !device_access_is_current(&ctx, device, true).await? {
        return Ok(unavailable_device(&ctx));
    }
    let dev = db::devices::get_device_by_id(device, &ctx.config.db)
        .await?
        .context("Device unavailable")?;
    ctx.track_device(dev.id, &dev.entity_id);
    let room = db::devices::get_room_id_by_entity(&dev.entity_id, &ctx.config.db)
        .await?
        .unwrap_or(room);
    let entity = ctx
        .config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&dev.entity_id))
        .await?
        .into_iter()
        .next()
        .context("HA unavailable")?;
    if !device_access_is_current(&ctx, device, true).await? {
        return Ok(unavailable_device(&ctx));
    }
    anyhow::ensure!(
        dev.entity_id.starts_with("climate.")
            && entity
                .attributes
                .temperature
                .is_some_and(|value| entity.temperature_target_is_valid(value)),
        "Temperature control unavailable"
    );
    let device_name = super::screens::control::labels::device_name(&dev, &entity, ctx.lang);
    let room_name = db::rooms::get_room_by_id(room, &ctx.config.db)
        .await?
        .map(|r| {
            ctx.track_room(r.id);
            r.display_name().to_owned()
        })
        .unwrap_or_default();
    let unit = entity
        .attributes
        .unit_of_measurement
        .as_deref()
        .unwrap_or("°");
    let temperature = |value: Option<f32>| {
        value
            .map(|value| format!("{value}{unit}"))
            .unwrap_or_default()
    };
    let title = words(ctx.lang, "Задать температуру", "Set temperature").to_owned();
    let parent = Payload::Control(ControlPayload::DeviceControl { room, device });
    let text = format!(
        "{device_name}\n{}: {}\n{}: {} – {}\n\n{}",
        t(ctx.lang, "climate.target"),
        temperature(entity.attributes.temperature),
        t(ctx.lang, "climate.limits"),
        temperature(entity.attributes.min_temp),
        temperature(entity.attributes.max_temp),
        words(
            ctx.lang,
            "Отправьте температуру в чат: 18,5 или 18.5.",
            "Send the temperature in chat: 18.5 or 18,5."
        )
    );
    Ok(View {
        header: Some(title.clone()),
        html_title: Some(title),
        html_context: Some(format!("{room_name} · {device_name}")),
        html_kind: Some("form".into()),
        html_back: Some(parent.clone()),
        text,
        payload: Payload::TemperatureInput { room, device },
        kb: InlineKeyboardMarkup::new(vec![vec![Button::callback(
            t(ctx.lang, "common.cancel"),
            parent.to_string(),
        )]]),
        next_state: Some(State::ExactTemperature {
            room_id: room,
            device_id: device,
        }),
        lang: ctx.lang,
        compact: true,
        notifications: ctx.notifications,
        ..Default::default()
    })
}

pub async fn device_details(ctx: RenderContext, room: i64, device: i64) -> Result<View> {
    if !device_access_is_current(&ctx, device, false).await? {
        return Ok(unavailable_device(&ctx));
    }
    let dev = db::devices::get_device_by_id(device, &ctx.config.db)
        .await?
        .context("Device unavailable")?;
    ctx.track_device(dev.id, &dev.entity_id);
    let room = db::devices::get_room_id_by_entity(&dev.entity_id, &ctx.config.db)
        .await?
        .unwrap_or(room);
    let state = ctx
        .config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&dev.entity_id))
        .await
        .ok()
        .and_then(|s| s.into_iter().next());
    if !device_access_is_current(&ctx, device, false).await? {
        return Ok(unavailable_device(&ctx));
    }
    let name = state
        .as_ref()
        .map(|entity| super::screens::control::labels::device_name(&dev, entity, ctx.lang))
        .unwrap_or_else(|| {
            dev.alias
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&dev.entity_id)
                .to_owned()
        });
    let room_name = db::rooms::get_room_by_id(room, &ctx.config.db)
        .await?
        .map(|room| {
            ctx.track_room(room.id);
            room.display_name().to_owned()
        })
        .unwrap_or_default();
    let summary = match state.as_ref() {
        Some(entity) => {
            let inverted = db::devices::is_state_inverted(&dev.entity_id, &ctx.config.db).await?;
            let alias = ctx
                .config
                .state_alias_for_display(&dev.entity_id, &entity.state, inverted);
            format!(
                "{name}\n{}: {}",
                words(ctx.lang, "Состояние", "State"),
                super::screens::control::labels::state(
                    entity,
                    ctx.lang,
                    inverted,
                    alias.as_deref()
                )
            )
        }
        None => format!(
            "{name}\n{}",
            words(ctx.lang, "Состояние HA недоступно", "HA state unavailable")
        ),
    };
    let text = format!(
        "{}\nEntity ID: {}\n{}",
        dev.alias.as_deref().unwrap_or(&dev.entity_id),
        dev.entity_id,
        state
            .as_ref()
            .map(|s| format!("HA: {}\n{:?}", s.state, s.attributes))
            .unwrap_or_else(
                || words(ctx.lang, "Состояние HA недоступно", "HA state unavailable").into()
            )
    );
    let parent = Payload::Control(ControlPayload::DeviceControl { room, device });
    let mut view = base(
        &ctx,
        text,
        Payload::DeviceDetails { room, device },
        vec![vec![Button::callback(
            t(ctx.lang, "common.back"),
            parent.to_string(),
        )]],
    );
    view.html_title = Some(words(ctx.lang, "Сведения об устройстве", "Device details").into());
    view.html_context = Some(room_name);
    view.html_summary = Some(summary);
    view.html_details_title =
        Some(words(ctx.lang, "Технические сведения", "Technical details").into());
    view.html_back = Some(parent);
    Ok(view)
}

async fn device_access_is_current(ctx: &RenderContext, device: i64, control: bool) -> Result<bool> {
    let access =
        db::access::get_effective_device_access(ctx.user_id, ctx.is_admin, device, &ctx.config.db)
            .await?;
    Ok(access.can_view
        && (!control || access.can_control)
        && db::admission_is_current(ctx.user_id, &ctx.admission_generation, &ctx.config.db).await?)
}

fn unavailable_device(ctx: &RenderContext) -> View {
    let mut view = base(
        ctx,
        words(ctx.lang, "Устройство недоступно", "Device unavailable").into(),
        Payload::Home,
        vec![],
    );
    view.notifications.clear();
    view
}

pub fn parse_temperature(text: &str) -> Result<f32> {
    let value: f32 = text.trim().replace(',', ".").parse()?;
    anyhow::ensure!(value.is_finite(), "Temperature must be finite");
    Ok(value)
}

pub async fn handle_input(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: super::handlers::MyDialogue,
    state: State,
) -> Result<()> {
    let user = msg.from.as_ref().context("User missing")?.id.0;
    let lang = db::get_user_language(user, &config.db)
        .await?
        .unwrap_or(config.default_language);
    if let State::ExactTemperature { room_id, device_id } = state {
        if !db::access::can_control_device(user, user == config.root_user, device_id, &config.db)
            .await?
        {
            bot.send_message(
                msg.chat.id,
                words(
                    lang,
                    "Управление недоступно. Вернитесь в меню.",
                    "Control unavailable. Return to the menu.",
                ),
            )
            .await?;
            dialogue.exit().await?;
            return Ok(());
        }
        let value = match parse_temperature(msg.text().unwrap_or("")) {
            Ok(value) => value,
            Err(_) => {
                bot.send_message(
                    msg.chat.id,
                    words(
                        lang,
                        "Введите число, например 18,5.",
                        "Enter a number, e.g. 18.5.",
                    ),
                )
                .await?;
                return Ok(());
            }
        };
        // Enqueue before yielding this update, preserving ordering against later ±.
        // The worker validates current limits/mode/ACL immediately before sending.
        let critical = super::confirmations::is_critical_device(device_id, &config)
            .await
            .unwrap_or(true);
        let queued = if critical {
            None
        } else {
            match config
                .climate_controls
                .enqueue(
                    config.clone(),
                    user,
                    device_id,
                    crate::core::climate_control::TemperatureCommand::Set(value),
                )
                .await
            {
                Ok(reply) => Some(reply),
                Err(_) => {
                    bot.send_message(msg.chat.id, t(lang, "climate.busy"))
                        .await?;
                    return Ok(());
                }
            }
        };
        let token = config.begin_view_update(user);
        crate::db::user::spawn_with_admission(async move {
            let result: Result<()> = async {
                let outcome = match queued { Some(reply) => Some(reply.await.context("Climate worker stopped")?), None => None };
                let payload = if outcome.is_some() {
                    Payload::Control(ControlPayload::DeviceControl { room: room_id, device: device_id })
                } else {
                    Payload::Control(ControlPayload::QuickAction { room: room_id, device: device_id, cmd: DeviceCmd::SetTemp(value) })
                };
                // Do not cancel an accepted write; navigation cancels only delivery.
                let mut view = super::router::router(payload, user, config.clone()).await?;
                let failed = matches!(&outcome, Some(Err(_))) || view.alert.is_some();
                if let Some(outcome) = outcome {
                    match outcome {
                        Ok(result) => view.notice = Some(super::screens::control::temperature_command_notice(lang, &result)),
                        Err(_) => view.alert = Some(words(lang, "Температура не принята или результат отправки неизвестен. Проверьте состояние, текущие границы и доступ; /cancel — отмена ввода.", "Temperature was rejected or the send outcome is unknown. Check state, current limits and access; /cancel to end input.").into()),
                    }
                }
                if token.is_cancelled() { return Ok(()); }
                if failed {
                    let state = State::ExactTemperature { room_id, device_id };
                    dialogue.update(state.clone()).await?;
                    view.next_state = Some(state);
                } else { dialogue.exit().await?; }
                let mid = config.sessions.get(&user).map(|session| MessageId(session.last_menu_id));
                if let Some(mid) = mid {
                    super::handlers::update_view_cancellable(&bot, msg.chat.id, mid, user, view, config.clone(), Some(&token)).await?;
                } else {
                    super::handlers::send_new_view_inner(&bot, msg.chat.id, user, view, config.clone(), Some(&token)).await?;
                }
                Ok(())
            }.await;
            if let Err(error) = result {
                log::warn!("Temperature input result delivery failed for {user}: {error}");
                if !token.is_cancelled() {
                    let _ = bot.send_message(msg.chat.id, words(lang, "Не удалось получить результат. Проверьте состояние устройства; ввод остаётся открытым.", "Could not read the result. Check the device state; input remains open.")).await;
                }
            }
            config.finish_view_update(user, &token);
        });
        return Ok(());
    }
    if !matches!(state, State::PersonalQuietHours) {
        return Ok(());
    }
    let saved = {
        let _lifecycle = config.admission_lifecycle.lock().await;
        if !db::user_exists(user, &config.db).await {
            return Ok(());
        }
        db::personal::set_quiet_interval(user, msg.text().unwrap_or(""), &config.db).await
    };
    if saved.is_err() {
        bot.send_message(
            msg.chat.id,
            words(
                lang,
                "Введите HH:MM–HH:MM с разными началом и концом, например 23:00–07:00.",
                "Enter HH:MM–HH:MM with different start and end, e.g. 23:00–07:00.",
            ),
        )
        .await?;
        return Ok(());
    }
    let token = config.begin_view_update(user);
    let result = async {
        let view = super::router::router(
            Payload::Personal(PersonalPayload::Settings),
            user,
            config.clone(),
        )
        .await?;
        if token.is_cancelled() {
            return Ok(());
        }
        dialogue.exit().await?;
        let mid = config
            .sessions
            .get(&user)
            .map(|session| MessageId(session.last_menu_id));
        if let Some(mid) = mid {
            super::handlers::update_view_cancellable(
                &bot,
                msg.chat.id,
                mid,
                user,
                view,
                config.clone(),
                Some(&token),
            )
            .await?;
        } else {
            super::handlers::send_new_view_inner(
                &bot,
                msg.chat.id,
                user,
                view,
                config.clone(),
                Some(&token),
            )
            .await?;
        }
        Ok(())
    }
    .await;
    config.finish_view_update(user, &token);
    result
}

#[cfg(test)]
#[path = "personal_structure_tests.rs"]
mod structure_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_temperature_accepts_decimal_comma_without_rounding() {
        for (text, expected) in [
            ("18", 18.0),
            ("18.5", 18.5),
            ("18,5", 18.5),
            ("18.125", 18.125),
        ] {
            assert_eq!(parse_temperature(text).unwrap(), expected);
        }
        for text in ["NaN", "inf", "-inf", "abc", ""] {
            assert!(parse_temperature(text).is_err());
        }
    }
}
