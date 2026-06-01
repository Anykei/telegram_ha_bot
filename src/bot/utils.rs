use teloxide::prelude::*;
use teloxide::types::MessageId;

pub const UI_PLACEHOLDER_BYTES: &[u8] = include_bytes!("assets/ha_logo.png");

pub fn spawn_delayed_delete(bot: Bot, chat_id: ChatId, msg_id: MessageId, delay_secs: u64) {
    debug!("Deleting message id {} delay: {}", msg_id, delay_secs);

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        let _ = bot.delete_message(chat_id, msg_id).await;
    });
}

pub async fn delete_message_after(bot: Bot, chat_id: ChatId, msg_id: MessageId, delay_secs: u64) {
    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
    let _ = bot.delete_message(chat_id, msg_id).await;
}

pub mod md {
    pub fn plain(text: &str) -> String {
        let mut escaped = String::with_capacity(text.len() * 2);
        for c in text.chars() {
            match c {
                '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '='
                | '|' | '{' | '}' | '.' | '!' => {
                    escaped.push('\\');
                    escaped.push(c);
                }
                _ => escaped.push(c),
            }
        }
        escaped
    }

    pub fn bold(text: &str) -> String {
        format!("*{}*", plain(text))
    }

    pub fn italic(text: &str) -> String {
        format!("_{}_", plain(text))
    }

    pub fn code(text: &str) -> String {
        format!("`{}`", plain(text))
    }
}

#[cfg(test)]
mod tests {
    use super::md;

    #[test]
    fn markdown_plain_escapes_v2_control_chars() {
        assert_eq!(
            md::plain("sensor.kitchen_temp (avg)_1!"),
            "sensor\\.kitchen\\_temp \\(avg\\)\\_1\\!"
        );
    }

    #[test]
    fn markdown_helpers_escape_inner_text() {
        assert_eq!(md::bold("A_B"), "*A\\_B*");
        assert_eq!(md::italic("10.5"), "_10\\.5_");
        assert_eq!(md::code("x`y"), "`x\\`y`");
    }
}
