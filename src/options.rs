use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Deserializer};
use std::fs;
use std::path::Path;

#[derive(Deserialize, Debug, Clone)]
pub struct AppOptions {
    pub bot_token: String,
    pub root_user: u64,

    // #[serde(deserialize_with = "deserialize_ids")]
    // pub allowed_users: Vec<u64>,
}

impl AppOptions {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        ensure!(path.exists(), "File options not found: {:?}", path);

        let content = fs::read_to_string(path)
            .with_context(|| format!("File options read failed: {:?}", path))?;

        let options: AppOptions = serde_json::from_str(&content)
            .context("Parsing error JSON options file")?;

        ensure!(!options.bot_token.is_empty(), "Options not valid: bot_token empty");
        ensure!(options.root_user != 0, "Options not valid: root_user not set (0)");
        Ok(options)
    }
}

fn deserialize_ids<'de, D>(deserializer: D) -> Result<Vec<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrInt {
        Str(String),
        Int(u64),
    }

    let raw_value = StringOrInt::deserialize(deserializer)?;

    let s = match raw_value {
        StringOrInt::Str(v) => v,
        StringOrInt::Int(v) => v.to_string(),
    };

    s.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().map_err(serde::de::Error::custom))
        .collect()
}