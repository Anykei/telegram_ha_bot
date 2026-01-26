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
}

impl AppOptions {
    /// Загружает и валидирует файл конфигурации.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        ensure!(path.exists(), "Configuration file not found: {:?}", path);

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read options file: {:?}", path))?;

        let options: AppOptions = serde_json::from_str(&content)
            .context("JSON schema mismatch in options file")?;

        // Бизнес-валидация
        ensure!(!options.bot_token.is_empty(), "bot_token cannot be empty");
        ensure!(options.root_user != 0, "root_user must be a valid Telegram ID");

        Ok(options)
    }
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

/// Если тебе нужно будет вернуть allowed_users (список через запятую), используй этот вариант:
fn deserialize_ids<'de, D>(deserializer: D) -> std::result::Result<Vec<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringIntOrList {
        Str(String),
        Int(u64),
        List(Vec<u64>),
    }

    match StringIntOrList::deserialize(deserializer)? {
        StringIntOrList::Int(i) => Ok(vec![i]),
        StringIntOrList::List(l) => Ok(l),
        StringIntOrList::Str(s) => s
            .split(',')
            .map(|part| part.trim().parse::<u64>().map_err(serde::de::Error::custom))
            .collect(),
    }
}