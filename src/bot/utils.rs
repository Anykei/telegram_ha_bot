use teloxide::types::{MessageId};
use teloxide::prelude::*;

pub const UI_PLACEHOLDER_BYTES: &[u8] = include_bytes!("assets/ha_logo.png");

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

/// Экранирует текст для режима Telegram MarkdownV2.
/// Согласно Google Style Guide: функции обработки строк должны быть эффективными (O(n)).
pub fn escape_markdown_v2(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        match c {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}