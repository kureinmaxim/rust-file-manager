#!/usr/bin/env bash
# rfm-vps — установка и обновление rust-file-manager на VPS.
#
# Команды (ставятся в /usr/local/sbin командой setup):
#   deploy           установить файловый менеджер, nginx и HTTPS, если их ещё нет
#   post_deploy      обновить установленный файловый менеджер (с проверкой и откатом)
#   rfm-vps status   показать, что установлено (только чтение)
#
# Первый запуск на сервере, под root:
#   curl -fsSL https://raw.githubusercontent.com/kureinmaxim/rust-file-manager/main/deploy/rfm-vps.sh | bash -s -- setup
#
# Те же шаги вручную и пояснения: DEPLOY_ALGORITHM.md, DEPLOY.md, POST_DEPLOY.md.
# Связка с Telegram-ботом (мини-приложение «Файлы») — TELEGRAM_MINIAPP.md;
# эта команда бота не ставит и не трогает.
#
# Скрипт не выводит секреты. Пароль администратора вводится скрыто, ключи
# создаются на сервере и пишутся в файлы с правами 600; на экране — только
# «да/нет» и длины.

set -uo pipefail

# --- Пути и имена. Переопределяются переменными окружения (для тестов) -------
: "${RFM_REPO_URL:=https://github.com/kureinmaxim/rust-file-manager.git}"
# Запасные источники кода, если GitHub недоступен (DEPLOY.md, раздел «Без GitHub»):
# свои зеркала и bundle, присланный с локальной машины.
: "${BUNDLE_DIR:=/var/cache/rfm-vps}"
: "${RFM_MIRRORS:=}"      # ещё зеркала rust-file-manager (git-адреса через пробел)
: "${GIT_TIMEOUT:=90}"    # секунд на fetch из одного источника
: "${GIT_CLONE_TIMEOUT:=600}"
# Адреса, не ответившие в этом запуске, больше не пробуем (переживает перезапуск команд).
: "${VPS_SRC_DOWN:=}"; export VPS_SRC_DOWN
: "${RFM_VPS_RAW:=https://raw.githubusercontent.com/kureinmaxim/rust-file-manager}"
: "${RFM_VPS_REF:=main}"
: "${RFM_BIN:=/usr/local/bin/rust-file-manager}"
: "${RFM_UNIT:=rust-file-manager.service}"
: "${UNIT_DIR:=/etc/systemd/system}"
: "${RFM_ENV:=/etc/rust-file-manager/env}"
: "${RFM_DATA:=/var/lib/rust-file-manager}"
: "${RFM_BACKUPS:=/var/backups/rust-file-manager}"
: "${BUILD_ROOT:=/root}"
: "${STATE_FILE:=/etc/rfm-vps.conf}"
: "${CMD_DIR:=/usr/local/sbin}"
: "${NGINX_DIR:=/etc/nginx}"
: "${LE_DIR:=/etc/letsencrypt}"
: "${SWAP_FILE:=/swapfile}"
: "${LOCK_FILE:=/run/rfm-vps.lock}"
: "${DELETE_BACKUPS:=/var/backups/rfm-delete}"   # копии команды delete
: "${CARGO_ENV:=$HOME/.cargo/env}"
: "${FM_USER:=filemgr}"
: "${FSTAB:=/etc/fstab}"
: "${MEMINFO:=/proc/meminfo}"
: "${POLL_SECONDS:=15}"      # как часто показывать ход сборки
: "${WAIT_SECONDS:=5}"       # пауза перед повторной проверкой служб
# Полная версия этих команд (вместе с ботом) ставится из другого репозитория
# под именем tgo-vps; эта версия её не заменяет.
: "${FULL_CMD:=tgo-vps}"
: "${FULL_SCRIPT:=/opt/TelegramOnly/scripts/tgo-vps.sh}"
CMD_NAME=rfm-vps            # имя установленной команды в CMD_DIR

RUST_MIN_MINOR=88          # Rust 1.88+
KEEP_BACKUPS=3             # сколько последних копий программы оставлять
BUILD_FREE_MB=768          # свободно на диске для сборки обновления FM, МБ
FRESH_FREE_MB=2048         # то же для первой сборки (Rust и зависимости)
BUILD_LOG="$BUILD_ROOT/rfm-build.log"

# Ответы на вопросы можно задать заранее переменными окружения:
# RFM_DOMAIN, HTTPS_MODE (certbot|cloudflare|none), LE_EMAIL, RFM_ADMIN_LOGIN,
# ENABLE_UFW (yes|no).
: "${RFM_DOMAIN:=}" "${HTTPS_MODE:=}" "${LE_EMAIL:=}" "${RFM_ADMIN_LOGIN:=}" "${ENABLE_UFW:=}"

CMD=''
ASSUME_YES=0
FORCE=0
CHECK_ONLY=0
ONLY=''                  # --only fm (у delete — без вопроса о компоненте)
BACKUP_DATA=0
RFM_REF=''
ADMIN_PASSWORD=''
ORIG_ARGS=()
INSTALL_FM=''
INSTALLED_NOW=0
FM_PORT=''               # порт программы на localhost для новой установки
FM_SOURCE=''             # откуда взят код файлового менеджера (GitHub, зеркало, bundle…)
DRY_RUN=0                # delete --dry-run: только показать
CONFIRM_HOST=''          # delete --confirm ИМЯ: подтверждение без вопроса
DEL_DIR=''               # копия этого запуска delete
DEL_PRESENT=()           # что нашлось на сервере
DEL_SELECTED=()          # что выбрано к удалению
REMOVED=()               # что удалено (для итога)
KEPT=()                  # что осталось на месте (другой диск)
FM_DOMAIN=''             # домен удалённого файлового менеджера
FM_BUILD=''              # его каталог сборки
LISTEN_BEFORE=''         # внешние порты до удаления (для ufw)
OTHERS_BEFORE=''         # другие службы сервера до начала команды (others_snapshot)
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
  IFS= read -r __ans || die "Нет ответа: $__q. Запустите в терминале или задайте ответы и --yes"
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
  IFS= read -r ans || return 1
  ans=${ans,,}
  [[ -z $ans ]] && ans=$def
  [[ $ans == y* || $ans == д* ]]
}

