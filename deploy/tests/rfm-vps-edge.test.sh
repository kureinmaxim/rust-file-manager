#!/usr/bin/env bash
# Дополнительные сценарии используют прежние заглушки, не изменяя старые тесты.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source <(sed '/^# =====/q' "$HERE/rfm-vps.test.sh")

echo '1. Установка, обновление и остановленная служба'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --yes; rc=$?
check 'FM устанавливается' test "$rc" -eq 0
check 'Docker не вызывается' no_line "$SB/calls.log" 'docker'
check 'связка с ботом не обещается' no_line "$SB/install" 'связать бота'
rfm_release 1.8.0
run "$SB/update" post_deploy --yes; rc=$?
check 'FM обновляется' test "$rc" -eq 0
check 'новая версия FM установлена' has_line "$RFM_BIN" 'version=1.8.0'
rm -f "$SB/state/active-$RFM_UNIT"
run "$SB/stopped" post_deploy --yes; rc=$?
check 'та же версия восстанавливает остановленную службу' test "$rc" -eq 0
check 'служба снова активна' test -f "$SB/state/active-$RFM_UNIT"

echo '2. EOF и неполные аргументы: быстрый отказ'
new_sandbox
timeout 5 bash "$SCRIPT" deploy </dev/null >"$SB/eof" 2>&1; rc=$?
check 'EOF не создаёт бесконечный опрос' test "$rc" -eq 1
timeout 5 env RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no \
  bash "$SCRIPT" deploy --yes </dev/null >"$SB/password-eof" 2>&1; rc=$?
check 'EOF пароля завершает команду' test "$rc" -eq 1
for option in --ref --only; do
  run "$SB/arg" deploy "$option"; rc=$?
  check "$option без значения — ошибка" test "$rc" -eq 1
done
HTTPS_MODE=invalid RFM_DOMAIN=files.example.com run "$SB/invalid" deploy --yes; rc=$?
check 'неверный HTTPS_MODE отвергается' test "$rc" -eq 1

echo '3. --check не обновляет команды'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --yes
run "$SB/setup" setup
printf '\n# новая версия команд\n' >>"$SB/raw/main/deploy/rfm-vps.sh"
before=$(sha256sum <"$CMD_DIR/rfm-vps")
bash "$CMD_DIR/post_deploy" --check >"$SB/check" 2>&1; rc=$?
check '--check успешен' test "$rc" -eq 0
check 'установленный скрипт не изменён' test "$(sha256sum <"$CMD_DIR/rfm-vps")" = "$before"
check 'самообновление не выполнялось' no_line "$SB/check" 'Команды deploy/post_deploy обновлены'
bash "$CMD_DIR/post_deploy" --yes >"$SB/self-update" 2>&1; rc=$?
check 'обычное обновление обновляет команды' has_line "$SB/self-update" 'Команды deploy/post_deploy обновлены'
check 'прежний скрипт сохранён' compgen -G "$CMD_DIR/rfm-vps.backup.*"

echo '4. Ошибка nginx не скрывается'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --yes
stub nginx 'exit 1'
RFM_DOMAIN=files.example.com HTTPS_MODE=certbot run "$SB/nginx-fail" deploy --yes; rc=$?
check 'ошибка nginx возвращает ненулевой код' test "$rc" -eq 1

echo '5. setup сохраняет чужие команды'
new_sandbox
printf 'чужая команда\n' >"$CMD_DIR/deploy"
before=$(sha256sum <"$CMD_DIR/deploy")
run "$SB/conflict" setup; rc=$?
check 'конфликт имени — ошибка' test "$rc" -eq 1
check 'чужая команда сохранена' test "$(sha256sum <"$CMD_DIR/deploy")" = "$before"

echo '6. Настоящий терминал: скрытый пароль, архив и отмена плана'
new_sandbox
export RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no RFM_ADMIN_LOGIN=admin
check 'интерактивная установка и скрытый пароль' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" install_fm
rfm_release 1.8.0
check 'отмена обновления в терминале' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" cancel_fm
check 'отмена сохраняет версию' has_line "$RFM_BIN" 'version=1.7.0'
check 'подтверждение обновления в терминале' python3 "$HERE/rfm-vps-tty.py" "$SCRIPT" update_fm
check 'подтверждённая версия установлена' has_line "$RFM_BIN" 'version=1.8.0'
unset RFM_DOMAIN HTTPS_MODE ENABLE_UFW RFM_ADMIN_LOGIN

echo '7. Запрошенный архив обязателен до замены программы'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --yes
rfm_release 1.8.0
before=$(sha256sum <"$RFM_BIN")
stub tar 'exit 1'
run "$SB/archive-fail" post_deploy --backup-data --yes; rc=$?
check 'ошибка архивации возвращается наружу' test "$rc" -eq 1
check 'старая программа сохранена' test "$(sha256sum <"$RFM_BIN")" = "$before"
check 'старая служба снова запущена' test -f "$SB/state/active-$RFM_UNIT"

printf '\nПройдено: %s, ошибок: %s\n' "$PASS" "$FAILED"
(( FAILED == 0 ))
