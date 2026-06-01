use crate::bot::models::View;
use crate::bot::screens::room;
use crate::core::devices::{ChartParams, InputIntent, InteractionResult};
use crate::core::types::RoomViewMode;
use crate::core::{devices, HeaderItem};
use crate::db;
use crate::models::AppConfig;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use teloxide::types::InlineKeyboardMarkup;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use postcard;

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
    WaitingForName {
        device_id: i64,
        room_id: i64,
    },
    WaitingForStateAlias {
        device_id: i64,
        original_state: String,
        room_id: i64,
    },
    WaitingForGraphInterval {
        device_id: i64,
        room_id: i64,
    },
    BackupDb {
        path: String,
    },
    AddUser {
        user_id: i64,
    },
    DeleteUser {
        user_id: i64,
    },
    AddCamera {
        room_id: i64,
    },
}

impl State {
    /// Создает стейт из интента, обогащая его необходимыми ID.
    pub fn from_intent(intent: InputIntent, device_id: i64, room_id: i64) -> Self {
        match intent {
            InputIntent::DefineGraphInterval { .. } => {
                State::WaitingForGraphInterval { device_id, room_id }
            }
        }
    }
}

impl From<devices::InputIntent> for State {
    fn from(intent: devices::InputIntent) -> Self {
        use crate::core::devices::InputIntent;

        match intent {
            InputIntent::DefineGraphInterval { device_id, room_id } => {
                State::WaitingForGraphInterval { device_id, room_id }
            }
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
            DeviceCmd::ShowChart { h, o } => DeviceAction::GenerateChart(ChartParams {
                period_hours: h,
                offset_hours: o,
            }),
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
    Camera(CameraPayload),
    Admin(AdminPayload),
    InDev,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ControlPayload {
    ListRooms,
    RoomDetail {
        room: i64,
    },
    DeviceControl {
        room: i64,
        device: i64,
    },
    QuickAction {
        room: i64,
        device: i64,
        cmd: DeviceCmd,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum CameraPayload {
    ListCameras,
    CameraDetail { id: i64 },
    Snapshot { id: i64 },
    Clip { id: i64, seconds: u32 },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum SettingsPayload {
    ListRooms,
    RoomDetail { room: i64 },
    DeviceDetail { room: i64, device: i64 },
    ToggleNotify { room: i64, device: i64 },
    ToggleHide { room: i64, device: i64 },
    EditName { room: i64, device: i64 },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AdminPayload {
    ListActions,
    ListUsers,
    Status,
    ConfirmBackup,
    CreateBackup,
    PromptAddUser,
    PromptDeleteUser,
    AddUser { id: u64 },
    CameraRooms,
    RoomCameras { room: i64 },
    PromptAddCamera { room: i64 },
    ConfirmDeleteCamera { room: i64, camera: i64 },
    DeleteCamera { room: i64, camera: i64 },
    UserProfile { id: u64 },
    CycleUserRole { id: u64 },
    ResetUserAccess { id: u64 },
    UserRooms { id: u64 },
    ToggleUserRoomAccess { id: u64, room: i64 },
    UserRoomDevices { id: u64, room: i64 },
    ToggleUserDeviceAccess { id: u64, room: i64, device: i64 },
    ToggleUserDeviceNotify { id: u64, room: i64, device: i64 },
    ConfirmDeleteUser { id: u64 },
    DeleteUser { id: u64 },
}

impl Payload {
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
        let bin = B64
            .decode(s)
            .map_err(|e| anyhow::anyhow!("Base64 decode failed for '{}': {}", s, e))?;

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

    info!(
        "ROUTER CALL: user_id={}, payload {}",
        user_id,
        payload.to_string()
    );

    let ctx = RenderContext {
        user_id,
        config: config.clone(),
        notifications,
        is_admin,
    };

    match payload {
        Payload::Home {} => Ok(super::screens::home::render(ctx).await?),
        Payload::Control(sub_payload) => Ok(router_control(ctx, sub_payload).await?),
        Payload::Settings(sub_payload) => Ok(router_settings(ctx, sub_payload).await?),
        Payload::Camera(sub_payload) => Ok(router_camera(ctx, sub_payload).await?),
        Payload::Admin(sub_payload) => Ok(router_admin(ctx, sub_payload).await?),
        Payload::InDev {} => Ok(super::screens::common::in_dev_menu(ctx, Payload::Home).await?),
    }
}

async fn router_control(ctx: RenderContext, payload: ControlPayload) -> anyhow::Result<View> {
    match payload {
        ControlPayload::ListRooms => {
            Ok(super::screens::rooms::render(ctx, RoomViewMode::Control).await?)
        }
        ControlPayload::RoomDetail { room } => {
            if !db::access::can_view_room(ctx.user_id, ctx.is_admin, room, &ctx.config.db).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Control(ControlPayload::ListRooms),
                ));
            }

            Ok(room::render(ctx, room, RoomViewMode::Control).await?)
        }
        ControlPayload::QuickAction { room, device, cmd } => {
            if !can_run_device_action(&ctx, device, &cmd).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Control(ControlPayload::RoomDetail { room }),
                ));
            }

            let action = devices::DeviceAction::from(cmd.clone());

            let result = devices::handle_device_interaction(&ctx.config, device, action).await?;

            match result {
                InteractionResult::Processed => {
                    Ok(room::render(ctx, room, RoomViewMode::Control).await?)
                }
                InteractionResult::RequiresDetail => Ok(
                    super::screens::control::device_control::render(ctx, room, device, cmd).await?,
                ),
                InteractionResult::RequiresInput(intent) => {
                    let state = State::from_intent(intent, device, room);
                    Ok(super::screens::control::sensor_view::render_manual_input(
                        room, device, state,
                    ))
                }
                InteractionResult::Error { error: e } => Ok(View {
                    alert: Option::from(e),
                    ..Default::default()
                }),
            }
        }
        _ => Ok(super::screens::common::in_dev_menu(
            ctx,
            Payload::Control(ControlPayload::ListRooms {}),
        )
        .await?),
    }
}

async fn can_run_device_action(
    ctx: &RenderContext,
    device_id: i64,
    cmd: &DeviceCmd,
) -> anyhow::Result<bool> {
    if action_requires_control(cmd) {
        db::access::can_control_device(ctx.user_id, ctx.is_admin, device_id, &ctx.config.db).await
    } else {
        db::access::can_view_device(ctx.user_id, ctx.is_admin, device_id, &ctx.config.db).await
    }
}

fn action_requires_control(cmd: &DeviceCmd) -> bool {
    matches!(
        cmd,
        DeviceCmd::Toggle
            | DeviceCmd::TurnOn
            | DeviceCmd::TurnOff
            | DeviceCmd::SetLevel(_)
            | DeviceCmd::SetTemp(_)
    )
}

fn access_denied_view(ctx: RenderContext, back_payload: Payload) -> View {
    View {
        notifications: ctx.notifications,
        text: "Недостаточно прав для этого действия".to_string(),
        alert: Some("Доступ ограничен профилем пользователя".to_string()),
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button(
            back_payload.clone(),
        )]]),
        payload: back_payload,
        ..Default::default()
    }
}

