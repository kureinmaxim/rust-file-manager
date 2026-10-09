#!/usr/bin/env bash
# Тесты deploy/rfm-vps.sh (команды deploy и post_deploy) в песочнице.
#
# systemd, Docker, apt, ufw, nginx, certbot и curl подменены заглушками, GitHub —
# локальными git-репозиториями, сборка Rust — заглушкой cargo. Настоящая система
# не меняется: все пути скрипта переопределены на временный каталог.
# Запуск от root (нужны install -o root и chmod):
#   bash deploy/tests/rfm-vps.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
SCRIPT=$REPO/deploy/rfm-vps.sh
PASS=0
FAILED=0

check() {  # описание команда…
  local desc=$1; shift
  if "$@" >/dev/null 2>&1; then
    PASS=$((PASS + 1)); printf '  ok    %s\n' "$desc"
  else
    FAILED=$((FAILED + 1)); printf '  FAIL  %s\n' "$desc"
  fi
}
has_line() { grep -qF -- "$2" "$1"; }
no_line() { ! grep -qF -- "$2" "$1"; }
val() { sed -n "s/^$2=//p" "$1" | tail -1; }
count() { grep -cF -- "$2" "$1" 2>/dev/null || true; }

# --- Песочница ---------------------------------------------------------------
new_sandbox() {
  SB=$(mktemp -d)
  export SB
  mkdir -p "$SB"/{bin,state/docker,repos,src,root,run,usr/local/bin,usr/local/sbin,etc/systemd/system,etc/sysctl.d}
  mkdir -p "$SB"/etc/nginx/{sites-available,sites-enabled}
  : >"$SB/calls.log"; : >"$SB/etc/fstab"; : >"$SB/cargo-env"
  write_stubs
  make_rfm_repo
  make_bot_repo
  export PATH="$SB/bin:$ORIG_PATH"
  export RFM_REPO_URL="file://$SB/repos/rfm.git" BOT_REPO_URL="file://$SB/repos/bot.git"
  export RFM_VPS_RAW="file://$SB/raw" RFM_UNIT=rust-file-manager.service
  export RFM_BIN=$SB/usr/local/bin/rust-file-manager UNIT_DIR=$SB/etc/systemd/system
  export RFM_ENV=$SB/etc/rust-file-manager/env RFM_DATA=$SB/var/lib/rust-file-manager
  export RFM_BACKUPS=$SB/var/backups/rust-file-manager BOT_DIR=$SB/opt/TelegramOnly
  export BOT_BACKUPS=$SB/var/backups/telegramonly BUILD_ROOT=$SB/root STATE_FILE=$SB/etc/rfm-vps.conf
  export CMD_DIR=$SB/usr/local/sbin NGINX_DIR=$SB/etc/nginx LE_DIR=$SB/etc/letsencrypt
  export SYSCTL_FILE=$SB/etc/sysctl.d/99-tcp-mtu-probing.conf SWAP_FILE=$SB/swapfile
  export LOCK_FILE=$SB/run/rfm-vps.lock CARGO_ENV=$SB/cargo-env FM_USER=root FSTAB=$SB/etc/fstab
  export POLL_SECONDS=1 WAIT_SECONDS=1
  export SSH_CONNECTION="203.0.113.5 50000 203.0.113.10 22"
  unset RFM_DOMAIN HTTPS_MODE LE_EMAIL RFM_ADMIN_LOGIN BOT_NETWORK ENABLE_UFW INSTALL_BOT INSTALL_FM
}

stub() {  # имя тело
  printf '#!/usr/bin/env bash\necho "%s $*" >>"$SB/calls.log"\n%s\n' "$1" "$2" >"$SB/bin/$1"
  chmod 755 "$SB/bin/$1"
}

