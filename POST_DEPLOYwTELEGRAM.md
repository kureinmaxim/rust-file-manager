# Обновление сервера: rust-file-manager и бот TelegramOnly

Справочник для **работающего VPS**, где бот TelegramOnly и файловый менеджер
rust-file-manager уже стоят и связаны. Сервер мог быть установлен по
[DEPLOYwTELEGRAM.md](DEPLOYwTELEGRAM.md) или по более ранним инструкциям.

Другие справочники этой серии:
- установка с нуля — [DEPLOYwTELEGRAM.md](DEPLOYwTELEGRAM.md);
- что где установлено, как ходят команды и где лежат файлы — [ARCHITECTUREwTELEGRAM.md](ARCHITECTUREwTELEGRAM.md).

Команды повторяют обновления рабочего сервера 07.10.2026: TelegramOnly
3.18.3 → 3.24.0 и rust-file-manager 1.3.0 → 1.5.0 → 1.5.1 → 1.6.0. Примеры
версий ниже — для следующего шага, до rust-file-manager **1.7.0** и
TelegramOnly **3.25.0**.

## Коротко: одна команда `post_deploy` (с 1.8.0)

```bash
post_deploy
```

Если сервер ставили до 1.8.0, сначала один раз установите команды под root:

```bash
curl -fsSL https://raw.githubusercontent.com/kureinmaxim/rust-file-manager/main/deploy/rfm-vps.sh | bash -s -- setup
```

Что делает `post_deploy`:
1. **Определяет, что стоит на сервере**: файловый менеджер, бот (в Docker или
   через systemd) или оба. Если ничего нет, предлагает `deploy`.
2. **Обновляет файловый менеджер**, если на GitHub новая версия:
   - сборка идёт в фоне в прежнем каталоге сборки (`/root/rfm-build…`);
   - прежняя программа сохраняется в копию;
   - замена атомарная, затем перезапуск и проверки: запущен нужный коммит,
     отвечают сайт, мини-приложение и API для бота;
   - если проверки не прошли, сам возвращает прежнюю версию.
3. **Обновляет бота** через `rebuild_bot.sh` и сверяет версию в контейнере. Бот,
   запущенный в сети хоста, перед этим закрепляется в ней строкой `COMPOSE_FILE`
   (шаг 0б ниже). Если в каталоге бота есть свои правки или коммиты, бота не
   трогает.
4. **Проверяет связку** бота с файловым менеджером и чинит её, если ключи
   разошлись.
5. **Убирает старое**: держит последние три копии программы и `.env`.

Если версия не изменилась, ничего не пересобирается и не перезапускается.
Перед работой команды сами обновляются из репозитория.

| Вариант | Что делает |
|---|---|
| `post_deploy --check` | только показать, есть ли обновления |
| `post_deploy --only fm` или `--only bot` | обновить одну часть |
| `post_deploy --ref v1.8.0` | поставить выпуск по тегу и остаться на нём; вернуться к последней версии: `--ref main` |
| `post_deploy --backup-data` | перед заменой программы ещё и сохранить архив данных |
| `post_deploy --force` | пересобрать и перезапустить, даже если версия та же |
| `rfm-vps status` | что стоит на сервере, только чтение |

Полный справочник по командам — [DEPLOYnew.md](DEPLOYnew.md). Ниже те же шаги
вручную.

## Как пользоваться

- **Где и как выполнять.** Под root по SSH (например, в Tabby), по одному
  блоку, сверяя вывод с пунктом «Ожидается». Если вывод другой, дальше не идите.
- **Только примеры.** В файле нет настоящих доменов, адресов, паролей и
  ключей. Свои значения подставляйте на сервере в строках с пометкой `← ваш`.
  Не публикуйте вывод, где виден ваш домен.
- **Секреты на экран не выводятся.** Ни один блок не печатает содержимое
  `/etc/rust-file-manager/env` и `/opt/TelegramOnly/.env` целиком, только
  безопасные строки.

| Пример в файле | Что подставить |
|---|---|
| `files.example.com` | домен файлового менеджера |
| `/root/rfm-build` | ваш каталог сборки (`ls -d /root/rfm-build*`) |
| `v1.7.0` | тег выпуска, который ставите (или `main`) |

