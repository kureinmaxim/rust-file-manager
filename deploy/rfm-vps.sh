#!/usr/bin/env bash
# rfm-vps — бот TelegramOnly и файловый менеджер rust-file-manager на одном VPS.
#
# Команды (ставятся в /usr/local/sbin командой setup):
#   deploy           установить то, чего на сервере ещё нет, и связать бота с файловым менеджером
#   post_deploy      обновить то, что уже установлено; сам определяет, что стоит на сервере
#   rfm-vps status   показать, что установлено (только чтение)
#
# Первый запуск на сервере, под root:
#   curl -fsSL https://raw.githubusercontent.com/kureinmaxim/rust-file-manager/main/deploy/rfm-vps.sh | bash -s -- setup
#
# Те же шаги вручную и пояснения: DEPLOYwTELEGRAM.md, POST_DEPLOYwTELEGRAM.md,
# схема сервера: ARCHITECTUREwTELEGRAM.md.
#
# Скрипт не выводит секреты. Пароль администратора вводится скрыто, ключи
# создаются на сервере и пишутся в файлы с правами 600; на экране — только
# «да/нет» и длины.

set -uo pipefail

# --- Пути и имена. Переопределяются переменными окружения (для тестов) -------
: "${RFM_REPO_URL:=https://github.com/kureinmaxim/rust-file-manager.git}"
: "${BOT_REPO_URL:=https://github.com/kureinmaxim/TelegramOnly.git}"
: "${RFM_VPS_RAW:=https://raw.githubusercontent.com/kureinmaxim/rust-file-manager}"
: "${RFM_VPS_REF:=main}"
: "${RFM_BIN:=/usr/local/bin/rust-file-manager}"
: "${RFM_UNIT:=rust-file-manager.service}"
: "${UNIT_DIR:=/etc/systemd/system}"
: "${RFM_ENV:=/etc/rust-file-manager/env}"
: "${RFM_DATA:=/var/lib/rust-file-manager}"
: "${RFM_BACKUPS:=/var/backups/rust-file-manager}"
: "${BOT_DIR:=/opt/TelegramOnly}"
: "${BOT_BACKUPS:=/var/backups/telegramonly}"
: "${BOT_CONTAINER:=telegram-helper-lite}"
: "${BOT_SERVICE:=telegram-helper}"
: "${BOT_UNIT:=telegramonly.service}"
: "${BRIDGE_UNIT:=rfm-internal-bridge.service}"
: "${BUILD_ROOT:=/root}"
: "${STATE_FILE:=/etc/rfm-vps.conf}"
: "${CMD_DIR:=/usr/local/sbin}"
: "${NGINX_DIR:=/etc/nginx}"
: "${LE_DIR:=/etc/letsencrypt}"
: "${SYSCTL_FILE:=/etc/sysctl.d/99-tcp-mtu-probing.conf}"
: "${SWAP_FILE:=/swapfile}"
: "${LOCK_FILE:=/run/rfm-vps.lock}"
: "${CARGO_ENV:=$HOME/.cargo/env}"
: "${FM_USER:=filemgr}"
: "${FSTAB:=/etc/fstab}"
: "${POLL_SECONDS:=15}"      # как часто показывать ход сборки
: "${WAIT_SECONDS:=5}"       # пауза при ожидании запуска бота

RUST_MIN_MINOR=88          # Rust 1.88+
KEEP_BACKUPS=3             # сколько последних копий программы и .env оставлять
BUILD_LOG="$BUILD_ROOT/rfm-build.log"

# Ответы на вопросы можно задать заранее переменными окружения:
# RFM_DOMAIN, HTTPS_MODE (certbot|cloudflare|none), LE_EMAIL, RFM_ADMIN_LOGIN,
# BOT_NETWORK (bridge|host), ENABLE_UFW (yes|no), INSTALL_BOT, INSTALL_FM (yes|no).
: "${RFM_DOMAIN:=}" "${HTTPS_MODE:=}" "${LE_EMAIL:=}" "${RFM_ADMIN_LOGIN:=}"
: "${BOT_NETWORK:=}" "${ENABLE_UFW:=}" "${INSTALL_BOT:=}" "${INSTALL_FM:=}"

CMD=''
ASSUME_YES=0
FORCE=0
CHECK_ONLY=0
BACKUP_DATA=0
ONLY=''
RFM_REF=''
ADMIN_PASSWORD=''
ORIG_ARGS=()
INSTALLED_NOW=0
SUMMARY=()
TODO=()

# --- Вывод -------------------------------------------------------------------
if [[ -t 1 ]]; then
  C_B=$'\e[1m' C_G=$'\e[32m' C_Y=$'\e[33m' C_R=$'\e[31m' C_0=$'\e[0m'
else
  C_B='' C_G='' C_Y='' C_R='' C_0=''
fi
step() { printf '\n%s==> %s%s\n' "$C_B" "$*" "$C_0"; }
say()  { printf '    %s\n' "$*"; }
ok()   { printf '    %s✓%s %s\n' "$C_G" "$C_0" "$*"; }
warn() { printf '    %s!%s %s\n' "$C_Y" "$C_0" "$*" >&2; }
fail() { printf '    %s✗%s %s\n' "$C_R" "$C_0" "$*" >&2; }
die()  { printf '\n%s✗ %s%s\n' "$C_R" "$*" "$C_0" >&2; exit 1; }

# --- Вопросы -----------------------------------------------------------------
# ask ПЕРЕМЕННАЯ "Вопрос" [по умолчанию] — уже заданную переменную не спрашивает.
ask() {
  local __var=$1 __q=$2 __def=${3:-} __ans=''
  [[ -n ${!__var:-} ]] && return 0
  if (( ASSUME_YES )); then printf -v "$__var" '%s' "$__def"; return 0; fi
  if [[ -n $__def ]]; then printf '    %s [%s]: ' "$__q" "$__def" >&2; else printf '    %s: ' "$__q" >&2; fi
  IFS= read -r __ans || true
  [[ -z $__ans ]] && __ans=$__def
  printf -v "$__var" '%s' "$__ans"
}

# ask_choice ПЕРЕМЕННАЯ "Вопрос" по_умолчанию "вариант1 вариант2 …"
ask_choice() {
  local __var=$1 __q=$2 __def=$3 __opts=" $4 " __v
  while :; do
    ask "$__var" "$__q ($4)" "$__def"
    __v=${!__var}
    [[ $__opts == *" $__v "* ]] && return 0
    warn "Нужно одно из: $4"
    printf -v "$__var" '%s' ''
    (( ASSUME_YES )) && die "Недопустимое значение для: $__q"
  done
}

# confirm "Вопрос" [y|n] — 0 = да
confirm() {
  local q=$1 def=${2:-y} ans=''
  if (( ASSUME_YES )); then [[ $def == y ]]; return; fi
  if [[ $def == y ]]; then printf '    %s [Д/н]: ' "$q" >&2; else printf '    %s [д/Н]: ' "$q" >&2; fi
  IFS= read -r ans || true
  ans=${ans,,}
  [[ -z $ans ]] && ans=$def
  [[ $ans == y* || $ans == д* ]]
}

