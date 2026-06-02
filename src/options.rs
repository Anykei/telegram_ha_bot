use crate::i18n::Language;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Deserializer};
use std::fs;
use std::path::Path;

/// Настройки приложения.
/// Соответствует Google Rust Style Guide: использование документационных комментариев и явная типизация.
#[derive(Deserialize, Debug, Clone)]
pub struct AppOptions {
    pub bot_token: String,

    /// ID владельца бота. Может прийти как число или как строка в кавычках.
    #[serde(deserialize_with = "flexible_u64")]
    pub root_user: u64,

    /// Как часто фоновый worker обновляет открытые экраны и служебные данные.
    #[serde(default = "default_background_maintenance_interval_s")]
    pub background_maintenance_interval_s: u64,

    /// Минимальная пауза между live refresh от событий Home Assistant.
    #[serde(default = "default_event_refresh_min_interval_s")]
    pub event_refresh_min_interval_s: u64,

    /// Сколько часов хранить открытую UI-сессию без активности пользователя.
    #[serde(default = "default_session_ttl_hours")]
    pub session_ttl_hours: u64,

    /// Дополнительный запас к Telegram RetryAfter, чтобы не попасть в граничную секунду лимита.
    #[serde(default = "default_telegram_retry_after_extra_delay_s")]
    pub telegram_retry_after_extra_delay_s: u64,

    /// Язык интерфейса по умолчанию для пользователей без персональной настройки.
    #[serde(default)]
    pub default_language: Language,

    /// Доступные длительности коротких видео с камер.
    #[serde(default = "default_camera_clip_intervals_s")]
    pub camera_clip_intervals_s: Vec<u32>,

    /// Длительность ролика по умолчанию для камеры, если она не задана у самой камеры.
    #[serde(default = "default_camera_default_clip_s")]
    pub camera_default_clip_s: u32,

    /// Максимальное продление событийной записи после последнего события.
    #[serde(default = "default_camera_recording_max_tail_seconds")]
    pub camera_recording_max_tail_seconds: u32,

    /// Максимальная длительность одного архивного видеофайла.
    #[serde(default = "default_camera_recording_max_segment_seconds")]
    pub camera_recording_max_segment_seconds: u32,

    /// Максимум событийных записей, которые пишутся одновременно.
    #[serde(default = "default_camera_recording_max_parallel_jobs")]
    pub camera_recording_max_parallel_jobs: usize,

    /// Корневая папка локального архива записей.
    #[serde(default = "default_camera_recording_storage_root")]
    pub camera_recording_storage_root: String,
}

impl AppOptions {
    /// Загружает и валидирует файл конфигурации.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        ensure!(path.exists(), "Configuration file not found: {:?}", path);

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read options file: {:?}", path))?;

        let options: AppOptions =
            serde_json::from_str(&content).context("JSON schema mismatch in options file")?;

        // Бизнес-валидация
        ensure!(!options.bot_token.is_empty(), "bot_token cannot be empty");
        ensure!(
            options.root_user != 0,
            "root_user must be a valid Telegram ID"
        );
        ensure!(
            options.background_maintenance_interval_s > 0,
            "background_maintenance_interval_s must be greater than 0"
        );
        ensure!(
            options.event_refresh_min_interval_s > 0,
            "event_refresh_min_interval_s must be greater than 0"
        );
        ensure!(
            options.session_ttl_hours > 0,
            "session_ttl_hours must be greater than 0"
        );
        ensure!(
            options.telegram_retry_after_extra_delay_s <= 60,
            "telegram_retry_after_extra_delay_s must be 60 seconds or less"
        );
        ensure!(
            !options.camera_clip_intervals_s.is_empty(),
            "camera_clip_intervals_s cannot be empty"
        );
        ensure!(
            options
                .camera_clip_intervals_s
                .iter()
                .all(|seconds| (1..=120).contains(seconds)),
            "camera_clip_intervals_s values must be between 1 and 120 seconds"
        );
        ensure!(
            (1..=120).contains(&options.camera_default_clip_s),
            "camera_default_clip_s must be between 1 and 120 seconds"
        );
        ensure!(
            (5..=3600).contains(&options.camera_recording_max_tail_seconds),
            "camera_recording_max_tail_seconds must be between 5 and 3600 seconds"
        );
        ensure!(
            (30..=3600).contains(&options.camera_recording_max_segment_seconds),
            "camera_recording_max_segment_seconds must be between 30 and 3600 seconds"
        );
        ensure!(
            (1..=16).contains(&options.camera_recording_max_parallel_jobs),
            "camera_recording_max_parallel_jobs must be between 1 and 16"
        );
        ensure!(
            !options.camera_recording_storage_root.trim().is_empty(),
            "camera_recording_storage_root cannot be empty"
        );

        Ok(options)
    }
}

fn default_background_maintenance_interval_s() -> u64 {
    15
}

fn default_event_refresh_min_interval_s() -> u64 {
    5
}

fn default_session_ttl_hours() -> u64 {
    24
}

fn default_telegram_retry_after_extra_delay_s() -> u64 {
    1
}

fn default_camera_clip_intervals_s() -> Vec<u32> {
    vec![5, 10, 15, 30]
}

fn default_camera_default_clip_s() -> u32 {
    10
}

fn default_camera_recording_max_tail_seconds() -> u32 {
    300
}

fn default_camera_recording_max_segment_seconds() -> u32 {
    300
}

fn default_camera_recording_max_parallel_jobs() -> usize {
    4
}

fn default_camera_recording_storage_root() -> String {
    "data/recordings".to_string()
}

/// Гибкий десериализатор для u64.
/// Поддерживает форматы: 12345 и "12345".
fn flexible_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrInt {
        Str(String),
        Int(u64),
    }

    match StringOrInt::deserialize(deserializer)? {
        StringOrInt::Int(i) => Ok(i),
        StringOrInt::Str(s) => s.parse::<u64>().map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_use_refresh_defaults_for_legacy_json() {
        let options: AppOptions = serde_json::from_str(
            r#"{
                "bot_token": "token",
                "root_user": "42"
            }"#,
        )
        .expect("options should parse");

        assert_eq!(options.background_maintenance_interval_s, 15);
        assert_eq!(options.event_refresh_min_interval_s, 5);
        assert_eq!(options.session_ttl_hours, 24);
        assert_eq!(options.telegram_retry_after_extra_delay_s, 1);
        assert_eq!(options.default_language, Language::Ru);
        assert_eq!(options.camera_clip_intervals_s, vec![5, 10, 15, 30]);
        assert_eq!(options.camera_default_clip_s, 10);
        assert_eq!(options.camera_recording_max_tail_seconds, 300);
        assert_eq!(options.camera_recording_max_segment_seconds, 300);
        assert_eq!(options.camera_recording_max_parallel_jobs, 4);
        assert_eq!(options.camera_recording_storage_root, "data/recordings");
    }

    #[test]
    fn options_accept_english_default_language() {
        let options: AppOptions = serde_json::from_str(
            r#"{
                "bot_token": "token",
                "root_user": 42,
                "default_language": "en"
            }"#,
        )
        .expect("options should parse");

        assert_eq!(options.default_language, Language::En);
    }
}
