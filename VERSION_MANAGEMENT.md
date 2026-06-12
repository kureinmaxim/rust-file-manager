# VERSION MANAGEMENT

Этот документ — про **управление версией приложения**, **датой релиза**, **build metadata** (git/время сборки) и короткие команды для rust-file-manager.

---

## Источник правды (обязательно)

Единственный источник правды версии и даты релиза — корневой `Cargo.toml`:

```toml
[package]
version = "X.Y.Z"

[package.metadata.release]
date = "DD.MM.YYYY"
```

Важно:
- Версию **не редактируем руками** — только через `cargo xtask` (см. ниже), иначе derived-файлы рассинхронизируются.
- Никакие пользовательские/локальные конфиги (`.env`, файлы на сервере) версию не задают — иначе возможны «понижения версии» при переносе между машинами.

---

## Что синхронизируем автоматически (derived files)

После изменения версии инструмент сам обновляет:

- `README.md` → строка «**Текущая версия:** X.Y.Z (DD.MM.YYYY)»
- `Cargo.lock` → запись пакета `rust-file-manager`
- дата релиза в `[package.metadata.release]` → ставится «сегодня»

Версия в самом приложении (футер UI, лог старта) приходит из `Cargo.toml` автоматически при сборке (`env!("CARGO_PKG_VERSION")`) — её синхронизировать не нужно.

---

## CLI справочник: `cargo xtask`

Инструмент — workspace-член `xtask/` (чистый Rust, без зависимостей); алиас прописан в `.cargo/config.toml`.

### Использование

```bash
cargo xtask <command> [args]
```

### Команды

```text
status                   Показать версию, дату релиза, статус README и git describe
sync                     Обновить дату релиза на сегодня и derived-файлы (версия не меняется)
set X.Y.Z                Установить явную версию + sync
bump patch               1.0.0 → 1.0.1 (+ sync)
bump minor               1.0.0 → 1.1.0 (+ sync)
bump major               1.0.0 → 2.0.0 (+ sync)
release [X.Y.Z]          sync + git commit «Release vX.Y.Z» + тег vX.Y.Z + push (с тегом)
```

### Примеры

```bash
cargo xtask status
cargo xtask bump patch
cargo xtask release          # релиз текущей версии
cargo xtask release 2.0.0    # установить версию и сразу зарелизить
```

---

## Build metadata для UI (футер)

Во время сборки `build.rs` зашивает в бинарник:

- `BUILD_GIT_COMMIT` — короткий хэш коммита (+ суффикс `-dirty`, если были незакоммиченные изменения)
- `BUILD_GIT_BRANCH` — ветка
- `BUILD_DATE` — дата/время сборки (UTC)

Где это видно:

- футер главной страницы: `rust-file-manager v1.0.0 · коммит a1b2c3d · сборка 2026-06-12 10:00 UTC`
- лог при старте демона: `starting server version=... commit=... built=...`

Если бинарник собирался вне git-репозитория, поля показываются как `unknown` — это нормально.

---

## Ежедневный workflow (dev)

### Проверить, что версия везде синхронизирована

```bash
cargo xtask status
```

### Привести всё к Cargo.toml (если есть рассинхрон)

```bash
cargo xtask sync
```

### Релиз (bump + коммит + тег + push)

```bash
cargo xtask bump patch      # или minor/major — поднимет версию и derived-файлы
cargo build --release       # убедиться, что собирается
cargo test
cargo xtask release         # коммит «Release vX.Y.Z», тег, push
```

`release` коммитит **только** `Cargo.toml`, `Cargo.lock` и `README.md` — остальные изменения должны быть закоммичены до этого.

---

## Семантика версий (коротко)

- **patch** — исправления без изменения поведения (багфиксы, документация);
- **minor** — новая функциональность, обратная совместимость сохранена;
- **major** — ломающие изменения (формат `.env`, маршруты, схема хранения).

---

## Быстрый troubleshooting

### «В футере коммит с суффиксом -dirty»

Бинарник собран при незакоммиченных изменениях. Закоммитьте и пересоберите — суффикс исчезнет.

### «В футере версия не совпадает с ожидаемой»

Проверьте:
- `Cargo.toml` (source of truth) и `cargo xtask status`;
- что бинарник пересобран **после** bump/sync и на сервер скопирован именно новый `target/release/rust-file-manager`;
- что systemd-сервис перезапущен: `sudo systemctl restart rust-file-manager`.

### «cargo xtask: command not found / no such subcommand»

Алиас живёт в `.cargo/config.toml` в корне проекта — команда работает только из каталога проекта (или подкаталогов).

### «README показывает DESYNC»

Кто-то поправил версию руками. Выполните `cargo xtask sync` — он перепишет строку «Текущая версия» из `Cargo.toml`.
