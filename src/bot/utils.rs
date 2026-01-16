use teloxide::types::{MessageId};
use teloxide::prelude::*;

pub fn spawn_delayed_delete(bot: Bot, chat_id: ChatId, msg_id: MessageId, delay_secs: u64) {
    debug!("Deleting message id {} delay: {}", msg_id, delay_secs);

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        let _ = bot.delete_message(chat_id, msg_id).await;
    });
}

pub async fn delete_message_after(bot: Bot, chat_id: ChatId, msg_id: MessageId, delay_secs: u64){
    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
    let _ = bot.delete_message(chat_id, msg_id).await;
}

pub fn escape_m2(text: &str) -> String {
    text.replace('.', "\\.").replace('-', "\\-").replace('_', "\\_").replace('*', "\\*").replace('|', "\\|")
}