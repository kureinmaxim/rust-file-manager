# GUIDE: HTTPS через Cloudflare, когда порт 443 занят

Сценарий: rust-file-manager работает на VPS за nginx, домен обслуживается
Cloudflare (оранжевое облако), но **порт 443 на сервере занят другим
сервисом** (VPN/прокси-транспорт: Caddy, Xray, Hysteria и т.п.), и трогать его
нельзя. Эта инструкция даёт полноценный HTTPS без участия порта 443.

Везде ниже `files.example.com` — плейсхолдер, подставьте свой домен.

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
2. Hostnames: `files.example.com` (или сразу `*.example.com`), RSA 2048.
3. Откроется окно с двумя блоками: **Origin Certificate** и **Private Key**.

> ⚠️ **Private Key показывается только в этом окне, один раз.** Закрыли, не
> сохранив — ключ не восстановить, только Revoke и выпустить новый.

### Как доставить PEM-файлы на сервер, не повредив

Самая частая ошибка всей схемы: вставка многострочного ключа прямо в
терминал/nano рвёт строки, и nginx отвечает
`PEM_read_bio_PrivateKey() failed ... bad end line`.

Надёжный способ — сохранить файлы локально и передать `scp`:

```bash
# на своей машине: вставить блоки в файлы, проверить ДО отправки
nano cert.pem   # блок Origin Certificate
nano key.pem    # блок Private Key
openssl x509 -in cert.pem -noout -subject -enddate
openssl pkey -in key.pem  -noout && echo "KEY OK"

scp cert.pem key.pem root@ВАШ_VPS:/etc/nginx/ssl/files.example.com/
rm key.pem cert.pem        # не оставлять ключ в рабочих папках
```

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
sudo ufw allow 2083/tcp
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
Нажмите **Deploy**. Режим SSL/TLS зоны — **Full (strict)**.

> Origin Rule действует только на этот hostname — остальные записи зоны
> (включая DNS-only, «серые» записи, которые Cloudflare не проксирует)
> не затрагиваются.

## 6. Проверка и типичные ошибки

Откройте `https://files.example.com` — страница входа. Если включали
`COOKIE_SECURE=true` — вход должен работать (браузер ходит по HTTPS).

| Симптом | Причина | Что делать |
|---|---|---|
| 525 SSL handshake failed | CF ещё ходит на :443 / правило не применилось | проверьте Origin Rule (Deploy?), подождите минуту, обновите с Shift |
| 521 Web server is down | nginx не слушает 2083 или ufw закрыл порт | `ss -tlnp \| grep 2083`, `ufw status` |
| 526 Invalid SSL certificate | сертификат не Origin CA / не на этот hostname | пересоздайте Origin cert с нужным hostname |
| `bad end line` при `nginx -t` | повреждён PEM при вставке | §3: scp вместо вставки или пересборка base64 |
| Вход «неверный пароль» при верном пароле | повреждён bcrypt-хеш в env | см. DEPLOY.md §2/§6 — перегенерировать через `sed` |

И финальный штрих: убедитесь, что сервис на :443 (ради которого всё
затевалось) жив, — `systemctl is-active <его-юнит>` и подключение клиентом.
