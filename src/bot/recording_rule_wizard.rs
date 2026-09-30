use crate::db::camera_recording_rules::{
    ConditionLogic, ConditionOperator, RecordingRuleActiveTime,
};
use crate::i18n::Language;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum WizardTriggerMode {
    OpenAndClose,
    OpenOnly,
    CloseOnly,
    Detected,
    Cleared,
    DetectedAndCleared,
    TurnedOn,
    TurnedOff,
    TurnedOnAndOff,
    AnyChange,
    NumericAbove,
    NumericBelow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingRuleWizard {
    pub draft_id: Option<i64>,
    pub revision: i64,
    pub committed_rule_id: Option<i64>,
    pub name: Option<String>,
    pub name_touched: bool,
    pub cooldown_s: Option<u32>,
    pub cooldown_touched: bool,
    pub max_segment_seconds: Option<u32>,
    pub pre_roll_enabled: Option<bool>,
    pub pre_roll_seconds: i64,
    pub custom_conditions: Option<Vec<WizardCondition>>,
    pub raw_text: Option<String>,
    pub pending_input: Option<String>,
    pub editing_text: bool,
    pub step: String,
    pub return_to_summary: bool,
    pub room_id: i64,
    pub camera_id: Option<i64>,
    pub source_device_id: Option<i64>,
    pub source_mode: Option<WizardTriggerMode>,
    pub source_value: Option<String>,
    pub extra_conditions: Vec<WizardCondition>,
    pub condition_logic: ConditionLogic,
    pub logic_touched: bool,
    pub tail_seconds: Option<u32>,
    pub retention_days: Option<u32>,
    pub active_time: RecordingRuleActiveTime,
    pub group_ids: Vec<i64>,
    pub pending_condition: Option<WizardPendingCondition>,
}

impl RecordingRuleWizard {
    pub fn new(room_id: i64) -> Self {
        Self {
            draft_id: None,
            revision: 0,
            committed_rule_id: None,
            name: None,
            name_touched: false,
            cooldown_s: None,
            cooldown_touched: false,
            max_segment_seconds: None,
            pre_roll_enabled: None,
            pre_roll_seconds: crate::db::camera_recording_rules::DEFAULT_PRE_ROLL_SECONDS,
            custom_conditions: None,
            raw_text: None,
            pending_input: None,
            editing_text: false,
            step: String::new(),
            return_to_summary: false,
            room_id,
            camera_id: None,
            source_device_id: None,
            source_mode: None,
            source_value: None,
            extra_conditions: Vec::new(),
            condition_logic: ConditionLogic::Any,
            logic_touched: false,
            tail_seconds: None,
            retention_days: None,
            active_time: RecordingRuleActiveTime::always(),
            group_ids: Vec::new(),
            pending_condition: None,
        }
    }

    /// Only an actual source change invalidates source-dependent fields.
    pub fn reset_source(&mut self, device_id: i64) {
        if self.source_device_id == Some(device_id) {
            return;
        }
        self.source_device_id = Some(device_id);
        self.source_mode = None;
        self.source_value = None;
        if let Some(conditions) = self.custom_conditions.take() {
            self.extra_conditions = conditions;
        }
        self.apply_default_logic();
    }

    pub fn set_source_mode(&mut self, mode: WizardTriggerMode, value: Option<String>) {
        let old_mode = self.source_mode;
        self.source_value = if mode.needs_value() {
            value.or_else(|| {
                old_mode
                    .filter(|old| old.needs_value())
                    .and(self.source_value.clone())
            })
        } else {
            None
        };
        self.source_mode = Some(mode);
        if old_mode != Some(mode) {
            if let Some(conditions) = self.custom_conditions.take() {
                self.extra_conditions = conditions;
            }
        }
        if !self.cooldown_touched {
            self.cooldown_s = Some(default_cooldown_s(mode));
        }
        self.apply_default_logic();
    }

    pub fn conditions(
        &self,
        entity_id: &str,
        mode: WizardTriggerMode,
    ) -> Result<Vec<WizardCondition>, String> {
        match &self.custom_conditions {
            Some(conditions) => Ok(conditions.clone()),
            None => build_final_conditions(
                entity_id,
                mode,
                self.source_value.as_deref(),
                &self.extra_conditions,
            ),
        }
    }

    pub fn effective_name(&self, lang: Language, source: &str, mode: WizardTriggerMode) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| build_rule_name(lang, source, mode, self.source_value.as_deref()))
    }

    pub fn add_extra_condition(&mut self, condition: WizardCondition) {
        if let Some(conditions) = self.custom_conditions.as_mut() {
            conditions.push(condition);
        } else {
            self.extra_conditions.push(condition);
        }
        self.apply_default_logic();
    }

    pub fn remove_extra_condition(&mut self, index: usize) -> bool {
        let conditions = self
            .custom_conditions
            .as_mut()
            .unwrap_or(&mut self.extra_conditions);
        if index >= conditions.len() {
            return false;
        }
        conditions.remove(index);
        self.apply_default_logic();
        true
    }

    pub fn toggle_logic(&mut self) {
        self.condition_logic = match self.condition_logic {
            ConditionLogic::All => ConditionLogic::Any,
            ConditionLogic::Any => ConditionLogic::All,
        };
        self.logic_touched = true;
    }

    pub fn toggle_group(&mut self, group_id: i64) {
        if let Some(index) = self.group_ids.iter().position(|id| *id == group_id) {
            self.group_ids.remove(index);
        } else {
            self.group_ids.push(group_id);
            self.group_ids.sort_unstable();
            self.group_ids.dedup();
        }
    }

    fn apply_default_logic(&mut self) {
        if self.logic_touched {
            return;
        }

        let source_needs_all = self.source_mode.is_some_and(|mode| {
            matches!(
                mode,
                WizardTriggerMode::NumericAbove | WizardTriggerMode::NumericBelow
            )
        });
        self.condition_logic = if source_needs_all || !self.extra_conditions.is_empty() {
            ConditionLogic::All
        } else {
            ConditionLogic::Any
        };
    }
}

