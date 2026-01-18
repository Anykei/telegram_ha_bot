use std::sync::Arc;
use anyhow::{Context, Result};
use crate::bot::router::DeviceCmd;
use crate::ha::HAClient;
use crate::ha::models::Entity;
use crate::models::AppConfig;

#[derive(Debug)]
pub enum InteractionResult {
    RefreshRoom,   // Команда выполнена (напр. свет переключен), нужно обновить список устройств
    OpenDetails,   // Действие не выполнилось, так как нужно открыть расширенное меню (детали)
    Error {error: String },
}

#[async_trait::async_trait]
pub trait SmartEntity {
    fn get_info(&self) -> (&Entity, &str);
    fn render_button_text(&self, alias: &str) -> String;
    fn render_button_text_with_state(&self, alias: &str) -> String;
    // Выполняет логику при нажатии (QuickAction)
    async fn on_click(&self, ha: &HAClient, cmd: DeviceCmd) -> InteractionResult;
}

pub enum SmartDevice {
    Switch(Entity),
    Light(Entity),
    Climate(Entity),
    Unknown(Entity),
}

impl SmartDevice {
    pub fn new(entity: Entity) -> Self {
        let domain = entity.entity_id.split('.').next().unwrap_or("");
        match domain {
            "switch" => Self::Switch(entity),
            "light"  => Self::Light(entity),
            "climate" => Self::Climate(entity),
            _ => Self::Unknown(entity),
        }
    }
}

#[async_trait::async_trait]
impl SmartEntity for SmartDevice {

    fn get_info(&self) -> (&Entity, &str) {
        match self {
            Self::Light(e) => (e, "light"),
            Self::Switch(e) => (e, "switch"),
            Self::Climate(e) => (e, "climate"),
            Self::Unknown(e) => {
                let d = e.entity_id.split('.').next().unwrap_or("unknown");
                (e, d)
            },
        }
    }

    fn render_button_text(&self, alias: &str) -> String {
        let (entity, domain) = self.get_info();
        let class = entity.device_class.as_deref().unwrap_or("");

        crate::core::presentation::StateFormatter::format_device_label(
            alias,
            domain,
            class,
            &entity.state
        )
    }

    fn render_button_text_with_state(&self, alias: &str) -> String {
        let (entity, domain) = self.get_info();
        let class = entity.device_class.as_deref().unwrap_or("");

        crate::core::presentation::StateFormatter::format_device_label_with_state(
            alias,
            domain,
            class,
            &entity.state
        )
    }

    async fn on_click(&self, ha: &HAClient, cmd: DeviceCmd) -> InteractionResult {
        match self {
            Self::Light(e) | Self::Switch(e) => {
                let _ = ha.toggle(&e.entity_id).await;
                InteractionResult::RefreshRoom
            }
            Self::Climate(_) => InteractionResult::OpenDetails,
            _ => InteractionResult::RefreshRoom,
        }
    }
}

pub async fn handle_device_interaction(
    config: &Arc<AppConfig>,
    device_id: i64,
    cmd: DeviceCmd,
) -> Result<InteractionResult> {
    let dev_db = crate::db::devices::get_device_by_id(device_id, &config.db)
        .await?
        .context("Device not found in database")?;

    let ha_state = config.ha_client
        .fetch_states_by_ids(&[dev_db.entity_id])
        .await?
        .into_iter()
        .next()
        .context("HA state not found")?;

    let smart_obj = SmartDevice::new(ha_state);
    let res = smart_obj.on_click(&config.ha_client, cmd).await;
    if matches!(res, InteractionResult::RefreshRoom) {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Ok(res)
}