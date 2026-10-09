# Установка с нуля: бот TelegramOnly и rust-file-manager на одном VPS

Справочник для **чистого VPS**. Порядок такой:
1. Подготовить сервер.
2. Поставить бота TelegramOnly.
3. Поставить файловый менеджер rust-file-manager с мини-приложением «Файлы».
4. Связать бота с файловым менеджером.

Другие справочники этой серии:
- обновление уже работающего сервера — [POST_DEPLOYwTELEGRAM.md](POST_DEPLOYwTELEGRAM.md);
- что где установлено, как ходят команды и где лежат файлы — [ARCHITECTUREwTELEGRAM.md](ARCHITECTUREwTELEGRAM.md).

Написано для rust-file-manager **1.7.0** и TelegramOnly **3.25.0**. Команды
повторяют те, которыми обновлялся рабочий сервер 07.10.2026. Целиком на чистом
VPS этот файл пока не прогонялся, поэтому каждый блок заканчивается проверкой.

## Коротко: одна команда `deploy` (с 1.8.0)

Всё, что описано ниже, делает команда `deploy`. На новом VPS под root:

```bash
apt-get update && apt-get install -y curl     # если curl ещё нет
curl -fsSL https://raw.githubusercontent.com/kureinmaxim/rust-file-manager/main/deploy/rfm-vps.sh | bash -s -- setup
deploy
```

Первая строка с `curl` ставит в `/usr/local/sbin` три команды: `deploy`,
`post_deploy` и `rfm-vps`. Сам скрипт — [deploy/rfm-vps.sh](deploy/rfm-vps.sh).
Он скачивается из публичного репозитория и выполняется от root, так же как
установщик Docker. Если хотите сначала прочитать его, скачайте файл командой
`curl -fsSLO …/deploy/rfm-vps.sh`, посмотрите и запустите `bash rfm-vps.sh setup`.

Что делает `deploy`:
1. **Смотрит, что уже стоит на сервере**: бот, файловый менеджер, оба или
   ничего. Ставит только недостающее.
2. **Сразу задаёт все вопросы**, дальше работает сам:
   - сеть Docker для бота: bridge или сеть хоста;
   - домен файлового менеджера и способ HTTPS: certbot, Cloudflare на 2083 или
     свой прокси. Если порт 443 занят, по умолчанию предлагается Cloudflare;
   - почта для Let's Encrypt, логин и пароль администратора сайта;
   - включать ли ufw.
3. **Ставит по шагам этого справочника**:
   - пакеты, обход PMTU, swap, файрвол;
   - бота. По ходу установщик бота спросит `BOT_TOKEN` и ваш Telegram ID, а
     GitHub — токен для приватного репозитория;
   - файловый менеджер. Сборка идёт в фоне и переживает обрыв SSH: команду
     можно запустить снова, она дождётся сборки;
   - nginx и HTTPS;
   - связку бота с файловым менеджером.
4. **Проверяет результат** и пишет, что осталось сделать в Telegram: включить
   мини-приложение в BotFather и один раз войти в него логином администратора.

Повторный `deploy` безопасен. Он ничего не переустанавливает и доделывает
отложенное: например, настраивает nginx на 2083, когда вы положили сертификат
Cloudflare, или чинит несовпадающий ключ бота. Ставить можно и по частям:
`deploy --only bot`, а позже `deploy` доставит файловый менеджер и свяжет его с
ботом.

Ответы можно передать заранее, тогда вопросов не будет (пароль всё равно
спрашивается):

```bash
RFM_DOMAIN=files.example.com HTTPS_MODE=certbot BOT_NETWORK=bridge ENABLE_UFW=yes deploy --yes
```

Обновлять потом — командой `post_deploy` ([POST_DEPLOYwTELEGRAM.md](POST_DEPLOYwTELEGRAM.md)).
Все вопросы, параметры, файлы и сообщения команд собраны в справочнике
[DEPLOYnew.md](DEPLOYnew.md).
Ниже те же шаги вручную: по ним видно, что делает команда, и по ним можно
пройти без неё.

## Как пользоваться

- **Где выполнять.** Команды рассчитаны на Debian 12 или Ubuntu 22.04+ и
  выполняются под **root** по SSH (например, в Tabby).
- **По одному блоку.** Вставляйте блоки по одному и сверяйте вывод с пунктом
  «Ожидается». Если вывод другой, дальше не идите.
- **Только примеры.** Все значения в этом файле — образцы. Свои значения
  подставляйте прямо на сервере в первых строках блоков (они помечены `← ваш`).
  Не записывайте в репозиторий и не пересылайте в чат свой домен, IP, токен
  бота, пароли и ключи.