write_stubs() {
  stub apt-get 'exit 0'
  stub sysctl 'exit 0'
  stub swapon '[[ ${1:-} == --show ]] && echo "/swapfile file 2G 0B -2"; exit 0'
  stub fallocate 'exit 0'
  stub mkswap 'exit 0'
  stub useradd 'exit 0'
  stub ss 'exit 0'
  stub rustc 'echo "rustc 1.96.0 (fake)"'
  stub rustup 'exit 0'
  stub nginx 'exit 0'
  stub certbot 'd=""; while (( $# )); do [[ $1 == -d ]] && d=$2; shift; done; mkdir -p "$LE_DIR/live/$d"'
  stub ufw '
st=$SB/state/ufw-active
case ${1:-} in
  status) [[ -f $st ]] && echo "Status: active" || echo "Status: inactive" ;;
  --force) touch "$st" ;;
esac
exit 0'
  stub systemctl '
st=$SB/state; q=0; args=()
for a in "$@"; do case $a in --quiet|-q|--no-pager) q=1 ;; *) args+=("$a") ;; esac; done
set -- "${args[@]}"
start_unit() {
  local unit=$1 m ver commit broken
  if [[ $unit == "$RFM_UNIT" ]]; then
    m=$(grep -m1 "FAKE rust-file-manager" "$RFM_BIN")
    ver=$(grep -o "version=[^ ]*" <<<"$m" | cut -d= -f2)
    commit=$(grep -o "commit=[^ ]*" <<<"$m" | cut -d= -f2)
    broken=$(grep -o "broken=[^ ]*" <<<"$m" | cut -d= -f2)
    if [[ $broken == 1 ]]; then rm -f "$st/active-$unit"; return; fi
    printf "%s|\e[2m2026-10-09T00:00:00Z\e[0m \e[32m INFO\e[0m \e[2mrust_file_manager\e[0m\e[2m:\e[0m starting server \e[3mversion\e[0m\e[2m=\e[0m\"%s\" \e[3mcommit\e[0m\e[2m=\e[0m\"%s\" miniapp=true\n" "$(date +%s)" "$ver" "$commit" >>"$st/journal"
  fi
  touch "$st/active-$unit"
}
case ${1:-} in
  cat) [[ -f $UNIT_DIR/${2:-x} ]] ;;
  is-active) if [[ -f $st/active-${2:-x} ]]; then ((q)) || echo active; exit 0; fi; ((q)) || echo inactive; exit 3 ;;
  restart|start) start_unit "$2" ;;
  stop) rm -f "$st/active-$2" ;;
  enable) [[ ${2:-} == --now ]] && start_unit "$3"; exit 0 ;;
  show) [[ -f $st/workdir ]] && cat "$st/workdir"; exit 0 ;;
  *) exit 0 ;;
esac'
  stub journalctl '
since=0; prev=""
for a in "$@"; do [[ $prev == --since ]] && since=${a#@}; prev=$a; done
[[ -f $SB/state/journal ]] || exit 0
while IFS="|" read -r t line; do (( t >= since )) && printf "%s\n" "$line"; done <"$SB/state/journal"
exit 0'
  stub docker '
d=$SB/state/docker
case ${1:-} in
  inspect)
    shift; fmt=""; [[ ${1:-} == -f ]] && { fmt=$2; shift 2; }
    [[ -f $d/container ]] || exit 1
    case $fmt in
      "") echo "[{}]" ;;
      *NetworkMode*) cat "$d/net" ;;
      *config_files*) cat "$d/label" ;;
      *Gateway*) [[ $(cat "$d/net") == host ]] || echo 172.18.0.1 ;;
      *State.Running*) cat "$d/running" 2>/dev/null || echo true ;;
      *working_dir*) cat "$d/workdir" 2>/dev/null ;;
    esac ;;
  exec) printf "version = \"%s\"\n" "$(cat "$d/version")" ;;
  compose) case ${2:-} in version) echo "Docker Compose version v2.29.0" ;; up) echo up >>"$d/recreated" ;; esac ;;
  logs) grep -q "^FILES_SERVICE_TOKEN=" "$BOT_DIR/.env" 2>/dev/null &&
          echo "2026-10-09 00:00:00,000 - files_menu - INFO - files: кнопки меню — выставлено 1, сброшено 0, ошибок 0" ;;
  network) echo 172.18.0.0/16 ;;
  --version) echo "Docker version 27.0.0 (fake)" ;;
