# Recording Rule Wizard Implementation Plan

## Цель

Реализовать мастер создания правил записи камер по спецификации
`docks/RECORDING_RULE_WIZARD_SPEC.md`, оставив текущий текстовый ввод как
расширенный режим.

Первый рабочий результат: root-пользователь может создать правило записи для
дверного `binary_sensor` через кнопки без ручного ввода строк условий.

Следующий целевой результат: root-пользователь может создать generic-правило
через мастер:

```text
source -> condition -> + condition -> create
```

Например: `sensor.temperature` изменился и стал выше `28`, или дверь открылась
и дополнительный `binary_sensor` показывает включенную охрану.

## Общие Правила

- Каждый stage должен компилироваться.
- Старый текстовый формат создания/редактирования правил не удалять.
- Не добавлять миграцию БД для первого cut и Phase 2 generic-мастера без
  `crossed_above/crossed_below`.
- Не передавать длинный `entity_id` во всех callback payload.
- Состояние мастера хранить в session/dialogue или коротком server-side state.
- Проверки доступа, существования камеры/датчиков и archived-флагов выполнять
  перед каждым критичным действием, особенно перед созданием правила.
- Камера остается привязана к выбранной комнате; источник и условия generic-
  мастера можно выбирать из всех комнат.

## Stage 0: Startup Version Log

Добавить вывод версии при запуске бота.

Файлы:

- `src/main.rs`
- опционально `src/models.rs`, если версию позже нужно показывать в diagnostics

Реализация:

```rust
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
```

В начале `main()` заменить общий лог:

```rust
info!("Starting Home Assistant Telegram Bot v{}", APP_VERSION);
```

Желательно вывести также важные runtime-параметры без секретов:

- путь к БД;
- путь к миграциям;
- `HA_URL`;
- `default_language`;
- `voice_enabled`;
- `camera_recording_storage_root`;
- `camera_recording_max_parallel_jobs`.

Важно:

- не логировать `bot_token`;
- не логировать `HA_TOKEN`;
- не логировать RTSP URL камер.

Проверка:

```bash
cargo fmt --check
cargo test --locked
```

## Stage 1: Wizard Data Model

Добавить внутренние типы мастера.

Вероятные файлы:

- `src/bot/router.rs`
- или новый файл `src/bot/recording_rule_wizard.rs`

Типы:

```rust
struct RecordingRuleWizard {
    room_id: i64,
    camera_id: Option<i64>,
    device_id: Option<i64>,
    entity_id: Option<String>,
    trigger_mode: Option<WizardTriggerMode>,
    tail_seconds: Option<u32>,
    retention_days: Option<u32>,
    group_id: Option<i64>,
}

enum WizardTriggerMode {
    OpenAndClose,
    OpenOnly,
    CloseOnly,
}
```

Для Phase 1 нужны только дверные режимы:

- `OpenAndClose`
- `OpenOnly`
- `CloseOnly`

Эта модель достаточна только для Phase 1. Для generic-мастера ее нужно заменить
или расширить до:

- `source: Option<WizardSourceDraft>`;
- `conditions: Vec<WizardConditionDraft>`;
- `condition_logic: All | Any`.

Подробная целевая модель описана в `docks/RECORDING_RULE_WIZARD_SPEC.md`.

Решение по хранению:

- Phase 1: хранить состояние в `State` dialogue, если это проще вписать в текущий
  teloxide flow;
- альтернативно: хранить в `AppConfig.sessions`/отдельной `DashMap` по user_id,
  а callback делать короткими.

Проверка:

```bash
cargo check --locked
```

## Stage 2: Trigger Candidates DB Helper

Добавить helper для получения датчиков комнаты, подходящих для мастера.

Файл:

- `src/db/devices.rs`

Тип:

```rust
pub struct RecordingTriggerCandidate {
    pub device_id: i64,
    pub entity_id: String,
    pub display_name: String,
    pub device_domain: String,
    pub device_class: String,
}
```

Функция:

