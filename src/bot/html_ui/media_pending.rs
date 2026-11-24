//! Video delivery state belongs to a worker, not a widget revision or HA entity.
use crate::{
    bot::{
        models::View,
        router::{AdminPayload, CameraPayload, Payload},
    },
    db,
    i18n::Language,
    models::{AppConfig, UiMessageMode},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use teloxide::{prelude::*, types::MessageId};
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Clip {
        camera: i64,
        seconds: u32,
    },
    Segment {
        camera: i64,
        session: i64,
        segment: i64,
    },
    All {
        camera: i64,
        session: i64,
    },
}
impl Target {
    pub fn from_payload(payload: &Payload) -> Option<Self> {
        match *payload {
            Payload::Camera(CameraPayload::Clip { id, seconds }) => Some(Self::Clip {
                camera: id,
                seconds,
            }),
            Payload::Admin(AdminPayload::RoomCameraClip {
                camera, seconds, ..
            }) => Some(Self::Clip { camera, seconds }),
            Payload::Camera(CameraPayload::SendRecordingSegment {
                camera,
                session,
                segment,
            }) => Some(Self::Segment {
                camera,
                session,
                segment,
            }),
            Payload::Camera(CameraPayload::SendRecordingAll { camera, session }) => {
                Some(Self::All { camera, session })
            }
            _ => None,
        }
    }
    pub fn camera(&self) -> i64 {
        match *self {
            Self::Clip { camera, .. } | Self::Segment { camera, .. } | Self::All { camera, .. } => {
                camera
            }
        }
    }
    fn conflicts(&self, other: &Self) -> bool {
        match (self, other) {
            // Different requested durations still capture the same camera.
            (Self::Clip { camera: a, .. }, Self::Clip { camera: b, .. }) => a == b,
            (
                Self::Segment {
                    camera: a,
                    session: x,
                    segment: i,
                },
                Self::Segment {
                    camera: b,
                    session: y,
                    segment: j,
                },
            ) => a == b && x == y && i == j,
            (
                Self::All {
                    camera: a,
                    session: x,
                },
                Self::All {
                    camera: b,
                    session: y,
                },
            )
            | (
                Self::All {
                    camera: a,
                    session: x,
                },
                Self::Segment {
                    camera: b,
                    session: y,
                    ..
                },
            )
            | (
                Self::Segment {
                    camera: a,
                    session: x,
                    ..
                },
                Self::All {
                    camera: b,
                    session: y,
                },
            ) => a == b && x == y,
            _ => false,
        }
    }
    pub fn matches_view(&self, payload: &Payload) -> bool {
        match (self, payload) {
            (Self::Clip { camera, .. }, Payload::Camera(CameraPayload::CameraDetail { id })) => {
                camera == id
            }
            (
                Self::Clip { camera, .. },
                Payload::Admin(AdminPayload::RoomCameraDetail { camera: id, .. }),
            ) => camera == id,
            (
                Self::Segment {
                    camera: a,
                    session: x,
                    ..
                }
                | Self::All {
                    camera: a,
                    session: x,
                },
                Payload::Camera(CameraPayload::RecordingSession {
                    camera: b,
                    session: y,
                }),
            ) => a == b && x == y,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub sent: usize,
    pub failed: usize,
    pub uncertain: bool,
    pub reported_in_chat: bool,
}
impl Outcome {
    pub fn sent() -> Self {
        Self {
            sent: 1,
            ..Self::default()
        }
    }
    pub fn failed() -> Self {
        Self {
            failed: 1,
            ..Self::default()
        }
    }
    pub fn reported_failure() -> Self {
        Self {
            reported_in_chat: true,
            ..Self::failed()
        }
    }
    pub fn from_error(error: &anyhow::Error) -> Self {
        Self {
            uncertain: error.is::<UploadUncertain>(),
            ..Self::failed()
        }
    }
    fn text(self, english: bool) -> String {
        if self.uncertain {
            if english {
                format!(
                    "Delivery not confirmed. Sent: {}. Check the chat before retrying.",
                    self.sent
                )
            } else {
                format!(
                    "Доставка не подтверждена. Отправлено: {}. Проверьте чат перед повтором.",
                    self.sent
                )
            }
        } else if self.failed > 0 && self.sent > 0 {
            if english {
                format!(
                    "Sent: {}. Unavailable or failed: {}.",
                    self.sent, self.failed
                )
            } else {
                format!(
                    "Отправлено: {}. Недоступно или не отправлено: {}.",
                    self.sent, self.failed
                )
            }
        } else if self.failed > 0 {
            if english {
                "Video could not be sent. You can retry.".into()
            } else {
                "Видео не отправлено. Можно повторить попытку.".into()
            }
        } else if english {
            format!("Videos sent: {}", self.sent)
        } else {
            format!("Видео отправлено: {}", self.sent)
        }
    }
}

#[derive(Debug)]
pub(crate) struct UploadUncertain;
impl std::fmt::Display for UploadUncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Video delivery was not confirmed by Telegram")
    }
}
impl std::error::Error for UploadUncertain {}