esac
exit 0'
  # curl: локальные проверки, getMe, внутренний API и скачивание самого скрипта.
  stub curl '
out=""; w=""; kstdin=0; hstdin=0; url=""
while (( $# )); do
  case $1 in
    -o) out=$2; shift ;;
    -w) w=$2; shift ;;
    -K) [[ $2 == - ]] && kstdin=1; shift ;;
    -H) [[ $2 == @- ]] && hstdin=1; shift ;;
    --max-time|--proto) shift ;;
    -*) ;;
    *) url=$1 ;;
  esac
  shift
done
st=$SB/state
if (( kstdin )); then
  cfg=$(cat); tok=$(grep -o "bot[0-9]*:[A-Za-z0-9_-]*" <<<"$cfg" | sed "s/^bot//")
  want=$(sed -n "s/^BOT_TOKEN=//p" "$BOT_DIR/.env")
  if [[ -n $tok && $tok == "$want" ]]; then
    echo "{\"ok\":true,\"result\":{\"id\":7000000001,\"is_bot\":true,\"first_name\":\"Files\",\"username\":\"files_example_bot\"}}"
  else echo "{\"ok\":false}"; fi
  exit 0
fi
code=000
case $url in
  file://*) cp "${url#file://}" "$out"; exit $? ;;
  http://127.0.0.1:8080/*) [[ -f $st/active-$RFM_UNIT ]] && code=200 ;;
  http://*:8091/internal/*)
    hdr=""; (( hstdin )) && hdr=$(cat)
    if [[ -f $st/active-$RFM_UNIT ]]; then
      want="Authorization: Bearer $(sed -n "s/^INTERNAL_API_TOKEN=//p" "$RFM_ENV")"
      [[ $hdr == "$want" ]] && code=200 || code=401
    fi ;;
  https://*) code=${PUBLIC_CODE:-200} ;;
esac
[[ -n $w ]] && printf "%s" "$code"
exit 0'
  # cargo: «собирает» программу-заглушку с версией и коммитом из исходников.
  stub cargo '
[[ ${1:-} == build ]] || exit 0
ver=$(grep -m1 "^version" Cargo.toml | cut -d\" -f2); commit=$(git rev-parse --short=7 HEAD)
echo "   Compiling rust-file-manager v$ver ($PWD)"
[[ -f FAIL_BUILD ]] && { echo "error: fake failure"; exit 101; }
broken=0; [[ -f BROKEN ]] && broken=1
mkdir -p target/release
cat >target/release/rust-file-manager <<EOF
#!/bin/sh
# FAKE rust-file-manager version=$ver commit=$commit broken=$broken
if [ "\$1" = hash-password ]; then cat >/dev/null; printf "\\\$2b\\\$12\\\$%s\\n" "\$(printf "a%.0s" \$(seq 53))"; fi
EOF
chmod 755 target/release/rust-file-manager
echo "    Finished \`release\` profile [optimized] target(s) in 0.01s"'
}

git_commit_all() { git -C "$1" add -A && git -C "$1" -c user.name=t -c user.email=t@example.com commit -qm "$2"; }

make_rfm_repo() {
  local s=$SB/src/rfm
  mkdir -p "$s/deploy" "$SB/raw/main/deploy"
  git init -q -b main "$s"
  printf '[package]\nname = "rust-file-manager"\nversion = "1.7.0"\n' >"$s/Cargo.toml"
  printf 'target/\n' >"$s/.gitignore"
  cp "$REPO/deploy/rust-file-manager.service" "$REPO/deploy/nginx.example.conf" "$SCRIPT" "$s/deploy/"
  git_commit_all "$s" "1.7.0"
  git clone -q --bare "$s" "$SB/repos/rfm.git"
  cp "$SCRIPT" "$SB/raw/main/deploy/rfm-vps.sh"
}

rfm_release() {  # версия [BROKEN]
  local s=$SB/src/rfm
  sed -i "s/^version = .*/version = \"$1\"/" "$s/Cargo.toml"
  rm -f "$s/BROKEN"; [[ ${2:-} == BROKEN ]] && touch "$s/BROKEN"
  git_commit_all "$s" "$1" && git -C "$s" push -q "$SB/repos/rfm.git" main
}

