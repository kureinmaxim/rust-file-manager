# GUIDE: HTTPS через Cloudflare, когда порт 443 занят

Сценарий: rust-file-manager работает на VPS за nginx, домен обслуживается
Cloudflare (оранжевое облако), но **порт 443 на сервере занят другим
сервисом** (VPN/прокси-транспорт: Caddy, Xray, Hysteria и т.п.), и трогать его
нельзя. Эта инструкция даёт полноценный HTTPS без участия порта 443.

Везде ниже `files.example.com` — плейсхолдер, подставьте свой домен.

**С командой `deploy` (HTTPS_MODE `cloudflare`)** nginx на 2083 настраивается
сам, шаг 4 не нужен. Вручную остаются Cloudflare и файлы сертификата. Порядок:

1. Origin-сертификат и файлы на сервере — §3. Сертификат, положенный до
   `deploy`, команда подхватит, и nginx на 2083 настроится в том же запуске.
   Положили позже — запустите `deploy` ещё раз.
2. Origin Rule, режим SSL/TLS, Cache Rule, Proxied у записи — §5.
3. Если у провайдера есть свой файрвол, откройте в нём TCP 2083. Включённый
   ufw `deploy` откроет сам.
4. `deploy`; при занятом 443 он сам предложит `cloudflare`.

---

## 1. Как устроен путь запроса и где он ломается

С включённым прокси Cloudflare запрос идёт в два прыжка:

```
Браузер ──HTTPS──▶ Cloudflare ──(каким способом?)──▶ ваш VPS ──▶ nginx ──▶ 127.0.0.1:8080
```

Способ второго прыжка задаёт режим **SSL/TLS** зоны в Cloudflare:

| Режим | Cloudflare → сервер | Требование к серверу |
|---|---|---|
| Flexible | обычный HTTP :80 | ничего |
| Full | HTTPS :443, сертификат не проверяется | любой cert на 443 |
| Full (strict) | HTTPS :443, сертификат проверяется | валидный cert на 443 |

Отсюда классическая ошибка **525 SSL handshake failed**: режим Full
(strict), Cloudflare стучится на :443, а там сидит не nginx, а чужой сервис,
который не отвечает TLS-сертификатом для вашего домена.

**Решение:** оставить Full (strict), но научить Cloudflare ходить на другой
порт, где nginx поднимет TLS специально для него.

> Быстрая альтернатива без серверных изменений — Configuration Rule с режимом
> Flexible только для этого hostname (Rules → Configuration Rules → Hostname
> equals `files.example.com` → SSL: Flexible). Минус: участок
> Cloudflare→сервер идёт открытым HTTP, пароль при входе путешествует по
> интернету в открытом виде. Годится как времянка, не как постоянное решение.

## 2. Какие порты понимает Cloudflare

Cloudflare проксирует HTTPS только на фиксированный набор портов:
**443, 2053, 2083, 2087, 2096, 8443**. Выбирайте любой, который на вашем
сервере свободен (проверьте: `ss -tlnp | grep -E ':(2053|2083|2087|2096|8443)\b'`).
В примерах ниже — **2083**.

## 3. Origin CA сертификат (бесплатный, на 15 лет)

Cloudflare выдаёт собственные сертификаты для связки «Cloudflare → ваш
сервер». Браузеры им не доверяют — но это не нужно: браузер видит
сертификат Cloudflare, а Origin-сертификат проверяет только сам Cloudflare.
Плюсы: бесплатно, 15 лет, не нужен certbot и продления.

1. Dashboard → ваша зона → **SSL/TLS → Origin Server → Create Certificate**.
2. Hostnames: Cloudflare подставляет `*.example.com` и `example.com`. Они
   тоже подойдут, но лучше убрать оба и вписать только `files.example.com`:
   ключ лежит на сервере, и тогда он годится только для этого сайта.
   Тип ключа RSA 2048, срок 15 лет.
3. Откроется окно с двумя блоками: **Origin Certificate** и **Private Key**.

> ⚠️ **Private Key показывается только в этом окне, один раз.** Закрыли, не
> сохранив — ключ не восстановить, только Revoke и выпустить новый.

