# Обновление, восстановление и перенос на другой VPS

Первичная установка и справочник `deploy` — [DEPLOY.md](DEPLOY.md).
Короткий порядок выбора действий — [DEPLOY_ALGORITHM.md](DEPLOY_ALGORITHM.md).
Обновление сохраняет домен, настройки, пользователей и загруженные файлы.

## Обновление одной командой

```bash
post_deploy
```

### Порядок работы

1. **Показывает, что стоит на сервере.** Если файловый менеджер не
   установлен, предлагает `deploy` и завершается. В терминале спрашивает,
   нужен ли архив данных (по умолчанию нет), показывает план и просит
   подтверждение. `--yes` убирает эти вопросы; запуск через stdin сохраняет
   прежнее неинтерактивное поведение.
2. **Обновляет файловый менеджер** из `main` (или из закреплённого тега):
   - если на сервере уже этот коммит и служба работает, ничего не делает;
   - иначе собирает в фоне в прежнем каталоге сборки. Зависимости там уже
     скомпилированы, поэтому сборка занимает несколько минут. При памяти
     меньше 3,5 ГБ без swap сначала создаёт swap 2 ГБ (если на диске свободно
     от 3 ГБ), чтобы сборка не вызвала нехватку памяти у других служб;
   - сохраняет прежнюю программу в `/var/backups/rust-file-manager/`;
   - атомарно заменяет программу и перезапускает службу;
   - проверяет, что запущен именно новый коммит, а сайт, мини-приложение и
     внутренний API (если включены) отвечают;
   - **если проверки не прошли, сам возвращает прежнюю версию** и сообщает об
     этом.
3. **Проверяет маршрут загрузок в nginx** управляемых сайтов и применяет
   конфигурацию через `nginx -t` и reload.
4. **Убирает старое:** держит три последние копии программы и архивов.
5. **Пишет итог:** что обновлено, с какой версии на какую.

Команда не трогает другие программы на сервере: Docker-контейнеры, VPN,
ботов и их настройки. Telegram-бот, если он связан с файловым менеджером,
обновляется своими средствами.

### Параметры

| Команда | Что делает |
|---|---|
| `post_deploy` | обновить файловый менеджер, если он устарел |
| `post_deploy --check` | показать обновление без замены программы, env и команд |
| `post_deploy --ref v1.8.0` | поставить выпуск по тегу и **остаться на нём**; следующие запуски тоже возьмут v1.8.0 |
| `post_deploy --ref main` | вернуться к последней версии из `main` |
| `post_deploy --force` | пересобрать и перезапустить, даже если версия та же |
| `post_deploy --backup-data` | перед заменой программы сохранить ещё и архив данных; служба на это время останавливается |
| `post_deploy --yes` | согласованное обновление без вопросов |

Прежний параметр `--only fm` принимается и ничего не меняет. Ошибки nginx
возвращаются как ненулевой код выхода.

Код завершения `0` значит, что всё обновлено или уже актуально. `1` значит, что
что-то не удалось: подробности в итоге.

### Первый запуск на сервере, установленном до 1.8.0

Достаточно поставить команды из [DEPLOY.md](DEPLOY.md) и выполнить `post_deploy`. Команда
найдёт:

- прежний каталог сборки `/root/rfm-build…` по его git-репозиторию;
- домен — по настройкам файлового менеджера.

Если на сервере вместо этих команд стоит их полная версия `tgo-vps` (вместе с
ботом автора), обновляйте ею: `setup` этой версии её не заменяет. Если на
сервере есть клон бота автора (`/opt/TelegramOnly`) с полной версией команд,
`deploy` и `post_deploy` сами переходят на неё и продолжают работу с теми же
параметрами; без неё обновляют только файловый менеджер и пишут об этом в итоге.

---

## Повторная установка и ошибка загрузки фото