- **Секреты не выводятся.** Блоки создают секреты прямо на сервере и пишут их в
  файлы с правами `600`. На экран выводится только длина ключа или «да/нет».

| Пример в файле | Что подставить |
|---|---|
| `files.example.com` | домен файлового менеджера |
| `203.0.113.10` | публичный адрес сервера (адрес из диапазона для документации; нужен только для SSH и DNS) |
| `files_example_bot`, `7000000001` | имя и числовой ID вашего бота |
| `anna` | логин участника для примеров |

## Что получится

```
 Telegram (облако)                         Браузер, мини-приложение в Telegram
      ▲  long polling, только исходящие              │ HTTPS
      │                                              ▼
 ┌────┴───────────────────────┐            ┌──────────────────────┐
 │ бот TelegramOnly (Docker)  │            │ nginx :443 или :2083 │
 │ /opt/TelegramOnly          │            └──────────┬───────────┘
 └────┬───────────────────────┘                       │ 127.0.0.1:8080
      │ служебный токен,                   ┌──────────▼───────────┐
      │ 127.0.0.1:8091 ──────────────────▶ │ rust-file-manager    │
      │ (внутренний API)                   │ (systemd)            │
      │                                    └──────────┬───────────┘
      │                                               ▼
      │                         /var/lib/rust-file-manager/uploads
      │                         shared/ — общие, home/ — личные, exchange/ — «Обмен»
```

Подробная схема — в [ARCHITECTUREwTELEGRAM.md](ARCHITECTUREwTELEGRAM.md).

## Шаг 0. Что подготовить заранее

1. **VPS:** Debian 12 или Ubuntu 22.04+, от 1 ГБ памяти и от 10 ГБ свободного
   диска. На 1 ГБ сборка Rust идёт только со swap, его создаёт шаг 1.
2. **Домен:** A-запись `files.example.com` → адрес VPS. Можно напрямую, можно
   через Cloudflare.
3. **Бот:** в @BotFather выполните `/newbot` и получите токен вида
   `7000000001:AA…`. Токен нужен только боту. Файловому менеджеру нужен лишь
   числовой ID бота — число до двоеточия.
4. **Ваш Telegram ID** — его покажет @userinfobot.
5. **Доступ к TelegramOnly.** Репозиторий приватный, поэтому нужен GitHub
   fine-grained token с правом только на чтение этого репозитория. При
   `git clone` его вводят вместо пароля.
6. **Порт 443.** Решите, кому он достанется:
   - **443 свободен** — HTTPS для файлового менеджера через certbot (шаг 2.7А);
   - **443 займёт VPN-транспорт** (VLESS, NaiveProxy) — HTTPS через Cloudflare
     на порт 2083 (шаг 2.7Б).
7. **Сеть Docker для бота:**

   | | bridge (по умолчанию) | сеть хоста (`compose.host.yaml`) |
   |---|---|---|
   | Когда выбирать | боту нужен только интернет | боту нужна сеть Tailscale/Headscale, например панель 3x-ui доступна только по mesh-адресу |
   | Как бот ходит в файловый менеджер | через мост socat на шлюзе docker-сети: `http://172.18.0.1:8091` | напрямую: `http://127.0.0.1:8091` |
   | Обход PMTU (см. шаг 1) | в контейнере и на хосте | только на хосте |

## Шаг 1. Подготовить VPS

```bash
ssh root@203.0.113.10          # ← адрес вашего сервера
```

Пакеты, обход PMTU и swap:

```bash
apt-get update
apt-get install -y git curl ca-certificates openssl build-essential pkg-config socat nginx ufw
# У части провайдеров теряются ICMP «fragmentation needed», и опрос Telegram
# зависает (TelegramOnly SERVER_OPS.md). Агрессивный MTU probing это обходит.
cat > /etc/sysctl.d/99-tcp-mtu-probing.conf <<'EOF'
net.ipv4.tcp_mtu_probing = 2
net.ipv4.tcp_base_mss = 1024
EOF
sysctl -p /etc/sysctl.d/99-tcp-mtu-probing.conf
# Swap 2 ГБ, если его нет: без него на 1 ГБ памяти сборка Rust падает
if [ -z "$(swapon --show)" ]; then
  fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile &&
  echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi
free -h; df -h /
```

Ожидается:
- строки `net.ipv4.tcp_mtu_probing = 2` и `net.ipv4.tcp_base_mss = 1024`;
- в `free -h` строка `Swap` не нулевая.

Файрвол. Сначала узнайте порт SSH, через который вы сейчас подключены:

```bash
SSH_PORT=$(echo "$SSH_CONNECTION" | awk '{print $4}')
echo "Вы подключены по SSH через порт: ${SSH_PORT:-НЕ НАЙДЕН}"
```

