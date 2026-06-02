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

    #[serde(default)]
    pub voice_enabled: bool,

    #[serde(default)]
    pub voice_stt_provider: VoiceSttProvider,

    #[serde(default)]
    pub voice_command_engine: VoiceCommandEngine,

    #[serde(default)]
    pub voice_ha_pipeline_id: Option<String>,

    #[serde(default = "default_voice_stt_sample_rate")]
    pub voice_stt_sample_rate: u32,

    #[serde(default = "default_true")]
    pub voice_confirm_dangerous: bool,

    #[serde(default = "default_voice_pending_ttl_s")]
    pub voice_pending_ttl_s: u64,

    #[serde(default = "default_voice_max_audio_size_mb")]
    pub voice_max_audio_size_mb: u64,

    #[serde(default = "default_voice_max_audio_duration_s")]
    pub voice_max_audio_duration_s: u32,

    #[serde(default = "default_voice_stt_timeout_s")]
    pub voice_stt_timeout_s: u64,

    #[serde(default = "default_true")]
    pub voice_show_recognized_text: bool,

    #[serde(default)]
    pub voice_response_format: VoiceResponseFormat,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSttProvider {
    #[default]
    HaPipeline,
    ExternalStt,
    LocalStt,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCommandEngine {
    #[default]
    LocalParser,
    HaConversationReadonly,
    #[serde(alias = "ha_conversation")]
    HaConversationFull,
}

impl VoiceCommandEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalParser => "local_parser",
            Self::HaConversationReadonly => "ha_conversation_readonly",
            Self::HaConversationFull => "ha_conversation_full",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LocalParser => "локальный",
            Self::HaConversationReadonly => "HA readonly",
            Self::HaConversationFull => "HA full",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "ha_conversation" | "ha_conversation_full" => Self::HaConversationFull,
            "ha_conversation_readonly" => Self::HaConversationReadonly,
            _ => Self::LocalParser,
        }
    }

    pub fn next_for_profile(self) -> Self {
        match self {
            Self::LocalParser => Self::HaConversationReadonly,
            Self::HaConversationReadonly => Self::HaConversationFull,
            Self::HaConversationFull => Self::LocalParser,
        }
    }
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceResponseFormat {
    #[default]
    Text,
    Voice,
    Both,
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
        ensure!(
            (10..=300).contains(&options.voice_pending_ttl_s),
            "voice_pending_ttl_s must be between 10 and 300 seconds"
        );
        ensure!(
            (1..=50).contains(&options.voice_max_audio_size_mb),
            "voice_max_audio_size_mb must be between 1 and 50"
        );
        ensure!(
            (1..=120).contains(&options.voice_max_audio_duration_s),
            "voice_max_audio_duration_s must be between 1 and 120 seconds"
        );
        ensure!(
            (5..=300).contains(&options.voice_stt_timeout_s),
            "voice_stt_timeout_s must be between 5 and 300 seconds"
        );
        ensure!(
            options.voice_stt_sample_rate >= 8000,
            "voice_stt_sample_rate must be at least 8000"
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

fn default_true() -> bool {
    true
}

fn default_voice_stt_sample_rate() -> u32 {
    16_000
}

fn default_voice_pending_ttl_s() -> u64 {
    60
}

fn default_voice_max_audio_size_mb() -> u64 {
    10
}

fn default_voice_max_audio_duration_s() -> u32 {
    30
}

fn default_voice_stt_timeout_s() -> u64 {
    45
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
        assert!(!options.voice_enabled);
        assert_eq!(options.voice_stt_provider, VoiceSttProvider::HaPipeline);
        assert_eq!(
            options.voice_command_engine,
            VoiceCommandEngine::LocalParser
        );
        assert_eq!(options.voice_ha_pipeline_id, None);
        assert_eq!(options.voice_stt_sample_rate, 16_000);
        assert!(options.voice_confirm_dangerous);
        assert_eq!(options.voice_pending_ttl_s, 60);
        assert_eq!(options.voice_max_audio_size_mb, 10);
        assert_eq!(options.voice_max_audio_duration_s, 30);
        assert_eq!(options.voice_stt_timeout_s, 45);
        assert!(options.voice_show_recognized_text);
        assert_eq!(options.voice_response_format, VoiceResponseFormat::Text);
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

    #[test]
    fn options_accept_ha_conversation_alias() {
        let options: AppOptions = serde_json::from_str(
            r#"{
                "bot_token": "token",
                "root_user": 42,
                "voice_command_engine": "ha_conversation"
            }"#,
        )
        .expect("options should parse");

        assert_eq!(
            options.voice_command_engine,
            VoiceCommandEngine::HaConversationFull
        );
    }
}
