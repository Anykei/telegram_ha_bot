use crate::bot::models::View;
use crate::bot::recording_rule_wizard::WizardTriggerMode;
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
    pub settings_origin: SettingsOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsOrigin {
    Home,
    Admin,
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
    AddRecordingRuleGroup {
        room_id: Option<i64>,
    },
    AddRecordingRuleGroupForWizard,
    AddRecordingRuleGroupForEdit {
        room_id: i64,
        rule_id: i64,
    },
    RenameRecordingRuleGroup {
        room_id: Option<i64>,
        group_id: i64,
    },
    AddActionGroup,
    RenameActionGroup {
        group_id: i64,
    },
    RenameHaNativeTarget {
        target_id: i64,
    },
    AddActionScheduleTime {
        target: db::action_groups::ActionTargetRef,
        command: db::action_groups::ActionScheduleCommand,
    },
    EditActionScheduleTime {
        target: db::action_groups::ActionTargetRef,
        schedule_id: i64,
    },
    EditRecordingRule {
        room_id: i64,
        rule_id: i64,
    },
    EditRecordingRuleNumber {
        room_id: i64,
        rule_id: i64,
        field: RecordingRuleEditField,
    },
    EditRecordingRuleActiveTime {
        room_id: i64,
        rule_id: i64,
    },
    EditRecordingRuleConditionValue {
        room_id: i64,
        rule_id: i64,
        device_id: i64,
        operator: db::camera_recording_rules::ConditionOperator,
    },
    RecordingRuleWizardSourceValue {
        mode: WizardTriggerMode,
    },
    RecordingRuleWizardActiveTimeValue,
    RecordingRuleWizardConditionValue,
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

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum RecordingRuleEditField {
    TailSeconds,
    MaxSegmentSeconds,
    CooldownSeconds,
    RetentionDays,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum RecordingRuleActiveTimePreset {
    Always,
    Night,
    Day,
}

impl RecordingRuleEditField {
    pub fn title(self) -> &'static str {
        match self {
            Self::TailSeconds => "Писать после события",
            Self::MaxSegmentSeconds => "Длина файла",
            Self::CooldownSeconds => "Пауза после записи",
            Self::RetentionDays => "Время хранения",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Self::RetentionDays => "д",
            _ => "с",
        }
    }
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
    Command(CommandPayload),
    AdminSettings(SettingsPayload),
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
    ActionGroups,
    ActionGroupDetail {
        group: i64,
    },
    ExecuteActionGroup {
        group: i64,
        command: db::action_groups::ActionGroupCommand,
    },
    HaNativeActionDetail {
        action: i64,
    },
    ExecuteHaNativeAction {
        action: i64,
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
    RecordingGroupModes,
    RecordingGroupModeDetail {
        group: i64,
    },
    ToggleRecordingGroupMode {
        group: i64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum CommandPayload {
    Confirm { id: i64 },
    Cancel { id: i64 },
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
    ToggleCritical {
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
    RecordingRuleGroupDetail {
        group: i64,
    },
    RecordingRuleGroupDetailForRoom {
        room: i64,
        group: i64,
    },
    RecordingRuleGroupRules {
        group: i64,
        page: u16,
        filter: RecordingRuleGroupRulesFilter,
    },
    RecordingRuleGroupRulesForRoom {
        room: i64,
        group: i64,
        page: u16,
        filter: RecordingRuleGroupRulesFilter,
    },
    PromptCreateRecordingRuleGroup,
    PromptCreateRecordingRuleGroupForRoom {
        room: i64,
    },
    PromptCreateRecordingRuleGroupForWizard,
    PromptCreateRecordingRuleGroupForEdit {
        room: i64,
        rule: i64,
    },
    PromptRenameRecordingRuleGroup {
        group: i64,
    },
    PromptRenameRecordingRuleGroupForRoom {
        room: i64,
        group: i64,
    },
    ToggleRecordingRuleGroup {
        group: i64,
    },
    ToggleRecordingRuleGroupForRoom {
        room: i64,
        group: i64,
    },
    ToggleRecordingRuleGroupDetail {
        group: i64,
    },
    ToggleRecordingRuleGroupDetailForRoom {
        room: i64,
        group: i64,
    },
    ConfirmDeleteRecordingRuleGroup {
        group: i64,
    },
    ConfirmDeleteRecordingRuleGroupForRoom {
        room: i64,
        group: i64,
    },
    DeleteRecordingRuleGroup {
        group: i64,
    },
    DeleteRecordingRuleGroupForRoom {
        room: i64,
        group: i64,
    },
    CycleRecordingRuleGroupAccess {
        group: i64,
    },
    CycleRecordingRuleGroupAccessForRoom {
        room: i64,
        group: i64,
    },
    ToggleRecordingRuleGroupRule {
        group: i64,
        rule: i64,
        page: u16,
        filter: RecordingRuleGroupRulesFilter,
    },
    ToggleRecordingRuleGroupRuleForRoom {
        room: i64,
        group: i64,
        rule: i64,
        page: u16,
        filter: RecordingRuleGroupRulesFilter,
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
    EnsureDefaultRuleGroupsForWizard,
    EnsureDefaultRuleGroupsForEdit {
        room: i64,
        rule: i64,
    },
    ToggleUserVoice {
        id: u64,
    },
    CycleUserVoiceEngine {
        id: u64,
    },
    StartRecordingRuleWizard {
        room: i64,
    },
    WizardPickCamera {
        room: i64,
        camera: i64,
    },
    WizardSourcePage {
        page: u16,
    },
    WizardPickEntity {
        room: i64,
        camera: i64,
        device: i64,
    },
    WizardPickMode {
        room: i64,
        camera: i64,
        device: i64,
        mode: WizardTriggerMode,
    },
    WizardPickTail {
        room: i64,
        camera: i64,
        device: i64,
        mode: WizardTriggerMode,
        tail: u32,
    },
    WizardPickRetention {
        room: i64,
        camera: i64,
        device: i64,
        mode: WizardTriggerMode,
        tail: u32,
        retention: u32,
    },
    WizardPickGroup {
        room: i64,
        camera: i64,
        device: i64,
        mode: WizardTriggerMode,
        tail: u32,
        retention: u32,
        group: Option<i64>,
    },
    WizardCreateRule {
        room: i64,
        camera: i64,
        device: i64,
        mode: WizardTriggerMode,
        tail: u32,
        retention: u32,
        group: Option<i64>,
    },
    WizardAdvancedText {
        room: i64,
        camera: i64,
        device: i64,
        mode: WizardTriggerMode,
        tail: u32,
        retention: u32,
        group: Option<i64>,
    },
    WizardConditions,
    WizardAddCondition,
    WizardConditionEntityPage {
        page: u16,
    },
    WizardPickConditionEntity {
        device: i64,
    },
    WizardPickConditionOperator {
        operator: db::camera_recording_rules::ConditionOperator,
    },
    WizardRemoveCondition {
        index: u8,
    },
    WizardToggleLogic,
    WizardNextTail,
    WizardPickWizardTail {
        tail: u32,
    },
    WizardPickWizardRetention {
        retention: u32,
    },
    WizardActiveTime,
    WizardPickActiveTimePreset {
        preset: RecordingRuleActiveTimePreset,
    },
    WizardPromptActiveTimeCustom,
    WizardToggleGroup {
        group: i64,
    },
    WizardConfirmGroups,
    WizardCreateCurrentRule,
    WizardAdvancedCurrentText,
    RecordingRuleEditMenu {
        room: i64,
        rule: i64,
    },
    RecordingRuleEditSensors {
        room: i64,
        rule: i64,
    },
    PromptEditRecordingRuleNumber {
        room: i64,
        rule: i64,
        field: RecordingRuleEditField,
    },
    RecordingRuleEditActiveTime {
        room: i64,
        rule: i64,
    },
    SetRecordingRuleActiveTimePreset {
        room: i64,
        rule: i64,
        preset: RecordingRuleActiveTimePreset,
    },
    PromptRecordingRuleActiveTimeCustom {
        room: i64,
        rule: i64,
    },
    RecordingRuleActiveDays {
        room: i64,
        rule: i64,
    },
    ToggleRecordingRuleActiveDay {
        room: i64,
        rule: i64,
        day: u8,
    },
    CycleRecordingRuleLogic {
        room: i64,
        rule: i64,
    },
    RecordingRuleEditSensorPage {
        room: i64,
        rule: i64,
        page: u16,
    },
    RecordingRuleEditPickSensor {
        room: i64,
        rule: i64,
        device: i64,
    },
    RecordingRuleEditPickSensorOperator {
        room: i64,
        rule: i64,
        device: i64,
        operator: db::camera_recording_rules::ConditionOperator,
    },
    DeleteRecordingRuleCondition {
        room: i64,
        rule: i64,
        condition: i64,
    },
    WizardGroups,
    WizardCancel,
    RecordingRuleEditGroups {
        room: i64,
        rule: i64,
    },
    ToggleRecordingRuleEditGroupItem {
        room: i64,
        rule: i64,
        group: i64,
    },
    ActionGroups,
    ActionGroupDetail {
        group: i64,
    },
    PromptCreateActionGroup,
    PromptRenameActionGroup {
        group: i64,
    },
    ConfirmDeleteActionGroup {
        group: i64,
    },
    DeleteActionGroup {
        group: i64,
    },
    ActionGroupItems {
        group: i64,
        page: u16,
        filter: db::action_groups::ActionGroupItemsFilter,
    },
    ToggleActionGroupItem {
        group: i64,
        device: i64,
        page: u16,
        filter: db::action_groups::ActionGroupItemsFilter,
    },
    ToggleActionGroupAccess {
        group: i64,
    },
    ToggleActionGroupEnabled {
        group: i64,
    },
    ExecuteActionGroup {
        group: i64,
        command: db::action_groups::ActionGroupCommand,
    },
    HaNativeActionDetail {
        action: i64,
    },
    ToggleHaNativeActionAccess {
        action: i64,
    },
    ToggleHaNativeActionEnabled {
        action: i64,
    },
    PromptHaNativeActionAlias {
        action: i64,
    },
    ResetHaNativeActionAlias {
        action: i64,
    },
    ExecuteHaNativeAction {
        action: i64,
    },
    ActionSchedules {
        target: db::action_groups::ActionTargetRef,
    },
    ActionScheduleDetail {
        target: db::action_groups::ActionTargetRef,
        schedule: i64,
    },
    PromptCreateActionScheduleTime {
        target: db::action_groups::ActionTargetRef,
        command: db::action_groups::ActionScheduleCommand,
    },
    PromptEditActionScheduleTime {
        target: db::action_groups::ActionTargetRef,
        schedule: i64,
    },
    ToggleActionScheduleDay {
        target: db::action_groups::ActionTargetRef,
        schedule: i64,
        day: u8,
    },
    CycleActionScheduleCommand {
        target: db::action_groups::ActionTargetRef,
        schedule: i64,
    },
    ToggleActionScheduleEnabled {
        target: db::action_groups::ActionTargetRef,
        schedule: i64,
    },
    ConfirmDeleteActionSchedule {
        target: db::action_groups::ActionTargetRef,
        schedule: i64,
    },
    DeleteActionSchedule {
        target: db::action_groups::ActionTargetRef,
        schedule: i64,
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingRuleGroupRulesFilter {
    All,
    Selected,
    Unselected,
}

impl RenderContext {
    pub fn settings_payload(&self, payload: SettingsPayload) -> Payload {
        match self.settings_origin {
            SettingsOrigin::Home => Payload::Settings(payload),
            SettingsOrigin::Admin => Payload::AdminSettings(payload),
        }
    }

    pub fn settings_root_back_payload(&self) -> Payload {
        match self.settings_origin {
            SettingsOrigin::Home => Payload::Home,
            SettingsOrigin::Admin => Payload::Admin(AdminPayload::ListActions),
        }
    }
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

    debug!(
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
        settings_origin: SettingsOrigin::Home,
    };

    let mut view = match payload {
        Payload::Home => super::screens::home::render(ctx).await?,
        Payload::Control(sub_payload) => router_control(ctx, sub_payload).await?,
        Payload::Settings(sub_payload) => router_settings(ctx, sub_payload).await?,
        Payload::Camera(sub_payload) => router_camera(ctx, sub_payload).await?,
        Payload::Command(_) => super::screens::common::in_dev_menu(ctx, Payload::Home).await?,
        Payload::Admin(sub_payload) => router_admin(ctx, sub_payload).await?,
        Payload::InDev => super::screens::common::in_dev_menu(ctx, Payload::Home).await?,
        Payload::AdminSettings(sub_payload) => {
            let mut ctx = ctx;
            ctx.settings_origin = SettingsOrigin::Admin;
            router_settings(ctx, sub_payload).await?
        }
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
        ControlPayload::ActionGroups => {
            Ok(super::screens::action_groups::render_user_list(ctx).await?)
        }
        ControlPayload::ActionGroupDetail { group } => {
            Ok(super::screens::action_groups::render_group_detail(ctx, group, false).await?)
        }
        ControlPayload::ExecuteActionGroup { group, command } => {
            let result = crate::core::action_groups::execute_action_group(
                group,
                command,
                crate::core::action_groups::ActionActor::User(ctx.user_id),
                ctx.config.clone(),
            )
            .await;
            let mut view =
                super::screens::action_groups::render_group_detail(ctx, group, false).await?;
            match result {
                Ok(result) if result.status() == "ok" => view.notice = Some(result.user_message()),
                Ok(result) => view.alert = Some(result.user_message()),
                Err(error) => view.alert = Some(error.to_string()),
            }
            Ok(view)
        }
        ControlPayload::HaNativeActionDetail { action } => {
            Ok(super::screens::action_groups::render_ha_native_detail(ctx, action, false).await?)
        }
        ControlPayload::ExecuteHaNativeAction { action } => {
            let result = crate::core::action_groups::execute_ha_native_target(
                action,
                crate::core::action_groups::ActionActor::User(ctx.user_id),
                ctx.config.clone(),
            )
            .await;
            let mut view =
                super::screens::action_groups::render_ha_native_detail(ctx, action, false).await?;
            match result {
                Ok(result) if result.status() == "ok" => view.notice = Some(result.user_message()),
                Ok(result) => view.alert = Some(result.user_message()),
                Err(error) => view.alert = Some(error.to_string()),
            }
            Ok(view)
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
        CameraPayload::RecordingGroupModes => {
            Ok(super::screens::cameras::render_recording_group_modes(ctx).await?)
        }
        CameraPayload::RecordingGroupModeDetail { group } => {
            let can_toggle = db::camera_recording_rule_groups::can_user_toggle_group(
                ctx.user_id,
                ctx.config.root_user,
                group,
                &ctx.config.db,
            )
            .await?;
            if !can_toggle {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingGroupModes),
                ));
            }
            Ok(super::screens::cameras::render_recording_group_mode_detail(ctx, group).await?)
        }
        CameraPayload::ToggleRecordingGroupMode { group } => {
            let can_toggle = db::camera_recording_rule_groups::can_user_toggle_group(
                ctx.user_id,
                ctx.config.root_user,
                group,
                &ctx.config.db,
            )
            .await?;
            if !can_toggle {
                return Ok(access_denied_view(
                    ctx,
                    Payload::Camera(CameraPayload::RecordingGroupModes),
                ));
            }
            let enabled =
                db::camera_recording_rule_groups::toggle_group_enabled(group, &ctx.config.db)
                    .await?;
            log_rule_group_activity(
                &ctx,
                group,
                if enabled {
                    "rule_group.enable"
                } else {
                    "rule_group.pause"
                },
                Some("user mode toggle"),
            )
            .await;
            let lang = ctx.lang;
            let mut view =
                super::screens::cameras::render_recording_group_mode_detail(ctx, group).await?;
            view.notice = Some(recording_rule_group_toggle_notice(lang, enabled));
            Ok(view)
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
                let back = ctx.settings_payload(SettingsPayload::ListRooms);
                return Ok(access_denied_view(ctx, back));
            }

            Ok(room::render(ctx, room, RoomViewMode::Settings).await?)
        }
        SettingsPayload::DeviceDetail { room, device } => {
            if !db::access::can_view_device(ctx.user_id, ctx.is_admin, device, &ctx.config.db)
                .await?
            {
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
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
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
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
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
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
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
            }

            super::screens::settings::device_settings::render_state_aliases(ctx, room, device).await
        }
        SettingsPayload::EditStateAlias {
            room,
            device,
            state,
        } => {
            if !ctx.is_admin {
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
            }

            let dev = db::devices::get_device_by_id(device, &ctx.config.db)
                .await?
                .context("Device not found")?;
            let current_alias =
                db::devices::get_state_alias(&dev.entity_id, &state, &ctx.config.db).await?;
            let current_payload = ctx.settings_payload(SettingsPayload::EditStateAlias {
                room,
                device,
                state: state.clone(),
            });
            let back_payload = ctx.settings_payload(SettingsPayload::StateAliases { room, device });
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
                current_payload,
                back_payload,
            ))
        }
        SettingsPayload::ResetStateAlias {
            room,
            device,
            state,
        } => {
            if !ctx.is_admin {
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
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
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
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
        SettingsPayload::ToggleCritical { room, device } => {
            if !ctx.is_admin {
                let back = ctx.settings_payload(SettingsPayload::RoomDetail { room });
                return Ok(access_denied_view(ctx, back));
            }

            let enabled = db::devices::toggle_device_critical(device, &ctx.config.db).await?;
            let mut view =
                super::screens::settings::device_settings::render(ctx, room, device).await?;
            view.notice = Some(if enabled {
                "Устройство помечено как критичное".to_string()
            } else {
                "Критичность устройства снята".to_string()
            });
            Ok(view)
        }
        _ => {
            let back = ctx.settings_payload(SettingsPayload::ListRooms {});
            Ok(super::screens::common::in_dev_menu(ctx, back).await?)
        }
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
            Payload::Admin(AdminPayload::PromptAddUser),
            Payload::Admin(AdminPayload::ListUsers),
        )),
        AdminPayload::PromptDeleteUser => {
            Ok(super::screens::admin::list_actions::render_user_input(
                ctx,
                State::DeleteUser { user_id: 0 },
                "Удаление пользователя",
                "Введите Telegram ID пользователя, у которого нужно забрать доступ.",
                Payload::Admin(AdminPayload::PromptDeleteUser),
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
        AdminPayload::StartRecordingRuleWizard { room } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_start(ctx, room)
                .await?,
        ),
        AdminPayload::WizardPickCamera { room, camera } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_entities(
                ctx, room, camera,
            )
            .await?,
        ),
        AdminPayload::WizardSourcePage { page } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_source_page(
                ctx, page,
            )
            .await?,
        ),
        AdminPayload::WizardPickEntity {
            room,
            camera,
            device,
        } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_modes(
                ctx, room, camera, device,
            )
            .await?,
        ),
        AdminPayload::WizardPickMode {
            room,
            camera,
            device,
            mode,
        } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_tail(
                ctx, room, camera, device, mode,
            )
            .await?,
        ),
        AdminPayload::WizardPickTail {
            room,
            camera,
            device,
            mode,
            tail,
        } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_retention(
                ctx, room, camera, device, mode, tail,
            )
            .await?,
        ),
        AdminPayload::WizardPickRetention {
            room,
            camera,
            device,
            mode,
            tail,
            retention,
        } => render_wizard_compat_active_time(
            ctx, room, camera, device, mode, tail, retention, None,
        )
        .await,
        AdminPayload::WizardPickGroup {
            room,
            camera,
            device,
            mode,
            tail,
            retention,
            group,
        } => render_wizard_compat_active_time(
            ctx, room, camera, device, mode, tail, retention, group,
        )
        .await,
        AdminPayload::WizardAdvancedText {
            room,
            camera,
            device,
            mode,
            tail,
            retention,
            group,
        } => render_wizard_compat_active_time(
            ctx, room, camera, device, mode, tail, retention, group,
        )
        .await,
        AdminPayload::WizardCreateRule {
            room,
            camera,
            device,
            mode,
            tail,
            retention,
            group,
        } => render_wizard_compat_active_time(
            ctx, room, camera, device, mode, tail, retention, group,
        )
        .await,
        AdminPayload::WizardConditions => {
            Ok(super::screens::admin::list_actions::render_recording_rule_wizard_conditions(ctx)
                .await?)
        }
        AdminPayload::WizardAddCondition => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_condition_entities(
                ctx,
            )
            .await?,
        ),
        AdminPayload::WizardConditionEntityPage { page } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_condition_entities_page(
                ctx, page,
            )
            .await?,
        ),
        AdminPayload::WizardPickConditionEntity { device } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_condition_operators(
                ctx, device,
            )
            .await?,
        ),
        AdminPayload::WizardPickConditionOperator { operator } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_condition_value_input(
                ctx, operator,
            )?,
        ),
        AdminPayload::WizardRemoveCondition { index } => {
            update_current_wizard(&ctx, |wizard| {
                let _ = wizard.remove_extra_condition(usize::from(index));
            })?;
            Ok(super::screens::admin::list_actions::render_recording_rule_wizard_conditions(ctx)
                .await?)
        }
        AdminPayload::WizardToggleLogic => {
            update_current_wizard(&ctx, |wizard| wizard.toggle_logic())?;
            Ok(super::screens::admin::list_actions::render_recording_rule_wizard_conditions(ctx)
                .await?)
        }
        AdminPayload::WizardNextTail => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_tail_from_state(ctx)
                .await?,
        ),
        AdminPayload::WizardPickWizardTail { tail } => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_retention_from_state(
                ctx, tail,
            )
            .await?,
        ),
        AdminPayload::WizardPickWizardRetention { retention } => {
            update_current_wizard(&ctx, |wizard| {
                wizard.retention_days = Some(retention);
            })?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_wizard_active_time_from_state(
                    ctx,
                    Some(retention),
                )
                .await?,
            )
        }
        AdminPayload::WizardActiveTime => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_active_time_from_state(
                ctx, None,
            )
            .await?,
        ),
        AdminPayload::WizardPickActiveTimePreset { preset } => {
            let active_time = active_time_from_preset(preset)?;
            update_current_wizard(&ctx, |wizard| {
                wizard.active_time = active_time;
            })?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_wizard_groups_from_state(
                    ctx,
                )
                .await?,
            )
        }
        AdminPayload::WizardPromptActiveTimeCustom => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_active_time_custom_input(
                ctx,
            )?,
        ),
        AdminPayload::WizardGroups => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_groups_from_state(ctx)
                .await?,
        ),
        AdminPayload::EnsureDefaultRuleGroupsForWizard => {
            if current_wizard(&ctx).is_none() {
                let mut view = super::screens::admin::list_actions::render(ctx).await?;
                view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
                return Ok(view);
            }
            db::camera_recording_rule_groups::ensure_default_groups(&ctx.config.db).await?;
            let lang = ctx.lang;
            log_rule_group_activity(&ctx, 0, "rule_group.create_defaults", None).await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_wizard_groups_from_state(
                    ctx,
                )
                .await?;
            view.notice = Some(t(lang, "admin.rule_groups.defaults_created").to_string());
            Ok(view)
        }
        AdminPayload::WizardCancel => cancel_current_wizard(ctx).await,
        AdminPayload::WizardToggleGroup { group } => {
            if current_wizard(&ctx).is_none() {
                return super::screens::admin::list_actions::render_recording_rule_wizard_groups_from_state(ctx)
                    .await;
            }
            if db::camera_recording_rule_groups::get_group(group, &ctx.config.db)
                .await?
                .is_none()
            {
                let lang = ctx.lang;
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_wizard_groups_from_state(
                        ctx,
                    )
                    .await?;
                view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
                return Ok(view);
            }
            update_current_wizard(&ctx, |wizard| wizard.toggle_group(group))?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_wizard_groups_from_state(
                    ctx,
                )
                .await?,
            )
        }
        AdminPayload::WizardConfirmGroups => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_confirm(ctx).await?,
        ),
        AdminPayload::WizardCreateCurrentRule => create_current_wizard_recording_rule(ctx).await,
        AdminPayload::WizardAdvancedCurrentText => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_advanced(ctx)
                .await?,
        ),
        AdminPayload::RecordingRuleEditMenu { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_edit_menu(ctx, room, rule)
                .await?,
        ),
        AdminPayload::RecordingRuleEditActiveTime { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_active_time(
                ctx, room, rule,
            )
            .await?,
        ),
        AdminPayload::SetRecordingRuleActiveTimePreset { room, rule, preset } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            let active_time = active_time_from_preset(preset)?;
            db::camera_recording_rules::update_rule_active_time(
                rule,
                active_time,
                &ctx.config.db,
            )
            .await?;
            let action = if active_time.enabled {
                "recording_rule.active_time_update"
            } else {
                "recording_rule.active_time_reset"
            };
            let activity_message = active_time_log_message(active_time);
            log_recording_rule_activity(&ctx, rule, action, Some(&activity_message)).await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_active_time(
                    ctx, room, rule,
                )
                .await?;
            view.notice = Some("Время активности обновлено".to_string());
            Ok(view)
        }
        AdminPayload::PromptRecordingRuleActiveTimeCustom { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_active_time_custom_input(
                ctx, room, rule,
            )
            .await?,
        ),
        AdminPayload::RecordingRuleActiveDays { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_active_days(ctx, room, rule)
                .await?,
        ),
        AdminPayload::ToggleRecordingRuleActiveDay { room, rule, day } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            let Some(current_rule) =
                db::camera_recording_rules::get_rule(rule, &ctx.config.db).await?
            else {
                return recording_rule_room_mismatch_view(ctx, room).await;
            };
            let active_time = current_rule.active_time();
            if !active_time.enabled
                || !db::camera_recording_rules::active_time_is_valid(active_time)
                || day > 6
            {
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_active_time(
                        ctx, room, rule,
                    )
                    .await?;
                view.alert = Some("Сначала выберите окно времени.".to_string());
                return Ok(view);
            }
            let bit = 1_i64 << day;
            let new_mask = active_time.days_mask ^ bit;
            if new_mask == 0 {
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_active_days(
                        ctx, room, rule,
                    )
                    .await?;
                view.alert = Some("Нужно выбрать хотя бы один день недели.".to_string());
                return Ok(view);
            }
            db::camera_recording_rules::update_rule_active_days_mask(
                rule,
                new_mask,
                &ctx.config.db,
            )
            .await?;
            log_recording_rule_activity(
                &ctx,
                rule,
                "recording_rule.active_time_update",
                Some("days_mask"),
            )
            .await;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_active_days(
                    ctx, room, rule,
                )
                .await?,
            )
        }
        AdminPayload::RecordingRuleEditSensors { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_edit_sensors(
                ctx, room, rule,
            )
            .await?,
        ),
        AdminPayload::RecordingRuleEditGroups { room, rule } => Ok(
            super::screens::admin::list_actions::render_recording_rule_edit_groups(
                ctx, room, rule,
            )
            .await?,
        ),
        AdminPayload::EnsureDefaultRuleGroupsForEdit { room, rule } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            db::camera_recording_rule_groups::ensure_default_groups(&ctx.config.db).await?;
            let lang = ctx.lang;
            log_rule_group_activity(&ctx, 0, "rule_group.create_defaults", None).await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_edit_groups(
                    ctx, room, rule,
                )
                .await?;
            view.notice = Some(t(lang, "admin.rule_groups.defaults_created").to_string());
            Ok(view)
        }
        AdminPayload::ToggleRecordingRuleEditGroupItem { room, rule, group } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            if db::camera_recording_rule_groups::get_group(group, &ctx.config.db)
                .await?
                .is_none()
            {
                let lang = ctx.lang;
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_edit_groups(
                        ctx, room, rule,
                    )
                    .await?;
                view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
                return Ok(view);
            }
            let selected =
                db::camera_recording_rule_groups::toggle_rule_in_group(rule, group, &ctx.config.db)
                    .await?;
            let activity_message = format!("rule {}", rule);
            log_rule_group_activity(
                &ctx,
                group,
                if selected {
                    "rule_group.add_rule"
                } else {
                    "rule_group.remove_rule"
                },
                Some(&activity_message),
            )
            .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_edit_groups(
                    ctx, room, rule,
                )
                .await?;
            view.notice = Some(if selected {
                "Правило добавлено в группу".to_string()
            } else {
                "Правило убрано из группы".to_string()
            });
            Ok(view)
        }
        AdminPayload::RecordingRuleEditSensorPage { room, rule, page } => Ok(
            super::screens::admin::list_actions::render_recording_rule_edit_sensor_page(
                ctx, room, rule, page,
            )
            .await?,
        ),
        AdminPayload::RecordingRuleEditPickSensor { room, rule, device } => Ok(
            super::screens::admin::list_actions::render_recording_rule_edit_sensor_operators(
                ctx, room, rule, device,
            )
            .await?,
        ),
        AdminPayload::RecordingRuleEditPickSensorOperator {
            room,
            rule,
            device,
            operator,
        } => Ok(
            super::screens::admin::list_actions::render_recording_rule_edit_sensor_value_input(
                ctx, room, rule, device, operator,
            )
            .await?,
        ),
        AdminPayload::PromptEditRecordingRuleNumber { room, rule, field } => Ok(
            super::screens::admin::list_actions::render_edit_recording_rule_number_input(
                ctx, room, rule, field,
            )
            .await?,
        ),
        AdminPayload::CycleRecordingRuleLogic { room, rule } => {
            let Some(current_rule) =
                db::camera_recording_rules::get_rule_for_room(rule, room, &ctx.config.db).await?
            else {
                return recording_rule_room_mismatch_view(ctx, room).await;
            };
            let next_logic = match current_rule.logic() {
                db::camera_recording_rules::ConditionLogic::All => {
                    db::camera_recording_rules::ConditionLogic::Any
                }
                db::camera_recording_rules::ConditionLogic::Any => {
                    db::camera_recording_rules::ConditionLogic::All
                }
            };
            db::camera_recording_rules::update_rule_logic(rule, next_logic, &ctx.config.db)
                .await?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_edit_sensors(
                    ctx, room, rule,
                )
                .await?,
            )
        },
        AdminPayload::DeleteRecordingRuleCondition {
            room,
            rule,
            condition,
        } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            let result =
                db::camera_recording_rules::delete_condition(rule, condition, &ctx.config.db)
                    .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_edit_sensors(
                    ctx, room, rule,
                )
                .await?;
            match result {
                Ok(()) => view.notice = Some("Сенсор удален из правила".to_string()),
                Err(error) => view.alert = Some(format!("Не удалось удалить сенсор: {}", error)),
            }
            Ok(view)
        }
        AdminPayload::PromptEditRecordingRule { room, rule } => Ok(
            super::screens::admin::list_actions::render_edit_recording_rule_input(ctx, room, rule)
                .await?,
        ),
        AdminPayload::ToggleRecordingRule { room, rule } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            db::camera_recording_rules::toggle_rule_enabled(rule, &ctx.config.db).await?;
            Ok(super::screens::admin::list_actions::render_recording_rules(ctx, room).await?)
        }
        AdminPayload::ToggleRecordingRuleNotify { room, rule } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            db::camera_recording_rules::toggle_rule_notifications(rule, &ctx.config.db).await?;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_detail(ctx, room, rule)
                    .await?,
            )
        }
        AdminPayload::DuplicateRecordingRule { room, rule } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
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
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
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
        AdminPayload::ActionGroups => {
            Ok(super::screens::action_groups::render_admin_list(ctx).await?)
        }
        AdminPayload::ActionGroupDetail { group } => {
            Ok(super::screens::action_groups::render_group_detail(ctx, group, true).await?)
        }
        AdminPayload::PromptCreateActionGroup => {
            Ok(super::screens::action_groups::render_create_group_input(ctx))
        }
        AdminPayload::PromptRenameActionGroup { group } => {
            Ok(super::screens::action_groups::render_rename_group_input(ctx, group))
        }
        AdminPayload::ConfirmDeleteActionGroup { group } => {
            Ok(super::screens::action_groups::render_confirm_delete_group(ctx, group).await?)
        }
        AdminPayload::DeleteActionGroup { group } => {
            let lang = ctx.lang;
            let result = db::action_groups::delete_action_group(group, &ctx.config.db).await;
            if result.is_ok() {
                log_action_activity(
                    &ctx,
                    "action_group",
                    "action_group",
                    group,
                    "action_group.delete",
                    "ok",
                    None,
                )
                .await;
            }
            let mut view = super::screens::action_groups::render_admin_list(ctx).await?;
            match result {
                Ok(()) => {
                    view.notice =
                        Some(crate::i18n::t(lang, "action_groups.group_deleted").to_string())
                }
                Err(error) => view.alert = Some(error.to_string()),
            }
            Ok(view)
        }
        AdminPayload::ActionGroupItems {
            group,
            page,
            filter,
        } => Ok(
            super::screens::action_groups::render_group_items(ctx, group, page, filter).await?,
        ),
        AdminPayload::ToggleActionGroupItem {
            group,
            device,
            page,
            filter,
        } => {
            let result =
                db::action_groups::toggle_action_group_item(group, device, &ctx.config.db).await;
            if let Ok(selected) = result.as_ref() {
                let message = format!("device {}", device);
                log_action_activity(
                    &ctx,
                    "action_group",
                    "action_group",
                    group,
                    if *selected {
                        "action_group.add_item"
                    } else {
                        "action_group.remove_item"
                    },
                    "ok",
                    Some(&message),
                )
                .await;
            }
            let mut view =
                super::screens::action_groups::render_group_items(ctx, group, page, filter).await?;
            if let Err(error) = result {
                view.alert = Some(error.to_string());
            }
            Ok(view)
        }
        AdminPayload::ToggleActionGroupAccess { group } => {
            let result = db::action_groups::toggle_action_group_access(group, &ctx.config.db).await;
            if let Ok(scope) = result.as_ref() {
                log_action_activity(
                    &ctx,
                    "action_group",
                    "action_group",
                    group,
                    "action_group.access_update",
                    "ok",
                    Some(scope),
                )
                .await;
            }
            let mut view =
                super::screens::action_groups::render_group_detail(ctx, group, true).await?;
            if let Err(error) = result {
                view.alert = Some(error.to_string());
            }
            Ok(view)
        }
        AdminPayload::ToggleActionGroupEnabled { group } => {
            let result =
                db::action_groups::toggle_action_group_enabled(group, &ctx.config.db).await;
            if let Ok(enabled) = result.as_ref() {
                let message = if *enabled { "enabled" } else { "paused" };
                log_action_activity(
                    &ctx,
                    "action_group",
                    "action_group",
                    group,
                    "action_group.enabled_update",
                    "ok",
                    Some(message),
                )
                .await;
            }
            let mut view =
                super::screens::action_groups::render_group_detail(ctx, group, true).await?;
            if let Err(error) = result {
                view.alert = Some(error.to_string());
            }
            Ok(view)
        }
        AdminPayload::ExecuteActionGroup { group, command } => {
            let result = crate::core::action_groups::execute_action_group(
                group,
                command,
                crate::core::action_groups::ActionActor::User(ctx.user_id),
                ctx.config.clone(),
            )
            .await;
            let mut view =
                super::screens::action_groups::render_group_detail(ctx, group, true).await?;
            match result {
                Ok(result) if result.status() == "ok" => view.notice = Some(result.user_message()),
                Ok(result) => view.alert = Some(result.user_message()),
                Err(error) => view.alert = Some(error.to_string()),
            }
            Ok(view)
        }
        AdminPayload::HaNativeActionDetail { action } => {
            Ok(super::screens::action_groups::render_ha_native_detail(ctx, action, true).await?)
        }
        AdminPayload::ToggleHaNativeActionAccess { action } => {
            let result =
                db::action_groups::toggle_ha_native_target_access(action, &ctx.config.db).await;
            if let Ok(scope) = result.as_ref() {
                log_action_activity(
                    &ctx,
                    "ha_native_target",
                    "ha_native_target",
                    action,
                    "ha_native_target.access_update",
                    "ok",
                    Some(scope),
                )
                .await;
            }
            let mut view =
                super::screens::action_groups::render_ha_native_detail(ctx, action, true).await?;
            if let Err(error) = result {
                view.alert = Some(error.to_string());
            }
            Ok(view)
        }
        AdminPayload::ToggleHaNativeActionEnabled { action } => {
            let result =
                db::action_groups::toggle_ha_native_target_enabled(action, &ctx.config.db).await;
            if let Ok(enabled) = result.as_ref() {
                let message = if *enabled { "enabled" } else { "paused" };
                log_action_activity(
                    &ctx,
                    "ha_native_target",
                    "ha_native_target",
                    action,
                    "ha_native_target.enabled_update",
                    "ok",
                    Some(message),
                )
                .await;
            }
            let mut view =
                super::screens::action_groups::render_ha_native_detail(ctx, action, true).await?;
            if let Err(error) = result {
                view.alert = Some(error.to_string());
            }
            Ok(view)
        }
        AdminPayload::PromptHaNativeActionAlias { action } => {
            Ok(super::screens::action_groups::render_ha_alias_input(ctx, action))
        }
        AdminPayload::ResetHaNativeActionAlias { action } => {
            let result =
                db::action_groups::reset_ha_native_target_alias(action, &ctx.config.db).await;
            if result.is_ok() {
                log_action_activity(
                    &ctx,
                    "ha_native_target",
                    "ha_native_target",
                    action,
                    "ha_native_target.alias_reset",
                    "ok",
                    None,
                )
                .await;
            }
            let mut view =
                super::screens::action_groups::render_ha_native_detail(ctx, action, true).await?;
            if let Err(error) = result {
                view.alert = Some(error.to_string());
            }
            Ok(view)
        }
        AdminPayload::ExecuteHaNativeAction { action } => {
            let result = crate::core::action_groups::execute_ha_native_target(
                action,
                crate::core::action_groups::ActionActor::User(ctx.user_id),
                ctx.config.clone(),
            )
            .await;
            let mut view =
                super::screens::action_groups::render_ha_native_detail(ctx, action, true).await?;
            match result {
                Ok(result) if result.status() == "ok" => view.notice = Some(result.user_message()),
                Ok(result) => view.alert = Some(result.user_message()),
                Err(error) => view.alert = Some(error.to_string()),
            }
            Ok(view)
        }
        AdminPayload::ActionSchedules { target } => {
            Ok(super::screens::action_groups::render_schedules(ctx, target).await?)
        }
        AdminPayload::ActionScheduleDetail { target, schedule } => Ok(
            super::screens::action_groups::render_schedule_detail(ctx, target, schedule).await?,
        ),
        AdminPayload::PromptCreateActionScheduleTime { target, command } => Ok(
            super::screens::action_groups::render_schedule_time_input(ctx, target, command).await?,
        ),
        AdminPayload::PromptEditActionScheduleTime { target, schedule } => Ok(
            super::screens::action_groups::render_edit_schedule_time_input(ctx, target, schedule)
                .await?,
        ),
        AdminPayload::ToggleActionScheduleDay {
            target,
            schedule,
            day,
        } => {
            let lang = ctx.lang;
            let Some(current) = db::action_groups::get_action_schedule(schedule, &ctx.config.db).await?
            else {
                let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
                view.alert =
                    Some(crate::i18n::t(lang, "action_groups.schedule_not_found").to_string());
                return Ok(view);
            };
            if current.target_ref()? != target {
                let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
                view.alert =
                    Some(crate::i18n::t(lang, "action_groups.schedule_not_found").to_string());
                return Ok(view);
            }
            let day_bit = 1_i64 << day;
            let mut next_mask = current.days_mask ^ day_bit;
            if next_mask == 0 {
                next_mask = current.days_mask;
            }
            db::action_groups::update_action_schedule_days(schedule, next_mask, &ctx.config.db)
                .await?;
            let message = db::action_groups::format_days_mask(next_mask);
            log_action_activity(
                &ctx,
                "action_group",
                "action_schedule",
                schedule,
                "action_group.schedule_days_update",
                "ok",
                Some(message),
            )
            .await;
            Ok(
                super::screens::action_groups::render_schedule_detail(ctx, target, schedule)
                    .await?,
            )
        }
        AdminPayload::CycleActionScheduleCommand { target, schedule } => {
            let lang = ctx.lang;
            let Some(current) = db::action_groups::get_action_schedule(schedule, &ctx.config.db).await?
            else {
                let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
                view.alert =
                    Some(crate::i18n::t(lang, "action_groups.schedule_not_found").to_string());
                return Ok(view);
            };
            if current.target_ref()? != target {
                let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
                view.alert =
                    Some(crate::i18n::t(lang, "action_groups.schedule_not_found").to_string());
                return Ok(view);
            }
            match db::action_groups::cycle_action_schedule_command(schedule, &ctx.config.db).await {
                Ok(command) => {
                    log_action_activity(
                        &ctx,
                        "action_group",
                        "action_schedule",
                        schedule,
                        "action_group.schedule_command_update",
                        "ok",
                        Some(&command),
                    )
                    .await;
                }
                Err(error) => {
                    let mut view =
                        super::screens::action_groups::render_schedule_detail(ctx, target, schedule)
                            .await?;
                    view.alert = Some(error.to_string());
                    return Ok(view);
                }
            }
            Ok(
                super::screens::action_groups::render_schedule_detail(ctx, target, schedule)
                    .await?,
            )
        }
        AdminPayload::ToggleActionScheduleEnabled { target, schedule } => {
            let lang = ctx.lang;
            let Some(current) = db::action_groups::get_action_schedule(schedule, &ctx.config.db).await?
            else {
                let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
                view.alert =
                    Some(crate::i18n::t(lang, "action_groups.schedule_not_found").to_string());
                return Ok(view);
            };
            if current.target_ref()? != target {
                let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
                view.alert =
                    Some(crate::i18n::t(lang, "action_groups.schedule_not_found").to_string());
                return Ok(view);
            }
            let enabled =
                db::action_groups::toggle_action_schedule_enabled(schedule, &ctx.config.db).await?;
            let message = if enabled { "enabled" } else { "paused" };
            log_action_activity(
                &ctx,
                "action_group",
                "action_schedule",
                schedule,
                "action_group.schedule_enabled_update",
                "ok",
                Some(message),
            )
            .await;
            Ok(
                super::screens::action_groups::render_schedule_detail(ctx, target, schedule)
                    .await?,
            )
        }
        AdminPayload::ConfirmDeleteActionSchedule { target, schedule } => Ok(
            super::screens::action_groups::render_confirm_delete_schedule(ctx, target, schedule)
                .await?,
        ),
        AdminPayload::DeleteActionSchedule { target, schedule } => {
            let lang = ctx.lang;
            let schedule_matches = db::action_groups::get_action_schedule(schedule, &ctx.config.db)
                .await?
                .map(|current| current.target_ref())
                .transpose()?
                .is_some_and(|current_target| current_target == target);
            if !schedule_matches {
                let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
                view.alert =
                    Some(crate::i18n::t(lang, "action_groups.schedule_not_found").to_string());
                return Ok(view);
            }
            let result = db::action_groups::delete_action_schedule(schedule, &ctx.config.db).await;
            if result.is_ok() {
                log_action_activity(
                    &ctx,
                    "action_group",
                    "action_schedule",
                    schedule,
                    "action_group.schedule_delete",
                    "ok",
                    None,
                )
                .await;
            }
            let mut view = super::screens::action_groups::render_schedules(ctx, target).await?;
            match result {
                Ok(()) => {
                    view.notice =
                        Some(crate::i18n::t(lang, "action_groups.schedule_deleted").to_string())
                }
                Err(error) => view.alert = Some(error.to_string()),
            }
            Ok(view)
        }
        AdminPayload::RecordingRuleGroups => {
            Ok(super::screens::admin::list_actions::render_recording_rule_groups(ctx).await?)
        }
        AdminPayload::RecordingRuleGroupsForRoom { room } => Ok(
            super::screens::admin::list_actions::render_recording_rule_groups_for_room(ctx, room)
                .await?,
        ),
        AdminPayload::RecordingRuleGroupDetail { group } => Ok(
            super::screens::admin::list_actions::render_recording_rule_group_detail(ctx, group)
                .await?,
        ),
        AdminPayload::RecordingRuleGroupDetailForRoom { room, group } => Ok(
            super::screens::admin::list_actions::render_recording_rule_group_detail_for_room(
                ctx, room, group,
            )
            .await?,
        ),
        AdminPayload::RecordingRuleGroupRules {
            group,
            page,
            filter,
        } => Ok(
            super::screens::admin::list_actions::render_recording_rule_group_rules(
                ctx, group, page, filter,
            )
            .await?,
        ),
        AdminPayload::RecordingRuleGroupRulesForRoom {
            room,
            group,
            page,
            filter,
        } => Ok(
            super::screens::admin::list_actions::render_recording_rule_group_rules_for_room(
                ctx, room, group, page, filter,
            )
            .await?,
        ),
        AdminPayload::PromptCreateRecordingRuleGroup => Ok(
            super::screens::admin::list_actions::render_create_recording_rule_group_input(
                ctx, None,
            )
            .await?,
        ),
        AdminPayload::PromptCreateRecordingRuleGroupForRoom { room } => Ok(
            super::screens::admin::list_actions::render_create_recording_rule_group_input(
                ctx,
                Some(room),
            )
            .await?,
        ),
        AdminPayload::PromptCreateRecordingRuleGroupForWizard => Ok(
            super::screens::admin::list_actions::render_create_recording_rule_group_for_wizard_input(
                ctx,
            )
            .await?,
        ),
        AdminPayload::PromptCreateRecordingRuleGroupForEdit { room, rule } => Ok(
            super::screens::admin::list_actions::render_create_recording_rule_group_for_edit_input(
                ctx, room, rule,
            )
            .await?,
        ),
        AdminPayload::PromptRenameRecordingRuleGroup { group } => Ok(
            super::screens::admin::list_actions::render_rename_recording_rule_group_input(
                ctx, None, group,
            )
            .await?,
        ),
        AdminPayload::PromptRenameRecordingRuleGroupForRoom { room, group } => Ok(
            super::screens::admin::list_actions::render_rename_recording_rule_group_input(
                ctx,
                Some(room),
                group,
            )
            .await?,
        ),
        AdminPayload::ToggleRecordingRuleGroup { group } => {
            let lang = ctx.lang;
            match db::camera_recording_rule_groups::toggle_group_enabled(group, &ctx.config.db)
                .await
            {
                Ok(enabled) => {
                    log_rule_group_activity(
                        &ctx,
                        group,
                        if enabled {
                            "rule_group.enable"
                        } else {
                            "rule_group.pause"
                        },
                        None,
                    )
                    .await;
                    let mut view =
                        super::screens::admin::list_actions::render_recording_rule_group_detail(
                            ctx, group,
                        )
                        .await?;
                    view.notice = Some(recording_rule_group_toggle_notice(lang, enabled));
                    Ok(view)
                }
                Err(error) => {
                    let mut view =
                        super::screens::admin::list_actions::render_recording_rule_groups(ctx)
                            .await?;
                    view.alert = Some(recording_rule_group_toggle_error(lang, &error));
                    Ok(view)
                }
            }
        }
        AdminPayload::ToggleRecordingRuleGroupForRoom { room, group } => {
            let lang = ctx.lang;
            match db::camera_recording_rule_groups::toggle_group_enabled(group, &ctx.config.db)
                .await
            {
                Ok(enabled) => {
                    log_rule_group_activity(
                        &ctx,
                        group,
                        if enabled {
                            "rule_group.enable"
                        } else {
                            "rule_group.pause"
                        },
                        None,
                    )
                    .await;
                    let mut view = super::screens::admin::list_actions::
                        render_recording_rule_group_detail_for_room(ctx, room, group)
                        .await?;
                    view.notice = Some(recording_rule_group_toggle_notice(lang, enabled));
                    Ok(view)
                }
                Err(error) => {
                    let mut view =
                        super::screens::admin::list_actions::render_recording_rule_groups_for_room(
                            ctx, room,
                        )
                        .await?;
                    view.alert = Some(recording_rule_group_toggle_error(lang, &error));
                    Ok(view)
                }
            }
        }
        AdminPayload::ToggleRecordingRuleGroupDetail { group } => {
            let lang = ctx.lang;
            let enabled =
                db::camera_recording_rule_groups::toggle_group_enabled(group, &ctx.config.db)
                    .await?;
            log_rule_group_activity(
                &ctx,
                group,
                if enabled {
                    "rule_group.enable"
                } else {
                    "rule_group.pause"
                },
                None,
            )
            .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_group_detail(
                    ctx, group,
                )
                .await?;
            view.notice = Some(recording_rule_group_toggle_notice(lang, enabled));
            Ok(view)
        }
        AdminPayload::ToggleRecordingRuleGroupDetailForRoom { room, group } => {
            let lang = ctx.lang;
            let enabled =
                db::camera_recording_rule_groups::toggle_group_enabled(group, &ctx.config.db)
                    .await?;
            log_rule_group_activity(
                &ctx,
                group,
                if enabled {
                    "rule_group.enable"
                } else {
                    "rule_group.pause"
                },
                None,
            )
            .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_group_detail_for_room(
                    ctx, room, group,
                )
                .await?;
            view.notice = Some(recording_rule_group_toggle_notice(lang, enabled));
            Ok(view)
        }
        AdminPayload::CycleRecordingRuleGroupAccess { group } => {
            let access_scope =
                db::camera_recording_rule_groups::cycle_group_access_scope(group, &ctx.config.db)
                    .await?;
            log_rule_group_activity(&ctx, group, "rule_group.access_update", Some(&access_scope))
                .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_group_detail(
                    ctx, group,
                )
                .await?;
            view.notice = Some("Доступ группы обновлен".to_string());
            Ok(view)
        }
        AdminPayload::CycleRecordingRuleGroupAccessForRoom { room, group } => {
            let access_scope =
                db::camera_recording_rule_groups::cycle_group_access_scope(group, &ctx.config.db)
                    .await?;
            log_rule_group_activity(&ctx, group, "rule_group.access_update", Some(&access_scope))
                .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_group_detail_for_room(
                    ctx, room, group,
                )
                .await?;
            view.notice = Some("Доступ группы обновлен".to_string());
            Ok(view)
        }
        AdminPayload::ConfirmDeleteRecordingRuleGroup { group } => {
            let lang = ctx.lang;
            Ok(super::screens::admin::list_actions::render_confirm_action(
                ctx,
                t(lang, "admin.rule_groups.delete_title"),
                t(lang, "admin.rule_groups.delete_confirm"),
                Payload::Admin(AdminPayload::DeleteRecordingRuleGroup { group }),
                Payload::Admin(AdminPayload::RecordingRuleGroupDetail { group }),
            ))
        }
        AdminPayload::ConfirmDeleteRecordingRuleGroupForRoom { room, group } => {
            let lang = ctx.lang;
            Ok(super::screens::admin::list_actions::render_confirm_action(
                ctx,
                t(lang, "admin.rule_groups.delete_title"),
                t(lang, "admin.rule_groups.delete_confirm"),
                Payload::Admin(AdminPayload::DeleteRecordingRuleGroupForRoom { room, group }),
                Payload::Admin(AdminPayload::RecordingRuleGroupDetailForRoom { room, group }),
            ))
        }
        AdminPayload::DeleteRecordingRuleGroup { group } => {
            let delete_result =
                db::camera_recording_rule_groups::delete_group(group, &ctx.config.db).await;
            let lang = ctx.lang;
            let (notice, alert) = match delete_result {
                Ok(()) => {
                    log_rule_group_activity(&ctx, group, "rule_group.delete", None).await;
                    (Some(t(lang, "admin.rule_groups.deleted").to_string()), None)
                }
                Err(error) => (None, Some(recording_rule_group_delete_error(lang, &error))),
            };
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_groups(ctx).await?;
            view.notice = notice;
            view.alert = alert;
            Ok(view)
        }
        AdminPayload::DeleteRecordingRuleGroupForRoom { room, group } => {
            let delete_result =
                db::camera_recording_rule_groups::delete_group(group, &ctx.config.db).await;
            let lang = ctx.lang;
            let (notice, alert) = match delete_result {
                Ok(()) => {
                    log_rule_group_activity(&ctx, group, "rule_group.delete", None).await;
                    (Some(t(lang, "admin.rule_groups.deleted").to_string()), None)
                }
                Err(error) => (None, Some(recording_rule_group_delete_error(lang, &error))),
            };
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_groups_for_room(
                    ctx, room,
                )
                .await?;
            view.notice = notice;
            view.alert = alert;
            Ok(view)
        }
        AdminPayload::ToggleRecordingRuleGroupRule {
            group,
            rule,
            page,
            filter,
        } => {
            if db::camera_recording_rule_groups::get_group(group, &ctx.config.db)
                .await?
                .is_none()
            {
                let lang = ctx.lang;
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_groups(ctx).await?;
                view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
                return Ok(view);
            }
            if db::camera_recording_rules::get_rule(rule, &ctx.config.db)
                .await?
                .is_none()
            {
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_group_rules(
                        ctx, group, page, filter,
                    )
                    .await?;
                view.alert = Some("Правило не найдено или удалено".to_string());
                return Ok(view);
            }
            let selected =
                db::camera_recording_rule_groups::toggle_rule_in_group(rule, group, &ctx.config.db)
                    .await?;
            let activity_message = format!("rule {}", rule);
            log_rule_group_activity(
                &ctx,
                group,
                if selected {
                    "rule_group.add_rule"
                } else {
                    "rule_group.remove_rule"
                },
                Some(&activity_message),
            )
            .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_group_rules(
                    ctx, group, page, filter,
                )
                .await?;
            view.notice = Some(if selected {
                "Правило добавлено в группу".to_string()
            } else {
                "Правило убрано из группы".to_string()
            });
            Ok(view)
        }
        AdminPayload::ToggleRecordingRuleGroupRuleForRoom {
            room,
            group,
            rule,
            page,
            filter,
        } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            if db::camera_recording_rule_groups::get_group(group, &ctx.config.db)
                .await?
                .is_none()
            {
                let lang = ctx.lang;
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_groups_for_room(
                        ctx, room,
                    )
                    .await?;
                view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
                return Ok(view);
            }
            let selected =
                db::camera_recording_rule_groups::toggle_rule_in_group(rule, group, &ctx.config.db)
                    .await?;
            let activity_message = format!("rule {}", rule);
            log_rule_group_activity(
                &ctx,
                group,
                if selected {
                    "rule_group.add_rule"
                } else {
                    "rule_group.remove_rule"
                },
                Some(&activity_message),
            )
            .await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_group_rules_for_room(
                    ctx, room, group, page, filter,
                )
                .await?;
            view.notice = Some(if selected {
                "Правило добавлено в группу".to_string()
            } else {
                "Правило убрано из группы".to_string()
            });
            Ok(view)
        }
        AdminPayload::ToggleRuleGroupItem { room, rule, group } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
            if db::camera_recording_rule_groups::get_group(group, &ctx.config.db)
                .await?
                .is_none()
            {
                let lang = ctx.lang;
                let mut view =
                    super::screens::admin::list_actions::render_recording_rule_detail(
                        ctx, room, rule,
                    )
                    .await?;
                view.alert = Some(t(lang, "admin.rule_groups.not_found").to_string());
                return Ok(view);
            }
            let selected =
                db::camera_recording_rule_groups::toggle_rule_in_group(rule, group, &ctx.config.db)
                    .await?;
            let activity_message = format!("rule {}", rule);
            log_rule_group_activity(
                &ctx,
                group,
                if selected {
                    "rule_group.add_rule"
                } else {
                    "rule_group.remove_rule"
                },
                Some(&activity_message),
            )
            .await;
            Ok(
                super::screens::admin::list_actions::render_recording_rule_detail(ctx, room, rule)
                    .await?,
            )
        }
        AdminPayload::EnsureDefaultRuleGroups => {
            db::camera_recording_rule_groups::ensure_default_groups(&ctx.config.db).await?;
            let lang = ctx.lang;
            log_rule_group_activity(&ctx, 0, "rule_group.create_defaults", None).await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_groups(ctx).await?;
            view.notice = Some(t(lang, "admin.rule_groups.defaults_created").to_string());
            Ok(view)
        }
        AdminPayload::EnsureDefaultRuleGroupsForRoom { room } => {
            db::camera_recording_rule_groups::ensure_default_groups(&ctx.config.db).await?;
            let lang = ctx.lang;
            log_rule_group_activity(&ctx, 0, "rule_group.create_defaults", None).await;
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_groups_for_room(
                    ctx, room,
                )
                .await?;
            view.notice = Some(t(lang, "admin.rule_groups.defaults_created").to_string());
            Ok(view)
        }
        AdminPayload::ToggleRecordingRuleDetail { room, rule } => {
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
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
            if !recording_rule_belongs_to_room(&ctx, room, rule).await? {
                return recording_rule_room_mismatch_view(ctx, room).await;
            }
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
        AdminPayload::ToggleUserVoice { id } => {
            if id != ctx.config.root_user {
                let enabled = db::access::toggle_user_voice_access(id, &ctx.config.db).await?;
                let mut view =
                    super::screens::admin::list_actions::render_user_profile(ctx, id).await?;
                view.notice = Some(if enabled {
                    "Голосовые команды включены".to_string()
                } else {
                    "Голосовые команды выключены".to_string()
                });
                Ok(view)
            } else {
                let mut view =
                    super::screens::admin::list_actions::render_user_profile(ctx, id).await?;
                view.alert = Some("Root всегда может использовать голосовые команды".to_string());
                Ok(view)
            }
        }
        AdminPayload::CycleUserVoiceEngine { id } => {
            let engine = db::access::cycle_user_voice_command_engine(
                id,
                ctx.config.voice_command_engine,
                &ctx.config.db,
            )
            .await?;
            let mut view =
                super::screens::admin::list_actions::render_user_profile(ctx, id).await?;
            view.notice = Some(format!("Voice engine: {}", engine.label()));
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

async fn recording_rule_belongs_to_room(
    ctx: &RenderContext,
    room_id: i64,
    rule_id: i64,
) -> anyhow::Result<bool> {
    Ok(
        db::camera_recording_rules::get_rule_for_room(rule_id, room_id, &ctx.config.db)
            .await?
            .is_some(),
    )
}

async fn recording_rule_room_mismatch_view(
    ctx: RenderContext,
    room_id: i64,
) -> anyhow::Result<View> {
    let mut view =
        super::screens::admin::list_actions::render_recording_rules(ctx, room_id).await?;
    view.alert = Some("Правило не найдено или не принадлежит выбранной комнате".to_string());
    Ok(view)
}

fn recording_rule_group_delete_error(lang: Language, error: &anyhow::Error) -> String {
    if error.to_string().contains("not found") {
        t(lang, "admin.rule_groups.not_found").to_string()
    } else {
        format!("{}: {}", t(lang, "admin.rule_groups.delete_failed"), error)
    }
}

fn recording_rule_group_toggle_notice(lang: Language, enabled: bool) -> String {
    t(
        lang,
        if enabled {
            "admin.rule_groups.enabled_notice"
        } else {
            "admin.rule_groups.paused_notice"
        },
    )
    .to_string()
}

fn recording_rule_group_toggle_error(lang: Language, error: &anyhow::Error) -> String {
    if error.to_string().contains("not found") {
        t(lang, "admin.rule_groups.not_found").to_string()
    } else {
        format!("{}: {}", t(lang, "admin.rule_groups.toggle_failed"), error)
    }
}

async fn log_rule_group_activity(
    ctx: &RenderContext,
    group_id: i64,
    action: &'static str,
    message: Option<&str>,
) {
    let entity_id = if group_id > 0 {
        Some(group_id.to_string())
    } else {
        None
    };
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(ctx.user_id),
            kind: "recording",
            entity_type: "rule_group",
            entity_id: entity_id.as_deref(),
            action,
            status: "ok",
            message,
        },
        &ctx.config.db,
    )
    .await;
}