Если порт не найден, файрвол не включайте: можно потерять доступ к серверу.
Если найден, в той же сессии:

```bash
ufw allow "${SSH_PORT}/tcp"
ufw allow 80/tcp
ufw allow 443/tcp        # для HTTPS через Cloudflare вместо этого: ufw allow 2083/tcp
ufw --force enable
ufw status verbose
```

Если ufw на сервере уже настроен (например, скриптом `deploy_fresh_vps.sh`
из TelegramOnly), не включайте его заново, только добавьте порты 80 и
443 (или 2083). Порты 8080, 8091 и 8000 наружу не открывайте: все три
слушают только локальный адрес.

---

# Часть 1. Бот TelegramOnly

## 1.1. Docker

```bash
command -v docker >/dev/null || curl -fsSL https://get.docker.com | sh
docker --version && docker compose version
```

Ожидаются версии Docker и Docker Compose v2.

## 1.2. Код бота

```bash
git clone https://github.com/kureinmaxim/TelegramOnly.git /opt/TelegramOnly
cd /opt/TelegramOnly && git log --oneline -1 && grep -m1 '^version' pyproject.toml
```

На запрос логина введите свой логин GitHub, на запрос пароля — токен из
шага 0. Ожидается `version = "3.25.0"` или новее.

## 1.3. Только для варианта «сеть хоста»

Пропустите этот шаг, если бот будет в обычной сети bridge.

```bash
cd /opt/TelegramOnly
cp -n example.env .env && chmod 600 .env
grep -q '^COMPOSE_FILE=' .env ||
  printf '\n# Бот в сети хоста: docker compose в этом каталоге берёт оба файла\nCOMPOSE_FILE=compose.yaml:compose.host.yaml\n' >> .env
grep '^COMPOSE_FILE=' .env
```

Ожидается `COMPOSE_FILE=compose.yaml:compose.host.yaml`.

Строка в `.env` нужна, чтобы любой `docker compose` в этом каталоге, в том
числе `scripts/rebuild_bot.sh`, брал оба файла. Без неё следующая пересборка
молча вернёт бота в сеть bridge, и он потеряет доступ к mesh.

## 1.4. Установить и запустить бота

```bash
cd /opt/TelegramOnly
bash scripts/install_telegramonly_docker.sh
```

Скрипт:
- спросит `BOT_TOKEN`, `ADMIN_USER_IDS` (ваш Telegram ID) и публичный адрес
  (Enter подставит IP сервера);
- сам создаст секреты API;
- соберёт образ и запустит контейнер `telegram-helper-lite`;
- включит еженедельную очистку диска.

Уже заполненные поля скрипт не переспрашивает. Токен вводите только в этот
запрос, не в чат.

Проверка:

```bash
docker ps --filter name=telegram-helper-lite --format '{{.Names}}: {{.Status}}'
docker inspect -f 'сеть: {{.HostConfig.NetworkMode}}' telegram-helper-lite
docker exec telegram-helper-lite grep -m1 '^version' /app/pyproject.toml
docker logs --tail 50 telegram-helper-lite 2>&1 | grep -E 'ERROR|Traceback' || echo "ошибок нет"
```

Ожидается:
- `telegram-helper-lite: Up …`;
- `сеть: telegramonly_default` (bridge) или `сеть: host`;
- `version = "3.25.0"`;
- `ошибок нет`.

В Telegram напишите боту `/start`, затем `/version`: он должен ответить 3.25.0.
Команда `/files` пока ответит, что «Файлы» не настроены. До части 3 так и
должно быть.

Если сборка образа упала, повторите её через `bash scripts/rebuild_bot.sh`:
скрипт обходит частую проблему DNS у BuildKit.

## 1.5. ID и имя бота

Они понадобятся файловому менеджеру. Команда печатает только ID и имя, токен
не выводится:

```bash
docker exec telegram-helper-lite python -c "import os,json,urllib.request; t=os.environ['BOT_TOKEN'].strip(); r=json.load(urllib.request.urlopen(f'https://api.telegram.org/bot{t}/getMe',timeout=20))['result']; print(r['id'], r['username'])"
```

Ожидается строка вида `7000000001 files_example_bot`.

---

# Часть 2. Файловый менеджер rust-file-manager

## 2.1. Rust

```bash
command -v cargo >/dev/null || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
rustc --version
```

Нужен Rust 1.88 или новее. Если версия старше, выполните `rustup update stable`.

## 2.2. Собрать

Сборка идёт в фоне через `nohup`, поэтому обрыв SSH её не прервёт. Бот всё
это время работает.

