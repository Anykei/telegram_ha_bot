use teloxide::types::{MessageId};
use teloxide::prelude::*;

pub const UI_PLACEHOLDER_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0x60, 0x60, 0x60, 0x00,
    0x00, 0x00, 0x04, 0x00, 0x01, 0x27, 0x34, 0x2D, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

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