ask_password() {
  local p1 p2
  while :; do
    printf '    Придумайте пароль администратора сайта (от 8 символов): ' >&2
    IFS= read -r -s p1 || die "Пароль не задан: ввод закончился"; printf '\n' >&2
    printf '    Повторите пароль: ' >&2
    IFS= read -r -s p2 || die "Повтор пароля не задан: ввод закончился"; printf '\n' >&2
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

prune_backups() {  # каталог шаблон
  local dir=$1 pattern=$2 f n=0
  while IFS= read -r f; do
    n=$((n + 1))
    (( n > KEEP_BACKUPS )) && rm -rf -- "$f"
  done < <(find "$dir" -maxdepth 1 -name "$pattern" -printf '%T@ %p\n' 2>/dev/null | sort -rn | cut -d' ' -f2-)
}

http_code() { curl -sS -o /dev/null -w '%{http_code}' --max-time "${2:-10}" "$1" 2>/dev/null || true; }

# Порты, которые службы сервера слушают не только на loopback: строки
# «порт/tcp|udp процесс». VPN (Xray, Hysteria2, NaiveProxy, Mieru, MTProto…)
# должен остаться доступным, если deploy включает ufw. DHCP-клиент не нужен.
public_listeners() {
  ss -H -tulpn 2>/dev/null | awk '
    { addr = $5; port = $5; sub(/:[^:]*$/, "", addr); sub(/.*:/, "", port)
      if (port !~ /^[0-9]+$/ || addr ~ /^(127\.|\[::1\]|::1$|\[::ffff:127\.)/) next
      proto = ($1 ~ /^udp/) ? "udp" : "tcp"
      if (proto == "udp" && (port == 68 || port == 546)) next
      name = $7; sub(/^users:\(\("/, "", name); sub(/".*/, "", name)
      print port "/" proto, (name == "" ? "?" : name) }' | sort -u
}

# Имя процесса, который слушает TCP-порт на любом адресе; пусто — порт свободен.
port_owner() {
  local name
  name=$(ss -H -tlnp 2>/dev/null | awk -v p="$1" '
    { port = $4; sub(/.*:/, "", port)
      if (port == p) { name = $6; sub(/^users:\(\("/, "", name); sub(/".*/, "", name)
                       print (name == "" ? "?" : name); exit } }')
  printf '%s' "$name"
}

# Другие службы сервера: порты, кроме самого файлового менеджера.
# Список снимается до установки или обновления и сверяется после: VPN, координатор
# Headscale с DERP, tailscaled, Headplane и др. должны остаться на месте. Мосты
# socat тоже сверяются: через них бот достаёт службы хоста (BotCriptoM, «Файлы»).
others_snapshot() {
  {
    ss -H -tulpn 2>/dev/null | awk '
      { port = $5; sub(/.*:/, "", port); name = $7; sub(/^users:\(\("/, "", name); sub(/".*/, "", name)
        if (port !~ /^[0-9]+$/ || name == "rust-file-manag") next
        proto = ($1 ~ /^udp/) ? "udp" : "tcp"
        # Эфемерные порты исходящих сокетов меняются сами: для них только имя процесса.
        # Сокеты ядра (WireGuard) без процесса — всегда по порту.
        if (name == "") print port "/" proto " ?"
        else if (port + 0 < 32768) print port "/" proto " " name
        else print name }'
  } | LC_ALL=C sort -u
}

others_check() {
  local missing i
  [[ -n $OTHERS_BEFORE ]] || return 0
  # Пересозданному контейнеру и его портам нужно несколько секунд.
  for i in 1 2 3 4 5 6; do
    missing=$(LC_ALL=C comm -23 <(printf '%s\n' "$OTHERS_BEFORE") <(others_snapshot) | paste -sd ';' - | sed 's/;/; /g')
    [[ -z $missing ]] && break
    (( i < 6 )) && sleep "$WAIT_SECONDS"
  done
  if [[ -z $missing ]]; then
    ok "другие службы сервера на месте (порты: $(grep -c . <<<"$OTHERS_BEFORE"))"
    SUMMARY+=("Другие службы сервера (VPN, Tailscale и др.): на месте")
    return 0
  fi
  fail "за время работы команды пропало: $missing"
  SUMMARY+=("Другие службы: ПРОПАЛО — $missing")
  TODO+=("Проверьте пропавшие службы: systemctl --failed; journalctl -p err -b")
  return 1
}

# --- Что стоит на сервере ----------------------------------------------------
unit_exists() { systemctl cat "$1" >/dev/null 2>&1; }
fm_present() { [[ -x $RFM_BIN ]] || unit_exists "$RFM_UNIT"; }

fm_start_line() {  # последняя строка запуска (с начала журнала или с @epoch)
  local from=()
  [[ -n ${1:-} ]] && from=(--since "@$1")
  journalctl -u "$RFM_UNIT" --no-pager -o cat "${from[@]}" 2>/dev/null | strip_ansi | grep 'starting server' | tail -1
}
line_field() { grep -o "$1=\"[^\"]*\"" | head -1 | cut -d'"' -f2; }
fm_version() { fm_start_line | line_field version; }
fm_commit()  { fm_start_line | line_field commit; }

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

print_status() {
  step "Что стоит на сервере"
  if fm_present; then
    say "Файловый менеджер: установлен, версия ${C_B}$(fm_version || true)${C_0} (коммит $(fm_commit || true)), служба $(systemctl is-active "$RFM_UNIT" 2>/dev/null || true)"
    say "  сайт: $(env_get "$RFM_ENV" PUBLIC_BASE_URL)  мини-приложение: $(env_get "$RFM_ENV" MINIAPP_ENABLED)  API для бота: $(env_get "$RFM_ENV" INTERNAL_BIND_ADDR)"
    say "  каталог сборки: $(fm_build_dir)"
  else
    say "Файловый менеджер: не установлен"
  fi
  # Подсказка только тем, у кого на сервере есть клон бота с полной версией
  # (или его прежний клон, где её ещё нет: переход с общих команд).
  local clone=${FULL_SCRIPT%/scripts/*}
  if [[ -f $FULL_SCRIPT ]]; then
    say "Эта команда обслуживает только файловый менеджер. Полная версия (вместе с ботом):"
    say "  bash $FULL_SCRIPT setup"
  elif [[ $clone != "$FULL_SCRIPT" && -d $clone/.git ]]; then
    say "Эта команда обслуживает только файловый менеджер. Полная версия (вместе с ботом):"
    say "  git -C $clone pull --ff-only && bash $FULL_SCRIPT setup"
  fi
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
  local pkgs=(git curl ca-certificates openssl build-essential pkg-config)
  # nginx занимает порт 80 сразу после установки: только если HTTPS через него.
  [[ $HTTPS_MODE == certbot || $HTTPS_MODE == cloudflare ]] && pkgs+=(nginx)
  [[ $HTTPS_MODE == certbot ]] && pkgs+=(certbot python3-certbot-nginx)
  [[ $ENABLE_UFW == yes ]] && pkgs+=(ufw)
  step "Системные пакеты"
  DEBIAN_FRONTEND=noninteractive apt-get update -q >/dev/null || die "apt-get update не прошёл"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${pkgs[@]}" >/dev/null || die "Не удалось поставить пакеты: ${pkgs[*]}"
  ok "${pkgs[*]}"
}

mem_mb() { awk '/MemTotal/ {print int($2/1024)}' "$MEMINFO"; }
# Нет swap, а памяти мало: сборка Rust может вызвать OOM и задеть VPN-службы.
swap_needed() { [[ -z $(swapon --show 2>/dev/null) ]] && (( $(mem_mb) < 3500 )); }

setup_swap() {  # [сколько МБ оставить на диске для сборки, по умолчанию 1024]
  local avail_mb need=$((2048 + ${1:-1024}))
  [[ -n $(swapon --show 2>/dev/null) ]] && { ok "swap уже есть"; return; }
  swap_needed || return 0
  if [[ -e $SWAP_FILE ]]; then
    warn "$SWAP_FILE уже есть, но swap не включён — не трогаю его; сборке может не хватить памяти"
    return 0
  fi
  # Заполненный диск остановил бы Docker, журналы и VPN-службы.
  avail_mb=$(df -Pm "$(dirname "$SWAP_FILE")" 2>/dev/null | awk 'NR == 2 {print $4}')
  if [[ ! $avail_mb =~ ^[0-9]+$ ]] || (( avail_mb < need )); then
    warn "Свободно меньше $need МБ на диске — swap не создаю; сборке может не хватить памяти"
    return 0
  fi
  say "Памяти $(mem_mb) МБ — создаю swap 2 ГБ, без него сборка Rust может упасть"
  if { fallocate -l 2G "$SWAP_FILE" 2>/dev/null || dd if=/dev/zero of="$SWAP_FILE" bs=1M count=2048 status=none; } &&
    chmod 600 "$SWAP_FILE" && mkswap -q "$SWAP_FILE" >/dev/null && swapon "$SWAP_FILE"; then
    grep -q "^$SWAP_FILE " "$FSTAB" || echo "$SWAP_FILE none swap sw 0 0" >>"$FSTAB"
    ok "swap 2 ГБ"
  else
    swapoff "$SWAP_FILE" 2>/dev/null
    rm -f -- "$SWAP_FILE"
    warn "swap создать не удалось — недописанный файл удалён"
  fi
}

# Свободно МБ на файловой системе каталога (или ближайшего существующего родителя).
free_mb() {
  local d=$1
  while [[ ! -e $d && $d == /* && $d != / ]]; do d=$(dirname "$d"); done
  df -Pm "$d" 2>/dev/null | awk 'NR == 2 {print $4}'
}

# Сборка не должна заполнить диск: на полном диске останавливаются Docker,
# журналы, база координатора Headscale и VPN-службы. Размер неизвестен — не мешаем.
disk_ok() {  # каталог нужно_МБ что
  local avail
  avail=$(free_mb "$1")
  [[ $avail =~ ^[0-9]+$ ]] || return 0
  (( avail >= $2 )) && return 0
  fail "$3: на диске свободно $avail МБ, нужно не меньше $2 МБ — не начинаю, чтобы не заполнить диск"
  say "Освободить место: journalctl --vacuum-size=100M; apt-get clean"
  return 1
}

setup_firewall() {
  local port web=0 https_port=443 spec
  local -a kept=()
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
  # Уже работающие службы (VPN и др.) не должны оказаться за новым файрволом.
  while read -r spec _; do
    ufw allow "$spec" >/dev/null || { warn "ufw: не удалось разрешить $spec — файрвол не включаю"; return; }
    kept+=("$spec")
  done < <(public_listeners)
  ufw --force enable >/dev/null && ok "ufw включён: SSH $port$( (( web )) && echo ", 80, $https_port")" ||
    { warn "ufw включить не удалось"; return; }
  (( ${#kept[@]} )) && ok "ufw: сохранён доступ к работающим службам: ${kept[*]}"
  return 0
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

# --- Источники кода ------------------------------------------------------------
# Основной (GitHub) всегда первый. Запасные — свои зеркала
# и bundle с локальной машины — нужны, только когда основной не ответил.
# Каждая строка вывода — «метка адрес».
code_sources() {  # bot|fm
  local kind=$1 repo url m f
  if [[ $kind == bot ]]; then
    repo=TelegramOnly
    url=$(git -C "$BOT_DIR" remote get-url origin 2>/dev/null) || url=${BOT_REPO_URL:-}
  else
    repo=rust-file-manager
    url=$RFM_REPO_URL
  fi
  if [[ -n $url ]]; then
    if [[ $url == *github.com* ]]; then echo "GitHub $url"; else echo "основной $url"; fi
  fi
  if [[ $kind == fm ]]; then for m in $RFM_MIRRORS; do echo "зеркало $m"; done; fi
  f=$BUNDLE_DIR/$repo.bundle
  if [[ -f $f ]]; then
    if bundle_trusted "$f"; then echo "bundle $f"
    else warn "$f не используется: файл или каталог чужой либо открыт на запись другим (нужно root, 0600/0700)"; fi
  fi
  return 0
}

primary_src() { [[ $1 == GitHub || $1 == основной ]]; }
source_labels() { code_sources "$1" | cut -d' ' -f1 | paste -sd, - | sed 's/,/, /g'; }

# Код из bundle выполняется от root: файл и каталог должны принадлежать нам и не
# быть доступны на запись другим, иначе любой пользователь сервера подложит свой код.
bundle_trusted() {
  local p uid mode
  for p in "$1" "$(dirname "$1")"; do
    read -r uid mode < <(stat -c '%u %a' "$p" 2>/dev/null) || return 1
    [[ $uid == "$EUID" ]] && (( (8#$mode & 8#022) == 0 )) || return 1
  done
}

# git без вопросов о пароле, с ограничением по времени; ключ хоста SSH проверяется
# по known_hosts как обычно.
src_git() {  # метка аргументы-git…
  shift
  GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND=${GIT_SSH_COMMAND:-ssh -o BatchMode=yes -o ConnectTimeout=10} \
    timeout "$GIT_TIMEOUT" git "$@"
}

# Как src_git, но адрес, который уже не ответил в этом запуске, не трогаем:
# недоступный GitHub не должен стоить таймаут на каждом шаге.
src_try() {  # метка адрес аргументы-git…
  local label=$1 url=$2
  shift 2
  [[ " $VPS_SRC_DOWN " == *" $url "* ]] && return 1
  src_git "$label" "$@" 2>/dev/null && return 0
  [[ $label == bundle ]] || VPS_SRC_DOWN+=" $url"
  return 1
}

# Версия a новее версии b (вида 1.2.3).
version_gt() { [[ -n $1 && $1 != "$2" && $(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1) == "$1" ]]; }

drop_refs() {  # каталог префикс — временные ссылки на полученные версии
  local r
  git -C "$1" for-each-ref --format='%(refname)' "$2" 2>/dev/null |
    while read -r r; do git -C "$1" update-ref -d "$r"; done
}

# --- Файловый менеджер -------------------------------------------------------
fm_checkout() {  # каталог ref
  local dir=$1 ref=$2 label url ver n=0 best='' best_ver='' best_label='' existed=0
  local -a depth
  FM_SOURCE=''
  if [[ ! -d $dir/.git ]]; then
    if [[ -e $dir ]]; then
      [[ -z $(ls -A "$dir" 2>/dev/null) ]] || { fail "$dir не пустой и это не клон git — разберите его вручную"; return 1; }
      existed=1
    fi
    while read -r label url; do
      # -b main: bundle клонируется только с явной веткой.
      if GIT_TIMEOUT=$GIT_CLONE_TIMEOUT src_try "$label" "$url" clone -q --depth 1 -b main "$url" "$dir"; then
        git -C "$dir" remote set-url origin "$RFM_REPO_URL"  # в следующий раз снова сначала основной
        break
      fi
      # Остатки прерванного clone; каталог был пуст или его не было.
      if (( existed )); then find "$dir" -mindepth 1 -delete 2>/dev/null; else rm -rf -- "$dir"; fi
    done < <(code_sources fm)
    [[ -d $dir/.git ]] || { fail "Код rust-file-manager не получен ни из одного источника ($(source_labels fm))"; return 1; }
  fi
  if [[ -n $(git -C "$dir" status --porcelain --untracked-files=no 2>/dev/null) ]]; then
    fail "В $dir изменены файлы — не трогаю. Сохраните правки или удалите каталог"
    return 1
  fi
  while read -r label url; do
    n=$((n + 1))
    depth=(--depth 1); [[ $label == bundle ]] && depth=()  # --depth из bundle в пустой клон падает
    src_try "$label" "$url" -C "$dir" fetch -q "${depth[@]}" "$url" "+$ref:refs/rfm-vps/src$n" || continue
    if primary_src "$label"; then best=refs/rfm-vps/src$n; best_label=$label; break; fi
    # Запасные: самая новая версия; при равных — источник раньше по списку.
    ver=$(git -C "$dir" show "refs/rfm-vps/src$n:Cargo.toml" 2>/dev/null | grep -m1 '^version' | cut -d'"' -f2)
    if [[ -z $best ]] || version_gt "$ver" "$best_ver"; then best=refs/rfm-vps/src$n; best_ver=$ver; best_label=$label; fi
  done < <(code_sources fm)
  if [[ -z $best ]]; then
    drop_refs "$dir" refs/rfm-vps/
    fail "Не удалось получить $ref ни из одного источника ($(source_labels fm))"
    return 1
  fi
  git -C "$dir" checkout -q --detach "$best" || { drop_refs "$dir" refs/rfm-vps/; fail "Не удалось переключиться на $ref"; return 1; }
  drop_refs "$dir" refs/rfm-vps/
  FM_SOURCE=$best_label
  primary_src "$FM_SOURCE" || say "код rust-file-manager взят из источника «$FM_SOURCE»"
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
# Порт программы на localhost: из её настроек, а до установки — выбранный свободный.
fm_port() {
  local bind
  bind=$(env_get "$RFM_ENV" BIND_ADDR)
  if [[ $bind =~ :([0-9]+)$ ]]; then echo "${BASH_REMATCH[1]}"; else echo "${FM_PORT:-8080}"; fi
}

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
  if [[ -n $expect && ( -z $commit || $expect != "$commit"* ) ]]; then
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
    echo "BIND_ADDR=127.0.0.1:${FM_PORT:-8080}"
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

fm_nginx_uploads() {
  local site enabled saved='' found=0 rc=0 restore_rc=0 i copy
  local -a sites=() copies=()
  [[ ${HTTPS_MODE:-$(state_get HTTPS_MODE)} != none ]] || return 0
  # Only our enabled sites; keep certificates and all other proxy settings.
  for site in "$NGINX_DIR/sites-available/rust-file-manager" "$NGINX_DIR/sites-available/rust-file-manager-ssl"; do
    enabled=$NGINX_DIR/sites-enabled/${site##*/}
    [[ -f $site && $site -ef $enabled ]] || continue
    grep -qE '^[[:space:]]*location[[:space:]]+/api/v1/uploads/?[[:space:]]*\{' "$site" || continue
    found=1
    if grep -qE '^[[:space:]]*location[[:space:]]+/api/v1/uploads/[[:space:]]*\{' "$site"; then
      [[ -n $saved ]] || saved=$(mktemp -d "$NGINX_DIR/.rfm-uploads.XXXXXX") || return 1
      copy=$saved/${site##*/}
      cp -p "$site" "$copy" || { rm -f -- "$copy"; rc=1; break; }
      copies+=("$copy"); sites+=("$site")
      # A slash here makes nginx redirect /uploads to an API route that did not exist.
      sed -i -E 's@^([[:space:]]*location[[:space:]]+)/api/v1/uploads/([[:space:]]*\{)@\1/api/v1/uploads\2@' "$site" || { rc=1; break; }
    fi
  done
  (( found )) || return 0
  # Reload even after a manual edit: changing a file alone does not update nginx.
  if (( ! rc )); then
    nginx -t >/dev/null 2>&1 && systemctl reload nginx || rc=1
  fi
  if (( rc )); then
    for i in "${!sites[@]}"; do
      cp -p "${copies[$i]}" "${sites[$i]}" || restore_rc=1
    done
    nginx -t >/dev/null 2>&1 && systemctl reload nginx >/dev/null 2>&1 || restore_rc=1
    if (( restore_rc )); then
      fail "nginx: исправление загрузок не применено; проверьте конфигурацию и службу nginx"
    else
      fail "nginx: исправление загрузок не применено; прежняя конфигурация восстановлена"
    fi
  else
    ok "nginx: маршрут загрузок проверен, конфигурация применена"
  fi
  if [[ -n $saved ]]; then
    rm -f -- "${copies[@]}"; rmdir "$saved"
  fi
  return "$rc"
}

fm_nginx() {
  local site_dir=$NGINX_DIR/sites-available en_dir=$NGINX_DIR/sites-enabled domain=$RFM_DOMAIN example
  [[ -n $domain ]] || domain=$(env_get "$RFM_ENV" PUBLIC_BASE_URL | sed 's#^https\?://##; s#/.*##')
  [[ -n $HTTPS_MODE ]] || return 0
  [[ -n $domain ]] || { warn "Домен неизвестен — nginx не настраиваю"; return 0; }
  case $HTTPS_MODE in
    none)
      say "HTTPS обеспечивает ваш прокси — nginx не трогаю"
      TODO+=("Направьте https://$domain на http://127.0.0.1:$(fm_port) в своём прокси")
      return 0 ;;
  esac
  if grep -RqsE "server_name[^;]*[[:space:]]${domain}[[:space:];]" "$en_dir"/ 2>/dev/null; then
    fm_nginx_uploads || return 1
    ok "nginx уже обслуживает $domain — настройки HTTPS сохранены"
  elif [[ $HTTPS_MODE == certbot ]]; then
    example=$(fm_build_dir)/deploy/nginx.example.conf
    sed -e "s/server_name example\.com;/server_name $domain;/" \
        -e "s#127\.0\.0\.1:8080#127.0.0.1:$(fm_port)#g" \
        -e 's/client_max_body_size 200M;/client_max_body_size 201M;/' "$example" >"$site_dir/rust-file-manager" &&
      ln -sf "$site_dir/rust-file-manager" "$en_dir/rust-file-manager" ||
      { fail "не удалось записать конфигурацию nginx"; return 1; }
    ok "nginx: $site_dir/rust-file-manager"
  else
    if [[ ! -s $NGINX_DIR/ssl/$domain/cert.pem || ! -s $NGINX_DIR/ssl/$domain/key.pem ]]; then
      warn "Нет сертификата Cloudflare в $NGINX_DIR/ssl/$domain/ (cert.pem, key.pem)"
      TODO+=("Положите Origin-сертификат Cloudflare в $NGINX_DIR/ssl/$domain/ (DEPLOY.md, раздел «HTTPS через Cloudflare») и запустите deploy ещё раз")
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
  local port
  port=$(fm_port)
  cat <<EOF
server {
    listen 2083 ssl;
    server_name $1;
    ssl_certificate     $NGINX_DIR/ssl/$1/cert.pem;
    ssl_certificate_key $NGINX_DIR/ssl/$1/key.pem;
    client_max_body_size 201M;

    # Мини-приложение: части загрузки идут сразу в приложение, без буфера на диске
    # Без слеша: GET/POST коллекции /uploads должны идти в API без редиректа.
    location /api/v1/uploads {
        client_max_body_size 10M;
        proxy_request_buffering off;
        proxy_pass http://127.0.0.1:$port;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    # В пути подписанной ссылки лежит токен: не писать её в журнал
    location /d/ {
        access_log off;
        proxy_pass http://127.0.0.1:$port;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_read_timeout 300;
    }

    location / {
        proxy_pass http://127.0.0.1:$port;
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
  # Запасной источник мог отстать от GitHub: версию старее работающей не ставим.
  if ! primary_src "$FM_SOURCE" && [[ -z $RFM_REF ]] &&
     version_gt "$(fm_version)" "$(grep -m1 '^version' "$dir/Cargo.toml" | cut -d'"' -f2)"; then
    warn "в источнике «$FM_SOURCE» версия старее работающей — файловый менеджер не трогаю"
    SUMMARY+=("Файловый менеджер: без изменений — источник «$FM_SOURCE» отстаёт, работает $(fm_version)")
    return 0
  fi
  if [[ -n $running && $head == "$running"* ]] && (( ! FORCE )) && systemctl is-active --quiet "$RFM_UNIT"; then
    ok "уже последняя версия"
    SUMMARY+=("Файловый менеджер: без изменений, $(fm_version)")
    return 0
  fi
  disk_ok "$dir" "$BUILD_FREE_MB" "Сборка файлового менеджера" ||
    { SUMMARY+=("Файловый менеджер: не обновлён — мало места на диске"); return 1; }
  # Как deploy: на маленьком VPS без swap сборка может вызвать OOM у VPN-служб.
  setup_swap
  fm_build "$dir" || return 1
  if cmp -s "$dir/target/release/rust-file-manager" "$RFM_BIN" && (( ! FORCE )) && systemctl is-active --quiet "$RFM_UNIT"; then
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
    systemctl stop "$RFM_UNIT" || { fail "Не удалось остановить службу для согласованной копии данных"; return 1; }
    if ! tar --acls --xattrs -cpf "$RFM_BACKUPS/data-$old_ver-$(ts).tar" -C "$RFM_DATA" .; then
      systemctl start "$RFM_UNIT" || fail "Прежнюю службу не удалось запустить"
      fail "Архив данных не создан — программу не заменяю"
      return 1
    fi
    systemctl start "$RFM_UNIT" || { fail "Прежняя служба не запустилась после архивации"; return 1; }
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
  fm_install_unit "$dir"
  fm_restart_checked "$head" || die "Файловый менеджер не запустился: journalctl -u $RFM_UNIT -n 50"
  state_set RFM_BUILD_DIR "$dir"
  state_set RFM_REF "$ref"
  SUMMARY+=("Файловый менеджер: установлен $(fm_version), https://$RFM_DOMAIN")
}


# --- delete ------------------------------------------------------------------
# Спрашивает по очереди, что убрать, показывает план и выполняет только после
# ввода имени сервера. Файлы не стираются, а переносятся в копию
# $DELETE_BACKUPS/<дата>-delete-…: вернуть можно обратным mv. Пересобираемое
# (каталог сборки, Rust, образы Docker) и общее для нескольких служб удаляется
# только по отдельному вопросу в конце.
in_list() { [[ " ${DEL_SELECTED[*]} " == *" $1 "* ]]; }

del_title() {
  case $1 in
    fm) echo "Файловый менеджер rust-file-manager (программа, данные, сайт nginx)" ;;
  esac
}

del_init() {
  [[ -n $DEL_DIR ]] && return 0
  [[ -L $DELETE_BACKUPS ]] && die "$DELETE_BACKUPS — ссылка; копию туда не кладу"
  install -d -m 0700 "$DELETE_BACKUPS" || die "Не удалось создать $DELETE_BACKUPS"
  DEL_DIR=$(mktemp -d "$DELETE_BACKUPS/$(date +%Y%m%d-%H%M%S)-delete-XXXX") || die "Не удалось создать копию в $DELETE_BACKUPS"
}

# Перенести путь в копию запуска (mv, не rm -rf). Системные каталоги не трогаем
# никогда; отдельный диск или точку монтирования оставляем на месте.
del_archive() {  # путь
  local src=$1 dst
  [[ -e $src || -L $src ]] || return 0
  case ${src%/} in
    ''|/|/etc|/var|/var/lib|/var/backups|/usr|/usr/local|/usr/local/bin|/opt|/root|/home|/boot|/srv|/tmp)
      fail "отказ: $src — системный каталог"; return 1 ;;
  esac
  dst=$DEL_DIR/files$src
  mkdir -p -m 0700 "$(dirname "$dst")" || return 1
  if mountpoint -q "$src" 2>/dev/null || [[ $(stat -c %d "$src") != "$(stat -c %d "$(dirname "$dst")")" ]]; then
    warn "$src на другом диске — оставляю на месте; удалите вручную, когда он не нужен"
    KEPT+=("$src")
    return 0
  fi
  mv -T -- "$src" "$dst"
}

fm_domain() {  # домен из PUBLIC_BASE_URL
  local u
  u=$(env_get "$RFM_ENV" PUBLIC_BASE_URL); u=${u#*://}
  echo "${u%%[/:]*}"
}

fm_delete() {
  local site env_dir removed_site=0
  step "Файловый менеджер: удаление"
  FM_DOMAIN=$(fm_domain)
  FM_BUILD=$(fm_build_dir)
  systemctl disable --now "$RFM_UNIT" >/dev/null 2>&1 || true
  for site in rust-file-manager rust-file-manager-ssl; do
    if [[ -e $NGINX_DIR/sites-enabled/$site || -L $NGINX_DIR/sites-enabled/$site ]]; then
      rm -f -- "$NGINX_DIR/sites-enabled/$site"; removed_site=1
    fi
    del_archive "$NGINX_DIR/sites-available/$site" || return 1
  done
  if (( removed_site )) && has nginx; then
    if nginx -t >/dev/null 2>&1; then systemctl reload nginx >/dev/null 2>&1 || warn "nginx не перечитал настройки"
    else warn "nginx -t не прошёл после удаления сайта — проверьте конфигурацию nginx"; fi
  fi
  # Сертификат Cloudflare Origin сам не перевыпускается — в копию.
  if [[ -n $FM_DOMAIN ]]; then del_archive "$NGINX_DIR/ssl/$FM_DOMAIN" || return 1; fi
  env_dir=$(dirname "$RFM_ENV")
  [[ ${env_dir##*/} == rust-file-manager ]] || env_dir=$RFM_ENV
  { del_archive "$UNIT_DIR/$RFM_UNIT" && del_archive "$UNIT_DIR/$RFM_UNIT.d" && del_archive "$RFM_BIN" &&
    del_archive "$env_dir" && del_archive "$RFM_DATA" && del_archive "$STATE_FILE"; } ||
    { fail "не удалось перенести файлы файлового менеджера в копию"; return 1; }
  systemctl daemon-reload >/dev/null 2>&1
  if [[ $FM_USER != root ]] && id "$FM_USER" >/dev/null 2>&1; then
    userdel "$FM_USER" >/dev/null 2>&1 || warn "пользователь $FM_USER не удалён"
  fi
  ok "файловый менеджер удалён; данные — в $DEL_DIR/files$RFM_DATA"
  REMOVED+=("Файловый менеджер: программа, настройки и данные в копии")
}

delete_build_leftovers() {
  local d rustup rust=0
  local -a dirs=()
  for d in "$FM_BUILD" "$BUILD_ROOT"/rfm-build "$BUILD_ROOT"/rfm-build-*; do
    # Только каталоги сборки этих команд (rfm-build*), а не любой git-клон.
    [[ -n $d && ${d##*/} == rfm-build* && -d $d/.git && " ${dirs[*]} " != *" $d "* ]] && dirs+=("$d")
  done
  rustup=$(dirname "$CARGO_ENV")/bin/rustup
  [[ -x $rustup ]] && rust=1
  (( ${#dirs[@]} || rust )) || return 0
  confirm "Удалить$( (( ${#dirs[@]} )) && echo " каталог сборки (${dirs[*]})")$( (( ${#dirs[@]} && rust )) && echo " и")$( (( rust )) && echo " Rust") — нужны только для сборки файлового менеджера и при новой установке появятся снова?" y || return 0
  (( ${#dirs[@]} )) && rm -rf -- "${dirs[@]}"
  rm -f -- "$BUILD_LOG"
  if (( rust )); then "$rustup" self uninstall -y >/dev/null 2>&1 || warn "Rust удалён не полностью"; fi
  ok "удалено:$( (( ${#dirs[@]} )) && echo " ${dirs[*]}")$( (( rust )) && echo " Rust")"
}

delete_swap() {
  local used avail
  [[ -f $SWAP_FILE ]] || return 0
  confirm "Удалить swap $SWAP_FILE? Он нужен был для сборки; на маленьком VPS пригодится и другим службам" n || return 0
  if swapon --show=NAME --noheadings 2>/dev/null | grep -qxF "$SWAP_FILE"; then
    used=$(swapon --show=NAME,USED --bytes --noheadings 2>/dev/null | awk -v f="$SWAP_FILE" '$1 == f {print int($2 / 1048576)}')
    avail=$(awk '/MemAvailable/ {print int($2 / 1024)}' "$MEMINFO")
    if (( ${used:-0} + 200 > ${avail:-0} )); then
      warn "в swap $used МБ, а свободной памяти $avail МБ — swap не выключаю"; return 0
    fi
    swapoff "$SWAP_FILE" || { warn "swapoff не прошёл"; return 0; }
  fi
  sed -i "\#^$SWAP_FILE[[:space:]]#d" "$FSTAB" && rm -f -- "$SWAP_FILE" && ok "swap $SWAP_FILE удалён"
}

delete_nginx_if_unused() {
  local f p
  local -a pkgs=()
  has nginx || return 0
  for f in "$NGINX_DIR"/sites-enabled/* "$NGINX_DIR"/conf.d/*.conf; do
    [[ -e $f && ${f##*/} != default ]] && return 0
  done
  for p in nginx nginx-common nginx-core nginx-full libnginx-mod-stream; do
    dpkg -s "$p" >/dev/null 2>&1 && pkgs+=("$p")
  done
  # certbot — только если сертификатов больше нет: их продлевает он.
  if ! compgen -G "$LE_DIR/live/*/cert.pem" >/dev/null; then
    for p in certbot python3-certbot-nginx; do dpkg -s "$p" >/dev/null 2>&1 && pkgs+=("$p"); done
  fi
  (( ${#pkgs[@]} )) || return 0
  confirm "nginx больше не обслуживает ни одного сайта — удалить ${pkgs[*]}?" n || return 0
  DEBIAN_FRONTEND=noninteractive apt-get purge -y -q "${pkgs[@]}" >/dev/null && ok "удалено: ${pkgs[*]}" ||
    warn "пакеты удалены не полностью"
}

# Порты, которые слушали удалённые службы: закрыть в ufw, если их больше никто не слушает.
delete_ufw_ports() {
  local spec
  local -a rules=()
  has ufw && ufw status 2>/dev/null | grep -q 'Status: active' || return 0
  for spec in $(comm -23 <(awk 'NF {print $1}' <<<"$LISTEN_BEFORE" | sort -u) <(public_listeners | awk '{print $1}' | sort -u)); do
    ufw status 2>/dev/null | grep -qE "^${spec}[[:space:]]" && rules+=("$spec")
  done
  (( ${#rules[@]} )) || return 0
  confirm "Закрыть в ufw порты удалённых служб: ${rules[*]}?" y || return 0
  for spec in "${rules[@]}"; do
    ufw delete allow "$spec" >/dev/null 2>&1 && ok "ufw: $spec закрыт" || warn "ufw: $spec не закрыт"
  done
}

delete_commands_if_unused() {
  local n target
  fm_present && return 0
  confirm "На сервере не осталось ни бота, ни файлового менеджера — удалить и сами команды deploy, post_deploy и delete?" n || return 0
  for n in deploy post_deploy post-deploy delete; do
    target=$(readlink "$CMD_DIR/$n" 2>/dev/null)
    [[ -L $CMD_DIR/$n && $target == "$CMD_NAME" ]] && rm -f -- "$CMD_DIR/$n"
  done
  rm -f -- "$CMD_DIR/$CMD_NAME" "$CMD_DIR/$CMD_NAME".backup.*
  ok "команды удалены"
}

delete_shared() {
  step "Общее для нескольких служб"
  if in_list fm; then
    if [[ -n $FM_DOMAIN && -d $LE_DIR/live/$FM_DOMAIN ]] && has certbot &&
       confirm "Удалить сертификат Let's Encrypt для $FM_DOMAIN? При новой установке он выпустится снова" y; then
      certbot delete --cert-name "$FM_DOMAIN" --non-interactive >/dev/null 2>&1 && ok "сертификат $FM_DOMAIN удалён" ||
        warn "certbot delete не прошёл"
    fi
    delete_build_leftovers
    delete_swap
  fi
  delete_nginx_if_unused
  # Последним: после nginx и Docker могли освободиться и их порты.
  delete_ufw_ports
  delete_commands_if_unused
  say "Остальное общее (sysctl, пакеты, SSH) команда не трогает"
}

delete_confirm_host() {
  local host=$1 ans
  if [[ -n $CONFIRM_HOST ]]; then
    [[ $CONFIRM_HOST == "$host" ]] || die "--confirm: имя этого сервера — $host. Ничего не удалено"
    return 0
  fi
  printf '    Для подтверждения введите имя сервера (%s): ' "$host" >&2
  IFS= read -r ans || die "Отменено — ничего не удалено"
  [[ $ans == "$host" ]] || die "Имя не совпало — ничего не удалено"
}

delete_summary() {
  local r
  for r in "${REMOVED[@]}"; do SUMMARY+=("Удалено: $r"); done
  if [[ -n $DEL_DIR ]]; then
    if [[ -z $(find "$DEL_DIR" -mindepth 1 -print -quit 2>/dev/null) ]]; then rmdir "$DEL_DIR" 2>/dev/null
    else SUMMARY+=("Копия: $DEL_DIR (вернуть — mv обратно из files/)"); TODO+=("Когда копия не нужна: rm -rf $DEL_DIR"); fi
  fi
  (( ${#KEPT[@]} )) && SUMMARY+=("Оставлено на месте (другой диск): ${KEPT[*]}")
  return 0
}

cmd_delete() {
  local rc=0
  [[ $EUID -eq 0 ]] || die "Запустите от root: sudo -i, затем команду ещё раз"
  (( ASSUME_YES )) && die "delete не принимает --yes: подтвердите именем сервера — в ответ на вопрос или --confirm ИМЯ"
  [[ -z $ONLY || $ONLY == fm ]] || die "Эта версия удаляет только файловый менеджер (--only fm)"
  if (( ! DRY_RUN )); then self_update; handover_to_full; take_lock; fi
  print_status
  step "Что можно удалить"
  if ! fm_present; then say "Файловый менеджер не установлен — удалять нечего"; return 0; fi
  say "• $(del_title fm)"
  say "Не трогаю: другие программы сервера (VPN, Docker, mesh-сеть), SSH"
  if [[ -d ${FULL_SCRIPT%/scripts/*}/.git ]]; then
    warn "На сервере есть бот: его связку с файловым менеджером убирает полная версия команд ($FULL_CMD delete)"
  fi
  if [[ -z $ONLY ]] && (( ! DRY_RUN )); then
    step "Что удалить (Enter — оставить)"
    confirm "Удалить: $(del_title fm)?" n || { say "Ничего не выбрано — ничего не удалено"; return 0; }
  fi
  DEL_SELECTED=(fm)
  step "План удаления"
  say "• файловый менеджер: остановить, убрать сайт nginx, перенести программу, настройки и данные ($(du -sh "$RFM_DATA" 2>/dev/null | cut -f1)) в копию"
  say "Файлы переносятся в $DELETE_BACKUPS — вернуть можно, удалить насовсем — когда убедитесь, что не нужны"
  say "Потом команда предложит убрать то общее, что стало ненужным (сертификат, Rust, swap, порты в ufw…)"
  if (( DRY_RUN )); then say "Это просмотр (--dry-run): ничего не изменено"; return 0; fi
  delete_confirm_host "$(hostname)"
  LISTEN_BEFORE=$(public_listeners)
  del_init
  fm_delete || rc=1
  (( rc )) || delete_shared
  delete_summary
  print_summary
  return "$rc"
}

# Команда delete появилась позже deploy/post_deploy, а самообновление ссылок не
# создаёт: добавляем ссылку сами, если запущена установленная команда и имя свободно.
ensure_delete_link() {
  [[ $(readlink -f "$0" 2>/dev/null) == "$(readlink -f "$CMD_DIR/$CMD_NAME" 2>/dev/null)" ]] || return 0
  [[ -e $CMD_DIR/delete || -L $CMD_DIR/delete ]] && return 0
  ln -s "$CMD_NAME" "$CMD_DIR/delete" 2>/dev/null &&
    say "Добавлена команда delete — удаление по выбору с копией (подробно: $CMD_NAME --help)"
}

# --- Самообновление команд ---------------------------------------------------
install_command() {  # скачанный файл целевой_файл
  local src=$1 target=$2 backup next
  if [[ -f $target ]]; then
    backup=$(mktemp "$CMD_DIR/rfm-vps.backup.XXXXXX") || return 1
    cp -p "$target" "$backup" || return 1
  fi
  next=$(mktemp "$CMD_DIR/rfm-vps.next.XXXXXX") || return 1
  install -m 0755 "$src" "$next" && mv -fT "$next" "$target"
}

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
  install_command "$tmp" "$cur" || { rm -f "$tmp"; die "Не удалось сохранить и обновить команды"; }
  rm -f "$tmp"
  say "Команды deploy/post_deploy обновлены ($url) — перезапускаю"
  RFM_VPS_REEXEC=1 exec "$cur" "$CMD" "${ORIG_ARGS[@]}"
}

# Сервер с клоном бота автора: команды сами переходят на полную версию
# (бот и файловый менеджер) из этого клона, как только она там есть. Порядок
# слияния репозиториев и ручной setup тогда не важны, бот не остаётся без
# обновлений. Без клона (обычный пользователь) ничего не происходит.
handover_to_full() {
  local clone=${FULL_SCRIPT%/scripts/*} rel tmp
  [[ ${RFM_VPS_HANDOVER:-} == 1 || $clone == "$FULL_SCRIPT" || ! -d $clone/.git ]] && return 0
  rel=${FULL_SCRIPT#"$clone"/}
  tmp=$(mktemp) || return 0
  if [[ -f $FULL_SCRIPT ]]; then
    cp "$FULL_SCRIPT" "$tmp"
  else
    # Приватный репозиторий: без запроса пароля; нет доступа — последняя полученная версия.
    GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND=${GIT_SSH_COMMAND:-ssh -o BatchMode=yes} \
      timeout 30 git -C "$clone" fetch -q origin main 2>/dev/null
    git -C "$clone" show "origin/main:$rel" >"$tmp" 2>/dev/null || : >"$tmp"
  fi
  if [[ $CMD == delete ]] && grep -q "$FULL_CMD" "$tmp" && ! grep -q 'cmd_delete' "$tmp"; then
    rm -f "$tmp"
    warn "Полная версия команд ($FULL_CMD) пока без delete — удаляю только файловый менеджер"
    TODO+=("Уберите FILES_* из .env бота и перезапустите его: файлового менеджера больше нет")
    return 0
  fi
  if ! grep -q "$FULL_CMD" "$tmp" || ! bash -n "$tmp" 2>/dev/null; then
    rm -f "$tmp"
    warn "На сервере есть бот, а полной версии команд ($FULL_CMD) в его репозитории пока нет — обновляю только файловый менеджер"
    SUMMARY+=("Бот: не обновлён — эта версия команд обслуживает только файловый менеджер")
    TODO+=("Когда в репозитории бота появится $rel, запустите команду ещё раз: она перейдёт на полную версию сама")
    return 0
  fi
  say "На сервере есть бот — перехожу на полную версию команд ($FULL_CMD: бот и файловый менеджер)"
  if ! RFM_VPS_HANDOVER=1 bash "$tmp" setup; then
    rm -f "$tmp"
    warn "Полная версия команд не установилась — обновляю только файловый менеджер"
    SUMMARY+=("Бот: не обновлён — полная версия команд не установилась (см. вывод выше)")
    return 0
  fi
  rm -f "$tmp"
  RFM_VPS_HANDOVER=1 exec "$CMD_DIR/$FULL_CMD" "$CMD" "${ORIG_ARGS[@]}"
}

# --- Команды -----------------------------------------------------------------
cmd_setup() {
  local tmp src name target skip_delete=0
  [[ $EUID -eq 0 ]] || die "Запустите от root"
  install -d -m 0755 "$CMD_DIR"
  for name in deploy post_deploy post-deploy delete; do
    [[ -e $CMD_DIR/$name || -L $CMD_DIR/$name ]] || continue
    target=$(readlink "$CMD_DIR/$name" 2>/dev/null)
    if [[ $name == delete && ( ! -L $CMD_DIR/$name || ( $target != "$CMD_NAME" && $target != "$FULL_CMD" ) ) ]]; then
      # Имя delete короткое и могло быть занято раньше: не мешает установке остальных.
      warn "$CMD_DIR/delete уже занят другой командой — удаление запускайте как: $CMD_NAME delete"
      skip_delete=1; continue
    fi
    # Полная версия уже обслуживает и файловый менеджер — её не заменяем.
    [[ $target == "$FULL_CMD" ]] &&
      die "$CMD_DIR/$name — полная версия команд ($FULL_CMD): она уже обновляет файловый менеджер. setup ничего не заменяет"
    if [[ ! -L $CMD_DIR/$name || $target != rfm-vps ]]; then
      die "$CMD_DIR/$name уже занят другой командой — setup ничего не заменяет. Выберите другой CMD_DIR"
    fi
  done
  tmp=$(mktemp)
  if [[ -f $0 && $(head -c 300 "$0" 2>/dev/null) == *rfm-vps* ]]; then
    cp "$0" "$tmp"; src=$0
  else
    src="$RFM_VPS_RAW/$RFM_VPS_REF/deploy/rfm-vps.sh"
    curl -fsSL "$src" -o "$tmp" || { rm -f "$tmp"; die "Не удалось скачать $src"; }
  fi
  bash -n "$tmp" || { rm -f "$tmp"; die "Скачанный скрипт повреждён"; }
  install_command "$tmp" "$CMD_DIR/rfm-vps" || { rm -f "$tmp"; die "Не удалось установить команды"; }
  rm -f "$tmp"
  ln -sfn rfm-vps "$CMD_DIR/deploy"
  ln -sfn rfm-vps "$CMD_DIR/post_deploy"
  ln -sfn rfm-vps "$CMD_DIR/post-deploy"
  (( skip_delete )) || ln -sfn rfm-vps "$CMD_DIR/delete"
  ok "команды установлены в $CMD_DIR из $src"
  cat <<EOF

    deploy         — установить файловый менеджер, nginx и HTTPS (только то, чего нет)
    post_deploy    — обновить установленный файловый менеджер
    rfm-vps status — что сейчас стоит на сервере

EOF
}

cmd_status() {
  print_status
  [[ -f $STATE_FILE ]] && say "настройки команд: $STATE_FILE"
  return 0
}

# Порты нового файлового менеджера и его nginx. Занятые другими службами
# (координатор Headscale и XHTTP на 8080, 3x-ui, NaiveProxy, DERP…) порты команда
# не отбирает: программе выбирается свободный порт, а при занятом порте HTTPS
# она останавливается до установки пакетов и долгой сборки.
choose_fm_port() {
  local p owner
  if [[ -e $RFM_ENV ]]; then FM_PORT=$(fm_port); return 0; fi
  for p in $(seq 8080 8099); do
    [[ $p == 8091 ]] && continue
    owner=$(port_owner "$p")
    if [[ -z $owner ]]; then FM_PORT=$p; return 0; fi
    say "Порт $p занят ($owner) — файловому менеджеру нужен другой"
  done
  die "Порты 8080–8099 заняты — освободите один или установите FM вручную (DEPLOY.md)"
}

fm_ports_check() {
  local p owner
  choose_fm_port
  # 8091 — внутренний API для Telegram-бота (TELEGRAM_MINIAPP.md); сайту он не нужен.
  owner=$(port_owner 8091)
  [[ -z $owner ]] || warn "Порт 8091 занят ($owner): сайту это не мешает, но для связки с ботом (TELEGRAM_MINIAPP.md) API понадобится другой порт"

  case $HTTPS_MODE in
    certbot)
      for p in 80 443; do
        owner=$(port_owner "$p")
        [[ -z $owner || $owner == nginx ]] ||
          die "Порт $p занят ($owner) — certbot через nginx не подойдёт. Выберите HTTPS_MODE=cloudflare или none"
      done ;;
    cloudflare)
      owner=$(port_owner 2083)
      [[ -z $owner || $owner == nginx ]] || die "Порт 2083 занят ($owner) — выберите HTTPS_MODE=none и свой прокси"
      owner=$(port_owner 80)
      if ! has nginx && [[ -n $owner ]]; then
        die "Порт 80 занят ($owner): новый nginx не запустится. Освободите порт или выберите HTTPS_MODE=none"
      fi ;;
  esac
}

deploy_questions() {
  local busy443='' others=''
  fm_present && INSTALL_FM=no || INSTALL_FM=yes
  [[ $INSTALL_FM == yes ]] || return 0
  step "Вопросы (дальше установка пойдёт сама)"
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
  fm_ports_check
  disk_ok "$BUILD_ROOT" "$FRESH_FREE_MB" "Сборка файлового менеджера" ||
    die "Освободите место на диске и запустите deploy снова"
  [[ $HTTPS_MODE == certbot ]] && ask LE_EMAIL "Почта для Let's Encrypt (Enter — без почты)" ""
  ask RFM_ADMIN_LOGIN "Логин администратора сайта" admin
  [[ -e $RFM_ENV ]] || ask_password
  if [[ -z $ENABLE_UFW ]] && has ufw && ! ufw status 2>/dev/null | grep -q 'Status: active'; then
    others=$(public_listeners | awk '$2 != "sshd" {printf "%s%s(%s)", sep, $1, $2; sep = " "}')
    if [[ -n $others ]]; then
      # Рабочий сервер (VPN и др.): файрвол только по явному согласию, --yes его не включает.
      say "ufw выключен, а на сервере уже работают службы: $others"
      say "Если включить ufw, эти порты останутся открыты; службы, которые сейчас остановлены, закроются"
      if confirm "Включить ufw?" n; then ENABLE_UFW=yes; else ENABLE_UFW=no; fi
    elif confirm "Включить файрвол ufw (SSH, 80 и порт HTTPS)?" y; then ENABLE_UFW=yes; else ENABLE_UFW=no; fi
  fi
  step "План"
  say "• файловый менеджер на https://$RFM_DOMAIN, HTTPS: $HTTPS_MODE, логин $RFM_ADMIN_LOGIN$([[ ${FM_PORT:-8080} != 8080 ]] && echo ", программа на 127.0.0.1:$FM_PORT")"
  if [[ $ENABLE_UFW == yes ]] && ! ufw status 2>/dev/null | grep -q 'Status: active'; then
    say "• включить ufw: SSH, порты сайта и уже работающих служб"
  fi
  confirm "Начинаем?" y || die "Отменено"
}

cmd_deploy() {
  local rc=0
  preflight
  self_update
  handover_to_full
  take_lock
  ensure_delete_link
  print_status
  deploy_questions
  OTHERS_BEFORE=$(others_snapshot)
  [[ -n $HTTPS_MODE ]] || HTTPS_MODE=$(state_get HTTPS_MODE)
  if [[ $INSTALL_FM == no ]]; then
    step "Файловый менеджер уже установлен"
    # Повторный deploy доделывает то, что в прошлый раз отложено: например,
    # nginx на 2083 после того, как положили сертификат Cloudflare.
    fm_nginx || rc=1
    final_checks || rc=1
    say "Для обновления используйте: post_deploy"
    print_summary
    return "$rc"
  fi
  INSTALLED_NOW=1
  install_packages
  step "Подготовка сервера"
  setup_swap "$FRESH_FREE_MB"
  setup_firewall
  fm_install_fresh
  if [[ -n $HTTPS_MODE ]]; then
    step "nginx и HTTPS"
    fm_nginx || rc=1
    state_set HTTPS_MODE "$HTTPS_MODE"
  fi
  final_checks || rc=1
  print_summary
  return "$rc"
}

post_deploy_questions() {
  step "План обновления"
  if [[ -t 0 ]] && (( ! ASSUME_YES )) && (( ! BACKUP_DATA )); then
    confirm "Сохранить архив данных файлового менеджера (потребуется место и короткая остановка)?" n && BACKUP_DATA=1
  fi
  say "• файловый менеджер: ${RFM_REF:-$(state_get RFM_REF)} (пустое значение = main); копия старой программы и проверка с откатом"
  if swap_needed; then
    say "• если понадобится сборка: swap 2 ГБ (памяти $(mem_mb) МБ, swap нет), при свободных 3 ГБ на диске"
  fi
  (( BACKUP_DATA )) && say "• перед заменой программы: архив данных (служба на это время остановится)"
  if [[ -t 0 ]] && (( ! ASSUME_YES )); then
    confirm "Обновляем по этому плану?" y || die "Отменено"
  fi
}

cmd_post_deploy() {
  local rc=0
  [[ $EUID -eq 0 ]] || die "Запустите от root: sudo -i, затем команду ещё раз"
  # Проверка не должна заменять установленные команды или перезапускать себя.
  if (( ! CHECK_ONLY )); then self_update; handover_to_full; take_lock; ensure_delete_link; fi
  print_status
  if ! fm_present; then
    step "Файловый менеджер не установлен"
    say "Для установки с нуля: deploy"
    return 1
  fi
  if (( CHECK_ONLY )); then check_updates; return $?; fi
  post_deploy_questions
  OTHERS_BEFORE=$(others_snapshot)
  fm_update && fm_nginx_uploads || rc=1
  final_checks || rc=1
  print_summary
  return "$rc"
}

check_updates() {
  local ref remote running rc=0 label url
  step "Доступные обновления (программы и настройки не меняю)"
  ref=${RFM_REF:-$(state_get RFM_REF)}; ref=${ref:-main}
  remote=''
  while read -r label url; do
    remote=$(src_try "$label" "$url" ls-remote "$url" "$ref" "$ref^{}" | tail -1 | cut -f1)
    [[ -n $remote ]] || continue
    primary_src "$label" || say "сверяю с источником «$label»"
    break
  done < <(code_sources fm)
  running=$(fm_commit); running=${running%-dirty}
  if [[ -z $remote ]]; then warn "не удалось узнать $ref ни в одном источнике"; rc=1
  elif [[ -n $running && $remote == "$running"* ]]; then ok "файловый менеджер: последняя версия ($running)"
  else say "файловый менеджер: есть обновление ${running:-?} → ${remote:0:7}"; fi
  say "Обновить: post_deploy"
  return "$rc"
}

final_checks() {
  local rc=0
  step "Проверка"
  if systemctl is-active --quiet "$RFM_UNIT"; then ok "файловый менеджер $(fm_version) работает"
  else fail "файловый менеджер не работает"; rc=1; fi
  if (( INSTALLED_NOW )); then
    TODO+=("Войдите на $(env_get "$RFM_ENV" PUBLIC_BASE_URL)/login логином администратора и пригласите участников")
    TODO+=("По желанию: мини-приложение для Telegram и API для своего бота — TELEGRAM_MINIAPP.md")
  fi
  others_check || rc=1
  return "$rc"
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
rfm-vps — установка и обновление rust-file-manager на VPS

  deploy [--ref REF] [--yes]
      Установить файловый менеджер, nginx и HTTPS, если их ещё нет.
      Сначала задаёт все вопросы, потом ставит сам.
  post_deploy [--check] [--ref REF] [--force] [--backup-data] [--yes]
      Обновить установленный файловый менеджер. --check — только показать,
      есть ли обновление. В терминале спрашивает архив данных и подтверждение
      плана; --yes — выполнить без этих вопросов.
      --ref закрепляет выпуск для следующих запусков (вернуться: --ref main).
      --force — пересобрать и перезапустить, даже если версия та же.
      --backup-data — перед заменой программы сохранить архив данных.
  delete [--dry-run] [--only fm] [--confirm ИМЯ]
      Удалить файловый менеджер: показывает план и выполняет после ввода имени
      сервера. Файлы переносятся в копию /var/backups/rfm-delete; в конце
      предлагает убрать ненужное общее (сертификат, Rust, swap, порты в ufw).
      --dry-run — только план; --confirm ИМЯ — без вопросов (общее остаётся).
  rfm-vps status    что стоит на сервере
  rfm-vps setup     (пере)установить команды deploy, post_deploy и delete

REF — ветка или тег rust-file-manager (по умолчанию main).
Инструкции: DEPLOY_ALGORITHM.md, DEPLOY.md и POST_DEPLOY.md в репозитории rust-file-manager.
EOF
}

main() {
  ORIG_ARGS=("$@")
  case ${0##*/} in
    deploy) CMD=deploy ;;
    post_deploy|post-deploy) CMD=post_deploy ;;
    delete) CMD=delete ;;
    *) CMD=${1:-help}; shift || true; ORIG_ARGS=("$@") ;;
  esac
  [[ $CMD == post-deploy ]] && CMD=post_deploy
  while (( $# )); do
    case $1 in
      --yes|-y) ASSUME_YES=1 ;;
      --force) FORCE=1 ;;
      --check) CHECK_ONLY=1 ;;
      --dry-run) DRY_RUN=1 ;;
      --confirm) [[ -n ${2:-} && ${2:-} != -* ]] || die "--confirm требует имя сервера"; CONFIRM_HOST=$2; shift ;;
      --backup-data) BACKUP_DATA=1 ;;
      --only)
        # Прежний параметр: файловый менеджер — единственный проект этой команды.
        [[ ${2:-} == fm ]] || die "--only: эта команда обслуживает только файловый менеджер (--only fm)"
        ONLY=fm; shift ;;
      --ref) [[ -n ${2:-} && ${2:-} != -* ]] || die "--ref требует ветку или тег"; RFM_REF=$2; shift ;;
      -h|--help) usage; return 0 ;;
      *) die "Неизвестный параметр: $1 (см. --help)" ;;
    esac
    shift
  done
  # Свои зеркала: переменная окружения, иначе строка RFM_MIRRORS=… в /etc/rfm-vps.conf.
  [[ -n $RFM_MIRRORS ]] || RFM_MIRRORS=$(state_get RFM_MIRRORS)
  case $CMD in
    setup) cmd_setup ;;
    deploy) cmd_deploy ;;
    post_deploy) cmd_post_deploy ;;
    delete) cmd_delete ;;
    status) cmd_status ;;
    help|-h|--help) usage ;;
    *) usage; return 1 ;;
  esac
}

main "$@"; exit $?