```bash
. "$HOME/.cargo/env"
RFM_REF=v1.7.0                  # ← тег выпуска; пока тега нет — main
RFM_BUILD_DIR=/root/rfm-build
test -d "$RFM_BUILD_DIR/.git" ||
  git clone -q --depth 1 https://github.com/kureinmaxim/rust-file-manager.git "$RFM_BUILD_DIR"
if cd "$RFM_BUILD_DIR" && git fetch -q --depth 1 origin "$RFM_REF" && git checkout -q --detach FETCH_HEAD; then
  git log --oneline -1; grep -m1 '^version' Cargo.toml
  nohup nice -n 10 env CARGO_BUILD_JOBS=1 cargo build --release --locked --bin rust-file-manager > /root/rfm-build.log 2>&1 &
  echo "Сборка запущена. Журнал: /root/rfm-build.log"
else
  echo "Код не получен — сборку НЕ запускаю"
fi
```

Ожидается:
- коммит выпуска и `version = "1.7.0"`;
- строка `Сборка запущена`.

Сверьте коммит с тегом на GitHub: в сборку должен попасть именно он.

**Как следить за сборкой.** Сборка пишет вывод не на экран, а в журнал
`/root/rfm-build.log`. Посмотреть последние строки:

```bash
tail -3 /root/rfm-build.log
```

- `Compiling …` или `Building …` — сборка ещё идёт; повторите через пару минут.
  На последнем этапе журнал по нескольку минут не меняется, это нормально.
- `Finished \`release\` profile [optimized] target(s) in …` — сборка готова.
- `error:` — сборка упала; пришлите или изучите `tail -30 /root/rfm-build.log`.
  Строки `warning:` ошибкой не считаются.

Смотреть в реальном времени: `tail -f /root/rfm-build.log`. Ctrl+C закрывает
только просмотр, сборка продолжается. Первая сборка на 2 ГБ памяти занимает
около 10 минут, на 1 ГБ со swap — до 25. Вторую сборку параллельно не
запускайте: на маленьком VPS это может подвесить SSH.

## 2.3. Пользователь службы и каталоги

Этот шаг можно выполнить, пока идёт сборка:

```bash
id filemgr >/dev/null 2>&1 ||
  useradd --system --user-group --home-dir /var/lib/rust-file-manager --shell /usr/sbin/nologin filemgr
install -d -o filemgr -g filemgr -m 0750 /var/lib/rust-file-manager /var/lib/rust-file-manager/uploads
install -d -o root -g root -m 0755 /etc/rust-file-manager
ls -ld /var/lib/rust-file-manager /var/lib/rust-file-manager/uploads /etc/rust-file-manager
```

Ожидаются три каталога: два принадлежат `filemgr`, `/etc/rust-file-manager` — `root`.

## 2.4. Установить программу (после `Finished`)

```bash
if grep -q 'Finished' /root/rfm-build.log && test ! -e /usr/local/bin/rust-file-manager; then
  install -o root -g root -m 0755 /root/rfm-build/target/release/rust-file-manager /usr/local/bin/rust-file-manager
  ls -la /usr/local/bin/rust-file-manager
else
  echo "Сборка не закончилась, или программа уже установлена:"; tail -2 /root/rfm-build.log
fi
```

Ожидается файл `/usr/local/bin/rust-file-manager`. Если программа уже стоит,
это не чистая установка: замену делайте по [POST_DEPLOYwTELEGRAM.md](POST_DEPLOYwTELEGRAM.md).

## 2.5. Настройки и секреты

Блок работает только при первой установке:
- дважды спрашивает пароль администратора сайта; ввод не отображается;
- сам создаёт `SESSION_SECRET` и `INTERNAL_API_TOKEN`;
- пишет `/etc/rust-file-manager/env` с правами `600`.

Если файл настроек уже есть, блок ничего не меняет. Подставьте свои значения
в первые три строки:

