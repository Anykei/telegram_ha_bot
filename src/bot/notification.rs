use crate::models::{AppConfig, NotificationData};
use chrono::{Local, Timelike, Utc};
use chrono_tz::Tz;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatId;

async fn send_notification_text_to_recipient(
    bot: Bot,
    config: Arc<AppConfig>,
    recipient: i64,
    message: String,
) -> anyhow::Result<()> {
    let chat_id = ChatId(recipient);
    let delay = config.delete_notification_messages_timeout_s;

    let msg = bot
        .send_message(chat_id, message.as_str())
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await?;
    // Expiry must not occupy one of the two notification delivery workers.
    crate::bot::utils::spawn_delayed_delete(bot, chat_id, msg.id, delay);
    Ok(())
}

pub async fn send_notification(
    _bot: Bot,
    config: Arc<AppConfig>,
    data: NotificationData,
) -> anyhow::Result<()> {
    crate::core::notification_delivery::enqueue(&config, data).await
}

pub(crate) async fn deliver_notification(
    bot: Bot,
    config: Arc<AppConfig>,
    user_id: i64,
    generation: String,
    message: String,
    critical: bool,
    scope: crate::models::NotificationScope,
) -> anyhow::Result<()> {
    if quiet_hours_active(config.clone(), user_id, critical).await {
        let entity_id = user_id.to_string();
        let _ = crate::db::activity_log::log(
            crate::db::activity_log::NewActivity {
                user_id: Some(user_id as u64),
                kind: "notification",
                entity_type: "user",
                entity_id: Some(&entity_id),
                action: "quiet_hours_suppressed",
                status: "ok",
                message: None,
            },
            &config.db,
        )
        .await;
        return Ok(());
    }

    let allowed = match &scope {
        crate::models::NotificationScope::Device(entity) => {
            crate::db::subscriptions::get_subscribers(entity, &config.db)
                .await?
                .contains(&user_id)
        }
        crate::models::NotificationScope::Camera(camera) => {
            crate::db::cameras::get_accessible_camera(
                user_id as u64,
                user_id as u64 == config.root_user,
                *camera,
                &config.db,
            )
            .await?
            .is_some()
        }
    };
    if !allowed
        || !crate::db::admission_is_current(user_id as u64, &generation, &config.db).await?
        // A queued message may outlive a switch from classic to HTML.
        || crate::core::notification_delivery::suppress_device_message(&config, user_id as u64, &scope).await?
    {
        return Ok(());
    }
    send_notification_text_to_recipient(bot, config, user_id, message).await
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
#[path = "notification_tests.rs"]
mod delivery_tests;

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
