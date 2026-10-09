#!/usr/bin/env bash
# Дополнительные сценарии используют прежние заглушки, не изменяя старые тесты.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source <(sed '/^# =====/q' "$HERE/rfm-vps.test.sh")

echo '1. Только файловый менеджер: установка, проверка и обновление'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --only fm --yes; rc=$?
check 'FM устанавливается без бота' test "$rc" -eq 0
check 'Docker не вызывается для установки FM' no_line "$SB/calls.log" 'docker compose'
check 'связка не обещается без бота' no_line "$SB/install" '• связать бота'
rfm_release 1.8.0
run "$SB/update" post_deploy --only fm --yes; rc=$?
check 'FM обновляется отдельно' test "$rc" -eq 0
check 'новая версия FM установлена' has_line "$RFM_BIN" 'version=1.8.0'
run "$SB/missing" post_deploy --only bot --yes; rc=$?
check 'выбор отсутствующего бота — ошибка' test "$rc" -eq 1
check 'есть команда установки бота' has_line "$SB/missing" 'deploy --only bot'
rm -f "$SB/state/active-$RFM_UNIT"
run "$SB/stopped" post_deploy --only fm --yes; rc=$?
check 'та же версия восстанавливает остановленную службу' test "$rc" -eq 0
check 'служба снова активна' test -f "$SB/state/active-$RFM_UNIT"

echo '2. Только бот: обновление и код без git'
new_sandbox
BOT_NETWORK=host ENABLE_UFW=no run "$SB/install" deploy --only bot --yes; rc=$?
check 'бот устанавливается отдельно' test "$rc" -eq 0
bot_release 3.26.0
run "$SB/update" post_deploy --only bot --yes; rc=$?
check 'бот обновляется без FM' test "$rc" -eq 0
check 'сеть host сохранена' test "$(cat "$SB/state/docker/net")" = host
mv "$BOT_DIR/.git" "$SB/bot-git-backup"
run "$SB/no-git" post_deploy --yes; rc=$?
check 'невозможность обновить код без git — ошибка' test "$rc" -eq 1
check 'причина пропуска видна в итоге' has_line "$SB/no-git" 'код доставлен без git'

echo '3. EOF и неполные аргументы: быстрый отказ'
new_sandbox
timeout 5 bash "$SCRIPT" deploy </dev/null >"$SB/eof" 2>&1; rc=$?
check 'EOF не создаёт бесконечный опрос' test "$rc" -eq 1
timeout 5 env RFM_DOMAIN=files.example.com HTTPS_MODE=none BOT_NETWORK=host ENABLE_UFW=no \
  bash "$SCRIPT" deploy --yes </dev/null >"$SB/password-eof" 2>&1; rc=$?
check 'EOF пароля завершает команду' test "$rc" -eq 1
for option in --ref --only; do
  run "$SB/arg" deploy "$option"; rc=$?
  check "$option без значения — ошибка" test "$rc" -eq 1
done
INSTALL_BOT=invalid run "$SB/invalid" deploy --yes; rc=$?
check 'неверный INSTALL_BOT отвергается' test "$rc" -eq 1

echo '4. --check не обновляет команды и учитывает --only'
new_sandbox
BOT_NETWORK=host ENABLE_UFW=no run "$SB/install" deploy --only bot --yes
run "$SB/setup" setup
printf '\n# новая версия команд\n' >>"$SB/raw/main/deploy/rfm-vps.sh"
before=$(sha256sum <"$CMD_DIR/rfm-vps")
bash "$CMD_DIR/post_deploy" --check --only bot >"$SB/check" 2>&1; rc=$?
check '--check успешен' test "$rc" -eq 0
check 'установленный скрипт не изменён' test "$(sha256sum <"$CMD_DIR/rfm-vps")" = "$before"
check 'самообновление не выполнялось' no_line "$SB/check" 'Команды deploy/post_deploy обновлены'
check 'отсутствующий FM не проверяется' no_line "$SB/check" 'файловый менеджер: есть обновление'
bash "$CMD_DIR/post_deploy" --only bot --yes >"$SB/self-update" 2>&1; rc=$?
check 'обычное обновление обновляет команды' has_line "$SB/self-update" 'Команды deploy/post_deploy обновлены'
check 'прежний скрипт сохранён' compgen -G "$CMD_DIR/rfm-vps.backup.*"

echo '5. Ошибки nginx и перезапуска связки не скрываются'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none BOT_NETWORK=host ENABLE_UFW=no run "$SB/install" deploy --yes
stub nginx 'exit 1'
rm -f "$NGINX_DIR/sites-enabled/rust-file-manager"
RFM_DOMAIN=files.example.com HTTPS_MODE=certbot run "$SB/nginx-fail" deploy --yes; rc=$?
check 'ошибка nginx возвращает ненулевой код' test "$rc" -eq 1
stub nginx 'exit 0'
stub docker '
d=$SB/state/docker
case ${1:-} in
  inspect) [[ -f $d/container ]] || exit 1; [[ ${2:-} == -f ]] && { case $3 in *NetworkMode*) echo host ;; *config_files*) echo "$BOT_DIR/compose.yaml" ;; esac; }; exit 0 ;;
  exec) printf "version = \"3.25.0\"\n"; exit 0 ;;
  compose) exit 1 ;;
esac
exit 0'
sed -i 's/^FILES_SERVICE_TOKEN=.*/FILES_SERVICE_TOKEN=wrong/' "$BOT_DIR/.env"
HTTPS_MODE=none run "$SB/link-fail" deploy --yes; rc=$?
check 'ошибка пересоздания бота возвращается наружу' test "$rc" -eq 1
check 'нет ложного сообщения об успешной связке' no_line "$SB/link-fail" 'Связка: настройки записаны'

