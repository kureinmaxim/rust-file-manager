#!/usr/bin/env bash
# Update/relink failures must preserve the already running bot and its protocols.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source <(sed '/^# =====/q' "$HERE/rfm-vps.test.sh")

new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none BOT_NETWORK=host ENABLE_UFW=no run "$SB/install" deploy --yes

echo '1. post_deploy не чистит общие Docker-образы'
: >"$SB/calls.log"
run "$SB/force" post_deploy --only bot --force --yes; rc=$?
check 'обычное обновление бота успешно' test "$rc" -eq 0
check 'общие образы не удаляются' no_line "$SB/calls.log" 'image prune'
check 'сеть host сохранена' test "$(cat "$SB/state/docker/net")" = host

echo '2. Защита конфигов при пересоздании связки'
printf 'raise SystemExit(1)\n' >"$BOT_DIR/scripts/rebuild_guard.py"
sed -i 's/^FILES_SERVICE_TOKEN=.*/FILES_SERVICE_TOKEN=synthetic-different-key/' "$BOT_DIR/.env"
: >"$SB/calls.log"
run "$SB/guard-fail" deploy --yes; rc=$?
check 'отказ проверки возвращает ошибку' test "$rc" -eq 1
check 'бот не пересоздаётся при отказе' no_line "$SB/calls.log" 'compose up'
check 'работающая версия сохранена' test "$(cat "$SB/state/docker/version")" = 3.25.0
check 'работающая сеть сохранена' test "$(cat "$SB/state/docker/net")" = host

echo '3. Неожиданная смена сети не считается успешным обновлением'
printf '\necho telegramonly_default >"$SB/state/docker/net"\n' >>"$SB/src/bot/scripts/rebuild_bot.sh"
git_commit_all "$SB/src/bot" 'synthetic network drift'
git -C "$SB/src/bot" push -q "$SB/repos/bot.git" main
run "$SB/network-drift" post_deploy --only bot --force --yes; rc=$?
check 'смена сети возвращает ошибку' test "$rc" -eq 1
check 'смена сети явно указана' has_line "$SB/network-drift" 'сеть бота изменилась'

printf '\nПройдено: %s, ошибок: %s\n' "$PASS" "$FAILED"
(( FAILED == 0 ))
