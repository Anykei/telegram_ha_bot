# Camera Recording Implementation Plan

## Цель

Реализовать событийную запись камер по спецификации `docs/camera-recording/README.md` без блокировки Telegram UI и без ломки текущих ручных действий `📸 Снимок` / `🎞 клип`.

## Общий порядок

Реализацию лучше делать маленькими этапами:

1. Data model.
2. DB layer.
3. Core session/worker.
4. Event matcher.
5. Telegram archive UI.
6. Admin rule UI.
7. Maintenance and recovery.
8. Notifications.
9. Localization.
10. Tests and hardening.

Каждый этап должен компилироваться и не ломать существующие камеры.

## Stage 0: Подготовка

- Проверить текущий `src/core/cameras.rs` и убедиться, что `capture_clip(camera, seconds)` можно переиспользовать для сегментов.
- Зафиксировать, что ручные кнопки камеры остаются отдельными от событийного архива.
- Добавить временные feature-independent helpers только если они реально нужны.

Проверка:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
```

## Stage 1: Миграции

Добавить миграцию с таблицами:

- `camera_recording_rules`
- `camera_recording_rule_conditions`
- `camera_recording_sessions`
- `camera_recording_segments`

Важно:

- active-сессия ищется по `camera_id`, а не по `rule_id + camera_id`;
- `event_group_id` не уникален;
- `camera_recording_segments.file_path` заполняется только после успешного atomic rename;
- `extended_by_rule_ids` можно хранить JSON-строкой.

Минимальная проверка:

```bash
cargo check --locked
```

## Stage 2: DB Layer

Добавить модули:

- `src/db/camera_recording_rules.rs`
- `src/db/camera_recording_rule_conditions.rs`
- `src/db/camera_recording_sessions.rs`
- `src/db/camera_recording_segments.rs`

Подключить в `src/db/mod.rs`.

Нужные функции:

- CRUD rules.
- CRUD conditions.
- `find_candidate_rules_by_entity(entity_id)`.
- `find_active_session_by_camera(camera_id)`.
- `create_session(...)`.
- `extend_session(session_id, rule_id, event_time, stop_after_at, trigger_summary)`.
- `mark_session_recording`.
- `mark_session_ready`.
- `mark_session_failed`.
- `create_segment`.
- `mark_segment_ready`.
- `mark_segment_failed`.
- `list_camera_sessions(camera_id, user_id/is_admin)`.
- `list_session_segments(session_id)`.
- `soft_delete_session(session_id)`.
- `find_expired_sessions`.
- `recover_stale_sessions`.

Тесты:

- создание правила с условиями;
- создание сессии;
- продление сессии;
- создание сегментов;
- soft-delete.

## Stage 3: Options and Settings

Добавить в `AppOptions`:

- `camera_recording_max_tail_seconds`, default `300`;
- `camera_recording_max_segment_seconds`, default `300`;
- `camera_recording_max_parallel_jobs`, default `4`;
- `camera_recording_storage_root`, default `data/recordings`.

Добавить в `db::settings` ключи:

- `camera_recording_default_tail_seconds = 60`;
- `camera_recording_default_max_segment_seconds = 300`;
- `camera_recording_default_cooldown_s = 0`;
- `camera_recording_default_retention_days = 30`;
- `camera_recording_max_storage_mb = 0`.

Проверить validation:

- tail `5..=max_tail`;
- segment `30..=max_segment`;
- parallel `1..=4` или шире, если решишь разрешить больше.

## Stage 4: Core Recording Worker

Добавить `src/core/camera_recording.rs`.

Основные типы:

- `RecordingJob { session_id }`
- `RecordingEventGroupId`
- `RecordingWorkerConfig`

В `AppConfig` добавить:

```rust
pub camera_recording_tx: tokio::sync::mpsc::Sender<RecordingJob>
```

В `main.rs`:

- создать `mpsc::channel::<RecordingJob>(32)`;
- положить sender в `AppConfig`;
- запустить `spawn_recording_worker(recording_rx, app_config, cancel_token)`.

Worker algorithm:

```text
receive RecordingJob
acquire semaphore
load session + rule + camera
session -> recording
loop:
  reload stop_after_at
  remaining = stop_after_at - now
  if remaining <= 0: break
  segment_duration = min(remaining, max_segment_seconds)
  create segment
  capture_clip(camera, segment_duration)
  write tmp file
  atomic rename tmp -> final file
  segment -> ready