echo '6. --only не исправляет и не перезапускает второй проект'
write_stubs
rfm_release 1.8.0
sed -i 's/^FILES_SERVICE_TOKEN=.*/FILES_SERVICE_TOKEN=wrong/' "$BOT_DIR/.env"
before=$(sha256sum <"$BOT_DIR/.env")
run "$SB/only" post_deploy --only fm --yes; rc=$?
check 'обновление FM успешно' test "$rc" -eq 0
check 'настройки бота не изменились' test "$(sha256sum <"$BOT_DIR/.env")" = "$before"
check 'связка не запускается' no_line "$SB/only" 'Связка бота и файлового менеджера'

echo '7. При сбое FM зависимое обновление бота откладывается'
rfm_release 1.8.1 BROKEN; bot_release 3.26.0
run "$SB/failed-fm" post_deploy --yes; rc=$?
check 'общая команда завершилась ошибкой' test "$rc" -eq 1
check 'версия бота сохранена' test "$(cat "$SB/state/docker/version")" = 3.25.0
check 'зависимое обновление явно пропущено' has_line "$SB/failed-fm" 'зависимое обновление бота пропущено'

echo '8. setup сохраняет чужие команды'
new_sandbox
printf 'чужая команда\n' >"$CMD_DIR/deploy"
before=$(sha256sum <"$CMD_DIR/deploy")
run "$SB/conflict" setup; rc=$?
check 'конфликт имени — ошибка' test "$rc" -eq 1
check 'чужая команда сохранена' test "$(sha256sum <"$CMD_DIR/deploy")" = "$before"

echo '9. Настоящий терминал: выбор проекта и отмена плана'
new_sandbox
export ENABLE_UFW=no
check 'интерактивный выбор только бота' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" install_bot
check 'FM не установлен при выборе bot' test ! -e "$RFM_BIN"
bot_release 3.26.0
check 'отмена обновления в терминале' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" cancel_bot
check 'отмена сохраняет версию' test "$(cat "$SB/state/docker/version")" = 3.25.0
check 'подтверждение обновления в терминале' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" update_bot
check 'подтверждённая версия установлена' test "$(cat "$SB/state/docker/version")" = 3.26.0

new_sandbox
export RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no RFM_ADMIN_LOGIN=admin
check 'интерактивный выбор FM и скрытый пароль' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" install_fm
BOT_NETWORK=host run "$SB/add-bot" deploy --yes
check 'оба установлены: выбор FM, архив и отмена' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" cancel_both

echo '10. Неполная связка и ключ без передачи в аргументы процессов'
sed -i '/^FILES_MINIAPP_URL=/d; s/^FILES_INTERNAL_URL=.*/FILES_INTERNAL_URL=/' "$BOT_DIR/.env"
run "$SB/partial-link" deploy --yes; rc=$?
check 'неполная связка восстановлена' test "$rc" -eq 0
check 'адрес API заполнен' test "$(val "$BOT_DIR/.env" FILES_INTERNAL_URL)" = http://127.0.0.1:8091
check 'адрес мини-приложения заполнен' test "$(val "$BOT_DIR/.env" FILES_MINIAPP_URL)" = https://files.example.com/tg/
check 'в env нет дублей адреса API' test "$(count "$BOT_DIR/.env" 'FILES_INTERNAL_URL=')" -eq 1
sed -i 's/^FILES_SERVICE_TOKEN=.*/FILES_SERVICE_TOKEN=wrong/' "$BOT_DIR/.env"
stub sed 'exec /usr/bin/sed "$@"'
run "$SB/safe-link" deploy --yes; rc=$?
check 'ключ синхронизирован' test "$rc" -eq 0
check 'в аргументах заглушек нет секретов' no_secrets "$SB/calls.log"

echo '11. Запрошенный архив обязателен до замены программы'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --only fm --yes
rfm_release 1.8.0
before=$(sha256sum <"$RFM_BIN")
stub tar 'exit 1'
run "$SB/archive-fail" post_deploy --only fm --backup-data --yes; rc=$?
check 'ошибка архивации возвращается наружу' test "$rc" -eq 1
check 'старая программа сохранена' test "$(sha256sum <"$RFM_BIN")" = "$before"
check 'старая служба снова запущена' test -f "$SB/state/active-$RFM_UNIT"

echo '12. Бот systemd: обновление и сбой зависимостей'
new_sandbox
git clone -q "$BOT_REPO_URL" "$BOT_DIR"
printf '[Service]\n' >"$UNIT_DIR/telegramonly.service"
cp "$BOT_DIR/example.env" "$BOT_DIR/.env"
bot_release 3.26.0
run "$SB/systemd-update" post_deploy --only bot --yes; rc=$?
check 'systemd-бот обновился' test "$rc" -eq 0
check 'systemd-бот запущен' test -f "$SB/state/active-telegramonly.service"
check 'Docker для обновления systemd не использован' no_line "$SB/calls.log" 'docker compose'
mkdir -p "$BOT_DIR/venv/bin"
printf '#!/usr/bin/env bash\nexit 1\n' >"$BOT_DIR/venv/bin/pip"
chmod 755 "$BOT_DIR/venv/bin/pip"
printf 'new-synthetic-dependency\n' >>"$SB/src/bot/requirements.txt"
bot_release 3.27.0
before=$(count "$SB/calls.log" 'systemctl restart telegramonly.service')
run "$SB/pip-fail" post_deploy --only bot --yes; rc=$?
check 'ошибка pip возвращается наружу' test "$rc" -eq 1
check 'бот со старыми зависимостями не перезапускается' test "$(count "$SB/calls.log" 'systemctl restart telegramonly.service')" -eq "$before"

printf '\nПройдено: %s, ошибок: %s\n' "$PASS" "$FAILED"
(( FAILED == 0 ))
