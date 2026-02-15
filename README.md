219791289# Home Assistant Telegram Bot 🏠📱

A high-performance asynchronous Telegram bot powered by Rust that provides seamless integration with Home Assistant, enabling real-time control and monitoring of smart home devices through Telegram.

## Features

### 🔌 Home Assistant Integration
- **WebSocket Connection** (tokio-tungstenite) for real-time event streaming
- **REST API** (reqwest) for device control and state queries
- **Persistent Connection** with exponential backoff reconnection logic
- **Auto-discovery** of devices from HA with configurable visibility

### 📱 Telegram Interface
- **Interactive Buttons** for multi-modal room and device control
- **Live State Updates** — UI refreshes when HA devices change state
- **Session Management** — persistent user state and menu history
- **Settings Panel** — user-customizable device visibility and notifications

### 📊 Analytics & Visualization
- **Sensor History Charts** — plotters-based PNG rendering of sensor data over time
- **Event Logging** — SQLite database tracking device state changes
- **Activity Timestamps** — track user interactions and device events

### 🎥 Video Processing (Extensible)
- **FFmpeg Integration** — `tokio::process` for video stream management
- **H.264 Encoding** — Telegram-optimized MP4 output
- **Concurrency Control** — bounded task queue prevents resource exhaustion

### ⚡ Performance & Reliability
- **Bounded Event Queue** (capacity=32) — prevents unbounded task spawning
- **Worker Pool Pattern** — fixed number of async workers processing notifications
- **Type-Safe Database** (sqlx) — compile-time SQL validation
- **Graceful Degradation** — full/queue events are logged, not silently dropped

### 🔒 Security & Access Control
- **User Whitelist** — database-driven permission model
- **Root Admin** — designated super-user with unrestricted access
- **Entity Subscriptions** — per-user notification subscriptions
- **Hidden Entities** — user controls device visibility in Control mode

## Architecture

```
telegram_ha_bot/
├── src/
│   ├── main.rs                 # Application entry point & setup
│   ├── models.rs               # AppConfig, shared state types
│   ├── config.rs               # Environment variable loading
│   ├── options.rs              # options.json parsing
│   │
│   ├── ha/                      # Home Assistant integration
│   │   ├── client.rs           # REST API wrapper (history, template, service calls)
│   │   ├── event_listener.rs   # WebSocket real-time event stream
│   │   └── models.rs           # HA data structures (Entity, NotifyEvent)
│   │
│   ├── bot/                     # Telegram bot layer (teloxide)
│   │   ├── handlers.rs         # Command & callback handlers
│   │   ├── router.rs           # Payload encoding/routing logic
│   │   ├── models.rs           # UI View structure
│   │   ├── notification.rs     # Notification dispatcher
│   │   └── screens/            # UI screen renderers
│   │       ├── home.rs
│   │       ├── room.rs
│   │       ├── control/
│   │       ├── settings/
│   │       └── admin/
│   │
│   ├── core/                    # Business logic & orchestration
│   │   ├── devices.rs          # Device control, state logic
│   │   ├── notification.rs     # Event processing & task distribution (bounded queue)
│   │   ├── presentation.rs     # State formatting & localization
│   │   ├── maintenance.rs      # Background tasks (cache updates)
│   │   └── types.rs            # Domain types
│   │
│   ├── db/                      # SQLite data layer (sqlx)
│   │   ├── devices.rs
│   │   ├── rooms.rs
│   │   ├── subscriptions.rs    # Visibility & notification settings
│   │   ├── user.rs
│   │   ├── device_event_log.rs
│   │   └── models.rs
│   │
│   ├── charts/                  # Data visualization (plotters)
│   │   └── mod.rs
│   │
│   └── video_engine/            # Video processing (FFmpeg)
│       └── mod.rs              # VideoProcessor, concurrency control
│
├── migrations/                  # SQLx database migrations
│   ├── 20260109120000_init.sql
│   ├── 20260109120001_add_table_rooms.sql
│   ├── 20260109120002_add_table_devices.sql
│   └── 20260115140000_add_hide_column.sql
│
├── Dockerfile                   # Container configuration
├── Cargo.toml                   # Dependencies
└── .env                         # Configuration (tokens, URLs)
```

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| **Async Runtime** | tokio 1.0 | Non-blocking I/O, task spawning |
| **WebSocket** | tokio-tungstenite 0.28 | HA real-time event stream |
| **HTTP Client** | reqwest 0.13 | HA REST API calls |
| **Telegram Bot** | teloxide 0.17 | Bot framework & API |
| **State Management** | dashmap 7.0 | Thread-safe in-memory cache |
| **Database** | sqlx 0.8 + SQLite | Type-safe SQL, migrations |
| **Serialization** | serde 1.0 | JSON/binary encoding |
| **Visualization** | plotters 0.3 | Chart rendering |
| **Error Handling** | anyhow 1.0 | Ergonomic error propagation |

## Installation & Setup