**Порядок.** Сначала файловый менеджер, потом бот. Новые функции бота
опираются на API файлового менеджера. В обратном сочетании ничего не ломается:
старый бот с новым файловым менеджером работает как раньше, а новый бот со
старым файловым менеджером просто не получает новых возможностей. Две сборки
одновременно не запускайте: на VPS с 1–2 ГБ памяти это может подвесить SSH.

## Совместимость выпусков

| rust-file-manager | TelegramOnly | Что появилось |
|---|---|---|
| 1.5.0 | 3.24.0 | мини-приложение «Файлы», команды `/files_*`, кнопка «Файлы» |
| 1.5.1 | — | подпапки и перенос файлов в браузере; верный размер файла при скачивании в Telegram |
| 1.6.0 | — | «Обмен» между администратором и каждым участником |
| 1.7.0 | 3.25.0 | уведомления об «Обмене» раз в 10 минут с кнопкой «Открыть обмен» |

Прочерк — бот обновлять не нужно.

---

## Шаг 0. Проверить сервер (только чтение)

```bash
echo "== сервер"; uptime; free -h; df -h / /root /var/backups
echo "== файловый менеджер"; systemctl is-active rust-file-manager.service
journalctl -u rust-file-manager.service --no-pager -o cat | grep 'starting server' | tail -1
grep -E '^(BIND_ADDR|UPLOAD_DIR|USERS_FILE|MINIAPP_ENABLED|TELEGRAM_BOT_USERNAME|PUBLIC_BASE_URL|INTERNAL_BIND_ADDR)=' /etc/rust-file-manager/env
echo "SESSION_SECRET задан: $(grep -cE '^SESSION_SECRET=.+' /etc/rust-file-manager/env)"
echo "каталоги сборки:"; ls -d /root/rfm-build* 2>/dev/null || echo "  нет"
echo "== бот"; git -C /opt/TelegramOnly log --oneline -1; git -C /opt/TelegramOnly status --short | head -5
docker exec telegram-helper-lite grep -m1 '^version' /app/pyproject.toml
docker inspect -f 'сеть: {{.HostConfig.NetworkMode}}' telegram-helper-lite
docker inspect -f 'compose-файлы: {{index .Config.Labels "com.docker.compose.project.config_files"}}' telegram-helper-lite
grep -E '^COMPOSE_FILE=' /opt/TelegramOnly/.env || echo "COMPOSE_FILE в .env не задан"
echo "строк FILES_* в .env бота: $(grep -cE '^FILES_(MINIAPP_URL|INTERNAL_URL|SERVICE_TOKEN)=' /opt/TelegramOnly/.env)"
```

Что смотреть в выводе:

| Строка | Норма | Если не так |
|---|---|---|
| `starting server version=…` | текущая версия файлового менеджера | служба не запускалась с прошлой загрузки — проверьте `systemctl status rust-file-manager` |
| `SESSION_SECRET задан` | `1` | без него каждый перезапуск выкидывает всех из мини-приложения; см. `DEPLOY.md` |
| `df` | свободно 3 ГБ и больше | освободите место (часть 3) до сборки |
| `git status --short` бота | пусто | на сервере правили код бота: сохраните правки, `git pull` их не примет |
| `compose-файлы` | `…compose.yaml` или `…compose.yaml,…compose.host.yaml` | — |
| `COMPOSE_FILE` | задан, если в `compose-файлы` есть `compose.host.yaml` | выполните шаг 0б |
| строк `FILES_*` | `3` | бот не связан с файловым менеджером: [DEPLOYwTELEGRAM.md](DEPLOYwTELEGRAM.md) §3.3–3.4 |

## Шаг 0б. Закрепить сеть хоста для бота (один раз)

Нужен только если бот запущен с `compose.host.yaml`, а `COMPOSE_FILE` в
`.env` не задан. Без этого `rebuild_bot.sh` пересоздаст бота в обычной сети
bridge, и он потеряет доступ к mesh-сети и к `127.0.0.1:8091`.

