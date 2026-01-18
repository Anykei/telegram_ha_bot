use std::sync::Arc;
use serde::{Deserialize, Serialize};
use crate::core::devices::InteractionResult;
use crate::bot::models::View;
use crate::bot::screens::room;
use crate::core::{devices, HeaderItem};
use crate::core::types::RoomViewMode;
use crate::models::AppConfig;


pub struct RenderContext {
    pub user_id: u64,
    pub config: Arc<AppConfig>,
    pub notifications: Vec<HeaderItem>,
    pub is_admin: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "c", content = "v", rename_all = "snake_case")]
pub enum DeviceCmd {
    #[serde(rename = "t")]
    Toggle,
    #[serde(rename = "ton")]
    TurnOn,
    #[serde(rename = "toff")]
    TurnOff,
    #[serde(rename = "s")]
    SetLevel(u8), // Для диммеров: {"c":"set_level","v":75}
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "t", rename_all = "snake_case")] // t - type
pub enum Payload {
    #[default]
    Home,
    #[serde(rename = "c")]
    Control (ControlPayload),
    #[serde(rename = "s")]
    Settings(SettingsPayload),
    #[serde(rename = "a")]
    Admin(AdminPayload),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ControlPayload {
    #[serde(rename = "l")]
    ListRooms,
    #[serde(rename = "r")]
    RoomDetail { room: i64},
    #[serde(rename = "d")]
    DeviceControl { room: i64, device: i64 },
    #[serde(rename = "q")]
    QuickAction { room: i64, device: i64, cmd: DeviceCmd },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SettingsPayload {
    #[serde(rename = "l")]
    ListRooms,
    #[serde(rename = "r")]
    RoomDetail { room: i64 },
    #[serde(rename = "d")]
    DeviceDetail { room: i64, device: i64},
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum AdminPayload {
    #[serde(rename = "l")]
    ListActions,
    #[serde(rename = "lu")]
    ListUsers,
    #[serde(rename = "a")]
    AddUser { id: u32 },
    #[serde(rename = "d")]
    DeleteUser { id: u32 },
}

impl Payload {
    pub fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

pub async fn router(
    payload: Payload,
    user_id: u64,
    config: Arc<AppConfig>,
) -> anyhow::Result<View> {
    let notifications = config.get_header_data(user_id).await;
    // let notifications = super::view::format_header(header_data);
    let is_admin = config.root_user == user_id;

    let ctx = RenderContext {
        user_id,
        config: config.clone(),
        notifications,
        is_admin,
    };

    match payload {
        Payload::Home {} => {
            Ok(super::screens::home::render(ctx).await?)
        }
        Payload::Control (sub_payload) => {
            Ok(router_control(ctx, sub_payload).await?)
        }
        Payload::Settings(sub_payload) => {
            Ok(router_settings(ctx, sub_payload).await?)
        }
        _ => {
            Ok(super::screens::common::default_menu(ctx).await?)
        }
    }
}

async fn router_control(ctx: RenderContext, payload: ControlPayload) -> anyhow::Result<View> {
    match payload {
        ControlPayload::ListRooms => Ok(super::screens::rooms::render(ctx, RoomViewMode::Control).await?),
        ControlPayload::RoomDetail {room} => Ok(room::render(ctx, room, RoomViewMode::Control).await?),
        ControlPayload::QuickAction { room, device, cmd } => {
            // Вызываем бизнес-логику одной строкой
            let result = devices::handle_device_interaction(&ctx.config, device, cmd.clone()).await?;

            // Роутер только решает, какой экран показать на основе результата
            match result {
                InteractionResult::RefreshRoom => {
                    Ok(room::render(ctx, room, RoomViewMode::Control).await?)
                }
                InteractionResult::OpenDetails => {
                    Ok(super::screens::control::device_detail::render(ctx, room, device, cmd).await?)
                }
                InteractionResult::Error { error: e } => {
                    Ok(View{
                        alert: Option::from(e),
                        ..std::default::Default::default()
                    })
                }
            }
        }
        _ => {
            Ok(super::screens::common::default_menu(ctx).await?)
        }
        // ControlPayload::DeviceDetail {id} => Ok(super::screens::common::default_menu(ctx).await?),
    }
}

async fn router_settings(ctx: RenderContext, payload: SettingsPayload) -> anyhow::Result<View> {
    match payload {
        SettingsPayload::ListRooms =>  Ok(super::screens::rooms::render(ctx, RoomViewMode::Settings).await?),
        SettingsPayload::RoomDetail {room} => Ok(room::render(ctx, room, RoomViewMode::Settings).await?),
        _ => {
            Ok(super::screens::common::default_menu(ctx).await?)
        }
        // SettingsPayload::DeviceDetail {id} =>  Ok(super::screens::common::default_menu(ctx).await?),
    }
}