```bash
RFM_DOMAIN=files.example.com     # ← ваш домен
BOT_ID=7000000001                # ← ID бота из шага 1.5
BOT_USERNAME=files_example_bot   # ← имя бота из шага 1.5, без @
F=/etc/rust-file-manager/env
if [ -e "$F" ]; then
  echo "$F уже существует — ничего не меняю"
else
  IFS= read -r -s -p 'Придумайте пароль администратора сайта: ' P1; printf '\n'
  IFS= read -r -s -p 'Повторите пароль: ' P2; printf '\n'
  HASH=''
  [ -n "$P1" ] && [ "$P1" = "$P2" ] && HASH=$(printf '%s' "$P1" | /usr/local/bin/rust-file-manager hash-password 2>/dev/null)
  unset P1 P2
  case "$HASH" in
    '$2'*)
      install -o root -g root -m 0600 /dev/null "$F"
      {
        echo 'BIND_ADDR=127.0.0.1:8080'
        echo 'UPLOAD_DIR=/var/lib/rust-file-manager/uploads'
        echo 'USERS_FILE=/var/lib/rust-file-manager/users.json'
        echo '# Логин администратора сайта'
        echo 'ADMIN_USERNAME=admin'
        echo "ADMIN_PASSWORD_HASH='$HASH'"
        echo "SESSION_SECRET=$(openssl rand -base64 64 | tr -d '\n')"
        echo 'MAX_FILE_SIZE_MB=200'
        echo 'COOKIE_SECURE=true'
        echo 'RUST_LOG=info'
        echo '# Мини-приложение «Файлы» в Telegram'
        echo 'MINIAPP_ENABLED=true'
        echo "TELEGRAM_BOT_ID=$BOT_ID"
        echo "TELEGRAM_BOT_USERNAME=$BOT_USERNAME"
        echo "PUBLIC_BASE_URL=https://$RFM_DOMAIN"
        echo 'UPLOAD_CHUNK_SIZE_MB=8'
        echo 'MAX_CHUNKED_FILE_SIZE_MB=4096'
        echo '# Внутренний API только для бота; наружу не открывать'
        echo 'INTERNAL_BIND_ADDR=127.0.0.1:8091'
        echo "INTERNAL_API_TOKEN=$(openssl rand -hex 32)"
      } >> "$F"
      echo "Записано: $F" ;;
    *) echo "Пароли пустые, не совпали или хеш не получен — ничего не записано" ;;
  esac
  unset HASH
fi
grep -E '^(BIND_ADDR|UPLOAD_DIR|USERS_FILE|ADMIN_USERNAME|COOKIE_SECURE|MINIAPP_ENABLED|TELEGRAM_BOT_ID|TELEGRAM_BOT_USERNAME|PUBLIC_BASE_URL|INTERNAL_BIND_ADDR)=' "$F"
for k in ADMIN_PASSWORD_HASH SESSION_SECRET INTERNAL_API_TOKEN; do
  printf '%s: длина %s\n' "$k" "$(sed -n "s/^$k=//p" "$F" | tr -d "'\n" | wc -c)"
done
```

Ожидается:
- десять строк настроек без секретов;
- `ADMIN_PASSWORD_HASH: длина 60`, `SESSION_SECRET: длина 88`,
  `INTERNAL_API_TOKEN: длина 64`.

Пояснения:
- **`ADMIN_USERNAME`** — логин администратора сайта; при желании замените
  `admin` в файле до запуска службы.
- **`SESSION_SECRET`** — из него выводятся ключи входа, токенов мини-приложения
  и ссылок на скачивание. Не меняйте его без причины: иначе все выйдут из
  системы.
- **`MAX_FILE_SIZE_MB`** — лимит одного файла в обычном веб-интерфейсе.
  Мини-приложение грузит частями по 8 МБ, для него действует лимит
  `MAX_CHUNKED_FILE_SIZE_MB` (4 ГБ).

## 2.6. Служба systemd

```bash
install -o root -g root -m 0644 /root/rfm-build/deploy/rust-file-manager.service /etc/systemd/system/rust-file-manager.service
systemctl daemon-reload
systemctl enable --now rust-file-manager.service && sleep 3
systemctl is-active rust-file-manager.service
journalctl -u rust-file-manager.service --no-pager -o cat | grep -E 'starting server|internal API' | tail -2
curl -sS -o /dev/null -w 'local /login: %{http_code}\n' http://127.0.0.1:8080/login
curl -sS -o /dev/null -w 'local /tg/: %{http_code}\n' http://127.0.0.1:8080/tg/
curl -sS -o /dev/null -w 'internal без ключа: %{http_code}\n' http://127.0.0.1:8091/internal/v1/health
sed -n 's/^INTERNAL_API_TOKEN=/Authorization: Bearer /p' /etc/rust-file-manager/env |
  curl -sS -H @- http://127.0.0.1:8091/internal/v1/health; echo
```

Ожидается:
- `active`;
- `starting server version="1.7.0" commit="…" … miniapp=true`;
- `internal API for the bot enabled addr=127.0.0.1:8091`;
- `local /login: 200`, `local /tg/: 200`;
- `internal без ключа: 401` — без ключа внутренний API не пускает;
- `{"success":true,"version":"1.7.0"}` — с ключом пускает.

Если служба не стартовала: `journalctl -u rust-file-manager.service -n 30 --no-pager`.

## 2.7. HTTPS и nginx

Выберите один вариант.

### А. Порт 443 свободен: certbot

