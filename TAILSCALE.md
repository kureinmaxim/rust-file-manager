# Доступ через Tailscale / Headscale

Как открывать файловый менеджер по своей tailnet-сети, минуя публичный
интернет и Cloudflare. Предполагается, что сервер уже развёрнут по
[DEPLOY.md](DEPLOY.md) и состоит в tailnet (в примерах — self-hosted
контрол-сервер Headscale).

Менять что-либо в приложении или nginx не нужно: nginx слушает все
интерфейсы сервера, включая Tailscale-интерфейс. Задача сводится к двум
вещам — направить клиентов tailnet на Tailscale-IP сервера и сохранить
работающий HTTPS.

> ⚠️ Почему нельзя просто открыть `http://100.x.y.z`: в конфиге приложения
> стоит `COOKIE_SECURE=true`, поэтому по незащищённому HTTP браузер не
> сохранит cookie сессии и вход работать не будет. HTTPS нужен в любом
> случае — а значит, нужен домен, на который выписан сертификат.

## Рекомендуемый способ: split DNS в Headscale

Внутри tailnet домен `files.example.com` резолвится в Tailscale-IP сервера,
снаружи — как обычно, в публичный IP через Cloudflare. Адрес в браузере и
сертификат Let's Encrypt одни и те же (имя совпадает), но трафик участников
tailnet идёт напрямую по WireGuard.

Бонус: лимит Cloudflare в 100 МБ на загрузку внутри tailnet не действует —
трафик идёт мимо прокси.

### 1. Узнать Tailscale-IP сервера

```bash
tailscale ip -4    # например 100.64.0.5
```

### 2. Добавить override в конфиг Headscale

В `/etc/headscale/config.yaml` (на машине, где работает Headscale):

```yaml
dns:
  magic_dns: true
  extra_records:
    - name: "files.example.com"
      type: "A"
      value: "100.64.0.5"   # Tailscale-IP сервера из шага 1
```

### 3. Перезапустить Headscale

```bash
sudo systemctl restart headscale
```

### 4. Проверить с клиента tailnet

```bash
nslookup files.example.com   # должен вернуть 100.x.y.z, а не публичный IP
curl -I https://files.example.com
```

Клиенты должны использовать DNS из tailnet (на клиенте:
`tailscale set --accept-dns=true`). После этого `https://files.example.com`
открывается напрямую по WireGuard, сертификат валиден, вход работает.

Кто не состоит в tailnet, продолжает ходить обычным путём — через
Cloudflare на публичный IP.

## Альтернативы

- **`tailscale serve`** — у официального Tailscale это самый простой способ
  (`tailscale serve --bg https / http://127.0.0.1:8080`): HTTPS на
  MagicDNS-имени поднимается автоматически. **С Headscale не работает**:
  сертификаты для `*.ts.net` выпускаются через инфраструктуру Tailscale,
  недоступную self-hosted контрол-серверу.

- **Отдельный поддомен для tailnet** — создать в Cloudflare запись
  `files-ts.example.com → 100.x.y.z` (приватный IP в публичном DNS — это
  допустимо) и выписать сертификат через DNS-01 challenge, потому что
  HTTP-01 до приватного адреса не достучится:

  ```bash
  sudo apt install -y python3-certbot-dns-cloudflare
  # ~/.secrets/cloudflare.ini: dns_cloudflare_api_token = <токен с правом DNS:Edit>
  sudo certbot certonly --dns-cloudflare \
      --dns-cloudflare-credentials ~/.secrets/cloudflare.ini \
      -d files-ts.example.com
  ```

  Плюс второй `server`-блок в nginx с этим именем и сертификатом. Работает,
  но движущихся частей больше, чем у split DNS.

## Проверочный список

- `tailscale status` на сервере — узел онлайн, IP совпадает с записью DNS.
- ufw не должен резать 443-й порт на интерфейсе `tailscale0` — правило
  `Nginx Full` из [DEPLOY.md](DEPLOY.md) разрешает доступ отовсюду, так что
  по умолчанию всё в порядке.
- Tailscale — только транспорт: вход по логину/паролю и изоляция личных зон
  работают одинаково, каким бы путём пользователь ни пришёл.