async fn router_camera(ctx: RenderContext, payload: CameraPayload) -> anyhow::Result<View> {
    match payload {
        CameraPayload::ListCameras => Ok(super::screens::cameras::render_list(ctx).await?),
        CameraPayload::CameraDetail { id }
        | CameraPayload::Snapshot { id }
        | CameraPayload::Clip { id, .. } => {
            if db::cameras::get_accessible_camera(ctx.user_id, ctx.is_admin, id, &ctx.config.db)
                .await?
                .is_none()
            {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::ListCameras),
                ));
            }

            Ok(super::screens::cameras::render_detail(ctx, id).await?)
        }
    }
}

async fn router_settings(ctx: RenderContext, payload: SettingsPayload) -> anyhow::Result<View> {
    if !ctx.is_admin {
        return Ok(access_denied_view(ctx, Payload::Home));
    }

    match payload {
        SettingsPayload::ListRooms => {
            Ok(super::screens::rooms::render(ctx, RoomViewMode::Settings).await?)
        }
        SettingsPayload::RoomDetail { room } => {
            if !db::access::can_view_room(ctx.user_id, ctx.is_admin, room, &ctx.config.db).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::ListRooms),
                ));
            }

            Ok(room::render(ctx, room, RoomViewMode::Settings).await?)
        }
        SettingsPayload::DeviceDetail { room, device } => {
            if !db::access::can_view_device(ctx.user_id, ctx.is_admin, device, &ctx.config.db)
                .await?
            {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::RoomDetail { room }),
                ));
            }

            Ok(super::screens::settings::device_settings::render(ctx, room, device).await?)
        }
        SettingsPayload::ToggleNotify { room, device } => {
            if !db::access::can_notify_entity_for_device_id(
                ctx.user_id,
                ctx.is_admin,
                device,
                &ctx.config.db,
            )
            .await?
            {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::RoomDetail { room }),
                ));
            }

            let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                .await?
                .context("Device not found")?;
            db::subscriptions::toggle_subscription(
                ctx.user_id as i64,
                &dev.entity_id,
                &ctx.config.db,
            )
            .await?;
            super::screens::settings::device_settings::render(ctx, room, device).await
        }

        SettingsPayload::ToggleHide { room, device } => {
            if !ctx.is_admin {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::RoomDetail { room }),
                ));
            }

            let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                .await?
                .context("Device not found")?;
            db::subscriptions::toggle_hidden(&dev.entity_id, &ctx.config.db).await?;
            super::screens::settings::device_settings::render(ctx, room, device).await
        }
        _ => Ok(super::screens::common::in_dev_menu(
            ctx,
            Payload::Settings(SettingsPayload::ListRooms {}),
        )
        .await?),
    }
}

