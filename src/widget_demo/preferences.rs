//! Demo preference storage, separate from credentials and the production DB.
use super::model::InterfaceMode;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u8,
    mode: InterfaceMode,
}

pub(super) fn path(options: &Path, bot: u64, user: u64) -> PathBuf {
    options.with_file_name(format!("widget-demo-interface-{bot}-{user}.json"))
}

pub(super) async fn load(path: &Path) -> Result<InterfaceMode> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InterfaceMode::default())
        }
        Err(error) => return Err(error).context("Cannot read interface preference"),
    };
    let preferences: Preferences =
        serde_json::from_slice(&bytes).context("Invalid interface preference")?;
    anyhow::ensure!(
        preferences.version == 1,
        "Unsupported interface preference version"
    );
    Ok(preferences.mode)
}

pub(super) async fn save(path: &Path, mode: InterfaceMode) -> Result<()> {
    let bytes = serde_json::to_vec(&Preferences { version: 1, mode })?;
    let temporary = path.with_extension("json.tmp");
    tokio::fs::write(&temporary, bytes)
        .await
        .context("Cannot write interface preference")?;
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).context("Cannot save interface preference");
    }
    Ok(())
}
