use crate::bot::models::View;
use crate::bot::screens::room;
use crate::core::devices::{ChartParams, InputIntent, InteractionResult};
use crate::core::types::RoomViewMode;
use crate::core::{devices, HeaderItem};
use crate::db;
use crate::i18n::{t, Language};
use crate::models::AppConfig;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use teloxide::types::InlineKeyboardMarkup;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};

pub struct RenderContext {
    pub user_id: u64,
    pub config: Arc<AppConfig>,
    pub notifications: Vec<HeaderItem>,
    pub is_admin: bool,
    pub lang: Language,
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
    AddRecordingRule {
        room_id: i64,
    },
    EditRecordingRule {
        room_id: i64,
        rule_id: i64,
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
    CameraDetail {
        id: i64,
    },
    Snapshot {
        id: i64,
    },
    Clip {
        id: i64,
        seconds: u32,
    },
    RecordingArchive {
        camera: i64,
    },
    RecordingSession {
        camera: i64,
        session: i64,
    },
    SendRecordingSegment {
        camera: i64,
        session: i64,
        segment: i64,
    },
    SendRecordingAll {
        camera: i64,
        session: i64,
    },
    ConfirmDeleteRecording {
        camera: i64,
        session: i64,
    },
    DeleteRecording {
        camera: i64,
        session: i64,
    },
    StopRecording {
        camera: i64,
        session: i64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum SettingsPayload {
    ListRooms,
    RoomDetail {
        room: i64,
    },
    DeviceDetail {
        room: i64,
        device: i64,
    },
    ToggleNotify {
        room: i64,
        device: i64,
    },
    ToggleHide {
        room: i64,
        device: i64,
    },
    EditName {
        room: i64,
        device: i64,
    },
    StateAliases {
        room: i64,
        device: i64,
    },
    EditStateAlias {
        room: i64,
        device: i64,
        state: String,
    },
    ResetStateAlias {
        room: i64,
        device: i64,
        state: String,
    },
    ToggleStateInversion {
        room: i64,
        device: i64,
    },
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
    AddUser {
        id: u64,
    },
    CameraRooms,
    RoomCameras {
        room: i64,
    },
    RoomCameraDetail {
        room: i64,
        camera: i64,
    },
    RoomCameraSnapshot {
        room: i64,
        camera: i64,
    },
    RoomCameraClip {
        room: i64,
        camera: i64,
        seconds: u32,
    },
    PromptAddCamera {
        room: i64,
    },
    ConfirmDeleteCamera {
        room: i64,
        camera: i64,
    },
    DeleteCamera {
        room: i64,
        camera: i64,
    },
    RecordingRules {
        room: i64,
    },
    PromptAddRecordingRule {
        room: i64,
    },
    ToggleRecordingRule {
        room: i64,
        rule: i64,
    },
    ConfirmDeleteRecordingRule {
        room: i64,
        rule: i64,
    },
    DeleteRecordingRule {
        room: i64,
        rule: i64,
    },
    CycleRecordingDefaultRetention {
        room: i64,
    },
    CycleRecordingStorageQuota {
        room: i64,
    },
    UiBackground,
    SetUiBackgroundCamera {
        camera: Option<i64>,
    },
    CycleUiBackgroundInterval,
    UserProfile {
        id: u64,
    },
    CycleUserRole {
        id: u64,
    },
    CycleUserLanguage {
        id: u64,
    },
    ResetUserAccess {
        id: u64,
    },
    UserRooms {
        id: u64,
    },
    ToggleUserRoomAccess {
        id: u64,
        room: i64,
    },
    UserRoomDevices {
        id: u64,
        room: i64,
    },
    ToggleUserDeviceAccess {
        id: u64,
        room: i64,
        device: i64,
    },
    ToggleUserDeviceNotify {
        id: u64,
        room: i64,
        device: i64,
    },
    ConfirmDeleteUser {
        id: u64,
    },
    DeleteUser {
        id: u64,
    },
    RecordingRuleDetail {
        room: i64,
        rule: i64,
    },
    ToggleRecordingRuleDetail {
        room: i64,
        rule: i64,
    },
    PromptEditRecordingRule {
        room: i64,
        rule: i64,
    },
    ToggleRecordingRuleNotify {
        room: i64,
        rule: i64,
    },
    DuplicateRecordingRule {
        room: i64,
        rule: i64,
    },
    TestRecordingRule {
        room: i64,
        rule: i64,
    },
    ToggleRecordingRuleNoise {
        room: i64,
        rule: i64,
    },
    CameraHealth {
        room: i64,
        camera: i64,
    },
    CheckCameraHealth {
        room: i64,
        camera: i64,
    },
    ActivityLog {
        filter: ActivityLogFilter,
    },
    RecordingRuleGroups,
    RecordingRuleGroupsForRoom {
        room: i64,
    },
    ToggleRecordingRuleGroup {
        group: i64,
    },
    ToggleRecordingRuleGroupForRoom {
        room: i64,
        group: i64,
    },
    ToggleRuleGroupItem {
        room: i64,
        rule: i64,
        group: i64,
    },
    EnsureDefaultRuleGroups,
    EnsureDefaultRuleGroupsForRoom {
        room: i64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum ActivityLogFilter {
    All,
    Errors,
    Cameras,
    Devices,
    Recording,
}

impl Payload {
    /// Сериализация в компактную Base64 строку.
    /// JSON (67 байт) -> Binary (~12 байт) -> Base64 (~16 символов).
    #[allow(clippy::inherent_to_string)]
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
    let lang = db::get_user_language(user_id, &config.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(config.default_language);

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
        lang,
    };

    let mut view = match payload {
        Payload::Home => super::screens::home::render(ctx).await?,
        Payload::Control(sub_payload) => router_control(ctx, sub_payload).await?,
        Payload::Settings(sub_payload) => router_settings(ctx, sub_payload).await?,
        Payload::Camera(sub_payload) => router_camera(ctx, sub_payload).await?,
        Payload::Admin(sub_payload) => router_admin(ctx, sub_payload).await?,
        Payload::InDev => super::screens::common::in_dev_menu(ctx, Payload::Home).await?,
    };
    let final_lang = db::get_user_language(user_id, &config.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(config.default_language);
    view.lang = final_lang;
    Ok(view)
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

async fn cycle_setting(
    key: &str,
    values: &[i64],
    default: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<i64> {
    let current = db::settings::get_i64(key, pool).await?.unwrap_or(default);
    let next = values
        .iter()
        .position(|value| *value == current)
        .map(|index| values[(index + 1) % values.len()])
        .unwrap_or(default);
    db::settings::set_i64(key, next, pool).await?;
    Ok(next)
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
        text: t(ctx.lang, "access.denied.text").to_string(),
        alert: Some(t(ctx.lang, "access.denied.alert").to_string()),
        kb: InlineKeyboardMarkup::new(vec![vec![crate::bot::screens::common::back_button_lang(
            ctx.lang,
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
            if !can_access_camera(&ctx, id).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::ListCameras),
                ));
            }

            Ok(super::screens::cameras::render_detail(ctx, id).await?)
        }
        CameraPayload::RecordingArchive { camera } => {
            if !can_access_camera(&ctx, camera).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::ListCameras),
                ));
            }

            Ok(super::screens::cameras::render_recording_archive(ctx, camera).await?)
        }
        CameraPayload::RecordingSession { camera, session }
        | CameraPayload::SendRecordingSegment {
            camera, session, ..
        }
        | CameraPayload::SendRecordingAll { camera, session } => {
            if !can_access_camera(&ctx, camera).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::ListCameras),
                ));
            }
            if !recording_session_belongs_to_camera(session, camera, &ctx.config.db).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingArchive { camera }),
                ));
            }

            Ok(super::screens::cameras::render_recording_session(ctx, camera, session).await?)
        }
        CameraPayload::ConfirmDeleteRecording { camera, session } => {
            if !ctx.is_admin {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingSession { camera, session }),
                ));
            }
            if !recording_session_belongs_to_camera(session, camera, &ctx.config.db).await? {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingArchive { camera }),
                ));
            }

            let lang = ctx.lang;
            Ok(super::screens::admin::list_actions::render_confirm_action(
                ctx,
                t(lang, "camera.recording.delete.title"),
                t(lang, "camera.recording.delete.confirm"),
                Payload::Camera(CameraPayload::DeleteRecording { camera, session }),
                Payload::Camera(CameraPayload::RecordingSession { camera, session }),
            ))
        }
        CameraPayload::StopRecording { camera, session } => {
            if !ctx.is_admin
                || !can_access_camera(&ctx, camera).await?
                || !recording_session_belongs_to_camera(session, camera, &ctx.config.db).await?
            {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingArchive { camera }),
                ));
            }

            db::camera_recording_sessions::request_stop(session, &ctx.config.db).await?;
            let lang = ctx.lang;
            let mut view =
                super::screens::cameras::render_recording_session(ctx, camera, session).await?;
            view.notice = Some(t(lang, "camera.recording.stop_notice").to_string());
            Ok(view)
        }
        CameraPayload::DeleteRecording { camera, session } => {
            if !ctx.is_admin {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingSession { camera, session }),
                ));
            }

            let Some(recording_session) =
                db::camera_recording_sessions::get_session(session, &ctx.config.db).await?
            else {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingArchive { camera }),
                ));
            };
            if recording_session.camera_id != camera {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingArchive { camera }),
                ));
            }

            crate::core::camera_recording::delete_recording_session_files(&ctx.config, session)
                .await?;
            let lang = ctx.lang;
            let mut view = super::screens::cameras::render_recording_archive(ctx, camera).await?;
            view.notice = Some(t(lang, "camera.recording.deleted_notice").to_string());
            Ok(view)
        }
    }
}

