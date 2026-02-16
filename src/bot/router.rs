use std::sync::Arc;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use crate::core::devices::{ChartParams, InputIntent, InteractionResult};
use crate::bot::models::View;
use crate::bot::screens::room;
use crate::core::{devices, HeaderItem};
use crate::core::types::RoomViewMode;
use crate::models::AppConfig;

use postcard;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use teloxide::types::InlineKeyboardMarkup;

pub struct RenderContext {
    pub user_id: u64,
    pub config: Arc<AppConfig>,
    pub notifications: Vec<HeaderItem>,
    pub is_admin: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    WaitingForName { device_id: i64, room_id: i64 },
    WaitingForStateAlias { device_id: i64, original_state: String, room_id: i64 },
    WaitingForGraphInterval { device_id: i64, room_id: i64 },
    BackupDb { path: String },
    AddUser { user_id: i64 },
    DeleteUser { user_id: i64 },
}

impl State {
    /// Создает стейт из интента, обогащая его необходимыми ID.
    pub fn from_intent(intent: InputIntent, device_id: i64, room_id: i64) -> Self {
        match intent {
            InputIntent::DefineGraphInterval { .. } =>
                State::WaitingForGraphInterval { device_id, room_id },
            InputIntent::RenameDevice { .. } =>
                State::WaitingForName { device_id, room_id },
            InputIntent::SetStateAlias { original_state, .. } =>
                State::WaitingForStateAlias { device_id, original_state, room_id },
        }
    }
}