```rust
pub async fn list_room_recording_trigger_candidates(
    room_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingTriggerCandidate>>
```

SQL должен брать:

- `id`;
- `entity_id`;
- `COALESCE(alias, ha_name, entity_id)`;
- `COALESCE(device_domain, substr(entity_id, 1, instr(entity_id, '.') - 1))`;
- `COALESCE(device_class, '')`.

Фильтр MVP:

- `room_id = ?`;
- `archived = 0`;
- только `binary_sensor`;
- `device_class IN ('door', 'window', 'opening', 'garage_door')`.

Сортировка:

- по display name;
- затем по `entity_id`.

Тесты:

- возвращает дверной `binary_sensor`;
- не возвращает archived device;
- не возвращает sensor/light в Phase 1.

Проверка:

```bash
cargo test --locked db::devices
```

## Stage 3: Rule Generation Helpers

Вынести генерацию rule input/conditions в маленькие чистые функции.

Вероятный файл:

- `src/bot/recording_rule_wizard.rs`

Функции:

```rust
fn build_wizard_rule_name(lang, display_name, mode) -> String
fn build_wizard_conditions(entity_id, mode) -> Vec<NewRecordingConditionData>
fn build_advanced_rule_text(summary) -> String
```

Для Phase 1:

- `OpenAndClose` -> `off -> on`, `on -> off`;
- `OpenOnly` -> `off -> on`;
- `CloseOnly` -> `on -> off`.

Дефолты:

- `tail_seconds = 60`;
- `max_segment_seconds = config.camera_recording_max_segment_seconds`;
- `cooldown_s = 0`;
- `retention_days` из `CAMERA_RECORDING_DEFAULT_RETENTION_DAYS`, fallback `30`;
- `notify_enabled` остается включенным через default БД.

Тесты:

- имя правила на русском;
- `OpenAndClose` дает две conditions;
- сгенерированный advanced text парсится текущим parser;
- значения не превышают лимиты config.

Проверка:

```bash
cargo test --locked recording_rule_wizard
```

## Stage 4: Admin Payload And Routing

Добавить callback payload для мастера.

Файл:

- `src/bot/router.rs`

Payload Phase 1 должен быть коротким:

```rust
StartRecordingRuleWizard { room: i64 }
WizardPickCamera { camera: i64 }
WizardPickEntity { device: i64 }
WizardPickMode { mode: String }
WizardPickTail { tail: u32 }
WizardPickRetention { retention: u32 }
WizardCreateRule
WizardAdvancedText
WizardCancel
```

Если текущая payload-сериализация требует контекст комнаты в каждом payload,
допускается оставить `room`, но не добавлять длинный `entity_id`.

Проверки:

- legacy payload tests не ломаются;
- payload size test проходит;
- back/cancel возвращает в `RecordingRules { room }`.

Проверка:

```bash
cargo test --locked bot::router
```

## Stage 5: Wizard Screens

Добавить экраны мастера.

Файл:

- `src/bot/screens/admin/list_actions.rs`

Экраны:

- start/select camera;
- select trigger entity;
- select trigger mode;
- select tail;
- select retention;
- optional group or skip;
- confirm.

Первое изменение в существующем экране:

- заменить кнопку `➕ Добавить правило` на:
  - `➕ Мастер`;
  - `⌨️ Расширенно`.

Если камер нет:

- показать кнопку добавления камеры;
- не продолжать мастер.

Если датчиков нет:

- показать переход в расширенный ввод;
- показать back.

Проверки:

- текст не требует ручного ввода entity_id;
- в summary видно камеру, датчик, режим, длительность, хранение;
- все callback payload компактные.

## Stage 6: Create Rule Handler

Добавить создание правила из wizard summary.

Вероятные файлы:

- `src/bot/router.rs`
- `src/bot/handlers.rs`
- `src/db/camera_recording_rules.rs`, если нужен helper транзакционного создания

Поведение:

1. Проверить root user.
2. Проверить, что camera существует, enabled и принадлежит room.
3. Проверить, что source и все condition devices существуют и `archived = 0`.
4. Проверить, что выбранный режим и операторы поддержаны текущей фазой мастера.
5. Проверить дубли.
6. Создать `camera_recording_rules`.
7. Создать conditions.
8. Если выбраны группы, добавить записи в `camera_recording_rule_group_items`.
9. Записать activity log.
10. Вернуть экран detail созданного правила или список правил с notice.

Лучше добавить транзакционный DB helper:

```rust
create_rule_with_conditions_and_group(...)
```

Это уберет состояние, где rule создан, а conditions не создались.

Проверки:

- правило появляется в списке;
- detail показывает две conditions;
- duplicate flow не создает дубль по умолчанию;
- rule из другой комнаты не создается.

## Stage 7: Advanced Fallback

Реализовать кнопку `✏️ Расширенно` в confirm screen.

Ограничение Telegram:

- нельзя предзаполнить пользовательское поле ввода;
- можно показать сгенерированный блок в сообщении и перевести dialogue в
  существующий `AddRecordingRule`.

Поведение:

```text
Скопируйте блок ниже, измените если нужно и отправьте сообщением:

<generated rule block>
```

Кнопки:

- `Назад`;
- `Отмена`.

Проверка:

- отправка показанного блока создает такое же правило через существующий parser;
- parser tests остаются зелеными.

## Stage 8: Groups

Phase 1 допускает одиночный выбор или пропуск группы, если это уже реализовано.
Целевое решение для generic-мастера:

- показать выбор групп в конце мастера, после tail/retention и перед confirm;
- вывести все существующие группы одной страницей;
- нажатие по группе переключает галочку;
- кнопка `Далее` доступна всегда, даже если ничего не выбрано;
- если групп нет, можно сразу продолжить без групп;
- не создавать базовые группы автоматически.

После создания:

- если выбраны groups, добавить rule во все выбранные группы;
- если ничего не выбрано, ничего не делать.

Проверки:

- выбранные группы отображаются в detail;
- пустой выбор создает rule без group;
- выключенная группа не должна внезапно блокировать правило без явного выбора.

## Stage 9: End-To-End Manual Test

Сценарий на текущей базе:

- room `4`;
- camera `3`;
- door sensor `binary_sensor.zamok_contact`.

Проверка:

1. Запустить бота.
2. Убедиться, что в логах есть версия.
3. Открыть `Админка -> Камеры -> Комната -> Правила записи`.
4. Нажать `Мастер`.
5. Выбрать/подтвердить `USB камера`.
6. Выбрать `Замок Дверь`.
7. Выбрать `Открытие и закрытие`.
8. Выбрать `60с`.
9. Выбрать `30д`.
10. Нажать `Создать`.
11. Проверить detail: две conditions.
12. Открыть/закрыть дверь в HA.
13. Проверить, что запись стартует или активная запись продлевается.

## Stage 10: Verification Before Merge

Обязательные проверки:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Ручные проверки:

- создание правила через мастер;
- создание правила через старый расширенный ввод;
- duplicate flow;
- back/cancel на каждом шаге;
- restart bot во время открытого wizard не ломает обычное меню.

## Stage 11: Generic Source Candidates

Расширить helper кандидатов мастера и вернуть датчики из всех комнат.

Файл:

- `src/db/devices.rs`

Новые domain:

- `binary_sensor`;
- `sensor`;
- `number`;
- `switch`;
- `light`.

Сортировка:

- текущая комната камеры первой;
- затем остальные комнаты по имени;
- внутри комнаты сначала типовые binary sensors: door/window/opening/garage_door;
- затем motion/presence;
- затем numeric sensors/numbers;
- затем switch/light;
- затем прочие поддерживаемые sensors/binary sensors.

Для `sensor`/`number`:

- показывать все поддерживаемые датчики;
- для numeric-операторов `above`/`below` валидировать введенное значение как
  число;