### Как доставить PEM-файлы на сервер, не повредив

Самая частая ошибка всей схемы: вставка многострочного ключа прямо в
терминал/nano рвёт строки, и nginx отвечает
`PEM_read_bio_PrivateKey() failed ... bad end line`. Другой частый случай —
терминал вставляет пустую строку после каждой строки ключа. Тогда
`openssl pkey` пишет `Could not read key ... No supported data to decode`, а в
файле вдвое больше строк, чем нужно.

Надёжный способ — сохранить файлы локально и передать `scp`:

```bash
# на своей машине: вставить блоки в файлы, проверить ДО отправки
nano cert.pem   # блок Origin Certificate
nano key.pem    # блок Private Key
openssl x509 -in cert.pem -noout -subject -enddate
openssl pkey -in key.pem  -noout && echo "KEY OK"

ssh root@ВАШ_VPS 'install -d -m 700 /etc/nginx/ssl/files.example.com'
scp cert.pem key.pem root@ВАШ_VPS:/etc/nginx/ssl/files.example.com/
rm key.pem cert.pem        # не оставлять ключ в рабочих папках
```

Вставка прямо на сервере тоже работает, если затем проверить файл:

```bash
install -d -m 700 /etc/nginx/ssl/files.example.com
cd /etc/nginx/ssl/files.example.com
nano cert.pem   # блок Origin Certificate целиком, со строками BEGIN и END
nano key.pem    # блок Private Key
chmod 600 key.pem
wc -l key.pem                                  # RSA 2048: 28 строк
sed -i '/^[[:space:]]*$/d' key.pem             # убрать пустые строки, если их больше
openssl x509 -in cert.pem -noout -subject -enddate
openssl x509 -in cert.pem -noout -pubkey | sha256sum
openssl pkey -in key.pem -pubout | sha256sum   # хеш должен совпасть с предыдущим
```

Выводятся только имя, срок и хеши открытых ключей — их можно показывать.
Сам ключ никуда не отправляйте и не вставляйте в чаты.

Если всё же вставляли в терминал и получили `bad end line` — иногда файл
можно спасти, пересобрав base64 (склеить и нарезать заново по 64 символа):

```bash
cd /etc/nginx/ssl/files.example.com
B64=$(awk '/BEGIN PRIVATE KEY/{f=1;next} /END PRIVATE KEY/{f=0} f' key.pem | tr -d ' \t\r\n')
{ echo '-----BEGIN PRIVATE KEY-----'; echo "$B64" | fold -w 64; echo '-----END PRIVATE KEY-----'; } > key-fixed.pem
openssl pkey -in key-fixed.pem -noout && echo "KEY OK" && mv key-fixed.pem key.pem
```

## 4. nginx: TLS-листенер на 2083

Порт 443 не трогаем вообще. Добавляем отдельный server-блок:

```bash
sudo mkdir -p /etc/nginx/ssl/files.example.com
# (файлы cert.pem / key.pem уже там после scp)
sudo chmod 600 /etc/nginx/ssl/files.example.com/key.pem

sudo tee /etc/nginx/sites-available/files-ssl >/dev/null <<'EOF'
server {
    listen 2083 ssl;
    server_name files.example.com;
    ssl_certificate     /etc/nginx/ssl/files.example.com/cert.pem;
    ssl_certificate_key /etc/nginx/ssl/files.example.com/key.pem;
    client_max_body_size 200M;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 300;
    }
}
EOF
sudo ln -s /etc/nginx/sites-available/files-ssl /etc/nginx/sites-enabled/

# перед reload убедиться, что ключ и сертификат — пара (хеши совпадают)
openssl x509 -noout -pubkey -in /etc/nginx/ssl/files.example.com/cert.pem | openssl md5
openssl pkey -noout -pubout -in /etc/nginx/ssl/files.example.com/key.pem  | openssl md5

sudo nginx -t && sudo systemctl reload nginx
sudo ufw allow 2083/tcp   # если ufw включён; файрвол провайдера — в его панели
```

Проверка на самом сервере:

```bash
curl -skI https://127.0.0.1:2083/login -H 'Host: files.example.com' | head -1
# ожидаем: HTTP/1.1 200 OK
```