make_bot_repo() {
  local s=$SB/src/bot
  mkdir -p "$s/scripts"
  git init -q -b main "$s"
  printf '[project]\nname = "telegramonly"\nversion = "3.25.0"\n' >"$s/pyproject.toml"
  printf 'BOT_TOKEN=your_bot_token_here\nADMIN_USER_IDS=123456789\n' >"$s/example.env"
  printf '.env\n' >"$s/.gitignore"
  printf 'services: {}\n' >"$s/compose.yaml"; printf 'services: {}\n' >"$s/compose.host.yaml"
  printf 'python-telegram-bot\n' >"$s/requirements.txt"
  cat >"$s/scripts/install_telegramonly_docker.sh" <<'EOF'
#!/usr/bin/env bash
cd "$(dirname "$0")/.." || exit 1
[ -f .env ] || cp example.env .env
sed -i 's/^BOT_TOKEN=.*/BOT_TOKEN=7000000001:AAFakeTokenForTests_0123456789/' .env
d=$SB/state/docker
if grep -q '^COMPOSE_FILE=.*compose.host.yaml' .env; then
  echo host >"$d/net"; echo "$PWD/compose.yaml,$PWD/compose.host.yaml" >"$d/label"
else
  echo telegramonly_default >"$d/net"; echo "$PWD/compose.yaml" >"$d/label"
fi
grep -m1 '^version' pyproject.toml | cut -d'"' -f2 >"$d/version"
touch "$d/container"
echo "fake installer done"
EOF
  # Как настоящий rebuild_bot.sh: без COMPOSE_FILE в .env бот оказывается в bridge.
  cat >"$s/scripts/rebuild_bot.sh" <<'EOF'
#!/usr/bin/env bash
cd "$(dirname "$0")/.." || exit 1
d=$SB/state/docker
grep -m1 '^version' pyproject.toml | cut -d'"' -f2 >"$d/version"
if grep -q '^COMPOSE_FILE=.*compose.host.yaml' .env; then echo host >"$d/net"; else echo telegramonly_default >"$d/net"; fi
echo "✓ Версия в контейнере: $(cat "$d/version") — совпадает с pyproject.toml."
EOF
  git_commit_all "$s" "3.25.0"
  git clone -q --bare "$s" "$SB/repos/bot.git"
}

bot_release() {  # версия
  local s=$SB/src/bot
  sed -i "s/^version = .*/version = \"$1\"/" "$s/pyproject.toml"
  git_commit_all "$s" "$1" && git -C "$s" push -q "$SB/repos/bot.git" main
}

run() {  # вывод_в_файл аргументы… (stdin передаётся дальше)
  local out=$1; shift
  bash "$SCRIPT" "$@" >"$out" 2>&1
}

no_secrets() {  # файл — в выводе нет пароля, ключей и токена бота
  local f=$1 s
  for s in "Secret-pass-1" "$(val "$RFM_ENV" INTERNAL_API_TOKEN)" "$(val "$RFM_ENV" SESSION_SECRET)" \
           "AAFakeTokenForTests_0123456789"; do
    [[ -z $s ]] && continue
    grep -qF -- "$s" "$f" && return 1
  done
  return 0
}

ORIG_PATH=$PATH
[[ $EUID -eq 0 ]] || { echo "Запустите от root"; exit 1; }

# ============================================================================
echo "1. Пустой сервер"
new_sandbox
run "$SB/o1" post_deploy; rc=$?
check "post_deploy без установленного завершается с ошибкой" test "$rc" -eq 1
check "и предлагает deploy" has_line "$SB/o1" "Для установки с нуля: deploy"
run "$SB/o1s" status; rc=$?
check "status завершается без ошибки" test "$rc" -eq 0
check "status: ничего не установлено" test "$(count "$SB/o1s" "не установлен")" -eq 2