ask_password() {
  local p1 p2
  while :; do
    printf '    Придумайте пароль администратора сайта (от 8 символов): ' >&2
    IFS= read -r -s p1 || true; printf '\n' >&2
    printf '    Повторите пароль: ' >&2
    IFS= read -r -s p2 || true; printf '\n' >&2
    if [[ ${#p1} -ge 8 && $p1 == "$p2" ]]; then ADMIN_PASSWORD=$p1; return 0; fi
    warn "Пароли короче 8 символов или не совпали — ещё раз"
    (( ASSUME_YES )) && [[ ! -t 0 ]] && die "Пароль не задан"
  done
}

# --- Мелочи ------------------------------------------------------------------
has() { command -v "$1" >/dev/null 2>&1; }
strip_ansi() { sed 's/\x1b\[[0-9;]*m//g'; }
ts() { date -u +%Y%m%dT%H%M%SZ; }

# Значение KEY из файла вида KEY=value (последнее), без кавычек вокруг.
env_get() {
  local v
  [[ -r $1 ]] || return 0
  v=$(sed -n "s/^$2=//p" "$1" | tail -1)
  v=${v%\'}; v=${v#\'}; v=${v%\"}; v=${v#\"}
  printf '%s' "$v"
}
env_has() { [[ -r $1 ]] && grep -q "^$2=" "$1"; }

state_get() { env_get "$STATE_FILE" "$1"; }
state_set() {
  local key=$1 val=$2
  install -d -m 0755 "$(dirname "$STATE_FILE")"
  [[ -f $STATE_FILE ]] || printf '# rfm-vps: настройки этого сервера (без секретов)\n' >"$STATE_FILE"
  sed -i "/^$key=/d" "$STATE_FILE"
  printf '%s=%s\n' "$key" "$val" >>"$STATE_FILE"
}

# Копия файла с секретами в каталог резервных копий (700); последние KEEP_BACKUPS.
backup_secret_file() {  # файл каталог префикс
  local src=$1 dir=$2 prefix=$3
  install -d -m 0700 "$dir"
  cp -p "$src" "$dir/$prefix-$(ts)" && prune_backups "$dir" "$prefix-*"
}
prune_backups() {  # каталог шаблон
  local dir=$1 pattern=$2 f n=0
  while IFS= read -r f; do
    n=$((n + 1))
    (( n > KEEP_BACKUPS )) && rm -rf -- "$f"
  done < <(find "$dir" -maxdepth 1 -name "$pattern" -printf '%T@ %p\n' 2>/dev/null | sort -rn | cut -d' ' -f2-)
}

http_code() { curl -sS -o /dev/null -w '%{http_code}' --max-time "${2:-10}" "$1" 2>/dev/null || true; }

# --- Что стоит на сервере ----------------------------------------------------
unit_exists() { systemctl cat "$1" >/dev/null 2>&1; }
fm_present() { [[ -x $RFM_BIN ]] || unit_exists "$RFM_UNIT"; }

bot_flavor() {  # docker | systemd | cloned | none
  if has docker && docker inspect "$BOT_CONTAINER" >/dev/null 2>&1; then echo docker
  elif unit_exists "$BOT_UNIT"; then echo systemd
  elif [[ -d $BOT_DIR/.git ]]; then echo cloned
  else echo none; fi
}
bot_present() { [[ $(bot_flavor) == docker || $(bot_flavor) == systemd ]]; }

fm_start_line() {  # последняя строка запуска (с начала журнала или с @epoch)
  local from=()
  [[ -n ${1:-} ]] && from=(--since "@$1")
  journalctl -u "$RFM_UNIT" --no-pager -o cat "${from[@]}" 2>/dev/null | strip_ansi | grep 'starting server' | tail -1
}
line_field() { grep -o "$1=\"[^\"]*\"" | head -1 | cut -d'"' -f2; }
fm_version() { fm_start_line | line_field version; }
fm_commit()  { fm_start_line | line_field commit; }

bot_net() { docker inspect -f '{{.HostConfig.NetworkMode}}' "$BOT_CONTAINER" 2>/dev/null; }
bot_container_version() {
  docker exec "$BOT_CONTAINER" grep -m1 -E '^version[[:space:]]*=' /app/pyproject.toml 2>/dev/null | cut -d'"' -f2
}
bot_repo_version() { grep -m1 -E '^version[[:space:]]*=' "$BOT_DIR/pyproject.toml" 2>/dev/null | cut -d'"' -f2; }
bot_running_version() {
  case $(bot_flavor) in
    docker) bot_container_version ;;
    systemd) bot_repo_version ;;
  esac
}
bot_linked() { env_has "$BOT_DIR/.env" FILES_SERVICE_TOKEN && env_has "$BOT_DIR/.env" FILES_INTERNAL_URL; }

fm_build_dir() {
  local d
  d=$(state_get RFM_BUILD_DIR)
  if [[ -n $d && -d $d/.git ]]; then echo "$d"; return; fi
  for d in "$BUILD_ROOT"/rfm-build "$BUILD_ROOT"/rfm-build-*; do
    [[ -d $d/.git ]] || continue
    git -C "$d" remote get-url origin 2>/dev/null | grep -qi 'rust-file-manager' && { echo "$d"; return; }
  done
  echo "$BUILD_ROOT/rfm-build"
}

# ID и @username бота по токену из его .env. Токен уходит в curl через stdin,
# а не в аргументы командной строки, и никуда не печатается.
bot_getme() {
  local token json id user
  token=$(env_get "$BOT_DIR/.env" BOT_TOKEN)
  [[ $token =~ ^[0-9]+:[A-Za-z0-9_-]+$ ]] || return 1
  json=$(printf 'url = "https://api.telegram.org/bot%s/getMe"\n' "$token" | curl -sS --max-time 20 -K - 2>/dev/null) || return 1
  id=$(grep -o '"id":[0-9]*' <<<"$json" | head -1 | cut -d: -f2)
  user=$(grep -o '"username":"[^"]*"' <<<"$json" | head -1 | cut -d'"' -f4)
  [[ -n $id && -n $user ]] || return 1
  printf '%s %s\n' "$id" "$user"
}

print_status() {
  local flavor ver net
  step "Что стоит на сервере"
  if fm_present; then
    say "Файловый менеджер: установлен, версия ${C_B}$(fm_version || true)${C_0} (коммит $(fm_commit || true)), служба $(systemctl is-active "$RFM_UNIT" 2>/dev/null || true)"
    say "  сайт: $(env_get "$RFM_ENV" PUBLIC_BASE_URL)  мини-приложение: $(env_get "$RFM_ENV" MINIAPP_ENABLED)  API для бота: $(env_get "$RFM_ENV" INTERNAL_BIND_ADDR)"
    say "  каталог сборки: $(fm_build_dir)"
  else
    say "Файловый менеджер: не установлен"
  fi
  flavor=$(bot_flavor)
  case $flavor in
    docker)
      ver=$(bot_container_version); net=$(bot_net)
      say "Бот TelegramOnly: Docker, версия ${C_B}${ver:-?}${C_0}, сеть $net, связан с файловым менеджером: $(bot_linked && echo да || echo нет)" ;;
    systemd)
      say "Бот TelegramOnly: systemd ($BOT_UNIT), версия ${C_B}$(bot_repo_version)${C_0}, связан с файловым менеджером: $(bot_linked && echo да || echo нет)" ;;
    cloned) say "Бот TelegramOnly: код в $BOT_DIR есть, но бот не установлен" ;;
    none)   say "Бот TelegramOnly: не установлен" ;;
  esac
}

# --- Общая подготовка (только deploy) ----------------------------------------
preflight() {
  [[ $EUID -eq 0 ]] || die "Запустите от root: sudo -i, затем команду ещё раз"
  [[ $(uname -s) == Linux ]] || die "Нужен Linux-сервер"
  has apt-get || die "Поддерживаются Debian и Ubuntu (нужен apt-get)"
  has systemctl || die "Нужен systemd"
}

take_lock() {
  exec 9>"$LOCK_FILE" || die "Не удалось создать $LOCK_FILE"
  flock -n 9 || die "Команда уже выполняется в другом окне"
}

install_packages() {
  local pkgs=(git curl ca-certificates openssl build-essential pkg-config socat nginx ufw)
  [[ $HTTPS_MODE == certbot ]] && pkgs+=(certbot python3-certbot-nginx)
  step "Системные пакеты"
  DEBIAN_FRONTEND=noninteractive apt-get update -q >/dev/null || die "apt-get update не прошёл"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${pkgs[@]}" >/dev/null || die "Не удалось поставить пакеты: ${pkgs[*]}"
  ok "${pkgs[*]}"
}