async fn router_admin(ctx: RenderContext, payload: AdminPayload) -> anyhow::Result<View> {
    if !ctx.is_admin {
        return Ok(super::screens::common::in_dev_menu(ctx, Payload::Home).await?);
    }

    match payload {
        AdminPayload::ListActions => Ok(super::screens::admin::list_actions::render(ctx).await?),
        AdminPayload::ListUsers => {
            Ok(super::screens::admin::list_actions::render_users(ctx).await?)
        }
        AdminPayload::Status => Ok(super::screens::admin::list_actions::render_status(ctx).await?),
        AdminPayload::ConfirmBackup => {
            Ok(super::screens::admin::list_actions::render_confirm_action(
                ctx,
                "Backup базы данных",
                "Создать резервную копию SQLite базы данных?",
                Payload::Admin(AdminPayload::CreateBackup),
                Payload::Admin(AdminPayload::Status),
            ))
        }
        AdminPayload::CreateBackup => {
            let path = crate::db::create_timestamped_backup(&ctx.config.db).await?;
            let mut view = super::screens::admin::list_actions::render_status(ctx).await?;
            view.alert = Some(format!("Backup создан: {}", path.display()));
            Ok(view)
        }
        AdminPayload::PromptAddUser => Ok(super::screens::admin::list_actions::render_user_input(
            ctx,
            State::AddUser { user_id: 0 },
            "Добавление пользователя",
            "Введите Telegram ID пользователя, которому нужно открыть доступ.",
        )),
        AdminPayload::PromptDeleteUser => {
            Ok(super::screens::admin::list_actions::render_user_input(
                ctx,
                State::DeleteUser { user_id: 0 },
                "Удаление пользователя",
                "Введите Telegram ID пользователя, у которого нужно забрать доступ.",
            ))
        }
        AdminPayload::AddUser { id } => {
            crate::db::add_user(id, &ctx.config.db).await?;
            let mut view = super::screens::admin::list_actions::render_users(ctx).await?;
            view.alert = Some(format!("Пользователь {} добавлен", id));
            Ok(view)
        }
        AdminPayload::CameraRooms => {
            Ok(super::screens::admin::list_actions::render_camera_rooms(ctx).await?)
        }
        AdminPayload::RoomCameras { room } => {
            Ok(super::screens::admin::list_actions::render_room_cameras(ctx, room).await?)
        }
        AdminPayload::PromptAddCamera { room } => {
            Ok(super::screens::admin::list_actions::render_add_camera_input(ctx, room).await?)
        }
        AdminPayload::ConfirmDeleteCamera { room, camera } => {
            let camera_name = db::cameras::get_camera(camera, &ctx.config.db)
                .await?
                .map(|camera| camera.name)
                .unwrap_or_else(|| camera.to_string());

            Ok(super::screens::admin::list_actions::render_confirm_action(
                ctx,
                "Удаление камеры",
                &format!("Удалить камеру «{}»?", camera_name),
                Payload::Admin(AdminPayload::DeleteCamera { room, camera }),
                Payload::Admin(AdminPayload::RoomCameras { room }),
            ))
        }
        AdminPayload::DeleteCamera { room, camera } => {
            db::cameras::disable_camera(camera, &ctx.config.db).await?;
            let mut view =
                super::screens::admin::list_actions::render_room_cameras(ctx, room).await?;
            view.alert = Some("Камера удалена".to_string());
            Ok(view)
        }
        AdminPayload::UserProfile { id } => {
            Ok(super::screens::admin::list_actions::render_user_profile(ctx, id).await?)
        }
        AdminPayload::CycleUserRole { id } => {
            if id != ctx.config.root_user {
                db::access::cycle_user_role(id, &ctx.config.db).await?;
            }

            Ok(super::screens::admin::list_actions::render_user_profile(ctx, id).await?)
        }
        AdminPayload::ResetUserAccess { id } => {
            let mut view = if id == ctx.config.root_user {
                let mut view =
                    super::screens::admin::list_actions::render_user_profile(ctx, id).await?;
                view.alert = Some("Root пользователя нельзя ограничить или сбросить".to_string());
                view
            } else {
                db::access::reset_user_access(id, &ctx.config.db).await?;
                let mut view =
                    super::screens::admin::list_actions::render_user_profile(ctx, id).await?;
                view.alert = Some(format!("Ограничения пользователя {} сброшены", id));
                view
            };
            view.payload = Payload::Admin(AdminPayload::UserProfile { id });
            Ok(view)
        }
        AdminPayload::UserRooms { id } => {
            Ok(super::screens::admin::list_actions::render_user_rooms(ctx, id).await?)
        }
        AdminPayload::ToggleUserRoomAccess { id, room } => {
            if id != ctx.config.root_user {
                db::access::toggle_room_view_access(id, room, &ctx.config.db).await?;
            }

            Ok(super::screens::admin::list_actions::render_user_rooms(ctx, id).await?)
        }
        AdminPayload::UserRoomDevices { id, room } => Ok(
            super::screens::admin::list_actions::render_user_room_devices(ctx, id, room).await?,
        ),
        AdminPayload::ToggleUserDeviceAccess { id, room, device } => {
            if id != ctx.config.root_user {
                let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                    .await?
                    .context("Device not found")?;
                db::access::toggle_device_access_mode(id, &dev.entity_id, &ctx.config.db).await?;
            }

            Ok(
                super::screens::admin::list_actions::render_user_room_devices(ctx, id, room)
                    .await?,
            )
        }
        AdminPayload::ToggleUserDeviceNotify { id, room, device } => {
            if id != ctx.config.root_user {
                let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                    .await?
                    .context("Device not found")?;
                db::access::toggle_device_notify_access(id, &dev.entity_id, &ctx.config.db).await?;
            }

            Ok(
                super::screens::admin::list_actions::render_user_room_devices(ctx, id, room)
                    .await?,
            )
        }
        AdminPayload::ConfirmDeleteUser { id } => {
            Ok(super::screens::admin::list_actions::render_confirm_action(
                ctx,
                "Удаление пользователя",
                &format!("Удалить пользователя {} и все его подписки?", id),
                Payload::Admin(AdminPayload::DeleteUser { id }),
                Payload::Admin(AdminPayload::ListUsers),
            ))
        }
        AdminPayload::DeleteUser { id } => {
            let mut view = if id == ctx.config.root_user {
                let mut view = super::screens::admin::list_actions::render_users(ctx).await?;
                view.alert = Some("Root пользователя нельзя удалить".to_string());
                view
            } else {
                crate::db::delete_user(id, &ctx.config.db).await?;
                ctx.config.sessions.remove(&id);
                ctx.config.ui_locks.remove(&id);
                let mut view = super::screens::admin::list_actions::render_users(ctx).await?;
                view.alert = Some(format!("Пользователь {} удален", id));
                view
            };
            view.payload = Payload::Admin(AdminPayload::ListUsers);
            Ok(view)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EnvPaths;
    use crate::{db, ha};
    use dashmap::DashMap;

    #[test]
    fn test_payload_integrity_and_size() {
        let payloads = [
            Payload::Control(ControlPayload::QuickAction {
                room: 1_000_000,
                device: 2_000_000,
                cmd: DeviceCmd::ShowChart { h: 168, o: -168 },
            }),
            Payload::Admin(AdminPayload::DeleteUser { id: 9_999_999_999 }),
            Payload::Admin(AdminPayload::ConfirmDeleteUser { id: 9_999_999_999 }),
            Payload::Admin(AdminPayload::ConfirmBackup),
            Payload::Admin(AdminPayload::CreateBackup),
        ];

        for original in payloads {
            let encoded = original.to_string();
            let len = encoded.len();

            assert!(len > 0, "Encoded string should not be empty");
            assert!(
                len <= 64,
                "🛑 Payload overflow: {} bytes used. Max is 64.",
                len
            );

            let restored = Payload::from_string(&encoded)
                .expect("Failed to decode payload from Base64/Binary");

            assert_eq!(
                restored, original,
                "Data corruption: restored payload differs from original"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires real Home Assistant configuration and database"]
    async fn test_sensor_render_preserves_payload_context() -> anyhow::Result<()> {
        let encoded_input = "AQMENAUYAA";
        let original_payload =
            Payload::from_string(encoded_input).expect("Failed to decode test payload");

        let paths = EnvPaths::load()
            .validate()
            .context("Error checking env variables.")?;

        let db_pool = db::init(
            &paths.db_url(),
            paths
                .migrations
                .to_str()
                .context("Путь к миграциям не валиден")?,
        )
        .await
        .context("Error initializing database pool.")?;

        let ha_client: Arc<dyn ha::HomeAssistantClient> =
            Arc::new(ha::init(paths.ha_url.clone(), paths.ha_token.clone()));

        let app_config = Arc::new(AppConfig {
            ha_client: ha_client.clone(),
            db: db_pool,
            root_user: 0,

            // delete_chart_timeout_s: 600,
            // delete_help_messages_timeout_s: 30,
            delete_notification_messages_timeout_s: 5,
            // delete_error_messages_timeout_s: 5,
            ttl_notifications: 1,
            background_maintenance_interval_s: 15,
            event_refresh_min_interval_s: 5,
            session_ttl_hours: 24,
            telegram_retry_after_extra_delay_s: 1,
            camera_default_clip_s: 10,
            camera_clip_intervals_s: vec![5, 10, 15],

            sessions: DashMap::new(),
            ui_locks: DashMap::new(),

            name_aliases: DashMap::new(),

            state_aliases: DashMap::new(),
            runtime_status: tokio::sync::RwLock::new(crate::models::RuntimeStatus::default()),
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