echo "2. deploy с нуля: бот в сети хоста, HTTPS через certbot"
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=certbot BOT_NETWORK=host ENABLE_UFW=yes run "$SB/o2" deploy --yes; rc=$?
check "deploy завершился успешно" test "$rc" -eq 0
check "контейнер бота создан" test -f "$SB/state/docker/container"
check "бот закреплён в сети хоста (COMPOSE_FILE)" test "$(val "$BOT_DIR/.env" COMPOSE_FILE)" = compose.yaml:compose.host.yaml
check "настройки FM с правами 600" test "$(stat -c %a "$RFM_ENV")" = 600
check "мини-приложение включено для бота из getMe" test "$(val "$RFM_ENV" TELEGRAM_BOT_ID)" = 7000000001
check "имя бота записано" test "$(val "$RFM_ENV" TELEGRAM_BOT_USERNAME)" = files_example_bot
check "адрес сайта записан" test "$(val "$RFM_ENV" PUBLIC_BASE_URL)" = https://files.example.com
check "ключ для бота — 64 символа" test "$(val "$RFM_ENV" INTERNAL_API_TOKEN | tr -d '\n' | wc -c)" -eq 64
check "хеш пароля bcrypt" has_line "$RFM_ENV" "ADMIN_PASSWORD_HASH='\$2b\$12\$"
check "в настройках нет повторов" test -z "$(cut -d= -f1 "$RFM_ENV" | grep -v '^#' | sort | uniq -d)"
check "служба FM запущена" test -f "$SB/state/active-rust-file-manager.service"
check "в .env бота адрес API через localhost" test "$(val "$BOT_DIR/.env" FILES_INTERNAL_URL)" = http://127.0.0.1:8091
check "ключи бота и FM совпадают" test "$(val "$BOT_DIR/.env" FILES_SERVICE_TOKEN)" = "$(val "$RFM_ENV" INTERNAL_API_TOKEN)"
check "адрес мини-приложения в .env бота" test "$(val "$BOT_DIR/.env" FILES_MINIAPP_URL)" = https://files.example.com/tg/
check "nginx: свой домен и лимит 201M" has_line "$NGINX_DIR/sites-available/rust-file-manager" "server_name files.example.com;"
check "nginx: сайт включён" test -L "$NGINX_DIR/sites-enabled/rust-file-manager"
check "certbot выпустил сертификат" test -d "$LE_DIR/live/files.example.com"
check "ufw: открыт порт SSH текущего подключения" has_line "$SB/calls.log" "ufw allow 22/tcp"
check "ufw: открыт 443" has_line "$SB/calls.log" "ufw allow 443/tcp"
check "ufw включён" has_line "$SB/calls.log" "ufw --force enable"
check "MTU probing записан" has_line "$SYSCTL_FILE" "net.ipv4.tcp_mtu_probing = 2"
check "состояние: каталог сборки" test "$(val "$STATE_FILE" RFM_BUILD_DIR)" = "$BUILD_ROOT/rfm-build"
check "состояние: способ HTTPS" test "$(val "$STATE_FILE" HTTPS_MODE)" = certbot
check "подсказка про BotFather с адресом /tg/" has_line "$SB/o2" "https://files.example.com/tg/"
check "подсказка про первый вход" has_line "$SB/o2" "https://t.me/files_example_bot?startapp"
check "в выводе нет секретов" no_secrets "$SB/o2"
check "секретов нет и в аргументах команд" no_secrets "$SB/calls.log"

echo "3. Повторный deploy на готовом сервере"
apt_before=$(count "$SB/calls.log" "apt-get")
run "$SB/o3" deploy --yes; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "сообщает, что всё установлено" has_line "$SB/o3" "Всё уже установлено"
check "пакеты не ставит" test "$(count "$SB/calls.log" "apt-get")" -eq "$apt_before"
check "ключи совпадают — ничего не меняет" has_line "$SB/o3" "ключи совпадают"

echo "4. post_deploy без новых версий"
restarts=$(count "$SB/calls.log" "systemctl restart rust-file-manager.service")
run "$SB/o4" post_deploy; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "FM и бот: уже последние версии" test "$(count "$SB/o4" "уже последняя версия")" -eq 2
check "FM не перезапускался" test "$(count "$SB/calls.log" "systemctl restart rust-file-manager.service")" -eq "$restarts"