```bash
cd /opt/TelegramOnly
cp -p .env "/root/telegramonly-env.bak-$(date -u +%Y%m%dT%H%M%SZ)"
grep -q '^COMPOSE_FILE=' .env ||
  printf '\n# Бот в сети хоста: docker compose в этом каталоге берёт оба файла\nCOMPOSE_FILE=compose.yaml:compose.host.yaml\n' >> .env
grep '^COMPOSE_FILE=' .env
```

Ожидается `COMPOSE_FILE=compose.yaml:compose.host.yaml`. Пересоздавать бота
сейчас не нужно: строка сработает при следующей пересборке. Копию `.env` с
секретами удалите после проверки (часть 3).

---

# Часть 1. Файловый менеджер

## 1.1. Собрать новый выпуск в фоне

Сборка идёт в отдельном каталоге, работающий файловый менеджер всё это время
обслуживает пользователей. Если на сервере есть старый клон с локальными
правками или другим remote, не собирайте в нём: блок создаст отдельный.

```bash
. "$HOME/.cargo/env"
RFM_REF=v1.7.0                  # ← тег выпуска; пока тега нет — main
RFM_BUILD_DIR=/root/rfm-build   # ← ваш каталог сборки
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
- коммит выпуска (сверьте с GitHub) и `version = "1.7.0"`;
- `Сборка запущена`.

Если `git checkout` отказался из-за локальных изменений в каталоге сборки,
ничего не перезаписывайте: соберите в новом каталоге, задав другой
`RFM_BUILD_DIR`.

**Как следить.** Журнал сборки — файл `/root/rfm-build.log`:

```bash
tail -3 /root/rfm-build.log
```

- `Compiling …` — сборка ещё идёт; повторите через пару минут. На последнем
  этапе журнал по нескольку минут не меняется, это нормально.
- `Finished \`release\` profile [optimized] target(s) in …` — сборка готова.
- `error:` — сборка упала: `tail -30 /root/rfm-build.log`. Строки `warning:`
  ошибкой не считаются; их текст можно посмотреть так:
  `grep -A8 '^warning' /root/rfm-build.log | head -30`.

Первая сборка в новом каталоге заняла на рабочем сервере 9 минут. Повторная в
том же каталоге быстрее: зависимости уже скомпилированы.

## 1.2. Резервная копия

Минимум для каждого обновления — прежняя программа, настройки и юнит:

```bash
RFM_UNIT=rust-file-manager.service
RFM_BIN=/usr/local/bin/rust-file-manager
OLD_VER=$(journalctl -u "$RFM_UNIT" --no-pager -o cat | grep 'starting server' | tail -1 | sed -n 's/.*version="\([^"]*\)".*/\1/p')
RFM_BACKUP="/var/backups/rust-file-manager/$(date -u +%Y%m%dT%H%M%SZ)-${OLD_VER:-old}"
install -d -m 0700 "$RFM_BACKUP" &&
cp -p "$RFM_BIN" "$RFM_BACKUP/rust-file-manager" &&
cp -p /etc/rust-file-manager/env "$RFM_BACKUP/env" &&
cp -p /etc/systemd/system/rust-file-manager.service "$RFM_BACKUP/unit.service" &&
ls -la "$RFM_BACKUP" && echo "Резервная копия: $RFM_BACKUP"
```

Ожидаются три файла: `rust-file-manager`, `env`, `unit.service`. Запишите путь
копии: он нужен для отката, в том числе из новой SSH-сессии.

**Полная копия данных.** Делайте её перед крупными выпусками или если давно не
делали. Служба останавливается на время архивации и запускается снова, даже
если архивация не удалась:

```bash
du -sh /var/lib/rust-file-manager; df -h /var/backups | tail -1
systemctl stop rust-file-manager.service &&
tar --acls --xattrs -cpf "$RFM_BACKUP/data.tar" -C /var/lib/rust-file-manager . ;
systemctl start rust-file-manager.service
ls -la "$RFM_BACKUP/data.tar"; systemctl is-active rust-file-manager.service
```

Ожидаются `data.tar` размером с хранилище и `active`. Свободного места на
диске должно быть больше, чем занимает `/var/lib/rust-file-manager`. В архиве
все файлы пользователей и `users.json`: каталог копии доступен только root.

