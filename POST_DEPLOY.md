# POST_DEPLOY.md — обновление работающей установки

Runbook для ситуации: файловый менеджер **уже задеплоен и работает**
(по [DEPLOY.md](DEPLOY.md)), а в репозитории вышла новая версия — например,
на сервере крутится `1.1.1`, а в репо уже `v1.2.0`. Здесь — как обновиться
аккуратно, с проверками и путём отката.

Все команды выполняются на VPS **под root** (или с `sudo`). Предполагается
раскладка из DEPLOY.md: клон репозитория на сервере, бинарник
`/usr/local/bin/rust-file-manager`, юнит `rust-file-manager.service`.

---

## 0. Узнать, какая версия работает сейчас

```bash
# версия и коммит, с которыми стартовал работающий процесс:
journalctl -u rust-file-manager --no-pager | grep 'starting server' | tail -1
# в строке будет: version="1.1.1" commit="abc1234" built="..."
```

То же самое видно в футере веб-интерфейса после входа:
`rust-file-manager v1.1.1 · коммит abc1234 · сборка ...`.

А какая версия доступна в репозитории:

```bash
cd /root/rust-file-manager        # путь к вашему клону
git fetch --tags
git describe --tags origin/main   # например: v1.2.0
```

Если версии совпадают — обновлять нечего, дальше не идём.

## 1. Прочитать, что изменилось

```bash
git log --oneline HEAD..origin/main      # список новых коммитов
```

Release notes: `https://github.com/kureinmaxim/rust-file-manager/releases`.
Особое внимание — изменениям в `.env.example` (могли появиться новые
переменные) и заметкам о миграции данных.

```bash
git diff HEAD..origin/main -- .env.example
```

## 2. Обновить код и собрать

```bash
cd /root/rust-file-manager
git pull
git describe --tags                       # убедиться: нужный тег, напр. v1.2.0
```

> **VPS с 1–2 ГБ RAM без swap:** release-сборка (LTO) может упасть по
> памяти на линковке. Добавьте временный swap:
>
> ```bash
> fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
> ```

```bash
cargo build --release                     # 5–15 минут на 2 vCPU
```

Сборка идёт **пока старая версия работает** — даунтайма на этом шаге нет.

## 3. Подменить бинарник (даунтайм ~5 секунд)

```bash
# бэкап текущего бинарника — это и есть путь отката
cp /usr/local/bin/rust-file-manager /usr/local/bin/rust-file-manager.prev

cp target/release/rust-file-manager /usr/local/bin/
systemctl restart rust-file-manager
```

Если в новой версии менялся `deploy/rust-file-manager.service` — обновить
и его (редко; видно в `git log` шага 1):

```bash
cp deploy/rust-file-manager.service /etc/systemd/system/
systemctl daemon-reload && systemctl restart rust-file-manager
```

## 4. Проверить

```bash
systemctl status rust-file-manager --no-pager | head -5    # active (running)
journalctl -u rust-file-manager -n 20 --no-pager           # starting server version="1.2.0" ...
curl -sI http://127.0.0.1:8080/login | head -1             # HTTP/1.1 200 OK
```

В логе не должно быть ошибок; строка `starting server` должна показывать
**новую** версию и коммит **без суффикса `-dirty`** (dirty = собрано из
изменённого дерева; после чистого `git pull` так быть не должно).

Затем из браузера: вход, файлы на месте, тестовая загрузка. Если на сервере
рядом живут другие сервисы — убедиться, что они не задеты:

```bash
systemctl --no-pager --type=service --state=running list-units | grep -E 'nginx|caddy|docker'
```

## 5. Прибраться

```bash
# если добавляли временный swap:
swapoff /swapfile && rm /swapfile

# сборочный кэш занимает ~2 ГБ; на тесном диске можно чистить после каждого обновления:
du -sh target/ && cargo clean
```

Бэкап-бинарник `rust-file-manager.prev` оставьте до следующего обновления.

---

## Откат (если новая версия повела себя плохо)

```bash
systemctl stop rust-file-manager
mv /usr/local/bin/rust-file-manager.prev /usr/local/bin/rust-file-manager
systemctl start rust-file-manager
journalctl -u rust-file-manager -n 10 --no-pager   # снова старая версия
```

Данные (`UPLOAD_DIR`, `users.json`) обновление не трогает — откат бинарника
безопасен. Исключение — если в release notes явно написано о миграции
формата данных: тогда сначала прочитать заметки к релизу.

---

## Частные случаи

### Обновление 1.1.1 → 1.2.0

Изменений формата данных и `.env` нет — достаточно шагов 0–5. Если ваша
сборка 1.1.1 была сделана из main **после** слияния multi-user (в футере
есть селектор зон «Мои/Общие файлы») — функционально ничего не изменится,
обновится только версия, метаданные сборки и инструмент `cargo xtask`.
Если же сборка была **до** multi-user (футера с зонами нет) — при первом
старте новая версия автоматически перенесёт папки категорий из корня
`uploads/` в общую зону `shared/`; это видно в логе строками
`migrated legacy category dir into shared zone`.

### «cargo: command not found» под sudo

Rust ставился пользователю в `~/.cargo` — собирайте без sudo, либо
запускайте по полному пути: `/root/.cargo/bin/cargo build --release`.

### Сборка убита на линковке (`signal: 9, SIGKILL`)

Не хватило памяти — добавьте swap (шаг 2) и повторите; сборка продолжится
с места падения.