impl Default for RecordingRuleWizard {
    fn default() -> Self {
        Self::new(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizardPendingCondition {
    pub device_id: i64,
    pub operator: Option<ConditionOperator>,
}

impl WizardTriggerMode {
    pub fn button_label(self, lang: Language) -> &'static str {
        match (lang, self) {
            (Language::Ru, Self::OpenAndClose) => "🚪 Открытие и закрытие",
            (Language::En, Self::OpenAndClose) => "🚪 Open and close",
            (Language::Ru, Self::OpenOnly) => "📬 Только открытие",
            (Language::En, Self::OpenOnly) => "📬 Open only",
            (Language::Ru, Self::CloseOnly) => "🔒 Только закрытие",
            (Language::En, Self::CloseOnly) => "🔒 Close only",
            (Language::Ru, Self::Detected) => "🏃 Появилось",
            (Language::En, Self::Detected) => "🏃 Detected",
            (Language::Ru, Self::Cleared) => "✅ Пропало",
            (Language::En, Self::Cleared) => "✅ Cleared",
            (Language::Ru, Self::DetectedAndCleared) => "↔️ Появилось и пропало",
            (Language::En, Self::DetectedAndCleared) => "↔️ Detected and cleared",
            (Language::Ru, Self::TurnedOn) => "⚡ Включение",
            (Language::En, Self::TurnedOn) => "⚡ Turned on",
            (Language::Ru, Self::TurnedOff) => "⏻ Выключение",
            (Language::En, Self::TurnedOff) => "⏻ Turned off",
            (Language::Ru, Self::TurnedOnAndOff) => "↔️ Включение и выключение",
            (Language::En, Self::TurnedOnAndOff) => "↔️ On and off",
            (Language::Ru, Self::AnyChange) => "🔁 Любое изменение",
            (Language::En, Self::AnyChange) => "🔁 Any change",
            (Language::Ru, Self::NumericAbove) => "⬆️ Стало выше значения",
            (Language::En, Self::NumericAbove) => "⬆️ Became above value",
            (Language::Ru, Self::NumericBelow) => "⬇️ Стало ниже значения",
            (Language::En, Self::NumericBelow) => "⬇️ Became below value",
        }
    }

    pub fn summary_label(self, lang: Language) -> &'static str {
        match (lang, self) {
            (Language::Ru, Self::OpenAndClose) => "открытие и закрытие",
            (Language::En, Self::OpenAndClose) => "open and close",
            (Language::Ru, Self::OpenOnly) => "только открытие",
            (Language::En, Self::OpenOnly) => "open only",
            (Language::Ru, Self::CloseOnly) => "только закрытие",
            (Language::En, Self::CloseOnly) => "close only",
            (Language::Ru, Self::Detected) => "появилось",
            (Language::En, Self::Detected) => "detected",
            (Language::Ru, Self::Cleared) => "пропало",
            (Language::En, Self::Cleared) => "cleared",
            (Language::Ru, Self::DetectedAndCleared) => "появилось и пропало",
            (Language::En, Self::DetectedAndCleared) => "detected and cleared",
            (Language::Ru, Self::TurnedOn) => "включение",
            (Language::En, Self::TurnedOn) => "turned on",
            (Language::Ru, Self::TurnedOff) => "выключение",
            (Language::En, Self::TurnedOff) => "turned off",
            (Language::Ru, Self::TurnedOnAndOff) => "включение и выключение",
            (Language::En, Self::TurnedOnAndOff) => "on and off",
            (Language::Ru, Self::AnyChange) => "любое изменение",
            (Language::En, Self::AnyChange) => "any change",
            (Language::Ru, Self::NumericAbove) => "стало выше значения",
            (Language::En, Self::NumericAbove) => "became above value",
            (Language::Ru, Self::NumericBelow) => "стало ниже значения",
            (Language::En, Self::NumericBelow) => "became below value",
        }
    }

    pub fn needs_value(self) -> bool {
        matches!(self, Self::NumericAbove | Self::NumericBelow)
    }

    fn is_multi_event(self) -> bool {
        matches!(
            self,
            Self::OpenAndClose | Self::DetectedAndCleared | Self::TurnedOnAndOff
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizardCondition {
    pub entity_id: String,
    pub operator: ConditionOperator,
    pub from_state: Option<String>,
    pub to_state: Option<String>,
    pub value: Option<String>,
}

pub fn build_rule_name(
    lang: Language,
    display_name: &str,
    mode: WizardTriggerMode,
    value: Option<&str>,
) -> String {
    match (lang, mode) {
        (Language::Ru, WizardTriggerMode::OpenAndClose) => {
            format!("{}: открытие или закрытие", display_name)
        }
        (Language::En, WizardTriggerMode::OpenAndClose) => {
            format!("{}: open or close", display_name)
        }
        (Language::Ru, WizardTriggerMode::OpenOnly) => format!("{}: открытие", display_name),
        (Language::En, WizardTriggerMode::OpenOnly) => format!("{}: open", display_name),
        (Language::Ru, WizardTriggerMode::CloseOnly) => format!("{}: закрытие", display_name),
        (Language::En, WizardTriggerMode::CloseOnly) => format!("{}: close", display_name),
        (Language::Ru, WizardTriggerMode::Detected) => format!("{}: появилось", display_name),
        (Language::En, WizardTriggerMode::Detected) => format!("{}: detected", display_name),
        (Language::Ru, WizardTriggerMode::Cleared) => format!("{}: пропало", display_name),
        (Language::En, WizardTriggerMode::Cleared) => format!("{}: cleared", display_name),
        (Language::Ru, WizardTriggerMode::DetectedAndCleared) => {
            format!("{}: появилось или пропало", display_name)
        }
        (Language::En, WizardTriggerMode::DetectedAndCleared) => {
            format!("{}: detected or cleared", display_name)
        }
        (Language::Ru, WizardTriggerMode::TurnedOn) => format!("{}: включение", display_name),
        (Language::En, WizardTriggerMode::TurnedOn) => format!("{}: turned on", display_name),
        (Language::Ru, WizardTriggerMode::TurnedOff) => format!("{}: выключение", display_name),
        (Language::En, WizardTriggerMode::TurnedOff) => format!("{}: turned off", display_name),
        (Language::Ru, WizardTriggerMode::TurnedOnAndOff) => {
            format!("{}: включение или выключение", display_name)
        }
        (Language::En, WizardTriggerMode::TurnedOnAndOff) => {
            format!("{}: on or off", display_name)
        }
        (Language::Ru, WizardTriggerMode::AnyChange) => {
            format!("{}: любое изменение", display_name)
        }
        (Language::En, WizardTriggerMode::AnyChange) => {
            format!("{}: any change", display_name)
        }
        (Language::Ru, WizardTriggerMode::NumericAbove) => {
            format!("{}: выше {}", display_name, value.unwrap_or("?"))
        }
        (Language::En, WizardTriggerMode::NumericAbove) => {
            format!("{}: above {}", display_name, value.unwrap_or("?"))
        }
        (Language::Ru, WizardTriggerMode::NumericBelow) => {
            format!("{}: ниже {}", display_name, value.unwrap_or("?"))
        }
        (Language::En, WizardTriggerMode::NumericBelow) => {
            format!("{}: below {}", display_name, value.unwrap_or("?"))
        }
    }
}

pub fn build_source_conditions(
    entity_id: &str,
    mode: WizardTriggerMode,
    value: Option<&str>,
    has_context_conditions: bool,
) -> Result<Vec<WizardCondition>, String> {
    if has_context_conditions && mode.is_multi_event() {
        return Ok(vec![any_change_condition(entity_id)]);
    }

    match mode {
        WizardTriggerMode::OpenAndClose
        | WizardTriggerMode::DetectedAndCleared
        | WizardTriggerMode::TurnedOnAndOff => Ok(vec![
            transition_condition(entity_id, "off", "on"),
            transition_condition(entity_id, "on", "off"),
        ]),
        WizardTriggerMode::OpenOnly | WizardTriggerMode::Detected | WizardTriggerMode::TurnedOn => {
            Ok(vec![transition_condition(entity_id, "off", "on")])
        }
        WizardTriggerMode::CloseOnly
        | WizardTriggerMode::Cleared
        | WizardTriggerMode::TurnedOff => Ok(vec![transition_condition(entity_id, "on", "off")]),
        WizardTriggerMode::AnyChange => Ok(vec![any_change_condition(entity_id)]),
        WizardTriggerMode::NumericAbove => {
            let value = normalized_number(value)?;
            Ok(vec![
                any_change_condition(entity_id),
                current_value_condition(entity_id, ConditionOperator::Above, &value),
            ])
        }
        WizardTriggerMode::NumericBelow => {
            let value = normalized_number(value)?;
            Ok(vec![
                any_change_condition(entity_id),
                current_value_condition(entity_id, ConditionOperator::Below, &value),
            ])
        }
    }
}

#[cfg(test)]
pub fn build_conditions(
    entity_id: &str,
    mode: WizardTriggerMode,
) -> Result<Vec<WizardCondition>, String> {
    build_source_conditions(entity_id, mode, None, false)
}

pub fn build_final_conditions(
    entity_id: &str,
    mode: WizardTriggerMode,
    value: Option<&str>,
    extra_conditions: &[WizardCondition],
) -> Result<Vec<WizardCondition>, String> {
    let mut conditions =
        build_source_conditions(entity_id, mode, value, !extra_conditions.is_empty())?;
    conditions.extend(extra_conditions.iter().cloned());
    Ok(conditions)
}

#[cfg(test)]
pub fn build_advanced_rule_text(
    name: &str,
    camera_id: i64,
    conditions: &[WizardCondition],
    tail_seconds: u32,
    max_segment_seconds: u32,
    cooldown_s: u32,
    retention_days: u32,
) -> String {
    build_advanced_rule_text_with_logic(
        name,
        camera_id,
        ConditionLogic::Any,
        conditions,
        tail_seconds,
        max_segment_seconds,
        cooldown_s,
        retention_days,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_advanced_rule_text_with_logic(
    name: &str,
    camera_id: i64,
    logic: ConditionLogic,
    conditions: &[WizardCondition],
    tail_seconds: u32,
    max_segment_seconds: u32,
    cooldown_s: u32,
    retention_days: u32,
) -> String {
    let mut lines = vec![
        name.to_string(),
        camera_id.to_string(),
        logic.as_str().to_string(),
    ];
    lines.extend(conditions.iter().map(condition_line));
    lines.push(tail_seconds.to_string());
    lines.push(max_segment_seconds.to_string());
    lines.push(cooldown_s.to_string());
    lines.push(retention_days.to_string());
    lines.join("\n")
}

pub fn condition_line(condition: &WizardCondition) -> String {
    format!(
        "{};{};{};{};{}",
        condition.entity_id,
        condition.operator.as_str(),
        condition.from_state.as_deref().unwrap_or(""),
        condition.to_state.as_deref().unwrap_or(""),
        condition.value.as_deref().unwrap_or("")
    )
}

pub fn condition_operator_label(operator: ConditionOperator, lang: Language) -> &'static str {
    match (lang, operator) {
        (Language::Ru, ConditionOperator::Is) => "равно",
        (Language::En, ConditionOperator::Is) => "equals",
        (Language::Ru, ConditionOperator::IsNot) => "не равно",
        (Language::En, ConditionOperator::IsNot) => "not equals",
        (Language::Ru, ConditionOperator::Contains) => "содержит",
        (Language::En, ConditionOperator::Contains) => "contains",
        (Language::Ru, ConditionOperator::Above) => "выше",
        (Language::En, ConditionOperator::Above) => "above",
        (Language::Ru, ConditionOperator::Below) => "ниже",
        (Language::En, ConditionOperator::Below) => "below",
        (Language::Ru, ConditionOperator::ChangedTo) => "изменилось на",
        (Language::En, ConditionOperator::ChangedTo) => "changed to",
        (Language::Ru, ConditionOperator::ChangedFromTo) => "изменилось с ... на ...",
        (Language::En, ConditionOperator::ChangedFromTo) => "changed from ... to ...",
    }
}

pub fn condition_from_input(
    entity_id: &str,
    operator: ConditionOperator,
    raw_input: &str,
) -> Result<WizardCondition, String> {
    let value = raw_input.trim();
    if value.is_empty() {
        return Err("Значение не должно быть пустым.".to_string());
    }

    match operator {
        ConditionOperator::Is | ConditionOperator::IsNot | ConditionOperator::Contains => {
            Ok(WizardCondition {
                entity_id: entity_id.to_string(),
                operator,
                from_state: None,
                to_state: None,
                value: Some(value.to_string()),
            })
        }
        ConditionOperator::Above | ConditionOperator::Below => {
            let value = normalized_number(Some(value))?;
            Ok(WizardCondition {
                entity_id: entity_id.to_string(),
                operator,
                from_state: None,
                to_state: None,
                value: Some(value),
            })
        }
        ConditionOperator::ChangedTo => Ok(WizardCondition {
            entity_id: entity_id.to_string(),
            operator,
            from_state: None,
            to_state: Some(value.to_string()),
            value: None,
        }),
        ConditionOperator::ChangedFromTo => {
            let (from, to) = parse_from_to(value)?;
            Ok(WizardCondition {
                entity_id: entity_id.to_string(),
                operator,
                from_state: Some(from),
                to_state: Some(to),
                value: None,
            })
        }
    }
}

pub fn source_threshold_value(raw_input: &str) -> Result<String, String> {
    normalized_number(Some(raw_input.trim()))
}

pub fn default_cooldown_s(mode: WizardTriggerMode) -> u32 {
    match mode {
        WizardTriggerMode::Detected
        | WizardTriggerMode::Cleared
        | WizardTriggerMode::DetectedAndCleared
        | WizardTriggerMode::NumericAbove
        | WizardTriggerMode::NumericBelow => 30,
        _ => 0,
    }
}

fn transition_condition(entity_id: &str, from: &str, to: &str) -> WizardCondition {
    WizardCondition {
        entity_id: entity_id.to_string(),
        operator: ConditionOperator::ChangedFromTo,
        from_state: Some(from.to_string()),
        to_state: Some(to.to_string()),
        value: None,
    }
}

fn any_change_condition(entity_id: &str) -> WizardCondition {
    WizardCondition {
        entity_id: entity_id.to_string(),
        operator: ConditionOperator::ChangedTo,
        from_state: None,
        to_state: None,
        value: None,
    }
}

fn current_value_condition(
    entity_id: &str,
    operator: ConditionOperator,
    value: &str,
) -> WizardCondition {
    WizardCondition {
        entity_id: entity_id.to_string(),
        operator,
        from_state: None,
        to_state: None,
        value: Some(value.to_string()),
    }
}

fn normalized_number(value: Option<&str>) -> Result<String, String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Введите числовое значение.".to_string())?;
    let number = value
        .parse::<f64>()
        .map_err(|_| "Значение должно быть числом.".to_string())?;
    if !number.is_finite() {
        return Err("Значение должно быть конечным числом.".to_string());
    }
    Ok(value.to_string())
}

fn parse_from_to(value: &str) -> Result<(String, String), String> {
    let parts = if let Some((from, to)) = value.split_once(';') {
        (from, to)
    } else if let Some((from, to)) = value.split_once("->") {
        (from, to)
    } else if let Some((from, to)) = value.split_once(',') {
        (from, to)
    } else {
        return Err("Введите два состояния в формате `from;to`.".to_string());
    };

    let from = parts.0.trim();
    let to = parts.1.trim();
    if from.is_empty() || to.is_empty() {
        return Err("Оба состояния должны быть заполнены.".to_string());
    }

    Ok((from.to_string(), to.to_string()))
}

pub const DRAFT_KIND: &str = "recording_rule";

pub async fn restore_draft(
    config: &crate::models::AppConfig,
    owner: u64,
    id: Option<i64>,
) -> anyhow::Result<Option<RecordingRuleWizard>> {
    let draft = match id {
        Some(id) => crate::db::ui_drafts::load(id, owner, DRAFT_KIND, &config.db).await?,
        None => crate::db::ui_drafts::latest(owner, DRAFT_KIND, &config.db).await?,
    };
    let Some(draft) = draft else {
        return Ok(None);
    };
    anyhow::ensure!(
        draft.schema_version == 1,
        "Unsupported recording draft version"
    );
    let mut wizard: RecordingRuleWizard = serde_json::from_str(&draft.payload)?;
    wizard.draft_id = Some(draft.id);
    wizard.revision = draft.revision;
    wizard.committed_rule_id = draft.committed_target_id;
    wizard.step = draft.step;
    Ok(Some(wizard))
}

/// Save a snapshot only after a successful input/navigation transition.
pub async fn persist_draft(config: &crate::models::AppConfig, owner: u64) -> anyhow::Result<()> {
    let Some(mut wizard) = config
        .sessions
        .get(&owner)
        .and_then(|s| s.recording_rule_wizard.clone())
    else {
        return Ok(());
    };
    if wizard.committed_rule_id.is_some() {
        return Ok(());
    }
    if !wizard.name_touched {
        if let (Some(device), Some(mode)) = (wizard.source_device_id, wizard.source_mode) {
            if let Some(source) =
                crate::db::devices::get_recording_wizard_candidate(device, &config.db).await?
            {
                let lang = crate::db::get_user_language(owner, &config.db)
                    .await?
                    .unwrap_or(config.default_language);
                wizard.name = Some(build_rule_name(
                    lang,
                    &source.display_name,
                    mode,
                    wizard.source_value.as_deref(),
                ));
            }
        }
    }

    if let Some(id) = wizard.draft_id {
        let json = serde_json::to_string(&wizard)?;
        let existing = crate::db::ui_drafts::load(id, owner, DRAFT_KIND, &config.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Draft expired; reopen the wizard"))?;
        // Revision is carried separately in the row, not in the payload comparison.
        let mut previous: RecordingRuleWizard = serde_json::from_str(&existing.payload)?;
        previous.revision = wizard.revision;
        previous.draft_id = wizard.draft_id;
        if previous == wizard && existing.step == wizard.step {
            return Ok(());
        }
        wizard.revision = crate::db::ui_drafts::save(
            id,
            owner,
            wizard.revision,
            &json,
            &wizard.step,
            if wizard.return_to_summary {
                "summary"
            } else {
                "linear"
            },
            false,
            config.session_ttl_hours,
            &config.db,
        )
        .await?;
    } else {
        let row = crate::db::ui_drafts::create(
            owner,
            DRAFT_KIND,
            &serde_json::to_string(&wizard)?,
            config.session_ttl_hours,
            &config.db,
        )
        .await?;
        wizard.draft_id = Some(row.id);
        wizard.revision = crate::db::ui_drafts::save(
            row.id,
            owner,
            row.revision,
            &serde_json::to_string(&wizard)?,
            &wizard.step,
            if wizard.return_to_summary {
                "summary"
            } else {
                "linear"
            },
            false,
            config.session_ttl_hours,
            &config.db,
        )
        .await?;
    }
    if let Some(mut session) = config.sessions.get_mut(&owner) {
        session.recording_rule_wizard = Some(wizard);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_close_generates_two_transition_conditions() {
        let conditions =
            build_conditions("binary_sensor.front_door", WizardTriggerMode::OpenAndClose)
                .expect("conditions");

        assert_eq!(conditions.len(), 2);
        assert_eq!(
            condition_line(&conditions[0]),
            "binary_sensor.front_door;changed_from_to;off;on;"
        );
        assert_eq!(
            condition_line(&conditions[1]),
            "binary_sensor.front_door;changed_from_to;on;off;"
        );
    }

    #[test]
    fn rule_name_uses_selected_language() {
        assert_eq!(
            build_rule_name(
                Language::Ru,
                "Замок Дверь",
                WizardTriggerMode::OpenAndClose,
                None
            ),
            "Замок Дверь: открытие или закрытие"
        );
        assert_eq!(
            build_rule_name(
                Language::En,
                "Front door",
                WizardTriggerMode::OpenAndClose,
                None
            ),
            "Front door: open or close"
        );
    }

    #[test]
    fn advanced_rule_text_matches_existing_input_format() {
        let conditions =
            build_conditions("binary_sensor.front_door", WizardTriggerMode::OpenAndClose)
                .expect("conditions");
        let text =
            build_advanced_rule_text("Front door: open or close", 3, &conditions, 60, 300, 0, 30);

        assert_eq!(
            text,
            "Front door: open or close\n3\nany\nbinary_sensor.front_door;changed_from_to;off;on;\nbinary_sensor.front_door;changed_from_to;on;off;\n60\n300\n0\n30"
        );
    }

    #[test]
    fn numeric_above_generates_any_change_plus_threshold() {
        let conditions = build_source_conditions(
            "sensor.kitchen_temperature",
            WizardTriggerMode::NumericAbove,
            Some("28"),
            false,
        )
        .expect("conditions");

        assert_eq!(conditions.len(), 2);
        assert_eq!(
            condition_line(&conditions[0]),
            "sensor.kitchen_temperature;changed_to;;;"
        );
        assert_eq!(
            condition_line(&conditions[1]),
            "sensor.kitchen_temperature;above;;;28"
        );
    }

    #[test]
    fn multi_event_with_context_uses_any_change_source() {
        let extra = condition_from_input("binary_sensor.security", ConditionOperator::Is, "on")
            .expect("condition");
        let conditions = build_final_conditions(
            "binary_sensor.front_door",
            WizardTriggerMode::OpenAndClose,
            None,
            &[extra],
        )
        .expect("conditions");

        assert_eq!(
            condition_line(&conditions[0]),
            "binary_sensor.front_door;changed_to;;;"
        );
        assert_eq!(
            condition_line(&conditions[1]),
            "binary_sensor.security;is;;;on"
        );
    }
}

#[cfg(test)]
mod remediation_tests {
    use super::*;

    fn filled() -> RecordingRuleWizard {
        let mut wizard = RecordingRuleWizard::new(3);
        wizard.camera_id = Some(5);
        wizard.source_device_id = Some(7);
        wizard.source_mode = Some(WizardTriggerMode::NumericAbove);
        wizard.source_value = Some("24.5".into());
        wizard.name = Some("My recording".into());
        wizard.name_touched = true;
        wizard.cooldown_s = Some(123);
        wizard.cooldown_touched = true;
        wizard.condition_logic = ConditionLogic::Any;
        wizard.logic_touched = true;
        wizard.tail_seconds = Some(120);
        wizard.retention_days = Some(90);
        wizard.active_time = RecordingRuleActiveTime::window(1320, 420, 0b001_1111).unwrap();
        wizard.group_ids = vec![1, 2];
        wizard.extra_conditions.push(
            condition_from_input("binary_sensor.guard", ConditionOperator::Is, "on").unwrap(),
        );
        wizard
    }

    #[test]
    fn source_changes_only_reset_source_dependencies() {
        let mut wizard = filled();
        let original = wizard.clone();
        wizard.reset_source(7);
        assert_eq!(wizard, original);
        wizard.reset_source(8);
        assert_eq!(wizard.source_mode, None);
        assert_eq!(wizard.source_value, None);
        let mut expected = original;
        expected.source_device_id = Some(8);
        expected.source_mode = None;
        expected.source_value = None;
        assert_eq!(wizard, expected);
    }

    #[test]
    fn opposite_numeric_operator_keeps_threshold_and_manual_defaults() {
        let mut wizard = filled();
        wizard.set_source_mode(WizardTriggerMode::NumericBelow, None);
        assert_eq!(wizard.source_value.as_deref(), Some("24.5"));
        assert_eq!(wizard.cooldown_s, Some(123));
        assert_eq!(wizard.condition_logic, ConditionLogic::Any);
        wizard.set_source_mode(WizardTriggerMode::AnyChange, None);
        assert!(wizard.source_value.is_none());
        assert_eq!(wizard.tail_seconds, Some(120));
        assert_eq!(wizard.group_ids, vec![1, 2]);
    }

    #[test]
    fn complete_draft_and_legacy_json_round_trip_without_losing_fields() {
        let mut wizard = filled();
        wizard.custom_conditions = Some(vec![condition_from_input(
            "sensor.other",
            ConditionOperator::Below,
            "0",
        )
        .unwrap()]);
        wizard.raw_text = Some("unfinished block".into());
        wizard.pending_input = Some("22:".into());
        wizard.max_segment_seconds = Some(240);
        wizard.pre_roll_enabled = Some(true);
        assert_eq!(
            serde_json::from_str::<RecordingRuleWizard>(&serde_json::to_string(&wizard).unwrap())
                .unwrap(),
            wizard
        );
        let legacy = serde_json::json!({"room_id":3,"camera_id":5,"source_device_id":7,"source_mode":"NumericAbove","source_value":"24.5"});
        let decoded: RecordingRuleWizard = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.source_value.as_deref(), Some("24.5"));
        assert_eq!(decoded.pre_roll_seconds, 15);
        assert_eq!(
            wizard
                .conditions("ignored", WizardTriggerMode::AnyChange)
                .unwrap(),
            wizard.custom_conditions.unwrap()
        );
    }

    #[test]
    fn thresholds_reject_non_finite_numbers() {
        for value in ["NaN", "inf", "-inf", "1e999"] {
            assert!(source_threshold_value(value).is_err());
        }
    }
}
