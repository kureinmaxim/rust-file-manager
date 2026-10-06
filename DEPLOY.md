# Развёртывание на VPS

Проверено для версии **1.3.0** 06.10.2026 по коду, `Cargo.lock` и документации
Cargo, nginx и Cloudflare. Команды рассчитаны на **Linux Ubuntu/Debian с
systemd**, Bash и доступом root или sudo.

Если приложение уже работает, начните с [обновления существующей установки](POST_DEPLOY.md).
Первичная установка ниже создаёт пользователя, каталоги и секреты; при
обновлении эти шаги повторять не нужно.

## Текущий адрес files.example.com

На предоставленном снимке DNS `files.example.com` указывает на `192.0.2.10`,
прокси Cloudflare включён. Проверка `https://files.example.com/login` вернула
HTTP 200 и страницу входа. Публичная страница входа не сообщает версию.

SSH используется через существующее подключение в Tabby. По предоставленному
выводу терминала подтверждены Linux x86_64, Rust/Cargo 1.96.0, активная
служба `rust-file-manager.service`, пользователь службы `filemgr`,
`WorkingDirectory=/var/lib/rust-file-manager` и юнит
`/etc/systemd/system/rust-file-manager.service`. На момент проверки: около
2 GiB RAM, 2 GiB swap и 9.7 GiB свободного места. Сборка рекомендуется с
одним заданием. Дополнительно подтверждены
`ExecStart=/usr/local/bin/rust-file-manager` и
`EnvironmentFile=/etc/rust-file-manager/env`. Порт SSH, фактические пути
uploads/users.json и текущая версия процесса ещё не подтверждены. Остальные настройки ниже — стандартная раскладка проекта. Для обычного SSH подключайтесь к адресу VPS или отдельному
DNS-only имени: стандартный HTTP-прокси Cloudflare порт SSH не обслуживает.

В остальных примерах `files.example.com` — домен для подстановки; для этой
установки используйте `files.example.com`.

## 0. Проверить сервер и выбрать схему HTTPS

```bash
uname -sm
free -h
df -h / /var/tmp /usr/local/bin
sudo ss -tlnp
```

Приложение слушает `127.0.0.1:8080`, наружу его отдаёт существующий reverse
proxy. Проверьте, свободен ли 8080 и кто уже занимает 80/443. Если там
работают Caddy, VPN или другие сайты, используйте их действующую схему либо
добавьте отдельный nginx-блок, не заменяя общую конфигурацию.

| Ситуация | Способ HTTPS |
|---|---|
| nginx может обслуживать 80 и 443 для домена | Let's Encrypt / Certbot, шаг 4А |
| 443 занят другим сервисом, домен через Cloudflare | Origin CA + nginx на 2083 + Origin Rule, шаг 4Б |
| Доступ через Tailscale/Headscale | [TAILSCALE.md](TAILSCALE.md); сертификат и cookie зависят от HTTP/HTTPS |

A-запись должна указывать на ваш VPS. При включённом Cloudflare-прокси
запрос идёт через Cloudflare; меняйте DNS только при изменении сервера.

## 1. Собрать и установить выбранный выпуск

Нужен **Rust 1.88 или новее**. Это максимальное заявленное требование
закреплённых зависимостей, включая Actix Web и time. На машине сборки:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config git curl ca-certificates
```

Если Rust ещё не установлен, установите его для пользователя, который
собирает приложение, по [инструкции rustup](https://rust-lang.github.io/rustup/installation/index.html):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustc --version
cargo --version
```

Если установленный Rust ниже требуемой версии, обновите выбранный toolchain
через `rustup update stable`. Если действует override на старую версию,
выберите `rustup override set stable` в новом сборочном каталоге. Рабочий
бинарник от этого не меняется.

Соберите конкретный тег в отдельном каталоге, сохраняя существующие клоны:

```bash
RFM_RELEASE=v1.3.0
RFM_COMMIT=be3cbd70b8177de0ddcaec3760de25452ac76a1b
RFM_BUILD_DIR=$(mktemp -d "$HOME/rfm-build-1.3.0.XXXXXX")
git clone --depth 1 --branch "$RFM_RELEASE" \
  https://github.com/kureinmaxim/rust-file-manager.git "$RFM_BUILD_DIR" &&
cd "$RFM_BUILD_DIR" &&
test "$(git rev-parse HEAD)" = "$RFM_COMMIT" &&
CARGO_BUILD_JOBS=1 cargo build --release --locked --bin rust-file-manager
```

