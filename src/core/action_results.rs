//! Persisted send outcomes. A missing response never proves that HA did nothing.
use crate::{db, i18n::Language, models::AppConfig};
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Sent,
    Partial,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Device,
    HaNative,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetResult {
    pub kind: TargetKind,
    pub id: i64,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub created_at: i64,
}

impl TargetResult {
    pub fn new(kind: TargetKind, id: i64, outcome: Outcome, reason: Option<&str>) -> Self {
        Self {
            kind,
            id,
            outcome,
            reason: reason.map(str::to_owned),
            created_at: chrono::Utc::now().timestamp(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub outcome: Outcome,
    pub targets: Vec<TargetResult>,
    pub created_at: i64,
}

impl ActionResult {
    pub fn new(targets: Vec<TargetResult>) -> Self {
        let sent = targets
            .iter()
            .filter(|t| t.outcome == Outcome::Sent)
            .count();
        let unknown = targets.iter().any(|t| t.outcome == Outcome::Unknown);
        let outcome = if !targets.is_empty() && sent == targets.len() {
            Outcome::Sent
        } else if sent > 0 {
            Outcome::Partial
        } else if unknown {
            Outcome::Unknown
        } else {
            Outcome::Failed
        };
        Self {
            outcome,
            targets,
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn summary(&self, lang: Language) -> String {
        let sent = self
            .targets
            .iter()
            .filter(|t| t.outcome == Outcome::Sent)
            .count();
        let failed = self
            .targets
            .iter()
            .filter(|t| t.outcome == Outcome::Failed)
            .count();
        let unknown = self
            .targets
            .iter()
            .filter(|t| t.outcome == Outcome::Unknown)
            .count();
        let en = lang == Language::En;
        let mut text = match self.outcome {
            Outcome::Sent if en => format!(
                "Commands sent: {sent} of {}. Waiting for state updates.",
                self.targets.len()
            ),
            Outcome::Sent => format!(
                "Команды отправлены {sent} из {}. Ожидается обновление состояний.",
                self.targets.len()
            ),
            Outcome::Partial if en => format!("Commands sent: {sent} of {}.", self.targets.len()),
            Outcome::Partial => format!("Команды отправлены {sent} из {}.", self.targets.len()),
            Outcome::Failed if en => "Commands were not sent.".into(),
            Outcome::Failed => "Команды не отправлены.".into(),
            Outcome::Unknown if en => {
                "The send outcome could not be determined. Check states first.".into()
            }
            Outcome::Unknown => {
                "Результат отправки не удалось определить. Сначала проверьте состояния.".into()
            }
        };
        if failed > 0 || unknown > 0 {
            text.push_str(&if en {
                format!("\nSent: {sent}; rejected: {failed}; unknown: {unknown}.")
            } else {
                format!("\nОтправлены: {sent}; отказ: {failed}; неизвестно: {unknown}.")
            });
        }
        text
    }

    /// Resolve names against today's ACL; never reveal names captured before revocation.
    pub async fn display(&self, user: u64, lang: Language, config: &AppConfig) -> Result<String> {
        let mut text = self.summary(lang);
        for target in &self.targets {
            if target.outcome == Outcome::Sent {
                continue;
            }
            let label = target_label(target, user, config)
                .await?
                .unwrap_or_else(|| {
                    if lang == Language::En {
                        "Unavailable target".into()
                    } else {
                        "Недоступная цель".into()
                    }
                });
            let status = match (target.outcome, lang == Language::En) {
                (Outcome::Unknown, true) => "unknown; check state",
                (Outcome::Unknown, false) => "исход неизвестен; проверьте состояние",
                (_, true) => "not sent",
                (_, false) => "не отправлено",
            };
            text.push_str(&format!("\n{label}: {status}"));
            if let Some(reason) = target.reason.as_deref() {
                let reason = reason
                    .split_once(" / ")
                    .map(|(english, russian)| {
                        if lang == Language::En {
                            english
                        } else {
                            russian
                        }
                    })
                    .unwrap_or(reason);
                text.push_str(&format!(" ({reason})"));
            }
        }
        Ok(text)
    }
}

pub async fn target_label(
    target: &TargetResult,
    user: u64,
    config: &AppConfig,
) -> Result<Option<String>> {
    let admin = user == config.root_user;
    match target.kind {
        TargetKind::Device => {
            if !db::access::can_view_device(user, admin, target.id, &config.db).await? {
                return Ok(None);
            }
            Ok(db::devices::get_device_by_id(target.id, &config.db)
                .await?
                .map(|d| d.alias.unwrap_or(d.entity_id)))
        }
        TargetKind::HaNative => Ok(
            db::action_groups::get_ha_native_target(target.id, &config.db)
                .await?
                .filter(|t| !t.is_archived() && (admin || t.is_visible_to_all_users()))
                .map(|t| t.display_name().to_string()),
        ),
    }
}

/// Only local construction/connect failures establish that no request was dispatched.
pub fn definitely_not_sent(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|e| e.downcast_ref::<reqwest::Error>())
        .any(|e| e.is_builder() || e.is_connect())
}

pub fn send_error(kind: TargetKind, id: i64, error: &anyhow::Error) -> TargetResult {
    if definitely_not_sent(error) {
        TargetResult::new(
            kind,
            id,
            Outcome::Failed,
            Some("HA connection unavailable / Нет соединения с HA"),
        )
    } else {
        TargetResult::new(
            kind,
            id,
            Outcome::Unknown,
            Some("HA response unavailable / Нет подтверждения HA"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregates_actual_outcomes_without_counting_unknown_as_failure() {
        let mut targets = (1..=9)
            .map(|id| TargetResult::new(TargetKind::Device, id, Outcome::Sent, None))
            .collect::<Vec<_>>();
        targets.push(TargetResult::new(
            TargetKind::Device,
            10,
            Outcome::Unknown,
            None,
        ));
        let result = ActionResult::new(targets);
        assert_eq!(result.outcome, Outcome::Partial);
        assert!(result.summary(Language::En).contains("9 of 10"));
        assert!(result
            .summary(Language::En)
            .contains("rejected: 0; unknown: 1"));
        let restored: ActionResult =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(restored.targets.len(), 10);
        assert_eq!(
            ActionResult::new(vec![TargetResult::new(
                TargetKind::Device,
                1,
                Outcome::Unknown,
                None
            )])
            .outcome,
            Outcome::Unknown
        );
    }
    #[test]
    fn generic_transport_error_does_not_prove_no_effect() {
        assert!(!definitely_not_sent(&anyhow::anyhow!(
            "HTTP 500 after sending"
        )));
    }
}
