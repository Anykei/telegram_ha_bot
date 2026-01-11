use std::sync::Arc;
use teloxide::Bot;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use crate::ha::NotifyEvent;
use anyhow::Result;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use crate::models::{AppConfig};

use crate::db;

pub async fn spawn_notification_processor(
    mut rx: mpsc::Receiver<NotifyEvent>,
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) {
    info!("Background notification processor started");

    tokio::spawn(async move {
        loop {
            tokio::select! {
            Some(event) = rx.recv() => {
                if let Err(e) = handle_notification_event(&bot, &config, event).await {
                    error!("Core failed to process notification: {}", e);
                }
            }
            _ = cancel_token.cancelled() => break,
        }
        }
    });
}

pub async fn handle_notification_event(
    bot: &Bot,
    config: &Arc<AppConfig>,
    event: NotifyEvent
) -> Result<()> {
    info!("Handling notification event: {:?}", event);

    if event.new_state == event.old_state {
        return Ok(());
    }

    let recipients = db::get_subscribers(&config.db, &event.entity_id).await?;
    if recipients.is_empty() {
        return Ok(());
    }

    let message_text = prepare_notification_text(config, &event).await?;

    info!("message_text: {}", message_text);
    for user_id in recipients {
        let _ = bot.send_message(ChatId(user_id), &message_text)
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;
    }

    Ok(())
}

async fn prepare_notification_text(config: &Arc<AppConfig>, event: &NotifyEvent) -> Result<String> {
    let aliases = db::get_aliases_map(&config.db).await;
    let state_aliases = db::get_state_aliases(&config.db).await;

    let display_name = aliases.get(&event.entity_id)
        .map(|s| s.as_str())
        .unwrap_or(&event.entity_id);

    let display_state = format_state_human(&event.entity_id, &event.new_state, &state_aliases);

    let text = format!("🔔 *{}*\nСтатус: *{}*",
                       escape_m2(display_name),
                       escape_m2(&display_state)
    );

    Ok(text)
}

pub fn format_state_human(entity_id: &str, state: &str, custom_map: &db::StateMap) -> String {
    if let Some(mapped) = custom_map.get(entity_id).and_then(|m| m.get(state)) {
        return mapped.clone();
    }
    match state {
        "on" => "Включено".to_string(),
        "off" => "Выключено".to_string(),
        _ => state.to_string(),
    }
}

fn escape_m2(text: &str) -> String {
    text.replace('.', "\\.").replace('-', "\\-").replace('_', "\\_").replace('*', "\\*").replace('|', "\\|")
}