`--locked` сохраняет версии из `Cargo.lock`; не выполняйте `cargo update`
при обычном развёртывании. При переходе на другой выпуск замените тег и
ожидаемый коммит. Продолжайте установку только после успешной сборки.

Шаблоны, CSS и JavaScript встроены в бинарник: отдельные файлы фронтенда на
VPS копировать не нужно. Бинарник macOS на Linux не работает; при сборке на
другой машине должны совпадать Linux-архитектура и совместимость системных
библиотек. Самый простой вариант — сборка на целевом VPS.

На VPS с малой памятью повторите сборку с `CARGO_BUILD_JOBS=1`. Если памяти
всё ещё недостаточно, используйте другую Linux-машину или отдельно
подготовьте swap с учётом файловой системы и места на диске.

Только для новой установки, где бинарника ещё нет:

```bash
sudo test ! -e /usr/local/bin/rust-file-manager &&
sudo install -o root -g root -m 0755 \
  "$RFM_BUILD_DIR/target/release/rust-file-manager" \
  /usr/local/bin/rust-file-manager
```

Для замены существующего бинарника используйте [POST_DEPLOY.md](POST_DEPLOY.md).

## 2. Создать пользователя, каталоги и настройки новой установки

Служба работает от отдельного пользователя `filemgr`:

```bash
id filemgr >/dev/null 2>&1 ||
  sudo useradd --system --user-group --home-dir /var/lib/rust-file-manager \
    --shell /usr/sbin/nologin filemgr
sudo install -d -o filemgr -g filemgr -m 0750 \
  /var/lib/rust-file-manager /var/lib/rust-file-manager/uploads
sudo install -d -o root -g root -m 0755 /etc/rust-file-manager
```

Создайте секреты **только при первичной установке**. Для пароля используйте
скрытый ввод, чтобы сам пароль не попал в историю команд:

```bash
IFS= read -r -s -p 'Пароль администратора: ' RFM_ADMIN_PASSWORD
printf '\n'
printf '%s' "$RFM_ADMIN_PASSWORD" | /usr/local/bin/rust-file-manager hash-password
unset RFM_ADMIN_PASSWORD
openssl rand -base64 64 | tr -d '\n'
printf '\n'
```

Сохраните полученные значения в `/etc/rust-file-manager/env` через локальный
редактор. Не присылайте их в чат и не добавляйте в Git. Создание файла ниже
выполнится только при его отсутствии:

```bash
sudo test ! -e /etc/rust-file-manager/env &&
sudo install -o root -g root -m 0600 /dev/null /etc/rust-file-manager/env &&
sudoedit /etc/rust-file-manager/env
```

Содержимое с заменёнными значениями:

```ini
BIND_ADDR=127.0.0.1:8080
UPLOAD_DIR=/var/lib/rust-file-manager/uploads
USERS_FILE=/var/lib/rust-file-manager/users.json
ADMIN_USERNAME=admin
ADMIN_PASSWORD_HASH='$2b$12$...вставьте-полный-хеш...'
SESSION_SECRET=вставьте-base64-строку
MAX_FILE_SIZE_MB=200
COOKIE_SECURE=true
RUST_LOG=info
```

Не используйте `source` для этого файла: его читает `EnvironmentFile`
службы. Одинарные кавычки вокруг bcrypt-хеша подходят этому формату и
обязательны при переносе значения в `.env`, который читает dotenvy.
`SESSION_SECRET` должен декодироваться минимум в 64 байта: иначе приложение
создаёт случайный ключ и вход сбрасывается после рестарта.

При HTTPS используйте `COOKIE_SECURE=true`. Для отдельного доступа по HTTP
через tailnet такая cookie не отправляется; выбор схемы описан в
[TAILSCALE.md](TAILSCALE.md). При обновлении сохраните действующие секреты.

## 3. Установить systemd-службу

