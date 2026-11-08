//! Navigation is based on routes, never on a translated button caption.
use super::{
    models::View,
    personal::{words, FavoriteAction, PersonalPayload},
    router::{AdminPayload as A, ControlPayload as C, Payload as P, SettingsPayload as S},
};
use teloxide::types::InlineKeyboardButtonKind;

fn callbacks(view: &View) -> impl DoubleEndedIterator<Item = P> + '_ {
    view.kb
        .inline_keyboard
        .iter()
        .flatten()
        .filter_map(|button| {
            let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                return None;
            };
            P::from_string(data).ok()
        })
}

pub(crate) fn parent(view: &View) -> Option<P> {
    if let Some(parent) = &view.html_back {
        return Some(parent.clone());
    }
    // These specialised screens are explicitly frozen by the migration plan.
    // Retain their existing footer contract instead of inferring a different
    // parent from attachment/favorite controls appended by the router.
    if view.html_media_controls
        || view.html_chart.is_some()
        || view.html_range.is_some()
        || matches!(view.payload, P::Camera(_))
    {
        return view
            .kb
            .inline_keyboard
            .iter()
            .flatten()
            .rev()
            .find_map(|button| {
                if button.text != crate::i18n::t(view.lang, "common.back") {
                    return None;
                }
                let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                    return None;
                };
                P::from_string(data).ok()
            });
    }
    let parent = match &view.payload {
        P::Home => return None,
        P::Help | P::InDev => P::Home,
        P::HelpTopic(_) => P::Help,
        P::TemperatureInput { room, device } | P::DeviceDetails { room, device } => {
            P::Control(C::DeviceControl {
                room: *room,
                device: *device,
            })
        }
        P::GroupDevices { group, .. } => P::Control(C::ActionGroupDetail { group: *group }),
        P::Control(
            C::ListRooms | C::RoomsPage { .. } | C::ActionGroups | C::ActionGroupsPage { .. },
        ) => P::Home,
        P::Control(C::RoomDetail { .. } | C::RoomPage { .. }) => P::Control(C::ListRooms),
        P::Control(C::DeviceControl { room, .. } | C::QuickAction { room, .. }) => {
            P::Control(C::RoomDetail { room: *room })
        }
        P::Control(C::ActionGroupDetail { .. } | C::HaNativeActionDetail { .. }) => {
            P::Control(C::ActionGroups)
        }
        P::Personal(p) => match p {
            PersonalPayload::Settings | PersonalPayload::Favorites { .. } => P::Home,
            PersonalPayload::FavoriteOptions { target } => match target {
                FavoriteAction::Device { room, device }
                | FavoriteAction::DeviceOn { room, device }
                | FavoriteAction::DeviceOff { room, device } => P::Control(C::DeviceControl {
                    room: *room,
                    device: *device,
                }),
                FavoriteAction::Camera { id } | FavoriteAction::Snapshot { id } => {
                    P::Camera(super::router::CameraPayload::CameraDetail { id: *id })
                }
                FavoriteAction::GroupOn { id } | FavoriteAction::GroupOff { id } => {
                    P::Control(C::ActionGroupDetail { group: *id })
                }
                FavoriteAction::Native { id } => {
                    P::Control(C::HaNativeActionDetail { action: *id })
                }
            },
            PersonalPayload::Subscription { .. } | PersonalPayload::SetSubscription { .. } => {
                P::Personal(PersonalPayload::Notifications { page: 0 })
            }
            _ => P::Personal(PersonalPayload::Settings),
        },
        P::Settings(p) | P::AdminSettings(p) => {
            let admin = matches!(view.payload, P::AdminSettings(_));
            let wrap = |p| {
                if admin {
                    P::AdminSettings(p)
                } else {
                    P::Settings(p)
                }
            };
            match p {
                S::ListRooms | S::RoomsPage { .. } => {
                    if admin {
                        P::Admin(A::ListActions)
                    } else {
                        P::Home
                    }
                }
                S::RoomDetail { .. } | S::RoomPage { .. } => wrap(S::ListRooms),
                S::DeviceDetail { room, .. } => wrap(S::RoomDetail { room: *room }),
                S::StateAliases { room, device } | S::EditName { room, device } => {
                    wrap(S::DeviceDetail {
                        room: *room,
                        device: *device,
                    })
                }
                S::EditStateAlias { room, device, .. }
                | S::StateAliasEditContext { room, device, .. } => wrap(S::StateAliases {
                    room: *room,
                    device: *device,
                }),
                _ => return footer_parent(view),
            }
        }
        P::Admin(p) => match p {
            A::ListActions => P::Home,
            A::ListUsers
            | A::ListUsersPage { .. }
            | A::CameraRooms
            | A::ActionGroups
            | A::ActionGroupsPage { .. }
            | A::RecordingRuleGroups
            | A::Status
            | A::UiBackground
            | A::ActivityLog { .. } => P::Admin(A::ListActions),
            A::UserProfile { .. } | A::PromptAddUser | A::PromptDeleteUser => {
                P::Admin(A::ListUsers)
            }
            A::UserRooms { id } | A::EditUserAlias { id } | A::ConfirmDeleteUser { id } => {
                P::Admin(A::UserProfile { id: *id })
            }
            A::UserRoomDevices { id, .. } => P::Admin(A::UserRooms { id: *id }),
            A::RoomCameras { .. } | A::RoomCamerasPage { .. } | A::RecordingRules { .. } => {
                P::Admin(A::CameraRooms)
            }
            A::RoomCameraDetail { room, .. } | A::PromptAddCamera { room } => {
                P::Admin(A::RoomCameras { room: *room })
            }
            A::CameraHealth { room, camera } | A::ConfirmDeleteCamera { room, camera } => {
                P::Admin(A::RoomCameraDetail {
                    room: *room,
                    camera: *camera,
                })
            }
            A::ActionGroupDetail { .. }
            | A::HaNativeActionDetail { .. }
            | A::PromptCreateActionGroup => P::Admin(A::ActionGroups),
            A::ActionGroupItems { group, .. }
            | A::PromptRenameActionGroup { group }
            | A::ConfirmDeleteActionGroup { group } => {
                P::Admin(A::ActionGroupDetail { group: *group })
            }
            A::ConfirmBackup => P::Admin(A::ListActions),
            _ => return footer_parent(view),
        },
        _ => return footer_parent(view),
    };
    Some(parent)
}

