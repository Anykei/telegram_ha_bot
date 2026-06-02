use crate::db;
use crate::models::AppConfig;
use anyhow::Result;
use sqlx::Row;
use std::sync::Arc;

pub(crate) fn normalize_command_text(text: &str) -> String {
    text.trim().to_lowercase().replace('ё', "е")
}

pub(crate) fn split_camera_video_command(rest: &str) -> (&str, Option<u32>) {
    let mut parts = rest.split_whitespace().collect::<Vec<_>>();
    let seconds = parts.last().and_then(|value| parse_duration_token(value));
    if seconds.is_some() {
        parts.pop();
    }
    if seconds.is_some() {
        let query_len = rest
            .trim_end()
            .rsplit_once(char::is_whitespace)
            .map(|(query, _)| query.trim())
            .unwrap_or(rest);
        (query_len, seconds)
    } else {
        (rest, None)
    }
}

fn parse_duration_token(value: &str) -> Option<u32> {
    let value = value.trim().to_lowercase();
    for suffix in ["сек", "с", "sec", "s"] {
        if let Some(number) = value.strip_suffix(suffix) {
            return number.parse::<u32>().ok();
        }
    }
    None
}

pub(crate) fn parse_device_text_command(
    text: &str,
) -> Option<(&str, crate::core::devices::DeviceAction)> {
    for (prefix, action) in [
        ("включи ", crate::core::devices::DeviceAction::TurnOn),
        ("включить ", crate::core::devices::DeviceAction::TurnOn),
        ("вкл ", crate::core::devices::DeviceAction::TurnOn),
        ("выключи ", crate::core::devices::DeviceAction::TurnOff),
        ("выключить ", crate::core::devices::DeviceAction::TurnOff),
        ("выкл ", crate::core::devices::DeviceAction::TurnOff),
        ("переключи ", crate::core::devices::DeviceAction::Toggle),
        ("переключить ", crate::core::devices::DeviceAction::Toggle),
        ("turn on ", crate::core::devices::DeviceAction::TurnOn),
        ("turn off ", crate::core::devices::DeviceAction::TurnOff),
    ] {
        if let Some(query) = text.strip_prefix(prefix) {
            return Some((query.trim(), action));
        }
    }

    for (suffix, action) in [
        (" вкл", crate::core::devices::DeviceAction::TurnOn),
        (" включи", crate::core::devices::DeviceAction::TurnOn),
        (" выкл", crate::core::devices::DeviceAction::TurnOff),
        (" выключи", crate::core::devices::DeviceAction::TurnOff),
        (" переключить", crate::core::devices::DeviceAction::Toggle),
        (" toggle", crate::core::devices::DeviceAction::Toggle),
        (" on", crate::core::devices::DeviceAction::TurnOn),
        (" off", crate::core::devices::DeviceAction::TurnOff),
    ] {
        if let Some(query) = text.strip_suffix(suffix) {
            return Some((query.trim(), action));
        }
    }
    None
}

pub(crate) async fn find_camera_for_text(
    user_id: u64,
    query: &str,
    config: &Arc<AppConfig>,
) -> Result<db::cameras::Camera> {
    let query = query.trim().trim_start_matches("камера").trim();
    let cameras =
        db::cameras::list_accessible_cameras(user_id, user_id == config.root_user, &config.db)
            .await?;

    if let Ok(id) = query.trim_start_matches('#').parse::<i64>() {
        if let Some(camera) = cameras.into_iter().find(|camera| camera.id == id) {
            return Ok(camera);
        }
        anyhow::bail!("камера #{} не найдена или недоступна", id);
    }

    let query = normalize_command_text(query);
    let matches = cameras
        .into_iter()
        .filter(|camera| normalize_command_text(&camera.name).contains(&query))
        .collect::<Vec<_>>();

    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one camera match")),
        0 => anyhow::bail!("камера не найдена"),
        _ => anyhow::bail!("найдено несколько камер, уточните название или ID"),
    }
}

pub(crate) async fn find_device_for_text(
    user_id: u64,
    is_admin: bool,
    query: &str,
    config: &Arc<AppConfig>,
) -> Result<crate::core::types::Device> {
    let query = normalize_command_text(query);
    if query.is_empty() {
        anyhow::bail!("пустое имя устройства");
    }

    let rows = sqlx::query(
        r#"
        SELECT d.id, d.entity_id, d.alias, r.area AS room_area, r.alias AS room_alias
        FROM devices d
        LEFT JOIN rooms r ON r.id = d.room_id
        WHERE d.archived = 0
        ORDER BY d.alias, d.entity_id
        "#,
    )
    .fetch_all(&config.db)
    .await?;

    let mut matches = Vec::new();
    for row in rows {
        let device = crate::core::types::Device {
            id: row.get("id"),
            entity_id: row.get("entity_id"),
            alias: row.get("alias"),
        };
        if !db::access::can_view_device(user_id, is_admin, device.id, &config.db).await? {
            continue;
        }
        let alias = device.alias.as_deref().unwrap_or(&device.entity_id);
        let room_area: Option<String> = row.get("room_area");
        let room_alias: Option<String> = row.get("room_alias");
        let haystack = normalize_command_text(&format!(
            "{} {} {} {}",
            alias,
            device.entity_id,
            room_area.as_deref().unwrap_or_default(),
            room_alias.as_deref().unwrap_or_default()
        ));
        if device_text_matches(&query, &haystack) {
            matches.push(device);
        }
    }

    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one device match")),
        0 => anyhow::bail!("устройство не найдено"),
        _ => anyhow::bail!("найдено несколько устройств, уточните название"),
    }
}

pub(crate) fn device_text_matches(query: &str, haystack: &str) -> bool {
    if haystack.contains(query) {
        return true;
    }

    let tokens = query_tokens(query);
    !tokens.is_empty() && tokens.iter().all(|token| token_matches(token, haystack))
}

fn query_tokens(query: &str) -> Vec<&str> {
    query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty() && !is_query_stop_word(token))
        .collect()
}

fn is_query_stop_word(token: &str) -> bool {
    matches!(
        token,
        "в" | "во" | "на" | "у" | "с" | "со" | "и" | "the" | "a"
    )
}

fn token_matches(token: &str, haystack: &str) -> bool {
    if haystack.contains(token) {
        return true;
    }

    if token.chars().count() < 5 {
        return false;
    }

    for suffix in [
        "ой", "ом", "ей", "ым", "им", "ах", "ях", "е", "и", "ы", "у", "ю", "я",
    ] {
        if let Some(stem) = token.strip_suffix(suffix) {
            if stem.chars().count() >= 4 && haystack.contains(stem) {
                return true;
            }
        }
    }

    fuzzy_token_matches(token, haystack)
}

fn fuzzy_token_matches(token: &str, haystack: &str) -> bool {
    let token_len = token.chars().count();
    if token_len < 4 {
        return false;
    }

    query_tokens(haystack)
        .into_iter()
        .filter(|candidate| {
            let candidate_len = candidate.chars().count();
            candidate_len >= 4 && token_len.abs_diff(candidate_len) <= 2
        })
        .any(|candidate| tokens_are_similar(token, candidate))
}

fn tokens_are_similar(left: &str, right: &str) -> bool {
    let min_len = left.chars().count().min(right.chars().count());
    let max_distance = if min_len <= 4 { 1 } else { 2 };

    strsim::levenshtein(left, right) <= max_distance || strsim::jaro_winkler(left, right) >= 0.88
}
