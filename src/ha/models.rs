use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Entity {
    pub entity_id: String,
    #[serde(default)]
    pub name: String,
    pub state: String,
    pub device_class: Option<String>,
    pub friendly_name: Option<String>, 
}

#[derive(Deserialize, Debug, Clone)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub entities: Vec<Entity>,
}

#[derive(Default, Deserialize, Debug, Clone)]
pub struct NotifyEvent {
    pub entity_id: String,
    pub old_state: String,
    pub new_state: String,
    pub friendly_name: String,
    pub device_class: Option<String>
}