echo "5. post_deploy --check при новых версиях"
rfm_release 1.8.0; bot_release 3.26.0
bin_before=$(md5sum <"$RFM_BIN")
run "$SB/o5" post_deploy --check; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "видит обновление FM" has_line "$SB/o5" "файловый менеджер: есть обновление"
check "видит обновление бота" has_line "$SB/o5" "бот: есть обновление 3.25.0 → 3.26.0"
check "программу не трогает" test "$(md5sum <"$RFM_BIN")" = "$bin_before"
check "бота не трогает" test "$(cat "$SB/state/docker/version")" = 3.25.0

echo "6. post_deploy обновляет оба проекта"
run "$SB/o6" post_deploy; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "запущен FM 1.8.0" has_line "$RFM_BIN" "version=1.8.0"
check "проверки после запуска прошли" has_line "$SB/o6" "запущен 1.8.0"
check "копия прежней программы сохранена" compgen -G "$RFM_BACKUPS/rust-file-manager-1.7.0-*"
check "каталог копий закрыт (700)" test "$(stat -c %a "$RFM_BACKUPS")" = 700
check "бот обновлён до 3.26.0" test "$(cat "$SB/state/docker/version")" = 3.26.0
check "бот остался в сети хоста" test "$(cat "$SB/state/docker/net")" = host
check "итог: FM 1.7.0 → 1.8.0" has_line "$SB/o6" "Файловый менеджер: 1.7.0 → 1.8.0"
check "итог: бот 3.25.0 → 3.26.0" has_line "$SB/o6" "Бот TelegramOnly: 3.25.0 → 3.26.0"
check "в выводе нет секретов" no_secrets "$SB/o6"

echo "6б. --ref закрепляет выпуск"
git -C "$SB/src/rfm" tag v1.8.0 && git -C "$SB/src/rfm" push -q "$SB/repos/rfm.git" v1.8.0
run "$SB/o6b" post_deploy --only fm --ref v1.8.0 --force; rc=$?
check "post_deploy --ref v1.8.0 успешен" test "$rc" -eq 0
check "выпуск запомнен" test "$(val "$STATE_FILE" RFM_REF)" = v1.8.0
run "$SB/o6c" post_deploy --only fm --ref main; rc=$?
check "возврат на main запомнен" test "$(val "$STATE_FILE" RFM_REF)" = main

echo "7. Неудачное обновление FM откатывается"
rfm_release 1.8.1 BROKEN
run "$SB/o7" post_deploy --only fm; rc=$?
check "post_deploy сообщает об ошибке" test "$rc" -eq 1
check "в итоге — откат" has_line "$SB/o7" "ОТКАТ на 1.8.0"
check "на месте снова программа 1.8.0" has_line "$RFM_BIN" "version=1.8.0"
check "служба снова работает" test -f "$SB/state/active-rust-file-manager.service"

echo "8. Бот в сети хоста без COMPOSE_FILE (как на старом сервере)"
rfm_release 1.8.2
sed -i '/COMPOSE_FILE/d' "$BOT_DIR/.env"
bot_release 3.27.0
run "$SB/o8" post_deploy; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "COMPOSE_FILE возвращён в .env" test "$(val "$BOT_DIR/.env" COMPOSE_FILE)" = compose.yaml:compose.host.yaml
check "после пересборки бот всё ещё в сети хоста" test "$(cat "$SB/state/docker/net")" = host
check "копия .env бота сохранена с правами 600" test "$(stat -c %a "$(ls -t "$BOT_BACKUPS"/env-* | head -1)")" = 600

echo "9. Ключ бота разошёлся с ключом FM"
sed -i 's/^FILES_SERVICE_TOKEN=.*/FILES_SERVICE_TOKEN=wrong/' "$BOT_DIR/.env"
recreated=$(wc -l <"$SB/state/docker/recreated" 2>/dev/null || echo 0)
run "$SB/o9" deploy --yes; rc=$?
check "deploy исправил ключ" test "$(val "$BOT_DIR/.env" FILES_SERVICE_TOKEN)" = "$(val "$RFM_ENV" INTERNAL_API_TOKEN)"
check "бот пересоздан" test "$(wc -l <"$SB/state/docker/recreated")" -gt "$recreated"
check "в выводе нет секретов" no_secrets "$SB/o9"

