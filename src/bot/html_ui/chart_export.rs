//! Export the original rendered PNG, never the widget's 32-bucket overview.
use super::*;
use crate::bot::{models::View, router::Payload};
use crate::i18n::Language;
use chrono::{DateTime, Local};
use teloxide::types::InputFile;

fn words(lang: Language, ru: &str, en: &str) -> String {
    if lang == Language::Ru { ru } else { en }.into()
}
fn caption(view: &View, mock: bool) -> Result<String> {
    let [start, end] = view.html_range.context("Missing chart bounds")?;
    let dates = [start, end].map(|value| {
        DateTime::from_timestamp(value, 0)
            .map(|d| {
                d.with_timezone(&Local)
                    .format("%d.%m.%Y %H:%M:%S UTC%:z")
                    .to_string()
            })
            .context("Invalid chart bounds")
    });
    let [start, end] = dates;
    // Plain text, bounded well below Telegram's caption limit, including UTF-16.
    let mut caption = format!(
        "{}\n{}\n{} — {}",
        view.html_title
            .as_deref()
            .unwrap_or("Graph")
            .chars()
            .take(160)
            .collect::<String>(),
        view.html_context
            .as_deref()
            .unwrap_or_default()
            .chars()
            .take(120)
            .collect::<String>(),
        start?,
        end?,
    );
    if mock {
        caption.push_str(&words(view.lang, "\nТестовые данные", "\nTest data"));
    }
    Ok(caption)
}

pub async fn send(
    bot: &Bot,
    q: &CallbackQuery,
    config: &Arc<AppConfig>,
    payload: &Payload,
) -> Result<()> {
    // Only the currently issued HTML action can export; native/stale/forwarded
    // callbacks cannot select another user's or another message's cached image.
    if !is_widget(q) {
        return reject(bot, q, "Откройте график заново / Reopen the chart").await;
    }
    let user = q.from.id.0;
    let Some(message) = q.message.as_ref() else {
        return Ok(());
    };
    let chat = message.chat().id;
    let lock = config.ui_lock_for(user);
    let _guard = lock.lock().await;
    let Some(session) = config
        .html_ui
        .sessions
        .get(&user)
        .filter(|s| {
            s.message == message.id().0
                && s.claimed
                && s.view.html_chart.is_some()
                && s.actions.values().any(|p| p == payload)
        })
        .map(|s| s.clone())
    else {
        return Ok(());
    };
    if !config.sessions.get(&user).is_some_and(|s| {
        s.last_menu_id == session.message && s.ui_message_mode == UiMessageMode::Html
    }) {
        return Ok(());
    }
    let mut view = session.view;
    if handlers::view_admission(&view, config, user, chat)
        .await?
        .as_deref()
        != Some(&session.generation)
    {
        config.html_ui.clear(user);
        transport::call(
            bot,
            "editMessageWidgetData",
            json!({"chat_id":chat.0,"message_id":session.message,"chat_data":"{\"denied\":true}"}),
        )
        .await?;
        return Ok(());
    }
    view.notice = None;
    view.alert = None;
    if let Some(bytes) = view.image.clone().filter(|b| !b.is_empty()) {
        let [start, end] = view.html_range.context("Missing chart bounds")?;
        let result = bot
            .send_document(
                chat,
                InputFile::memory(bytes).file_name(format!("graph_{start}_{end}.png")),
            )
            .caption(caption(&view, config.ha_client.is_mock())?)
            .await;
        match result {
            Ok(sent) => {
                // Admission can change while Telegram processes the upload.
                if handlers::view_admission(&view, config, user, chat)
                    .await?
                    .as_deref()
                    != Some(&session.generation)
                {
                    let _ = bot.delete_message(chat, sent.id).await;
                    config.html_ui.clear(user);
                    transport::call(
                        bot,
                        "editMessageWidgetData",
                        json!({"chat_id":chat.0,"message_id":session.message,"chat_data":"{\"denied\":true}"}),
                    )
                    .await?;
                    return Ok(());
                }
                view.notice = Some(words(view.lang, "PNG отправлен.", "PNG sent."));
            }
            Err(error) => {
                log::warn!("Chart PNG upload failed: {error}");
                view.alert = Some(words(
                    view.lang,
                    "Отправка не подтверждена. Проверьте чат.",
                    "Upload not confirmed. Check the chat.",
                ));
            }
        }
    } else {
        view.alert = Some(words(
            view.lang,
            "Нет данных за выбранный период.",
            "No data for this period.",
        ));
    }
    // Keep the exact displayed snapshot and original action-free refresh route.
    // A new revision releases the claim; replaying the old key only resyncs it.
    // Failed uploads are never retried automatically, including on Read.
    deliver(
        bot,
        config,
        user,
        chat,
        Some(MessageId(session.message)),
        &view,
        None,
    )
    .await?;
    Ok(())
}