pub(crate) struct Receipt {
    pub bot: Bot,
    pub widget: bool,
    pub rendered: Option<oneshot::Receiver<()>>,
}

struct Entry {
    id: u64,
    generation: String,
    target: Target,
    outcome: Option<Outcome>,
    shown: bool,
}
#[derive(Default)]
pub(crate) struct Registry {
    entries: Mutex<HashMap<u64, Vec<Entry>>>,
}
impl Registry {
    pub fn begin(
        &self,
        config: &Arc<AppConfig>,
        user: u64,
        generation: &str,
        target: Target,
        receipt: Receipt,
    ) -> Option<Operation> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let entries = entries.entry(user).or_default();
        entries.retain(|e| {
            if e.generation != generation {
                if let Target::All { session, .. } = e.target {
                    config.finish_recording_send(user, session);
                }
                return false;
            }
            true
        });
        if entries
            .iter()
            .any(|e| e.outcome.is_none() && e.target.conflicts(&target))
        {
            return None;
        }
        entries.retain(|e| e.outcome.is_none() || e.target.camera() != target.camera());
        let id = super::sequence();
        if let Target::All { session, .. } = target {
            if !config.start_recording_send(user, session) {
                return None;
            }
        }
        entries.push(Entry {
            id,
            generation: generation.into(),
            target: target.clone(),
            outcome: None,
            shown: false,
        });
        Some(Operation {
            config: config.clone(),
            user,
            pending: Some((id, receipt)),
            target,
            generation: generation.into(),
        })
    }
    pub fn blocks(&self, user: u64, generation: &str, target: &Target) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&user)
            .is_some_and(|entries| {
                entries.iter().any(|e| {
                    e.generation == generation && e.outcome.is_none() && e.target.conflicts(target)
                })
            })
    }
    pub fn decorate(&self, user: u64, generation: &str, view: &mut View) -> Option<u64> {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let entries = entries.get(&user)?;
        let entry = entries
            .iter()
            .rev()
            .find(|e| e.generation == generation && e.target.matches_view(&view.payload))?;
        let outcome = entry.outcome?;
        let text = outcome.text(view.lang == Language::En);
        if outcome.failed > 0 || outcome.uncertain {
            view.alert = Some(text);
        } else {
            view.notice = Some(text);
        }
        Some(entry.id)
    }
    pub fn mark_shown(&self, user: u64, id: Option<u64>) {
        let Some(id) = id else {
            return;
        };
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = entries
            .get_mut(&user)
            .and_then(|entries| entries.iter_mut().find(|e| e.id == id))
        {
            entry.shown = true;
        }
    }
    fn is_shown(&self, user: u64, id: u64) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&user)
            .and_then(|entries| entries.iter().find(|e| e.id == id))
            .is_none_or(|e| e.shown)
    }
    fn finish(&self, user: u64, id: u64, outcome: Option<Outcome>) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let Some(operations) = entries.get_mut(&user) else {
            return false;
        };
        let Some(index) = operations.iter().position(|entry| entry.id == id) else {
            return false;
        };
        if let Some(outcome) = outcome {
            operations[index].outcome = Some(outcome);
        } else {
            operations.remove(index);
            if operations.is_empty() {
                entries.remove(&user);
            }
        }
        true
    }
}

pub(crate) struct Operation {
    config: Arc<AppConfig>,
    user: u64,
    pending: Option<(u64, Receipt)>,
    target: Target,
    generation: String,
}
impl Operation {
    fn release(&mut self, outcome: Outcome) -> Option<(u64, Receipt)> {
        let (id, receipt) = self.pending.take()?;
        // Only a widget renders saved results. A chat receipt is sent once.
        let saved_outcome = receipt.widget.then_some(outcome);
        if !self
            .config
            .html_ui
            .media
            .finish(self.user, id, saved_outcome)
        {
            return None;
        }
        if let Target::All { session, .. } = self.target {
            self.config.finish_recording_send(self.user, session);
        }
        Some((id, receipt))
    }
    pub async fn complete(mut self, outcome: Outcome) {
        if let Some((id, receipt)) = self.release(outcome) {
            present_receipt(
                &self.config,
                self.user,
                id,
                &self.generation,
                &self.target,
                receipt,
                outcome,
            )
            .await;
        }
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        // Cancellation is not proof that a Telegram upload failed. Never retry it.
        let outcome = Outcome {
            uncertain: true,
            ..Outcome::failed()
        };
        if let Some((id, receipt)) = self
            .release(outcome)
            .filter(|_| tokio::runtime::Handle::try_current().is_ok())
        {
            let (config, user, generation, target) = (
                self.config.clone(),
                self.user,
                self.generation.clone(),
                self.target.clone(),
            );
            db::user::spawn_with_admission(async move {
                present_receipt(&config, user, id, &generation, &target, receipt, outcome).await;
            });
        }
    }
}

