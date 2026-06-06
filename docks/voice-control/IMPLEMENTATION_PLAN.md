# Voice Control Implementation Plan

## Цель

Реализовать голосовое управление без обхода существующих прав доступа:

```text
Telegram voice -> HA Assist STT -> text -> bot command engine -> access guard -> HA service call
```

Home Assistant Assist в MVP используется только как STT pipeline со
`start_stage = "stt"` и `end_stage = "stt"`. Команды управления не выполняются
через Conversation API.

## Общие правила реализации

- Каждый этап должен компилироваться.
- Сначала расширяется текстовый command layer, потом добавляется voice-вход.
- Все управляющие команды проходят через существующие профили доступа.
- `critical` устройства требуют подтверждения.
- Pending confirmations хранятся в SQLite и живут до TTL.
- Временные audio-файлы удаляются после распознавания.
- Conversation API добавляется только для read-only вопросов отдельным этапом.

## Stage 0. Подготовка

Проверить текущие точки интеграции:

- `src/bot/text_commands.rs` - существующий быстрый текстовый слой.
- `src/bot/handlers.rs` - обработка сообщений Telegram.
- `src/bot/router.rs` - callback payload и pending-подтверждения.
- `src/core/devices.rs` - выполнение действий над устройствами.
- `src/db/access.rs` - проверка доступов.
- `src/db/devices.rs` - флаг `critical`.
- `src/ha/event_listener.rs` и HA WebSocket-клиент - ориентир для нового
  Assist pipeline WebSocket.

Проверка:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

## Stage 1. Command Engine

Вынести существующую логику из `src/bot/text_commands.rs` в общий command
engine, который сможет принимать текст как из обычного сообщения, так и после
STT.

Предлагаемые модули:

```text
src/core/commands.rs
src/core/command_parser.rs
src/core/command_access.rs
```

Минимальные типы:

```rust
pub enum CommandSource {
    Text,
    Voice,
}

pub enum CommandIntent {
    DeviceAction {
        device_id: i64,
        action: DeviceAction,
    },
    CameraSnapshot {
        camera_id: i64,
    },
    CameraClip {
        camera_id: i64,
        seconds: Option<u32>,
    },
    CameraArchive {
        camera_id: i64,
    },
    ReadOnlyQuestion {
        text: String,
    },
}

pub enum CommandSafety {
    Safe,
    RequiresConfirmation {
        reason: String,
    },
}

pub enum CommandExecution {
    Done {
        message: String,
    },
    NeedsConfirmation {
        pending_id: i64,
        message: String,
    },
    Failed {
        message: String,
    },
}
```

Что перенести/сохранить:

- поиск устройства по alias, `entity_id`, комнате и словам в любом порядке;
- поиск камеры по имени или ID;
- команды `свет коридор вкл`, `свет коридор выкл`;
- команды `снимок вход`, `видео камера 3 10`, `архив камера 3`;
- отказ при неоднозначном поиске.

Проверки доступа:

- неизвестный пользователь не выполняет команду;
- `can_view_room`;
- `can_view_device`;
- `can_control_device` для управляющих действий;
- доступ к камере через `get_accessible_camera`;
- `can_use_voice` проверяется только для `CommandSource::Voice`.

Тесты:

- парсинг включения/выключения света;
- поиск по комнате и alias;
- отказ при нескольких совпадениях;
- отказ без доступа;
- voice-команда без `can_use_voice` отклоняется.

## Stage 2. User Profile: `can_use_voice`

Добавить право voice в профиль пользователя.

DB:

```sql
ALTER TABLE user_profiles ADD COLUMN can_use_voice INTEGER NOT NULL DEFAULT 1;
```

Если точное имя таблицы/колонки отличается, использовать существующую модель
профилей доступа.

Admin UI:

- в карточке пользователя показать `Голос: ВКЛ/ВЫКЛ`;
- добавить кнопку переключения;
- root всегда может использовать voice;
- для `guest` можно оставить default `1`, потому что доступы всё равно
  ограничивают комнаты и устройства.

DB helpers:

```rust
can_use_voice(user_id, is_admin, pool) -> Result<bool>
toggle_user_voice_access(user_id, pool) -> Result<bool>
```

Тесты:

- root всегда allowed;
- обычный user зависит от флага;
- default value после миграции равен allowed.

## Stage 3. Critical Devices In Confirmation Flow

Флаг `critical` уже хранится в `devices`. На этом этапе он начинает влиять на
command engine.

Правило:

- если команда управляющая;
- и target device помечен как `critical`;
- и `voice_confirm_dangerous = true`;
- то command engine создает pending confirmation вместо немедленного service
  call.

Также confirmation нужен для:

- массовых действий;
- замков, ворот, дверей;
- сигнализации;
- климатических действий выше заданного лимита.

Для MVP можно начать только с флага `critical` и массовых команд.

Тесты:

- обычное устройство выполняется сразу;
- critical-устройство создает pending;
- critical не запрещает действие после подтверждения.

## Stage 4. Pending Voice/Action Sessions

Добавить SQLite-таблицу pending-команд.

Пример схемы:

```sql
CREATE TABLE IF NOT EXISTS pending_commands (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    source TEXT NOT NULL,
    command_text TEXT NOT NULL,
    intent_json TEXT NOT NULL,
    reason TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    expires_at TEXT NOT NULL,
    confirmed_at TEXT,
    cancelled_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK(source IN ('text', 'voice')),
    CHECK(status IN ('pending', 'confirmed', 'cancelled', 'expired', 'executed', 'failed'))
);
```