async fn can_access_camera(ctx: &RenderContext, camera_id: i64) -> anyhow::Result<bool> {
    Ok(
        db::cameras::get_accessible_camera(ctx.user_id, ctx.is_admin, camera_id, &ctx.config.db)
            .await?
            .is_some(),
    )
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
        SettingsPayload::StateAliases { room, device } => {
            if !db::access::can_view_device(ctx.user_id, ctx.is_admin, device, &ctx.config.db)
                .await?
            {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::RoomDetail { room }),
                ));
            }

            super::screens::settings::device_settings::render_state_aliases(ctx, room, device).await
        }
        SettingsPayload::EditStateAlias {
            room,
            device,
            state,
        } => {
            if !ctx.is_admin {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::RoomDetail { room }),
                ));
            }

            let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                .await?
                .context("Device not found")?;
            let current_alias =
                db::devices::get_state_alias(&dev.entity_id, &state, &ctx.config.db).await?;
            Ok(super::screens::admin::list_actions::render_user_input(
                ctx,
                State::WaitingForStateAlias {
                    device_id: device,
                    original_state: state.clone(),
                    room_id: room,
                },
                "Алиас состояния",
                &format!(
                    "Введите новое имя для состояния `{}`.\nТекущий алиас: {}",
                    state,
                    current_alias.unwrap_or_else(|| "не задан".to_string())
                ),
                Payload::Settings(SettingsPayload::StateAliases { room, device }),
            ))
        }
        SettingsPayload::ResetStateAlias {
            room,
            device,
            state,
        } => {
            if !ctx.is_admin {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::RoomDetail { room }),
                ));
            }

            let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                .await?
                .context("Device not found")?;
            db::devices::delete_state_alias(&dev.entity_id, &state, &ctx.config.db).await?;
            ctx.config.remove_state_alias_cache(&dev.entity_id, &state);

            let mut view =
                super::screens::settings::device_settings::render_state_aliases(ctx, room, device)
                    .await?;
            view.notice = Some("Алиас состояния сброшен".to_string());
            Ok(view)
        }
        SettingsPayload::ToggleStateInversion { room, device } => {
            if !ctx.is_admin {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Settings(SettingsPayload::RoomDetail { room }),
                ));
            }

            let enabled = db::devices::toggle_state_inversion(device, &ctx.config.db).await?;
            let mut view =
                super::screens::settings::device_settings::render_state_aliases(ctx, room, device)
                    .await?;
            view.notice = Some(if enabled {
                "Логическая инверсия включена".to_string()
            } else {
                "Логическая инверсия выключена".to_string()
            });
            Ok(view)
        }
        _ => Ok(super::screens::common::in_dev_menu(
            ctx,
            Payload::Settings(SettingsPayload::ListRooms {}),
        )
        .await?),
    }
}

