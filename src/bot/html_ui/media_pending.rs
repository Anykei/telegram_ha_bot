//! Video delivery state belongs to a worker, not a widget revision or HA entity.
use crate::{
    bot::{
        models::View,
        router::{AdminPayload, CameraPayload, Payload},
    },
    db,
    i18n::Language,
    models::AppConfig,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

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
            id: Some(id),
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
    fn finish(&self, user: u64, id: u64, outcome: Outcome) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let Some(entry) = entries
            .get_mut(&user)
            .and_then(|e| e.iter_mut().find(|e| e.id == id))
        else {
            return false;
        };
        entry.outcome = Some(outcome);
        true
    }
}

pub(crate) struct Operation {
    config: Arc<AppConfig>,
    user: u64,
    id: Option<u64>,
    target: Target,
    generation: String,
}
impl Operation {
    fn release(&mut self, outcome: Outcome) {
        if let Some(id) = self.id.take() {
            // An old admission's completion must not release a newer send-all guard.
            if self.config.html_ui.media.finish(self.user, id, outcome) {
                if let Target::All { session, .. } = self.target {
                    self.config.finish_recording_send(self.user, session);
                }
            }
        }
    }
    pub async fn complete(mut self, outcome: Outcome) {
        let Some(id) = self.id else {
            return;
        };
        self.release(outcome);
        // A queued refresh may be skipped while a callback owns the UI. Keep
        // requesting a coalesced read until this exact receipt was displayed.
        for _ in 0..120 {
            if self.config.html_ui.media.is_shown(self.user, id) {
                return;
            }
            if self
                .config
                .runtime_status
                .read()
                .await
                .shutdown_requested_at
                .is_some()
            {
                return;
            }
            if !db::admission_is_current(self.user, &self.generation, &self.config.db)
                .await
                .unwrap_or(false)
            {
                return;
            }
            let compatible = self
                .config
                .sessions
                .get(&self.user)
                .and_then(|s| Payload::from_string(&s.current_context).ok())
                .is_some_and(|p| self.target.matches_view(&p));
            if !compatible {
                return;
            }
            let _ = crate::core::enqueue_current_refresh(&self.config, self.user).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        // Cancellation is not proof that a Telegram upload failed. Never retry it.
        self.release(Outcome {
            uncertain: true,
            ..Outcome::failed()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .begin(&config, 1, "g1", segment.clone())
            .unwrap();
        config.html_ui.clear(1);
        assert!(config.html_ui.media.blocks(1, "g1", &segment));
        assert!(config
            .html_ui
            .media
            .begin(&config, 1, "g1", all.clone())
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
            )
            .unwrap();
        drop(first);
        drop(independent);
        let all_guard = config
            .html_ui
            .media
            .begin(&config, 1, "g1", all.clone())
            .unwrap();
        assert!(config.is_recording_send_in_progress(1, 10));
        assert!(config
            .html_ui
            .media
            .begin(&config, 1, "g1", segment)
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
            .begin(&config, 1, "old", target.clone())
            .unwrap();
        let current = config
            .html_ui
            .media
            .begin(&config, 1, "new", target.clone())
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
            },
            Outcome {
                sent: 1,
                failed: 1,
                uncertain: true,
            },
        ] {
            let target = Target::All {
                camera: 1,
                session: 10,
            };
            let mut operation = config
                .html_ui
                .media
                .begin(&config, 1, "g", target.clone())
                .unwrap();
            operation.release(outcome);
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
            )
            .unwrap();
        let id = operation.id.unwrap();
        let mut view = View {
            payload: Payload::Camera(CameraPayload::CameraDetail { id: 1 }),
            ..Default::default()
        };
        let pending = config.html_ui.media.decorate(1, "g", &mut view);
        assert_eq!(pending, None);
        operation.release(Outcome::sent());
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