Для пересборки установленного сервера выполните `post_deploy --force`.
Архив данных — по необходимости, затем подтвердите план. Архив данных по
умолчанию выключен; прежний бинарник для отката сохраняется автоматически.
Сброс ОС и повторный ввод пароля не требуются. Полный порядок —
[DEPLOY_ALGORITHM.md, раздел 6](DEPLOY_ALGORITHM.md#6-повторная-установка-программы-после-исправления).

Обновление FM исправляет активные управляемые сайты nginx: коллекция
`/api/v1/uploads` проксируется без 301; API принимает адрес также со слешем.
Конфигурация проверяется и применяется через reload; остальные настройки
HTTPS сохраняются. При ошибке изменённые строки возвращаются. `--check` и
свой прокси `HTTPS_MODE=none` nginx не изменяют. После обновления закройте
Mini App, откройте снова и проверьте фото.

## Ручное обновление файлового менеджера

### 0. Проверить работающую службу и реальные пути

```bash
uname -sm
sudo systemctl show rust-file-manager.service \
  -p ActiveState -p SubState -p MainPID -p User -p WorkingDirectory \
  -p FragmentPath -p ExecStart -p EnvironmentFiles
sudo journalctl -u rust-file-manager.service --no-pager -o cat |
  grep 'starting server' | tail -1
free -h
df -h / /var/tmp /usr/local/bin
```

`ExecStart` показывает установленный бинарник, `EnvironmentFiles` — путь
настроек. Не публикуйте содержимое env, хеши паролей, пользователей и ключи.
Если служба не найдена, эти команды установки пока применять нельзя.
Проверьте фактические `UPLOAD_DIR` и `USERS_FILE` локально на сервере, не
копируя секреты в чат или репозиторий.

Для стандартной раскладки из DEPLOY.md задайте переменные ниже. Замените
пути, адрес и имя службы на подтверждённые значения своей установки.
Вместо `EXPECTED_FULL_COMMIT_SHA` укажите полный коммит выбранного выпуска из доверенного
репозитория перед выполнением команд:

```bash
RFM_UNIT=rust-file-manager.service
RFM_BIN=/usr/local/bin/rust-file-manager
RFM_DATA=/var/lib/rust-file-manager
RFM_ENV=/etc/rust-file-manager/env
RFM_LOCAL_URL=http://127.0.0.1:8080/login
RFM_PUBLIC_URL=https://files.example.com/login
RFM_RELEASE=main
RFM_COMMIT=EXPECTED_FULL_COMMIT_SHA
sudo test -f "$RFM_BIN" && sudo test ! -L "$RFM_BIN" &&
sudo test -d "$RFM_DATA" && sudo test -f "$RFM_ENV" &&
sudo systemctl is-active "$RFM_UNIT"
```

Проверка рассчитана на обычный файл бинарника. Если `ExecStart` использует
симлинк на каталог релизов, сохраните и обновляйте именно эту схему.

Примеры рассчитаны на типовой юнит из проекта. Значения домена, IP,
объёма памяти и версию работающей машины определяйте на своём VPS локально;
эти сведения не фиксируются в публичном репозитории. CLI-команда
`rust-file-manager --version` не реализована: используйте журнал и футер.

### 1. Выбрать выпуск и собрать отдельно

Шаблоны и Mini App встроены в бинарник, поэтому их обновление требует
пересборки. Для текущего кода нужен Rust **1.88 или новее**. Перед переходом
между выпусками проверьте изменения форматов данных и настроек в
[истории коммитов](https://github.com/kureinmaxim/rust-file-manager/commits/main/).

Исходники: [GitHub](https://github.com/kureinmaxim/rust-file-manager/tree/main).
Зафиксируйте проверенный полный коммит в `RFM_COMMIT` выше и используйте
`Cargo.lock`. Если ветка сдвинулась после выбора коммита, проверка ниже
остановит сборку: сначала пересмотрите выбранный выпуск.

```bash
command -v cargo
rustc --version
cargo --version
```

Если Rust установлен через rustup, но отсутствует в PATH:

```bash
. "$HOME/.cargo/env"
```

При слишком старом toolchain обновите его через `rustup update stable`.
Если действует override на старую версию, после клонирования выберите
`rustup override set stable` в сборочном каталоге и проверьте `rustc --version`.
Первичная установка Rust и системных пакетов — в DEPLOY.md.

Получите выпуск в новый каталог; действующий клон и его локальные изменения
останутся на месте:

```bash
RFM_BUILD_DIR=$(mktemp -d "$HOME/rfm-build.XXXXXX")
git clone --depth 1 --branch "$RFM_RELEASE" \
  https://github.com/kureinmaxim/rust-file-manager.git "$RFM_BUILD_DIR" &&
cd "$RFM_BUILD_DIR" &&
test "$(git rev-parse HEAD)" = "$RFM_COMMIT" &&
test -z "$(git status --porcelain)" &&
CARGO_BUILD_JOBS=1 cargo build --release --locked --bin rust-file-manager
```

Старая служба продолжает работать во время сборки. Продолжайте только после
успешного завершения команды. Одно задание сборки выбрано для VPS с небольшой памятью. Существующий
swap сохраняйте; создавать или удалять swap для обычного обновления не нужно.

Если `CARGO_TARGET_DIR` или `build.target` настроены нестандартно, подставьте
фактический путь итогового Linux-бинарника вместо `target/release/...` ниже.
Сборка на macOS не создаёт подходящий VPS-бинарник без отдельной Linux-среды
или настроенной кросс-компиляции.

<a id="backup-before-update"></a>

### 2. Сохранить прежний бинарник и настройки

Создайте отдельный каталог с закрытыми правами. Не перезаписывайте один
`*.prev`: при повторной попытке он может уже содержать нерабочую версию.

```bash
RFM_BACKUP="/var/backups/rust-file-manager/$(date -u +%Y%m%dT%H%M%SZ)-$$"
RFM_UNIT_FILE=$(sudo systemctl show "$RFM_UNIT" -p FragmentPath --value)
sudo install -d -o root -g root -m 0700 "$RFM_BACKUP" &&
sudo cp -p -- "$RFM_BIN" "$RFM_BACKUP/rust-file-manager" &&
sudo cp -p -- "$RFM_ENV" "$RFM_BACKUP/env" &&
sudo cp -p -- "$RFM_UNIT_FILE" "$RFM_BACKUP/unit.service"
printf 'Резервная копия: %s\n' "$RFM_BACKUP"
```

Также сохраните существующие systemd drop-in файлы, если они есть.
Настройки не нужно заново генерировать или заменять шаблоном.

Убедитесь в наличии свежей копии uploads и `users.json`. Для согласованной
копии используйте snapshot хранилища либо остановку записи на время копии.
Если оба находятся внутри подтверждённого `RFM_DATA`, пример ниже сохраняет
весь этот каталог и возобновляет прежнюю службу даже при ошибке архивации:

```bash
sudo env RFM_UNIT="$RFM_UNIT" RFM_DATA="$RFM_DATA" RFM_BACKUP="$RFM_BACKUP" \
  bash -euo pipefail <<'SH'
trap 'systemctl start "$RFM_UNIT"' EXIT
systemctl stop "$RFM_UNIT"
tar --acls --xattrs -cpf "$RFM_BACKUP/data.tar" -C "$RFM_DATA" .
SH
```

Этот необязательный блок останавливает сервис на всё время копирования.
Для большого хранилища заранее выберите snapshot/другой способ, проверьте
размер и свободное место. Если данные расположены вне `RFM_DATA`, копируйте
их по реальным путям; архив рабочего каталога сам по себе их не сохранит.

Запишите путь `RFM_BACKUP`: он понадобится при откате или новой SSH-сессии.

### 3. Атомарно заменить бинарник и перезапустить

Установка создаёт файл рядом с действующим бинарником, затем выполняет
переименование в пределах той же файловой системы. Это сохраняет целый
старый файл до завершения подготовки и не пишет в работающий executable.
Пример сохраняет владельца, группу и обычные права установленного бинарника:

```bash
RFM_NEXT="${RFM_BIN}.next-${RFM_RELEASE}-$$"
RFM_BIN_UID=$(sudo stat -c '%u' "$RFM_BIN")
RFM_BIN_GID=$(sudo stat -c '%g' "$RFM_BIN")
RFM_BIN_MODE=$(sudo stat -c '%a' "$RFM_BIN")
sudo install -o "$RFM_BIN_UID" -g "$RFM_BIN_GID" -m "$RFM_BIN_MODE" \
  "$RFM_BUILD_DIR/target/release/rust-file-manager" "$RFM_NEXT" &&
sudo mv -fT -- "$RFM_NEXT" "$RFM_BIN" &&
sudo systemctl restart "$RFM_UNIT"
```

Если установлены специальные ACL, capabilities или SELinux-контекст,
примените действующую политику установки до рестарта; приведённый пример
сохраняет только обычного владельца/группу/mode. Для юнита из проекта таких
дополнительных атрибутов не требуется.

Юнит, env, nginx и Cloudflare в этом выпуске обновлять не нужно. Если вы
меняете юнит отдельно, сохраните локальные настройки и выполните
`systemctl daemon-reload` перед перезапуском. Смена секретов сбрасывает
сессии; действующий `SESSION_SECRET` сохраните.

### 4. Проверить службу, версию и публичный сайт

```bash
sudo systemctl is-active "$RFM_UNIT"
sudo systemctl status "$RFM_UNIT" --no-pager
sudo journalctl -u "$RFM_UNIT" --no-pager -o cat |
  grep 'starting server' | tail -1
curl --fail --silent --show-error --max-time 10 \
  -o /dev/null -w '%{http_code}\n' "$RFM_LOCAL_URL"
curl --fail --silent --show-error --max-time 20 \
  -o /dev/null -w '%{http_code}\n' "$RFM_PUBLIC_URL"
```

Ожидаются `active`, строка с версией выбранного выпуска и его коммитом без `-dirty`
и оба HTTP-ответа 200. Сравните время строки запуска с текущим рестартом:
старая успешная запись не подтверждает запуск новой версии.

В браузере после входа проверьте футер, личные/общие зоны, загрузку и
скачивание отдельного тестового файла, папки бэкапов **HA / Серверы / Project**,
поиск, переименование и вид на телефоне. Существующие файлы для теста не
переименовывайте и не удаляйте. При необходимости обновите страницу без
браузерного кэша.

Если localhost отвечает, а домен нет, проверьте журнал действующего reverse
proxy и Cloudflare отдельно. При HTTP 413 учитывайте три разных лимита:
на файл в приложении, на тело запроса в nginx, на запрос в Cloudflare.
Пользовательский лимит 500 MB не снимает ограничение прокси Cloudflare.

### 5. Откат бинарника

При неуспешной проверке верните сохранённый бинарник, не восстанавливая
данные поверх новых загрузок. В той же сессии `RFM_BACKUP` уже задан; после
переподключения сначала укажите записанный путь к нужной копии.

```bash
sudo test -f "$RFM_BACKUP/rust-file-manager" &&
sudo systemctl stop "$RFM_UNIT" &&
sudo cp -p -- "$RFM_BACKUP/rust-file-manager" "${RFM_BIN}.rollback-$$" &&
sudo mv -fT -- "${RFM_BIN}.rollback-$$" "$RFM_BIN" &&
sudo systemctl start "$RFM_UNIT"
sudo systemctl is-active "$RFM_UNIT"
sudo journalctl -u "$RFM_UNIT" --no-pager -o cat |
  grep 'starting server' | tail -1
```

Возврат бинарника достаточен только при совместимых форматах данных.
При обновлении с версии до multi-user
первый старт переносит старые категории в `shared/`; такой переход требует
отдельной проверки копии и плана восстановления данных. Не восстанавливайте
старый архив поверх новых файлов без оценки изменений после обновления.

Оставьте копию прежнего бинарника и данных до подтверждения работы. Очистка
сборочного каталога и старых копий — отдельный шаг после успешной проверки.

## Частые проблемы

| Симптом | Проверка и действие |
|---|---|
| `cargo: command not found` | Загрузить `$HOME/.cargo/env` для пользователя сборки; собирать без sudo |
| Cargo требует более новый Rust | Проверить `rustc --version` и выбранный toolchain; минимум 1.88 |
| Сборка убита (`SIGKILL`) | Проверить память; повторить с `CARGO_BUILD_JOBS=1`, при необходимости собрать на другой Linux-машине |
| `Text file busy` | Не копировать поверх запущенного файла; использовать соседний файл и rename из шага 3 |
| `Exec format error` | Проверить Linux и архитектуру; macOS-бинарник не подходит |
| Сервис не стартует | Прочитать `journalctl -u "$RFM_UNIT" -n 30 --no-pager`, проверить пути и права локально; не публиковать секреты |
| HTTP 413 | Проверить лимиты приложения, nginx и Cloudflare; multipart требует небольшого запаса |
| Старый интерфейс после рестарта | Сравнить `ExecStart`, время запуска, версию/коммит в журнале и футере; шаблоны встроены в бинарник |

Shell-синтаксис примеров проверен. Эти команды не были выполнены на рабочем
VPS; подтвердите фактическую раскладку шагом 0 перед применением.

## Перенос на другой VPS

Инструкция для `rust-file-manager`, подготовлена **06.10.2026**. Перенос
сохраняет домен `files.example.com`, файлы, пользователей, приглашения и настройки.
Первичная установка — [DEPLOY.md](DEPLOY.md), обновление версии на том же
сервере — [начало этой инструкции](#обновление-одной-командой).

Команды рассчитаны на Ubuntu/Debian, systemd, **Bash и root на обоих VPS**.
Работайте в двух подписанных вкладках Tabby: **СТАРЫЙ VPS** и **НОВЫЙ VPS**.
Новый IP и SSH-порты в примерах нужно заменить своими; реальный перенос по
этой инструкции пока не выполнялся.

### Если меняется только IP того же VPS

Перенос данных и переустановка программы не нужны. Сохраните прежние
значения DNS, настройте новый адрес по инструкции провайдера, проверьте
SSH через новый IP, firewall и доступность proxy. Если nginx/SSH привязаны
к конкретному старому IP, исправьте соответствующие listen-настройки.
`BIND_ADDR=127.0.0.1:8080` менять не нужно.

После проверки HTTPS обновите A/AAAA нужного hostname в Cloudflare по
разделу 7. Доменный сертификат не требует замены только из-за нового IP.
Записи tailnet/split DNS меняются лишь при изменении соответствующего
внутреннего адреса. Для полноценного переезда на другую машину — шаги ниже.

### 0. Зафиксировать исходное состояние

Ниже приведены **только примеры**, а не параметры реального сервера:

| Параметр | Значение |
|---|---|
| Публичный домен | `files.example.com`, через Cloudflare |
| Пример старого IPv4 | `192.0.2.10` |
| ОС / архитектура | Linux x86_64 |
| Служба | `rust-file-manager.service` |
| Пользователь службы | `filemgr` |
| Рабочий каталог | `/var/lib/rust-file-manager` |
| Бинарник | `/usr/local/bin/rust-file-manager` |
| Env-файл | `/etc/rust-file-manager/env` |
| Юнит | `/etc/systemd/system/rust-file-manager.service` |

Перед переносом определите версию процесса, SSH-порты и фактические
`UPLOAD_DIR`/`USERS_FILE` на своих серверах локально. Не публикуйте env,
`users.json`, пароли, ключи или ссылки-приглашения.

**НА ОБОИХ VPS:** задайте одинаковые переменные, заменив домен, оба
IP `192.0.2.10`/`203.0.113.10` и порты своими значениями. Эти IP
зарезервированы для документации и не являются готовыми серверами.

```bash
RFM_DOMAIN=files.example.com
RFM_OLD_IP=192.0.2.10
RFM_NEW_IP=203.0.113.10
RFM_OLD_SSH_PORT=22
RFM_NEW_SSH_PORT=22
RFM_UNIT=rust-file-manager.service
RFM_BIN=/usr/local/bin/rust-file-manager
RFM_DATA=/var/lib/rust-file-manager
RFM_ENV=/etc/rust-file-manager/env
RFM_UNIT_FILE=/etc/systemd/system/rust-file-manager.service
RFM_NEW_HOST="root@${RFM_NEW_IP}"
RFM_OLD_HOST="root@${RFM_OLD_IP}"
```

Команды ниже предполагают, что uploads и `users.json` находятся внутри
`RFM_DATA`. Если пути другие, включите их во **все** этапы копирования,
проверки и отката и согласуйте права и `ReadWritePaths` на новом узле.
Каталог назначения должен быть выделен исключительно этому приложению.

**НА СТАРОМ VPS:**

```bash
hostname
uname -sm
systemctl show "$RFM_UNIT" \
  -p ActiveState -p User -p WorkingDirectory -p ExecStart \
  -p EnvironmentFiles -p FragmentPath -p DropInPaths
journalctl -u "$RFM_UNIT" --no-pager -o cat | grep 'starting server' | tail -1
id filemgr
du -sh "$RFM_DATA"
df -h / /var/backups
apt update && apt install -y rsync
rsync --version | head -1
ssh -p "$RFM_NEW_SSH_PORT" "$RFM_NEW_HOST" 'hostname; uname -sm'
```

Проверьте SSH fingerprint нового VPS по панели/консоли провайдера. Пароли
вводите в терминале; не переносите приватные SSH-ключи между VPS ради rsync.
Нужен rsync 3.x на Linux; встроенный rsync macOS не подходит для всех флагов.

Зафиксируйте версию из журнала или футера после входа, текущие A/AAAA,
proxy status, SSL/TLS, Origin Rules, порт HTTPS и способ обновления сертификата.
Если используется Cloudflare Load Balancer, Tunnel или отдельный override
origin-адреса, одного изменения A-записи недостаточно: учитывайте этот маршрут.

### 1. Подготовить новый VPS без переключения DNS

Переносите текущую версию программы; обновление выпуска выполняйте отдельным
шагом после переезда. Для прямого копирования бинарника нужны Linux x86_64
и совместимые системные библиотеки. При другой архитектуре/ABI соберите
**ту же версию** из исходников по DEPLOY.md; не заменяйте её автоматически
произвольным HEAD основной ветки. Для сборки текущего кода требуется Rust 1.88+;
для переноса старого выпуска проверьте требования именно его зависимостей.

**НА НОВОМ VPS:**

```bash
hostname
uname -sm
free -h
df -h / /var/lib /usr/local/bin
apt update && apt install -y rsync curl ca-certificates
test ! -e "$RFM_BIN" && test ! -e "$RFM_ENV" && test ! -e "$RFM_UNIT_FILE"
id filemgr >/dev/null 2>&1 ||
  useradd --system --user-group --home-dir "$RFM_DATA" \
    --shell /usr/sbin/nologin filemgr
install -d -o filemgr -g filemgr -m 0750 "$RFM_DATA"
install -d -o root -g root -m 0755 /etc/rust-file-manager
```

Если проверка отсутствия бинарника/env/юнита вернула ошибку, остановитесь:
на получателе уже есть установка, её нельзя перезаписывать этим сценарием.
Диска должно хватать на данные, временные файлы и резервные копии. Пока не
запускайте приложение с пустым хранилищем. На новом VPS не генерируйте новый
admin-хеш, SESSION_SECRET или пустой `users.json`.

### 2. Сохранить резервную копию и предварительно скопировать данные

**НА СТАРОМ VPS:** сохраните бинарник, env и юнит в закрытом каталоге:

```bash
RFM_BACKUP="/var/backups/rust-file-manager/move-$(date -u +%Y%m%dT%H%M%SZ)-$$"
install -d -m 0700 "$RFM_BACKUP" &&
cp -p -- "$RFM_BIN" "$RFM_BACKUP/rust-file-manager" &&
cp -p -- "$RFM_ENV" "$RFM_BACKUP/env" &&
cp -p -- "$RFM_UNIT_FILE" "$RFM_BACKUP/unit.service"
printf 'Резервная копия: %s\n' "$RFM_BACKUP"
```

Сохраните также systemd drop-in файлы и настройки виртуального хоста.
Подготовьте копию данных/snapshot; согласованную копию получают при остановке
записи в шаге 5. Не размещайте её внутри `RFM_DATA`.

Перед копированием убедитесь, что новый каталог не содержит чужие данные.
**НА СТАРОМ VPS:** первый проход выполняется, пока приложение ещё работает:

```bash
rsync -aHAX --chown=filemgr:filemgr --info=progress2 \
  -e "ssh -p $RFM_NEW_SSH_PORT" \
  "$RFM_DATA/" "$RFM_NEW_HOST:$RFM_DATA/" &&
rsync -a --chown=root:root --chmod=F600 \
  -e "ssh -p $RFM_NEW_SSH_PORT" "$RFM_ENV" "$RFM_NEW_HOST:$RFM_ENV" &&
rsync -a --chown=root:root \
  -e "ssh -p $RFM_NEW_SSH_PORT" "$RFM_UNIT_FILE" "$RFM_NEW_HOST:$RFM_UNIT_FILE"
```

Завершающий `/` означает копирование содержимого каталога. Сохраняются
личные `home/`, общие `shared/`, бэкапы **Серверы / HA / Project** и JSON
пользователей/приглашений. Предварительная копия ещё не является согласованной.
Владелец данных назначается по имени `filemgr`, независимо от отличий UID.
Если есть дополнительные ACL с другими пользователями или SELinux-метки,
проверьте их отображение на новой системе отдельно. Симлинки копируются
как ссылки; внешние цели не переносятся автоматически.

Если у службы есть стандартный drop-in каталог, скопируйте его:

```bash
if test -d "${RFM_UNIT_FILE}.d"; then
  rsync -a --chown=root:root -e "ssh -p $RFM_NEW_SSH_PORT" \
    "${RFM_UNIT_FILE}.d/" "$RFM_NEW_HOST:${RFM_UNIT_FILE}.d/"
fi
```

Drop-in файлы вне этого пути, например в `/run/systemd/system`, учитывайте
по `DropInPaths`. Не копируйте все системные конфиги других сервисов.

Для совместимого Linux **НА СТАРОМ VPS** скопируйте текущий бинарник:

```bash
rsync -a --chown=root:root -e "ssh -p $RFM_NEW_SSH_PORT" \
  "$RFM_BIN" "$RFM_NEW_HOST:$RFM_BIN"
```

**НА НОВОМ VPS:** проверьте права, библиотеки и загрузите юнит:

```bash
stat -c '%U:%G %a %n' "$RFM_BIN" "$RFM_ENV" "$RFM_DATA"
ldd "$RFM_BIN"
systemctl daemon-reload
systemctl disable --now "$RFM_UNIT"
```

Бинарник должен быть исполняемым, env — root:root с mode 600, данные доступны
`filemgr`. При `not found` в ldd устраните несовместимость до запуска.
Статически связанный бинарник может сообщить, что он не dynamic executable.
Сохраните значения ADMIN_PASSWORD_HASH и SESSION_SECRET без изменений.
Если секрет сессий прежде отсутствовал/был некорректным, пользователям
потребуется войти заново. Не выводите env в логи для проверки.

### 3. Подготовить HTTPS и маршруты нового VPS

Перенесите или создайте **только конфигурацию домена файлового менеджера**:
nginx/Caddy, сертификат с ключом и необходимые настройки автоматического
продления. Общую конфигурацию proxy не заменяйте целиком.

- Для действующей схемы Cloudflare + Origin CA + порт 2083 сохраните этот
  порт, hostname, Full (strict) и Origin Rule; см. [GUIDE_cloudflare.md](GUIDE_cloudflare.md).
- Можно перенести существующую пару сертификат/ключ через SSH или выпустить
  новый Origin CA для того же hostname. Ключ храните с закрытыми правами.
  Старый сертификат не отзывайте до завершения окна отката.
- Для Let's Encrypt подготовьте сертификат до переключения: DNS-01 либо
  перенос действующего сертификата с его ключом и схемой продления. У Certbot
  `live/` содержит ссылки на `archive/`; копирования одного `live/` недостаточно.
  HTTP-01 обычно попадёт на старый сервер, пока DNS указывает туда.

Откройте на новом VPS реальный SSH-порт и порты выбранной HTTPS-схемы.
Учтите firewall в панели провайдера и ограничения по адресам Cloudflare.
8080 оставьте на localhost. Учитывайте лимиты multipart/nginx/Cloudflare
из DEPLOY.md; один перенос IP не увеличивает допустимый размер запроса.

**НА НОВОМ VPS**, если используется nginx:

```bash
nginx -t && systemctl reload nginx
```

Если используется tailnet, зарегистрируйте новый узел отдельно и подготовьте
новые ACL/маршруты. Не клонируйте состояние tailscaled действующего узла.
До переключения split DNS старый внутренний адрес должен оставаться доступен.

### 4. Проверить новый сервер до изменения DNS

**НА НОВОМ VPS:** запустите только для проверки:

```bash
systemctl start "$RFM_UNIT"
systemctl is-active "$RFM_UNIT"
journalctl -u "$RFM_UNIT" --no-pager -o cat | grep 'starting server' | tail -1
curl --fail --silent --show-error --max-time 10 \
  -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/login
```

Ожидаются active и 200, версия соответствует перенесённому бинарнику.
Если `RUST_LOG` скрывает запись запуска, проверьте версию в футере после входа.
При проверке не создавайте реальные аккаунты/приглашения и не меняйте файлы.

С административной машины либо VPS проверьте именно **новый origin**, сохраняя
hostname/SNI. Для публично доверенного сертификата на 443:

```bash
curl --fail --silent --show-error --max-time 20 \
  --resolve "$RFM_DOMAIN:443:$RFM_NEW_IP" \
  -o /dev/null -w '%{http_code}\n' "https://$RFM_DOMAIN/login"
```

Для Origin CA на 2083 загрузите подходящий RSA/ECC корневой сертификат из
[официальной документации Cloudflare](https://developers.cloudflare.com/ssl/origin-configuration/origin-ca/#cloudflare-origin-ca-root-certificate)
на машину проверки. Подставьте путь к этому публичному CA-файлу:

```bash
RFM_CA=/root/cloudflare-origin-ca-root.pem
curl --fail --silent --show-error --max-time 20 --cacert "$RFM_CA" \
  --resolve "$RFM_DOMAIN:2083:$RFM_NEW_IP" \
  -o /dev/null -w '%{http_code}\n' "https://$RFM_DOMAIN:2083/login"
```

Origin CA обычно не доверяется браузером напрямую; `curl -k` не подтверждает
доверие сертификату. Если firewall допускает только Cloudflare, выполняйте
TLS-проверку на новом VPS через `--resolve "$RFM_DOMAIN:2083:127.0.0.1"`.
Доступность origin для Cloudflare проверяется также правилами firewall.
Не отключайте COOKIE_SECURE ради проверки входа через локальный HTTP.

После предварительной проверки **НА НОВОМ VPS**:

```bash
systemctl stop "$RFM_UNIT"
```

### 5. Остановить запись и выполнить финальную синхронизацию

С этого шага начинается окно недоступности. У нового сервера служба остановлена.
**НА СТАРОМ VPS** остановите файловый менеджер и проверьте состояние:

```bash
systemctl stop "$RFM_UNIT"
systemctl is-active "$RFM_UNIT"
```

Ожидается `inactive` (команда is-active вернёт ненулевой код). Не
останавливайте весь nginx/Caddy и остальные приложения. Если есть внешние
задачи, меняющие эти данные, остановите их запись на время финальной копии.

Сделайте snapshot либо согласованную копию данных. Пример архивирования,
если на старом диске хватает места и все данные находятся внутри RFM_DATA:

```bash
tar --acls --xattrs -cpf "$RFM_BACKUP/data.tar" -C "$RFM_DATA" .
```

**НА СТАРОМ VPS:** сначала просмотрите финальный dry-run:

```bash
rsync -aHAX --chown=filemgr:filemgr --delete-delay --dry-run --itemize-changes \
  -e "ssh -p $RFM_NEW_SSH_PORT" "$RFM_DATA/" "$RFM_NEW_HOST:$RFM_DATA/"
```

`--delete-delay` удаляет лишние файлы **на получателе внутри RFM_DATA**,
чтобы перенести также удаления, произошедшие после первого прохода.
Проверяйте направление, адрес и каталог: назначение должно быть только новой
копией этого приложения. Затем выполните финальный проход и повторите env:

```bash
rsync -aHAX --chown=filemgr:filemgr --delete-delay --info=progress2 \
  -e "ssh -p $RFM_NEW_SSH_PORT" "$RFM_DATA/" "$RFM_NEW_HOST:$RFM_DATA/" &&
rsync -a --chown=root:root --chmod=F600 \
  -e "ssh -p $RFM_NEW_SSH_PORT" "$RFM_ENV" "$RFM_NEW_HOST:$RFM_ENV"
```

До завершения переноса не меняйте конфигурацию службы/proxy. После копии
сравните содержимое через checksum dry-run; это читает все данные и может
занять значительное время:

```bash
rsync -aHAXc --chown=filemgr:filemgr --delete-delay --dry-run --itemize-changes \
  -e "ssh -p $RFM_NEW_SSH_PORT" "$RFM_DATA/" "$RFM_NEW_HOST:$RFM_DATA/"
```

Продолжайте только при коде завершения 0 и отсутствии необъяснённых изменений.
Сравните env без вывода его содержимого: SHA-256 старого и нового файла
должен совпасть. Хеши также остаются в локальном терминале:

```bash
sha256sum "$RFM_ENV"
ssh -p "$RFM_NEW_SSH_PORT" "$RFM_NEW_HOST" \
  "sha256sum '$RFM_ENV'"
```

Если копирование/проверка не удались, DNS пока не меняйте. Убедитесь, что
новый сервис остановлен, и возобновите старый через `systemctl start "$RFM_UNIT"`.
Не запускайте одновременно обе копии для пользователей.

### 6. Запустить новую копию и оставить старую остановленной

**НА НОВОМ VPS:**

```bash
systemctl enable --now "$RFM_UNIT"
systemctl is-active "$RFM_UNIT"
curl --fail --silent --show-error --max-time 10 \
  -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/login
```

Повторите TLS-проверку из шага 4. На старом VPS служба остаётся остановленной;
отключите её автоматический запуск, сохранив бинарник и данные:

**НА СТАРОМ VPS:**

```bash
systemctl disable "$RFM_UNIT"
```

Старые HTTP-запросы могут получать 502, пока маршрут ещё ведёт на старый
узел; при необходимости настройте maintenance-ответ только для этого домена.
Старый узел не должен принимать новые загрузки, удаления или регистрации.

### 7. Переключить DNS и внутренние адреса

В Cloudflare: **DNS → Records → запись `files` → Edit → IPv4 нового VPS → Save**.
Сохраните прежний proxy status (оранжевое облако) и Full (strict). Для IPv6
обновите также AAAA; если IPv6 на новом узле не настроен, не оставляйте старую
AAAA для этого hostname. Если для имени есть несколько A/AAAA, учитывайте
каждую запись, чтобы трафик не попадал на старую машину.

Не меняйте `ha.example.com`, `headscale.example.com`, `proxy.example.com` и другие
сервисы вместе с файловым менеджером. Проверьте Origin Rules: порт и
дополнительные override должны соответствовать новому origin.

При включённом прокси `dig` показывает адреса Cloudflare, а не IPv4 origin;
подтверждайте новый адрес в панели DNS и запросы в журнале нового proxy.
DNS-only записи учитывают заданный TTL; proxied-записи используют Auto.
Не считайте переключение мгновенным по одному ответу DNS.

Если есть Headscale split DNS, отдельное tailnet-имя, локальный `/etc/hosts`
или закладки по IP, обновите их отдельно на новый внутренний адрес; см.
[TAILSCALE.md](TAILSCALE.md). Публичный IPv4 и tailnet-IP — разные адреса.

### 8. Проверить после переключения

С внешней машины и отдельно из tailnet, если используется:

```bash
curl --fail --silent --show-error --max-time 20 \
  -o /dev/null -w '%{http_code}\n' "https://$RFM_DOMAIN/login"
```

В браузере войдите под существующим аккаунтом. Проверьте личные и общие
файлы, бэкапы Серверы/HA/Project, список участников и футер. Затем загрузите
отдельный тестовый файл, скачайте его и сравните содержимое; подтвердите
в журнале нового proxy, что запрос обслужила новая машина.

На новом VPS проверьте службу, доступный диск и ошибки журнала. У Let's
Encrypt проверьте настроенное продление (`certbot renew --dry-run` после
готовности соответствующего challenge). Origin CA также имеет срок действия.

Старый VPS и резервные копии сохраняйте до завершения выбранного окна
наблюдения. После успешного переезда подтверждённой рабочей копией является
новый VPS; настройте резервное копирование уже для него.

### 9. Откат

**Если на новом VPS ещё не было изменений:** остановите новую службу через
`systemctl disable --now "$RFM_UNIT"`,
проверьте, что старые данные актуальны, запустите старую через
`systemctl enable --now "$RFM_UNIT"`, затем верните A/AAAA и внутренний DNS.
Новая копия остаётся остановленной.

**Если на новом VPS уже появились загрузки, удаления, участники или приглашения:**
простой возврат DNS потеряет эти изменения. Сначала остановите новую копию;
старую держите остановленной. Сохраните snapshot/копии обеих версий данных.
**НА НОВОМ VPS:** выполните обратный dry-run и перенос актуальных данных:

```bash
systemctl disable --now "$RFM_UNIT"
rsync -aHAX --chown=filemgr:filemgr --delete-delay --dry-run --itemize-changes \
  -e "ssh -p $RFM_OLD_SSH_PORT" "$RFM_DATA/" "$RFM_OLD_HOST:$RFM_DATA/"
```

Получатель теперь **старый** VPS: проверьте, что удаления ожидаемы и есть
резервная копия. После проверки:

```bash
rsync -aHAX --chown=filemgr:filemgr --delete-delay \
  -e "ssh -p $RFM_OLD_SSH_PORT" "$RFM_DATA/" "$RFM_OLD_HOST:$RFM_DATA/" &&
rsync -a --chown=root:root --chmod=F600 \
  -e "ssh -p $RFM_OLD_SSH_PORT" "$RFM_ENV" "$RFM_OLD_HOST:$RFM_ENV"
rsync -aHAXc --chown=filemgr:filemgr --delete-delay --dry-run --itemize-changes \
  -e "ssh -p $RFM_OLD_SSH_PORT" "$RFM_DATA/" "$RFM_OLD_HOST:$RFM_DATA/"
```

При успешной сверке **НА СТАРОМ VPS**:

```bash
systemctl enable --now "$RFM_UNIT"
systemctl is-active "$RFM_UNIT"
curl --fail --silent --show-error --max-time 10 \
  -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/login
```

Затем верните прежние A/AAAA, Origin Rules и split DNS. Новый сервис остаётся
остановленным до завершения отката, включая возможную перезагрузку VPS.
Если одновременно с переносом менялся формат данных или версия, сначала
подтвердите совместимость старого бинарника; поэтому обновление отделено
от миграции. Автоматически объединять два расходящихся users.json нельзя.

### Источники и проверки

- [rsync: флаги копирования, права, dry-run и удаления](https://download.samba.org/pub/rsync/rsync.1).
- [Cloudflare: смена origin IP через DNS](https://developers.cloudflare.com/dns/manage-dns-records/how-to/create-dns-records/#update-an-origin-ip-address).
- [Cloudflare: A/AAAA, proxy status и TTL](https://developers.cloudflare.com/dns/manage-dns-records/reference/dns-record-types/).
- [Cloudflare Origin CA: сертификаты и доверие](https://developers.cloudflare.com/ssl/origin-configuration/origin-ca/).

Shell-синтаксис примеров и локальные ссылки проверены. Полный перенос,
переключение DNS, сравнение реальных данных и восстановление по этой
инструкции не выполнялись. Все реальные параметры подставляйте локально; в публичном репозитории
оставляйте только нейтральные примеры.