setup_sysctl() {
  # Обход PMTU-дыры у части провайдеров (TelegramOnly SERVER_OPS.md): без него
  # опрос Telegram зависает. В сети хоста бот берёт эти значения только с хоста.
  if [[ ! -f $SYSCTL_FILE ]]; then
    printf 'net.ipv4.tcp_mtu_probing = 2\nnet.ipv4.tcp_base_mss = 1024\n' >"$SYSCTL_FILE"
  fi
  sysctl -q -p "$SYSCTL_FILE" >/dev/null 2>&1 && ok "MTU probing включён ($SYSCTL_FILE)" || warn "sysctl не применился"
}

setup_swap() {
  local mem_mb
  [[ -n $(swapon --show 2>/dev/null) ]] && { ok "swap уже есть"; return; }
  mem_mb=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
  (( mem_mb >= 3500 )) && return
  say "Памяти ${mem_mb} МБ — создаю swap 2 ГБ, без него сборка Rust может упасть"
  { fallocate -l 2G "$SWAP_FILE" 2>/dev/null || dd if=/dev/zero of="$SWAP_FILE" bs=1M count=2048 status=none; } &&
    chmod 600 "$SWAP_FILE" && mkswap -q "$SWAP_FILE" >/dev/null && swapon "$SWAP_FILE" &&
    { grep -q "^$SWAP_FILE " "$FSTAB" || echo "$SWAP_FILE none swap sw 0 0" >>"$FSTAB"; } &&
    ok "swap 2 ГБ" || warn "swap создать не удалось"
}

setup_firewall() {
  local port web=0 https_port=443
  has ufw || return 0
  [[ $HTTPS_MODE == cloudflare ]] && https_port=2083
  { [[ $INSTALL_FM == yes ]] || fm_present; } && [[ $HTTPS_MODE != none ]] && web=1
  if ufw status 2>/dev/null | grep -q 'Status: active'; then
    if (( web )); then
      ufw allow 80/tcp >/dev/null; ufw allow "$https_port/tcp" >/dev/null
      ok "ufw уже включён — добавлены порты 80 и $https_port"
    fi
    return
  fi
  [[ $ENABLE_UFW == yes ]] || { warn "ufw выключен — файрвол не трогаю"; return; }
  port=$(awk '{print $4}' <<<"${SSH_CONNECTION:-}")
  if [[ ! $port =~ ^[0-9]+$ ]]; then
    warn "Не вижу порт SSH текущего подключения — ufw не включаю, чтобы не потерять доступ"
    return
  fi
  ufw allow "$port/tcp" >/dev/null || { warn "ufw: не удалось открыть SSH — файрвол не включаю"; return; }
  if (( web )); then ufw allow 80/tcp >/dev/null; ufw allow "$https_port/tcp" >/dev/null; fi
  ufw --force enable >/dev/null && ok "ufw включён: SSH $port$( (( web )) && echo ", 80, $https_port")" ||
    warn "ufw включить не удалось"
}

ensure_docker() {
  has docker && docker compose version >/dev/null 2>&1 && return 0
  step "Docker"
  # Официальный установщик Docker (так же ставит TelegramOnly).
  curl -fsSL https://get.docker.com | sh >/dev/null || die "Docker не установился"
  docker compose version >/dev/null 2>&1 || die "Нет docker compose v2"
  ok "$(docker --version)"
}

ensure_rust() {
  local minor
  # shellcheck source=/dev/null
  [[ -f $CARGO_ENV ]] && . "$CARGO_ENV"
  if ! has cargo; then
    step "Rust"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null 2>&1 ||
      die "rustup не установился"
    # shellcheck source=/dev/null
    . "$CARGO_ENV"
  fi
  minor=$(rustc --version | awk '{print $2}' | cut -d. -f2)
  if [[ $minor =~ ^[0-9]+$ ]] && (( minor < RUST_MIN_MINOR )); then
    say "Rust старше 1.$RUST_MIN_MINOR — обновляю"
    rustup update stable >/dev/null 2>&1 || die "rustup update stable не прошёл"
  fi
  has cargo || die "cargo не найден"
}

# --- Файловый менеджер -------------------------------------------------------
fm_checkout() {  # каталог ref
  local dir=$1 ref=$2
  if [[ ! -d $dir/.git ]]; then
    git clone -q --depth 1 "$RFM_REPO_URL" "$dir" || { fail "git clone $RFM_REPO_URL не прошёл"; return 1; }
  fi
  if [[ -n $(git -C "$dir" status --porcelain --untracked-files=no 2>/dev/null) ]]; then
    fail "В $dir изменены файлы — не трогаю. Сохраните правки или удалите каталог"
    return 1
  fi
  git -C "$dir" fetch -q --depth 1 origin "$ref" && git -C "$dir" checkout -q --detach FETCH_HEAD ||
    { fail "Не удалось получить $ref"; return 1; }
}

# Сборка в отдельной сессии: обрыв SSH и Ctrl+C её не прерывают. Если сборка уже
# идёт (например, после обрыва), ждём её, а не запускаем вторую.
fm_build() {
  local dir=$1 pid start now last rc=0
  ensure_rust
  start=$(date +%s)
  pid=$(pgrep -f 'cargo build --release --locked --bin rust-file-manager' | head -1 || true)
  if [[ -n $pid ]]; then
    say "Сборка уже идёт (процесс $pid) — жду её"
  else
    : >"$BUILD_LOG"
    ( cd "$dir" && exec setsid nohup nice -n 10 env CARGO_BUILD_JOBS=1 \
        cargo build --release --locked --bin rust-file-manager >"$BUILD_LOG" 2>&1 </dev/null ) &
    pid=$!
    say "Сборка запущена, журнал: $BUILD_LOG"
  fi
  say "Обрыв SSH сборку не прервёт: потом запустите команду ещё раз, она дождётся результата"
  while kill -0 "$pid" 2>/dev/null; do
    sleep "$POLL_SECONDS"
    now=$(date +%s)
    last=$(grep -E '^\s*(Compiling|Finished|error)' "$BUILD_LOG" 2>/dev/null | tail -1 | sed 's/^ *//')
    printf '    … %d мин %02d с  %s\n' $(((now - start) / 60)) $(((now - start) % 60)) "${last:0:70}"
  done
  wait "$pid" 2>/dev/null || rc=$?
  if grep -q 'Finished' "$BUILD_LOG" && [[ -x $dir/target/release/rust-file-manager ]] && (( rc == 0 || rc == 127 )); then
    ok "$(grep 'Finished' "$BUILD_LOG" | tail -1 | sed 's/^ *//')"
    return 0
  fi
  fail "Сборка не удалась. Последние строки журнала:"
  tail -30 "$BUILD_LOG" >&2
  return 1
}

fm_url_local() { local b; b=$(env_get "$RFM_ENV" BIND_ADDR); echo "http://${b:-127.0.0.1:8080}"; }

# Проверка после запуска: служба жива, запущен ожидаемый коммит, отвечают сайт,
# мини-приложение и внутренний API. Публичный адрес — только предупреждение.
fm_health() {  # epoch_запуска [ожидаемый_коммит]
  local since=$1 expect=${2:-} line commit base code public
  for _ in $(seq 1 20); do
    line=$(fm_start_line "$since")
    [[ -n $line ]] && break
    sleep 1
  done
  sleep 2
  systemctl is-active --quiet "$RFM_UNIT" || { fail "служба $RFM_UNIT не работает"; return 1; }
  [[ -n $line ]] || { fail "в журнале нет строки запуска"; return 1; }
  commit=$(line_field commit <<<"$line"); commit=${commit%-dirty}
  if [[ -n $expect && $expect != "$commit"* ]]; then
    fail "запущен коммит $commit, а ожидался ${expect:0:7}"; return 1
  fi
  base=$(fm_url_local)
  code=$(http_code "$base/login"); [[ $code == 200 ]] || { fail "$base/login ответил $code"; return 1; }
  if [[ $(env_get "$RFM_ENV" MINIAPP_ENABLED) == true ]]; then
    code=$(http_code "$base/tg/"); [[ $code == 200 ]] || { fail "$base/tg/ ответил $code"; return 1; }
  fi
  if [[ -n $(env_get "$RFM_ENV" INTERNAL_BIND_ADDR) ]]; then
    code=$(sed -n 's/^INTERNAL_API_TOKEN=/Authorization: Bearer /p' "$RFM_ENV" |
      curl -sS -o /dev/null -w '%{http_code}' --max-time 10 -H @- \
        "http://$(env_get "$RFM_ENV" INTERNAL_BIND_ADDR)/internal/v1/health" 2>/dev/null || true)
    [[ $code == 200 ]] || { fail "внутренний API ответил $code"; return 1; }
  fi
  ok "запущен $(line_field version <<<"$line") ($commit): сайт, мини-приложение и API для бота отвечают"
  public=$(env_get "$RFM_ENV" PUBLIC_BASE_URL)
  if [[ -n $public ]]; then
    code=$(http_code "$public/login" 20)
    if [[ $code == 200 ]]; then ok "снаружи: $public отвечает"; else warn "снаружи $public/login ответил ${code:-ничего} — проверьте DNS, nginx и HTTPS"; fi
  fi
}

