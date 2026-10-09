#!/usr/bin/env bash
# Migration of existing nginx sites, including edits made manually before update.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source <(sed '/^# =====/q' "$HERE/rfm-vps.test.sh")

old_upload_location() {
  sed -i 's@location /api/v1/uploads {@location /api/v1/uploads/ {@' "$1"
}

echo '1. Новая установка и обновление прежнего сайта certbot'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=certbot ENABLE_UFW=no run "$SB/install" deploy --yes
site=$NGINX_DIR/sites-available/rust-file-manager
check 'новый сайт не перенаправляет URL коллекции' has_line "$site" 'location /api/v1/uploads {'
old_upload_location "$site"
cp -p "$site" "$SB/old-site"
sed 's@location /api/v1/uploads/ {@location /api/v1/uploads {@' "$site" >"$SB/expected-site"
run "$SB/check" post_deploy --check --only fm; rc=$?
check '--check успешен' test "$rc" -eq 0
check '--check не изменяет nginx' cmp -s "$site" "$SB/old-site"
: >"$SB/calls.log"
run "$SB/update" post_deploy --only fm --yes; rc=$?
check 'обновление FM исправляет старый сайт' test "$rc" -eq 0
check 'изменён только слеш маршрута' cmp -s "$site" "$SB/expected-site"
check 'конфигурация проверена' has_line "$SB/calls.log" 'nginx -t'
check 'nginx перечитал конфигурацию' has_line "$SB/calls.log" 'systemctl reload nginx'
check 'обновление FM не вызывает Docker' no_line "$SB/calls.log" 'docker'
: >"$SB/calls.log"
run "$SB/repeat" post_deploy --only fm --yes; rc=$?
check 'повторное обновление успешно' test "$rc" -eq 0
check 'уже исправленный сайт сохранён' cmp -s "$site" "$SB/expected-site"
check 'ручная правка также применяется через reload' has_line "$SB/calls.log" 'systemctl reload nginx'

echo '2. Свой прокси и неуправляемые сайты'
old_upload_location "$site"
: >"$SB/calls.log"
HTTPS_MODE=none run "$SB/own-proxy" post_deploy --only fm --yes; rc=$?
check 'свой прокси не изменён' cmp -s "$site" "$SB/old-site"
rm -f "$NGINX_DIR/sites-enabled/rust-file-manager"
run "$SB/inactive" post_deploy --only fm --yes; rc=$?
check 'неактивный сайт не изменён' cmp -s "$site" "$SB/old-site"
ln -s "$site" "$NGINX_DIR/sites-enabled/rust-file-manager"

echo '3. Неудачная проверка/reload: восстановление прежней конфигурации'
stub nginx 'exit 1'
run "$SB/invalid" post_deploy --yes; rc=$?
check 'ошибка nginx возвращается наружу' test "$rc" -eq 1
check 'невалидный патч откатывается' cmp -s "$site" "$SB/old-site"
write_stubs
stub systemctl '[[ ${1:-} == reload && ${2:-} == nginx ]] && exit 1; exit 0'
run "$SB/reload-fail" post_deploy --only fm --yes; rc=$?
check 'ошибка reload возвращается наружу' test "$rc" -eq 1
check 'после ошибки reload сохранена прежняя конфигурация' cmp -s "$site" "$SB/old-site"
write_stubs
run "$SB/redeploy" deploy --yes; rc=$?
check 'повторный deploy исправляет прежний сайт' test "$rc" -eq 0
check 'deploy сохраняет остальную конфигурацию' cmp -s "$site" "$SB/expected-site"
check 'временные копии удалены' test -z "$(find "$NGINX_DIR" -maxdepth 1 -name '.rfm-uploads.*' -print)"

echo '4. Прежний Cloudflare SSL-сайт'
cf=$NGINX_DIR/sites-available/rust-file-manager-ssl
sed -e 's/location \/api\/v1\/uploads {/location \/api\/v1\/uploads\/ {/' -e 's/listen 80;/listen 2083 ssl;/' "$site" >"$cf"
ln -s "$cf" "$NGINX_DIR/sites-enabled/rust-file-manager-ssl"
sed 's@location /api/v1/uploads/ {@location /api/v1/uploads {@' "$cf" >"$SB/expected-cf"
HTTPS_MODE=cloudflare run "$SB/cloudflare" post_deploy --only fm --yes; rc=$?
check 'обновление Cloudflare успешно' test "$rc" -eq 0
check 'SSL-сайт исправлен без изменения остальных строк' cmp -s "$cf" "$SB/expected-cf"
check 'в выводе нет секретов' no_secrets "$SB/cloudflare"

printf '\nПройдено: %s, ошибок: %s\n' "$PASS" "$FAILED"
(( FAILED == 0 ))
