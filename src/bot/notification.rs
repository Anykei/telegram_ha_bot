use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use crate::models::{AppConfig, NotificationData};
use crate::bot::view;


pub async fn send_notification_all_recipients(
    bot: Bot,
    config: Arc<AppConfig>,
    recipients: Vec<i64>,
    message: String
) -> anyhow::Result<()> {
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

pub async fn send_notification(
    bot: Bot,
    config: Arc<AppConfig>,
    data: NotificationData,
) -> anyhow::Result<()> {
    let message_text = view::format_notification(&data);
    let shared_message = Arc::new(message_text);

    for user_id in data.recipients {
        let m = Arc::clone(&shared_message);
        send_notification_text_to_recipient(bot.clone(), config.clone(), user_id, m.to_string()).await;
    }

    Ok(())
}