async fn log_recording_rule_activity(
    ctx: &RenderContext,
    rule_id: i64,
    action: &'static str,
    message: Option<&str>,
) {
    let entity_id = rule_id.to_string();
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(ctx.user_id),
            kind: "recording",
            entity_type: "recording_rule",
            entity_id: Some(&entity_id),
            action,
            status: "ok",
            message,
        },
        &ctx.config.db,
    )
    .await;
}

async fn log_action_activity(
    ctx: &RenderContext,
    kind: &'static str,
    entity_type: &'static str,
    entity_id: i64,
    action: &'static str,
    status: &'static str,
    message: Option<&str>,
) {
    let entity_id = entity_id.to_string();
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(ctx.user_id),
            kind,
            entity_type,
            entity_id: Some(&entity_id),
            action,
            status,
            message,
        },
        &ctx.config.db,
    )
    .await;
}

fn active_time_from_preset(
    preset: RecordingRuleActiveTimePreset,
) -> anyhow::Result<db::camera_recording_rules::RecordingRuleActiveTime> {
    use db::camera_recording_rules::{RecordingRuleActiveTime, ACTIVE_TIME_ALL_DAYS_MASK};

    Ok(match preset {
        RecordingRuleActiveTimePreset::Always => RecordingRuleActiveTime::always(),
        RecordingRuleActiveTimePreset::Night => {
            RecordingRuleActiveTime::window(22 * 60, 7 * 60, ACTIVE_TIME_ALL_DAYS_MASK)?
        }
        RecordingRuleActiveTimePreset::Day => {
            RecordingRuleActiveTime::window(8 * 60, 20 * 60, ACTIVE_TIME_ALL_DAYS_MASK)?
        }
    })
}

