use crate::bot::models::View;
use crate::bot::router::{DeviceCmd, Payload, RenderContext};
use anyhow::Context;

use crate::core::devices::SmartDevice;

pub async fn render(
    ctx: RenderContext,
    room_id: i64,
    device_id: i64,
    cmd: DeviceCmd // Команда пробрасывается из роутера
) -> anyhow::Result<View> {
    let db = &ctx.config.db;

    // 1. Загружаем данные (как ты и делал)
    let dev_db = crate::db::devices::get_device_by_id(device_id, db).await?
        .context("Device not found")?;
    let ha_ent = ctx.config.ha_client.fetch_states_by_ids(&[dev_db.entity_id.clone()]).await?
        .into_iter().next().context("HA state missing")?;

    let smart_obj = SmartDevice::new(ha_ent);

    // 2. Диспетчеризация отрисовки
    match smart_obj {
        SmartDevice::Sensor(e) | SmartDevice::BinarySensor(e) => {
            super::sensor_view::render(ctx, room_id, dev_db, e, cmd).await
        }
        // SmartDevice::Light(e) => {
        //     super::device_screens::light::render(ctx, room_id, dev_db, e).await
        // }
        // ... другие типы
        _ => crate::bot::screens::common::in_dev_menu(ctx, Payload::Control(crate::bot::router::ControlPayload::RoomDetail {room: room_id})).await
    }
}
