#!/usr/bin/env bash
# Тесты deploy/rfm-vps.sh (deploy и post_deploy файлового менеджера) в песочнице.
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
  export PATH="$SB/bin:$ORIG_PATH"
  export RFM_REPO_URL="file://$SB/repos/rfm.git"
  export RFM_VPS_RAW="file://$SB/raw" RFM_UNIT=rust-file-manager.service
  export RFM_BIN=$SB/usr/local/bin/rust-file-manager UNIT_DIR=$SB/etc/systemd/system
  export RFM_ENV=$SB/etc/rust-file-manager/env RFM_DATA=$SB/var/lib/rust-file-manager
  export RFM_BACKUPS=$SB/var/backups/rust-file-manager FULL_SCRIPT=$SB/opt/full/tgo-vps.sh
  export BUILD_ROOT=$SB/root STATE_FILE=$SB/etc/rfm-vps.conf
  export CMD_DIR=$SB/usr/local/sbin NGINX_DIR=$SB/etc/nginx LE_DIR=$SB/etc/letsencrypt
  export SWAP_FILE=$SB/swapfile
  export LOCK_FILE=$SB/run/rfm-vps.lock CARGO_ENV=$SB/cargo-env FM_USER=root FSTAB=$SB/etc/fstab
  export POLL_SECONDS=1
  export SSH_CONNECTION="203.0.113.5 50000 203.0.113.10 22"
  unset RFM_DOMAIN HTTPS_MODE LE_EMAIL RFM_ADMIN_LOGIN ENABLE_UFW
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
  stub docker 'exit 0'  # публичная команда Docker не вызывает
  # curl: локальные проверки, внутренний API и скачивание самого скрипта.
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

run() {  # вывод_в_файл аргументы… (stdin передаётся дальше)
  local out=$1; shift
  bash "$SCRIPT" "$@" >"$out" 2>&1
}

