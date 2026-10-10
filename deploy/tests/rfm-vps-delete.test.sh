#!/usr/bin/env bash
# Команда delete: удаление файлового менеджера с копией и вопросы об общем.
# Синтетические заглушки: настоящие службы, файлы и файрвол не затрагиваются.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source <(sed '/^# =====/q' "$HERE/rfm-vps.test.sh")
HOST=$(hostname)

sandbox() {
  new_sandbox
  export DELETE_BACKUPS=$SB/var/backups/rfm-delete
  stub dpkg 'exit 1'  # пакеты nginx/certbot «не установлены»: без вопроса об их удалении
}
install_fm() {  # режим HTTPS
  printf 'Secret-pass-1\nSecret-pass-1\n' |
    RFM_DOMAIN=files.example.com HTTPS_MODE=$1 ENABLE_UFW=no run "$SB/install" deploy --yes
}
copy_dir() { find "$DELETE_BACKUPS" -mindepth 1 -maxdepth 1 -type d -name '*-delete-*' | head -1; }
in_copy() { test -e "$(copy_dir)/files$1"; }

echo '1. --dry-run показывает план и ничего не меняет'
sandbox
install_fm certbot
run "$SB/dry" delete --dry-run; rc=$?
check 'просмотр успешен' test "$rc" -eq 0
check 'файловый менеджер в списке' has_line "$SB/dry" 'Файловый менеджер rust-file-manager'
check 'в плане перенос данных в копию' has_line "$SB/dry" 'данные'
check 'сказано, что ничего не изменено' has_line "$SB/dry" 'ничего не изменено'
check 'программа на месте' test -x "$RFM_BIN"
check 'копия не создана' test ! -e "$DELETE_BACKUPS"

echo '2. Отказ и неверное имя сервера — ничего не удалено'
printf '\n' | run "$SB/no" delete; rc=$?
check 'Enter на вопросе — ничего не выбрано' has_line "$SB/no" 'Ничего не выбрано'
printf 'д\nне-тот-сервер\n' | run "$SB/wrong" delete; rc=$?
check 'неверное имя — отказ' test "$rc" -eq 1
check 'объяснение' has_line "$SB/wrong" 'Имя не совпало'
check 'программа на месте' test -x "$RFM_BIN"
check 'служба работает' test -f "$SB/state/active-$RFM_UNIT"
run "$SB/yes" delete --yes; rc=$?
check '--yes не принимается' test "$rc" -eq 1
run "$SB/bot" delete --only bot; rc=$?
check '--only bot — ошибка' test "$rc" -eq 1

echo '3. Удаление: копия, сайт nginx, общее по вопросам'
mkdir -p "$RFM_DATA/uploads" && echo keep >"$RFM_DATA/uploads/keep.txt"
: >"$SWAP_FILE"; printf '%s none swap sw 0 0\n' "$SWAP_FILE" >>"$FSTAB"
: >"$SB/calls.log"
# удалить, имя, сертификат — да, сборка и Rust — да, swap — да, сами команды — нет
printf 'д\n%s\nд\nд\nд\nн\n' "$HOST" | run "$SB/del" delete; rc=$?
check 'удаление успешно' test "$rc" -eq 0
check 'служба остановлена и выключена' has_line "$SB/calls.log" "systemctl disable --now $RFM_UNIT"
check 'программа ушла в копию' in_copy "$RFM_BIN"
check 'загруженные файлы в копии' in_copy "$RFM_DATA/uploads/keep.txt"
check 'настройки в копии' in_copy "$RFM_ENV"
check 'unit в копии' in_copy "$UNIT_DIR/$RFM_UNIT"
check 'настройки команд в копии' in_copy "$STATE_FILE"
check 'сайт nginx выключен' test ! -e "$NGINX_DIR/sites-enabled/rust-file-manager"
check 'файл сайта в копии' in_copy "$NGINX_DIR/sites-available/rust-file-manager"
check 'nginx перечитан' has_line "$SB/calls.log" 'systemctl reload nginx'
check 'копия только для root' test "$(stat -c %a "$(copy_dir)")" = 700
check 'сертификат удалён через certbot' has_line "$SB/calls.log" 'certbot delete --cert-name files.example.com'
check 'каталог сборки удалён' test ! -d "$BUILD_ROOT/rfm-build"
check 'Rust удалён' has_line "$SB/calls.log" 'rustup self uninstall -y'
check 'swap-файл удалён' test ! -e "$SWAP_FILE"
check 'swap убран из fstab' no_line "$FSTAB" "$SWAP_FILE"
check 'в итоге путь копии' has_line "$SB/del" 'Копия:'
check 'подсказка, как удалить копию насовсем' has_line "$SB/del" 'rm -rf'