Стандартный юнит — [deploy/rust-file-manager.service](deploy/rust-file-manager.service):

```bash
sudo install -o root -g root -m 0644 deploy/rust-file-manager.service \
  /etc/systemd/system/rust-file-manager.service
sudo systemctl daemon-reload
sudo systemctl enable --now rust-file-manager.service
sudo systemctl is-active rust-file-manager.service
curl --fail --silent --show-error --max-time 10 \
  -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/login
```

Ожидаются `active` и `200`. При ошибке старта сначала прочитайте журнал
службы. Юнит разрешает запись в `/var/lib/rust-file-manager`; если данные
размещены иначе, согласуйте `WorkingDirectory`, `ReadWritePaths` и
права пользователя с фактическими путями. Не копируйте этот пример поверх
настроенной службы при обычном обновлении.

## 4. Reverse proxy и HTTPS

### А. nginx обслуживает стандартные порты

Этот пример применим, если nginx может использовать 80/443. На сервере с
готовым proxy настройте отдельный виртуальный хост в его конфигурации.

```bash
sudo apt install -y nginx certbot python3-certbot-nginx
sudo cp deploy/nginx.example.conf /etc/nginx/sites-available/files.example.com
sudo sed -i \
  's/example\.com/files.example.com/g; s/client_max_body_size 200M;/client_max_body_size 201M;/' \
  /etc/nginx/sites-available/files.example.com
sudo ln -s /etc/nginx/sites-available/files.example.com \
  /etc/nginx/sites-enabled/files.example.com
sudo nginx -t && sudo systemctl reload nginx
sudo certbot --nginx -d files.example.com
sudo certbot renew --dry-run
```

Замените `files.example.com` в командах на свой домен. Не удаляйте
`sites-enabled/default` и другие сайты без проверки, для чего они нужны.
Перед Certbot сохраните nginx-конфигурацию; плагин меняет её для HTTPS.

### Б. 443 занят, используется Cloudflare

Пошаговая схема — [GUIDE_cloudflare.md](GUIDE_cloudflare.md): Origin CA,
nginx на 2083, Origin Rule для нужного hostname, SSL/TLS **Full (strict)**.
Cloudflare поддерживает HTTPS на 2083. Настройте лимит nginx с запасом по
шагу 5. При обновлении приложения уже работающие сертификаты и правила
Cloudflare заново создавать не нужно.

## 5. Лимиты загрузки и сеть

У приложения лимит **на файл**, у nginx `client_max_body_size` — **на тело
запроса**, включая multipart-разметку. В интерфейсе 1.3.0 файлы очереди
отправляются отдельными запросами. Для `MAX_FILE_SIZE_MB=200` практический
пример nginx — `client_max_body_size 201M;`; для стороннего клиента,
передающего несколько файлов одним запросом, учитывайте их суммарный размер.
После изменения nginx проверяйте конфигурацию и выполняйте reload.

Cloudflare Free/Pro ограничивает запросы 100 MB, Business — 200 MB; настройка
зоны может быть ниже. Поэтому пользовательский лимит 500 MB не означает,
что такой файл пройдёт через Cloudflare. Крупные файлы можно отправлять по
защищённому доступу через tailnet; смену схемы публичного доступа выполняйте
отдельно. Интерфейс не разбивает один файл на части.

При настройке firewall сначала проверьте существующие правила и фактический
порт SSH. Разрешите порт SSH и только порты выбранной схемы (80/443 либо
2083). Порт приложения 8080 оставляйте на localhost. На работающем VPS
обновление бинарника не требует повторного `ufw enable` или замены правил.

## 6. Проверить установку

```bash
sudo systemctl status rust-file-manager.service --no-pager
sudo journalctl -u rust-file-manager.service --no-pager -o cat |
  grep 'starting server' | tail -1
curl --fail --silent --show-error --max-time 20 \
  -o /dev/null -w '%{http_code}\n' https://files.example.com/login
```

В строке запуска ожидаются `version="1.3.0"` и `commit="be3cbd7"`, публичный
`/login` должен вернуть 200. Затем войдите и проверьте футер, зоны файлов,
тестовую загрузку/скачивание и папки бэкапов. Параметр `--version` в бинарнике
не реализован: для версии используйте журнал запуска и футер после входа.

