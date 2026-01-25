use std::sync::Arc;
use anyhow::Result;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use crate::models::{AppConfig, NotificationData};


pub async fn send_notification_all_recipients(
    bot: Bot,
    config: Arc<AppConfig>,
    recipients: Vec<i64>,
    message: String
) -> Result<()> {
    for recipient in recipients {
        send_notification_text_to_recipient(bot.clone(), config.clone(), recipient, message.clone()).await;
    }
    Ok(())
}

pub async fn send_notification_text_to_recipient(bot: Bot,
                                                 config: Arc<AppConfig>,
                                                 recipient: i64,
                                                 message: String){
    let chat_id = ChatId(recipient);
    let delay = config.delete_notification_messages_timeout_s;

    tokio::spawn(async move {
        if let Ok(msg) = bot.send_message(chat_id, message.as_str())
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await
        {
            crate::bot::utils::delete_message_after(bot, chat_id, msg.id, delay).await;
        }
    });
}

pub async fn send_notification(bot: Bot, config: Arc<AppConfig>, data: NotificationData) -> Result<()> {
    for user_id in data.recipients {
        let m = data.human_state.clone();
        send_notification_text_to_recipient(bot.clone(), config.clone(), user_id, m).await;
    }
    Ok(())
}