impl From<devices::InputIntent> for State {
    fn from(intent: devices::InputIntent) -> Self {
        use crate::core::devices::InputIntent;

        match intent {
            InputIntent::RenameDevice { device_id, room_id } =>
                State::WaitingForName { device_id, room_id },
            InputIntent::SetStateAlias { device_id, room_id, original_state } =>
                State::WaitingForStateAlias { device_id, original_state, room_id },
            InputIntent::DefineGraphInterval { device_id, room_id } =>
                State::WaitingForGraphInterval { device_id, room_id },
        }
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DeviceCmd {
    #[default]
    Toggle,
    TurnOn,
    TurnOff,
    SetLevel(u8),
    SetTemp(f32),
    ShowChart {
        h: u32,
        o: i32,
    },
    EnterManualInput,
}

impl From<DeviceCmd> for devices::DeviceAction {
    fn from(cmd: DeviceCmd) -> Self {
        use crate::core::devices::DeviceAction;
        match cmd {
            DeviceCmd::Toggle => DeviceAction::Toggle,
            DeviceCmd::TurnOn => DeviceAction::TurnOn,
            DeviceCmd::TurnOff => DeviceAction::TurnOff,
            DeviceCmd::SetLevel(v) => DeviceAction::SetLevel(v),
            DeviceCmd::SetTemp(v) => DeviceAction::SetTemperature(v),
            DeviceCmd::ShowChart { h, o } => DeviceAction::GenerateChart(ChartParams { period_hours: h, offset_hours: o }),
            DeviceCmd::EnterManualInput => DeviceAction::EnterManualInput,
        }
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Payload {
    #[default]
    Home,
    Control(ControlPayload),
    Settings(SettingsPayload),
    Admin(AdminPayload),
    InDev
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ControlPayload {
    ListRooms,
    RoomDetail {
        room: i64
    },
    DeviceControl {
        room: i64,
        device: i64
    },
    QuickAction {
        room: i64,
        device: i64,
        cmd: DeviceCmd,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum SettingsPayload {
    ListRooms,
    RoomDetail { room: i64 },
    DeviceDetail { room: i64, device: i64},
    ToggleNotify {
        room: i64,
        device: i64
    },
    ToggleHide {
        room: i64,
        device: i64
    },
    EditName {
        room: i64,
        device: i64
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AdminPayload {
    ListActions,
    ListUsers,
    AddUser { id: u32 },
    DeleteUser { id: u32 },
}

impl Payload {
    pub fn to_string_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    // pub fn from_str(s: &str) -> Self {
    //     serde_json::from_str(s)
    //         .map_err(|e| { error!("Parse error: {}", e); e })
    // }
    //
    /// Сериализация в компактную Base64 строку.
    /// JSON (67 байт) -> Binary (~12 байт) -> Base64 (~16 символов).
    pub fn to_string(&self) -> String {
        match postcard::to_allocvec(self) {
            Ok(bin) => B64.encode(bin),
            Err(e) => {
                log::error!("Serialization failed: {}", e);
                String::new()
            }
        }
    }

    pub fn from_string(s: &str) -> Result<Self, anyhow::Error> {
        let bin = B64.decode(s).map_err(|e| {
            anyhow::anyhow!("Base64 decode failed for '{}': {}", s, e)
        })?;

        postcard::from_bytes(&bin).map_err(|e| {
            // Google Standard: Детальный лог ошибки десериализации
            anyhow::anyhow!("Binary decode failed. Bytes: {:?}, Error: {}", bin, e)
        })
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

    info!("ROUTER CALL: user_id={}, payload {}", user_id, payload.to_string());

    let ctx = RenderContext {
        user_id,
        config: config.clone(),
        notifications,
        is_admin
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
        Payload::InDev {} => {
            Ok(super::screens::common::in_dev_menu(ctx, Payload::Home).await?)
        }
        _ => {
            Ok(super::screens::common::in_dev_menu(ctx, Payload::Home).await?)
        }
    }
}

async fn router_control(ctx: RenderContext, payload: ControlPayload) -> anyhow::Result<View> {
    match payload {
        ControlPayload::ListRooms => Ok(super::screens::rooms::render(ctx, RoomViewMode::Control).await?),
        ControlPayload::RoomDetail {room} => Ok(room::render(ctx, room, RoomViewMode::Control).await?),
        ControlPayload::QuickAction {room, device, cmd } => {
            let action = devices::DeviceAction::from(cmd.clone());

            let result = devices::handle_device_interaction(&ctx.config, device, action).await?;

            match result {
                InteractionResult::Processed => {
                    Ok(room::render(ctx, room, RoomViewMode::Control).await?)
                }
                InteractionResult::RequiresDetail => {
                    Ok(super::screens::control::device_control::render(ctx, room, device, cmd).await?)
                }
                InteractionResult::RequiresInput(intent) => {
                    let state = State::from_intent(intent, device, room);
                    Ok(super::screens::control::sensor_view::render_manual_input(room, device, state))
                }
                InteractionResult::Error { error: e } => {
                    Ok(View{
                        alert: Option::from(e),
                        ..Default::default()
                    })
                }
            }
        }
        _ => {
            Ok(super::screens::common::in_dev_menu(ctx, Payload::Control(ControlPayload::ListRooms {})).await?)
        }
    }
}

async fn router_settings(ctx: RenderContext, payload: SettingsPayload) -> anyhow::Result<View> {
    use crate::db;
    match payload {
        SettingsPayload::ListRooms =>  Ok(super::screens::rooms::render(ctx, RoomViewMode::Settings).await?),
        SettingsPayload::RoomDetail {room} => Ok(room::render(ctx, room, RoomViewMode::Settings).await?),
        SettingsPayload::DeviceDetail {room, device} => Ok(super::screens::settings::device_settings::render(ctx, room, device).await?),
        SettingsPayload::ToggleNotify { room, device } => {
            let dev = db::devices::get_device_by_id(device, &ctx.config.db).await?.context("Device not found")?;
            db::subscriptions::toggle_subscription(ctx.user_id as i64, &dev.entity_id, &ctx.config.db).await?;
            super::screens::settings::device_settings::render(ctx, room, device).await
        }

        SettingsPayload::ToggleHide { room, device } => {
            let dev = db::devices::get_device_by_id(device, &ctx.config.db).await?.context("Device not found")?;
            db::subscriptions::toggle_hidden(&dev.entity_id, &ctx.config.db).await?;
            super::screens::settings::device_settings::render(ctx, room, device).await
        }
        _ => {
            Ok(super::screens::common::in_dev_menu(ctx, Payload::Settings(SettingsPayload::ListRooms {})).await?)
        }
    }
}

#[cfg(test)]
mod tests {
    use dashmap::DashMap;
    use crate::{db, ha};
    use crate::config::EnvPaths;
    use super::*;

    #[test]
    fn test_payload_integrity_and_size() {
        let original = Payload::Control(ControlPayload::QuickAction {
            room: 1_000_000,
            device: 2_000_000,
            cmd: DeviceCmd::ShowChart { h: 168, o: -168 },
        });

        let encoded = original.to_string();
        let len = encoded.len();

        println!("Binary/B64 Buffer use: {}/64 bytes", len);
        println!("Encoded String: {}", encoded);

        assert!(len > 0, "Encoded string should not be empty");
        assert!(len <= 64, "🛑 Payload overflow: {} bytes used. Max is 64.", len);

        let restored = Payload::from_string(&encoded)
            .expect("Failed to decode payload from Base64/Binary");

        assert_eq!(restored, original, "Data corruption: restored payload differs from original");
    }

    #[tokio::test]
    async fn test_sensor_render_preserves_payload_context() -> anyhow::Result<()> {
        let encoded_input = "AQMENAUYAA";
        let original_payload = Payload::from_string(encoded_input)
            .expect("Failed to decode test payload");

        let paths = EnvPaths::load()
            .validate()
            .context("Error checking env variables.")?;

        let db_pool = db::init(&paths.db_url(), paths.migrations.to_str().context("Путь к миграциям не валиден")?)
            .await
            .context("Error initializing database pool.")?;

        let ha_client = Arc::new(ha::init(paths.ha_url.clone(), paths.ha_token.clone()));

        let app_config = Arc::new(AppConfig {
            ha_client: ha_client.clone(),
            db: db_pool,
            root_user: 0,

            // delete_chart_timeout_s: 600,
            // delete_help_messages_timeout_s: 30,
            delete_notification_messages_timeout_s: 5,
            // delete_error_messages_timeout_s: 5,
            ttl_notifications: 1,
            background_maintenance_interval_s:15,

            sessions: DashMap::new(),

            name_aliases: DashMap::new(),

            state_aliases: DashMap::new(),
        });

        let user_id = 219791289;

        let view = router(original_payload.clone(), user_id, app_config).await?;

        assert_eq!(
            view.payload,
            original_payload,
            "Context mismatch! The screen 'downgraded' the navigation state.\nExpected: {:?}\nActual: {:?}",
            original_payload,
            view.payload
        );

        Ok(())
    }
}