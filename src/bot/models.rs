use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    WaitingForName { entity_id: String, room_name: String },
    WaitingForStateAlias { entity_id: String, original_state: String, room_name: String },
    WaitingForGraphInterval { entity_id: String, room_name: String },
}