fn active_time_log_message(
    active_time: db::camera_recording_rules::RecordingRuleActiveTime,
) -> String {
    if !active_time.enabled {
        return "always".to_string();
    }

    format!(
        "{}-{} mask={}",
        db::camera_recording_rules::format_minute(active_time.from_minute),
        db::camera_recording_rules::format_minute(active_time.to_minute),
        active_time.days_mask
    )
}

async fn cancel_current_wizard(ctx: RenderContext) -> anyhow::Result<View> {
    let room_id = current_wizard(&ctx).map(|wizard| wizard.room_id);
    {
        if let Some(mut session) = ctx.config.sessions.get_mut(&ctx.user_id) {
            session.recording_rule_wizard = None;
        }
    }

    if let Some(room_id) = room_id.filter(|room_id| *room_id > 0) {
        let mut view =
            super::screens::admin::list_actions::render_recording_rules(ctx, room_id).await?;
        view.notice = Some("Создание правила отменено".to_string());
        Ok(view)
    } else {
        let mut view = super::screens::admin::list_actions::render(ctx).await?;
        view.notice = Some("Создание правила отменено".to_string());
        Ok(view)
    }
}

async fn create_current_wizard_recording_rule(ctx: RenderContext) -> anyhow::Result<View> {
    let Some(wizard) = current_wizard(&ctx) else {
        let mut view = super::screens::admin::list_actions::render(ctx).await?;
        view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
        return Ok(view);
    };
    let Some(camera_id) = wizard.camera_id else {
        let mut view =
            super::screens::admin::list_actions::render_recording_rules(ctx, wizard.room_id)
                .await?;
        view.alert = Some("Камера не выбрана".to_string());
        return Ok(view);
    };
    let Some(source_device_id) = wizard.source_device_id else {
        let mut view =
            super::screens::admin::list_actions::render_recording_rules(ctx, wizard.room_id)
                .await?;
        view.alert = Some("Источник не выбран".to_string());
        return Ok(view);
    };
    let Some(source_mode) = wizard.source_mode else {
        let mut view =
            super::screens::admin::list_actions::render_recording_rules(ctx, wizard.room_id)
                .await?;
        view.alert = Some("Событие источника не выбрано".to_string());
        return Ok(view);
    };

    let Some(camera) = db::cameras::get_camera(camera_id, &ctx.config.db).await? else {
        let mut view =
            super::screens::admin::list_actions::render_recording_rules(ctx, wizard.room_id)
                .await?;
        view.alert = Some("Камера не найдена".to_string());
        return Ok(view);
    };
    if camera.room_id != Some(wizard.room_id) {
        let mut view =
            super::screens::admin::list_actions::render_recording_rules(ctx, wizard.room_id)
                .await?;
        view.alert = Some("Камера не принадлежит выбранной комнате".to_string());
        return Ok(view);
    }

    let Some(source) =
        db::devices::get_recording_wizard_candidate(source_device_id, &ctx.config.db).await?
    else {
        let mut view =
            super::screens::admin::list_actions::render_recording_rules(ctx, wizard.room_id)
                .await?;
        view.alert = Some("Источник не найден или архивирован".to_string());
        return Ok(view);
    };

    let tail_seconds = wizard
        .tail_seconds
        .unwrap_or(60)
        .max(5)
        .min(ctx.config.camera_recording_max_tail_seconds);
    let max_segment_seconds = ctx.config.camera_recording_max_segment_seconds;
    let retention_days = wizard.retention_days.unwrap_or(30).clamp(1, 365);
    let conditions = crate::bot::recording_rule_wizard::build_final_conditions(
        &source.entity_id,
        source_mode,
        wizard.source_value.as_deref(),
        &wizard.extra_conditions,
    )
    .map_err(anyhow::Error::msg)?;

    if let Some(existing_rule_id) = find_duplicate_wizard_rule(
        camera_id,
        wizard.condition_logic,
        &conditions,
        wizard.active_time,
        &ctx.config.db,
    )
    .await?
    {
        let mut view = super::screens::admin::list_actions::render_recording_rule_detail(
            ctx,
            wizard.room_id,
            existing_rule_id,
        )
        .await?;
        view.notice = Some("Похожее правило уже существует".to_string());
        return Ok(view);
    }

    let name = crate::bot::recording_rule_wizard::build_rule_name(
        ctx.lang,
        &source.display_name,
        source_mode,
        wizard.source_value.as_deref(),
    );
    let condition_drafts = conditions
        .iter()
        .map(
            |condition| db::camera_recording_rules::NewRecordingConditionDraft {
                entity_id: &condition.entity_id,
                operator: condition.operator,
                from_state: condition.from_state.as_deref(),
                to_state: condition.to_state.as_deref(),
                value: condition.value.as_deref(),
            },
        )
        .collect::<Vec<_>>();

    let rule_id = db::camera_recording_rules::create_rule_with_conditions_and_groups(
        db::camera_recording_rules::NewRecordingRule {
            name: &name,
            camera_id,
            condition_logic: wizard.condition_logic,
            tail_seconds: i64::from(tail_seconds),
            max_segment_seconds: i64::from(max_segment_seconds),
            cooldown_s: i64::from(crate::bot::recording_rule_wizard::default_cooldown_s(
                source_mode,
            )),
            retention_days: i64::from(retention_days),
        },
        &condition_drafts,
        &wizard.group_ids,
        &ctx.config.db,
    )
    .await?;

    if wizard.active_time.enabled {
        db::camera_recording_rules::update_rule_active_time(
            rule_id,
            wizard.active_time,
            &ctx.config.db,
        )
        .await?;
    }

    if let Some(mut session) = ctx.config.sessions.get_mut(&ctx.user_id) {
        session.recording_rule_wizard = None;
    }

    let rule_id_text = rule_id.to_string();
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: Some(ctx.user_id),
            kind: "recording",
            entity_type: "rule",
            entity_id: Some(&rule_id_text),
            action: "recording_rule_wizard_created",
            status: "ok",
            message: Some(&name),
        },
        &ctx.config.db,
    )
    .await;

    let mut view = super::screens::admin::list_actions::render_recording_rule_detail(
        ctx,
        wizard.room_id,
        rule_id,
    )
    .await?;
    view.notice = Some("Правило создано".to_string());
    Ok(view)
}