echo "10. setup и самообновление команд"
run "$SB/o10" setup; rc=$?
check "setup установил rfm-vps" test -x "$CMD_DIR/rfm-vps"
check "команда deploy" test -L "$CMD_DIR/deploy"
check "команда post_deploy" test -L "$CMD_DIR/post_deploy"
"$CMD_DIR/deploy" --help >"$SB/o10h" 2>&1
check "deploy --help по имени команды" has_line "$SB/o10h" "post_deploy [--check]"
printf '\n# новая версия\n' >>"$SB/raw/main/deploy/rfm-vps.sh"
"$CMD_DIR/post_deploy" --yes >"$SB/o10u" 2>&1
check "post_deploy обновил команды перед работой" has_line "$SB/o10u" "Команды deploy/post_deploy обновлены"
check "установлена новая версия" cmp -s "$CMD_DIR/rfm-vps" "$SB/raw/main/deploy/rfm-vps.sh"
check "после перезапуска работа продолжилась" has_line "$SB/o10u" "План обновления"
"$CMD_DIR/post_deploy" --check >"$SB/o10v" 2>&1
check "повторно не обновляет" no_line "$SB/o10v" "обновлены"

# ============================================================================
echo "11. deploy с нуля: бот в bridge, HTTPS через Cloudflare"
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=cloudflare BOT_NETWORK=bridge ENABLE_UFW=yes run "$SB/o11" deploy --yes; rc=$?
check "deploy завершился успешно" test "$rc" -eq 0
check "адрес API через шлюз docker-сети" test "$(val "$BOT_DIR/.env" FILES_INTERNAL_URL)" = http://172.18.0.1:8091
check "мост socat на шлюзе" has_line "$UNIT_DIR/rfm-internal-bridge.service" "bind=172.18.0.1,"
check "ufw: 8091 только для docker-сети" has_line "$SB/calls.log" "ufw allow from 172.18.0.0/16 to 172.18.0.1 port 8091 proto tcp"
check "ufw: открыт 2083, а не 443" has_line "$SB/calls.log" "ufw allow 2083/tcp"
check "без сертификата — подсказка, куда его положить" has_line "$SB/o11" "Положите Origin-сертификат Cloudflare"
check "сайт на 2083 пока не создан" test ! -e "$NGINX_DIR/sites-available/rust-file-manager-ssl"
mkdir -p "$NGINX_DIR/ssl/files.example.com"
printf 'CERT\n' >"$NGINX_DIR/ssl/files.example.com/cert.pem"; printf 'KEY\n' >"$NGINX_DIR/ssl/files.example.com/key.pem"
run "$SB/o11b" deploy --yes; rc=$?
check "повторный deploy создал сайт на 2083" has_line "$NGINX_DIR/sites-available/rust-file-manager-ssl" "listen 2083 ssl;"
check "с нужным доменом" has_line "$NGINX_DIR/sites-available/rust-file-manager-ssl" "server_name files.example.com;"
check "ключ сертификата закрыт (600)" test "$(stat -c %a "$NGINX_DIR/ssl/files.example.com/key.pem")" = 600
check "nginx-переменные не раскрыты" has_line "$NGINX_DIR/sites-available/rust-file-manager-ssl" 'proxy_set_header Host $host;'