### Prerequisites
- Rust 1.70+
- Home Assistant instance with API token
- Telegram Bot token (from [@BotFather](https://t.me/botfather))
- FFmpeg (for video processing)

### 1. Clone Repository
```bash
git clone https://github.com/yourusername/telegram_ha_bot.git
cd telegram_ha_bot
```

### 2. Configure Environment

Create `.env` file:
```bash
RUST_LOG=info
OPTIONS_PATH="data/options.json"
MIGRATIONS_PATH="./migrations"
DATABASE_PATH="data/bot_data.db"
HA_TOKEN="your_ha_long_lived_token_here"
HA_URL="http://homeassistant.local:8123/"
```

Create `data/options.json`:
```json
{
  "bot_token": "your_telegram_bot_token_here",
  "root_user": "your_telegram_user_id"
}
```

### 3. Build & Run

**Development:**
```bash
cargo run
```

**Release (optimized):**
```bash
cargo build --release
./target/release/telegram_ha_bot
```

**Docker:**
```bash
docker build -t telegram-ha-bot .
docker run --env-file .env -v $(pwd)/data:/app/data telegram-ha-bot
```

## Usage

### Bot Commands
| Command | Description |
|---------|-------------|
| `/start` | Show main menu (rooms/devices) |

### Navigation
1. **Home** → Select a room
2. **Room View** (Control mode) → Toggle devices, view state
3. **Room View** (Settings mode) → Configure notifications, visibility
4. **Device Settings** → Rename, hide/show, subscribe to events

### Device Visibility
- **New devices** auto-hidden by default (user must explicitly enable)
- **Control Mode** → Shows only visible devices for quick toggling
- **Settings Mode** → All devices visible for configuration
- **Toggle Hide** → Easy one-click visibility switching

### Notifications
- **Per-Device Subscription** — users choose which state changes trigger alerts
- **Live UI Refresh** — watching users see real-time state updates
- **Event Queue** — bounded (capacity=32) prevents memory exhaustion
- **Graceful Degradation** — full queue events logged, not silently dropped

## Performance & Reliability

### Event Processing
- **Bounded Queue**: Max 32 queued events (prevents unbounded task growth)
- **Worker Pool**: Single async worker processes events sequentially
- **Backpressure**: Overfull queue events logged with `warn!` level
- **Example**: 100 HA events/sec → queue fills → new events dropped + logged

### WebSocket Reconnection
- **Exponential Backoff**: 500ms → 1s → 2s → ... → 30s (max)
- **Reset on Success**: Backoff resets after successful connection
- **Heartbeat Handling**: Empty frames logged as `debug`, not `warn`

### Database
- **Type-Safe Queries**: sqlx compile-time validation prevents SQL errors
- **Migrations**: Automatic schema management on startup
- **Error Handling**: Explicit `Result<bool>` instead of `unwrap()` hiding errors

## Configuration

### Hidden Entities (Visibility Control)

**Default behavior:**
```sql
-- New device discovered
INSERT INTO hidden_entities (entity_id, hide) VALUES ('light.kitchen', 1);
-- hide=1 → hidden in Control mode
```

**User toggle:**
```rust
await toggle_hidden(pool, "light.kitchen")
// If hide=1 → UPDATE to hide=0 (visible)
// If hide=0 → UPDATE to hide=1 (hidden)
```

### Notification Subscriptions

**Per-entity subscriber list:**
```sql
SELECT user_id FROM subscriptions WHERE entity_id = 'sensor.temperature';
```

**Toggle subscription:**
```rust
await toggle_subscription(pool, user_id, entity_id)
// If subscribed → DELETE (unsubscribe)
// If not subscribed → INSERT (subscribe)
```

## Logging

Set via `RUST_LOG` environment variable:

```bash
# Info level (default)
RUST_LOG=info

# Debug level (verbose)
RUST_LOG=debug

# Module-specific
RUST_LOG=telegram_ha_bot::ha=debug,telegram_ha_bot::bot=info
```

## Troubleshooting

### WebSocket Connection Issues
```
WARNING: Connection to WS failed: Connection refused. Retrying in 500ms...
```
**Solution:** Ensure HA instance is running and `HA_URL` is correct.

### Empty WebSocket Frames
```
DEBUG: Received empty WebSocket frame (heartbeat)
```
**Expected behavior** — HA sends heartbeat pings to keep connection alive.

### All Devices Hidden
1. Check `hidden_entities` table: `SELECT * FROM hidden_entities;`
2. Run `UPDATE hidden_entities SET hide = 0;` to unhide all
3. Toggle individual devices in Settings mode

### Permission Denied
```
Error: User 123456789 not in whitelist
```
**Solution:** Add user to database or set as `root_user` in `options.json`.

## Contributing

Contributions welcome! Please:
1. Fork repository
2. Create feature branch (`git checkout -b feature/amazing-feature`)
3. Commit changes (`git commit -m 'Add amazing feature'`)
4. Push to branch (`git push origin feature/amazing-feature`)
5. Open Pull Request

## License

Licensed under the Apache License 2.0. See [LICENSE](LICENSE) file for details.

---

**Made with ❤️ for Home Assistant enthusiasts**

Questions? Issues? Open a GitHub issue or contact the maintainers.