- если оператор не подходит типу сущности, не показывать его в списке операторов
  или возвращать понятную ошибку перед сохранением условия.

Тесты:

- возвращает `sensor.temperature`;
- возвращает `number`;
- не возвращает archived device;
- возвращает датчики из другой комнаты;
- текущая комната идет первой;
- порядок кандидатов стабилен.

## Stage 12: Generic Wizard State And Payload

Перейти от одного `entity_id + trigger_mode` к состоянию:

```rust
RecordingRuleWizard {
    room_id,
    camera_id,
    source,
    conditions,
    condition_logic,
    tail_seconds,
    retention_days,
    group_ids,
}
```

Добавить payload:

- `WizardPickSource`;
- `WizardPickSourceMode`;
- `WizardAddCondition`;
- `WizardPickConditionEntity`;
- `WizardPickConditionOperator`;
- `WizardEnterConditionValue`;
- `WizardSaveCondition`;
- `WizardRemoveCondition`;
- `WizardToggleLogic`;
- `WizardToggleGroup`;
- `WizardConfirmGroups`.

Важно:

- `entity_id` не класть в callback payload;
- значение условия получать текстовым вводом через dialogue state;
- payload size tests должны остаться зелеными.

## Stage 13: Generic Condition Screens

Файл:

- `src/bot/screens/admin/list_actions.rs`

Добавить экраны:

- summary условий;
- выбор source entity с группировкой по комнатам;
- выбор condition entity с группировкой по комнатам;
- выбор оператора;
- ввод значения;
- удаление условия;
- переключение `all`/`any`.

Операторы Phase 2 без миграции:

- `is`;
- `is_not`;
- `contains`;
- `above`;
- `below`;
- `changed_to`;
- `changed_from_to`.

UX-правило:

- системные названия операторов показывать только в advanced text;
- пользователю показывать `равно`, `не равно`, `выше`, `ниже`,
  `изменилось на`, `изменилось с ... на ...`.
- `crossed_above/crossed_below` не показывать до отдельной миграции и matcher.

## Stage 14: Generic Rule Generation

Расширить генератор условий.

Примеры генерации:

- door open+close:
  - `changed_from_to off -> on`;
  - `changed_from_to on -> off`;
  - logic `any`;
- sensor any change:
  - `changed_to` без `to_state`;
- sensor above threshold:
  - `changed_to` без `to_state`;
  - `above` со значением;
  - logic `all`;
- source plus context condition:
  - source event condition;
  - context condition;
  - logic `all`.

Тесты:

- generated advanced text парсится текущим parser;
- numeric threshold требует число;
- `all/any` сохраняется правильно;
- duplicate detection сравнивает нормализованный набор условий.

## Stage 15: Crossed Threshold Operators

Не включать в Phase 2, если не делаем миграцию.

Когда понадобится точное "пересекло порог":

- добавить операторы `crossed_above` и `crossed_below`;
- обновить DB validation / CHECK constraint, если он есть;
- обновить matcher, чтобы сравнивал old/new numeric state;
- добавить тесты для:
  - `27 -> 29` matches `crossed_above 28`;
  - `29 -> 30` не matches `crossed_above 28`;
  - `unknown -> 29` не matches;
  - нечисловые значения не создают запись.

## Cut Line For First Release

В первый релиз мастера входят:

- лог версии при запуске;
- door/window/opening wizard;
- выбор камеры;
- выбор датчика;
- open/close/open+close modes;
- tail/retention presets;
- страница групп с галочками и `Далее`, даже если ничего не выбрано;
- создание правила;
- advanced fallback.

Не входят в первый cut, но входят в следующий generic-этап:

- motion/presence;
- switch/light;
- sensor/number;
- несколько условий;
- переключение `all/any`;
- ввод значения условия;
- numeric threshold "стало выше/ниже".

Не входят без отдельной миграции:

- `crossed_above`;
- `crossed_below`;
- export/import;
- diagnostics screen;
- автоматическое создание групп;
- редактор произвольных выражений.