echo '4. Без вопросов: --only fm --confirm ИМЯ, общее не трогается'
sandbox
install_fm certbot
run "$SB/auto" delete --only fm --confirm "$HOST" </dev/null; rc=$?
check 'удаление успешно' test "$rc" -eq 0
check 'программа ушла в копию' in_copy "$RFM_BIN"
check 'сертификат не тронут' no_line "$SB/calls.log" 'certbot delete'
check 'каталог сборки на месте' test -d "$BUILD_ROOT/rfm-build"
sandbox
install_fm none
run "$SB/bad" delete --only fm --confirm не-тот-сервер </dev/null; rc=$?
check 'чужое имя в --confirm — отказ' test "$rc" -eq 1
check 'программа на месте' test -x "$RFM_BIN"

echo '5. ufw: закрыть порты, которые больше никто не слушает'
sandbox
install_fm certbot
stub ss 'cat "$SB/state/ss.txt" 2>/dev/null; exit 0'
printf 'tcp LISTEN 0 511 0.0.0.0:443 0.0.0.0:* users:(("nginx",pid=1,fd=6))\n' >"$SB/state/ss.txt"
stub nginx '[[ ${1:-} == -t ]] && sed -i "/:443 /d" "$SB/state/ss.txt"; exit 0'
stub ufw '
case ${1:-} in
  status) echo "Status: active"; echo "443/tcp                    ALLOW       Anywhere" ;;
esac
exit 0'
# удалить, имя, сертификат — нет, сборка — нет, порт — закрыть, команды — нет
printf 'д\n%s\nн\nн\nд\nн\n' "$HOST" | run "$SB/ufw" delete; rc=$?
check 'удаление успешно' test "$rc" -eq 0
check 'вопрос о порте задан' has_line "$SB/ufw" 'Закрыть в ufw порты удалённых служб: 443/tcp'
check 'правило удалено' has_line "$SB/calls.log" 'ufw delete allow 443/tcp'
check 'других правил не трогали' test "$(count "$SB/calls.log" 'ufw delete')" -eq 1

echo '6. setup ставит команду delete'
sandbox
run "$SB/setup" setup; rc=$?
check 'setup успешен' test "$rc" -eq 0
check 'delete — ссылка на rfm-vps' test "$(readlink "$CMD_DIR/delete")" = rfm-vps

install_fm none
rm -f "$CMD_DIR/delete"
bash "$CMD_DIR/post_deploy" --yes >"$SB/relink" 2>&1
check 'post_deploy добавляет ссылку delete' test "$(readlink "$CMD_DIR/delete")" = rfm-vps
sandbox
printf 'чужая\n' >"$CMD_DIR/delete"
run "$SB/foreign" setup; rc=$?
check 'чужой delete не мешает setup' test "$rc" -eq 0
check 'чужой delete сохранён' has_line "$CMD_DIR/delete" 'чужая'
check 'подсказка, как вызвать удаление' has_line "$SB/foreign" 'rfm-vps delete'
check 'deploy всё равно установлен' test "$(readlink "$CMD_DIR/deploy")" = rfm-vps

echo '7. Сервер с ботом: delete переходит на полную версию, если та умеет удалять'
sandbox
install_fm none
run "$SB/setup" setup
clone=$SB/opt/TelegramOnly
git init -q -b main "$clone" && mkdir -p "$clone/scripts"
cat >"$clone/scripts/tgo-vps.sh" <<'EOF'
#!/usr/bin/env bash
# tgo-vps — synthetic full version for the handover test
case $1 in
  setup) install -m 755 "$0" "$CMD_DIR/tgo-vps"
         for n in deploy post_deploy post-deploy delete; do ln -sfn tgo-vps "$CMD_DIR/$n"; done ;;
  *) echo "FULL VERSION RAN $*" ;;
esac
EOF
git_commit_all "$clone" init
export FULL_SCRIPT=$clone/scripts/tgo-vps.sh
printf '\n' | bash "$CMD_DIR/delete" >"$SB/old-full" 2>&1
check 'полная версия без delete — предупреждение' has_line "$SB/old-full" 'пока без delete'
check 'дальше работает публичная delete' has_line "$SB/old-full" 'Что можно удалить'
check 'ссылки остаются на публичной версии' test "$(readlink "$CMD_DIR/delete")" = rfm-vps
printf '# cmd_delete\n' >>"$clone/scripts/tgo-vps.sh"; git_commit_all "$clone" delete
printf '\n' | bash "$CMD_DIR/delete" >"$SB/new-full" 2>&1
check 'переход на полную версию с теми же параметрами' has_line "$SB/new-full" 'FULL VERSION RAN delete'
unset FULL_SCRIPT

printf '\nПройдено: %s, ошибок: %s\n' "$PASS" "$FAILED"
(( FAILED == 0 ))
