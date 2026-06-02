use chrono::{DateTime, Local, Utc};

pub fn datetime(value: DateTime<Utc>) -> String {
    value
        .with_timezone(&Local)
        .format("%d.%m.%Y %H:%M")
        .to_string()
}

pub fn optional_datetime(value: Option<DateTime<Utc>>) -> String {
    value.map(datetime).unwrap_or_else(|| "нет".to_string())
}

pub fn decimal_mb(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    format!("{:.1} MB", bytes as f64 / MB)
}

pub fn bytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        decimal_mb(bytes)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}

pub fn quota(value_mb: i64) -> String {
    if value_mb <= 0 {
        "выкл".to_string()
    } else if value_mb >= 1024 {
        format!("{}ГБ", value_mb / 1024)
    } else {
        format!("{}МБ", value_mb)
    }
}
