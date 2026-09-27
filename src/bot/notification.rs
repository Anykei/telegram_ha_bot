use crate::models::{AppConfig, NotificationData};
use chrono::{Local, Timelike, Utc};
use chrono_tz::Tz;
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
        if quiet_hours_active(config.clone(), user_id, data.critical).await {
            let entity_id = user_id.to_string();
            let _ = crate::db::activity_log::log(
                crate::db::activity_log::NewActivity {
                    user_id: Some(user_id as u64),
                    kind: "notification",
                    entity_type: "user",
                    entity_id: Some(&entity_id),
                    action: "quiet_hours_suppressed",
                    status: "ok",
                    message: Some(&data.human_state),
                },
                &config.db,
            )
            .await;
            continue;
        }

        let m = data.human_state.clone();
        send_notification_text_to_recipient(bot.clone(), config.clone(), user_id, m).await;
    }
    Ok(())
}

async fn quiet_hours_active(config: Arc<AppConfig>, user_id: i64, critical: bool) -> bool {
    let schedule = match crate::db::user_notification_schedule::get(user_id, &config.db).await {
        Ok(Some(schedule)) => schedule,
        Ok(None) => return false,
        Err(error) => {
            log::warn!(
                "Failed to load notification schedule for {}: {}",
                user_id,
                error
            );
            return false;
        }
    };

    if schedule.quiet_enabled == 0 {
        return false;
    }
    if critical {
        return false;
    }

    let Some(from) = schedule.quiet_from.as_deref() else {
        return false;
    };
    let Some(to) = schedule.quiet_to.as_deref() else {
        return false;
    };
    if !crate::db::user_notification_schedule::valid_hh_mm(from)
        || !crate::db::user_notification_schedule::valid_hh_mm(to)
    {
        log::warn!("Invalid quiet hours for user {}", user_id);
        return false;
    }

    let from = parse_minutes(from);
    let to = parse_minutes(to);
    if from == to {
        return false;
    }

    let Some(current) = current_minutes(schedule.timezone.as_deref(), user_id) else {
        return false;
    };
    if from < to {
        current >= from && current < to
    } else {
        current >= from || current < to
    }
}

fn parse_minutes(value: &str) -> u32 {
    let (hours, minutes) = value.split_once(':').unwrap_or(("00", "00"));
    hours.parse::<u32>().unwrap_or(0) * 60 + minutes.parse::<u32>().unwrap_or(0)
}

fn current_minutes(timezone: Option<&str>, user_id: i64) -> Option<u32> {
    let Some(timezone) = timezone.map(str::trim).filter(|value| !value.is_empty()) else {
        let now = Local::now();
        return Some(now.hour() * 60 + now.minute());
    };

    let Ok(timezone) = timezone.parse::<Tz>() else {
        log::warn!(
            "Invalid quiet hours timezone '{}' for user {}; ignoring quiet hours",
            timezone,
            user_id
        );
        return None;
    };

    let now = Utc::now().with_timezone(&timezone);
    Some(now.hour() * 60 + now.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hours_accepts_iana_timezone() {
        assert!(current_minutes(Some("Europe/Moscow"), 1).is_some());
        assert!(current_minutes(Some("UTC"), 1).is_some());
    }

    #[test]
    fn quiet_hours_rejects_invalid_timezone() {
        assert!(current_minutes(Some("not-a-timezone"), 1).is_none());
        assert!(current_minutes(Some("+03:99"), 1).is_none());
    }
}
