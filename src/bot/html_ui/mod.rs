//! HTML is a presentation adapter for existing routes, not a second command engine.
pub(crate) mod chart_export;
pub(crate) mod media_pending;
mod origin;
pub(crate) mod pending;
mod presentation;
pub(crate) mod range;
mod recovery;
pub(crate) mod temperature;
#[cfg(test)]
mod tests;
pub(crate) mod transport;

use super::{handlers, models::View, router::Payload};
use crate::{
    db,
    models::{AppConfig, UiMessageMode},
};
use anyhow::{Context, Result};
use dashmap::DashMap;
use presentation::{Page, Prepared};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use teloxide::{
    prelude::*,
    types::{CallbackQuery, ChatId, MessageId},
};
use tokio_util::sync::CancellationToken;

const HTML: &str = include_str!("../../../ui/widgets/dist/release.html");
const GRAPH_HTML: &str = include_str!("../../../ui/widgets/dist/graph.html");
const RANGE_HTML: &str = include_str!("../../../ui/widgets/dist/range.html");
const TEMPERATURE_HTML: &str = include_str!("../../../ui/widgets/dist/temperature.html");
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
fn sequence() -> u64 {
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct WidgetSession {
    message: i32,
    generation: String,
    view: View,
    pages: Vec<Page>,
    actions: HashMap<String, Payload>,
    index: usize,
    claimed: bool,
    anchor: Option<Payload>,
    origin: origin::Trail,
    origin_request: Option<Payload>,
    /// One-shot source-list destination and direction (backwards opens its last part).
    list_navigation: Option<(Payload, bool)>,
}
#[derive(Default)]
pub struct HtmlUi {
    pub enabled: bool,
    sessions: DashMap<u64, WidgetSession>,
    ranges: DashMap<u64, range::Draft>,
    pub(crate) pending: pending::PendingCommands,
    pub(crate) media: media_pending::Registry,
}
impl HtmlUi {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }
    pub fn clear(&self, user: u64) {
        self.sessions.remove(&user);
        self.ranges.remove(&user);
        self.pending.clear(user);
    }
    pub fn input_is_open(&self, user: u64, message: i32) -> bool {
        self.sessions
            .get(&user)
            .is_some_and(|session| session.message == message && session.view.next_state.is_some())
    }
}

pub async fn preferred(config: &AppConfig, user: u64) -> bool {
    config.html_ui.enabled
        && db::interface::get(user, &config.db).await.ok()
            == Some(db::interface::InterfaceMode::Html)
}
pub fn eligible(view: &View) -> bool {
    presentation::prepare(view, "0".repeat(32), 1).is_ok()
}
pub fn renderer_matches(config: &AppConfig, user: u64, view: &View) -> bool {
    config
        .html_ui
        .sessions
        .get(&user)
        .is_some_and(|session| renderer(&session.view) == renderer(view))
}
/// The shared command router historically returns the room after on/off. Keep
/// an HTML command issued inside the device's own controls on that device.
pub(crate) fn device_write_origin(
    config: &AppConfig,
    user: u64,
    action: &Payload,
) -> Option<Payload> {
    if !config
        .sessions
        .get(&user)
        .is_some_and(|s| s.ui_message_mode == UiMessageMode::Html)
    {
        return None;
    }
    let Payload::Control(super::router::ControlPayload::QuickAction { room, device, cmd }) = action
    else {
        return None;
    };
    if !super::router::action_requires_control(cmd) {
        return None;
    }
    let session = config.html_ui.sessions.get(&user)?;
    if session.view.html_chart.is_some()
        || session.view.html_range.is_some()
        || session.view.html_media_controls
    {
        return None;
    }
    let expected = Payload::Control(super::router::ControlPayload::DeviceControl {
        room: *room,
        device: *device,
    });
    (session.view.payload.for_refresh() == expected).then_some(expected)
}
fn renderer(view: &View) -> &'static str {
    if view.html_temperature.is_some() {
        TEMPERATURE_HTML
    } else if view.html_chart.is_some() {
        GRAPH_HTML
    } else if view.html_range.is_some() {
        RANGE_HTML
    } else {
        HTML
    }
}
pub async fn reject(bot: &Bot, query: &CallbackQuery, text: &str) -> Result<()> {
    answer(bot, query, if is_widget(query) { "denied" } else { text }).await
}
pub fn is_widget(query: &CallbackQuery) -> bool {
    query.chat_instance == "html-transport" && query.id.0.starts_with("html:")
}
pub async fn answer(bot: &Bot, query: &CallbackQuery, text: &str) -> Result<()> {
    if is_widget(query) {
        let body = if text == "denied" {
            json!({"denied":true})
        } else {
            json!({"accepted":true})
        };
        transport::call(
            bot,
            "answerWidgetQuery",
            json!({"widget_query_id":&query.id.0[5..],"data":body.to_string()}),
        )
        .await?;
    } else {
        let reply = bot.answer_callback_query(query.id.clone());
        if text.is_empty() {
            reply.await?;
        } else {
            reply.text(text).await?;
        }
    }
    Ok(())
}

