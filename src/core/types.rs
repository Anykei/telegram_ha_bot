#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomViewMode {
    Control,
    Settings,
}

#[derive(sqlx::FromRow, Debug)]
pub struct Device {
    pub id: i64,
    pub room_id: i64,
    pub entity_id: String,
    pub alias: Option<String>,
    pub device_class: String,
    pub device_domain: String,
}