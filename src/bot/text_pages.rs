use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup};

use super::{models::View, router::Payload};
use crate::{db, i18n::t, models::AppConfig};

pub const TEXT_LIMIT: usize = 4096;
const PAGE_RESERVE: usize = 80;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TextPage {
    pub screen: i64,
    pub page: u16,
}

#[derive(Serialize, Deserialize)]
struct PageContext {
    #[serde(default)]
    receipt_resources: Option<Vec<crate::core::types::DisplaySource>>,
    source: String,
    // Command receipts cannot be recreated by rerunning a read-only screen.
    // Keep them for their owner, without caching device state or keyboard actions.
    notice: Option<String>,
    alert: Option<String>,
    // A cancelled field must not reopen just because its long text is paged.
    #[serde(default)]
    suppress_input: bool,
}

pub(crate) async fn source_payload(view: &View, user: u64, config: &AppConfig) -> Result<Payload> {
    if let Some(page) = view.text_page {
        let saved = db::ui_text_pages::context(page.screen, user, &config.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Text page context missing"))?;
        let saved: PageContext = serde_json::from_str(&saved)?;
        return Payload::from_string(&saved.source);
    }
    Ok(view.payload.for_refresh())
}

pub fn text_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Prefer line boundaries, preserve all whitespace and never split a UTF-8 character.
fn split_text(text: &str, limit: usize) -> Vec<String> {
    let mut rest = text;
    let mut pages = Vec::new();
    while text_len(rest) > limit {
        let mut units = 0;
        let mut end = 0;
        let mut newline = 0;
        for (index, ch) in rest.char_indices() {
            if units + ch.len_utf16() > limit {
                break;
            }
            units += ch.len_utf16();
            end = index + ch.len_utf8();
            if ch == '\n' && units > limit / 2 {
                newline = end;
            }
        }
        if newline > 0 {
            end = newline;
        }
        pages.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    pages.push(rest.to_string());
    pages
}

fn content_limit(view: &View) -> usize {
    if view.image.is_some() {
        1024
    } else {
        TEXT_LIMIT
    }
}

fn layout(view: &View) -> (String, Vec<String>) {
    let limit = content_limit(view);
    let status = View {
        header: view.header.clone(),
        notifications: view.notifications.clone(),
        alert: view.alert.clone(),
        notice: view.notice.clone(),
        lang: view.lang,
        compact: view.compact,
        rendered_at: view.rendered_at,
        ..Default::default()
    }
    .get_full_plain_text();
    if text_len(&status) <= limit / 2 && !view.text.is_empty() {
        let budget = limit - text_len(&status) - PAGE_RESERVE;
        return (status, split_text(&view.text, budget));
    }
    // An unusually large status/alert block is itself paginated, without dropping
    // events. Its summary remains visible while every original line is accessible.
    let summary = format!(
        "{}: {}",
        t(view.lang, "common.status_items"),
        view.notifications.len()
    );
    let budget = limit - text_len(&summary) - PAGE_RESERVE;
    (summary, split_text(&view.get_full_plain_text(), budget))
}

pub fn page_text(view: &View, requested: u16) -> String {
    let (status, pages) = layout(view);
    let page = usize::from(requested).min(pages.len() - 1);
    format!(
        "{status}\n\n{}\n\n{} {} / {}",
        pages[page],
        t(view.lang, "action_groups.page"),
        page + 1,
        pages.len()
    )
}

fn select_page(view: &mut View, screen: i64, requested: u16) {
    let pages = layout(view).1.len();
    let page = usize::from(requested).min(pages - 1) as u16;
    // Remove only our pager, preserving the screen's own Refresh/filter buttons.
    view.kb.inline_keyboard.retain(|row| !row.iter().all(|b| (b.text.starts_with("📄") || b.text.starts_with("◀ 📄")) && matches!(&b.kind,
        InlineKeyboardButtonKind::CallbackData(data) if matches!(Payload::from_string(data), Ok(Payload::TextPage { .. })))));
    let source = view.payload.to_string();
    let payload = Payload::TextPage { screen, page };
    for button in view.kb.inline_keyboard.iter_mut().flatten() {
        if let InlineKeyboardButtonKind::CallbackData(data) = &mut button.kind {
            if *data == source {
                *data = payload.to_string();
            }
        }
    }
    let mut row = Vec::new();
    if page > 0 {
        row.push(InlineKeyboardButton::callback(
            "◀ 📄",
            Payload::TextPage {
                screen,
                page: page - 1,
            }
            .to_string(),
        ));
    }
    row.push(InlineKeyboardButton::callback(
        format!("📄 {} / {}", usize::from(page) + 1, pages),
        payload.to_string(),
    ));
    if usize::from(page) + 1 < pages && page < u16::MAX {
        row.push(InlineKeyboardButton::callback(
            "📄 ▶",
            Payload::TextPage {
                screen,
                page: page + 1,
            }
            .to_string(),
        ));
    }
    view.kb.inline_keyboard.push(row);
    view.text_page = Some(TextPage { screen, page });
    view.payload = payload;
}

pub async fn prepare(view: &mut View, user: u64, config: &AppConfig) -> Result<()> {
    if view.text_page.is_none() && text_len(&view.get_full_plain_text()) <= content_limit(view) {
        return Ok(());
    }
    let page = view.text_page.map_or(0, |current| current.page);
    let source = source_payload(view, user, config).await?;
    anyhow::ensure!(
        !matches!(source, Payload::TextPage { .. }),
        "Nested text page context"
    );
    let context = PageContext {
        receipt_resources: Some(if view.notice.is_some() || view.alert.is_some() {
            view.displayed_resources.clone()
        } else {
            Vec::new()
        }),
        source: source.to_string(),
        notice: view.notice.clone(),
        alert: view.alert.clone(),
        suppress_input: view.next_state.is_none(),
    };
    let screen =
        db::ui_text_pages::register(user, &serde_json::to_string(&context)?, &config.db).await?;
    select_page(view, screen, page);
    Ok(())
}

pub async fn render(screen: i64, page: u16, user: u64, config: Arc<AppConfig>) -> Result<View> {
    let saved = db::ui_text_pages::context(screen, user, &config.db)
        .await?
        .and_then(|value| serde_json::from_str::<PageContext>(&value).ok());
    let source = saved
        .as_ref()
        .and_then(|saved| Payload::from_string(&saved.source).ok());
    let Some(source) = source.filter(|p| !matches!(p, Payload::TextPage { .. })) else {
        let lang = db::get_user_language(user, &config.db)
            .await?
            .unwrap_or(config.default_language);
        return Ok(View {
            text: t(lang, "common.page_expired").into(),
            lang,
            notifications: config.get_header_data(user).await,
            kb: InlineKeyboardMarkup::new(vec![vec![
                super::screens::common::main_menu_button_lang(lang),
            ]]),
            ..Default::default()
        });
    };
    // The ID comes from a rendered screen, never from a callback containing an
    // arbitrary action. Still normalize legacy commands before routing again.
    let source = source.for_refresh();
    let mut view = Box::pin(super::router::router(source.clone(), user, config.clone())).await?;
    if source_payload(&view, user, &config).await? != source {
        // Access denial/deletion redirects to a different screen. Never attach
        // a previous receipt or its pagination to that replacement.
        return Ok(view);
    }
    if let Some(saved) = saved {
        if saved.suppress_input {
            view.next_state = None;
        }
        // Legacy cached receipts have no record of the devices they disclosed.
        // Keep the freshly rendered screen, without restoring unguarded text.
        if let Some(resources) = saved.receipt_resources {
            view.displayed_resources.extend(resources);
            view.notice = saved.notice.or(view.notice);
            view.alert = match (saved.alert, view.alert) {
                (Some(previous), Some(current)) if previous != current => {
                    Some(format!("{current}\n\n{previous}"))
                }
                (previous, current) => previous.or(current),
            };
        }
    }
    select_page(&mut view, screen, page);
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_media_keeps_every_caption_page_within_photo_limit() {
        let view = View {
            image: Some(vec![1, 2, 3]),
            text: "Длинное имя 🌡\n".repeat(800),
            ..Default::default()
        };
        let (_, parts) = layout(&view);
        assert_eq!(parts.concat(), view.text);
        for page in 0..parts.len() {
            assert!(text_len(&page_text(&view, page as u16)) <= 1024);
        }
        assert!(view.image.is_some());
    }

    #[test]
    fn split_preserves_all_unicode_and_whitespace() {
        let text = "\nРусский текст 🏡👨‍👩‍👧‍👦\n".repeat(500);
        let pages = split_text(&text, 1000);
        assert_eq!(pages.concat(), text);
        assert!(pages.iter().all(|page| text_len(page) <= 1000));
    }

    #[test]
    fn every_page_keeps_status_and_all_body_characters() {
        let view = View {
            text: "Данные о состоянии\n".repeat(1000),
            compact: true,
            notifications: vec![crate::core::HeaderItem {
                source: None,
                additional_sources: Vec::new(),
                icon: "🔴".into(),
                label: "Камера".into(),
                value: "Идёт запись".into(),
                last_update: chrono::Utc::now(),
            }],
            ..Default::default()
        };
        let (status, parts) = layout(&view);
        assert!(status.contains("Идёт запись"));
        assert_eq!(parts.concat(), view.text);
        for page in 0..parts.len() {
            let text = page_text(&view, page as u16);
            assert!(text_len(&text) <= TEXT_LIMIT);
            assert!(text.contains("Идёт запись"));
        }
    }

    #[test]
    fn oversized_status_is_fully_available_on_pages() {
        let view = View {
            alert: Some("Очень длинное предупреждение\n".repeat(600)),
            ..Default::default()
        };
        let (_, parts) = layout(&view);
        assert_eq!(parts.concat(), view.get_full_plain_text());
        assert!(parts.len() > 1);
        assert!(text_len(&page_text(&view, u16::MAX)) <= TEXT_LIMIT);
    }
}