pub async fn deliver(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user: u64,
    chat: ChatId,
    message: Option<MessageId>,
    view: &View,
    cancellation: Option<&CancellationToken>,
) -> Result<Option<MessageId>> {
    if !presentation::supported(view) {
        return Ok(None);
    }
    let Some(generation) = handlers::view_admission(view, config, user, chat).await? else {
        return Ok(None);
    };
    let mut decorated = view.clone();
    let origin = origin::prepare(
        config.html_ui.sessions.get(&user).as_deref(),
        &generation,
        &mut decorated,
    );
    let media_receipt = config
        .html_ui
        .media
        .decorate(user, &generation, &mut decorated);
    let view = &decorated;
    let rev: String = sqlx::query_scalar("SELECT lower(hex(randomblob(16)))")
        .fetch_one(&config.db)
        .await?;
    let pending = config.html_ui.pending.kinds(user, &generation);
    let Prepared { mut pages, actions } =
        match presentation::prepare_with_pending(view, rev, sequence(), &pending) {
            Ok(prepared) => prepared,
            Err(error) => {
                log::debug!("HTML uses classic fallback: {error}");
                return Ok(None);
            }
        };
    for button in pages.iter_mut().flat_map(|p| p.rows.iter_mut().flatten()) {
        if actions
            .get(&button.key)
            .and_then(media_pending::Target::from_payload)
            .is_some_and(|target| config.html_ui.media.blocks(user, &generation, &target))
        {
            button.kind = "wait".into();
        }
    }
    let index = config
        .html_ui
        .sessions
        .get(&user)
        .filter(|old| {
            old.list_navigation.as_ref().is_some_and(|(target, _)| {
                target == &presentation::list_identity(&view.payload.for_refresh())
            }) || old.view.payload.for_refresh() == view.payload.for_refresh()
        })
        .map(|old| {
            if let Some((_, backwards)) = old.list_navigation.as_ref().filter(|(target, _)| {
                target == &presentation::list_identity(&view.payload.for_refresh())
            }) {
                return if *backwards { pages.len() - 1 } else { 0 };
            }
            old.anchor
                .as_ref()
                .and_then(|anchor| {
                    pages.iter().position(|page| {
                        page.rows
                            .iter()
                            .flatten()
                            .any(|button| actions.get(&button.key) == Some(anchor))
                    })
                })
                .unwrap_or_else(|| old.index.min(pages.len() - 1))
        })
        .unwrap_or(0);
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Ok(None);
    }
    let data = pages[index].json()?;
    let id = if let Some(message) = message {
        transport::call(
            bot,
            "editMessageWidgetData",
            json!({"chat_id":chat.0,"message_id":message.0,"chat_data":data}),
        )
        .await?;
        message
    } else {
        let html = renderer(view);
        let mut request = json!({
            "chat_id": chat.0, "html": html, "width": 0, "height": 600,
            "allow_queries": true, "chat_data": data
        });
        if config.ha_client.is_mock() {
            request["caption"] = json!(if view.lang == crate::i18n::Language::En {
                "Test home · Virtual devices"
            } else {
                "Тестовый дом · Устройства виртуальные"
            });
        }
        let sent = transport::call(bot, "sendWidget", request).await?;
        MessageId(
            sent["message_id"]
                .as_i64()
                .and_then(|id| i32::try_from(id).ok())
                .context("Missing widget message ID")?,
        )
    };
    if handlers::view_admission(view, config, user, chat)
        .await?
        .is_none()
    {
        config.html_ui.clear(user);
        let _ = transport::call(
            bot,
            "editMessageWidgetData",
            json!({"chat_id":chat.0,"message_id":id.0,"chat_data":"{\"denied\":true}"}),
        )
        .await;
        if message.is_none() {
            let _ = bot.delete_message(chat, id).await;
        }
        return Ok(None);
    }
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        if message.is_none() {
            let _ = bot.delete_message(chat, id).await;
        }
        return Ok(None);
    }
    if !matches!(
        view.html_kind.as_deref(),
        Some("range" | "calendar" | "time")
    ) {
        config.html_ui.ranges.remove(&user);
    }
    config.html_ui.media.mark_shown(user, media_receipt);
    config.html_ui.sessions.insert(
        user,
        WidgetSession {
            message: id.0,
            generation,
            view: view.clone(),
            pages,
            actions,
            index,
            claimed: false,
            list_navigation: None,
            anchor: None,
            origin,
            origin_request: None,
        },
    );
    Ok(Some(id))
}

