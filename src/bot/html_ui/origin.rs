//! Bounded, admission-scoped origins for ordinary HTML screens.
use super::{presentation, WidgetSession};
use crate::bot::{
    models::View,
    personal::PersonalPayload as Personal,
    router::{ControlPayload as Control, Payload},
};
use teloxide::types::InlineKeyboardButtonKind;

const LIMIT: usize = 24;

#[derive(Clone, Default)]
pub(super) struct Trail {
    routes: Vec<Payload>,
}

/// A write may resolve to a read route, but the write itself is never a parent.
/// Media and chart routes deliberately retain their existing navigation contract.
pub(super) fn read_route(payload: &Payload) -> Option<Payload> {
    if matches!(payload, Payload::Control(Control::QuickAction { cmd, .. })
        if !crate::bot::router::action_requires_control(cmd))
    {
        return None;
    }
    let route = payload.for_refresh();
    matches!(
        &route,
        Payload::Home
            | Payload::Help
            | Payload::HelpTopic(_)
            | Payload::DeviceDetails { .. }
            | Payload::TemperatureInput { .. }
            | Payload::GroupDevices { .. }
            | Payload::Control(
                Control::ListRooms
                    | Control::RoomsPage { .. }
                    | Control::RoomDetail { .. }
                    | Control::RoomPage { .. }
                    | Control::DeviceControl { .. }
                    | Control::ActionGroups
                    | Control::ActionGroupsPage { .. }
                    | Control::ActionGroupDetail { .. }
                    | Control::HaNativeActionDetail { .. }
            )
            | Payload::Personal(
                Personal::Settings
                    | Personal::Interface
                    | Personal::QuietInput
                    | Personal::Favorites { .. }
                    | Personal::Notifications { .. }
                    | Personal::Subscription { .. }
                    | Personal::FavoriteOptions { .. }
            )
    )
    .then_some(route)
}

fn same_screen(a: &Payload, b: &Payload) -> bool {
    presentation::list_identity(a) == presentation::list_identity(b)
}

pub(super) fn prepare(old: Option<&WidgetSession>, generation: &str, view: &mut View) -> Trail {
    if view.html_chart.is_some()
        || view.html_range.is_some()
        || view.html_temperature.is_some()
        || view.html_media_controls
    {
        return Trail::default();
    }
    let old = old.filter(|s| s.generation == generation);
    if matches!(view.payload, Payload::Control(Control::ReviewAction { .. })) {
        let trail = old.map(|s| s.origin.clone()).unwrap_or_default();
        // Active confirmations retain their real Cancel callback. A completed
        // receipt only needs a read-only return to the original screen.
        if view.html_kind.as_deref() != Some("confirmation")
            && crate::bot::screen_structure::parent(view)
                .is_some_and(|p| !matches!(p, Payload::Home))
        {
            if let Some(parent) = trail
                .routes
                .iter()
                .rev()
                .find(|p| !matches!(p, Payload::TemperatureInput { .. }))
            {
                set_parent(view, parent.clone());
            }
        }
        return trail;
    }
    let Some(mut route) = read_route(&view.payload) else {
        return Trail::default();
    };
    if matches!(route, Payload::Home) {
        return Trail::default();
    }
    // Some source renderers normalize page routes to their base payload. Retain
    // the requested page (or the current one during Read) as the return target.
    if let Some(requested) = old
        .and_then(|s| s.origin_request.as_ref())
        .filter(|p| same_screen(p, &route))
    {
        route = requested.clone();
    } else if let Some(current) = old
        .and_then(|s| s.origin.routes.last())
        .filter(|p| same_screen(p, &route))
    {
        route = current.clone();
    }
    let mut trail = old.map(|s| s.origin.clone()).unwrap_or_default();
    if !trail.routes.iter().any(|p| same_screen(p, &route))
        && old
            .and_then(|s| crate::bot::screen_structure::parent(&s.view))
            .is_some_and(|parent| same_screen(&parent, &route))
    {
        // A direct/deep entry has no recorded ancestors. Its explicit Back is
        // still an upward transition, never a new child of the screen we left.
        return Trail {
            routes: vec![route],
        };
    }
    if trail.routes.is_empty() {
        if let Some(previous) = old
            .filter(|s| {
                !s.view.html_media_controls
                    && s.view.html_chart.is_none()
                    && s.view.html_range.is_none()
            })
            .and_then(|s| read_route(&s.view.payload))
        {
            trail.routes.push(previous);
        }
    }
    if let Some(index) = trail.routes.iter().rposition(|p| same_screen(p, &route)) {
        trail.routes.truncate(index + 1);
        trail.routes[index] = route;
    } else {
        trail.routes.push(route);
    }
    if trail.routes.len() > LIMIT {
        trail.routes.drain(..trail.routes.len() - LIMIT);
    }
    if trail.routes.len() >= 2 {
        set_parent(view, trail.routes[trail.routes.len() - 2].clone());
    }
    trail
}

fn set_parent(view: &mut View, parent: Payload) {
    let original = crate::bot::screen_structure::parent(view);
    // Preserve an explicit Home action. Replace only the original parent
    // footer, avoiding a second visible "Back" that still targets the room.
    if let Some(original) = original.filter(|p| !matches!(p, Payload::Home) && p != &parent) {
        if let Some(button) = view
            .kb
            .inline_keyboard
            .iter_mut()
            .flatten()
            .rev()
            .find(|b| {
                matches!(&b.kind, InlineKeyboardButtonKind::CallbackData(data)
                if Payload::from_string(data).ok().as_ref() == Some(&original))
            })
        {
            button.kind = InlineKeyboardButtonKind::CallbackData(parent.to_string());
        }
    }
    view.html_back = Some(parent);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::router::{CameraPayload, DeviceCmd};

    #[test]
    fn trail_is_bounded_and_home_clears_it() {
        let mut old = None;
        for device in 0..40 {
            let mut view = View {
                payload: Payload::Control(Control::DeviceControl { room: 1, device }),
                ..Default::default()
            };
            let origin = prepare(old.as_ref(), "g", &mut view);
            assert!(origin.routes.len() <= LIMIT);
            old = Some(WidgetSession {
                message: 1,
                generation: "g".into(),
                view,
                pages: vec![],
                actions: Default::default(),
                index: 0,
                claimed: false,
                anchor: None,
                origin,
                origin_request: None,
                list_navigation: None,
            });
        }
        assert_eq!(old.as_ref().unwrap().origin.routes.len(), LIMIT);
        let mut home = View {
            payload: Payload::Home,
            ..Default::default()
        };
        assert!(prepare(old.as_ref(), "g", &mut home).routes.is_empty());
    }

    #[test]
    fn origins_only_store_safe_read_routes() {
        let write = Payload::Control(Control::QuickAction {
            room: 1,
            device: 2,
            cmd: DeviceCmd::TurnOn,
        });
        assert_eq!(
            read_route(&write),
            Some(Payload::Control(Control::RoomDetail { room: 1 }))
        );
        assert_eq!(
            read_route(&Payload::Personal(Personal::SetSubscription {
                device: 2,
                enabled: true
            })),
            Some(Payload::Personal(Personal::Subscription { device: 2 }))
        );
        assert!(read_route(&Payload::Camera(CameraPayload::Clip { id: 1, seconds: 5 })).is_none());
        assert!(read_route(&Payload::Control(Control::QuickAction {
            room: 1,
            device: 2,
            cmd: DeviceCmd::OpenChartRange
        }))
        .is_none());
    }
}