/// Older screen builders end their keyboard with parent/home. Decode only that
/// dedicated footer; never search content rows for a button with a matching label.
/// Complex forms can override it explicitly through html_back.
fn footer_parent(view: &View) -> Option<P> {
    let rows = &view.kb.inline_keyboard;
    for row in rows.iter().rev().take(2) {
        let payloads: Vec<_> = row
            .iter()
            .filter_map(|button| {
                let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                    return None;
                };
                P::from_string(data).ok()
            })
            .collect();
        if payloads.is_empty() {
            break;
        }
        if payloads.iter().all(|p| matches!(p, P::Home)) {
            continue;
        }
        // A navigation footer has a single parent, optionally followed by Home.
        if payloads.len() > 1 && !payloads.iter().any(|p| matches!(p, P::Home)) {
            break;
        }
        return payloads.into_iter().find(|p| {
            !matches!(p, P::Home) && p != &view.payload.for_refresh() && p == &p.for_refresh()
        });
    }
    callbacks(view).find(|p| matches!(p, P::Home))
}

pub(crate) fn apply(view: &mut View) {
    // The graph/calendar and media contracts stay unchanged in this migration.
    if view.html_chart.is_some() || view.html_range.is_some() || view.html_media_controls {
        return;
    }
    if view.html_back.is_none() {
        view.html_back = parent(view);
    }
    let w = |ru, en| words(view.lang, ru, en).to_string();
    if view.html_title.is_none() {
        view.html_title = view.header.clone().or_else(|| {
            Some(match &view.payload {
                P::Home => w("Мой дом", "My home"),
                P::Help => w("Помощь", "Help"),
                P::DeviceDetails { .. } => w("Сведения об устройстве", "Device details"),
                P::TemperatureInput { .. } => w("Задать температуру", "Set temperature"),
                P::GroupDevices { .. } => w("Устройства сценария", "Scene devices"),
                P::Personal(p) => match p {
                    PersonalPayload::Favorites { .. } => w("Избранное", "Favorites"),
                    PersonalPayload::FavoriteOptions { .. } => {
                        w("Добавить в избранное", "Add favorite")
                    }
                    PersonalPayload::Notifications { .. } => {
                        w("Мои уведомления", "My notifications")
                    }
                    PersonalPayload::Subscription { .. } => {
                        w("Подписка на устройство", "Device subscription")
                    }
                    PersonalPayload::QuietInput => w("Тихие часы", "Quiet hours"),
                    PersonalPayload::Interface => w("Вид интерфейса", "Interface"),
                    _ => w("Мои настройки", "My settings"),
                },
                P::Settings(p) | P::AdminSettings(p) => match p {
                    S::ListRooms | S::RoomsPage { .. } => {
                        w("Настройки устройств", "Device settings")
                    }
                    S::RoomDetail { .. } | S::RoomPage { .. } => {
                        w("Устройства комнаты", "Room devices")
                    }
                    S::StateAliases { .. } => w("Названия состояний", "State names"),
                    S::EditName { .. } => w("Имя устройства", "Device name"),
                    S::EditStateAlias { .. } | S::StateAliasEditContext { .. } => {
                        w("Название состояния", "State name")
                    }
                    _ => w("Параметры устройства", "Device settings"),
                },
                P::Admin(p) => match p {
                    A::ListActions => w("Администрирование", "Administration"),
                    A::ListUsers | A::ListUsersPage { .. } => w("Пользователи", "Users"),
                    A::UserProfile { .. } => w("Профиль пользователя", "User profile"),
                    A::UserRooms { .. } => w("Доступ к комнатам", "Room access"),
                    A::UserRoomDevices { .. } => w("Доступ к устройствам", "Device access"),
                    A::CameraRooms => w("Настройки камер", "Camera settings"),
                    A::RoomCameras { .. } | A::RoomCamerasPage { .. } => {
                        w("Камеры комнаты", "Room cameras")
                    }
                    A::Status => w("Диагностика", "Diagnostics"),
                    _ => w("Администрирование", "Administration"),
                },
                P::Schedule(_) => w("Расписание", "Schedule"),
                P::InDev => w("В разработке", "In development"),
                _ => w("Управление", "Controls"),
            })
        });
    }
    if view.html_context.is_none() {
        view.html_context = Some(match &view.payload {
            P::Admin(
                A::UserProfile { id } | A::UserRooms { id } | A::UserRoomDevices { id, .. },
            ) => format!("{} #{id}", w("Пользователь", "User")),
            P::Admin(_) | P::AdminSettings(_) => w("Администрирование", "Administration"),
            P::Personal(_) => w("Личные настройки", "Personal settings"),
            _ => String::new(),
        });
    }
    if matches!(view.payload, P::Admin(_) | P::AdminSettings(_)) {
        for button in view.kb.inline_keyboard.iter().flatten() {
            let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                continue;
            };
            let label = if matches!(
                P::from_string(data),
                Ok(P::Admin(A::InheritUserDeviceAccess { .. }))
            ) {
                Some(w(
                    "Наследовать права\nкомнаты / профиля",
                    "Inherit access\nroom / profile",
                ))
            } else if button.text.contains(": ") {
                // Keep the value (role, access, voice engine) visible on line 2
                // instead of clipping it after a long setting name.
                Some(button.text.replacen(": ", "\n", 1))
            } else {
                None
            };
            if let Some(label) = label {
                view.html_button_labels.entry(data.clone()).or_insert(label);
            }
        }
    }
    for button in view.kb.inline_keyboard.iter().flatten() {
        let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
            continue;
        };
        let label = match P::from_string(data).ok() {
            Some(P::Personal(PersonalPayload::SetSubscription { enabled, .. })) => {
                Some(if enabled {
                    w("Включить\nуведомления", "Enable\nnotifications")
                } else {
                    w("Выключить\nуведомления", "Disable\nnotifications")
                })
            }
            Some(P::Personal(PersonalPayload::FavoriteOptions { .. })) => {
                Some(w("☆ В\nизбранное", "☆ Add\nfavorite"))
            }
            Some(P::Personal(PersonalPayload::Settings)) if matches!(view.payload, P::Home) => {
                Some(w("Мои\nнастройки", "My\nsettings"))
            }
            _ => None,
        };
        if let Some(label) = label {
            view.html_button_labels.entry(data.clone()).or_insert(label);
        }
    }
}
