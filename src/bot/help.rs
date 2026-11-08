//! Short, task-oriented help shared by classic menus and HTML widgets.
use super::{
    models::View,
    personal::{words, PersonalPayload},
    router::{CameraPayload, ControlPayload, Payload, RenderContext},
};
use crate::{db, i18n::Language};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use teloxide::types::{InlineKeyboardButton as Button, InlineKeyboardMarkup};

// Append only: topic indices are stored in Telegram callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Topic {
    Controls,
    Charts,
    Cameras,
    Favorites,
    Notifications,
    Input,
    Navigation,
    Troubleshooting,
}

impl Topic {
    pub const ALL: [Self; 8] = [
        Self::Controls,
        Self::Charts,
        Self::Cameras,
        Self::Favorites,
        Self::Notifications,
        Self::Input,
        Self::Navigation,
        Self::Troubleshooting,
    ];

    pub fn title(self, lang: Language) -> &'static str {
        let (ru, en) = match self {
            Self::Controls => ("Управление", "Controls"),
            Self::Charts => ("Графики", "Charts"),
            Self::Cameras => ("Камеры", "Cameras"),
            Self::Favorites => ("Избранное", "Favorites"),
            Self::Notifications => ("Уведомления", "Notifications"),
            Self::Input => ("Текст и голос", "Text and voice"),
            Self::Navigation => ("Навигация", "Navigation"),
            Self::Troubleshooting => ("Неполадки", "Problems"),
        };
        words(lang, ru, en)
    }

    fn text(self, lang: Language) -> &'static str {
        let (ru, en) = match self {
            Self::Controls => (
                "Комнаты → устройство: нажмите нужное действие. В HTML кнопка заблокирована, пока команда применяется. Не повторяйте её при ожидании.\n\n«Сценарии» запускают группы действий и сценарии HA. Если появилось подтверждение, проверьте действие перед запуском.",
                "Rooms → device: select an action. In HTML, its button is locked while the change is pending; do not repeat the command.\n\nScenarios runs action groups and HA scripts/scenes. If confirmation appears, review the action before running it.",
            ),
            Self::Charts => (
                "Откройте датчик в комнате. В HTML «Период» позволяет выбрать даты и время начала и конца. Примените интервал, чтобы обновить график.\n\n«Отправить PNG» пришлёт отдельное изображение: его можно увеличить, сохранить или переслать.",
                "Open a sensor in a room. In HTML, Period lets you choose start/end dates and times. Apply the range to update the chart.\n\nSend PNG delivers a separate image you can zoom, save or forward.",
            ),
            Self::Cameras => (
                "«Снимок» отправит фото. Для видео выберите длительность и дождитесь завершения: кнопка временно заблокирована.\n\nФото и видео приходят отдельно. Видео по кнопке сохраняется в архив, если это включено в настройках камеры. Срок удаления сообщения задаёт администратор.",
                "Snapshot sends a photo. For video, choose a duration and wait; the button is temporarily locked.\n\nMedia arrives separately. Manual clips enter the archive if enabled in camera settings. The administrator sets the message deletion delay.",
            ),
            Self::Favorites => (
                "В карточке устройства, камеры или сценария выберите «В избранное». Можно сохранить переход к объекту или доступное действие.\n\nОткройте «Избранное», чтобы поставить пункт первым или удалить его. Избранное действие выполняется по нажатию.",
                "Select Add to favorites on a device, camera or scenario. Save a link to the object or an available action.\n\nOpen Favorites to move an entry to the top or remove it. Tapping a favorite action executes it.",
            ),
            Self::Notifications => (
                "В карточке устройства включите уведомления. «Мои настройки» → «Мои уведомления» — ваши подписки.\n\nВ «Моих настройках» задайте интервал тихих часов и включите их отдельно. Критические уведомления приходят и в тихие часы.",
                "Enable notifications on a device card. My settings → My notifications lists your subscriptions.\n\nIn My settings, set the quiet hours interval and enable it separately. Critical notifications still arrive during quiet hours.",
            ),
            Self::Input => (
                "Напишите: «включи свет в кухне». Укажите комнату и устройство; если найдено несколько вариантов, выберите нужный.\n\n/cancel отменяет ввод текущего поля, сохраняя поля черновика. Уже принятую команду он не отменяет.",
                "Type “turn on kitchen light”. Include the room and device; choose a match if several are found.\n\n/cancel cancels the current input and keeps saved draft fields. It does not undo an accepted command.",
            ),
            Self::Navigation => (
                "«Назад» — предыдущий уровень; ↻ — обновить данные. Стрелки снизу листают содержимое.\n\n/start — главное меню\n/help — помощь\n/ui — выбор интерфейса\n/html — HTML-карточки\n/classic — кнопки Telegram\n/cancel — отмена ввода",
                "Back returns to the previous level; ↻ refreshes data. Bottom arrows page through content.\n\n/start — home\n/help — help\n/ui — choose interface\n/html — HTML cards\n/classic — Telegram buttons\n/cancel — cancel input",
            ),
            Self::Troubleshooting => (
                "Нет ответа? Обновите карточку и проверьте состояние перед повторной командой. /start откроет главное меню заново.\n\nНет устройства или управления? Обратитесь к администратору: доступ может быть ограничен, а устройство — недоступно в HA.",
                "No response? Refresh and check the state before repeating a command. /start reopens home.\n\nMissing a device or controls? Ask the administrator: access may be restricted or the device unavailable in HA.",
            ),
        };
        words(lang, ru, en)
    }

    fn destination(self, lang: Language) -> Option<(&'static str, Payload)> {
        Some(match self {
            Self::Controls | Self::Charts => (
                words(lang, "Открыть комнаты", "Open rooms"),
                Payload::Control(ControlPayload::ListRooms),
            ),
            Self::Cameras => (
                words(lang, "Открыть камеры", "Open cameras"),
                Payload::Camera(CameraPayload::ListCameras),
            ),
            Self::Favorites => (
                words(lang, "Открыть избранное", "Open favorites"),
                Payload::Personal(PersonalPayload::Favorites { page: 0 }),
            ),
            Self::Notifications => (
                words(lang, "Мои настройки", "My settings"),
                Payload::Personal(PersonalPayload::Settings),
            ),
            Self::Navigation => (
                words(lang, "Вид интерфейса", "Interface"),
                Payload::Personal(PersonalPayload::Interface),
            ),
            Self::Input | Self::Troubleshooting => return None,
        })
    }
}

