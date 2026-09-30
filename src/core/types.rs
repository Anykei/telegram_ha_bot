#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomViewMode {
    Control,
    Settings,
}

#[derive(sqlx::FromRow, Debug)]
pub struct Device {
    pub id: i64,
    pub entity_id: String,
    pub alias: Option<String>,
}

/// Identity of data already embedded in a rendered screen or its keyboard.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum DisplaySource {
    Device { id: i64, entity_id: String },
    Camera { id: i64 },
    Room { id: i64 },
    ActionGroup { id: i64 },
    HaNative { id: i64, entity_id: String },
    RecordingGroup { id: i64 },
}