## 1.3. Заменить программу и перезапустить

Новая программа ставится рядом со старой и подменяет её переименованием, так
что работающий файл не перезаписывается.

```bash
RFM_DOMAIN=files.example.com                                # ← ваш домен
RFM_NEW=/root/rfm-build/target/release/rust-file-manager    # ← ваш каталог сборки
RFM_BIN=/usr/local/bin/rust-file-manager
grep -q 'Finished' /root/rfm-build.log && test -x "$RFM_NEW" &&
install -o root -g root -m 0755 "$RFM_NEW" "$RFM_BIN.next" &&
mv -fT "$RFM_BIN.next" "$RFM_BIN" &&
systemctl restart rust-file-manager.service && sleep 3
systemctl is-active rust-file-manager.service
journalctl -u rust-file-manager.service --no-pager -o cat | grep 'starting server' | tail -1
curl -sS -o /dev/null -w 'local /login: %{http_code}\n' http://127.0.0.1:8080/login
curl -sS -o /dev/null -w 'public /login: %{http_code}\n' "https://$RFM_DOMAIN/login"
curl -sS -o /dev/null -w 'public /tg/: %{http_code}\n' "https://$RFM_DOMAIN/tg/"
sed -n 's/^INTERNAL_API_TOKEN=/Authorization: Bearer /p' /etc/rust-file-manager/env |
  curl -sS -H @- http://127.0.0.1:8091/internal/v1/health; echo
```

Ожидается:
- `active`;
- `starting server version="1.7.0" commit="…" … miniapp=true` с временем
  только что прошедшего перезапуска (старая строка ничего не доказывает);
- три ответа `200`;
- `{"success":true,"version":"1.7.0"}` — внутренний API для бота работает.

В браузере войдите на сайт и проверьте версию в футере. Шаблоны и
мини-приложение вшиты в программу: если видите старый вид, обновите страницу
без кэша (Ctrl+F5), а мини-приложение в Telegram закройте и откройте снова.

**Обновление настроек.** Обычно выпуск их не меняет. Если в описании выпуска
есть новые строки для `/etc/rust-file-manager/env`, добавьте их редактором
(`sudoedit /etc/rust-file-manager/env`) до перезапуска. Существующие секреты
не меняйте.

## 1.4. Откат файлового менеджера

Если после обновления что-то сломалось, верните прежнюю программу. Данные
трогать не нужно: новые выпуски только добавляют поля в `users.json` и новые
каталоги (например, `exchange/` в 1.6.0), а старые выпуски лишнее игнорируют.

```bash
ls -d /var/backups/rust-file-manager/*/
RFM_BACKUP=/var/backups/rust-file-manager/ВСТАВЬТЕ_КАТАЛОГ   # ← ваш путь из шага 1.2
test -f "$RFM_BACKUP/rust-file-manager" &&
cp -p "$RFM_BACKUP/rust-file-manager" /usr/local/bin/rust-file-manager.rollback &&
mv -fT /usr/local/bin/rust-file-manager.rollback /usr/local/bin/rust-file-manager &&
systemctl restart rust-file-manager.service && sleep 3
systemctl is-active rust-file-manager.service
journalctl -u rust-file-manager.service --no-pager -o cat | grep 'starting server' | tail -1
```

Ожидается прежняя версия в строке запуска. Архив `data.tar` поверх текущих
данных не распаковывайте: потеряются файлы, загруженные после копии.
Восстанавливайте его, только если повреждены сами данные.

---

# Часть 2. Бот TelegramOnly

## 2.1. Обновить код и пересобрать

```bash
cd /opt/TelegramOnly
git remote get-url origin | sed -E 's#//[^@/]*@#//***@#'
git pull --ff-only origin main && git log --oneline -1 && grep -m1 '^version' pyproject.toml
```

Первая строка показывает, откуда сервер берёт код; токен, если он зашит в
адрес, заменяется на `***`. Ожидаются новый коммит и `version = "3.25.0"`.