pub async fn render(ctx: RenderContext, topic: Option<Topic>) -> Result<View> {
    let title = topic
        .map(|t| t.title(ctx.lang))
        .unwrap_or(words(ctx.lang, "Помощь", "Help"));
    let parent = if topic.is_some() {
        Payload::Help
    } else {
        Payload::Home
    };
    let mut text = topic
        .map(|t| t.text(ctx.lang))
        .unwrap_or(words(
            ctx.lang,
            "Выберите тему, чтобы узнать, как пользоваться ботом.",
            "Choose a topic to learn how to use the bot.",
        ))
        .to_owned();
    if topic == Some(Topic::Input) {
        let voice = ctx.config.voice_enabled
            && db::access::can_use_voice(ctx.user_id, ctx.is_admin, &ctx.config.db).await?;
        text.push_str(words(
            ctx.lang,
            if voice {
                "\n\nТакже можно отправить голосовое сообщение."
            } else {
                "\n\nГолосовой ввод вам недоступен."
            },
            if voice {
                "\n\nYou can also send a voice message."
            } else {
                "\n\nVoice input is unavailable to you."
            },
        ));
    }
    let mut rows = if let Some(topic) = topic {
        topic
            .destination(ctx.lang)
            .map(|(label, route)| vec![vec![Button::callback(label, route.to_string())]])
            .unwrap_or_default()
    } else {
        Topic::ALL
            .chunks(2)
            .map(|topics| {
                topics
                    .iter()
                    .map(|t| {
                        Button::callback(t.title(ctx.lang), Payload::HelpTopic(*t).to_string())
                    })
                    .collect()
            })
            .collect()
    };
    rows.push(vec![super::screens::common::back_button_lang(
        ctx.lang,
        parent.clone(),
    )]);
    Ok(View {
        header: Some(title.into()),
        html_title: Some(title.into()),
        html_context: Some(
            words(
                ctx.lang,
                if topic.is_some() {
                    "Помощь"
                } else {
                    "Мой дом"
                },
                if topic.is_some() { "Help" } else { "My home" },
            )
            .into(),
        ),
        html_back: Some(parent),
        text,
        payload: topic.map(Payload::HelpTopic).unwrap_or(Payload::Help),
        kb: InlineKeyboardMarkup::new(rows),
        lang: ctx.lang,
        compact: true,
        ..Default::default()
    })
}
