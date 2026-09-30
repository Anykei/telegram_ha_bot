use crate::{core::types::DisplaySource, db, models::AppConfig};
use anyhow::Result;

/// Check the resources captured when text, image or button labels were read.
/// Callers separately retain and validate the original admission generation.
pub(crate) async fn sources_allowed(
    sources: &[DisplaySource],
    user_id: u64,
    config: &AppConfig,
) -> Result<bool> {
    if !db::user_exists(user_id, &config.db).await {
        return Ok(false);
    }
    for source in sources {
        if !source_allowed(source, user_id, config).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn source_allowed(source: &DisplaySource, user_id: u64, config: &AppConfig) -> Result<bool> {
    let is_admin = user_id == config.root_user;
    let pool = &config.db;
    Ok(match source {
        DisplaySource::Device { id, entity_id } => {
            db::devices::get_device_by_id(*id, pool)
                .await?
                .is_some_and(|device| device.entity_id == *entity_id)
                && db::access::can_view_device(user_id, is_admin, *id, pool).await?
        }
        DisplaySource::Camera { id } => {
            db::cameras::get_accessible_camera(user_id, is_admin, *id, pool)
                .await?
                .is_some()
        }
        DisplaySource::Room { id } => {
            db::rooms::get_room_by_id(*id, pool).await?.is_some()
                && db::rooms::can_open_room(user_id, is_admin, *id, pool).await?
        }
        DisplaySource::ActionGroup { id } => db::action_groups::get_action_group(*id, pool)
            .await?
            .is_some_and(|group| {
                is_admin || (group.is_enabled() && group.is_visible_to_all_users())
            }),
        DisplaySource::RecordingGroup { id } => {
            db::camera_recording_rule_groups::get_group(*id, pool)
                .await?
                .is_some_and(|group| is_admin || group.is_visible_to_all_users())
        }
        DisplaySource::HaNative { id, entity_id } => {
            db::action_groups::get_ha_native_target(*id, pool)
                .await?
                .is_some_and(|target| {
                    target.entity_id == *entity_id
                        && (is_admin
                            || (target.is_enabled()
                                && !target.is_archived()
                                && target.is_visible_to_all_users()))
                })
        }
    })
}