```bash
RFM_DOMAIN=files.example.com     # ← ваш домен
apt-get install -y certbot python3-certbot-nginx
sed -e "s/server_name example\.com;/server_name $RFM_DOMAIN;/" \
    -e 's/client_max_body_size 200M;/client_max_body_size 201M;/' \
    /root/rfm-build/deploy/nginx.example.conf > /etc/nginx/sites-available/rust-file-manager
ln -sf /etc/nginx/sites-available/rust-file-manager /etc/nginx/sites-enabled/rust-file-manager
nginx -t && systemctl reload nginx
certbot --nginx -d "$RFM_DOMAIN"
certbot renew --dry-run
```

Certbot спросит почту и согласие с условиями, затем сам допишет HTTPS в
конфигурацию.

### Б. Порт 443 занят: Cloudflare и порт 2083

Подробно с пояснениями — [GUIDE_cloudflare.md](GUIDE_cloudflare.md). Коротко:

**Б1. Сертификат.** В Cloudflare откройте **SSL/TLS → Origin Server → Create
Certificate** и сохраните `cert.pem` и `key.pem` на своём компьютере. Ключ
показывается один раз.

**Б2. Передача на сервер.** Эти команды выполняются на вашем компьютере, в
каталоге с сохранёнными файлами:

```bash
VPS=root@203.0.113.10          # ← адрес вашего сервера
DOMAIN=files.example.com       # ← ваш домен
ssh "$VPS" "install -d -m 0700 /etc/nginx/ssl/$DOMAIN" &&
scp cert.pem key.pem "$VPS:/etc/nginx/ssl/$DOMAIN/" &&
rm key.pem && echo "Ключ передан на сервер и удалён с компьютера"
```

Ключ удаляется с компьютера, только если он действительно скопирован.

**Б3. Блок nginx на 2083.** Дальше снова на сервере:

```bash
RFM_DOMAIN=files.example.com     # ← ваш домен
cat > /etc/nginx/sites-available/rust-file-manager-ssl <<'EOF'
server {
    listen 2083 ssl;
    server_name files.example.com;
    ssl_certificate     /etc/nginx/ssl/files.example.com/cert.pem;
    ssl_certificate_key /etc/nginx/ssl/files.example.com/key.pem;
    client_max_body_size 201M;

    # Мини-приложение: части загрузки идут сразу в приложение, без буфера на диске
    location /api/v1/uploads/ {
        client_max_body_size 10M;
        proxy_request_buffering off;
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # В пути подписанной ссылки лежит токен: не писать её в журнал
    location /d/ {
        access_log off;
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 300;
    }

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 300;
        proxy_send_timeout 300;
    }
}
EOF
sed -i "s/files\.example\.com/$RFM_DOMAIN/g" /etc/nginx/sites-available/rust-file-manager-ssl
chmod 600 "/etc/nginx/ssl/$RFM_DOMAIN/key.pem"
ln -sf /etc/nginx/sites-available/rust-file-manager-ssl /etc/nginx/sites-enabled/rust-file-manager-ssl
nginx -t && systemctl reload nginx
ufw allow 2083/tcp
```

Ожидается `syntax is ok` и `test is successful`.

**Б4. Настройки Cloudflare:**
- **SSL/TLS** → режим **Full (strict)**;
- **Rules → Origin Rules** → Hostname equals `files.example.com` →
  Destination Port `2083`;
- **Caching → Cache Rules** → **Bypass cache** для путей, начинающихся с
  `/api/` и `/d/`.

### Проверка снаружи (оба варианта)

```bash
RFM_DOMAIN=files.example.com     # ← ваш домен
curl -sS -o /dev/null -w 'public /login: %{http_code}\n' "https://$RFM_DOMAIN/login"
curl -sS -o /dev/null -w 'public /tg/: %{http_code}\n' "https://$RFM_DOMAIN/tg/"
```

Ожидается `200` и `200`.

**Лимиты.** На Cloudflare Free один запрос ограничен 100 МБ, поэтому большие
файлы через обычный веб-интерфейс не пройдут. Мини-приложение этого не
замечает: оно шлёт файл частями по 8 МБ.

## 2.8. Первый вход в браузере

1. Откройте `https://files.example.com` и войдите логином `admin` и паролем из
   шага 2.5.
2. В футере должна быть версия 1.7.0.
3. Загрузите и скачайте тестовый файл.

---

# Часть 3. Связать бота и файловый менеджер

## 3.1. Включить мини-приложение в BotFather

1. Откройте **@BotFather**, выполните `/mybots` и выберите своего бота.
2. Нажмите **Bot Settings → Configure Mini App → Enable Mini App**.
3. Отправьте адрес `https://files.example.com/tg/` — со слешем в конце.

## 3.2. Стать администратором файлов в Telegram

1. На телефоне откройте `https://t.me/files_example_bot?startapp` или кнопку
   «Открыть» в профиле бота.
