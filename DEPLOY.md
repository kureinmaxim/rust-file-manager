# Развёртывание на VPS (hip.kurein.me)

Пошаговая инструкция: бинарник + systemd + nginx + HTTPS (Let's Encrypt) за Cloudflare.

Приложение слушает только `127.0.0.1:8080`, наружу его отдаёт nginx по HTTPS.

Если сервер состоит в сети Tailscale/Headscale, доступ можно организовать и
через неё, минуя публичный интернет — см. [TAILSCALE.md](TAILSCALE.md).

## 0. DNS (Cloudflare)

A-запись `hip.kurein.me` должна указывать на IP вашего VPS. Прокси Cloudflare
(оранжевое облако) оставить можно, но учтите:

- в разделе **SSL/TLS** поставьте режим **Full (strict)** после выпуска сертификата (шаг 4);
- бесплатный план Cloudflare ограничивает загрузку **100 МБ на запрос** — если
  нужны файлы больше, переключите запись на «DNS only».

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

Два критичных момента:

- хеш пароля — **обязательно в одинарных кавычках**, иначе `$2b$12$...`
  обрежется при подстановке переменных (приложение это проверит и откажется
  стартовать);
- `COOKIE_SECURE=true` обязателен при работе по HTTPS за nginx.

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
curl -I http://127.0.0.1:8080             # сервер отвечает
```

## 4. nginx + HTTPS

```bash
sudo apt install -y nginx certbot python3-certbot-nginx
sudo cp deploy/nginx.example.conf /etc/nginx/sites-available/hip.kurein.me
sudo sed -i 's/example.com/hip.kurein.me/' /etc/nginx/sites-available/hip.kurein.me
sudo ln -s /etc/nginx/sites-available/hip.kurein.me /etc/nginx/sites-enabled/
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t && sudo systemctl reload nginx

sudo certbot --nginx -d hip.kurein.me
```

В конфиге стоит `client_max_body_size 200M` — держите его равным
`MAX_FILE_SIZE_MB`. После выпуска сертификата включите в Cloudflare режим
**Full (strict)**.

## 5. Файрвол

```bash
sudo ufw allow OpenSSH
sudo ufw allow 'Nginx Full'   # 80 + 443
sudo ufw enable
```

Порт 8080 наружу не открывайте — приложение слушает только localhost.

## 6. Проверка

Откройте `https://hip.kurein.me` — страница входа, логин `admin` + ваш пароль.
Логи: `journalctl -u rust-file-manager -f`.

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
└── home/
    ├── alice/           # личная зона alice
    │   └── Фото/ ...
    └── bob/             # личная зона bob
```

При обновлении со старой (однопользовательской) версии существующие папки
категорий автоматически переносятся в `shared/` при первом старте.

## Обновление приложения

```bash
cd ~/rust-file-manager && git pull
cargo build --release
sudo cp target/release/rust-file-manager /usr/local/bin/
sudo systemctl restart rust-file-manager
```