echo "12. Сначала только бот, потом файловый менеджер"
new_sandbox
BOT_NETWORK=host ENABLE_UFW=no run "$SB/o12" deploy --yes --only bot; rc=$?
check "deploy --only bot успешен" test "$rc" -eq 0
check "бот установлен" test -f "$SB/state/docker/container"
check "файловый менеджер не ставился" test ! -e "$RFM_BIN"
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none run "$SB/o12b" deploy --yes; rc=$?
check "второй deploy успешен" test "$rc" -eq 0
check "поставил только FM" no_line "$SB/o12b" "fake installer done"
check "FM сразу с мини-приложением" test "$(val "$RFM_ENV" MINIAPP_ENABLED)" = true
check "и связан с ботом" test "$(val "$BOT_DIR/.env" FILES_SERVICE_TOKEN)" = "$(val "$RFM_ENV" INTERNAL_API_TOKEN)"
check "HTTPS none: nginx не трогает" test -z "$(ls "$NGINX_DIR/sites-available")"
check "FM запускался один раз (настройки бота — до первого запуска)" test "$(count "$SB/calls.log" "systemctl restart rust-file-manager.service")" -eq 1

echo "13. Сборка упала — программа не тронута"
rfm_release 1.9.0
touch "$SB/src/rfm/FAIL_BUILD"; git_commit_all "$SB/src/rfm" "fail" && git -C "$SB/src/rfm" push -q "$SB/repos/rfm.git" main
bin_before=$(md5sum <"$RFM_BIN")
run "$SB/o13" post_deploy --only fm; rc=$?
check "post_deploy сообщает об ошибке" test "$rc" -eq 1
check "показывает журнал сборки" has_line "$SB/o13" "error: fake failure"
check "программа не изменилась" test "$(md5sum <"$RFM_BIN")" = "$bin_before"

echo "14. deploy с ответами на вопросы (без --yes)"
new_sandbox
# сеть, домен (сначала неверный), HTTPS (Enter = certbot), почта (Enter), логин (Enter),
# пароль: короткий и ещё раз, затем верный дважды; ufw (Enter = да); «Начинаем?» (Enter)
printf '%s\n' host "not a domain" files.example.com "" "" "" short short Secret-pass-1 Secret-pass-1 "" "" |
  run "$SB/o14" deploy; rc=$?
check "deploy завершился успешно" test "$rc" -eq 0
check "неверный домен переспрошен" has_line "$SB/o14" "Это не похоже на домен"
check "короткий пароль переспрошен" has_line "$SB/o14" "Пароли короче 8 символов"
check "выбран certbot по умолчанию" test "$(val "$STATE_FILE" HTTPS_MODE)" = certbot
check "логин по умолчанию admin" test "$(val "$RFM_ENV" ADMIN_USERNAME)" = admin
check "бот в сети хоста" test "$(cat "$SB/state/docker/net")" = host
check "ufw включён по умолчанию" has_line "$SB/calls.log" "ufw --force enable"
check "в выводе нет пароля" no_secrets "$SB/o14"

echo "15. На сервере нет ничего, а доступа к репозиторию бота нет"
new_sandbox
export BOT_REPO_URL="file://$SB/repos/no-such-repo.git"
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=certbot BOT_NETWORK=host ENABLE_UFW=no run "$SB/o15" deploy --yes; rc=$?
check "deploy останавливается с ошибкой" test "$rc" -eq 1
check "подсказывает про токен и SSH-ключ" has_line "$SB/o15" "BOT_REPO_URL=git@github.com:kureinmaxim/TelegramOnly.git deploy"
check "останавливается до сборки файлового менеджера" test "$(count "$SB/calls.log" "cargo build")" -eq 0
check "не оставляет пустой клон" test ! -e "$BOT_DIR/.git"

echo "16. Код бота доставлен без git (rsync)"
new_sandbox
mkdir -p "$BOT_DIR" && cp -r "$SB/src/bot/." "$BOT_DIR/" && rm -rf "$BOT_DIR/.git"
export BOT_REPO_URL="file://$SB/repos/no-such-repo.git"
BOT_NETWORK=bridge ENABLE_UFW=no run "$SB/o16" deploy --yes --only bot; rc=$?
check "deploy --only bot успешен" test "$rc" -eq 0
check "предупреждает, что код без git" has_line "$SB/o16" "без git — использую как есть"
check "установщик бота запущен из этого кода" has_line "$SB/o16" "fake installer done"

echo
echo "Пройдено: $PASS, ошибок: $FAILED"
(( FAILED == 0 ))