fn matching_view(config: &AppConfig, user: u64, target: &Target) -> Option<(MessageId, String)> {
    config.sessions.get(&user).and_then(|session| {
        let payload = Payload::from_string(&session.current_context).ok()?;
        target.matches_view(&payload).then(|| {
            (
                MessageId(session.last_menu_id),
                session.current_context.clone(),
            )
        })
    })
}

async fn present_receipt(
    config: &Arc<AppConfig>,
    user: u64,
    id: u64,
    generation: &str,
    target: &Target,
    mut receipt: Receipt,
    outcome: Outcome,
) {
    // Wait for the initiating callback to settle its screen and release its
    // update token before deciding which presentation owns the result.
    if let Some(rendered) = receipt.rendered.take() {
        let _ = rendered.await;
    }
    if !receipt.widget {
        send_chat_receipt(config, user, generation, target, &receipt.bot, outcome).await;
        return;
    }
    // A queued refresh may be skipped while a callback owns the UI. Keep
    // requesting a coalesced read until this exact receipt was displayed.
    for _ in 0..120 {
        if config.html_ui.media.is_shown(user, id)
            || config
                .runtime_status
                .read()
                .await
                .shutdown_requested_at
                .is_some()
            || !db::admission_is_current(user, generation, &config.db)
                .await
                .unwrap_or(false)
        {
            return;
        }
        let uses_native_message = config
            .sessions
            .get(&user)
            .is_some_and(|session| session.ui_message_mode != UiMessageMode::Html);
        if uses_native_message || !super::preferred(config, user).await {
            if config.html_ui.media.finish(user, id, None) {
                send_chat_receipt(config, user, generation, target, &receipt.bot, outcome).await;
            }
            return;
        }
        if matching_view(config, user, target).is_none() {
            return;
        }
        let _ = crate::core::enqueue_current_refresh(config, user).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn send_chat_receipt(
    config: &Arc<AppConfig>,
    user: u64,
    generation: &str,
    target: &Target,
    bot: &Bot,
    outcome: Outcome,
) {
    let lang = db::get_user_language(user, &config.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(config.default_language);
    if !db::cameras::get_accessible_camera(
        user,
        user == config.root_user,
        target.camera(),
        &config.db,
    )
    .await
    .is_ok_and(|camera| camera.is_some())
        || !db::admission_is_current(user, generation, &config.db)
            .await
            .unwrap_or(false)
    {
        return;
    }
    if !outcome.reported_in_chat
        && bot
            .send_message(ChatId(user as i64), outcome.text(lang == Language::En))
            .await
            .is_err()
    {
        log::warn!("operation=video_receipt user={} result=failed", user);
    }
    // The Classic archive temporarily hides Send all. Rebuild that keyboard
    // once if it is still open, leaving any newer screen in place.
    if matches!(target, Target::All { .. }) {
        if let Some((message, context)) = matching_view(config, user, target) {
            let _ = crate::bot::handlers::refresh_current_view(
                bot,
                config,
                user,
                ChatId(user as i64),
                message,
                &context,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget_receipt() -> Receipt {
        Receipt {
            bot: Bot::new("123:test"),
            widget: true,
            rendered: None,
        }
    }

    #[tokio::test]
    async fn media_guard_survives_ui_reset_and_blocks_overlapping_archive_work() {
        let config = crate::test_support::config().await;
        let segment = Target::Segment {
            camera: 1,
            session: 10,
            segment: 100,
        };
        let all = Target::All {
            camera: 1,
            session: 10,
        };
        let first = config
            .html_ui
            .media
            .begin(&config, 1, "g1", segment.clone(), widget_receipt())
            .unwrap();
        config.html_ui.clear(1);
        assert!(config.html_ui.media.blocks(1, "g1", &segment));
        assert!(config
            .html_ui
            .media
            .begin(&config, 1, "g1", all.clone(), widget_receipt())
            .is_none());
        let independent = config
            .html_ui
            .media
            .begin(
                &config,
                1,
                "g1",
                Target::Segment {
                    camera: 1,
                    session: 10,
                    segment: 101,
                },
                widget_receipt(),
            )
            .unwrap();
        drop(first);
        drop(independent);
        let all_guard = config
            .html_ui
            .media
            .begin(&config, 1, "g1", all.clone(), widget_receipt())
            .unwrap();
        assert!(config.is_recording_send_in_progress(1, 10));
        assert!(config
            .html_ui
            .media
            .begin(&config, 1, "g1", segment, widget_receipt())
            .is_none());
        drop(all_guard);
        assert!(!config.is_recording_send_in_progress(1, 10));
        assert!(!config.html_ui.media.blocks(1, "g1", &all));
    }

    #[tokio::test]
    async fn old_media_completion_cannot_release_new_admission_or_new_operation() {
        let config = crate::test_support::config().await;
        let target = Target::All {
            camera: 1,
            session: 10,
        };
        let old = config
            .html_ui
            .media
            .begin(&config, 1, "old", target.clone(), widget_receipt())
            .unwrap();
        let current = config
            .html_ui
            .media
            .begin(&config, 1, "new", target.clone(), widget_receipt())
            .unwrap();
        drop(old);
        assert!(config.is_recording_send_in_progress(1, 10));
        assert!(config.html_ui.media.blocks(1, "new", &target));
        drop(current);
        assert!(!config.is_recording_send_in_progress(1, 10));
    }

    #[tokio::test]
    async fn video_receipts_are_scoped_and_distinguish_failed_partial_unknown_delivery() {
        let config = crate::test_support::config().await;
        for outcome in [
            Outcome::failed(),
            Outcome {
                sent: 2,
                failed: 1,
                uncertain: false,
                reported_in_chat: false,
            },
            Outcome {
                sent: 1,
                failed: 1,
                uncertain: true,
                reported_in_chat: false,
            },
        ] {
            let target = Target::All {
                camera: 1,
                session: 10,
            };
            let mut operation = config
                .html_ui
                .media
                .begin(&config, 1, "g", target.clone(), widget_receipt())
                .unwrap();
            let _ = operation.release(outcome);
            let mut view = View {
                payload: Payload::Camera(CameraPayload::RecordingSession {
                    camera: 1,
                    session: 10,
                }),
                ..Default::default()
            };
            let _ = config.html_ui.media.decorate(1, "g", &mut view);
            assert!(view.alert.is_some());
            assert!(view.notice.is_none());
            let mut home = View {
                payload: Payload::Home,
                ..Default::default()
            };
            let _ = config.html_ui.media.decorate(1, "g", &mut home);
            assert!(home.alert.is_none());
            assert!(!config.html_ui.media.blocks(1, "g", &target));
        }
    }

    #[tokio::test]
    async fn an_older_pending_render_cannot_acknowledge_the_completion_receipt() {
        let config = crate::test_support::config().await;
        let mut operation = config
            .html_ui
            .media
            .begin(
                &config,
                1,
                "g",
                Target::Clip {
                    camera: 1,
                    seconds: 10,
                },
                widget_receipt(),
            )
            .unwrap();
        let id = operation.pending.as_ref().unwrap().0;
        let mut view = View {
            payload: Payload::Camera(CameraPayload::CameraDetail { id: 1 }),
            ..Default::default()
        };
        let pending = config.html_ui.media.decorate(1, "g", &mut view);
        assert_eq!(pending, None);
        let _ = operation.release(Outcome::sent());
        config.html_ui.media.mark_shown(1, pending);
        assert!(!config.html_ui.media.is_shown(1, id));
        let receipt = config.html_ui.media.decorate(1, "g", &mut view);
        assert_eq!(receipt, Some(id));
        config.html_ui.media.mark_shown(1, receipt);
        assert!(config.html_ui.media.is_shown(1, id));
    }

    #[test]
    fn video_controls_share_guard_for_durations_and_admin_but_not_snapshot() {
        let clip =
            Target::from_payload(&Payload::Camera(CameraPayload::Clip { id: 2, seconds: 5 }))
                .unwrap();
        let admin = Target::from_payload(&Payload::Admin(AdminPayload::RoomCameraClip {
            room: 1,
            camera: 2,
            seconds: 30,
        }))
        .unwrap();
        assert!(clip.conflicts(&admin));
        assert!(
            Target::from_payload(&Payload::Camera(CameraPayload::Snapshot { id: 2 })).is_none()
        );
        assert!(!clip.matches_view(&Payload::Home));
    }
}