/// Resolve a server-issued key only after the normal admission middleware.
/// UI lock serializes claims with delivery, pagination, and view replacement.
/// Answers every query here; Some forwards a validated payload to the router.
pub async fn resolve(
    bot: &Bot,
    query: &CallbackQuery,
    config: &Arc<AppConfig>,
) -> Result<Option<String>> {
    let user = query.from.id.0;
    let Some(message) = query.message.as_ref() else {
        answer(bot, query, "denied").await?;
        return Ok(None);
    };
    let lock = config.ui_lock_for(user);
    let _guard = lock.lock().await;
    if query.data.as_deref() == Some("r1:home") {
        recovery::home(bot, query, config).await?;
        return Ok(None);
    }
    let snapshot = config
        .html_ui
        .sessions
        .get(&user)
        .map(|value| value.clone());
    let Some(snapshot) = snapshot else {
        answer(bot, query, "denied").await?;
        return Ok(None);
    };
    let current = config.sessions.get(&user).is_some_and(|session| {
        session.last_menu_id == message.id().0 && session.ui_message_mode == UiMessageMode::Html
    });
    if !config.html_ui.enabled
        || !current
        || snapshot.message != message.id().0
        || !db::user::admission_is_current(user, &snapshot.generation, &config.db).await?
        || handlers::view_admission(&snapshot.view, config, user, message.chat().id)
            .await?
            .is_none()
    {
        answer(bot, query, "denied").await?;
        return Ok(None);
    }
    let data = query.data.as_deref().unwrap_or("");
    let parts: Vec<_> = data.split(':').collect();
    if data.len() > 64
        || parts.len() != 3
        || parts[0] != "r1"
        || parts[1].len() != 32
        || !parts[1].bytes().all(|b| b.is_ascii_hexdigit())
        || parts[2].is_empty()
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        answer(bot, query, "denied").await?;
        return Ok(None);
    }
    let refresh_input =
        parts[2] == "read" && snapshot.view.next_state.is_some() && !snapshot.claimed;
    if parts[2] == "read" && !refresh_input {
        answer(bot, query, "").await?;
        config
            .html_ui
            .pending
            .reconcile(config, user, &snapshot.generation)
            .await;
        return Ok(Some(snapshot.view.payload.for_refresh().to_string()));
    }
    if snapshot.claimed {
        answer(bot, query, "").await?;
        return Ok(None);
    }
    // Telegram may deliver a tap just after a background refresh replaced the
    // revision. Admission and message identity were checked above. Never run
    // the obsolete key against the new action map, or treat this race as ACL
    // revocation: return the current page and let the user tap its fresh controls.
    let stale = parts[1] != snapshot.pages[0].rev;
    let page = if refresh_input || stale {
        if stale {
            log::info!("HTML: resynchronizing a tap after a background refresh");
        }
        Some(snapshot.index)
    } else {
        parts[2]
            .strip_prefix('p')
            .and_then(|p| p.parse::<usize>().ok())
            .filter(|&p| p < snapshot.pages.len())
    };
    if let Some(index) = page {
        let mut page = snapshot.pages[index].clone();
        page.seq = sequence();
        // Answer contains the same bounded page; no unrelated backend action is executed.
        transport::call(bot,"answerWidgetQuery",json!({"widget_query_id":&query.id.0[5..],"data":serde_json::to_string(&json!({"page":page}))?})).await?;
        transport::call(bot,"editMessageWidgetData",json!({"chat_id":message.chat().id.0,"message_id":message.id().0,"chat_data":page.json()?})).await?;
        if let Some(mut session) = config.html_ui.sessions.get_mut(&user) {
            session.index = index;
            session.pages[index] = page;
        }
        return Ok(None);
    }
    let selected = temperature::resolve(&snapshot.view, &snapshot.actions, parts[2]);
    let Some(action) = selected.as_ref().or_else(|| snapshot.actions.get(parts[2])) else {
        answer(bot, query, "denied").await?;
        return Ok(None);
    };
    if media_pending::Target::from_payload(action).is_some_and(|target| {
        config
            .html_ui
            .media
            .blocks(user, &snapshot.generation, &target)
    }) {
        answer(bot, query, "").await?;
        return Ok(Some(snapshot.view.payload.for_refresh().to_string()));
    }
    if let Payload::Control(super::router::ControlPayload::QuickAction { device, cmd, .. }) = action
    {
        if super::router::action_requires_control(cmd)
            && config
                .html_ui
                .pending
                .blocks(user, *device, &snapshot.generation)
        {
            // A refreshed revision must not turn a still-pending command into a new write.
            answer(bot, query, "").await?;
            return Ok(Some(snapshot.view.payload.for_refresh().to_string()));
        }
    }
    if let Some(mut session) = config.html_ui.sessions.get_mut(&user) {
        session.claimed = true;
        session.origin_request = origin::read_route(action);
        session.list_navigation =
            snapshot.pages[snapshot.index]
                .list
                .as_ref()
                .and_then(|(_, previous, next)| {
                    if previous.as_deref() == Some(parts[2]) {
                        Some((presentation::list_identity(&action.for_refresh()), true))
                    } else if next.as_deref() == Some(parts[2]) {
                        Some((presentation::list_identity(&action.for_refresh()), false))
                    } else {
                        None
                    }
                });
        if let Payload::Control(super::router::ControlPayload::QuickAction { room, device, cmd }) =
            action
        {
            if super::router::action_requires_control(cmd) {
                session.anchor = Some(Payload::Control(
                    super::router::ControlPayload::DeviceControl {
                        room: *room,
                        device: *device,
                    },
                ));
            }
        }
    }
    // Acknowledge before handing off to handlers that may wait for HA. A failed
    // ACK keeps the claim consumed; recovery must read state, never retry a write.
    answer(bot, query, "").await?;
    Ok(Some(action.to_string()))
}