async fn find_duplicate_wizard_rule(
    camera_id: i64,
    logic: db::camera_recording_rules::ConditionLogic,
    conditions: &[crate::bot::recording_rule_wizard::WizardCondition],
    active_time: db::camera_recording_rules::RecordingRuleActiveTime,
    pool: &sqlx::SqlitePool,
) -> anyhow::Result<Option<i64>> {
    let expected = wizard_condition_keys(conditions);
    for rule in db::camera_recording_rules::list_rules(pool).await? {
        if rule.camera_id != camera_id
            || rule.logic() != logic
            || rule.active_time() != active_time
            || !rule.is_enabled()
        {
            continue;
        }

        let existing_conditions =
            db::camera_recording_rules::list_conditions(rule.id, pool).await?;
        if recording_condition_keys(&existing_conditions) == expected {
            return Ok(Some(rule.id));
        }
    }

    Ok(None)
}

fn current_wizard(
    ctx: &RenderContext,
) -> Option<crate::bot::recording_rule_wizard::RecordingRuleWizard> {
    ctx.config
        .sessions
        .get(&ctx.user_id)
        .and_then(|session| session.recording_rule_wizard.clone())
}

fn update_current_wizard<F>(ctx: &RenderContext, update: F) -> anyhow::Result<bool>
where
    F: FnOnce(&mut crate::bot::recording_rule_wizard::RecordingRuleWizard),
{
    let Some(mut session) = ctx.config.sessions.get_mut(&ctx.user_id) else {
        log::warn!(
            "Recording rule wizard callback ignored for user {}: session is missing",
            ctx.user_id
        );
        return Ok(false);
    };
    let Some(wizard) = session.recording_rule_wizard.as_mut() else {
        log::warn!(
            "Recording rule wizard callback ignored for user {}: wizard is missing",
            ctx.user_id
        );
        return Ok(false);
    };
    update(wizard);
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn render_wizard_compat_active_time(
    ctx: RenderContext,
    room: i64,
    camera: i64,
    device: i64,
    mode: WizardTriggerMode,
    tail: u32,
    retention: u32,
    group: Option<i64>,
) -> anyhow::Result<View> {
    match update_wizard_compat_selection(&ctx, room, camera, device, mode, tail, retention, group) {
        Ok(()) => Ok(
            super::screens::admin::list_actions::render_recording_rule_wizard_active_time_from_state(
                ctx,
                None,
            )
            .await?,
        ),
        Err(err) => {
            log::warn!(
                "Legacy recording rule wizard callback for user {} is stale: {}",
                ctx.user_id,
                err
            );
            let mut view =
                super::screens::admin::list_actions::render_recording_rule_wizard_start(ctx, room)
                    .await?;
            view.alert = Some("Сессия мастера устарела. Откройте мастер заново.".to_string());
            Ok(view)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_wizard_compat_selection(
    ctx: &RenderContext,
    room: i64,
    camera: i64,
    device: i64,
    mode: WizardTriggerMode,
    tail: u32,
    retention: u32,
    group: Option<i64>,
) -> anyhow::Result<()> {
    let Some(mut session) = ctx.config.sessions.get_mut(&ctx.user_id) else {
        return Err(anyhow::anyhow!("User session not found"));
    };
    let wizard = session
        .recording_rule_wizard
        .get_or_insert_with(|| crate::bot::recording_rule_wizard::RecordingRuleWizard::new(room));
    let selection_changed = wizard.room_id != room
        || wizard.camera_id != Some(camera)
        || wizard.source_device_id != Some(device)
        || wizard.source_mode != Some(mode)
        || wizard.tail_seconds != Some(tail)
        || wizard.retention_days != Some(retention);
    {
        wizard.room_id = room;
        wizard.camera_id = Some(camera);
        wizard.source_device_id = Some(device);
        wizard.source_mode = Some(mode);
        wizard.tail_seconds = Some(tail);
        wizard.retention_days = Some(retention);
        if selection_changed {
            wizard.active_time = db::camera_recording_rules::RecordingRuleActiveTime::always();
        }
        wizard.group_ids = group.into_iter().collect();
    }
    Ok(())
}

fn wizard_condition_keys(
    conditions: &[crate::bot::recording_rule_wizard::WizardCondition],
) -> Vec<String> {
    let mut keys = conditions
        .iter()
        .map(|condition| {
            format!(
                "{}|{}|{}|{}|{}",
                condition.entity_id,
                condition.operator.as_str(),
                condition.from_state.as_deref().unwrap_or(""),
                condition.to_state.as_deref().unwrap_or(""),
                condition.value.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

fn recording_condition_keys(
    conditions: &[db::camera_recording_rules::RecordingRuleCondition],
) -> Vec<String> {
    let mut keys = conditions
        .iter()
        .map(|condition| {
            format!(
                "{}|{}|{}|{}|{}",
                condition.entity_id,
                condition.operator,
                condition.from_state.as_deref().unwrap_or(""),
                condition.to_state.as_deref().unwrap_or(""),
                condition.value.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys
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
            Payload::Admin(AdminPayload::RecordingRuleEditMenu {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::RecordingRuleEditSensors {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::RecordingRuleEditGroups {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleEditGroupItem {
                room: 1_000_000,
                rule: 2_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::PromptEditRecordingRuleNumber {
                room: 1_000_000,
                rule: 2_000_000,
                field: RecordingRuleEditField::TailSeconds,
            }),
            Payload::Admin(AdminPayload::CycleRecordingRuleLogic {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::RecordingRuleEditSensorPage {
                room: 1_000_000,
                rule: 2_000_000,
                page: 12,
            }),
            Payload::Admin(AdminPayload::RecordingRuleEditPickSensor {
                room: 1_000_000,
                rule: 2_000_000,
                device: 3_000_000,
            }),
            Payload::Admin(AdminPayload::RecordingRuleEditPickSensorOperator {
                room: 1_000_000,
                rule: 2_000_000,
                device: 3_000_000,
                operator: db::camera_recording_rules::ConditionOperator::Above,
            }),
            Payload::Admin(AdminPayload::DeleteRecordingRuleCondition {
                room: 1_000_000,
                rule: 2_000_000,
                condition: 4_000_000,
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
            Payload::Admin(AdminPayload::RecordingRuleGroupDetail { group: 3_000_000 }),
            Payload::Admin(AdminPayload::RecordingRuleGroupDetailForRoom {
                room: 1_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::RecordingRuleGroupRules {
                group: 3_000_000,
                page: 12,
                filter: RecordingRuleGroupRulesFilter::Selected,
            }),
            Payload::Admin(AdminPayload::RecordingRuleGroupRulesForRoom {
                room: 1_000_000,
                group: 3_000_000,
                page: 12,
                filter: RecordingRuleGroupRulesFilter::Unselected,
            }),
            Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroup),
            Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroupForRoom { room: 1_000_000 }),
            Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroupForWizard),
            Payload::Admin(AdminPayload::PromptCreateRecordingRuleGroupForEdit {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::PromptRenameRecordingRuleGroup { group: 3_000_000 }),
            Payload::Admin(AdminPayload::PromptRenameRecordingRuleGroupForRoom {
                room: 1_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroup { group: 3_000_000 }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroupForRoom {
                room: 1_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroupDetail { group: 3_000_000 }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroupDetailForRoom {
                room: 1_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::ConfirmDeleteRecordingRuleGroup { group: 3_000_000 }),
            Payload::Admin(AdminPayload::ConfirmDeleteRecordingRuleGroupForRoom {
                room: 1_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::DeleteRecordingRuleGroup { group: 3_000_000 }),
            Payload::Admin(AdminPayload::DeleteRecordingRuleGroupForRoom {
                room: 1_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroupRule {
                group: 3_000_000,
                rule: 2_000_000,
                page: 12,
                filter: RecordingRuleGroupRulesFilter::Selected,
            }),
            Payload::Admin(AdminPayload::ToggleRecordingRuleGroupRuleForRoom {
                room: 1_000_000,
                group: 3_000_000,
                rule: 2_000_000,
                page: 12,
                filter: RecordingRuleGroupRulesFilter::Unselected,
            }),
            Payload::Admin(AdminPayload::ToggleRuleGroupItem {
                room: 1_000_000,
                rule: 2_000_000,
                group: 3_000_000,
            }),
            Payload::Admin(AdminPayload::ToggleUserVoice { id: 9_999_999_999 }),
            Payload::Admin(AdminPayload::CycleUserVoiceEngine { id: 9_999_999_999 }),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroups),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForRoom { room: 1_000_000 }),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForWizard),
            Payload::Admin(AdminPayload::EnsureDefaultRuleGroupsForEdit {
                room: 1_000_000,
                rule: 2_000_000,
            }),
            Payload::Admin(AdminPayload::StartRecordingRuleWizard { room: 1_000_000 }),
            Payload::Admin(AdminPayload::WizardPickCamera {
                room: 1_000_000,
                camera: 2_000_000,
            }),
            Payload::Admin(AdminPayload::WizardPickEntity {
                room: 1_000_000,
                camera: 2_000_000,
                device: 3_000_000,
            }),
            Payload::Admin(AdminPayload::WizardPickMode {
                room: 1_000_000,
                camera: 2_000_000,
                device: 3_000_000,
                mode: WizardTriggerMode::OpenAndClose,
            }),
            Payload::Admin(AdminPayload::WizardCreateRule {
                room: 1_000_000,
                camera: 2_000_000,
                device: 3_000_000,
                mode: WizardTriggerMode::OpenAndClose,
                tail: 300,
                retention: 90,
                group: Some(4_000_000),
            }),
            Payload::Admin(AdminPayload::WizardGroups),
            Payload::Admin(AdminPayload::WizardCancel),
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

    #[test]
    fn legacy_admin_list_actions_payload_still_decodes() {
        let restored =
            Payload::from_string("BAA").expect("legacy admin list actions payload should decode");

        assert_eq!(restored, Payload::Admin(AdminPayload::ListActions));
    }

    #[test]
    fn all_router_payload_paths_roundtrip_and_fit_callback_limit() {
        let payloads = all_router_payload_samples();
        assert!(
            payloads.len() >= 100,
            "router payload sample list is unexpectedly small"
        );

        for original in payloads {
            let route = payload_route_name(&original);
            let encoded = original.to_string();

            assert!(!encoded.is_empty(), "{} encoded payload is empty", route);
            assert!(
                encoded.len() <= 64,
                "{} payload overflow: {} bytes used. Max is 64. Payload: {:?}",
                route,
                encoded.len(),
                original
            );

            let restored = Payload::from_string(&encoded)
                .unwrap_or_else(|error| panic!("{} failed to decode: {}", route, error));

            assert_eq!(
                restored, original,
                "{} payload roundtrip changed the route",
                route
            );
        }
    }

    fn all_router_payload_samples() -> Vec<Payload> {
        let mut payloads = vec![
            Payload::Home,
            Payload::InDev,
            Payload::Command(CommandPayload::Confirm { id: 1 }),
            Payload::Command(CommandPayload::Cancel { id: 1 }),
        ];

        payloads.extend(control_payload_samples().into_iter().map(Payload::Control));
        payloads.extend(
            settings_payload_samples()
                .into_iter()
                .map(Payload::Settings),
        );
        payloads.extend(
            settings_payload_samples()
                .into_iter()
                .map(Payload::AdminSettings),
        );
        payloads.extend(camera_payload_samples().into_iter().map(Payload::Camera));
        payloads.extend(admin_payload_samples().into_iter().map(Payload::Admin));
        payloads
    }

    fn control_payload_samples() -> Vec<ControlPayload> {
        vec![
            ControlPayload::ListRooms,
            ControlPayload::RoomDetail { room: 1 },
            ControlPayload::DeviceControl { room: 1, device: 2 },
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::Toggle,
            },
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::TurnOn,
            },
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::TurnOff,
            },
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::SetLevel(42),
            },
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::SetTemp(21.5),
            },
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::ShowChart { h: 168, o: -24 },
            },
            ControlPayload::QuickAction {
                room: 1,
                device: 2,
                cmd: DeviceCmd::EnterManualInput,
            },
            ControlPayload::ActionGroups,
            ControlPayload::ActionGroupDetail { group: 3 },
            ControlPayload::ExecuteActionGroup {
                group: 3,
                command: db::action_groups::ActionGroupCommand::Toggle,
            },
            ControlPayload::ExecuteActionGroup {
                group: 3,
                command: db::action_groups::ActionGroupCommand::TurnOn,
            },
            ControlPayload::HaNativeActionDetail { action: 4 },
            ControlPayload::ExecuteHaNativeAction { action: 4 },
        ]
    }

    fn settings_payload_samples() -> Vec<SettingsPayload> {
        vec![
            SettingsPayload::ListRooms,
            SettingsPayload::RoomDetail { room: 1 },
            SettingsPayload::DeviceDetail { room: 1, device: 2 },
            SettingsPayload::ToggleNotify { room: 1, device: 2 },
            SettingsPayload::ToggleHide { room: 1, device: 2 },
            SettingsPayload::EditName { room: 1, device: 2 },
            SettingsPayload::StateAliases { room: 1, device: 2 },
            SettingsPayload::EditStateAlias {
                room: 1,
                device: 2,
                state: "on".to_string(),
            },
            SettingsPayload::ResetStateAlias {
                room: 1,
                device: 2,
                state: "off".to_string(),
            },
            SettingsPayload::ToggleStateInversion { room: 1, device: 2 },
            SettingsPayload::ToggleCritical { room: 1, device: 2 },
        ]
    }

    fn camera_payload_samples() -> Vec<CameraPayload> {
        vec![
            CameraPayload::ListCameras,
            CameraPayload::CameraDetail { id: 1 },
            CameraPayload::Snapshot { id: 1 },
            CameraPayload::Clip { id: 1, seconds: 10 },
            CameraPayload::RecordingArchive { camera: 1 },
            CameraPayload::RecordingSession {
                camera: 1,
                session: 2,
            },
            CameraPayload::SendRecordingSegment {
                camera: 1,
                session: 2,
                segment: 3,
            },
            CameraPayload::SendRecordingAll {
                camera: 1,
                session: 2,
            },
            CameraPayload::ConfirmDeleteRecording {
                camera: 1,
                session: 2,
            },
            CameraPayload::DeleteRecording {
                camera: 1,
                session: 2,
            },
            CameraPayload::StopRecording {
                camera: 1,
                session: 2,
            },
            CameraPayload::RecordingGroupModes,
            CameraPayload::RecordingGroupModeDetail { group: 3 },
            CameraPayload::ToggleRecordingGroupMode { group: 3 },
        ]
    }

    fn admin_payload_samples() -> Vec<AdminPayload> {
        let mut payloads = vec![
            AdminPayload::ListActions,
            AdminPayload::ListUsers,
            AdminPayload::Status,
            AdminPayload::ConfirmBackup,
            AdminPayload::CreateBackup,
            AdminPayload::PromptAddUser,
            AdminPayload::PromptDeleteUser,
            AdminPayload::AddUser { id: 9 },
            AdminPayload::CameraRooms,
            AdminPayload::RoomCameras { room: 1 },
            AdminPayload::RoomCameraDetail { room: 1, camera: 2 },
            AdminPayload::RoomCameraSnapshot { room: 1, camera: 2 },
            AdminPayload::RoomCameraClip {
                room: 1,
                camera: 2,
                seconds: 10,
            },
            AdminPayload::PromptAddCamera { room: 1 },
            AdminPayload::ConfirmDeleteCamera { room: 1, camera: 2 },
            AdminPayload::DeleteCamera { room: 1, camera: 2 },
            AdminPayload::RecordingRules { room: 1 },
            AdminPayload::PromptAddRecordingRule { room: 1 },
            AdminPayload::ToggleRecordingRule { room: 1, rule: 2 },
            AdminPayload::ConfirmDeleteRecordingRule { room: 1, rule: 2 },
            AdminPayload::DeleteRecordingRule { room: 1, rule: 2 },
            AdminPayload::CycleRecordingDefaultRetention { room: 1 },
            AdminPayload::CycleRecordingStorageQuota { room: 1 },
            AdminPayload::UiBackground,
            AdminPayload::SetUiBackgroundCamera { camera: None },
            AdminPayload::SetUiBackgroundCamera { camera: Some(2) },
            AdminPayload::CycleUiBackgroundInterval,
            AdminPayload::UserProfile { id: 9 },
            AdminPayload::CycleUserRole { id: 9 },
            AdminPayload::CycleUserLanguage { id: 9 },
            AdminPayload::ResetUserAccess { id: 9 },
            AdminPayload::UserRooms { id: 9 },
            AdminPayload::ToggleUserRoomAccess { id: 9, room: 1 },
            AdminPayload::UserRoomDevices { id: 9, room: 1 },
            AdminPayload::ToggleUserDeviceAccess {
                id: 9,
                room: 1,
                device: 2,
            },
            AdminPayload::ToggleUserDeviceNotify {
                id: 9,
                room: 1,
                device: 2,
            },
            AdminPayload::ConfirmDeleteUser { id: 9 },
            AdminPayload::DeleteUser { id: 9 },
            AdminPayload::RecordingRuleDetail { room: 1, rule: 2 },
            AdminPayload::ToggleRecordingRuleDetail { room: 1, rule: 2 },
            AdminPayload::PromptEditRecordingRule { room: 1, rule: 2 },
            AdminPayload::ToggleRecordingRuleNotify { room: 1, rule: 2 },
            AdminPayload::DuplicateRecordingRule { room: 1, rule: 2 },
            AdminPayload::TestRecordingRule { room: 1, rule: 2 },
            AdminPayload::ToggleRecordingRuleNoise { room: 1, rule: 2 },
            AdminPayload::CameraHealth { room: 1, camera: 2 },
            AdminPayload::CheckCameraHealth { room: 1, camera: 2 },
            AdminPayload::RecordingRuleGroups,
            AdminPayload::RecordingRuleGroupsForRoom { room: 1 },
            AdminPayload::RecordingRuleGroupDetail { group: 3 },
            AdminPayload::RecordingRuleGroupDetailForRoom { room: 1, group: 3 },
            AdminPayload::RecordingRuleGroupRules {
                group: 3,
                page: 1,
                filter: RecordingRuleGroupRulesFilter::All,
            },
            AdminPayload::RecordingRuleGroupRulesForRoom {
                room: 1,
                group: 3,
                page: 1,
                filter: RecordingRuleGroupRulesFilter::Selected,
            },
            AdminPayload::PromptCreateRecordingRuleGroup,
            AdminPayload::PromptCreateRecordingRuleGroupForRoom { room: 1 },
            AdminPayload::PromptCreateRecordingRuleGroupForWizard,
            AdminPayload::PromptCreateRecordingRuleGroupForEdit { room: 1, rule: 2 },
            AdminPayload::PromptRenameRecordingRuleGroup { group: 3 },
            AdminPayload::PromptRenameRecordingRuleGroupForRoom { room: 1, group: 3 },
            AdminPayload::ToggleRecordingRuleGroup { group: 3 },
            AdminPayload::ToggleRecordingRuleGroupForRoom { room: 1, group: 3 },
            AdminPayload::ToggleRecordingRuleGroupDetail { group: 3 },
            AdminPayload::ToggleRecordingRuleGroupDetailForRoom { room: 1, group: 3 },
            AdminPayload::CycleRecordingRuleGroupAccess { group: 3 },
            AdminPayload::CycleRecordingRuleGroupAccessForRoom { room: 1, group: 3 },
            AdminPayload::ConfirmDeleteRecordingRuleGroup { group: 3 },
            AdminPayload::ConfirmDeleteRecordingRuleGroupForRoom { room: 1, group: 3 },
            AdminPayload::DeleteRecordingRuleGroup { group: 3 },
            AdminPayload::DeleteRecordingRuleGroupForRoom { room: 1, group: 3 },
            AdminPayload::ToggleRecordingRuleGroupRule {
                group: 3,
                rule: 2,
                page: 1,
                filter: RecordingRuleGroupRulesFilter::All,
            },
            AdminPayload::ToggleRecordingRuleGroupRuleForRoom {
                room: 1,
                group: 3,
                rule: 2,
                page: 1,
                filter: RecordingRuleGroupRulesFilter::Unselected,
            },
            AdminPayload::ToggleRuleGroupItem {
                room: 1,
                rule: 2,
                group: 3,
            },
            AdminPayload::EnsureDefaultRuleGroups,
            AdminPayload::EnsureDefaultRuleGroupsForRoom { room: 1 },
            AdminPayload::EnsureDefaultRuleGroupsForWizard,
            AdminPayload::EnsureDefaultRuleGroupsForEdit { room: 1, rule: 2 },
            AdminPayload::ToggleUserVoice { id: 9 },
            AdminPayload::CycleUserVoiceEngine { id: 9 },
            AdminPayload::StartRecordingRuleWizard { room: 1 },
            AdminPayload::WizardPickCamera { room: 1, camera: 2 },
            AdminPayload::WizardSourcePage { page: 1 },
            AdminPayload::WizardPickEntity {
                room: 1,
                camera: 2,
                device: 3,
            },
            AdminPayload::WizardPickTail {
                room: 1,
                camera: 2,
                device: 3,
                mode: WizardTriggerMode::OpenAndClose,
                tail: 30,
            },
            AdminPayload::WizardPickRetention {
                room: 1,
                camera: 2,
                device: 3,
                mode: WizardTriggerMode::OpenAndClose,
                tail: 30,
                retention: 14,
            },
            AdminPayload::WizardPickGroup {
                room: 1,
                camera: 2,
                device: 3,
                mode: WizardTriggerMode::OpenAndClose,
                tail: 30,
                retention: 14,
                group: None,
            },
            AdminPayload::WizardPickGroup {
                room: 1,
                camera: 2,
                device: 3,
                mode: WizardTriggerMode::OpenAndClose,
                tail: 30,
                retention: 14,
                group: Some(4),
            },
            AdminPayload::WizardCreateRule {
                room: 1,
                camera: 2,
                device: 3,
                mode: WizardTriggerMode::OpenAndClose,
                tail: 30,
                retention: 14,
                group: Some(4),
            },
            AdminPayload::WizardAdvancedText {
                room: 1,
                camera: 2,
                device: 3,
                mode: WizardTriggerMode::OpenAndClose,
                tail: 30,
                retention: 14,
                group: Some(4),
            },
            AdminPayload::WizardConditions,
            AdminPayload::WizardAddCondition,
            AdminPayload::WizardConditionEntityPage { page: 1 },
            AdminPayload::WizardPickConditionEntity { device: 3 },
            AdminPayload::WizardRemoveCondition { index: 1 },
            AdminPayload::WizardToggleLogic,
            AdminPayload::WizardNextTail,
            AdminPayload::WizardPickWizardTail { tail: 30 },
            AdminPayload::WizardPickWizardRetention { retention: 14 },
            AdminPayload::WizardActiveTime,
            AdminPayload::WizardPromptActiveTimeCustom,
            AdminPayload::WizardToggleGroup { group: 4 },
            AdminPayload::WizardConfirmGroups,
            AdminPayload::WizardCreateCurrentRule,
            AdminPayload::WizardAdvancedCurrentText,
            AdminPayload::RecordingRuleEditMenu { room: 1, rule: 2 },
            AdminPayload::RecordingRuleEditActiveTime { room: 1, rule: 2 },
            AdminPayload::PromptRecordingRuleActiveTimeCustom { room: 1, rule: 2 },
            AdminPayload::RecordingRuleActiveDays { room: 1, rule: 2 },
            AdminPayload::ToggleRecordingRuleActiveDay {
                room: 1,
                rule: 2,
                day: 1,
            },
            AdminPayload::RecordingRuleEditSensors { room: 1, rule: 2 },
            AdminPayload::PromptEditRecordingRuleNumber {
                room: 1,
                rule: 2,
                field: RecordingRuleEditField::TailSeconds,
            },
            AdminPayload::CycleRecordingRuleLogic { room: 1, rule: 2 },
            AdminPayload::RecordingRuleEditSensorPage {
                room: 1,
                rule: 2,
                page: 1,
            },
            AdminPayload::RecordingRuleEditPickSensor {
                room: 1,
                rule: 2,
                device: 3,
            },
            AdminPayload::DeleteRecordingRuleCondition {
                room: 1,
                rule: 2,
                condition: 4,
            },
            AdminPayload::WizardGroups,
            AdminPayload::WizardCancel,
            AdminPayload::RecordingRuleEditGroups { room: 1, rule: 2 },
            AdminPayload::ToggleRecordingRuleEditGroupItem {
                room: 1,
                rule: 2,
                group: 3,
            },
            AdminPayload::ActionGroups,
            AdminPayload::ActionGroupDetail { group: 3 },
            AdminPayload::PromptCreateActionGroup,
            AdminPayload::PromptRenameActionGroup { group: 3 },
            AdminPayload::ConfirmDeleteActionGroup { group: 3 },
            AdminPayload::DeleteActionGroup { group: 3 },
            AdminPayload::ActionGroupItems {
                group: 3,
                page: 1,
                filter: db::action_groups::ActionGroupItemsFilter::All,
            },
            AdminPayload::ToggleActionGroupItem {
                group: 3,
                device: 4,
                page: 1,
                filter: db::action_groups::ActionGroupItemsFilter::Selected,
            },
            AdminPayload::ToggleActionGroupAccess { group: 3 },
            AdminPayload::ToggleActionGroupEnabled { group: 3 },
            AdminPayload::ExecuteActionGroup {
                group: 3,
                command: db::action_groups::ActionGroupCommand::TurnOff,
            },
            AdminPayload::HaNativeActionDetail { action: 4 },
            AdminPayload::ToggleHaNativeActionAccess { action: 4 },
            AdminPayload::ToggleHaNativeActionEnabled { action: 4 },
            AdminPayload::PromptHaNativeActionAlias { action: 4 },
            AdminPayload::ResetHaNativeActionAlias { action: 4 },
            AdminPayload::ExecuteHaNativeAction { action: 4 },
            AdminPayload::ActionSchedules {
                target: db::action_groups::ActionTargetRef::BotGroup(3),
            },
            AdminPayload::ActionScheduleDetail {
                target: db::action_groups::ActionTargetRef::BotGroup(3),
                schedule: 5,
            },
            AdminPayload::PromptCreateActionScheduleTime {
                target: db::action_groups::ActionTargetRef::BotGroup(3),
                command: db::action_groups::ActionScheduleCommand::TurnOn,
            },
            AdminPayload::PromptEditActionScheduleTime {
                target: db::action_groups::ActionTargetRef::BotGroup(3),
                schedule: 5,
            },
            AdminPayload::ToggleActionScheduleDay {
                target: db::action_groups::ActionTargetRef::HaNative(4),
                schedule: 5,
                day: 6,
            },
            AdminPayload::CycleActionScheduleCommand {
                target: db::action_groups::ActionTargetRef::BotGroup(3),
                schedule: 5,
            },
            AdminPayload::ToggleActionScheduleEnabled {
                target: db::action_groups::ActionTargetRef::BotGroup(3),
                schedule: 5,
            },
            AdminPayload::ConfirmDeleteActionSchedule {
                target: db::action_groups::ActionTargetRef::HaNative(4),
                schedule: 5,
            },
            AdminPayload::DeleteActionSchedule {
                target: db::action_groups::ActionTargetRef::HaNative(4),
                schedule: 5,
            },
        ];

        payloads.extend(
            activity_log_filter_samples()
                .into_iter()
                .map(|filter| AdminPayload::ActivityLog { filter }),
        );
        payloads.extend(wizard_mode_samples().into_iter().map(|mode| {
            AdminPayload::WizardPickMode {
                room: 1,
                camera: 2,
                device: 3,
                mode,
            }
        }));
        payloads.extend(
            recording_rule_edit_field_samples()
                .into_iter()
                .map(|field| AdminPayload::PromptEditRecordingRuleNumber {
                    room: 1,
                    rule: 2,
                    field,
                }),
        );
        payloads.extend(active_time_preset_samples().into_iter().flat_map(|preset| {
            [
                AdminPayload::SetRecordingRuleActiveTimePreset {
                    room: 1,
                    rule: 2,
                    preset,
                },
                AdminPayload::WizardPickActiveTimePreset { preset },
            ]
        }));
        payloads.extend(
            condition_operator_samples()
                .into_iter()
                .map(|operator| AdminPayload::WizardPickConditionOperator { operator }),
        );
        payloads.extend(condition_operator_samples().into_iter().map(|operator| {
            AdminPayload::RecordingRuleEditPickSensorOperator {
                room: 1,
                rule: 2,
                device: 3,
                operator,
            }
        }));

        payloads
    }

    fn wizard_mode_samples() -> Vec<WizardTriggerMode> {
        vec![
            WizardTriggerMode::OpenAndClose,
            WizardTriggerMode::OpenOnly,
            WizardTriggerMode::CloseOnly,
            WizardTriggerMode::Detected,
            WizardTriggerMode::Cleared,
            WizardTriggerMode::DetectedAndCleared,
            WizardTriggerMode::TurnedOn,
            WizardTriggerMode::TurnedOff,
            WizardTriggerMode::TurnedOnAndOff,
            WizardTriggerMode::AnyChange,
            WizardTriggerMode::NumericAbove,
            WizardTriggerMode::NumericBelow,
        ]
    }

    fn condition_operator_samples() -> Vec<db::camera_recording_rules::ConditionOperator> {
        vec![
            db::camera_recording_rules::ConditionOperator::ChangedTo,
            db::camera_recording_rules::ConditionOperator::ChangedFromTo,
            db::camera_recording_rules::ConditionOperator::Is,
            db::camera_recording_rules::ConditionOperator::IsNot,
            db::camera_recording_rules::ConditionOperator::Contains,
            db::camera_recording_rules::ConditionOperator::Above,
            db::camera_recording_rules::ConditionOperator::Below,
        ]
    }

    fn recording_rule_edit_field_samples() -> Vec<RecordingRuleEditField> {
        vec![
            RecordingRuleEditField::TailSeconds,
            RecordingRuleEditField::MaxSegmentSeconds,
            RecordingRuleEditField::CooldownSeconds,
            RecordingRuleEditField::RetentionDays,
        ]
    }

    fn active_time_preset_samples() -> Vec<RecordingRuleActiveTimePreset> {
        vec![
            RecordingRuleActiveTimePreset::Always,
            RecordingRuleActiveTimePreset::Night,
            RecordingRuleActiveTimePreset::Day,
        ]
    }

    fn activity_log_filter_samples() -> Vec<ActivityLogFilter> {
        vec![
            ActivityLogFilter::All,
            ActivityLogFilter::Errors,
            ActivityLogFilter::Cameras,
            ActivityLogFilter::Devices,
            ActivityLogFilter::Recording,
        ]
    }

    fn payload_route_name(payload: &Payload) -> &'static str {
        match payload {
            Payload::Home => "Payload::Home",
            Payload::Control(payload) => control_route_name(payload),
            Payload::Settings(payload) => settings_route_name(payload),
            Payload::Camera(payload) => camera_route_name(payload),
            Payload::Admin(payload) => admin_route_name(payload),
            Payload::InDev => "Payload::InDev",
            Payload::Command(payload) => command_route_name(payload),
            Payload::AdminSettings(payload) => settings_route_name(payload),
        }
    }

    fn command_route_name(payload: &CommandPayload) -> &'static str {
        match payload {
            CommandPayload::Confirm { .. } => "CommandPayload::Confirm",
            CommandPayload::Cancel { .. } => "CommandPayload::Cancel",
        }
    }

    fn control_route_name(payload: &ControlPayload) -> &'static str {
        match payload {
            ControlPayload::ListRooms => "ControlPayload::ListRooms",
            ControlPayload::RoomDetail { .. } => "ControlPayload::RoomDetail",
            ControlPayload::DeviceControl { .. } => "ControlPayload::DeviceControl",
            ControlPayload::QuickAction { .. } => "ControlPayload::QuickAction",
            ControlPayload::ActionGroups => "ControlPayload::ActionGroups",
            ControlPayload::ActionGroupDetail { .. } => "ControlPayload::ActionGroupDetail",
            ControlPayload::ExecuteActionGroup { .. } => "ControlPayload::ExecuteActionGroup",
            ControlPayload::HaNativeActionDetail { .. } => "ControlPayload::HaNativeActionDetail",
            ControlPayload::ExecuteHaNativeAction { .. } => "ControlPayload::ExecuteHaNativeAction",
        }
    }

    fn settings_route_name(payload: &SettingsPayload) -> &'static str {
        match payload {
            SettingsPayload::ListRooms => "SettingsPayload::ListRooms",
            SettingsPayload::RoomDetail { .. } => "SettingsPayload::RoomDetail",
            SettingsPayload::DeviceDetail { .. } => "SettingsPayload::DeviceDetail",
            SettingsPayload::ToggleNotify { .. } => "SettingsPayload::ToggleNotify",
            SettingsPayload::ToggleHide { .. } => "SettingsPayload::ToggleHide",
            SettingsPayload::EditName { .. } => "SettingsPayload::EditName",
            SettingsPayload::StateAliases { .. } => "SettingsPayload::StateAliases",
            SettingsPayload::EditStateAlias { .. } => "SettingsPayload::EditStateAlias",
            SettingsPayload::ResetStateAlias { .. } => "SettingsPayload::ResetStateAlias",
            SettingsPayload::ToggleStateInversion { .. } => "SettingsPayload::ToggleStateInversion",
            SettingsPayload::ToggleCritical { .. } => "SettingsPayload::ToggleCritical",
        }
    }

    fn camera_route_name(payload: &CameraPayload) -> &'static str {
        match payload {
            CameraPayload::ListCameras => "CameraPayload::ListCameras",
            CameraPayload::CameraDetail { .. } => "CameraPayload::CameraDetail",
            CameraPayload::Snapshot { .. } => "CameraPayload::Snapshot",
            CameraPayload::Clip { .. } => "CameraPayload::Clip",
            CameraPayload::RecordingArchive { .. } => "CameraPayload::RecordingArchive",
            CameraPayload::RecordingSession { .. } => "CameraPayload::RecordingSession",
            CameraPayload::SendRecordingSegment { .. } => "CameraPayload::SendRecordingSegment",
            CameraPayload::SendRecordingAll { .. } => "CameraPayload::SendRecordingAll",
            CameraPayload::ConfirmDeleteRecording { .. } => "CameraPayload::ConfirmDeleteRecording",
            CameraPayload::DeleteRecording { .. } => "CameraPayload::DeleteRecording",
            CameraPayload::StopRecording { .. } => "CameraPayload::StopRecording",
            CameraPayload::RecordingGroupModes => "CameraPayload::RecordingGroupModes",
            CameraPayload::RecordingGroupModeDetail { .. } => {
                "CameraPayload::RecordingGroupModeDetail"
            }
            CameraPayload::ToggleRecordingGroupMode { .. } => {
                "CameraPayload::ToggleRecordingGroupMode"
            }
        }
    }

    fn admin_route_name(payload: &AdminPayload) -> &'static str {
        match payload {
            AdminPayload::ListActions => "AdminPayload::ListActions",
            AdminPayload::ListUsers => "AdminPayload::ListUsers",
            AdminPayload::Status => "AdminPayload::Status",
            AdminPayload::ConfirmBackup => "AdminPayload::ConfirmBackup",
            AdminPayload::CreateBackup => "AdminPayload::CreateBackup",
            AdminPayload::PromptAddUser => "AdminPayload::PromptAddUser",
            AdminPayload::PromptDeleteUser => "AdminPayload::PromptDeleteUser",
            AdminPayload::AddUser { .. } => "AdminPayload::AddUser",
            AdminPayload::CameraRooms => "AdminPayload::CameraRooms",
            AdminPayload::RoomCameras { .. } => "AdminPayload::RoomCameras",
            AdminPayload::RoomCameraDetail { .. } => "AdminPayload::RoomCameraDetail",
            AdminPayload::RoomCameraSnapshot { .. } => "AdminPayload::RoomCameraSnapshot",
            AdminPayload::RoomCameraClip { .. } => "AdminPayload::RoomCameraClip",
            AdminPayload::PromptAddCamera { .. } => "AdminPayload::PromptAddCamera",
            AdminPayload::ConfirmDeleteCamera { .. } => "AdminPayload::ConfirmDeleteCamera",
            AdminPayload::DeleteCamera { .. } => "AdminPayload::DeleteCamera",
            AdminPayload::RecordingRules { .. } => "AdminPayload::RecordingRules",
            AdminPayload::PromptAddRecordingRule { .. } => "AdminPayload::PromptAddRecordingRule",
            AdminPayload::ToggleRecordingRule { .. } => "AdminPayload::ToggleRecordingRule",
            AdminPayload::ConfirmDeleteRecordingRule { .. } => {
                "AdminPayload::ConfirmDeleteRecordingRule"
            }
            AdminPayload::DeleteRecordingRule { .. } => "AdminPayload::DeleteRecordingRule",
            AdminPayload::CycleRecordingDefaultRetention { .. } => {
                "AdminPayload::CycleRecordingDefaultRetention"
            }
            AdminPayload::CycleRecordingStorageQuota { .. } => {
                "AdminPayload::CycleRecordingStorageQuota"
            }
            AdminPayload::UiBackground => "AdminPayload::UiBackground",
            AdminPayload::SetUiBackgroundCamera { .. } => "AdminPayload::SetUiBackgroundCamera",
            AdminPayload::CycleUiBackgroundInterval => "AdminPayload::CycleUiBackgroundInterval",
            AdminPayload::UserProfile { .. } => "AdminPayload::UserProfile",
            AdminPayload::CycleUserRole { .. } => "AdminPayload::CycleUserRole",
            AdminPayload::CycleUserLanguage { .. } => "AdminPayload::CycleUserLanguage",
            AdminPayload::ResetUserAccess { .. } => "AdminPayload::ResetUserAccess",
            AdminPayload::UserRooms { .. } => "AdminPayload::UserRooms",
            AdminPayload::ToggleUserRoomAccess { .. } => "AdminPayload::ToggleUserRoomAccess",
            AdminPayload::UserRoomDevices { .. } => "AdminPayload::UserRoomDevices",
            AdminPayload::ToggleUserDeviceAccess { .. } => "AdminPayload::ToggleUserDeviceAccess",
            AdminPayload::ToggleUserDeviceNotify { .. } => "AdminPayload::ToggleUserDeviceNotify",
            AdminPayload::ConfirmDeleteUser { .. } => "AdminPayload::ConfirmDeleteUser",
            AdminPayload::DeleteUser { .. } => "AdminPayload::DeleteUser",
            AdminPayload::RecordingRuleDetail { .. } => "AdminPayload::RecordingRuleDetail",
            AdminPayload::ToggleRecordingRuleDetail { .. } => {
                "AdminPayload::ToggleRecordingRuleDetail"
            }
            AdminPayload::PromptEditRecordingRule { .. } => "AdminPayload::PromptEditRecordingRule",
            AdminPayload::ToggleRecordingRuleNotify { .. } => {
                "AdminPayload::ToggleRecordingRuleNotify"
            }
            AdminPayload::DuplicateRecordingRule { .. } => "AdminPayload::DuplicateRecordingRule",
            AdminPayload::TestRecordingRule { .. } => "AdminPayload::TestRecordingRule",
            AdminPayload::ToggleRecordingRuleNoise { .. } => {
                "AdminPayload::ToggleRecordingRuleNoise"
            }
            AdminPayload::CameraHealth { .. } => "AdminPayload::CameraHealth",
            AdminPayload::CheckCameraHealth { .. } => "AdminPayload::CheckCameraHealth",
            AdminPayload::ActivityLog { .. } => "AdminPayload::ActivityLog",
            AdminPayload::RecordingRuleGroups => "AdminPayload::RecordingRuleGroups",
            AdminPayload::RecordingRuleGroupsForRoom { .. } => {
                "AdminPayload::RecordingRuleGroupsForRoom"
            }
            AdminPayload::RecordingRuleGroupDetail { .. } => {
                "AdminPayload::RecordingRuleGroupDetail"
            }
            AdminPayload::RecordingRuleGroupDetailForRoom { .. } => {
                "AdminPayload::RecordingRuleGroupDetailForRoom"
            }
            AdminPayload::RecordingRuleGroupRules { .. } => "AdminPayload::RecordingRuleGroupRules",
            AdminPayload::RecordingRuleGroupRulesForRoom { .. } => {
                "AdminPayload::RecordingRuleGroupRulesForRoom"
            }
            AdminPayload::PromptCreateRecordingRuleGroup => {
                "AdminPayload::PromptCreateRecordingRuleGroup"
            }
            AdminPayload::PromptCreateRecordingRuleGroupForRoom { .. } => {
                "AdminPayload::PromptCreateRecordingRuleGroupForRoom"
            }
            AdminPayload::PromptCreateRecordingRuleGroupForWizard => {
                "AdminPayload::PromptCreateRecordingRuleGroupForWizard"
            }
            AdminPayload::PromptCreateRecordingRuleGroupForEdit { .. } => {
                "AdminPayload::PromptCreateRecordingRuleGroupForEdit"
            }
            AdminPayload::PromptRenameRecordingRuleGroup { .. } => {
                "AdminPayload::PromptRenameRecordingRuleGroup"
            }
            AdminPayload::PromptRenameRecordingRuleGroupForRoom { .. } => {
                "AdminPayload::PromptRenameRecordingRuleGroupForRoom"
            }
            AdminPayload::ToggleRecordingRuleGroup { .. } => {
                "AdminPayload::ToggleRecordingRuleGroup"
            }
            AdminPayload::ToggleRecordingRuleGroupForRoom { .. } => {
                "AdminPayload::ToggleRecordingRuleGroupForRoom"
            }
            AdminPayload::ToggleRecordingRuleGroupDetail { .. } => {
                "AdminPayload::ToggleRecordingRuleGroupDetail"
            }
            AdminPayload::ToggleRecordingRuleGroupDetailForRoom { .. } => {
                "AdminPayload::ToggleRecordingRuleGroupDetailForRoom"
            }
            AdminPayload::CycleRecordingRuleGroupAccess { .. } => {
                "AdminPayload::CycleRecordingRuleGroupAccess"
            }
            AdminPayload::CycleRecordingRuleGroupAccessForRoom { .. } => {
                "AdminPayload::CycleRecordingRuleGroupAccessForRoom"
            }
            AdminPayload::ConfirmDeleteRecordingRuleGroup { .. } => {
                "AdminPayload::ConfirmDeleteRecordingRuleGroup"
            }
            AdminPayload::ConfirmDeleteRecordingRuleGroupForRoom { .. } => {
                "AdminPayload::ConfirmDeleteRecordingRuleGroupForRoom"
            }
            AdminPayload::DeleteRecordingRuleGroup { .. } => {
                "AdminPayload::DeleteRecordingRuleGroup"
            }
            AdminPayload::DeleteRecordingRuleGroupForRoom { .. } => {
                "AdminPayload::DeleteRecordingRuleGroupForRoom"
            }
            AdminPayload::ToggleRecordingRuleGroupRule { .. } => {
                "AdminPayload::ToggleRecordingRuleGroupRule"
            }
            AdminPayload::ToggleRecordingRuleGroupRuleForRoom { .. } => {
                "AdminPayload::ToggleRecordingRuleGroupRuleForRoom"
            }
            AdminPayload::ToggleRuleGroupItem { .. } => "AdminPayload::ToggleRuleGroupItem",
            AdminPayload::EnsureDefaultRuleGroups => "AdminPayload::EnsureDefaultRuleGroups",
            AdminPayload::EnsureDefaultRuleGroupsForRoom { .. } => {
                "AdminPayload::EnsureDefaultRuleGroupsForRoom"
            }
            AdminPayload::EnsureDefaultRuleGroupsForWizard => {
                "AdminPayload::EnsureDefaultRuleGroupsForWizard"
            }
            AdminPayload::EnsureDefaultRuleGroupsForEdit { .. } => {
                "AdminPayload::EnsureDefaultRuleGroupsForEdit"
            }
            AdminPayload::ToggleUserVoice { .. } => "AdminPayload::ToggleUserVoice",
            AdminPayload::CycleUserVoiceEngine { .. } => "AdminPayload::CycleUserVoiceEngine",
            AdminPayload::StartRecordingRuleWizard { .. } => {
                "AdminPayload::StartRecordingRuleWizard"
            }
            AdminPayload::WizardPickCamera { .. } => "AdminPayload::WizardPickCamera",
            AdminPayload::WizardSourcePage { .. } => "AdminPayload::WizardSourcePage",
            AdminPayload::WizardPickEntity { .. } => "AdminPayload::WizardPickEntity",
            AdminPayload::WizardPickMode { .. } => "AdminPayload::WizardPickMode",
            AdminPayload::WizardPickTail { .. } => "AdminPayload::WizardPickTail",
            AdminPayload::WizardPickRetention { .. } => "AdminPayload::WizardPickRetention",
            AdminPayload::WizardPickGroup { .. } => "AdminPayload::WizardPickGroup",
            AdminPayload::WizardCreateRule { .. } => "AdminPayload::WizardCreateRule",
            AdminPayload::WizardAdvancedText { .. } => "AdminPayload::WizardAdvancedText",
            AdminPayload::WizardConditions => "AdminPayload::WizardConditions",
            AdminPayload::WizardAddCondition => "AdminPayload::WizardAddCondition",
            AdminPayload::WizardConditionEntityPage { .. } => {
                "AdminPayload::WizardConditionEntityPage"
            }
            AdminPayload::WizardPickConditionEntity { .. } => {
                "AdminPayload::WizardPickConditionEntity"
            }
            AdminPayload::WizardPickConditionOperator { .. } => {
                "AdminPayload::WizardPickConditionOperator"
            }
            AdminPayload::WizardRemoveCondition { .. } => "AdminPayload::WizardRemoveCondition",
            AdminPayload::WizardToggleLogic => "AdminPayload::WizardToggleLogic",
            AdminPayload::WizardNextTail => "AdminPayload::WizardNextTail",
            AdminPayload::WizardPickWizardTail { .. } => "AdminPayload::WizardPickWizardTail",
            AdminPayload::WizardPickWizardRetention { .. } => {
                "AdminPayload::WizardPickWizardRetention"
            }
            AdminPayload::WizardActiveTime => "AdminPayload::WizardActiveTime",
            AdminPayload::WizardPickActiveTimePreset { .. } => {
                "AdminPayload::WizardPickActiveTimePreset"
            }
            AdminPayload::WizardPromptActiveTimeCustom => {
                "AdminPayload::WizardPromptActiveTimeCustom"
            }
            AdminPayload::WizardToggleGroup { .. } => "AdminPayload::WizardToggleGroup",
            AdminPayload::WizardConfirmGroups => "AdminPayload::WizardConfirmGroups",
            AdminPayload::WizardCreateCurrentRule => "AdminPayload::WizardCreateCurrentRule",
            AdminPayload::WizardAdvancedCurrentText => "AdminPayload::WizardAdvancedCurrentText",
            AdminPayload::RecordingRuleEditMenu { .. } => "AdminPayload::RecordingRuleEditMenu",
            AdminPayload::RecordingRuleEditActiveTime { .. } => {
                "AdminPayload::RecordingRuleEditActiveTime"
            }
            AdminPayload::SetRecordingRuleActiveTimePreset { .. } => {
                "AdminPayload::SetRecordingRuleActiveTimePreset"
            }
            AdminPayload::PromptRecordingRuleActiveTimeCustom { .. } => {
                "AdminPayload::PromptRecordingRuleActiveTimeCustom"
            }
            AdminPayload::RecordingRuleActiveDays { .. } => "AdminPayload::RecordingRuleActiveDays",
            AdminPayload::ToggleRecordingRuleActiveDay { .. } => {
                "AdminPayload::ToggleRecordingRuleActiveDay"
            }
            AdminPayload::RecordingRuleEditSensors { .. } => {
                "AdminPayload::RecordingRuleEditSensors"
            }
            AdminPayload::PromptEditRecordingRuleNumber { .. } => {
                "AdminPayload::PromptEditRecordingRuleNumber"
            }
            AdminPayload::CycleRecordingRuleLogic { .. } => "AdminPayload::CycleRecordingRuleLogic",
            AdminPayload::RecordingRuleEditSensorPage { .. } => {
                "AdminPayload::RecordingRuleEditSensorPage"
            }
            AdminPayload::RecordingRuleEditPickSensor { .. } => {
                "AdminPayload::RecordingRuleEditPickSensor"
            }
            AdminPayload::RecordingRuleEditPickSensorOperator { .. } => {
                "AdminPayload::RecordingRuleEditPickSensorOperator"
            }
            AdminPayload::DeleteRecordingRuleCondition { .. } => {
                "AdminPayload::DeleteRecordingRuleCondition"
            }
            AdminPayload::WizardGroups => "AdminPayload::WizardGroups",
            AdminPayload::WizardCancel => "AdminPayload::WizardCancel",
            AdminPayload::RecordingRuleEditGroups { .. } => "AdminPayload::RecordingRuleEditGroups",
            AdminPayload::ToggleRecordingRuleEditGroupItem { .. } => {
                "AdminPayload::ToggleRecordingRuleEditGroupItem"
            }
            AdminPayload::ActionGroups => "AdminPayload::ActionGroups",
            AdminPayload::ActionGroupDetail { .. } => "AdminPayload::ActionGroupDetail",
            AdminPayload::PromptCreateActionGroup => "AdminPayload::PromptCreateActionGroup",
            AdminPayload::PromptRenameActionGroup { .. } => "AdminPayload::PromptRenameActionGroup",
            AdminPayload::ConfirmDeleteActionGroup { .. } => {
                "AdminPayload::ConfirmDeleteActionGroup"
            }
            AdminPayload::DeleteActionGroup { .. } => "AdminPayload::DeleteActionGroup",
            AdminPayload::ActionGroupItems { .. } => "AdminPayload::ActionGroupItems",
            AdminPayload::ToggleActionGroupItem { .. } => "AdminPayload::ToggleActionGroupItem",
            AdminPayload::ToggleActionGroupAccess { .. } => "AdminPayload::ToggleActionGroupAccess",
            AdminPayload::ToggleActionGroupEnabled { .. } => {
                "AdminPayload::ToggleActionGroupEnabled"
            }
            AdminPayload::ExecuteActionGroup { .. } => "AdminPayload::ExecuteActionGroup",
            AdminPayload::HaNativeActionDetail { .. } => "AdminPayload::HaNativeActionDetail",
            AdminPayload::ToggleHaNativeActionAccess { .. } => {
                "AdminPayload::ToggleHaNativeActionAccess"
            }
            AdminPayload::ToggleHaNativeActionEnabled { .. } => {
                "AdminPayload::ToggleHaNativeActionEnabled"
            }
            AdminPayload::PromptHaNativeActionAlias { .. } => {
                "AdminPayload::PromptHaNativeActionAlias"
            }
            AdminPayload::ResetHaNativeActionAlias { .. } => {
                "AdminPayload::ResetHaNativeActionAlias"
            }
            AdminPayload::ExecuteHaNativeAction { .. } => "AdminPayload::ExecuteHaNativeAction",
            AdminPayload::ActionSchedules { .. } => "AdminPayload::ActionSchedules",
            AdminPayload::ActionScheduleDetail { .. } => "AdminPayload::ActionScheduleDetail",
            AdminPayload::PromptCreateActionScheduleTime { .. } => {
                "AdminPayload::PromptCreateActionScheduleTime"
            }
            AdminPayload::PromptEditActionScheduleTime { .. } => {
                "AdminPayload::PromptEditActionScheduleTime"
            }
            AdminPayload::ToggleActionScheduleDay { .. } => "AdminPayload::ToggleActionScheduleDay",
            AdminPayload::CycleActionScheduleCommand { .. } => {
                "AdminPayload::CycleActionScheduleCommand"
            }
            AdminPayload::ToggleActionScheduleEnabled { .. } => {
                "AdminPayload::ToggleActionScheduleEnabled"
            }
            AdminPayload::ConfirmDeleteActionSchedule { .. } => {
                "AdminPayload::ConfirmDeleteActionSchedule"
            }
            AdminPayload::DeleteActionSchedule { .. } => "AdminPayload::DeleteActionSchedule",
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
            ha_url: paths.ha_url.clone(),
            ha_token: paths.ha_token.clone(),
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
            voice_enabled: false,
            voice_stt_provider: crate::options::VoiceSttProvider::HaPipeline,
            voice_command_engine: crate::options::VoiceCommandEngine::LocalParser,
            voice_ha_pipeline_id: None,
            voice_stt_sample_rate: 16_000,
            voice_confirm_dangerous: true,
            voice_pending_ttl_s: 60,
            voice_max_audio_size_mb: 10,
            voice_max_audio_duration_s: 30,
            voice_stt_timeout_s: 45,
            voice_show_recognized_text: true,
            voice_response_format: crate::options::VoiceResponseFormat::Text,

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