fm_create_user_dirs() {
  id "$FM_USER" >/dev/null 2>&1 ||
    useradd --system --user-group --home-dir "$RFM_DATA" --shell /usr/sbin/nologin "$FM_USER" ||
    die "Не удалось создать пользователя $FM_USER"
  install -d -o "$FM_USER" -g "$FM_USER" -m 0750 "$RFM_DATA" "$RFM_DATA/uploads"
  install -d -o root -g root -m 0755 "$(dirname "$RFM_ENV")"
}

fm_write_env() {  # каталог_сборки
  local bin=$1/target/release/rust-file-manager hash tmp
  [[ -e $RFM_ENV ]] && { ok "настройки $RFM_ENV уже есть — не меняю"; return 0; }
  [[ -n $ADMIN_PASSWORD ]] || ask_password
  hash=$(printf '%s' "$ADMIN_PASSWORD" | "$bin" hash-password 2>/dev/null)
  ADMIN_PASSWORD=''
  [[ $hash == '$2'* ]] || die "Не удалось получить хеш пароля"
  tmp=$(mktemp "$(dirname "$RFM_ENV")/.env.XXXXXX")
  chmod 600 "$tmp"
  {
    echo '# rust-file-manager — создано командой deploy'
    echo 'BIND_ADDR=127.0.0.1:8080'
    echo "UPLOAD_DIR=$RFM_DATA/uploads"
    echo "USERS_FILE=$RFM_DATA/users.json"
    echo '# Логин администратора сайта'
    echo "ADMIN_USERNAME=${RFM_ADMIN_LOGIN:-admin}"
    echo "ADMIN_PASSWORD_HASH='$hash'"
    echo "SESSION_SECRET=$(openssl rand -base64 64 | tr -d '\n')"
    echo 'MAX_FILE_SIZE_MB=200'
    echo 'COOKIE_SECURE=true'
    echo 'RUST_LOG=info'
    echo "PUBLIC_BASE_URL=https://$RFM_DOMAIN"
    echo 'UPLOAD_CHUNK_SIZE_MB=8'
    echo 'MAX_CHUNKED_FILE_SIZE_MB=4096'
  } >>"$tmp"
  mv -f "$tmp" "$RFM_ENV"
  ok "настройки записаны: $RFM_ENV (секреты на экран не выводятся)"
}

# Мини-приложение и внутренний API для бота — добавить, если их нет.
# Существующие строки не меняются. FM_ENV_CHANGED=1, если что-то дописано.
fm_ensure_bot_settings() {
  local id='' user='' backed=0
  FM_ENV_CHANGED=0
  [[ -f $RFM_ENV ]] || return 0
  if ! env_has "$RFM_ENV" INTERNAL_BIND_ADDR; then
    backup_secret_file "$RFM_ENV" "$RFM_BACKUPS" env; backed=1
    {
      echo '# Внутренний API только для бота; наружу не открывать (deploy)'
      echo 'INTERNAL_BIND_ADDR=127.0.0.1:8091'
      echo "INTERNAL_API_TOKEN=$(openssl rand -hex 32)"
    } >>"$RFM_ENV"
    FM_ENV_CHANGED=1
    ok "включён внутренний API для бота (127.0.0.1:8091)"
  fi
  if ! env_has "$RFM_ENV" MINIAPP_ENABLED; then
    if read -r id user < <(bot_getme); then
      (( backed )) || backup_secret_file "$RFM_ENV" "$RFM_BACKUPS" env
      local chunk=1 chunked=1
      env_has "$RFM_ENV" UPLOAD_CHUNK_SIZE_MB && chunk=0
      env_has "$RFM_ENV" MAX_CHUNKED_FILE_SIZE_MB && chunked=0
      {
        echo '# Мини-приложение «Файлы» в Telegram (deploy)'
        echo 'MINIAPP_ENABLED=true'
        echo "TELEGRAM_BOT_ID=$id"
        echo "TELEGRAM_BOT_USERNAME=$user"
        (( chunk )) && echo 'UPLOAD_CHUNK_SIZE_MB=8'
        (( chunked )) && echo 'MAX_CHUNKED_FILE_SIZE_MB=4096'
      } >>"$RFM_ENV"
      if ! env_has "$RFM_ENV" PUBLIC_BASE_URL && [[ -n $RFM_DOMAIN ]]; then
        echo "PUBLIC_BASE_URL=https://$RFM_DOMAIN" >>"$RFM_ENV"
      fi
      FM_ENV_CHANGED=1
      ok "включено мини-приложение для @$user"
    else
      warn "Не удалось узнать ID бота (проверьте BOT_TOKEN в $BOT_DIR/.env) — мини-приложение не включено"
      TODO+=("Включить мини-приложение: после исправления BOT_TOKEN запустите deploy ещё раз")
    fi
  fi
}

fm_install_unit() {  # каталог_сборки
  local unit=$UNIT_DIR/$RFM_UNIT
  if [[ ! -f $unit ]]; then
    install -o root -g root -m 0644 "$1/deploy/rust-file-manager.service" "$unit" || die "Нет файла службы"
    systemctl daemon-reload
  fi
  systemctl enable "$RFM_UNIT" >/dev/null 2>&1
}

fm_restart_checked() {  # [ожидаемый_коммит]
  local t0
  t0=$(date +%s)
  systemctl restart "$RFM_UNIT" || { fail "systemctl restart $RFM_UNIT не прошёл"; return 1; }
  fm_health "$t0" "${1:-}"
}