2. Появится экран «Свяжите Telegram с аккаунтом». Введите логин и пароль
   администратора сайта.
3. После привязки откроется главная мини-приложения.

Теперь ваш Telegram — «администратор файлов», и бот сможет выдавать
приглашения командой `/files_invite`.

## 3.3. Мост для бота (только сеть bridge)

Пропустите этот шаг, если бот в сети хоста.

Из контейнера в сети bridge адрес `127.0.0.1` указывает на сам контейнер, а
не на сервер. Поэтому на шлюзе docker-сети ставится мост socat, который
пересылает запросы бота на `127.0.0.1:8091`:

```bash
GW=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.Gateway}}{{end}}' telegram-helper-lite)
SUBNET=$(docker network inspect -f '{{range .IPAM.Config}}{{.Subnet}}{{end}}' telegramonly_default)
echo "шлюз: $GW  подсеть: $SUBNET"
sed "s/bind=172\.18\.0\.1/bind=$GW/" /root/rfm-build/deploy/rfm-internal-bridge.service \
  > /etc/systemd/system/rfm-internal-bridge.service
systemctl daemon-reload && systemctl enable --now rfm-internal-bridge && sleep 2
systemctl is-active rfm-internal-bridge
ufw allow from "$SUBNET" to "$GW" port 8091 proto tcp
curl -sS -o /dev/null -w 'через мост без ключа: %{http_code}\n' "http://$GW:8091/internal/v1/health"
```

Ожидается:
- `шлюз: 172.18.0.1  подсеть: 172.18.0.0/16` (или ваши значения);
- `active`;
- `через мост без ключа: 401`.

Правило ufw открывает 8091 только для docker-сети бота. Из интернета этот
адрес недоступен.

## 3.4. Строки `FILES_*` в `.env` бота

Блок сам выбирает адрес файлового менеджера по сети бота. Ключ копируется из
настроек файлового менеджера и на экран не выводится. Затем блок пересоздаёт
контейнер бота: новые строки `.env` бот читает только при пересоздании
(`restart` их не перечитывает).

```bash
RFM_DOMAIN=files.example.com     # ← ваш домен
cd /opt/TelegramOnly
if [ "$(docker inspect -f '{{.HostConfig.NetworkMode}}' telegram-helper-lite)" = host ]; then
  FILES_URL=http://127.0.0.1:8091
else
  FILES_URL="http://$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.Gateway}}{{end}}' telegram-helper-lite):8091"
fi
if grep -qE '^FILES_(MINIAPP_URL|INTERNAL_URL|SERVICE_TOKEN)=' .env; then
  echo "FILES_* уже есть — ничего не добавляю"
else
  {
    echo
    echo '# «Файлы» — мини-приложение rust-file-manager на этом же VPS'
    echo "FILES_MINIAPP_URL=https://$RFM_DOMAIN/tg/"
    echo "FILES_INTERNAL_URL=$FILES_URL"
    echo "FILES_SERVICE_TOKEN=$(sed -n 's/^INTERNAL_API_TOKEN=//p' /etc/rust-file-manager/env)"
    echo 'FILES_MENU_BUTTON=webapp'
  } >> .env
  echo "Добавлено"
fi
grep -E '^(FILES_MINIAPP_URL|FILES_INTERNAL_URL|FILES_MENU_BUTTON|COMPOSE_FILE)=' .env
echo "Ключи совпадают: $( [ "$(sed -n 's/^FILES_SERVICE_TOKEN=//p' .env)" = "$(sed -n 's/^INTERNAL_API_TOKEN=//p' /etc/rust-file-manager/env)" ] && echo да || echo НЕТ )"
docker compose up -d --force-recreate telegram-helper
sleep 30
docker inspect -f 'сеть: {{.HostConfig.NetworkMode}}' telegram-helper-lite
docker logs --since 2m telegram-helper-lite 2>&1 | grep -E 'files|ERROR|Traceback' | tail -8
```

Ожидается:
- строки `FILES_*` (и `COMPOSE_FILE` для сети хоста), `Ключи совпадают: да`;
- та же сеть, что в шаге 1.4;
- в журнале `files: кнопки меню — выставлено 1, сброшено 0, ошибок 0`.
  Единица — это вы, пока единственный привязанный пользователь.

Остальные переменные можно не задавать, у них есть значения по умолчанию:

| Переменная | По умолчанию | Что делает |
|---|---|---|
| `FILES_MENU_SYNC_MINUTES` | `10` | как часто бот сверяет кнопку «Файлы» со списком привязок |
| `FILES_NOTIFY` | `true` | писать в личку, когда в «Обмен» пришёл файл |
| `FILES_NOTIFY_MINUTES` | `10` | как часто бот проверяет «Обмен» |

