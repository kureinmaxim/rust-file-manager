# Rust File Manager

Лёгкий self-hosted файловый менеджер на Rust (Actix Web): загрузка, скачивание, переименование и удаление файлов через веб-интерфейс. Файлы автоматически раскладываются по категориям (Фото, Документы, Программы, Видео, Другие) по расширению.

## Возможности

- 🔐 Вход по логину/паролю (bcrypt-хэш, cookie-сессии с подписью, `SameSite=Strict`, `HttpOnly`)
- 📤 Загрузка файлов с ограничением размера и автокатегоризацией
- 📂 Просмотр и скачивание по категориям, общий объём хранилища
- ✏️ Переименование и удаление с защитой от path traversal
- ⚙️ Вся конфигурация через переменные окружения / `.env`
- 📝 Структурированные логи (`tracing`)

## Быстрый старт

```bash
# 1. Сборка
cargo build --release

# 2. Сгенерировать хэш пароля администратора
echo 'мой-надёжный-пароль' | ./target/release/rust-file-manager hash-password

# 3. Настроить окружение
cp .env.example .env
# впишите ADMIN_PASSWORD_HASH и SESSION_SECRET (openssl rand -base64 64)

# 4. Запуск
./target/release/rust-file-manager
```

Откройте http://127.0.0.1:8080 — увидите страницу входа.

## Конфигурация

| Переменная | По умолчанию | Назначение |
|---|---|---|
| `BIND_ADDR` | `127.0.0.1:8080` | Адрес/порт приложения |
| `UPLOAD_DIR` | `uploads` | Каталог хранения файлов |
| `MAX_FILE_SIZE_MB` | `200` | Лимит размера одного файла |
| `ADMIN_USERNAME` | `admin` | Имя пользователя |
| `ADMIN_PASSWORD_HASH` | — (обязательно) | bcrypt-хэш пароля |
| `SESSION_SECRET` | случайный при старте | base64 от ≥64 случайных байт |
| `COOKIE_SECURE` | `false` | `true` при работе по HTTPS |
| `RUST_LOG` | `info` | Уровень логирования |

## Деплой на VPS

Примеры конфигов лежат в [`deploy/`](deploy/):

- [`deploy/rust-file-manager.service`](deploy/rust-file-manager.service) — systemd-юнит (отдельный непривилегированный пользователь, sandbox-директивы);
- [`deploy/nginx.example.conf`](deploy/nginx.example.conf) — reverse proxy через nginx.

Кратко:

```bash
# на сервере
sudo useradd --system --create-home --home /var/lib/rust-file-manager filemgr
sudo cp target/release/rust-file-manager /usr/local/bin/
sudo mkdir -p /etc/rust-file-manager
sudo cp .env /etc/rust-file-manager/env && sudo chmod 600 /etc/rust-file-manager/env
sudo cp deploy/rust-file-manager.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now rust-file-manager
```

При работе за nginx по HTTPS установите `COOKIE_SECURE=true`.

## Безопасность

- Пароль нигде не хранится в открытом виде — только bcrypt-хэш в окружении.
- Все маршруты, кроме `/login`, требуют авторизованной сессии.
- Имена файлов и категории валидируются на сервере: запрещены `..`, разделители путей и NUL — выйти за пределы `UPLOAD_DIR` нельзя.
- Cookie сессии: `HttpOnly`, `SameSite=Strict`, подпись секретным ключом; срок жизни 12 часов.
- Мутирующие запросы защищены от CSRF политикой `SameSite=Strict`.

## Разработка

```bash
cargo test          # юнит-тесты (категоризация, санитизация имён)
cargo clippy        # линтер
RUST_LOG=debug cargo run
```

Структура:

```
src/
  main.rs        # bootstrap: конфиг, маршруты, middleware
  config.rs      # AppConfig из переменных окружения
  auth.rs        # вход/выход, guard-middleware сессий
  files.rs       # index, upload, delete, rename
  categories.rs  # категории, санитизация имён файлов (+ тесты)
templates/       # HTML (встраивается в бинарник при сборке)
deploy/          # примеры systemd / nginx
```

## Лицензия

[MIT](LICENSE)