fm_nginx() {
  local site_dir=$NGINX_DIR/sites-available en_dir=$NGINX_DIR/sites-enabled domain=$RFM_DOMAIN example
  [[ -n $domain ]] || domain=$(env_get "$RFM_ENV" PUBLIC_BASE_URL | sed 's#^https\?://##; s#/.*##')
  [[ -n $HTTPS_MODE ]] || return 0
  [[ -n $domain ]] || { warn "Домен неизвестен — nginx не настраиваю"; return 0; }
  case $HTTPS_MODE in
    none)
      say "HTTPS обеспечивает ваш прокси — nginx не трогаю"
      TODO+=("Направьте https://$domain на http://127.0.0.1:8080 в своём прокси")
      return 0 ;;
  esac
  if grep -RqsE "server_name[^;]*[[:space:]]${domain}[[:space:];]" "$en_dir"/ 2>/dev/null; then
    ok "nginx уже обслуживает $domain — конфигурацию не трогаю"
  elif [[ $HTTPS_MODE == certbot ]]; then
    example=$(fm_build_dir)/deploy/nginx.example.conf
    sed -e "s/server_name example\.com;/server_name $domain;/" \
        -e 's/client_max_body_size 200M;/client_max_body_size 201M;/' "$example" >"$site_dir/rust-file-manager" &&
      ln -sf "$site_dir/rust-file-manager" "$en_dir/rust-file-manager" ||
      { fail "не удалось записать конфигурацию nginx"; return 1; }
    ok "nginx: $site_dir/rust-file-manager"
  else
    if [[ ! -s $NGINX_DIR/ssl/$domain/cert.pem || ! -s $NGINX_DIR/ssl/$domain/key.pem ]]; then
      warn "Нет сертификата Cloudflare в $NGINX_DIR/ssl/$domain/ (cert.pem, key.pem)"
      TODO+=("Положите Origin-сертификат Cloudflare в $NGINX_DIR/ssl/$domain/ (DEPLOYwTELEGRAM.md §2.7Б) и запустите deploy ещё раз")
      return 0
    fi
    chmod 600 "$NGINX_DIR/ssl/$domain/key.pem"
    nginx_cloudflare_site "$domain" >"$site_dir/rust-file-manager-ssl" &&
      ln -sf "$site_dir/rust-file-manager-ssl" "$en_dir/rust-file-manager-ssl" ||
      { fail "не удалось записать конфигурацию nginx"; return 1; }
    ok "nginx: $site_dir/rust-file-manager-ssl (порт 2083)"
    TODO+=("Cloudflare: SSL/TLS Full (strict), Origin Rule $domain → порт 2083, Bypass cache для /api/* и /d/*")
  fi
  nginx -t >/dev/null 2>&1 || { fail "nginx -t: ошибка в конфигурации"; nginx -t; return 1; }
  systemctl reload nginx || { fail "nginx не перезагрузился"; return 1; }
  if [[ $HTTPS_MODE == certbot && ! -d $LE_DIR/live/$domain ]]; then
    local mail=(--register-unsafely-without-email)
    [[ -n $LE_EMAIL ]] && mail=(-m "$LE_EMAIL")
    if certbot --nginx -d "$domain" --non-interactive --agree-tos --redirect "${mail[@]}" >/dev/null 2>&1; then
      ok "сертификат Let's Encrypt для $domain"
    else
      warn "certbot не выпустил сертификат: проверьте, что A-запись $domain указывает на этот сервер"
      TODO+=("Выпустить сертификат: certbot --nginx -d $domain, или запустите deploy ещё раз")
    fi
  fi
}

nginx_cloudflare_site() {  # домен
  cat <<EOF
server {
    listen 2083 ssl;
    server_name $1;
    ssl_certificate     $NGINX_DIR/ssl/$1/cert.pem;
    ssl_certificate_key $NGINX_DIR/ssl/$1/key.pem;
    client_max_body_size 201M;

    # Мини-приложение: части загрузки идут сразу в приложение, без буфера на диске
    location /api/v1/uploads/ {
        client_max_body_size 10M;
        proxy_request_buffering off;
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    # В пути подписанной ссылки лежит токен: не писать её в журнал
    location /d/ {
        access_log off;
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_read_timeout 300;
    }

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_read_timeout 300;
        proxy_send_timeout 300;
    }
}
EOF
}

# Новый выпуск файлового менеджера. Если проверки после перезапуска не прошли —
# возвращает прежнюю программу.
fm_update() {
  local dir ref head running backup old_ver
  dir=$(fm_build_dir)
  ref=${RFM_REF:-$(state_get RFM_REF)}; ref=${ref:-main}
  step "Файловый менеджер: обновление из $ref"
  fm_checkout "$dir" "$ref" || return 1
  # --ref запоминается: следующий post_deploy останется на этом выпуске.
  [[ -n $RFM_REF ]] && state_set RFM_REF "$RFM_REF"
  head=$(git -C "$dir" rev-parse HEAD)
  running=$(fm_commit); running=${running%-dirty}
  say "работает: $(fm_version || true) (${running:-?}), в $ref: $(grep -m1 '^version' "$dir/Cargo.toml" | cut -d'"' -f2) (${head:0:7})"
  if [[ -n $running && $head == "$running"* ]] && (( ! FORCE )); then
    ok "уже последняя версия"
    SUMMARY+=("Файловый менеджер: без изменений, $(fm_version)")
    return 0
  fi
  fm_build "$dir" || return 1
  if cmp -s "$dir/target/release/rust-file-manager" "$RFM_BIN" && (( ! FORCE )); then
    ok "собранная программа совпадает с установленной"
    SUMMARY+=("Файловый менеджер: без изменений, $(fm_version)")
    return 0
  fi
  old_ver=$(fm_version); old_ver=${old_ver:-old}
  install -d -m 0700 "$RFM_BACKUPS"
  backup="$RFM_BACKUPS/rust-file-manager-$old_ver-$(ts)"
  cp -p "$RFM_BIN" "$backup" || { fail "не удалось сохранить прежнюю программу"; return 1; }
  if (( BACKUP_DATA )); then
    say "Копия данных (служба остановлена на время архивации)…"
    systemctl stop "$RFM_UNIT"
    tar --acls --xattrs -cpf "$RFM_BACKUPS/data-$old_ver-$(ts).tar" -C "$RFM_DATA" . || warn "архив данных не создан"
    systemctl start "$RFM_UNIT"
  fi
  install -o root -g root -m 0755 "$dir/target/release/rust-file-manager" "$RFM_BIN.next" &&
    mv -fT "$RFM_BIN.next" "$RFM_BIN" || { fail "не удалось заменить программу"; return 1; }
  if fm_restart_checked "$head"; then
    prune_backups "$RFM_BACKUPS" 'rust-file-manager-*'
    prune_backups "$RFM_BACKUPS" 'data-*.tar'
    state_set RFM_BUILD_DIR "$dir"
    SUMMARY+=("Файловый менеджер: $old_ver → $(fm_version) (${head:0:7})")
    return 0
  fi
  fail "Новая версия не прошла проверку — возвращаю прежнюю ($old_ver)"
  cp -p "$backup" "$RFM_BIN.rollback" && mv -fT "$RFM_BIN.rollback" "$RFM_BIN" && systemctl restart "$RFM_UNIT"
  sleep 3
  systemctl is-active --quiet "$RFM_UNIT" && warn "Прежняя версия $old_ver снова работает" ||
    fail "Служба не запустилась и после отката: journalctl -u $RFM_UNIT -n 50"
  SUMMARY+=("Файловый менеджер: ОТКАТ на $old_ver, новая версия не запустилась (журнал сборки $BUILD_LOG)")
  return 1
}

fm_install_fresh() {
  local dir ref head
  dir=$(fm_build_dir)
  ref=${RFM_REF:-main}
  step "Файловый менеджер: установка"
  fm_checkout "$dir" "$ref" || die "Не удалось получить код rust-file-manager"
  fm_build "$dir" || die "Сборка не удалась"
  head=$(git -C "$dir" rev-parse HEAD)
  fm_create_user_dirs
  if [[ ! -e $RFM_BIN ]]; then
    install -o root -g root -m 0755 "$dir/target/release/rust-file-manager" "$RFM_BIN" || die "Не удалось установить программу"
  fi
  fm_write_env "$dir"
  bot_present && fm_ensure_bot_settings
  fm_install_unit "$dir"
  fm_restart_checked "$head" || die "Файловый менеджер не запустился: journalctl -u $RFM_UNIT -n 50"
  state_set RFM_BUILD_DIR "$dir"
  state_set RFM_REF "$ref"
  SUMMARY+=("Файловый менеджер: установлен $(fm_version), https://$RFM_DOMAIN")
}

# --- Бот ---------------------------------------------------------------------
bot_clone() {
  [[ -d $BOT_DIR/.git ]] && return 0
  say "Репозиторий TelegramOnly приватный: на запрос Username введите логин GitHub,"
  say "на запрос Password — токен GitHub с правом чтения этого репозитория"
  git clone -q "$BOT_REPO_URL" "$BOT_DIR" || die "Не удалось скачать TelegramOnly"
  ok "код бота: $BOT_DIR, версия $(bot_repo_version)"
}

