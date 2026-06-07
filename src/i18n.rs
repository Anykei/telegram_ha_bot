use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Ru,
    En,
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ru => "Русский",
            Self::En => "English",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Ru => Self::En,
            Self::En => Self::Ru,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "ru" | "russian" | "русский" => Some(Self::Ru),
            "en" | "english" => Some(Self::En),
            _ => None,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

pub fn t(lang: Language, key: &str) -> &str {
    match (lang, key) {
        (Language::Ru, "app.title") => "HA Telegram Bot",
        (Language::En, "app.title") => "HA Telegram Bot",

        (Language::Ru, "common.back") => "⬅️ Назад",
        (Language::En, "common.back") => "⬅️ Back",
        (Language::Ru, "common.main_menu") => "🏠 В главное меню",
        (Language::En, "common.main_menu") => "🏠 Main menu",
        (Language::Ru, "common.in_development") => "В разработке",
        (Language::En, "common.in_development") => "In development",
        (Language::Ru, "common.updated") => "Обновлено:",
        (Language::En, "common.updated") => "Updated:",
        (Language::Ru, "common.no") => "нет",
        (Language::En, "common.no") => "none",
        (Language::Ru, "common.notice") => "ГОТОВО:",
        (Language::En, "common.notice") => "DONE:",
        (Language::Ru, "common.error") => "ОШИБКА:",
        (Language::En, "common.error") => "ERROR:",
        (Language::Ru, "common.confirm") => "✅ Подтвердить",
        (Language::En, "common.confirm") => "✅ Confirm",
        (Language::Ru, "common.cancel") => "↩️ Отмена",
        (Language::En, "common.cancel") => "↩️ Cancel",
        (Language::Ru, "common.prev") => "◀️ Назад",
        (Language::En, "common.prev") => "◀️ Prev",
        (Language::Ru, "common.next") => "Далее ▶️",
        (Language::En, "common.next") => "Next ▶️",
        (Language::Ru, "common.done") => "Готово",
        (Language::En, "common.done") => "Done",
        (Language::Ru, "access.denied.text") => "Недостаточно прав для этого действия",
        (Language::En, "access.denied.text") => "Not enough permissions for this action",
        (Language::Ru, "access.denied.alert") => "Доступ ограничен профилем пользователя",
        (Language::En, "access.denied.alert") => "Access is restricted by the user profile",

        (Language::Ru, "home.title") => "Главное меню",
        (Language::En, "home.title") => "Main menu",
        (Language::Ru, "home.control") => "🏠 Управление",
        (Language::En, "home.control") => "🏠 Control",
        (Language::Ru, "home.cameras") => "📹 Камеры",
        (Language::En, "home.cameras") => "📹 Cameras",
        (Language::Ru, "home.settings") => "⚙️ Настройки",
        (Language::En, "home.settings") => "⚙️ Settings",
        (Language::Ru, "home.admin") => "🛠 Админка",
        (Language::En, "home.admin") => "🛠 Admin",

        (Language::Ru, "system.ok") => "Все спокойно",
        (Language::En, "system.ok") => "All quiet",
        (Language::Ru, "system.label") => "Система",
        (Language::En, "system.label") => "System",
        (Language::Ru, "system.shutdown.label") => "Система",
        (Language::En, "system.shutdown.label") => "System",
        (Language::Ru, "system.shutdown.value") => "Остановка сервиса",
        (Language::En, "system.shutdown.value") => "Service shutdown",
        (Language::Ru, "recording.header") => "Запись",
        (Language::En, "recording.header") => "Recording",
        (Language::Ru, "recording.active") => "идет",
        (Language::En, "recording.active") => "active",
        (Language::Ru, "recording.remaining") => "осталось",
        (Language::En, "recording.remaining") => "remaining",

        (Language::Ru, "admin.menu") => "Админ меню",
        (Language::En, "admin.menu") => "Admin menu",
        (Language::Ru, "admin.version") => "Версия бота",
        (Language::En, "admin.version") => "Bot version",
        (Language::Ru, "admin.users") => "👥 Список пользователей",
        (Language::En, "admin.users") => "👥 Users",
        (Language::Ru, "admin.status") => "📊 Статус системы",
        (Language::En, "admin.status") => "📊 System status",
        (Language::Ru, "admin.cameras") => "📹 Камеры",
        (Language::En, "admin.cameras") => "📹 Cameras",
        (Language::Ru, "admin.settings") => "⚙️ Настройки",
        (Language::En, "admin.settings") => "⚙️ Settings",
        (Language::Ru, "admin.add") => "➕ Добавить",
        (Language::En, "admin.add") => "➕ Add",
        (Language::Ru, "admin.delete") => "➖ Удалить",
        (Language::En, "admin.delete") => "➖ Delete",
        (Language::Ru, "admin.users.title") => "Пользователи с доступом",
        (Language::En, "admin.users.title") => "Users with access",
        (Language::Ru, "admin.user_profile.title") => "Профиль пользователя",
        (Language::En, "admin.user_profile.title") => "User profile",
        (Language::Ru, "admin.role") => "Роль",
        (Language::En, "admin.role") => "Role",
        (Language::Ru, "admin.rooms.label") => "Комнаты",
        (Language::En, "admin.rooms.label") => "Rooms",
        (Language::Ru, "admin.devices.label") => "Устройства",
        (Language::En, "admin.devices.label") => "Devices",
        (Language::Ru, "admin.notifications_off") => "Уведомления выключены",
        (Language::En, "admin.notifications_off") => "Notifications off",
        (Language::Ru, "admin.user_profile.root_note") => {
            "Root имеет полный доступ. Ограничения профиля не применяются."
        }
        (Language::En, "admin.user_profile.root_note") => {
            "Root has full access. Profile restrictions are ignored."
        }
        (Language::Ru, "admin.user_profile.role_hint") => {
            "Роли переключаются по кругу: user → child → guest. Для child/guest комнаты закрываются, нужные откройте вручную."
        }
        (Language::En, "admin.user_profile.role_hint") => {
            "Roles cycle as user → child → guest. For child/guest rooms are closed; open the needed ones manually."
        }
        (Language::Ru, "admin.change_role") => "🔁 Сменить роль",
        (Language::En, "admin.change_role") => "🔁 Change role",
        (Language::Ru, "admin.reset_access") => "♻️ Сбросить доступы",
        (Language::En, "admin.reset_access") => "♻️ Reset access",
        (Language::Ru, "admin.rooms") => "🏠 Комнаты",
        (Language::En, "admin.rooms") => "🏠 Rooms",
        (Language::Ru, "admin.delete_user") => "🗑 Удалить пользователя",
        (Language::En, "admin.delete_user") => "🗑 Delete user",
        (Language::Ru, "admin.language") => "🌐 Язык",
        (Language::En, "admin.language") => "🌐 Language",
        (Language::Ru, "admin.ui_background") => "🖼 Фон UI",
        (Language::En, "admin.ui_background") => "🖼 UI background",
        (Language::Ru, "admin.ui_background.title") => "Фон интерфейса",
        (Language::En, "admin.ui_background.title") => "Interface background",
        (Language::Ru, "admin.ui_background.current") => "Сейчас",
        (Language::En, "admin.ui_background.current") => "Current",
        (Language::Ru, "admin.ui_background.stock") => "стоковая картинка",
        (Language::En, "admin.ui_background.stock") => "stock image",
        (Language::Ru, "admin.ui_background.stock_button") => "🖼 Стоковая картинка",
        (Language::En, "admin.ui_background.stock_button") => "🖼 Stock image",
        (Language::Ru, "admin.ui_background.interval") => "⏱ Интервал",
        (Language::En, "admin.ui_background.interval") => "⏱ Interval",
        (Language::Ru, "admin.ui_background.hint") => {
            "Кадр обновляется не чаще одного раза в выбранный интервал. Если свежего кадра еще нет, бот показывает предыдущий кадр или стоковую картинку."
        }
        (Language::En, "admin.ui_background.hint") => {
            "The frame is refreshed no more often than the selected interval. If a fresh frame is not ready yet, the bot shows the previous frame or the stock image."
        }
        (Language::Ru, "admin.ui_background.updated") => "Фон интерфейса обновлен",
        (Language::En, "admin.ui_background.updated") => "Interface background updated",
        (Language::Ru, "admin.ui_background.camera_missing") => "Камера не найдена",
        (Language::En, "admin.ui_background.camera_missing") => "Camera not found",
        (Language::Ru, "admin.ui_background.interval_changed") => "Интервал обновления",
        (Language::En, "admin.ui_background.interval_changed") => "Refresh interval",
        (Language::Ru, "admin.activity_log") => "📜 Журнал",
        (Language::En, "admin.activity_log") => "📜 Activity",
        (Language::Ru, "admin.activity_log.title") => "Журнал активности",
        (Language::En, "admin.activity_log.title") => "Activity log",
        (Language::Ru, "admin.activity_log.filter") => "Фильтр",
        (Language::En, "admin.activity_log.filter") => "Filter",
        (Language::Ru, "admin.activity_log.empty") => "событий пока нет",
        (Language::En, "admin.activity_log.empty") => "no events yet",
        (Language::Ru, "admin.activity_log.all") => "Все",
        (Language::En, "admin.activity_log.all") => "All",
        (Language::Ru, "admin.activity_log.errors") => "Ошибки",
        (Language::En, "admin.activity_log.errors") => "Errors",
        (Language::Ru, "admin.activity_log.cameras") => "Камеры",
        (Language::En, "admin.activity_log.cameras") => "Cameras",
        (Language::Ru, "admin.activity_log.recording") => "Запись",
        (Language::En, "admin.activity_log.recording") => "Recording",
        (Language::Ru, "admin.activity_log.devices_short") => "Устр.",
        (Language::En, "admin.activity_log.devices_short") => "Devices",
        (Language::Ru, "admin.rule_groups") => "🧩 Группы правил",
        (Language::En, "admin.rule_groups") => "🧩 Rule groups",
        (Language::Ru, "admin.rule_groups.create_defaults") => "➕ Создать базовые",
        (Language::En, "admin.rule_groups.create_defaults") => "➕ Create defaults",
        (Language::Ru, "admin.rule_groups.create_custom") => "➕ Создать группу",
        (Language::En, "admin.rule_groups.create_custom") => "➕ Create group",
        (Language::Ru, "admin.rule_groups.empty") => {
            "Группы правил\n\nГрупп пока нет. Создайте свою группу или нажмите «Создать базовые»."
        }
        (Language::En, "admin.rule_groups.empty") => {
            "Rule groups\n\nThere are no groups yet. Create your own group or tap “Create defaults”."
        }
        (Language::Ru, "admin.rule_groups.hint") => {
            "Группы правил\n\nНажмите группу, чтобы открыть управление. В карточке правила можно добавить правило в группу."
        }
        (Language::En, "admin.rule_groups.hint") => {
            "Rule groups\n\nTap a group to manage it. A rule can be added to a group from the rule card."
        }
        (Language::Ru, "admin.rule_groups.rules_count") => "правил",
        (Language::En, "admin.rule_groups.rules_count") => "rules",
        (Language::Ru, "admin.rule_groups.rules") => "📋 Правила в группе",
        (Language::En, "admin.rule_groups.rules") => "📋 Rules in group",
        (Language::Ru, "admin.rule_groups.rules_title") => "Правила группы",
        (Language::En, "admin.rule_groups.rules_title") => "Group rules",
        (Language::Ru, "admin.rule_groups.rules_filter") => "Фильтр",
        (Language::En, "admin.rule_groups.rules_filter") => "Filter",
        (Language::Ru, "admin.rule_groups.selected_count") => "Выбрано",
        (Language::En, "admin.rule_groups.selected_count") => "Selected",
        (Language::Ru, "admin.rule_groups.page") => "Страница",
        (Language::En, "admin.rule_groups.page") => "Page",
        (Language::Ru, "admin.rule_groups.rules_hint") => {
            "Отметьте правила галочкой. Выключенные правила показаны с ⏸."
        }
        (Language::En, "admin.rule_groups.rules_hint") => {
            "Tick the rules to include them. Disabled rules are marked with ⏸."
        }
        (Language::Ru, "admin.rule_groups.rules_empty") => "Правил нет",
        (Language::En, "admin.rule_groups.rules_empty") => "No rules",
        (Language::Ru, "admin.rule_groups.filter_all") => "Все",
        (Language::En, "admin.rule_groups.filter_all") => "All",
        (Language::Ru, "admin.rule_groups.filter_selected") => "Выбранные",
        (Language::En, "admin.rule_groups.filter_selected") => "Selected",
        (Language::Ru, "admin.rule_groups.filter_unselected") => "Не выбранные",
        (Language::En, "admin.rule_groups.filter_unselected") => "Unselected",
        (Language::Ru, "admin.rule_groups.detail") => "Группа правил",
        (Language::En, "admin.rule_groups.detail") => "Rule group",
        (Language::Ru, "admin.rule_groups.name") => "Название",
        (Language::En, "admin.rule_groups.name") => "Name",
        (Language::Ru, "admin.rule_groups.current_name") => "Текущее название",
        (Language::En, "admin.rule_groups.current_name") => "Current name",
        (Language::Ru, "admin.rule_groups.status") => "Статус",
        (Language::En, "admin.rule_groups.status") => "Status",
        (Language::Ru, "admin.rule_groups.enabled") => "включена",
        (Language::En, "admin.rule_groups.enabled") => "enabled",
        (Language::Ru, "admin.rule_groups.paused") => "на паузе",
        (Language::En, "admin.rule_groups.paused") => "paused",
        (Language::Ru, "admin.rule_groups.enable") => "▶️ Включить",
        (Language::En, "admin.rule_groups.enable") => "▶️ Enable",
        (Language::Ru, "admin.rule_groups.pause") => "⏸ Пауза",
        (Language::En, "admin.rule_groups.pause") => "⏸ Pause",
        (Language::Ru, "admin.rule_groups.rename") => "✏️ Переименовать",
        (Language::En, "admin.rule_groups.rename") => "✏️ Rename",
        (Language::Ru, "admin.rule_groups.delete") => "🗑 Удалить",
        (Language::En, "admin.rule_groups.delete") => "🗑 Delete",
        (Language::Ru, "admin.rule_groups.detail_hint") => {
            "Удаление группы не удаляет правила записи, а только убирает связь с группой."
        }
        (Language::En, "admin.rule_groups.detail_hint") => {
            "Deleting a group does not delete recording rules; it only removes group links."
        }
        (Language::Ru, "admin.rule_groups.create_title") => "Создание группы правил",
        (Language::En, "admin.rule_groups.create_title") => "Create rule group",
        (Language::Ru, "admin.rule_groups.create_prompt") => {
            "Введите название группы. Например: Охрана, Ночь, Двери."
        }
        (Language::En, "admin.rule_groups.create_prompt") => {
            "Enter a group name. For example: Security, Night, Doors."
        }
        (Language::Ru, "admin.rule_groups.rename_title") => "Переименование группы",
        (Language::En, "admin.rule_groups.rename_title") => "Rename group",
        (Language::Ru, "admin.rule_groups.rename_prompt") => "Введите новое название группы.",
        (Language::En, "admin.rule_groups.rename_prompt") => "Enter a new group name.",
        (Language::Ru, "admin.rule_groups.not_found") => "Группа не найдена",
        (Language::En, "admin.rule_groups.not_found") => "Group not found",
        (Language::Ru, "admin.rule_groups.deleted") => "Группа удалена",
        (Language::En, "admin.rule_groups.deleted") => "Group deleted",
        (Language::Ru, "admin.rule_groups.enabled_notice") => "Группа включена",
        (Language::En, "admin.rule_groups.enabled_notice") => "Group enabled",
        (Language::Ru, "admin.rule_groups.paused_notice") => "Группа поставлена на паузу",
        (Language::En, "admin.rule_groups.paused_notice") => "Group paused",
        (Language::Ru, "admin.rule_groups.delete_title") => "Удаление группы правил",
        (Language::En, "admin.rule_groups.delete_title") => "Delete rule group",
        (Language::Ru, "admin.rule_groups.delete_confirm") => {
            "Удалить группу правил? Сами правила записи останутся, будет удалена только связь с группой."
        }
        (Language::En, "admin.rule_groups.delete_confirm") => {
            "Delete this rule group? Recording rules will remain; only group links will be removed."
        }
        (Language::Ru, "admin.rule_groups.delete_failed") => "Не удалось удалить группу",
        (Language::En, "admin.rule_groups.delete_failed") => "Failed to delete group",
        (Language::Ru, "admin.rule_groups.toggle_failed") => "Не удалось переключить группу",
        (Language::En, "admin.rule_groups.toggle_failed") => "Failed to toggle group",
        (Language::Ru, "admin.camera.health") => "🩺 Health",
        (Language::En, "admin.camera.health") => "🩺 Health",
        (Language::Ru, "admin.camera.health.check_now") => "🔄 Проверить сейчас",
        (Language::En, "admin.camera.health.check_now") => "🔄 Check now",
        (Language::Ru, "admin.camera.health.not_found") => "Камера не найдена",
        (Language::En, "admin.camera.health.not_found") => "Camera not found",
        (Language::Ru, "admin.camera.health.no_checks") => {
            "Проверок пока нет.\nНажмите «Проверить сейчас»."
        }
        (Language::En, "admin.camera.health.no_checks") => {
            "No checks yet.\nTap “Check now”."
        }
        (Language::Ru, "admin.camera.health.last_check") => "Последняя проверка",
        (Language::En, "admin.camera.health.last_check") => "Last check",
        (Language::Ru, "admin.camera.health.last_size") => "Последний размер",
        (Language::En, "admin.camera.health.last_size") => "Last size",
        (Language::Ru, "admin.camera.health.last_error") => "Последняя ошибка",
        (Language::En, "admin.camera.health.last_error") => "Last error",
        (Language::Ru, "admin.camera.health.check_started") => "Проверка камеры запущена",
        (Language::En, "admin.camera.health.check_started") => "Camera check started",
        (Language::Ru, "admin.rule_groups.defaults_created") => "Базовые группы созданы",
        (Language::En, "admin.rule_groups.defaults_created") => "Default groups created",
        (Language::Ru, "camera.title") => "📹 Камеры",
        (Language::En, "camera.title") => "📹 Cameras",
        (Language::Ru, "camera.list.empty") => "Камеры\n\nНет доступных камер.",
        (Language::En, "camera.list.empty") => "Cameras\n\nNo available cameras.",
        (Language::Ru, "camera.list.pick") => "Камеры\n\nВыберите камеру.",
        (Language::En, "camera.list.pick") => "Cameras\n\nChoose a camera.",
        (Language::Ru, "camera.detail.header") => "📹 Камера",
        (Language::En, "camera.detail.header") => "📹 Camera",
        (Language::Ru, "camera.snapshot") => "📸 Снимок",
        (Language::En, "camera.snapshot") => "📸 Snapshot",
        (Language::Ru, "camera.archive.button") => "🗂 Архив записей",
        (Language::En, "camera.archive.button") => "🗂 Recordings",
        (Language::Ru, "camera.stop_recording") => "⏹ Остановить запись",
        (Language::En, "camera.stop_recording") => "⏹ Stop recording",
        (Language::Ru, "camera.delete") => "🗑 Удалить",
        (Language::En, "camera.delete") => "🗑 Delete",
        (Language::Ru, "camera.detail.help") => {
            "Снимок отправляется сразу. Видео можно выбрать по длительности.\nДлительность по умолчанию для камеры"
        }
        (Language::En, "camera.detail.help") => {
            "Snapshot is sent immediately. Video duration can be selected.\nDefault camera duration"
        }
        (Language::Ru, "camera.recording.rule") => "правило",
        (Language::En, "camera.recording.rule") => "rule",
        (Language::Ru, "camera.recording.remaining") => "осталось",
        (Language::En, "camera.recording.remaining") => "remaining",
        (Language::Ru, "camera.archive.header") => "🗂 Архив записей",
        (Language::En, "camera.archive.header") => "🗂 Recordings",
        (Language::Ru, "camera.archive.empty") => "Записей пока нет.",
        (Language::En, "camera.archive.empty") => "No recordings yet.",
        (Language::Ru, "camera.archive.count") => "Записей",
        (Language::En, "camera.archive.count") => "Recordings",
        (Language::Ru, "camera.recording.header") => "🎞 Запись",
        (Language::En, "camera.recording.header") => "🎞 Recording",
        (Language::Ru, "camera.recording.send_video") => "🎞 Отправить видео",
        (Language::En, "camera.recording.send_video") => "🎞 Send video",
        (Language::Ru, "camera.recording.send_all") => "🎞 Отправить все части",
        (Language::En, "camera.recording.send_all") => "🎞 Send all parts",
        (Language::Ru, "camera.recording.camera") => "Камера",
        (Language::En, "camera.recording.camera") => "Camera",
        (Language::Ru, "camera.recording.event") => "Событие",
        (Language::En, "camera.recording.event") => "Event",
        (Language::Ru, "camera.recording.start") => "Начало",
        (Language::En, "camera.recording.start") => "Start",
        (Language::Ru, "camera.recording.end") => "Конец",
        (Language::En, "camera.recording.end") => "End",
        (Language::Ru, "camera.recording.duration") => "Длительность",
        (Language::En, "camera.recording.duration") => "Duration",
        (Language::Ru, "camera.recording.files") => "Файлов",
        (Language::En, "camera.recording.files") => "Files",
        (Language::Ru, "camera.recording.keep_until") => "Хранить до",
        (Language::En, "camera.recording.keep_until") => "Keep until",
        (Language::Ru, "camera.recording.failure_reason") => "Причина",
        (Language::En, "camera.recording.failure_reason") => "Reason",
        (Language::Ru, "camera.recording.partial_warning") => {
            "⚠️ Запись завершилась с ошибкой. Доступны готовые части."
        }
        (Language::En, "camera.recording.partial_warning") => {
            "⚠️ Recording finished with an error. Ready parts are available."
        }
        (Language::Ru, "camera.recording.failed_warning") => "⚠️ Запись не удалась.",
        (Language::En, "camera.recording.failed_warning") => "⚠️ Recording failed.",
        (Language::Ru, "camera.recording.active_warning") => "⏳ Запись еще идет.",
        (Language::En, "camera.recording.active_warning") => "⏳ Recording is still running.",
        (Language::Ru, "camera.recording.sending_warning") => "⏳ Отправка всех частей уже идет.",
        (Language::En, "camera.recording.sending_warning") => "⏳ Sending all parts is already running.",
        (Language::Ru, "camera.recording.delete.title") => "Удаление записи",
        (Language::En, "camera.recording.delete.title") => "Delete recording",
        (Language::Ru, "camera.recording.delete.confirm") => {
            "Удалить запись и все файлы сегментов?"
        }
        (Language::En, "camera.recording.delete.confirm") => {
            "Delete the recording and all segment files?"
        }
        (Language::Ru, "camera.recording.stop_notice") => {
            "Запись остановится после текущего сегмента"
        }
        (Language::En, "camera.recording.stop_notice") => {
            "Recording will stop after the current segment"
        }
        (Language::Ru, "camera.recording.deleted_notice") => "Запись удалена",
        (Language::En, "camera.recording.deleted_notice") => "Recording deleted",

        (Language::Ru, "lang.changed") => "Язык пользователя изменен",
        (Language::En, "lang.changed") => "User language changed",

        (_, _) => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_languages() {
        assert_eq!(Language::parse("ru"), Some(Language::Ru));
        assert_eq!(Language::parse("en"), Some(Language::En));
        assert_eq!(Language::parse("English"), Some(Language::En));
        assert_eq!(Language::parse("de"), None);
    }

    #[test]
    fn translates_known_key_and_falls_back_to_key() {
        assert_eq!(t(Language::En, "home.title"), "Main menu");
        assert_eq!(t(Language::Ru, "unknown.key"), "unknown.key");
    }
}
