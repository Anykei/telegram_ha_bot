use crate::bot::models::View;
use crate::bot::router::{DeviceCmd, RenderContext};

pub async fn render(ctx: RenderContext, room_id: i64,device_id: i64, cmd: DeviceCmd,) -> anyhow::Result<View> {

    Ok(crate::bot::screens::common::default_menu(ctx).await?)
}