## 5. Origin Rule: направить Cloudflare на 2083

Dashboard → зона → **Rules → Origin Rules → Create rule**:

| Поле формы | Значение |
|---|---|
| Rule name | `files-port-2083` (любое) |
| If incoming requests match | Custom filter expression |
| Field / Operator / Value | **Hostname** / **equals** / `files.example.com` |
| Host Header / SNI / DNS Record | Preserve (не трогать) |
| **Destination Port** | **Rewrite to → 2083** |

Expression Preview должен показать `(http.host eq "files.example.com")`.
Нажмите **Deploy**.

> Origin Rule действует только на этот hostname — остальные записи зоны
> (включая DNS-only, «серые» записи, которые Cloudflare не проксирует)
> не затрагиваются.

### Режим SSL/TLS: Full (strict)

**SSL/TLS → Overview** меняет режим **всей зоны**. Full принимает любой
сертификат сервера, Full (strict) — только Origin CA или выданный центром
сертификации. Если в зоне есть другие Proxied записи, у которых на сервере
самоподписанный сертификат, после переключения они начнут отвечать 526.

- Других Proxied записей нет (DNS → Records, столбец Proxy status) —
  выберите **Full (strict)** и Save.
- Есть или не уверены — режим зоны не трогайте. **Rules → Configuration
  Rules → Create rule**: Hostname equals `files.example.com`, ниже
  **SSL → Full (strict)**, Deploy.

### Cache Rule: не кэшировать API и скачивания

Cloudflare кэширует ответы по расширению файла. Без правила файл, скачанный
по `/d/…` (например `.jpg` или `.pdf`), мог бы отдаваться из кэша Cloudflare
без проверки доступа в файловом менеджере.

**Rules → Cache Rules → Create rule** (на форме — **Edit expression**):

| Поле формы | Значение |
|---|---|
| Rule name | `files-bypass-api` (любое) |
| Expression | `(http.host eq "files.example.com" and (starts_with(http.request.uri.path, "/api/") or starts_with(http.request.uri.path, "/d/")))` |
| Cache eligibility | **Bypass cache** (по умолчанию стоит Eligible for cache) |

Нажмите **Deploy**.

### Запись DNS: Proxied

**DNS → Records**: у `files.example.com` включите **Proxied** (оранжевое
облако). С DNS only Cloudflare в пути нет: браузер идёт прямо на 443 сервера,
к занявшему его сервису, и ни Origin Rule, ни сертификат не работают.
`getent ahosts files.example.com` на сервере после этого показывает адреса
Cloudflare — это нормально.

## 6. Проверка и типичные ошибки

Откройте `https://files.example.com` — страница входа. Если включали
`COOKIE_SECURE=true` — вход должен работать (браузер ходит по HTTPS).

| Симптом | Причина | Что делать |
|---|---|---|
| 525 SSL handshake failed | CF ещё ходит на :443 / правило не применилось | проверьте Origin Rule (Deploy?), подождите минуту, обновите с Shift |
| 521 Web server is down | nginx не слушает 2083, порт закрыт ufw или файрволом провайдера | `ss -tlnp \| grep 2083`, `ufw status`, панель провайдера |
| 526 Invalid SSL certificate | сертификат не Origin CA / не на этот hostname | пересоздайте Origin cert с нужным hostname |
| `bad end line` при `nginx -t` | повреждён PEM при вставке | §3: scp вместо вставки или пересборка base64 |
| `openssl pkey`: `Could not read key ... No supported data to decode` | после каждой строки ключа вставилась пустая (`wc -l key.pem` около 56 вместо 28) | §3: `sed -i '/^[[:space:]]*$/d' key.pem`, затем сверьте хеши |
| Сайт не открывается или открывается чужой сервис на 443 | у записи DNS only, Cloudflare не в пути | §5: включите Proxied |
| Вход «неверный пароль» при верном пароле | повреждён bcrypt-хеш в env | см. DEPLOY.md §2/§6 — перегенерировать через `sed` |

И финальный штрих: убедитесь, что сервис на :443 (ради которого всё
затевалось) жив, — `systemctl is-active <его-юнит>` и подключение клиентом.
