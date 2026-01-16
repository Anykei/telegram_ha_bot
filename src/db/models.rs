use chrono::{DateTime, Utc};
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct AggregatedAlert {
    pub entity_id: String,
    pub last_state: String,
    pub event_count: i64,
    pub last_updated: DateTime<Utc>,
}