# Перенос файлового сервера на новый IP и VPS

Инструкция для `rust-file-manager`, подготовлена **06.10.2026**. Перенос
сохраняет домен `files.example.com`, файлы, пользователей, приглашения и настройки.
Первичная установка — [DEPLOY.md](DEPLOY.md), обновление версии на том же
сервере — [POST_DEPLOY.md](POST_DEPLOY.md).

Команды рассчитаны на Ubuntu/Debian, systemd, **Bash и root на обоих VPS**.
Работайте в двух подписанных вкладках Tabby: **СТАРЫЙ VPS** и **НОВЫЙ VPS**.
Новый IP и SSH-порты в примерах нужно заменить своими; реальный перенос по
этой инструкции пока не выполнялся.

## Если меняется только IP того же VPS

Перенос данных и переустановка программы не нужны. Сохраните прежние
значения DNS, настройте новый адрес по инструкции провайдера, проверьте
SSH через новый IP, firewall и доступность proxy. Если nginx/SSH привязаны
к конкретному старому IP, исправьте соответствующие listen-настройки.
`BIND_ADDR=127.0.0.1:8080` менять не нужно.

После проверки HTTPS обновите A/AAAA нужного hostname в Cloudflare по
разделу 7. Доменный сертификат не требует замены только из-за нового IP.
Записи tailnet/split DNS меняются лишь при изменении соответствующего
внутреннего адреса. Для полноценного переезда на другую машину — шаги ниже.

## 0. Зафиксировать исходное состояние

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

## 1. Подготовить новый VPS без переключения DNS

Переносите текущую версию программы; обновление выпуска выполняйте отдельным
шагом после переезда. Для прямого копирования бинарника нужны Linux x86_64
и совместимые системные библиотеки. При другой архитектуре/ABI соберите
**ту же версию** из исходников по DEPLOY.md; не заменяйте её автоматически
произвольным HEAD основной ветки. Для сборки 1.3.0 требуется Rust 1.88+.

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

## 2. Сохранить резервную копию и предварительно скопировать данные

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

## 3. Подготовить HTTPS и маршруты нового VPS

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

## 4. Проверить новый сервер до изменения DNS

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

## 5. Остановить запись и выполнить финальную синхронизацию

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

## 6. Запустить новую копию и оставить старую остановленной

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

## 7. Переключить DNS и внутренние адреса

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

## 8. Проверить после переключения

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

## 9. Откат

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

## Источники и проверки

- [rsync: флаги копирования, права, dry-run и удаления](https://download.samba.org/pub/rsync/rsync.1).
- [Cloudflare: смена origin IP через DNS](https://developers.cloudflare.com/dns/manage-dns-records/how-to/create-dns-records/#update-an-origin-ip-address).
- [Cloudflare: A/AAAA, proxy status и TTL](https://developers.cloudflare.com/dns/manage-dns-records/reference/dns-record-types/).
- [Cloudflare Origin CA: сертификаты и доверие](https://developers.cloudflare.com/ssl/origin-configuration/origin-ca/).

Shell-синтаксис примеров и локальные ссылки проверены. Полный перенос,
переключение DNS, сравнение реальных данных и восстановление по этой
инструкции не выполнялись. Все реальные параметры подставляйте локально; в публичном репозитории
оставляйте только нейтральные примеры.