Если вход не работает, проверьте полноту bcrypt-хеша в локальном редакторе
и работу secure-cookie через HTTPS. Пароль не вставляйте в `curl -d` или
командную строку: он попадёт в историю и может быть виден другим процессам.

---

# Многопользовательский режим

У каждого пользователя есть **личная зона** (видна только ему) и **общая
зона** для обмена файлами между всеми участниками.

## Как пригласить человека

1. Войдите как администратор.
2. В блоке «Пользователи» нажмите **«Создать ссылку-приглашение»**.
3. Отправьте ссылку человеку (мессенджером, почтой — как удобно).
4. Он откроет ссылку, придумает имя и пароль — аккаунт создаётся сразу.

Свойства ссылки:

- **одноразовая** — после регистрации перестаёт действовать;
- **истекает через 7 дней**, если не использована;
- токен — 256 бит случайности, подобрать его нельзя.

## Зоны и права

| Зона | Кто видит | Кто может загружать/удалять |
|---|---|---|
| 🔒 Мои файлы (`home/<имя>/`) | только владелец | только владелец |
| 👥 Общие файлы (`shared/`) | все вошедшие | все вошедшие |

- Доступ к чужой личной зоне невозможен ни по прямой ссылке, ни через
  манипуляции с путём: имя папки берётся из сессии, а не из URL, и каждый
  сегмент пути валидируется.
- Без входа не отдаётся ни один файл (включая общие).
- Администратор управляет пользователями, но через веб-интерфейс чужие личные
  файлы тоже не видит (на сервере они доступны ему как root — это надо
  понимать).

## Управление пользователями

- Список пользователей и кнопка удаления — в блоке «Пользователи» у админа.
- Удаление пользователя **удаляет и его личную папку** со всеми файлами.
- Сессия удалённого пользователя перестаёт действовать немедленно.
- Учётные записи хранятся в `users.json` (`USERS_FILE`), пароли — только в
  виде bcrypt-хешей. Файл стоит включить в бэкап.

## Структура хранилища

```
uploads/
├── shared/              # общая зона
│   ├── Фото/ Документы/ ...
│   └── Бэкапы/
│       ├── Серверы/ HA/ Project/
└── home/
    ├── alice/           # личная зона alice
    │   └── Фото/ Бэкапы/ ...
    └── bob/             # личная зона bob
```

Обычные категории заполняются автоматически по расширению файла. Раздел
«Бэкапы» (папки Серверы / HA / Project) — только вручную: при загрузке
выберите нужную папку в селекторе «Категория». Папки бэкапов есть и в общей
зоне, и в личной зоне каждого пользователя.

При обновлении со старой (однопользовательской) версии существующие папки
категорий автоматически переносятся в `shared/` при первом старте.

## Обновление и переустановка

- Обновление работающего приложения до **1.3.0**, резервная копия, атомарная
  замена бинарника и откат: [POST_DEPLOY.md](POST_DEPLOY.md).
- Переустановка программы с сохранением данных использует тот же порядок:
  собрать нужный выпуск, установить бинарник, сохранить существующие env,
  `users.json`, uploads, пользователя и службу. Не повторять генерацию
  секретов и создание пустого env.
- Развёртывание с нуля с пустым хранилищем — отдельная операция. Удаление
  текущих пользователей и файлов не является частью обновления.

## Источники и границы проверки

- [Cargo: параметры сборки, --locked и --jobs](https://doc.rust-lang.org/cargo/commands/cargo-build.html).
- [nginx: client_max_body_size](https://nginx.org/en/docs/http/ngx_http_core_module.html#client_max_body_size).
- [Certbot: плагин nginx](https://eff-certbot.readthedocs.io/en/stable/using.html#nginx).
- [Cloudflare: поддерживаемые порты](https://developers.cloudflare.com/fundamentals/reference/network-ports/).
- [Cloudflare: лимиты запросов и ошибка 413](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/4xx-client-error/error-413/).

Команды сверены с проектом и их shell-синтаксис проверен. Полная установка
на чистом Linux и обновление рабочего VPS по этой инструкции пока не
выполнялись; фактические пути и SSH-доступ проверяются перед применением.