Если `git pull` просит пароль или пишет `Authentication failed`, пароль — это
GitHub-токен на чтение репозитория. Если пишет `Not possible to fast-forward`,
на сервере есть свои коммиты: остановитесь и разберитесь, `reset --hard` не
делайте.

Затем пересборка только бота:

```bash
cd /opt/TelegramOnly
bash scripts/rebuild_bot.sh --prune
docker inspect -f 'сеть: {{.HostConfig.NetworkMode}}' telegram-helper-lite
```

Ожидается:
- в блоке «Результат» строка `✓ Версия в контейнере: 3.25.0 — совпадает с pyproject.toml`;
- та же сеть, что в шаге 0: `host` или `telegramonly_default`.

Скрипт сам обходит падение сборки из-за DNS у BuildKit, пересоздаёт только
контейнер бота и сверяет версию. `--prune` после сборки чистит кэш Docker.

Если `COMPOSE_FILE` в `.env` не задан, а бот в сети хоста, и вы не делали
шаг 0б, запускайте с приставкой:
`COMPOSE_FILE=compose.yaml:compose.host.yaml bash scripts/rebuild_bot.sh --prune`.

Если `scripts/rebuild_bot.sh` не найден, на сервере очень старая версия бота:
сначала выполните `git pull` из начала этого шага, скрипт появится вместе с
кодом.

## 2.2. Новые строки в `.env` бота

Для 3.25.0 новых обязательных строк нет. Уведомления об «Обмене» включены по
умолчанию; частоту можно поменять строкой `FILES_NOTIFY_MINUTES=10`, выключить
их — строкой `FILES_NOTIFY=false`.

Если правили `.env`, сначала сохраните копию, а после правки пересоздайте
контейнер. `docker compose restart` новые строки не перечитывает.

```bash
cd /opt/TelegramOnly
cp -p .env "/root/telegramonly-env.bak-$(date -u +%Y%m%dT%H%M%SZ)"
# … правка .env редактором …
docker compose up -d --force-recreate telegram-helper
sleep 30
docker logs --since 2m telegram-helper-lite 2>&1 | grep -E 'files|ERROR|Traceback' | tail -8
```

## 2.3. Проверка из Telegram

1. **`/version`** — 3.25.0.
2. **`/files_status`** — файловый менеджер 1.7.0, диск, участники, число
   привязанных.
3. **Кнопка «Файлы»** слева от поля ввода на месте. Наберите `/`: список
   команд показывается, как раньше.
4. **«Обмен».** Отправьте участнику тестовый файл. В течение 10 минут ему
   придёт «📥 Администратор прислал вам файл…» с кнопкой «Открыть обмен». В
   журнале бота появится строка об отправке:

   ```bash
   docker logs --since 15m telegram-helper-lite 2>&1 | grep 'files: событий обмена'
   ```

   Ожидается `files: событий обмена 1 — уведомлений отправлено 1, ошибок 0`.

## 2.4. Откат бота

```bash
cd /opt/TelegramOnly
git log --oneline -5
git checkout --detach ВСТАВЬТЕ_КОММИТ &&      # ← коммит до обновления, из списка выше
bash scripts/rebuild_bot.sh
```

Скрипт без `--pull` собирает ровно ту версию, что сейчас в каталоге. Вернуться
на основную ветку потом: `git checkout main && bash scripts/rebuild_bot.sh`.
Пока каталог в состоянии `detached HEAD`, `git pull` не работает.

Если сломался только `.env`, верните копию из шага 2.2:
`cp -p /root/telegramonly-env.bak-<время> .env`, затем
`docker compose up -d --force-recreate telegram-helper`.

---

# Часть 3. После обновления

## 3.1. Уборка

Когда всё проработает пару дней, удалите лишнее:

```bash
ls -la /var/backups/rust-file-manager/
ls -d /root/rfm-build* /root/rfm-build*.log /root/telegramonly-env.bak-* 2>/dev/null
du -sh /root/rfm-build*/target 2>/dev/null
df -h /
```

Что можно удалять:
- **Старые резервные копии файлового менеджера.** Оставьте одну-две последние:
  `rm -rf /var/backups/rust-file-manager/<старый каталог>`.