session -> ready
send ready notification
```

Важно:

- не держать DB transaction во время записи видео;
- tmp-файл удалять при ошибке;
- `file_path` писать только после rename;
- ошибки не должны содержать RTSP URL.

## Stage 5: Event Matcher

Добавить matcher рядом с notification processor или отдельным модулем:

```text
src/core/camera_recording_matcher.rs
```

Логика:

1. Получить HA event.
2. Найти candidate rules по `event.entity_id`.
3. Проверить операторы:
   - `changed_to`
   - `changed_from_to`
   - `is`
   - `is_not`
   - `contains`
   - `above`
   - `below`
4. Для `all` запросить текущие состояния контекстных entity.
5. В транзакции:
   - найти active session по `camera_id`;
   - если есть, продлить;
   - если нет, проверить cooldown;
   - создать session;
6. Отправить `RecordingJob`.

При переполненной очереди:

- session -> failed;
- error `recording queue is full`;
- log warn.

## Stage 6: Archive UI

Расширить `CameraPayload`:

- `RecordingArchive { camera }`
- `RecordingSession { camera, session }`
- `SendRecordingSegment { session, segment }`
- `SendRecordingAll { session }`
- `DeleteRecording { camera, session }`
- `ConfirmDeleteRecording { camera, session }`

Экраны:

- список камер уже есть;
- в карточку камеры добавить `🗂 Архив записей`;
- экран архива камеры;
- карточка сессии;
- отправка одного сегмента;
- отправка всех ready-сегментов по порядку.

Access:

- перед каждым экраном и отправкой файла проверять `get_accessible_camera`;
- root/admin видит все;
- пользователь без доступа получает отказ.

## Stage 7: Admin Rule UI

Добавить в админку камер:

- `⚙️ Правила записи`;
- список правил;
- создание правила wizard;
- включить/выключить;
- удалить;
- редактировать числа;
- настройка `camera_recording_default_retention_days`;
- настройка `camera_recording_max_storage_mb`.

Wizard MVP:

1. Выбор камеры.
2. Выбор `И` / `ИЛИ`.
3. Добавление условия.
4. Добавить еще условие или продолжить.
5. Tail seconds.
6. Max segment seconds.
7. Cooldown.
8. Retention days.
9. Название.
10. Подтверждение.

## Stage 8: Maintenance

Расширить `src/core/maintenance.rs`:

- очистка expired sessions;
- удаление файлов сегментов;
- soft-delete rows;
- recovery старых `queued/recording`;
- удаление tmp-файлов;
- storage quota cleanup.

Порядок cleanup:

1. Recovery stale active sessions.
2. Delete expired sessions.
3. Apply storage quota.
4. Log counts.

## Stage 9: Notifications

После `session -> ready`:

- собрать recipients: `root_user + subscribers(trigger entities)`;
- дедуплицировать;
- проверить `notification_sent_at`;
- отправить сообщение;
- заполнить `notification_sent_at`.

Для partial/failed MVP:

- обычное “готово” не отправлять;
- ошибка видна в архиве;
- отдельный alert root_user можно добавить позже.

## Stage 10: Localization

Добавить ключи `ru/en`:

- archive title;
- no recordings;
- recording ready;
- recording failed;
- partial recording;
- send video;
- send all parts;
- delete recording;
- recording rules;
- add rule;
- condition operators;
- storage quota;
- queue full;
- access denied.

Важно: новые экраны не должны содержать хардкод русского текста.

## Stage 11: Tests

Минимальный набор перед merge:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
```

Обязательные тесты:

- rule CRUD;
- condition operators;
- any/all matcher;
- active session extension by same camera;
- different cameras from same event;
- segment creation;
- partial recording;
- queue overflow;
- recovery;
- cleanup expired;
- storage quota;
- access checks for archive.

## Suggested PR Breakdown

Если делать постепенно, удобнее разбить на такие PR/коммиты:

1. DB migrations + DB layer.
2. Options/settings + models.
3. Recording worker without UI.
4. Event matcher integration.
5. Archive UI.
6. Admin rule UI.
7. Maintenance + quota.
8. Notifications + localization.
9. Tests/hardening.

## Definition of Done

- Новое правило можно создать из админки.
- Событие HA запускает запись.
- Повторные события той же камеры продлевают запись.
- Несколько камер могут писать параллельно.
- Запись режется на сегменты.
- Архив виден в карточке камеры.
- Пользователь может отправить сегмент или все части.
- Просроченные файлы удаляются.
- После рестарта старые active-сессии не зависают.
- Уведомление приходит root_user и подписчикам.
- Все новые строки локализованы на `ru/en`.
- `cargo fmt`, `cargo check`, `cargo test` проходят.