bot_pin_compose_files() {  # бот в сети хоста: закрепить compose.host.yaml в .env
  local label files e=$BOT_DIR/.env
  label=$(docker inspect -f '{{index .Config.Labels "com.docker.compose.project.config_files"}}' "$BOT_CONTAINER" 2>/dev/null)
  [[ $label == *,* ]] || return 0
  env_has "$e" COMPOSE_FILE && return 0
  files=$(tr ',' '\n' <<<"$label" | sed "s#^$BOT_DIR/##" | paste -sd: -)
  backup_secret_file "$e" "$BOT_BACKUPS" env
  printf '\n# Бот запущен несколькими compose-файлами: docker compose в этом каталоге берёт их все\nCOMPOSE_FILE=%s\n' "$files" >>"$e"
  ok "в .env бота записано COMPOSE_FILE=$files — пересборка не выбьет бота из его сети"
}

bot_install_fresh() {
  step "Бот TelegramOnly: установка"
  ensure_docker
  bot_clone
  if [[ $BOT_NETWORK == host ]]; then
    [[ -f $BOT_DIR/.env ]] || cp "$BOT_DIR/example.env" "$BOT_DIR/.env"
    chmod 600 "$BOT_DIR/.env"
    env_has "$BOT_DIR/.env" COMPOSE_FILE ||
      printf '\n# Бот в сети хоста: docker compose в этом каталоге берёт оба файла\nCOMPOSE_FILE=compose.yaml:compose.host.yaml\n' >>"$BOT_DIR/.env"
  fi
  say "Установщик бота спросит BOT_TOKEN, ваш Telegram ID и публичный адрес сервера"
  ( cd "$BOT_DIR" && bash scripts/install_telegramonly_docker.sh ) || die "Установка бота не удалась"
  docker inspect "$BOT_CONTAINER" >/dev/null 2>&1 || die "Контейнер $BOT_CONTAINER не появился"
  ok "бот $(bot_container_version) запущен, сеть $(bot_net)"
  SUMMARY+=("Бот TelegramOnly: установлен $(bot_container_version), Docker, сеть $(bot_net)")
}

bot_update() {
  local flavor before after behind ahead cver rver old_head before_ver
  flavor=$(bot_flavor)
  step "Бот TelegramOnly: обновление"
  if [[ -n $(git -C "$BOT_DIR" status --porcelain --untracked-files=no 2>/dev/null) ]]; then
    warn "В $BOT_DIR изменены файлы — бота не обновляю:"; git -C "$BOT_DIR" status --short | head -5 >&2
    SUMMARY+=("Бот: пропущен — в $BOT_DIR есть несохранённые правки"); return 1
  fi
  [[ $flavor == docker ]] && bot_pin_compose_files
  git -C "$BOT_DIR" fetch -q origin main || { fail "git fetch не прошёл (нужен токен GitHub?)"; SUMMARY+=("Бот: не обновлён — git fetch не прошёл"); return 1; }
  behind=$(git -C "$BOT_DIR" rev-list --count HEAD..origin/main)
  ahead=$(git -C "$BOT_DIR" rev-list --count origin/main..HEAD)
  rver=$(git -C "$BOT_DIR" show origin/main:pyproject.toml 2>/dev/null | grep -m1 -E '^version' | cut -d'"' -f2)
  cver=$(bot_running_version)
  say "работает: ${cver:-?}, на GitHub: ${rver:-?}"
  if (( ahead > 0 )); then
    warn "В $BOT_DIR есть свои коммиты ($ahead), которых нет на GitHub — не обновляю"
    SUMMARY+=("Бот: пропущен — локальные коммиты в $BOT_DIR"); return 1
  fi
  if (( behind == 0 )) && [[ $cver == "$(bot_repo_version)" ]] && (( ! FORCE )); then
    ok "уже последняя версия"; SUMMARY+=("Бот: без изменений, $cver"); return 0
  fi
  old_head=$(git -C "$BOT_DIR" rev-parse HEAD)
  before_ver=$cver
  git -C "$BOT_DIR" merge -q --ff-only origin/main || { fail "git merge --ff-only не прошёл"; return 1; }
  if [[ $flavor == docker ]]; then
    before=$(bot_net)
    ( cd "$BOT_DIR" && bash scripts/rebuild_bot.sh --prune ) || { fail "rebuild_bot.sh завершился с ошибкой"; return 1; }
    after=$(bot_net)
    [[ $before == "$after" ]] || warn "сеть бота изменилась: $before → $after"
    docker image prune -f >/dev/null 2>&1 || true
    cver=$(bot_container_version)
    if [[ $cver != "$(bot_repo_version)" ]]; then
      fail "в контейнере $cver, а в репозитории $(bot_repo_version) — образ не пересобрался"
      SUMMARY+=("Бот: ОШИБКА — образ не пересобрался (см. вывод rebuild_bot.sh)"); return 1
    fi
  else
    if ! git -C "$BOT_DIR" diff --quiet "$old_head" HEAD -- requirements.txt && [[ -x $BOT_DIR/venv/bin/pip ]]; then
      "$BOT_DIR/venv/bin/pip" install -q -r "$BOT_DIR/requirements.txt" || warn "pip install не прошёл"
    fi
    systemctl restart "$BOT_UNIT" && sleep "$WAIT_SECONDS"
    systemctl is-active --quiet "$BOT_UNIT" || { fail "$BOT_UNIT не запустился"; return 1; }
  fi
  ok "бот $(bot_running_version)"
  SUMMARY+=("Бот TelegramOnly: ${before_ver:-?} → $(bot_running_version)")
}

bot_recreate() {
  case $(bot_flavor) in
    docker) ( cd "$BOT_DIR" && docker compose up -d --force-recreate "$BOT_SERVICE" >/dev/null 2>&1 ) ;;
    systemd) systemctl restart "$BOT_UNIT" ;;
  esac
}

# Адрес внутреннего API, по которому бот достаёт файловый менеджер. Для сети
# bridge ставит мост socat на шлюзе docker-сети и открывает его только для неё.
bot_files_url() {
  local net gw subnet unit=$UNIT_DIR/$BRIDGE_UNIT
  if [[ $(bot_flavor) == systemd ]]; then echo http://127.0.0.1:8091; return; fi
  net=$(bot_net)
  if [[ $net == host ]]; then echo http://127.0.0.1:8091; return; fi
  gw=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.Gateway}}{{end}}' "$BOT_CONTAINER" 2>/dev/null)
  [[ $gw =~ ^[0-9]+(\.[0-9]+){3}$ ]] || { fail "не удалось узнать шлюз docker-сети бота"; return 1; }
  subnet=$(docker network inspect -f '{{range .IPAM.Config}}{{.Subnet}}{{end}}' "$net" 2>/dev/null)
  has socat || DEBIAN_FRONTEND=noninteractive apt-get install -y -q socat >/dev/null
  if [[ ! -f $unit ]]; then
    cat >"$unit" <<EOF
# socat-мост: бот TelegramOnly (docker-сеть bridge) → внутренний API rust-file-manager.
# Создано командой deploy (rfm-vps). Шлюз docker-сети: $gw.
[Unit]
Description=socat bridge: docker gateway -> rust-file-manager internal API
After=docker.service $RFM_UNIT
Requires=docker.service

[Service]
ExecStart=/usr/bin/socat TCP-LISTEN:8091,bind=$gw,fork,reuseaddr TCP:127.0.0.1:8091
Restart=always
RestartSec=3
DynamicUser=true
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable --now "$BRIDGE_UNIT" >/dev/null 2>&1 || { fail "мост $BRIDGE_UNIT не запустился"; return 1; }
    ok "мост socat $gw:8091 → 127.0.0.1:8091" >&2
  fi
  if has ufw && ufw status 2>/dev/null | grep -q 'Status: active' && [[ -n $subnet ]]; then
    ufw allow from "$subnet" to "$gw" port 8091 proto tcp >/dev/null && ok "ufw: 8091 открыт только для $subnet" >&2
  fi
  echo "http://$gw:8091"
}