no_secrets() {  # файл — в выводе нет пароля и ключей
  local f=$1 s
  for s in "Secret-pass-1" "$(val "$RFM_ENV" INTERNAL_API_TOKEN)" "$(val "$RFM_ENV" SESSION_SECRET)"; do
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
check "status: файловый менеджер не установлен" has_line "$SB/o1s" "Файловый менеджер: не установлен"
check "без клона бота нет подсказки о полной версии" no_line "$SB/o1s" "Полная версия"

echo "2. deploy с нуля: HTTPS через certbot"
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=certbot ENABLE_UFW=yes run "$SB/o2" deploy --yes; rc=$?
check "deploy завершился успешно" test "$rc" -eq 0
check "настройки FM с правами 600" test "$(stat -c %a "$RFM_ENV")" = 600
check "адрес сайта записан" test "$(val "$RFM_ENV" PUBLIC_BASE_URL)" = https://files.example.com
check "хеш пароля bcrypt" has_line "$RFM_ENV" "ADMIN_PASSWORD_HASH='\$2b\$12\$"
check "в настройках нет повторов" test -z "$(cut -d= -f1 "$RFM_ENV" | grep -v '^#' | sort | uniq -d)"
check "мини-приложение само не включается" no_line "$RFM_ENV" "MINIAPP_ENABLED"
check "служба FM запущена" test -f "$SB/state/active-rust-file-manager.service"
check "nginx: свой домен и лимит 201M" has_line "$NGINX_DIR/sites-available/rust-file-manager" "server_name files.example.com;"
check "nginx: сайт включён" test -L "$NGINX_DIR/sites-enabled/rust-file-manager"
check "certbot выпустил сертификат" test -d "$LE_DIR/live/files.example.com"
check "ufw: открыт порт SSH текущего подключения" has_line "$SB/calls.log" "ufw allow 22/tcp"
check "ufw: открыт 443" has_line "$SB/calls.log" "ufw allow 443/tcp"
check "ufw включён" has_line "$SB/calls.log" "ufw --force enable"
check "состояние: каталог сборки" test "$(val "$STATE_FILE" RFM_BUILD_DIR)" = "$BUILD_ROOT/rfm-build"
check "состояние: способ HTTPS" test "$(val "$STATE_FILE" HTTPS_MODE)" = certbot
check "Docker не вызывается" no_line "$SB/calls.log" "docker"
check "бот не упоминается" no_line "$SB/o2" "TelegramOnly"
check "подсказка про вход администратора" has_line "$SB/o2" "https://files.example.com/login"
check "в выводе нет секретов" no_secrets "$SB/o2"
check "секретов нет и в аргументах команд" no_secrets "$SB/calls.log"

echo "3. Повторный deploy на готовом сервере"
apt_before=$(count "$SB/calls.log" "apt-get")
run "$SB/o3" deploy --yes; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "сообщает, что всё установлено" has_line "$SB/o3" "Файловый менеджер уже установлен"
check "пакеты не ставит" test "$(count "$SB/calls.log" "apt-get")" -eq "$apt_before"

echo "4. post_deploy без новых версий"
restarts=$(count "$SB/calls.log" "systemctl restart rust-file-manager.service")
run "$SB/o4" post_deploy; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "уже последняя версия" has_line "$SB/o4" "уже последняя версия"
check "FM не перезапускался" test "$(count "$SB/calls.log" "systemctl restart rust-file-manager.service")" -eq "$restarts"

echo "5. post_deploy --check при новой версии"
rfm_release 1.8.0
bin_before=$(md5sum <"$RFM_BIN")
run "$SB/o5" post_deploy --check; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "видит обновление FM" has_line "$SB/o5" "файловый менеджер: есть обновление"
check "программу не трогает" test "$(md5sum <"$RFM_BIN")" = "$bin_before"

echo "6. post_deploy обновляет файловый менеджер"
run "$SB/o6" post_deploy; rc=$?
check "завершился успешно" test "$rc" -eq 0
check "запущен FM 1.8.0" has_line "$RFM_BIN" "version=1.8.0"
check "проверки после запуска прошли" has_line "$SB/o6" "запущен 1.8.0"
check "копия прежней программы сохранена" compgen -G "$RFM_BACKUPS/rust-file-manager-1.7.0-*"
check "каталог копий закрыт (700)" test "$(stat -c %a "$RFM_BACKUPS")" = 700
check "итог: FM 1.7.0 → 1.8.0" has_line "$SB/o6" "Файловый менеджер: 1.7.0 → 1.8.0"
check "в выводе нет секретов" no_secrets "$SB/o6"

echo "6б. --ref закрепляет выпуск"
git -C "$SB/src/rfm" tag v1.8.0 && git -C "$SB/src/rfm" push -q "$SB/repos/rfm.git" v1.8.0
run "$SB/o6b" post_deploy --ref v1.8.0 --force; rc=$?
check "post_deploy --ref v1.8.0 успешен" test "$rc" -eq 0
check "выпуск запомнен" test "$(val "$STATE_FILE" RFM_REF)" = v1.8.0
run "$SB/o6c" post_deploy --only fm --ref main; rc=$?
check "прежний --only fm принимается" test "$rc" -eq 0
check "возврат на main запомнен" test "$(val "$STATE_FILE" RFM_REF)" = main
run "$SB/o6d" post_deploy --only bot; rc=$?
check "--only bot — ошибка" test "$rc" -eq 1
check "объяснено, что команда только для FM" has_line "$SB/o6d" "только файловый менеджер"

echo "7. Неудачное обновление FM откатывается"
rfm_release 1.8.1 BROKEN
run "$SB/o7" post_deploy; rc=$?
check "post_deploy сообщает об ошибке" test "$rc" -eq 1
check "в итоге — откат" has_line "$SB/o7" "ОТКАТ на 1.8.0"
check "на месте снова программа 1.8.0" has_line "$RFM_BIN" "version=1.8.0"
check "служба снова работает" test -f "$SB/state/active-rust-file-manager.service"

echo "8. setup и самообновление команд"
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

echo "9. Полная версия (вместе с ботом) не заменяется"
for name in deploy post_deploy post-deploy; do ln -sfn tgo-vps "$CMD_DIR/$name"; done
run "$SB/o9" setup; rc=$?
check "setup поверх полной версии — ошибка" test "$rc" -eq 1
check "ссылки на полную версию сохранены" test "$(readlink "$CMD_DIR/deploy")" = tgo-vps
check "объяснено, что полная версия уже обновляет FM" has_line "$SB/o9" "полная версия команд"
mkdir -p "$(dirname "$FULL_SCRIPT")" && printf '# tgo-vps\n' >"$FULL_SCRIPT"
run "$SB/o9s" status
check "при клоне бота — подсказка о полной версии" has_line "$SB/o9s" "bash $FULL_SCRIPT setup"
mkdir -p "$SB/opt/clone/.git"
FULL_SCRIPT=$SB/opt/clone/scripts/tgo-vps.sh run "$SB/o9c" status
check "прежний клон без полной версии — подсказка обновить его" has_line "$SB/o9c" "git -C $SB/opt/clone pull --ff-only"

# ============================================================================
echo "10. deploy с нуля: HTTPS через Cloudflare"
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=cloudflare ENABLE_UFW=yes run "$SB/o11" deploy --yes; rc=$?
check "deploy завершился успешно" test "$rc" -eq 0
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

echo "11. Сборка упала — программа не тронута"
rfm_release 1.9.0
touch "$SB/src/rfm/FAIL_BUILD"; git_commit_all "$SB/src/rfm" "fail" && git -C "$SB/src/rfm" push -q "$SB/repos/rfm.git" main
bin_before=$(md5sum <"$RFM_BIN")
run "$SB/o13" post_deploy; rc=$?
check "post_deploy сообщает об ошибке" test "$rc" -eq 1
check "показывает журнал сборки" has_line "$SB/o13" "error: fake failure"
check "программа не изменилась" test "$(md5sum <"$RFM_BIN")" = "$bin_before"

echo "12. deploy с ответами на вопросы (без --yes)"
new_sandbox
# домен (сначала неверный), HTTPS (Enter = certbot), почта (Enter), логин (Enter),
# пароль: короткий и ещё раз, затем верный дважды; ufw (Enter = да); «Начинаем?» (Enter)
printf '%s\n' "not a domain" files.example.com "" "" "" short short Secret-pass-1 Secret-pass-1 "" "" |
  run "$SB/o14" deploy; rc=$?
check "deploy завершился успешно" test "$rc" -eq 0
check "неверный домен переспрошен" has_line "$SB/o14" "Это не похоже на домен"
check "короткий пароль переспрошен" has_line "$SB/o14" "Пароли короче 8 символов"
check "выбран certbot по умолчанию" test "$(val "$STATE_FILE" HTTPS_MODE)" = certbot
check "логин по умолчанию admin" test "$(val "$RFM_ENV" ADMIN_USERNAME)" = admin
check "ufw включён по умолчанию" has_line "$SB/calls.log" "ufw --force enable"
check "в выводе нет пароля" no_secrets "$SB/o14"

echo "13. HTTPS через свой прокси: nginx не нужен"
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/o15" deploy --yes; rc=$?
check "deploy успешен" test "$rc" -eq 0
check "nginx не ставится" no_line "$SB/calls.log" " nginx "
check "сайты nginx не создаются" test -z "$(ls "$NGINX_DIR/sites-available")"
check "подсказка направить свой прокси" has_line "$SB/o15" "http://127.0.0.1:8080"

echo
echo "Пройдено: $PASS, ошибок: $FAILED"
(( FAILED == 0 ))
