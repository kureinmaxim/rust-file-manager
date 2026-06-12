# Развёртывание на VPS

Пошаговая инструкция: бинарник + systemd + nginx + HTTPS. Везде ниже
используется домен-плейсхолдер `files.example.com` — подставьте свой.

Приложение слушает только `127.0.0.1:8080`, наружу его отдаёт nginx.

Если сервер состоит в сети Tailscale/Headscale, доступ можно организовать и
через неё, минуя публичный интернет — см. [TAILSCALE.md](TAILSCALE.md).

## 0. DNS

A-запись `files.example.com` должна указывать на IP вашего VPS.

Если DNS обслуживается Cloudflare с включённым прокси (оранжевое облако) —
прочитайте [GUIDE_cloudflare.md](GUIDE_cloudflare.md) **до** шага 4: выбор
способа HTTPS зависит от того, свободен ли на сервере порт 443. Также учтите,
что бесплатный план Cloudflare ограничивает загрузку **100 МБ на запрос**.

## 1. Сборка и установка бинарника

На VPS (Ubuntu/Debian):

```bash
sudo apt update && sudo apt install -y build-essential pkg-config git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust 1.75+
source "$HOME/.cargo/env"

git clone https://github.com/kureinmaxim/rust-file-manager.git
cd rust-file-manager
cargo build --release
sudo cp target/release/rust-file-manager /usr/local/bin/
```

Шаблоны вшиты в бинарник — на сервере нужен только он (можно собрать на своей
машине под Linux x86_64 и скопировать по `scp`).

> **Мало памяти?** На VPS с 1–2 ГБ RAM без swap линковка release-сборки
> (LTO) может упасть по памяти. Добавьте временный swap:
> `fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile`
> — а после сборки уберите: `swapoff /swapfile && rm /swapfile`.

## 2. Пользователь, директории, секреты

```bash
sudo useradd --system --home /var/lib/rust-file-manager filemgr
sudo mkdir -p /var/lib/rust-file-manager/uploads
sudo chown -R filemgr:filemgr /var/lib/rust-file-manager

sudo mkdir -p /etc/rust-file-manager
```

Сгенерируйте хеш пароля администратора и секрет сессий:

```bash
echo 'ваш-пароль-админа' | rust-file-manager hash-password
openssl rand -base64 64 | tr -d '\n'; echo
```

> Пароль придумывайте длинный (16+ символов): сервис смотрит в интернет,
> ограничителя попыток входа в приложении нет — защита целиком на стойкости
> пароля. Удобно сгенерировать: `openssl rand -base64 16`.

Создайте `/etc/rust-file-manager/env`:

```ini
BIND_ADDR=127.0.0.1:8080
UPLOAD_DIR=/var/lib/rust-file-manager/uploads
USERS_FILE=/var/lib/rust-file-manager/users.json
ADMIN_USERNAME=admin
ADMIN_PASSWORD_HASH='$2b$12$...вставьте-хеш...'
SESSION_SECRET=вставьте-base64-строку
MAX_FILE_SIZE_MB=200
COOKIE_SECURE=true
```

Три критичных момента:

- хеш пароля — **обязательно в одинарных кавычках**, иначе `$2b$12$...`
  обрежется при подстановке переменных (приложение это проверит и откажется
  стартовать);
- длинный хеш удобнее вписывать не руками, а командой —
  `NEW_HASH=$(echo 'пароль' | rust-file-manager hash-password)` и затем
  `sed -i "s|^ADMIN_PASSWORD_HASH=.*|ADMIN_PASSWORD_HASH='${NEW_HASH}'|" /etc/rust-file-manager/env` —
  при копировании из терминала строка легко рвётся;
- `COOKIE_SECURE=true` ставьте при работе по HTTPS. Если планируете заходить
  и по http (например, через Tailscale по IP) — оставьте `false`, иначе вход
  по http работать не будет.

```bash
sudo chmod 600 /etc/rust-file-manager/env
```

## 3. systemd

Готовый юнит лежит в `deploy/rust-file-manager.service`:

```bash
sudo cp deploy/rust-file-manager.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rust-file-manager
systemctl status rust-file-manager        # active (running)
curl -sI http://127.0.0.1:8080/login | head -1   # HTTP/1.1 200 OK
```

## 4. nginx + HTTPS

**Сначала проверьте, кто занимает порт 443:**

```bash
sudo ss -tlnp | grep ':443'
```

### Вариант А — порт 443 свободен (обычный сервер)

Классическая схема с Let's Encrypt:

```bash
sudo apt install -y nginx certbot python3-certbot-nginx
sudo cp deploy/nginx.example.conf /etc/nginx/sites-available/files.example.com
sudo sed -i 's/example.com/files.example.com/' /etc/nginx/sites-available/files.example.com
sudo ln -s /etc/nginx/sites-available/files.example.com /etc/nginx/sites-enabled/
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t && sudo systemctl reload nginx

sudo certbot --nginx -d files.example.com
```

### Вариант Б — порт 443 занят другим сервисом

Типичная ситуация, если на том же VPS живёт VPN/прокси-транспорт (Caddy,
Xray, Hysteria и т.п.), которому нужен 443. **Не запускайте certbot --nginx** —
он попытается добавить nginx-листенер на 443 и сломает тот сервис.

Вместо этого HTTPS терминируется на Cloudflare, а nginx поднимает TLS на
альтернативном порту (2083) с бесплатным Origin CA сертификатом — полная
пошаговая инструкция: [GUIDE_cloudflare.md](GUIDE_cloudflare.md).

В обоих вариантах держите `client_max_body_size` в nginx равным
`MAX_FILE_SIZE_MB` приложения.

## 5. Файрвол

```bash
sudo ufw allow OpenSSH
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp        # вариант А
sudo ufw allow 2083/tcp       # вариант Б (Cloudflare-порт)
sudo ufw enable
```

Порт 8080 наружу не открывайте — приложение слушает только localhost.

## 6. Проверка

Откройте `https://files.example.com` — страница входа, логин `admin` + ваш
пароль. Логи: `journalctl -u rust-file-manager -f`.

Если вход говорит «Неверное имя пользователя или пароль», а пароль точно
верный — почти наверняка повреждён хеш в env-файле (см. §2, способ с `sed`).
Проверить можно прямо на сервере, минуя браузер:

```bash
curl -s -X POST http://127.0.0.1:8080/login -d 'username=admin&password=ВАШ-ПАРОЛЬ'
```

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

## Обновление приложения

Коротко:

```bash
cd ~/rust-file-manager && git pull
cargo build --release
sudo cp target/release/rust-file-manager /usr/local/bin/
sudo systemctl restart rust-file-manager
```

Полный runbook с проверками версий, бэкапом бинарника, откатом и разбором
частных случаев — [POST_DEPLOY.md](POST_DEPLOY.md).
