//! Shared presentation model: both renderers execute the same explicit actions.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum InterfaceMode {
    Classic,
    #[default]
    Html,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Screen {
    #[default]
    Home,
    Rooms,
    Room,
    Device,
    Settings,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct Item {
    pub label: &'static str,
    pub detail: &'static str,
    pub action: &'static str,
    pub selected: bool,
    pub primary: bool,
}

impl Item {
    fn new(label: &'static str, detail: &'static str, action: &'static str) -> Self {
        Self {
            label,
            detail,
            action,
            selected: false,
            primary: false,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct Hero {
    pub label: &'static str,
    pub value: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct Page {
    pub v: u8,
    pub rev: u64,
    pub screen: Screen,
    pub eyebrow: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub hero: Option<Hero>,
    pub items: Vec<Item>,
    pub nav: Vec<Item>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Model {
    pub mode: InterfaceMode,
    pub screen: Screen,
    pub on: bool,
    pub rev: u64,
}

impl Model {
    pub fn page(&self) -> Page {
        let state = if self.on {
            "Включён"
        } else {
            "Выключен"
        };
        let mut page = Page {
            v: 1,
            rev: self.rev,
            screen: self.screen,
            eyebrow: "Мой дом",
            title: "Дом",
            description: "",
            hero: None,
            items: Vec::new(),
            nav: Vec::new(),
        };
        match self.screen {
            Screen::Home => {
                page.description = "Всё нужное — под рукой.";
                page.items = vec![
                    Item::new("Комнаты", "1 комната · 1 устройство", "rooms"),
                    Item::new("Торшер", state, "device"),
                    Item::new("Мои настройки", "Вид интерфейса", "settings"),
                ];
            }
            Screen::Rooms => {
                page.title = "Комнаты";
                page.description = "Выберите комнату.";
                page.items = vec![Item::new("Гостиная", "1 устройство", "room")];
                page.nav = vec![Item::new("Дом", "", "home")];
            }
            Screen::Room => {
                page.eyebrow = "Комнаты";
                page.title = "Гостиная";
                page.description = "Откройте устройство для управления.";
                page.items = vec![Item::new("Торшер", state, "device")];
                page.nav = vec![
                    Item::new("Комнаты", "", "rooms"),
                    Item::new("Дом", "", "home"),
                ];
            }
            Screen::Device => {
                page.eyebrow = "Гостиная";
                page.title = "Торшер";
                page.hero = Some(Hero {
                    label: "Текущее состояние",
                    value: state,
                });
                let mut action = Item::new(
                    if self.on {
                        "Выключить свет"
                    } else {
                        "Включить свет"
                    },
                    "",
                    if self.on { "off" } else { "on" },
                );
                action.primary = true;
                page.items = vec![action];
                page.nav = vec![
                    Item::new("Гостиная", "", "room"),
                    Item::new("Вид", "", "settings"),
                ];
            }
            Screen::Settings => {
                page.eyebrow = "Мои настройки";
                page.title = "Вид интерфейса";
                page.description = "Выбор сохранится после перезапуска.";
                page.items = vec![
                    Item::new("Обычный", "Кнопки Telegram", "classic"),
                    Item::new("HTML", "Управление внутри карточки", "html"),
                ];
                page.items[0].selected = self.mode == InterfaceMode::Classic;
                page.items[1].selected = self.mode == InterfaceMode::Html;
                page.nav = vec![
                    Item::new("Дом", "", "home"),
                    Item::new("Торшер", "", "device"),
                ];
            }
        }
        page
    }

    /// A request must come from the current page and revision, including navigation.
    /// A stale request only reconciles the screen; it can never replay an action.
    pub fn action<'a>(&self, data: &'a str) -> Result<Option<&'a str>> {
        ensure!(data.len() <= 64, "Request too long");
        let mut parts = data.split(':');
        ensure!(parts.next() == Some("p1"), "Invalid protocol");
        let rev = parts.next().context("Missing revision")?.parse::<u64>()?;
        let action = parts.next().context("Missing action")?;
        ensure!(parts.next().is_none(), "Invalid request");
        // Refresh can reconcile a failed edit without replaying a stale command.
        if action == "read" {
            return Ok(Some(action));
        }
        if rev != self.rev {
            return Ok(None);
        }
        let page = self.page();
        ensure!(
            action == "read"
                || page
                    .items
                    .iter()
                    .chain(&page.nav)
                    .any(|item| item.action == action),
            "Action unavailable on this screen"
        );
        Ok(Some(action))
    }

    /// Rendering a page and changing presentation never execute device commands.
    pub fn apply(&mut self, action: &str) -> Result<()> {
        match action {
            "home" => self.screen = Screen::Home,
            "rooms" => self.screen = Screen::Rooms,
            "room" => self.screen = Screen::Room,
            "device" => self.screen = Screen::Device,
            "settings" => self.screen = Screen::Settings,
            "classic" => self.mode = InterfaceMode::Classic,
            "html" => self.mode = InterfaceMode::Html,
            "on" if self.screen == Screen::Device => self.on = true,
            "off" if self.screen == Screen::Device => self.on = false,
            "read" => {}
            _ => anyhow::bail!("Unsupported action"),
        }
        self.rev += 1;
        Ok(())
    }
}
