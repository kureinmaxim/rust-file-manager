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

Приложение читает настройки из файла `.env` — обычного текстового файла вида «ключ=значение» в каталоге проекта. В репозитории лежит шаблон `.env.example`; из него нужно сделать свой `.env` и заполнить два секретных значения. По шагам:

### 1. Сборка

```bash
cargo build --release
```

### 2. Создать файл настроек из шаблона

```bash
cp .env.example .env
```

### 3. Сгенерировать хэш пароля администратора

Приложение не хранит пароль открытым текстом — только его bcrypt-хэш (необратимую «свёртку»). Придумайте пароль и получите хэш встроенной командой:

```bash
echo 'придумайте-тут-свой-пароль' | ./target/release/rust-file-manager hash-password
```

Команда напечатает строку вида `$2b$12$Xy9z...` — скопируйте её в `.env` в поле `ADMIN_PASSWORD_HASH`. Сам пароль вы будете вводить на странице входа, а приложение сверит его с хэшем.

### 4. Сгенерировать ключ сессий

`SESSION_SECRET` — длинная случайная строка, которой подписываются cookie сессий. Придумывать её не нужно:

```bash
openssl rand -base64 64 | tr -d '\n'; echo
```

Результат скопируйте в `.env` в поле `SESSION_SECRET`. Если ключ не задать, при каждом перезапуске сервера он создаётся заново и все сессии сбрасываются — придётся входить снова.

### 5. Проверить итоговый `.env`

Откройте `.env` в редакторе (`nano .env`) — он должен выглядеть так:

```
BIND_ADDR=127.0.0.1:8080
UPLOAD_DIR=uploads
MAX_FILE_SIZE_MB=200
ADMIN_USERNAME=admin
ADMIN_PASSWORD_HASH=$2b$12$сюда-вставили-результат-шага-3
SESSION_SECRET=сюда-вставили-результат-шага-4
COOKIE_SECURE=false
RUST_LOG=info
```

Значения вставляйте как есть, без кавычек — символы `$` в хэше внутри `.env` безопасны.

### 6. Запуск

```bash
./target/release/rust-file-manager
```

Откройте http://127.0.0.1:8080 — увидите страницу входа. Логин — `admin` (или ваш `ADMIN_USERNAME`), пароль — тот, что вы придумали на шаге 3.

> ⚠️ Файл `.env` содержит секреты: он уже добавлен в `.gitignore` и не должен попадать в git, чаты или скриншоты.

## Смена пароля администратора

Пароль нигде не хранится — хранится только его хэш, поэтому «смена пароля» — это генерация нового хэша и замена старого в настройках.

1. Сгенерируйте хэш нового пароля:

   ```bash
   echo 'новый-пароль' | ./target/release/rust-file-manager hash-password
   ```

2. Откройте файл настроек и замените значение `ADMIN_PASSWORD_HASH` на новый хэш:
   - при локальном запуске — `.env` в каталоге проекта;
   - на сервере (если ставили по инструкции ниже) — `/etc/rust-file-manager/env`.

3. Перезапустите приложение, чтобы оно перечитало настройки:

   ```bash
   # локально: остановите (Ctrl+C) и запустите снова
   ./target/release/rust-file-manager

   # на сервере
   sudo systemctl restart rust-file-manager
   ```

После перезапуска старый пароль перестаёт действовать, активные сессии остаются живы до истечения срока (12 часов). Если нужно немедленно разлогинить все устройства — заодно замените `SESSION_SECRET` на новый (`openssl rand -base64 64 | tr -d '\n'`): подпись старых cookie станет недействительной, и все сессии сбросятся сразу.

Таким же образом меняется и имя пользователя — переменная `ADMIN_USERNAME`.

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