# Связать бота с файловым менеджером: настройки на обеих сторонах, без вывода ключей.
link_bot_fm() {
  local e=$BOT_DIR/.env token url public changed=0
  bot_present && fm_present || return 0
  step "Связка бота и файлового менеджера"
  fm_ensure_bot_settings
  if (( FM_ENV_CHANGED )); then
    fm_restart_checked || { fail "файловый менеджер не запустился с новыми настройками"; return 1; }
  fi
  token=$(env_get "$RFM_ENV" INTERNAL_API_TOKEN)
  public=$(env_get "$RFM_ENV" PUBLIC_BASE_URL)
  [[ -n $token && -n $public ]] || { warn "в настройках файлового менеджера нет INTERNAL_API_TOKEN или PUBLIC_BASE_URL"; return 1; }
  if ! env_has "$e" FILES_SERVICE_TOKEN; then
    url=$(bot_files_url) || return 1
    backup_secret_file "$e" "$BOT_BACKUPS" env
    {
      echo
      echo '# «Файлы» — мини-приложение rust-file-manager на этом же VPS (deploy)'
      echo "FILES_MINIAPP_URL=$public/tg/"
      echo "FILES_INTERNAL_URL=$url"
      echo "FILES_SERVICE_TOKEN=$token"
      echo 'FILES_MENU_BUTTON=webapp'
    } >>"$e"
    changed=1
    ok "в .env бота добавлены FILES_* (адрес $url)"
  elif [[ $(env_get "$e" FILES_SERVICE_TOKEN) != "$token" ]]; then
    backup_secret_file "$e" "$BOT_BACKUPS" env
    sed -i "s|^FILES_SERVICE_TOKEN=.*|FILES_SERVICE_TOKEN=$token|" "$e"
    changed=1
    ok "ключ бота приведён к ключу файлового менеджера"
  else
    ok "бот уже связан с файловым менеджером, ключи совпадают"
  fi
  if (( changed )); then
    bot_recreate
    say "Жду, пока бот запустится…"
    for _ in $(seq 1 12); do
      sleep "$WAIT_SECONDS"
      if [[ $(bot_flavor) == docker ]] && docker logs --since 3m "$BOT_CONTAINER" 2>&1 | grep -q 'files: кнопки меню'; then break; fi
      [[ $(bot_flavor) == systemd ]] && break
    done
    [[ $(bot_flavor) == docker ]] && docker logs --since 3m "$BOT_CONTAINER" 2>&1 | grep -E 'files: кнопки меню' | tail -1 | sed 's/^/    /'
    SUMMARY+=("Связка: бот видит файловый менеджер, кнопка «Файлы» включена")
  fi
}

# --- Самообновление команд ---------------------------------------------------
# Установленные команды перед работой сверяются с репозиторием (та же ветка или
# тег, что и для сборки) и при отличии обновляются и перезапускаются — до
# вопросов, чтобы ничего не спрашивать дважды.
self_update() {
  local cur tmp ref url
  [[ ${RFM_VPS_REEXEC:-} == 1 ]] && return 0
  cur=$(readlink -f "$CMD_DIR/rfm-vps" 2>/dev/null) || return 0
  [[ -f $cur && $(readlink -f "$0") == "$cur" ]] || return 0
  ref=${RFM_REF:-$(state_get RFM_REF)}; ref=${ref:-$RFM_VPS_REF}
  url="$RFM_VPS_RAW/$ref/deploy/rfm-vps.sh"
  tmp=$(mktemp) || return 0
  if ! curl -fsSL --max-time 20 "$url" -o "$tmp" 2>/dev/null; then
    rm -f "$tmp"; warn "не удалось проверить новую версию команд ($url) — продолжаю с текущей"; return 0
  fi
  if cmp -s "$tmp" "$cur"; then rm -f "$tmp"; return 0; fi
  if ! bash -n "$tmp" 2>/dev/null || ! grep -q 'rfm-vps' "$tmp"; then
    rm -f "$tmp"; warn "скачанная версия команд повреждена — продолжаю с текущей"; return 0
  fi
  install -m 0755 "$tmp" "$cur"; rm -f "$tmp"
  say "Команды deploy/post_deploy обновлены ($url) — перезапускаю"
  RFM_VPS_REEXEC=1 exec "$cur" "$CMD" "${ORIG_ARGS[@]}"
}

# --- Команды -----------------------------------------------------------------
cmd_setup() {
  local tmp src
  [[ $EUID -eq 0 ]] || die "Запустите от root"
  install -d -m 0755 "$CMD_DIR"
  tmp=$(mktemp)
  if [[ -f $0 && $(head -c 300 "$0" 2>/dev/null) == *rfm-vps* ]]; then
    cp "$0" "$tmp"; src=$0
  else
    src="$RFM_VPS_RAW/$RFM_VPS_REF/deploy/rfm-vps.sh"
    curl -fsSL "$src" -o "$tmp" || { rm -f "$tmp"; die "Не удалось скачать $src"; }
  fi
  bash -n "$tmp" || { rm -f "$tmp"; die "Скачанный скрипт повреждён"; }
  install -m 0755 "$tmp" "$CMD_DIR/rfm-vps"; rm -f "$tmp"
  ln -sfn rfm-vps "$CMD_DIR/deploy"
  ln -sfn rfm-vps "$CMD_DIR/post_deploy"
  ln -sfn rfm-vps "$CMD_DIR/post-deploy"
  ok "команды установлены в $CMD_DIR из $src"
  cat <<EOF

    deploy        — установить на этот сервер бота и файловый менеджер (только то, чего нет)
    post_deploy   — обновить то, что уже установлено
    rfm-vps status — что сейчас стоит на сервере

EOF
}

cmd_status() {
  print_status
  [[ -f $STATE_FILE ]] && say "настройки команд: $STATE_FILE"
  return 0
}

deploy_questions() {
  local busy443=''
  step "Вопросы (дальше установка пойдёт сама)"
  if [[ -z $INSTALL_BOT ]]; then
    if bot_present; then INSTALL_BOT=no; elif [[ $ONLY == fm ]]; then INSTALL_BOT=no; else INSTALL_BOT=yes; fi
  fi
  if [[ -z $INSTALL_FM ]]; then
    if fm_present; then INSTALL_FM=no; elif [[ $ONLY == bot ]]; then INSTALL_FM=no; else INSTALL_FM=yes; fi
  fi
  if [[ $INSTALL_FM == no && $INSTALL_BOT == no ]]; then return 0; fi
  if [[ $INSTALL_BOT == yes ]]; then
    ask_choice BOT_NETWORK "Сеть Docker для бота: bridge — обычная, host — если боту нужна mesh-сеть Tailscale/Headscale" bridge "bridge host"
  fi
  if [[ $INSTALL_FM == yes ]]; then
    while [[ ! $RFM_DOMAIN =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?\.[A-Za-z]{2,}$ ]]; do
      [[ -n $RFM_DOMAIN ]] && warn "Это не похоже на домен: $RFM_DOMAIN"
      RFM_DOMAIN=''
      ask RFM_DOMAIN "Домен файлового менеджера (A-запись на этот сервер), например files.example.com"
      (( ASSUME_YES )) && [[ ! $RFM_DOMAIN =~ \. ]] && die "Задайте RFM_DOMAIN"
    done
    ss -tlnH 2>/dev/null | awk '{print $4}' | grep -qE ':443$' && busy443=1
    if [[ -n $busy443 ]]; then say "Порт 443 уже занят другим сервисом — подойдёт Cloudflare на порт 2083"; fi
    ask_choice HTTPS_MODE "HTTPS: certbot — 443 свободен; cloudflare — через Cloudflare на 2083; none — свой прокси" \
      "$([[ -n $busy443 ]] && echo cloudflare || echo certbot)" "certbot cloudflare none"
    [[ $HTTPS_MODE == certbot ]] && ask LE_EMAIL "Почта для Let's Encrypt (Enter — без почты)" ""
    ask RFM_ADMIN_LOGIN "Логин администратора сайта" admin
    [[ -e $RFM_ENV ]] || ask_password
  fi
  if [[ -z $ENABLE_UFW ]] && has ufw && ! ufw status 2>/dev/null | grep -q 'Status: active'; then
    if confirm "Включить файрвол ufw (SSH, 80 и порт HTTPS)?" y; then ENABLE_UFW=yes; else ENABLE_UFW=no; fi
  fi
  step "План"
  [[ $INSTALL_BOT == yes ]] && say "• бот TelegramOnly в Docker, сеть $BOT_NETWORK"
  [[ $INSTALL_FM == yes ]] && say "• файловый менеджер на https://$RFM_DOMAIN, HTTPS: $HTTPS_MODE, логин $RFM_ADMIN_LOGIN"
  say "• связать бота с файловым менеджером (мини-приложение «Файлы», уведомления)"
  confirm "Начинаем?" y || die "Отменено"
}