- **Лишние каталоги сборки.** Оставьте один текущий: в нём уже скомпилированы
  зависимости, и следующая сборка будет быстрее. Остальные — `rm -rf <каталог>`.
  Вместо удаления каталога можно освободить 1–2 ГБ командой
  `cd <каталог> && cargo clean`.
- **Копии `.env` бота** `/root/telegramonly-env.bak-*`. В них токен бота и все
  секреты, так что удалите их, как только убедились, что бот работает:
  `rm /root/telegramonly-env.bak-*`.
- **Кэш Docker** чистит `rebuild_bot.sh --prune` и еженедельный таймер
  `telegramonly-maintenance.timer`.

## 3.2. Теги выпусков

Теги ставятся на вашем компьютере, в клоне репозитория, после слияния PR.
Тег должен указывать на тот коммит, который собран на сервере: сверьте с
`git log --oneline -1` из шага 1.1.

```bash
git fetch origin
git log --oneline -1 origin/main
git tag -a v1.7.0 origin/main -m "Release v1.7.0: exchange notifications"
git push origin v1.7.0
```

---

## Чего не делать

| Не делать | Почему и как правильно |
|---|---|
| `docker compose down`, чтобы перезапустить бота | Гасит весь проект: Dockhand, прокси сокета, сети. Правильно — `docker compose up -d --force-recreate telegram-helper` |
| `docker compose restart` после правки `.env` | Не перечитывает `.env`. Правильно — `up -d --force-recreate telegram-helper` |
| Две сборки сразу (cargo и docker) на маленьком VPS | Может кончиться память и подвиснуть SSH. Собирайте по очереди |
| Пересоздавать бота много раз подряд | На VPS с 1 ГБ это запускает swap-шторм и блокирует SSH (TelegramOnly `SERVER_OPS.md`) |
| Копировать новую программу поверх работающей | `Text file busy` или повреждённая служба. Правильно — `install` в `.next` и `mv -fT` (шаг 1.3) |
| `cargo update` или сборка без `--locked` | Меняет версии зависимостей. Собирайте строго по `Cargo.lock` |
| Менять `SESSION_SECRET` или `INTERNAL_API_TOKEN` при обновлении | Все выйдут из системы; бот потеряет доступ к файловому менеджеру, пока ключи не совпадут снова |
| Выводить `env` и `.env` целиком на экран или в чат | В них пароли и ключи. Смотрите отдельные строки через `grep -E '^(ИМЯ)='` |

## Частые проблемы

| Симптом | Причина и что сделать |
|---|---|
| `cargo: command not found` | Загрузите окружение Rust: `. "$HOME/.cargo/env"` |
| Cargo требует более новый Rust | `rustup update stable`; минимум 1.88 |
| Сборка убита (`SIGKILL`, `signal: 9`) | Не хватило памяти: нужен swap и `CARGO_BUILD_JOBS=1`; не запускайте параллельно сборку бота |
| После перезапуска в журнале старая версия | Новая программа не встала на место: проверьте `Finished` в журнале сборки и путь `RFM_NEW`, повторите шаг 1.3 |
| `public /login` не 200, а `local` — 200 | Проблема в nginx или Cloudflare, не в программе: `nginx -t`, журнал nginx, правила Cloudflare |
| `/files_status`: «файловый менеджер недоступен» | Не задан `INTERNAL_BIND_ADDR`, бот выпал из сети хоста (шаг 0б) или в сети bridge не работает мост `rfm-internal-bridge` |
| `/files_status`: «отклонил токен бота» | `FILES_SERVICE_TOKEN` в `.env` бота ≠ `INTERNAL_API_TOKEN` файлового менеджера |
| `✗ РАСХОЖДЕНИЕ ВЕРСИЙ` в конце `rebuild_bot.sh` | Образ не пересобрался: `docker build --network=host -t telegram-helper-lite:latest .`, затем `docker compose up -d --force-recreate telegram-helper` |
| Бот после пересборки потерял mesh-сеть | Его пересоздали без `compose.host.yaml`: шаг 0б, затем `docker compose up -d --force-recreate telegram-helper` |
| В Telegram при скачивании размер «57 байт» | Файловый менеджер старше 1.5.1: обновите его |
