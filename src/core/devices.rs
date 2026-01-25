use std::sync::Arc;
use anyhow::{Context, Result};
use crate::ha::HAClient;
use crate::ha::models::Entity;
use crate::models::AppConfig;

#[derive(Debug, Clone)]
pub enum InputIntent {
    RenameDevice { device_id: i64, room_id: i64 },
    SetStateAlias { device_id: i64, room_id: i64, original_state: String },
    DefineGraphInterval { device_id: i64, room_id: i64 },
}

#[derive(Debug)]
pub enum InteractionResult {
    Processed,
    RequiresDetail,
    RequiresInput(InputIntent),
    Error {error: String },
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceAction {
    Toggle,
    TurnOn,
    TurnOff,
    SetLevel(u8),
    SetTemperature(f32),
    GenerateChart(ChartParams),
    EnterManualInput,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartParams {
    pub period_hours: u32,
    pub offset_hours: i32, // 0 - текущее время, -24 - вчера и т.д.
}

#[async_trait::async_trait]
pub trait SmartEntity {
    fn get_info(&self) -> (&Entity, &str);
    fn render_button_text(&self, alias: &str) -> String;
    fn render_button_text_with_state(&self, alias: &str) -> String;
    // Выполняет логику при нажатии (QuickAction)
    async fn on_click(&self, ha: &HAClient, action: DeviceAction) -> InteractionResult;
}

pub enum SmartDevice {
    Switch(Entity),
    Light(Entity),
    Climate(Entity),
    Sensor(Entity),
    BinarySensor(Entity),
    Number(Entity),
    Unknown(Entity),
}

impl SmartDevice {
    pub fn new(entity: Entity) -> Self {
        let domain = entity.entity_id.split('.').next().unwrap_or("");
        match domain {
            "switch" => Self::Switch(entity),
            "light"  => Self::Light(entity),
            "climate" => Self::Climate(entity),
            "sensor" => Self::Sensor(entity),
            "binary_sensor" => Self::BinarySensor(entity),
            "number" => Self::Number(entity),
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
            Self::Sensor(e) => (e, "sensor"),
            Self::BinarySensor(e) => (e, "binary_sensor"),
            Self::Number(e) => (e, "number"),
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

    async fn on_click(&self, ha: &HAClient, action: DeviceAction) -> InteractionResult {
        let (entity, domain) = self.get_info();
        let entity_id = &entity.entity_id;

        match self {
            Self::Light(_) | Self::Switch(_) => {
                let service = match action {
                    DeviceAction::Toggle => "toggle",
                    DeviceAction::TurnOn => "turn_on",
                    DeviceAction::TurnOff => "turn_off",
                    DeviceAction::SetLevel(v) => {
                        let data = serde_json::json!({ "brightness": v });
                        return if ha.call_service_with_data(domain, "turn_on", entity_id, data).await.is_ok() {
                            InteractionResult::Processed
                        } else {
                            InteractionResult::Error { error: "Failed to set brightness".into() }
                        };
                    }
                    _ => "toggle",
                };

                if ha.call_service(domain, service, entity_id).await.is_ok() {
                    InteractionResult::Processed
                } else {
                    InteractionResult::Error { error: "HA Service Call Failed".into() }
                }
            }

            Self::Climate(_) => {
                match action {
                    DeviceAction::SetTemperature(tmp) => {
                        let data = serde_json::json!({ "temperature": tmp });
                        if ha.call_service_with_data("climate", "set_temperature", entity_id, data).await.is_ok() {
                            InteractionResult::RequiresDetail
                        } else {
                            InteractionResult::Error { error: "Failed to set temperature".into() }
                        }
                    }
                    DeviceAction::Toggle => {
                        let _ = ha.call_service("climate", "toggle", entity_id).await;
                        InteractionResult::Processed
                    }
                    _ => InteractionResult::RequiresDetail,
                }
            }
            Self::Sensor(_) | Self::BinarySensor(_) => {
                match action {
                    DeviceAction::GenerateChart(_) => InteractionResult::RequiresDetail,
                    DeviceAction::EnterManualInput => {
                        InteractionResult::RequiresInput(InputIntent::DefineGraphInterval {
                            device_id: 0,
                            room_id: 0
                        })
                    }
                    _ => InteractionResult::RequiresDetail,
                }
            }

            Self::Number(_) => InteractionResult::RequiresDetail,

            Self::Unknown(e) => {
                let _ = ha.call_service(domain, "toggle", &e.entity_id).await;
                InteractionResult::Processed
            }
        }
    }
}

pub async fn handle_device_interaction(
    config: &Arc<AppConfig>,
    device_id: i64,
    action: DeviceAction,
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
    let res = smart_obj.on_click(&config.ha_client, action).await;
    if matches!(res, InteractionResult::Processed) {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Ok(res)
}