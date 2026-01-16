use crate::core::HeaderItem;
use crate::models::NotificationData;

pub fn format_header(items: Vec<HeaderItem>) -> String {
    if items.is_empty() {
        return super::utils::escape_m2("_Ожидание данных..._\n────────────────────\n\n");
    }

    let mut lines = Vec::new();

    for item in items {
        let time_str = format_last_update(item.last_update);

        lines.push(format!(
            "{} {}: `{}`  _\\({}\\)_", // Используем курсив и скобки для времени
            item.icon,
            super::utils::escape_m2(&item.label),
            super::utils::escape_m2(&item.value),
            super::utils::escape_m2(&time_str)
        ));
    }

    format!("{}\n────────────────────\n\n", lines.join("\n"))
}

pub fn format_notification(data: &NotificationData) -> String {
    format!(
        "🔔 *{}*\nСтатус: *{}*",
        super::utils::escape_m2(&data.display_name),
        super::utils::escape_m2(&data.human_state)
    )
}

use chrono::{DateTime, Utc, Local, Duration};

fn format_last_update(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let diff = now - dt;
    let seconds = diff.num_seconds();

    if seconds < 60 {
        if seconds < 15 {
            return "только что".to_string();
        }
        let rounded_seconds = (seconds / 15) * 15;
        return format!("{}с назад", rounded_seconds);
    }

    if diff < Duration::hours(1) {
        return format!("{}м назад", diff.num_minutes());
    }

    let local_dt = dt.with_timezone(&Local);
    if local_dt.date_naive() == Local::now().date_naive() {
        local_dt.format("%H:%M").to_string()
    } else {
        local_dt.format("%d %b %H:%M").to_string()
    }
}