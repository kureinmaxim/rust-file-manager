# Обновление работающей установки

Проверено для выпуска **v1.3.0** 06.10.2026. Этот порядок также подходит для
переустановки программы с сохранением файлов и пользователей. Первичная
установка описана в [DEPLOY.md](DEPLOY.md).

Команды рассчитаны на Ubuntu/Debian, Bash, systemd и root/sudo. Выполняйте
блоки последовательно в одной SSH-сессии (например, в Tabby); при ошибке
не переходите к установке. Если приложение запущено контейнером или другим
менеджером процессов, сначала определите его фактический способ обновления.

## 0. Проверить работающую службу и реальные пути

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
пути, адрес и имя службы на подтверждённые значения своей установки:

```bash
RFM_UNIT=rust-file-manager.service
RFM_BIN=/usr/local/bin/rust-file-manager
RFM_DATA=/var/lib/rust-file-manager
RFM_ENV=/etc/rust-file-manager/env
RFM_LOCAL_URL=http://127.0.0.1:8080/login
RFM_PUBLIC_URL=https://files.example.com/login
RFM_RELEASE=v1.3.0
RFM_COMMIT=be3cbd70b8177de0ddcaec3760de25452ac76a1b
sudo test -f "$RFM_BIN" && sudo test ! -L "$RFM_BIN" &&
sudo test -d "$RFM_DATA" && sudo test -f "$RFM_ENV" &&
sudo systemctl is-active "$RFM_UNIT"
```

Проверка рассчитана на обычный файл бинарника. Если `ExecStart` использует
симлинк на каталог релизов, сохраните и обновляйте именно эту схему.

Для `files.example.com` по выводу терминала подтверждены Linux x86_64,
Rust/Cargo 1.96.0 и активная systemd-служба `rust-file-manager.service` от
`filemgr`, с рабочим каталогом `/var/lib/rust-file-manager` и юнитом
`/etc/systemd/system/rust-file-manager.service`. Также подтверждён HTTP 200
страницы входа через Cloudflare. На VPS около 2 GiB RAM и 2 GiB swap;
используйте одно задание сборки. Подтверждены бинарник
`/usr/local/bin/rust-file-manager` и env `/etc/rust-file-manager/env`.
Размещение uploads/`users.json`, порт SSH и установленная версия процесса
пока не подтверждены: строки запуска в присланном выводе нет. Версию узнавайте из журнала и футера: CLI-команда
`rust-file-manager --version` не реализована.

## 1. Выбрать выпуск и собрать отдельно

Для 1.3.0 формат `users.json`, расположение многопользовательского хранилища
и env не менялись. Новые шаблоны требуют пересборки бинарника. Минимум Rust
— **1.88**; скачивание теперь возвращает attachment, то есть загруженные
HTML/SVG и другие файлы скачиваются вместо встроенного просмотра.

Исходники: [GitHub](https://github.com/kureinmaxim/rust-file-manager/tree/v1.3.0),
ожидаемый полный коммит — значение `RFM_COMMIT` выше. Используйте тег и
`Cargo.lock`, не произвольный HEAD основной ветки.

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
RFM_BUILD_DIR=$(mktemp -d "$HOME/rfm-build-1.3.0.XXXXXX")
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

## 2. Сохранить прежний бинарник и настройки

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

## 3. Атомарно заменить бинарник и перезапустить

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

## 4. Проверить службу, версию и публичный сайт

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

Ожидаются `active`, строка `version="1.3.0" commit="be3cbd7"` без `-dirty`
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

## 5. Откат бинарника

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

Для перехода с уже многопользовательской версии на 1.3.0 миграции данных
нет: возврат бинарника достаточен. При обновлении с версии до multi-user
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
