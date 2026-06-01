use crate::models::{AppConfig, NotificationData};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatId;

pub async fn send_notification_text_to_recipient(
    bot: Bot,
    config: Arc<AppConfig>,
    recipient: i64,
    message: String,
) {
    let chat_id = ChatId(recipient);
    let delay = config.delete_notification_messages_timeout_s;

    tokio::spawn(async move {
        match bot
            .send_message(chat_id, message.as_str())
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await
        {
            Ok(msg) => {
                crate::bot::utils::delete_message_after(bot, chat_id, msg.id, delay).await;
            }
            Err(e) => {
                log::error!("Failed to send notification to {}: {}", recipient, e);
            }
        }
    });
}

pub async fn send_notification(
    bot: Bot,
    config: Arc<AppConfig>,
    data: NotificationData,
) -> anyhow::Result<()> {
    for user_id in data.recipients {
        let m = data.human_state.clone();
        send_notification_text_to_recipient(bot.clone(), config.clone(), user_id, m).await;
    }
    Ok(())
}
