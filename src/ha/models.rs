use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Entity {
    pub entity_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub device_class: Option<String>,
    #[serde(default)]
    pub attributes: EntityAttributes,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct EntityAttributes {
    pub hvac_modes: Option<Vec<String>>,
    pub current_temperature: Option<f32>,
    pub temperature: Option<f32>,
    pub target_temp_low: Option<f32>,
    pub target_temp_high: Option<f32>,
    pub target_temp_step: Option<f32>,
    pub min_temp: Option<f32>,
    pub max_temp: Option<f32>,
    pub unit_of_measurement: Option<String>,
}

impl Entity {
    pub fn supports_hvac_mode(&self, mode: &str) -> bool {
        self.entity_id.starts_with("climate.")
            && !matches!(self.state.as_str(), "" | "unknown" | "unavailable")
            && self
                .attributes
                .hvac_modes
                .as_ref()
                .is_some_and(|modes| modes.iter().any(|value| value == mode))
    }

    pub fn temperature_target_is_valid(&self, target: f32) -> bool {
        !matches!(self.state.as_str(), "unavailable" | "unknown" | "heat_cool")
            && self.attributes.temperature.is_some_and(f32::is_finite)
            && target.is_finite()
            && self
                .attributes
                .min_temp
                .is_some_and(|min| min.is_finite() && target >= min)
            && self
                .attributes
                .max_temp
                .is_some_and(|max| max.is_finite() && target <= max)
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub entities: Vec<Entity>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct NotifyEvent {
    pub entity_id: String,
    pub old_state: String,
    pub new_state: String,
    pub friendly_name: String,
    pub device_class: Option<String>,
    /// Local receipt time, captured before queueing or fetching rule context.
    #[serde(default = "chrono::Utc::now")]
    pub received_at: chrono::DateTime<chrono::Utc>,
}

impl Default for NotifyEvent {
    fn default() -> Self {
        Self {
            entity_id: String::new(),
            old_state: String::new(),
            new_state: String::new(),
            friendly_name: String::new(),
            device_class: None,
            received_at: chrono::Utc::now(),
        }
    }
}
