use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Entity {
    pub entity_id: String,
    pub name: String,
    pub state: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Room {
    pub name: String,
    pub entities: Vec<Entity>,
}

pub struct NotifyEvent {
    pub entity_id: String,
    pub old_state: String,
    pub new_state: String,
    pub friendly_name: String,
}