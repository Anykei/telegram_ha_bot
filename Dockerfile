FROM lukemathwalker/cargo-chef:latest-rust-1.87-slim-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- ЭТАП 2: Builder (Сборка) ---
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Устанавливаем зависимости для компиляции (включая шрифты и ssl)
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    libfreetype6-dev \
    libfontconfig1-dev \
    && rm -rf /var/lib/apt/lists/*

# Сборка кэша зависимостей
RUN cargo chef cook --release --recipe-path recipe.json

# Сборка самого бота
COPY . .
RUN cargo build --release --bin telegram_ha_bot

# --- ЭТАП 3: Runtime (Финальный образ) ---
FROM debian:bookworm-slim AS runtime

# 1. Устанавливаем tini и системные библиотеки
# 2. Очищаем кэш apt сразу после установки
RUN apt-get update && apt-get install -y \
    tini \
    ca-certificates \
    libssl3 \
    libsqlite3-0 \
    libfreetype6 \
    libfontconfig1 \
    libc6-dev \
    fonts-dejavu \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Копируем бинарник из билдера
COPY --from=builder /app/target/release/telegram_ha_bot /app/bot

# Создаем пользователя, чтобы не запускать от root (Security Best Practice)
# При работе в HA контейнере надо выключать botuser
# RUN useradd -m botuser
# USER botuser

# Переменные окружения для Rust логов и SQLite (путь /data критичен для HA)
ENV RUST_LOG=info

ENV MIGRATIONS_PATH="./migrations"

ENV OPTIONS_PATH="/data/options.json"
ENV DATABASE_URL="/data/bot_data.db"

# HA_TOKEN генерирует HA при запуске контейнера
# ENV HA_TOKEN=""
ENV HA_URL="http://supervisor/core"

# Используем tini как точку входа.
ENTRYPOINT ["/usr/bin/tini", "--", "/app/bot"]