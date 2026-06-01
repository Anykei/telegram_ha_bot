# Telegram HA Bot

Telegram HA Bot - это Telegram-бот для управления Home Assistant из чата. Он
показывает комнаты и устройства, умеет обновлять открытые экраны, отправлять
уведомления, строить графики датчиков, работать с короткими клипами камер и
ограничивать доступ разным пользователям.

Проект рассчитан на сценарий, где Home Assistant и камеры доступны боту в
локальной сети, а пользователь управляет домом через Telegram из любой точки.

## Возможности

- Управление устройствами Home Assistant по комнатам.
- Отдельные экраны для света, сенсоров, климата, числовых сущностей и настроек.
- Уведомления по изменениям состояний Home Assistant.
- Графики истории для датчиков.
- Админка в Telegram: пользователи, профили доступа, камеры, статус системы,
  резервная копия базы.
- Персональные профили: разные пользователи могут видеть разные комнаты и
  устройства.
- Раздел камер без внешнего HTTPS-сервера: снимок и короткое видео по RTSP или
  HTTP-потоку.
- Восстановление активных UI-сессий после перезапуска.
- Защита от отката интерфейса при фоновых обновлениях: меню пользователя
  редактируются последовательно и проверяют текущий контекст перед обновлением.

## Как это устроено

Бот синхронизируется с Home Assistant через REST API и WebSocket events. Список
комнат, устройств, подписок, пользовательских прав и камер хранится в SQLite.

Основные части:

- `src/bot` - Telegram handlers, router и экраны.
- `src/core` - фоновое обслуживание, уведомления, камеры, представление данных.
- `src/db` - SQLite-репозитории и правила доступа.
- `src/ha` - клиент Home Assistant и WebSocket listener.
- `migrations` - схема базы данных.
- `data/options.json` - локальная конфигурация бота, обычно не коммитится.

## Требования

- Rust 1.87+.
- SQLite.
- Home Assistant с long-lived access token или Supervisor token.
- Telegram bot token от BotFather.
- Для камер: доступный для бота RTSP/HTTP-поток.
- Для сборки с камерами: системные библиотеки FFmpeg/LibAV.

На Debian/Ubuntu для локальной сборки обычно нужны:

```bash
sudo apt-get update
sudo apt-get install -y \
  pkg-config clang libclang-dev libssl-dev libsqlite3-dev \
  libfreetype6-dev libfontconfig1-dev \
  libavformat-dev libavcodec-dev libavutil-dev libswscale-dev libswresample-dev
```

На macOS удобнее поставить зависимости через Homebrew:

```bash
brew install ffmpeg pkg-config sqlite openssl
```

## Конфигурация

Бот читает пути и Home Assistant из переменных окружения:

```env
OPTIONS_PATH=data/options.json
DATABASE_PATH=data/bot_data.db
MIGRATIONS_PATH=./migrations

HA_URL=http://homeassistant.local:8123
HA_TOKEN=your_home_assistant_long_lived_token

RUST_LOG=info
```

В Home Assistant Add-on окружении вместо `HA_TOKEN` может использоваться
`SUPERVISOR_TOKEN`, а `HA_URL` по умолчанию равен `http://supervisor/core`.

`options.json`:

```json
{
  "bot_token": "123456:telegram_bot_token",
  "root_user": "219791289",
  "background_maintenance_interval_s": 15,
  "event_refresh_min_interval_s": 5,
  "session_ttl_hours": 24,
  "telegram_retry_after_extra_delay_s": 1,
  "camera_clip_intervals_s": [5, 10, 15, 30],
  "camera_default_clip_s": 10
}
```

Поля `background_maintenance_interval_s`, `event_refresh_min_interval_s`,
`session_ttl_hours`, `telegram_retry_after_extra_delay_s`,
`camera_clip_intervals_s` и `camera_default_clip_s` имеют значения по умолчанию.

Камеры в `options.json` не добавляются. Они создаются через Telegram-админку и
хранятся в SQLite.

## Запуск локально

```bash
cargo run
```

При первом запуске применяются миграции из `MIGRATIONS_PATH`, создается или
обновляется SQLite-база, затем бот подключается к Home Assistant.

Для проверки перед запуском:

```bash
cargo fmt --check
cargo check
cargo test
```

## Запуск в Docker

Сборка:

```bash
docker build -t telegram-ha-bot .
```

Пример запуска:

```bash
docker run --rm \
  --name telegram-ha-bot \
  -e OPTIONS_PATH=/data/options.json \
  -e DATABASE_PATH=/data/bot_data.db \
  -e MIGRATIONS_PATH=/app/migrations \
  -e HA_URL=http://homeassistant.local:8123 \
  -e HA_TOKEN=your_home_assistant_token \
  -e RUST_LOG=info \
  -v "$PWD/data:/data" \
  telegram-ha-bot
```

Если контейнер не резолвит `homeassistant.local`, укажите IP Home Assistant,
настройте DNS или используйте подходящий Docker network mode.

## Первый запуск

1. Создайте Telegram-бота через BotFather.
2. Укажите `bot_token` и `root_user` в `options.json`.
3. Укажите `HA_URL` и `HA_TOKEN` в `.env` или окружении контейнера.
4. Запустите бота.
5. Напишите боту `/start` от пользователя `root_user`.
6. В админке добавьте остальных пользователей.

`root_user` всегда имеет полный доступ и видит админку.

## Профили доступа

Профили нужны, чтобы разные люди видели разные части дома. Например:

- взрослый пользователь видит все комнаты и устройства;
- ребенок не видит котел или серверную;
- гость видит только свет в гостиной.

В админке доступны:

- список пользователей;
- профиль пользователя;
- смена роли;
- сброс доступов;
- настройка доступа к комнатам;
- настройка доступа к устройствам внутри комнаты;
- включение и отключение уведомлений по устройствам.

Режимы доступа:

- `полный` - видно и можно управлять;
- `просмотр` - видно, но управление запрещено;
- `скрыто` - не видно в меню и недоступно по callback.

Роли переключаются по кругу:

```text
user -> child -> guest -> user
```

Для `child` и `guest` комнаты по умолчанию закрываются, после чего админ
открывает нужные комнаты и устройства вручную.

## Камеры

Раздел камер работает без Telegram Mini App и без внешнего HTTPS-сервера. Бот
получает кадр или короткий клип сам и отправляет его в чат как фото или видео.

Пользователь видит только камеры тех комнат, к которым у него есть доступ.

Админский путь добавления:

```text
Админка -> Камеры -> выбрать комнату -> Добавить камеру
```

Формат ввода:

```text
Название
RTSP URL
Интервал видео в секундах
Snapshot URL необязательно
```

Пример для IP-камеры:

```text
Вход
rtsp://user:password@192.168.1.50:554/stream1
10
http://192.168.1.50/snapshot.jpg
```

Можно вводить одной строкой через `;`:

```text
Вход; rtsp://user:password@192.168.1.50:554/stream1; 10; http://192.168.1.50/snapshot.jpg
```

Если `Snapshot URL` не указан, бот попробует взять кадр из видеопотока. Если
камера или go2rtc умеет отдавать JPEG snapshot, лучше указать отдельный
snapshot URL: это быстрее и стабильнее.

Настройка доступных длительностей роликов:

```json
{
  "camera_clip_intervals_s": [5, 10, 15, 30],
  "camera_default_clip_s": 10
}
```

Значения должны быть от 1 до 120 секунд.

### go2rtc

Если камера заведена в go2rtc, можно добавить поток так:

```text
USB камера
rtsp://homeassistant.local:8554/usb_camera
10
http://homeassistant.local:1984/api/frame.jpeg?src=usb_camera
```

Пример stream в go2rtc:

```yaml
streams:
  usb_camera: ffmpeg:device?video=/dev/v4l/by-id/usb-Sonix_Technology_Co.__Ltd._USB_2.0_Camera_SN0001-video-index0&input_format=mjpeg&video_size=1920x1080&framerate=30#video=h264#raw=-preset ultrafast -tune zerolatency
```

Для коротких MP4-клипов желательно, чтобы поток уже был H.264. Для MJPEG/USB
камер удобнее делать H.264 в go2rtc и отдавать боту RTSP-поток go2rtc.

### Ограничения камер

- Telegram Bot API не дает встроить настоящий live RTSP-плеер прямо в чат.
- Для настоящего live-просмотра обычно нужен внешний HTTPS-адрес, Mini App или
  отдельная веб-страница.
- Текущая реализация делает снимок или короткий MP4-клип.
- Обработка видео выполняется через `ffmpeg-next` и системные LibAV/FFmpeg
  библиотеки, без запуска внешней `ffmpeg` команды.
- Задачи камер ограничены по параллельности и выполняются в отдельных OS
  threads, чтобы не блокировать Tokio runtime.

## Уведомления и графики

Бот слушает события Home Assistant через WebSocket. При изменении состояния он
может отправлять уведомления подписанным пользователям, но учитывает доступы:
если устройство скрыто или уведомления отключены для пользователя, событие не
будет отправлено.

Для датчиков доступен просмотр истории и графики. В Docker-образ добавлены
шрифты и библиотеки, нужные для генерации изображений графиков.

## Админка

Админка доступна только `root_user`.

Основные разделы:

- пользователи;
- профили доступа;
- камеры по комнатам;
- настройки комнат и устройств;
- статус системы;
- backup SQLite-базы.

Экран статуса показывает:

- состояние Home Assistant;
- последний heartbeat фонового worker;
- результат последней синхронизации HA;
- число пользователей, комнат, устройств, подписок и событий;
- активные UI-сессии;
- паузы live refresh после Telegram rate limit.

## Безопасность

- Не коммитьте `.env`, `data/options.json` и SQLite-базу.
- Если токен Telegram или Home Assistant попал в чат, логи или репозиторий,
  перевыпустите его.
- `root_user` должен быть вашим Telegram user id, не username.
- Камеры могут содержать логин и пароль прямо в RTSP URL. Относитесь к базе и
  backup-файлам как к секретам.

## Отладка

Включите подробные логи:

```env
RUST_LOG=info
```

Частые проблемы:

- `HA_TOKEN not set` - не задан `HA_TOKEN` или `SUPERVISOR_TOKEN`.
- `homeassistant.local` не открывается из Docker - используйте IP адрес,
  настройте DNS или сетевой режим контейнера.
- Камера не отдает клип - проверьте, что RTSP URL доступен именно с машины или
  контейнера, где запущен бот.
- Клип не воспроизводится в Telegram - попробуйте H.264 поток через go2rtc.
- Снимок с камеры медленный - добавьте отдельный `Snapshot URL`.
- Telegram временно ограничивает редактирование сообщений - бот ставит live
  refresh на паузу и показывает это в статусе системы.

## Лицензия

Код проекта распространяется под Apache License 2.0. Полный текст находится в
файле `LICENSE`.

Проект использует `ffmpeg-next`, который линкуется с системными библиотеками
FFmpeg/LibAV. Если вы распространяете готовый бинарник или Docker-образ,
проверьте лицензионные обязательства используемой сборки FFmpeg/LibAV и
включенных кодеков.