cmd_deploy() {
  preflight
  self_update
  take_lock
  print_status
  deploy_questions
  [[ -n $HTTPS_MODE ]] || HTTPS_MODE=$(state_get HTTPS_MODE)
  if [[ $INSTALL_FM == no && $INSTALL_BOT == no ]]; then
    step "Всё уже установлено"
    # Повторный deploy доделывает то, что в прошлый раз отложено: например,
    # nginx на 2083 после того, как положили сертификат Cloudflare.
    fm_present && fm_nginx
    link_bot_fm
    say "Для обновления используйте: post_deploy"
    print_summary
    return 0
  fi
  INSTALLED_NOW=1
  install_packages
  step "Подготовка сервера"
  setup_sysctl
  setup_swap
  setup_firewall
  [[ $INSTALL_BOT == yes ]] && bot_install_fresh
  [[ $INSTALL_FM == yes ]] && fm_install_fresh
  if fm_present && [[ -n $HTTPS_MODE ]]; then
    step "nginx и HTTPS"
    fm_nginx
    state_set HTTPS_MODE "$HTTPS_MODE"
  fi
  link_bot_fm
  final_checks
  print_summary
}

cmd_post_deploy() {
  local any=0 rc=0
  [[ $EUID -eq 0 ]] || die "Запустите от root: sudo -i, затем команду ещё раз"
  self_update
  take_lock
  print_status
  fm_present && any=1
  bot_present && any=1
  if (( ! any )); then
    step "На сервере нет ни бота, ни файлового менеджера"
    say "Для установки с нуля: deploy"
    return 1
  fi
  if (( CHECK_ONLY )); then check_updates; return 0; fi
  # Сначала файловый менеджер, потом бот: новые функции бота опираются на API
  # файлового менеджера, а две сборки сразу на маленьком VPS опасны.
  if fm_present && [[ $ONLY != bot ]]; then fm_update || rc=1; fi
  if bot_present && [[ $ONLY != fm ]]; then
    [[ -d $BOT_DIR/.git ]] && { bot_update || rc=1; } || warn "Нет $BOT_DIR/.git — бота не обновляю"
  fi
  link_bot_fm || rc=1
  final_checks
  print_summary
  return "$rc"
}

check_updates() {
  local dir ref remote running rver
  step "Доступные обновления (ничего не меняю)"
  if fm_present; then
    dir=$(fm_build_dir); ref=${RFM_REF:-$(state_get RFM_REF)}; ref=${ref:-main}
    remote=$(git ls-remote "$RFM_REPO_URL" "$ref" "$ref^{}" 2>/dev/null | tail -1 | cut -f1)
    running=$(fm_commit); running=${running%-dirty}
    if [[ -z $remote ]]; then warn "не удалось узнать $ref на GitHub"
    elif [[ -n $running && $remote == "$running"* ]]; then ok "файловый менеджер: последняя версия ($running)"
    else say "файловый менеджер: есть обновление ${running:-?} → ${remote:0:7}"; fi
  fi
  if bot_present && [[ -d $BOT_DIR/.git ]]; then
    if git -C "$BOT_DIR" fetch -q origin main 2>/dev/null; then
      rver=$(git -C "$BOT_DIR" show origin/main:pyproject.toml 2>/dev/null | grep -m1 -E '^version' | cut -d'"' -f2)
      if [[ $(git -C "$BOT_DIR" rev-list --count HEAD..origin/main) == 0 ]]; then ok "бот: последняя версия ($(bot_running_version))"
      else say "бот: есть обновление $(bot_running_version) → $rver"; fi
    else
      warn "бот: git fetch не прошёл"
    fi
  fi
  say "Обновить: post_deploy"
}

final_checks() {
  local user
  step "Проверка"
  if fm_present; then
    systemctl is-active --quiet "$RFM_UNIT" && ok "файловый менеджер $(fm_version) работает" || fail "файловый менеджер не работает"
  fi
  if bot_present; then
    ok "бот $(bot_running_version) $([[ $(bot_flavor) == docker ]] && echo "(сеть $(bot_net))")"
  fi
  if fm_present && bot_present && [[ $(env_get "$RFM_ENV" MINIAPP_ENABLED) == true ]]; then
    user=$(env_get "$RFM_ENV" TELEGRAM_BOT_USERNAME)
    TODO+=("Проверьте в Telegram: /version, /files_status; кнопка «Файлы» у привязанных")
    if (( INSTALLED_NOW )); then
      TODO+=("@BotFather → /mybots → @$user → Bot Settings → Configure Mini App → Enable Mini App → $(env_get "$RFM_ENV" PUBLIC_BASE_URL)/tg/")
      TODO+=("Откройте https://t.me/$user?startapp и войдите логином администратора — так ваш Telegram станет администратором файлов")
    fi
  fi
}

print_summary() {
  local s
  step "Итог"
  for s in "${SUMMARY[@]}"; do say "• $s"; done
  if (( ${#TODO[@]} )); then
    printf '\n%sОсталось сделать вручную:%s\n' "$C_B" "$C_0"
    for s in "${TODO[@]}"; do say "• $s"; done
  fi
  printf '\n'
}

usage() {
  cat <<'EOF'
rfm-vps — бот TelegramOnly и rust-file-manager на одном VPS

  deploy [--only bot|fm] [--ref REF] [--yes]
      Установить то, чего на сервере нет, и связать бота с файловым менеджером.
      Сначала задаёт все вопросы, потом ставит сам.
  post_deploy [--check] [--only bot|fm] [--ref REF] [--force] [--backup-data]
      Обновить установленное. --check — только показать, есть ли обновления.
      --ref закрепляет выпуск для следующих запусков (вернуться: --ref main).
      --force — пересобрать и перезапустить, даже если версия та же.
      --backup-data — перед заменой программы сохранить архив данных.
  rfm-vps status    что стоит на сервере
  rfm-vps setup     (пере)установить команды deploy и post_deploy

REF — ветка или тег rust-file-manager (по умолчанию main).
Справка: DEPLOYwTELEGRAM.md и POST_DEPLOYwTELEGRAM.md в репозитории rust-file-manager.
EOF
}

main() {
  ORIG_ARGS=("$@")
  case ${0##*/} in
    deploy) CMD=deploy ;;
    post_deploy|post-deploy) CMD=post_deploy ;;
    *) CMD=${1:-help}; shift || true; ORIG_ARGS=("$@") ;;
  esac
  [[ $CMD == post-deploy ]] && CMD=post_deploy
  while (( $# )); do
    case $1 in
      --yes|-y) ASSUME_YES=1 ;;
      --force) FORCE=1 ;;
      --check) CHECK_ONLY=1 ;;
      --backup-data) BACKUP_DATA=1 ;;
      --only) ONLY=${2:-}; shift ;;
      --ref) RFM_REF=${2:-}; shift ;;
      -h|--help) usage; return 0 ;;
      *) die "Неизвестный параметр: $1 (см. --help)" ;;
    esac
    shift
  done
  [[ -z $ONLY || $ONLY == bot || $ONLY == fm ]] || die "--only: bot или fm"
  case $CMD in
    setup) cmd_setup ;;
    deploy) cmd_deploy ;;
    post_deploy) cmd_post_deploy ;;
    status) cmd_status ;;
    help|-h|--help) usage ;;
    *) usage; return 1 ;;
  esac
}

main "$@"; exit $?