## 3.5. Проверка из Telegram

1. **`/files_status`** — версия файлового менеджера 1.7.0, диск, участники,
   «привязано: 1».
2. **Кнопка «Файлы».** Закройте и снова откройте чат с ботом: слева от поля
   ввода вместо «Меню» появится кнопка **«Файлы»**. Если её нет, перезапустите
   Telegram: клиент запоминает кнопку.
3. **Команды.** Наберите `/`: список команд показывается, как раньше.
4. **`/files`** — сообщение с кнопкой, которая открывает мини-приложение.
5. **`/files_invite`** — одноразовая ссылка-приглашение для участника, живёт
   7 дней. Участник открывает её в Telegram, придумывает логин и пароль, и его
   Telegram сразу привязан.
6. **«Обмен».** Отправьте участнику файл в «Обмен» в мини-приложении или в
   браузере. В течение 10 минут ему придёт сообщение «📥 Администратор прислал
   вам файл…» с кнопкой «Открыть обмен». Бот может писать только тем, кто хотя
   бы раз нажал у него «Старт».

## 3.6. Что включить в резервные копии

| Что | Где | Зачем |
|---|---|---|
| Настройки файлового менеджера | `/etc/rust-file-manager/env` | секреты: хеш пароля, `SESSION_SECRET`, ключ для бота |
| Учётные записи и все файлы | `/var/lib/rust-file-manager/` (`users.json` и `uploads/`) | пользователи, привязки Telegram, общие, личные и обменные файлы |
| Настройки бота | `/opt/TelegramOnly/.env` и `/opt/TelegramOnly/*_config.json` | токен бота, секреты API, конфигурации транспортов |

Копии храните вне этого сервера и в зашифрованном виде: в них все секреты.

---

## Сводка адресов

| Адрес | Кто слушает | Кто может подключиться |
|---|---|---|
| `:443` или `:2083` | nginx, HTTPS | интернет (браузер, Telegram) |
| `127.0.0.1:8080` | rust-file-manager: сайт, мини-приложение, API | только nginx на этом сервере |
| `127.0.0.1:8091` | rust-file-manager: внутренний API | только бот, по служебному токену |
| `172.18.0.1:8091` | мост socat (только сеть bridge) | только docker-сеть бота |
| `:8000` | REST API бота | в сети bridge слушает только `127.0.0.1`; в сети хоста — все адреса, и наружу его закрывает ufw (шаг 1) |

## Если что-то не так

| Симптом | Причина и что сделать |
|---|---|
| Сборка оборвалась, в журнале `SIGKILL` или `signal: 9` | Не хватило памяти: проверьте swap (шаг 1) и `CARGO_BUILD_JOBS=1` |
| `git clone` TelegramOnly просит пароль и падает | Вместо пароля нужен GitHub-токен с правом на чтение репозитория |
| Служба не стартует, в журнале `Configuration error` | Читайте текст ошибки: обычно не задан `TELEGRAM_BOT_ID` или `INTERNAL_API_TOKEN` короче 32 символов |
| `/files_status`: «файловый менеджер недоступен» | Bridge: не запущен `rfm-internal-bridge`, нет правила ufw или неверный шлюз в `FILES_INTERNAL_URL`. Сеть хоста: `FILES_INTERNAL_URL` должен быть `http://127.0.0.1:8091` |
| `/files_status`: «файловый менеджер отклонил токен бота» | `FILES_SERVICE_TOKEN` ≠ `INTERNAL_API_TOKEN`: повторите проверку «Ключи совпадают» из шага 3.4 |
| «Данные Telegram не прошли проверку подписи» | `TELEGRAM_BOT_ID` не совпадает с ботом, из которого открыто приложение |
| «Клиент Telegram не передал подпись» | Обновите Telegram |
| Нет кнопки «Файлы» | Перезапустите Telegram; проверьте в `/files_status`, что ваш Telegram привязан (шаг 3.2) |
| Загрузка в мини-приложении падает с 413 | В nginx нет блока `/api/v1/uploads/` или `client_max_body_size` меньше 10M |
| Сайт снаружи отвечает 525 или 521 | Cloudflare: не задано Origin Rule на 2083, не открыт порт 2083 или неверный сертификат |
| Уведомления об «Обмене» не приходят | Подождите 10 минут; получатель должен хотя бы раз нажать у бота «Старт»; проверьте, что нет `FILES_NOTIFY=false` |

**Чего не делать с ботом.** Не перезапускайте его через `docker compose down`:
команда гасит весь проект. Правильно — `docker compose up -d --force-recreate
telegram-helper` из `/opt/TelegramOnly`. Подробности — в `SERVER_OPS.md`
репозитория TelegramOnly.