Payload:

```rust
ConfirmPendingCommand { id: i64 }
CancelPendingCommand { id: i64 }
```

Поведение:

- pending создается только после access check;
- confirm повторно проверяет права и TTL;
- expired pending не выполняется;
- после выполнения status становится `executed`;
- maintenance worker чистит старые pending.

Тесты:

- pending создается с TTL;
- expired pending не выполняется;
- confirm выполняет action;
- cancel не выполняет action;
- pending переживает рестарт как запись в SQLite.

## Stage 5. Voice Options

Добавить в `AppOptions`:

```json
{
  "voice_enabled": false,
  "voice_stt_provider": "ha_pipeline",
  "voice_command_engine": "local_parser",
  "voice_ha_pipeline_id": null,
  "voice_stt_sample_rate": 16000,
  "voice_confirm_dangerous": true,
  "voice_pending_ttl_s": 60,
  "voice_max_audio_size_mb": 10,
  "voice_max_audio_duration_s": 30,
  "voice_stt_timeout_s": 45,
  "voice_show_recognized_text": true,
  "voice_response_format": "text"
}
```

Validation:

- `voice_pending_ttl_s`: `10..=300`;
- `voice_max_audio_size_mb`: `1..=50`;
- `voice_max_audio_duration_s`: `1..=120`;
- `voice_stt_timeout_s`: `5..=300`;
- `voice_stt_sample_rate`: минимум `8000`, лучше `16000`;
- provider/engine/response format только из разрешенных значений.

Язык:

- брать из языка интерфейса пользователя;
- fallback - `default_language`.

Тесты:

- default values;
- invalid provider rejected;
- invalid limits rejected;
- user language maps to STT language.

## Stage 6. HA Assist STT Pipeline Client

Добавить клиент для WebSocket API Assist pipeline.

Предлагаемый модуль:

```text
src/ha/assist_pipeline.rs
```

Нужные операции:

- получить список pipeline;
- выбрать `voice_ha_pipeline_id` или preferred/default;
- проверить, что pipeline поддерживает STT;
- запустить `assist_pipeline/run`:

```json
{
  "type": "assist_pipeline/run",
  "start_stage": "stt",
  "end_stage": "stt",
  "input": {
    "sample_rate": 16000
  }
}
```

- дождаться `stt-start`;
- отправить PCM chunks как binary messages с `stt_binary_handler_id`;
- отправить end marker;
- дождаться STT result;
- вернуть распознанный текст.

Ошибки:

- pipeline not found;
- STT provider missing;
- timeout;
- unsupported audio;
- HA WebSocket disconnected.

Тесты:

- parsing pipeline list response;
- selecting configured pipeline;
- selecting default pipeline;
- converting HA error event to user-facing error.

## Stage 7. Telegram Voice Input

Добавить обработку voice-сообщений в Telegram handlers.

Flow:

1. Проверить `voice_enabled`.
2. Проверить пользователя и `can_use_voice`.
3. Проверить длительность Telegram voice.
4. Скачать файл через Telegram API.
5. Проверить размер.
6. Декодировать OGG/Opus -> PCM mono.
7. Отправить PCM в HA Assist STT pipeline.
8. Показать распознанный текст, если `voice_show_recognized_text`.
9. Передать текст в command engine с `CommandSource::Voice`.
10. Вернуть результат или confirmation screen.
11. Удалить временный аудиофайл.

Декодирование:

- сначала попробовать через `ffmpeg-next`;
- если код получается слишком сложным или нестабильным, добавить отдельный
  Rust-пакет для OGG/Opus decoding;
- внешний `ffmpeg` процесс не использовать.

Тесты:

- voice disabled;
- too long voice rejected;
- too large audio rejected;
- STT error produces readable message;
- temp file cleanup on success and error.

## Stage 8. Admin Settings UI

Добавить настройки voice в админку:

- voice enabled;
- provider;
- pipeline id или default;
- response format;
- show recognized text;
- max duration;
- max size;
- pending TTL.

Для MVP можно часть оставить в `options.json`, а в UI добавить только:

- включить/выключить voice;
- `can_use_voice` на профиле пользователя.

## Stage 9. Read-Only Conversation API

Этот этап не блокирует MVP управления голосом.

Добавить:

- `process_conversation(text, language, agent_id)`;
- режим только для query/read-only;
- проверку target/success entity перед показом ответа;
- запрет `action_done` для control-команд.

Если HA вернул `action_done`, бот должен считать это unsafe для read-only пути
и показать ошибку, а не полагаться на результат.

Тесты:

- query_answer shown;
- no_intent_match readable;
- action_done rejected in readonly mode;
- target entity without access hidden/denied.

## Stage 10. Final Hardening

Проверки перед включением:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Manual smoke:

1. Voice disabled -> voice rejected.
2. Voice enabled, user without `can_use_voice` -> rejected.
3. `включи свет коридор` -> works for allowed user.
4. Same command for user without device access -> denied.
5. Critical device -> confirmation screen.
6. Confirmation after TTL -> expired.
7. Telegram voice too long -> rejected.
8. STT unavailable -> readable error.
9. Restart with pending command inside TTL -> still confirmable.
10. Restart with expired pending command -> not executable.

## Recommended Order

1. Command engine refactor.
2. `can_use_voice`.
3. Pending SQLite confirmations.
4. Critical confirmation in command engine.
5. Voice options.
6. HA Assist STT client.
7. Telegram voice handler.
8. Admin settings.
9. Read-only Conversation API.
10. Hardening and tests.