async fn recording_session_belongs_to_camera(
    session_id: i64,
    camera_id: i64,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<bool> {
    Ok(db::camera_recording_sessions::get_session(session_id, pool)
        .await?
        .map(|session| session.camera_id == camera_id)
        .unwrap_or(false))
}

async fn router_admin(mut ctx: RenderContext, payload: AdminPayload) -> anyhow::Result<View> {
    if !ctx.is_admin {
        return super::screens::common::in_dev_menu(ctx, Payload::Home).await;
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
            view.notice = Some(format!("Backup создан: {}", path.display()));
            Ok(view)
        }
        AdminPayload::PromptAddUser => Ok(super::screens::admin::list_actions::render_user_input(
            ctx,
            State::AddUser { user_id: 0 },
            "Добавление пользователя",
            "Введите Telegram ID пользователя, которому нужно открыть доступ.",
            Payload::Admin(AdminPayload::ListUsers),
        )),
        AdminPayload::PromptDeleteUser => {
            Ok(super::screens::admin::list_actions::render_user_input(
                ctx,
                State::DeleteUser { user_id: 0 },
                "Удаление пользователя",
                "Введите Telegram ID пользователя, у которого нужно забрать доступ.",
                Payload::Admin(AdminPayload::ListUsers),
            ))
        }
        AdminPayload::AddUser { id } => {
            crate::db::add_user(id, &ctx.config.db).await?;
            let mut view = super::screens::admin::list_actions::render_users(ctx).await?;
            view.notice = Some(format!("Пользователь {} добавлен", id));
            Ok(view)
        }
        AdminPayload::CameraRooms => {
            Ok(super::screens::admin::list_actions::render_camera_rooms(ctx).await?)
        }
        AdminPayload::RoomCameras { room } => {
            Ok(super::screens::admin::list_actions::render_room_cameras(ctx, room).await?)
        }
        AdminPayload::RoomCameraDetail { room, camera }
        | AdminPayload::RoomCameraSnapshot { room, camera }
        | AdminPayload::RoomCameraClip { room, camera, .. } => Ok(
            super::screens::admin::list_actions::render_room_camera_detail(ctx, room, camera)
                .await?,
        ),
        AdminPayload::PromptAddCamera { room } => {
            Ok(super::screens::admin::list_actions::render_add_camera_input(ctx, room).await?)
        }
        AdminPayload::RecordingRules { room } => {
            Ok(super::screens::admin::list_actions::render_recording_rules(ctx, room).await?)
        }
        AdminPayload::RecordingRuleDetail { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_detail(ctx, room, rule)
                .await?,
        ),
        AdminPayload::PromptAddRecordingRule { room } => Ok(
            super::screens::admin::list_actions::render_add_recording_rule_input(ctx, room).await?,
        ),
        AdminPayload::PromptEditRecordingRule { room, rule } => Ok(
            super::screens::admin::list_actions::render_edit_recording_rule_input(ctx, room, rule)
                .await?,
        ),
        AdminPayload::ToggleRecordingRule { room, rule } => {
            db::camera_recording_rules::toggle_rule_enabled(rule, &ctx.config.db).await?;
            Ok(super::screens::admin::list_actions::render_recording_rules(ctx, room).await?)
        }
        AdminPayload::ToggleRecordingRuleNotify { room, rule } => {
            db::camera_recording_rules::toggle_rule_notifications(rule, &ctx.config.db).await?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_detail(ctx, room, rule)
                    .await?,
            )
        }
        AdminPayload::DuplicateRecordingRule { room, rule } => {
            let new_rule_id =
                db::camera_recording_rules::duplicate_rule(rule, &ctx.config.db).await?;
            let mut view = super::screens::admin::list_actions::render_recording_rule_detail(
                ctx,
                room,
                new_rule_id,
            )
            .await?;
            view.notice = Some("Копия правила создана".to_string());
            Ok(view)
        }
        AdminPayload::TestRecordingRule { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_test(ctx, room, rule)
                .await?,
        ),
        AdminPayload::ToggleRecordingRuleNoise { room, rule } => {
            db::camera_recording_rules::toggle_rule_noise(rule, &ctx.config.db).await?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_detail(ctx, room, rule)
                    .await?,
            )
        }
        AdminPayload::CameraHealth { room, camera } => Ok(
            super::screens::admin::list_actions::render_camera_health(ctx, room, camera).await?,
        ),
        AdminPayload::CheckCameraHealth { room, camera } => {
            let lang = ctx.lang;
            super::handlers::spawn_camera_health_check(ctx.user_id, camera, ctx.config.clone());
            let mut view =
                super::screens::admin::list_actions::render_camera_health(ctx, room, camera)
                    .await?;
            view.notice = Some(t(lang, "admin.camera.health.check_started").to_string());
            Ok(view)
        }
        AdminPayload::ActivityLog { filter } => {
            Ok(super::screens::admin::list_actions::render_activity_log(ctx, filter).await?)
        }
        AdminPayload::RecordingRuleGroups => {
            Ok(super::screens::admin::list_actions::render_recording_rule_groups(ctx).await?)
        }
        AdminPayload::RecordingRuleGroupsForRoom { room } => Ok(
            super::screens::admin::list_actions::render_recording_rule_groups_for_room(ctx, room)
                .await?,
        ),
        AdminPayload::ToggleRecordingRuleGroup { group } => {
            db::camera_recording_rule_groups::toggle_group_enabled(group, &ctx.config.db).await?;
            Ok(super::screens::admin::list_actions::render_recording_rule_groups(ctx).await?)
        }
        AdminPayload::ToggleRecordingRuleGroupForRoom { room, group } => {
            db::camera_recording_rule_groups::toggle_group_enabled(group, &ctx.config.db).await?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_groups_for_room(
                    ctx, room,
                )
                .await?,
            )
        }
        AdminPayload::ToggleRuleGroupItem { room, rule, group } => {
            db::camera_recording_rule_groups::toggle_rule_in_group(rule, group, &ctx.config.db)
                .await?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_detail(ctx, room, rule)
                    .await?,
            )
        }
        AdminPayload::EnsureDefaultRuleGroups => {
            db::camera_recording_rule_groups::ensure_default_groups(&ctx.config.db).await?;
            let lang = ctx.lang;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_groups(ctx).await?;
            view.notice = Some(t(lang, "admin.rule_groups.defaults_created").to_string());
            Ok(view)
        }
        AdminPayload::EnsureDefaultRuleGroupsForRoom { room } => {
            db::camera_recording_rule_groups::ensure_default_groups(&ctx.config.db).await?;
            let lang = ctx.lang;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_groups_for_room(
                    ctx, room,
                )
                .await?;
            view.notice = Some(t(lang, "admin.rule_groups.defaults_created").to_string());
            Ok(view)
        }
        AdminPayload::ToggleRecordingRuleDetail { room, rule } => {
            db::camera_recording_rules::toggle_rule_enabled(rule, &ctx.config.db).await?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_detail(ctx, room, rule)
                    .await?,
            )
        }
        AdminPayload::ConfirmDeleteRecordingRule { room, rule } => {
            Ok(super::screens::admin::list_actions::render_confirm_action(
                ctx,
                "Удаление правила записи",
                "Удалить правило событийной записи?",
                Payload::Admin(AdminPayload::DeleteRecordingRule { room, rule }),
                Payload::Admin(AdminPayload::RecordingRules { room }),
            ))
        }
        AdminPayload::DeleteRecordingRule { room, rule } => {
            db::camera_recording_rules::soft_delete_rule(rule, &ctx.config.db).await?;
            let mut view =
                super::screens::admin::list_actions::render_recording_rules(ctx, room).await?;
            view.notice = Some("Правило удалено".to_string());
            Ok(view)
        }
        AdminPayload::CycleRecordingDefaultRetention { room } => {
            cycle_setting(
                db::settings::CAMERA_RECORDING_DEFAULT_RETENTION_DAYS,
                &[7, 14, 30, 60, 90],
                30,
                &ctx.config.db,
            )
            .await?;
            Ok(super::screens::admin::list_actions::render_recording_rules(ctx, room).await?)
        }
        AdminPayload::CycleRecordingStorageQuota { room } => {
            cycle_setting(
                db::settings::CAMERA_RECORDING_MAX_STORAGE_MB,
                &[0, 1024, 5120, 10240],
                0,
                &ctx.config.db,
            )
            .await?;
            Ok(super::screens::admin::list_actions::render_recording_rules(ctx, room).await?)
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
            view.notice = Some("Камера удалена".to_string());
            Ok(view)
        }
        AdminPayload::UiBackground => {
            Ok(super::screens::admin::list_actions::render_ui_background(ctx).await?)
        }
        AdminPayload::SetUiBackgroundCamera { camera } => {
            let lang = ctx.lang;
            match camera {
                Some(camera_id) => {
                    if db::cameras::get_camera(camera_id, &ctx.config.db)
                        .await?
                        .is_none()
                    {
                        let mut view =
                            super::screens::admin::list_actions::render_ui_background(ctx).await?;
                        view.alert = Some(
                            crate::i18n::t(lang, "admin.ui_background.camera_missing").to_string(),
                        );
                        return Ok(view);
                    }

                    db::settings::set_i64(
                        db::settings::UI_BACKGROUND_CAMERA_ID,
                        camera_id,
                        &ctx.config.db,
                    )
                    .await?;
                }
                None => {
                    db::settings::delete(db::settings::UI_BACKGROUND_CAMERA_ID, &ctx.config.db)
                        .await?;
                }
            }

            crate::core::ui_background::clear(&ctx.config).await;
            let mut view = super::screens::admin::list_actions::render_ui_background(ctx).await?;
            view.notice = Some(crate::i18n::t(lang, "admin.ui_background.updated").to_string());
            Ok(view)
        }
        AdminPayload::CycleUiBackgroundInterval => {
            let lang = ctx.lang;
            let current = crate::core::ui_background::refresh_interval_s(&ctx.config).await;
            let next = crate::core::ui_background::next_interval(current);
            db::settings::set_i64(
                db::settings::UI_BACKGROUND_REFRESH_S,
                i64::from(next),
                &ctx.config.db,
            )
            .await?;
            crate::core::ui_background::clear(&ctx.config).await;
            let mut view = super::screens::admin::list_actions::render_ui_background(ctx).await?;
            view.notice = Some(format!(
                "{}: {}с",
                crate::i18n::t(lang, "admin.ui_background.interval_changed"),
                next
            ));
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
        AdminPayload::CycleUserLanguage { id } => {
            let new_lang =
                db::cycle_user_language(id, ctx.config.default_language, &ctx.config.db).await?;
            let alert_lang = if id == ctx.user_id {
                new_lang
            } else {
                ctx.lang
            };
            if id == ctx.user_id {
                ctx.lang = new_lang;
            }
            let mut view =
                super::screens::admin::list_actions::render_user_profile(ctx, id).await?;
            view.notice = Some(crate::i18n::t(alert_lang, "lang.changed").to_string());
            Ok(view)
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
                view.notice = Some(format!("Ограничения пользователя {} сброшены", id));
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
                view.notice = Some(format!("Пользователь {} удален", id));
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
            Payload::Admin(AdminPayload::RoomCameraDetail {
                room: 1_000_000,
                camera: 2_000_000,
            }),
            Payload::Admin(AdminPayload::RoomCameraClip {
                room: 1_000_000,
                camera: 2_000_000,
                seconds: 120,
            }),
            Payload::Admin(AdminPayload::RecordingRuleDetail {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleDetail {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::PromptEditRecordingRule {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleNotify {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::DuplicateRecordingRule {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::TestRecordingRule {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleNoise {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Camera(CameraPayload::StopRecording {
                camera: 1_000_000,
                session: 2_000_000,
            }),
            Payload::Admin(AdminPayload::CameraHealth {
                room: 1_000_000,
                camera: 2_000_000,
            }),
            Payload::Admin(AdminPayload::CheckCameraHealth {
                room: 1_000_000,
                camera: 2_000_000,
            }),
            Payload::Admin(AdminPayload::ActivityLog {
                filter: ActivityLogFilter::Recording,
            }),
            Payload::Admin(AdminPayload::RecordingRuleGroups),
            Payload::Admin(AdminPayload::RecordingRuleGroupsForRoom { room: 1_000_000 }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroup { group: 3_000_000 }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroupForRoom {
                room: 1_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRuleGroupItem {
                room: 1_000_000,
                rule: 2_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroups),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForRoom { room: 1_000_000 }),
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

        let (recording_tx, _recording_rx) =
            tokio::sync::mpsc::channel::<crate::core::camera_recording::RecordingJob>(1);
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
            default_language: crate::i18n::Language::Ru,
            camera_default_clip_s: 10,
            camera_clip_intervals_s: vec![5, 10, 15],
            camera_recording_max_tail_seconds: 300,
            camera_recording_max_segment_seconds: 300,
            camera_recording_max_parallel_jobs: 4,
            camera_recording_storage_root: "data/recordings".to_string(),
            camera_recording_tx: recording_tx,

            sessions: DashMap::new(),
            ui_locks: DashMap::new(),
            recording_sends_in_progress: DashMap::new(),

            name_aliases: DashMap::new(),

            state_aliases: DashMap::new(),
            ui_background_cache: tokio::sync::Mutex::new(None),
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
