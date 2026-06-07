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
        (Language::Ru, "home.action_groups") => "⚡ Группы действий",
        (Language::En, "home.action_groups") => "⚡ Action groups",
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
        (Language::Ru, "admin.action_groups") => "⚡ Группы действий",
        (Language::En, "admin.action_groups") => "⚡ Action groups",
        (Language::Ru, "action_groups.title") => "⚡ Группы действий",
        (Language::En, "action_groups.title") => "⚡ Action groups",
        (Language::Ru, "action_groups.title_plain") => "Группы действий",
        (Language::En, "action_groups.title_plain") => "Action groups",
        (Language::Ru, "action_groups.empty") => "Доступных быстрых действий пока нет.",
        (Language::En, "action_groups.empty") => "No quick actions are available yet.",
        (Language::Ru, "action_groups.sync_failed") => {
            "Не удалось обновить действия Home Assistant"
        }
        (Language::En, "action_groups.sync_failed") => {
            "Failed to refresh Home Assistant actions"
        }
        (Language::Ru, "action_groups.bot_groups") => "Группы бота",
        (Language::En, "action_groups.bot_groups") => "Bot groups",
        (Language::Ru, "action_groups.create_group") => "➕ Создать группу",
        (Language::En, "action_groups.create_group") => "➕ Create group",
        (Language::Ru, "action_groups.devices_count") => "устройств",
        (Language::En, "action_groups.devices_count") => "devices",
        (Language::Ru, "action_groups.not_found_suffix") => " · не найдено",
        (Language::En, "action_groups.not_found_suffix") => " · not found",
        (Language::Ru, "action_groups.group_not_found") => "Группа не найдена",
        (Language::En, "action_groups.group_not_found") => "Group not found",
        (Language::Ru, "action_groups.group_no_access") => "Недостаточно прав для этой группы",
        (Language::En, "action_groups.group_no_access") => "Not enough permissions for this group",
        (Language::Ru, "action_groups.group_paused") => "Группа на паузе",
        (Language::En, "action_groups.group_paused") => "Group is paused",
        (Language::Ru, "action_groups.group_deleted") => "Группа удалена",
        (Language::En, "action_groups.group_deleted") => "Group deleted",
        (Language::Ru, "action_groups.action_not_found") => "Действие не найдено",
        (Language::En, "action_groups.action_not_found") => "Action not found",
        (Language::Ru, "action_groups.action_no_access") => {
            "Недостаточно прав для этого действия"
        }
        (Language::En, "action_groups.action_no_access") => {
            "Not enough permissions for this action"
        }
        (Language::Ru, "action_groups.action_paused") => "Действие на паузе",
        (Language::En, "action_groups.action_paused") => "Action is paused",
        (Language::Ru, "action_groups.dynamic_turn_off") => "⏻ Выключить устройства",
        (Language::En, "action_groups.dynamic_turn_off") => "⏻ Turn devices off",
        (Language::Ru, "action_groups.dynamic_turn_on") => "▶️ Активировать устройства",
        (Language::En, "action_groups.dynamic_turn_on") => "▶️ Activate devices",
        (Language::Ru, "action_groups.turn_on_all") => "Включить все",
        (Language::En, "action_groups.turn_on_all") => "Turn all on",
        (Language::Ru, "action_groups.turn_off_all") => "Выключить все",
        (Language::En, "action_groups.turn_off_all") => "Turn all off",
        (Language::Ru, "action_groups.execute") => "▶️ Выполнить",
        (Language::En, "action_groups.execute") => "▶️ Run",
        (Language::Ru, "action_groups.devices_button") => "📋 Устройства",
        (Language::En, "action_groups.devices_button") => "📋 Devices",
        (Language::Ru, "action_groups.schedules_button") => "🕒 Расписания",
        (Language::En, "action_groups.schedules_button") => "🕒 Schedules",
        (Language::Ru, "action_groups.access_button") => "🌐 Доступ",
        (Language::En, "action_groups.access_button") => "🌐 Access",
        (Language::Ru, "action_groups.pause_group") => "⏸ Пауза группы",
        (Language::En, "action_groups.pause_group") => "⏸ Pause group",
        (Language::Ru, "action_groups.pause_action") => "⏸ Пауза действия",
        (Language::En, "action_groups.pause_action") => "⏸ Pause action",
        (Language::Ru, "action_groups.resume") => "▶️ Снять с паузы",
        (Language::En, "action_groups.resume") => "▶️ Resume",
        (Language::Ru, "action_groups.rename") => "✏️ Переименовать",
        (Language::En, "action_groups.rename") => "✏️ Rename",
        (Language::Ru, "action_groups.delete") => "🗑 Удалить",
        (Language::En, "action_groups.delete") => "🗑 Delete",
        (Language::Ru, "action_groups.alias") => "✏️ Алиас",
        (Language::En, "action_groups.alias") => "✏️ Alias",
        (Language::Ru, "action_groups.reset_alias") => "♻️ Сбросить",
        (Language::En, "action_groups.reset_alias") => "♻️ Reset",
        (Language::Ru, "action_groups.state") => "Статус",
        (Language::En, "action_groups.state") => "State",
        (Language::Ru, "action_groups.group_status") => "Группа",
        (Language::En, "action_groups.group_status") => "Group",
        (Language::Ru, "action_groups.access") => "Доступ",
        (Language::En, "action_groups.access") => "Access",
        (Language::Ru, "action_groups.devices") => "Устройств",
        (Language::En, "action_groups.devices") => "Devices",
        (Language::Ru, "action_groups.available_to_you") => "Доступно вам",
        (Language::En, "action_groups.available_to_you") => "Available to you",
        (Language::Ru, "action_groups.archived") => "архивировано",
        (Language::En, "action_groups.archived") => "archived",
        (Language::Ru, "action_groups.archived_suffix") => " · архивировано",
        (Language::En, "action_groups.archived_suffix") => " · archived",
        (Language::Ru, "action_groups.and_more") => "...и еще",
        (Language::En, "action_groups.and_more") => "...and",
        (Language::Ru, "action_groups.source") => "Источник",
        (Language::En, "action_groups.source") => "Source",
        (Language::Ru, "action_groups.type") => "Тип",
        (Language::En, "action_groups.type") => "Type",
        (Language::Ru, "action_groups.ha_name") => "Имя в HA",
        (Language::En, "action_groups.ha_name") => "HA name",
        (Language::Ru, "action_groups.bot_alias") => "Алиас в боте",
        (Language::En, "action_groups.bot_alias") => "Bot alias",
        (Language::Ru, "action_groups.status") => "Статус",
        (Language::En, "action_groups.status") => "Status",
        (Language::Ru, "action_groups.note") => "Пометка",
        (Language::En, "action_groups.note") => "Note",
        (Language::Ru, "action_groups.not_found_in_ha") => "не найдено в Home Assistant",
        (Language::En, "action_groups.not_found_in_ha") => "not found in Home Assistant",
        (Language::Ru, "action_groups.managed_in_ha") => "управляется в Home Assistant",
        (Language::En, "action_groups.managed_in_ha") => "managed in Home Assistant",
        (Language::Ru, "action_groups.devices_title") => "📋 Устройства группы",
        (Language::En, "action_groups.devices_title") => "📋 Group devices",
        (Language::Ru, "action_groups.devices_group") => "Устройства группы",
        (Language::En, "action_groups.devices_group") => "Group devices",
        (Language::Ru, "action_groups.filter") => "Фильтр",
        (Language::En, "action_groups.filter") => "Filter",
        (Language::Ru, "action_groups.filter_all") => "Все",
        (Language::En, "action_groups.filter_all") => "All",
        (Language::Ru, "action_groups.filter_selected") => "Выбранные",
        (Language::En, "action_groups.filter_selected") => "Selected",
        (Language::Ru, "action_groups.filter_unselected") => "Не выбранные",
        (Language::En, "action_groups.filter_unselected") => "Unselected",
        (Language::Ru, "action_groups.filter_all_plain") => "все",
        (Language::En, "action_groups.filter_all_plain") => "all",
        (Language::Ru, "action_groups.filter_selected_plain") => "выбранные",
        (Language::En, "action_groups.filter_selected_plain") => "selected",
        (Language::Ru, "action_groups.filter_unselected_plain") => "не выбранные",
        (Language::En, "action_groups.filter_unselected_plain") => "unselected",
        (Language::Ru, "action_groups.selected") => "Выбрано",
        (Language::En, "action_groups.selected") => "Selected",
        (Language::Ru, "action_groups.page") => "Страница",
        (Language::En, "action_groups.page") => "Page",
        (Language::Ru, "action_groups.schedules_title") => "🕒 Расписания",
        (Language::En, "action_groups.schedules_title") => "🕒 Schedules",
        (Language::Ru, "action_groups.schedules_plain") => "Расписания",
        (Language::En, "action_groups.schedules_plain") => "Schedules",
        (Language::Ru, "action_groups.schedule_title") => "🕒 Расписание",
        (Language::En, "action_groups.schedule_title") => "🕒 Schedule",
        (Language::Ru, "action_groups.schedule_target_not_found") => "Цель расписания не найдена",
        (Language::En, "action_groups.schedule_target_not_found") => "Schedule target not found",
        (Language::Ru, "action_groups.schedule_not_found") => "Расписание не найдено",
        (Language::En, "action_groups.schedule_not_found") => "Schedule not found",
        (Language::Ru, "action_groups.schedule_deleted") => "Расписание удалено",
        (Language::En, "action_groups.schedule_deleted") => "Schedule deleted",
        (Language::Ru, "action_groups.add_turn_on_schedule") => "➕ Включить",
        (Language::En, "action_groups.add_turn_on_schedule") => "➕ Turn on",
        (Language::Ru, "action_groups.add_turn_off_schedule") => "➕ Выключить",
        (Language::En, "action_groups.add_turn_off_schedule") => "➕ Turn off",
        (Language::Ru, "action_groups.add_schedule") => "➕ Добавить расписание",
        (Language::En, "action_groups.add_schedule") => "➕ Add schedule",
        (Language::Ru, "action_groups.total") => "Всего",
        (Language::En, "action_groups.total") => "Total",
        (Language::Ru, "action_groups.disable_schedule") => "⏸ Выключить расписание",
        (Language::En, "action_groups.disable_schedule") => "⏸ Disable schedule",
        (Language::Ru, "action_groups.enable_schedule") => "▶️ Включить расписание",
        (Language::En, "action_groups.enable_schedule") => "▶️ Enable schedule",
        (Language::Ru, "action_groups.edit_time") => "✏️ Изменить время",
        (Language::En, "action_groups.edit_time") => "✏️ Edit time",
        (Language::Ru, "action_groups.command_button") => "🔁 Действие",
        (Language::En, "action_groups.command_button") => "🔁 Action",
        (Language::Ru, "action_groups.command") => "Действие",
        (Language::En, "action_groups.command") => "Action",
        (Language::Ru, "action_groups.time") => "Время",
        (Language::En, "action_groups.time") => "Time",
        (Language::Ru, "action_groups.days") => "Дни",
        (Language::En, "action_groups.days") => "Days",
        (Language::Ru, "action_groups.create_title") => "Создание группы действий",
        (Language::En, "action_groups.create_title") => "Create action group",
        (Language::Ru, "action_groups.create_prompt") => {
            "Введите название новой группы действий."
        }
        (Language::En, "action_groups.create_prompt") => "Enter the new action group name.",
        (Language::Ru, "action_groups.rename_title") => "Переименование группы действий",
        (Language::En, "action_groups.rename_title") => "Rename action group",
        (Language::Ru, "action_groups.rename_prompt") => "Введите новое название группы.",
        (Language::En, "action_groups.rename_prompt") => "Enter the new group name.",
        (Language::Ru, "action_groups.alias_title") => "Алиас HA-действия",
        (Language::En, "action_groups.alias_title") => "HA action alias",
        (Language::Ru, "action_groups.alias_prompt") => {
            "Введите локальное название действия в боте."
        }
        (Language::En, "action_groups.alias_prompt") => "Enter the local action name in the bot.",
        (Language::Ru, "action_groups.schedule_time_title") => "Время расписания",
        (Language::En, "action_groups.schedule_time_title") => "Schedule time",
        (Language::Ru, "action_groups.schedule_time_prompt") => {
            "Введите время запуска в формате HH:MM. Например: 07:30"
        }
        (Language::En, "action_groups.schedule_time_prompt") => {
            "Enter the run time in HH:MM format. Example: 07:30"
        }
        (Language::Ru, "action_groups.schedule_time_edit_prompt") => {
            "Введите новое время в формате HH:MM. Например: 23:30"
        }
        (Language::En, "action_groups.schedule_time_edit_prompt") => {
            "Enter the new time in HH:MM format. Example: 23:30"
        }
        (Language::Ru, "action_groups.delete_group_title") => "Удаление группы действий",
        (Language::En, "action_groups.delete_group_title") => "Delete action group",
        (Language::Ru, "action_groups.delete_group_prompt") => "Удалить группу",
        (Language::En, "action_groups.delete_group_prompt") => "Delete group",
        (Language::Ru, "action_groups.delete_schedule_title") => "Удаление расписания",
        (Language::En, "action_groups.delete_schedule_title") => "Delete schedule",
        (Language::Ru, "action_groups.delete_schedule_prompt") => "Удалить расписание?",
        (Language::En, "action_groups.delete_schedule_prompt") => "Delete schedule?",
        (Language::Ru, "action_groups.access_all") => "доступно всем",
        (Language::En, "action_groups.access_all") => "available to all",
        (Language::Ru, "action_groups.access_admin") => "только администратору",
        (Language::En, "action_groups.access_admin") => "admin only",
        (Language::Ru, "action_groups.aggregate_all_on") => "все включено",
        (Language::En, "action_groups.aggregate_all_on") => "all on",
        (Language::Ru, "action_groups.aggregate_all_off") => "выключено",
        (Language::En, "action_groups.aggregate_all_off") => "off",
        (Language::Ru, "action_groups.aggregate_mixed") => "смешано",
        (Language::En, "action_groups.aggregate_mixed") => "mixed",
        (Language::Ru, "action_groups.aggregate_unknown") => "неизвестно",
        (Language::En, "action_groups.aggregate_unknown") => "unknown",
        (Language::Ru, "action_groups.enabled") => "активна",
        (Language::En, "action_groups.enabled") => "active",
        (Language::Ru, "action_groups.paused") => "на паузе",
        (Language::En, "action_groups.paused") => "paused",
        (Language::Ru, "action_groups.schedule_enabled") => "включено",
        (Language::En, "action_groups.schedule_enabled") => "enabled",
        (Language::Ru, "action_groups.schedule_disabled") => "выключено",
        (Language::En, "action_groups.schedule_disabled") => "disabled",
        (Language::Ru, "action_groups.command_turn_on") => "включить",
        (Language::En, "action_groups.command_turn_on") => "turn on",
        (Language::Ru, "action_groups.command_turn_off") => "выключить",
        (Language::En, "action_groups.command_turn_off") => "turn off",
        (Language::Ru, "action_groups.command_execute") => "выполнить",
        (Language::En, "action_groups.command_execute") => "run",
        (Language::Ru, "action_groups.state_on") => "включено",
        (Language::En, "action_groups.state_on") => "on",
        (Language::Ru, "action_groups.state_off") => "выключено",
        (Language::En, "action_groups.state_off") => "off",
        (Language::Ru, "action_groups.state_unavailable") => "недоступно",
        (Language::En, "action_groups.state_unavailable") => "unavailable",
        (Language::Ru, "action_groups.state_unknown") => "неизвестно",
        (Language::En, "action_groups.state_unknown") => "unknown",
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
