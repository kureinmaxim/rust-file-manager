#!/usr/bin/env bash
# deploy/post_deploy файлового менеджера на сервере, где уже работают VPN-протоколы.
# Синтетические заглушки: настоящие службы, порты и файрвол не затрагиваются.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source <(sed '/^# =====/q' "$HERE/rfm-vps.test.sh")

# ss читает слушающие сокеты из $SB/state/ss.txt (формат ss -H -tulpn).
vpn_listeners() {
  stub ss '
f=$SB/state/ss.txt; [[ -f $f ]] || exit 0
case "$*" in *u*) cat "$f" ;; *) awk "\$1 == \"tcp\" {sub(/^tcp +/, \"\"); print}" "$f" ;; esac'
  cat >"$SB/state/ss.txt" <<'EOF'
tcp LISTEN 0 4096 *:443 *:* users:(("xray",pid=11,fd=3))
udp UNCONN 0 0 *:443 *:* users:(("hysteria",pid=12,fd=4))
tcp LISTEN 0 4096 0.0.0.0:29999 0.0.0.0:* users:(("mita",pid=13,fd=5))
tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:(("sshd",pid=14,fd=3))
tcp LISTEN 0 2048 127.0.0.1:8000 0.0.0.0:* users:(("python3",pid=15,fd=6))
udp UNCONN 0 0 0.0.0.0:68 0.0.0.0:* users:(("dhclient",pid=16,fd=7))
udp UNCONN 0 0 0.0.0.0:41641 0.0.0.0:* users:(("tailscaled",pid=18,fd=9))
udp UNCONN 0 0 *:3478 *:* users:(("derper",pid=19,fd=3))
tcp LISTEN 0 4096 100.64.0.5:3000 0.0.0.0:* users:(("headplane",pid=20,fd=4))
EOF
}
line_before() { awk -v a="$2" -v b="$3" 'index($0, a) && !x {x = NR} index($0, b) && !y {y = NR} END {exit !(x && y && x < y)}' "$1"; }

echo '1. ufw выключен, VPN работает: --yes не включает файрвол'
new_sandbox
vpn_listeners
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=cloudflare run "$SB/no-ufw" deploy --yes; rc=$?
check 'установка успешна' test "$rc" -eq 0
check 'ufw не включён' no_line "$SB/calls.log" 'ufw --force enable'
check 'работающие службы перечислены' has_line "$SB/no-ufw" '443/udp(hysteria)'
check 'loopback-порт не считается внешним' no_line "$SB/no-ufw" '8000/tcp'

echo '2. Явное ENABLE_UFW=yes сохраняет порты VPN'
new_sandbox
vpn_listeners
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=cloudflare ENABLE_UFW=yes run "$SB/ufw" deploy --yes; rc=$?
check 'установка успешна' test "$rc" -eq 0
check 'ufw включён' has_line "$SB/calls.log" 'ufw --force enable'
for spec in 443/tcp 443/udp 29999/tcp 41641/udp 3478/udp 3000/tcp; do
  check "порт $spec разрешён до включения ufw" line_before "$SB/calls.log" "ufw allow $spec" 'ufw --force enable'
done
check 'loopback-порт не открывается' no_line "$SB/calls.log" 'ufw allow 8000'
check 'DHCP-клиент не открывается' no_line "$SB/calls.log" 'ufw allow 68/udp'

echo '3. Порт 8080 занят (координатор Headscale, XHTTP): программе — свободный порт'
new_sandbox
vpn_listeners
printf 'tcp LISTEN 0 4096 127.0.0.1:8080 0.0.0.0:* users:(("docker-proxy",pid=17,fd=8))\n' >>"$SB/state/ss.txt"
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=cloudflare ENABLE_UFW=no run "$SB/busy" deploy --yes; rc=$?
check 'установка успешна' test "$rc" -eq 0
check 'названы порт и процесс' has_line "$SB/busy" 'Порт 8080 занят (docker-proxy)'
check 'в плане указан выбранный порт' has_line "$SB/busy" 'программа на 127.0.0.1:8081'
check 'программа слушает 8081' test "$(val "$RFM_ENV" BIND_ADDR)" = 127.0.0.1:8081
mkdir -p "$NGINX_DIR/ssl/files.example.com"
printf 'CERT\n' >"$NGINX_DIR/ssl/files.example.com/cert.pem"; printf 'KEY\n' >"$NGINX_DIR/ssl/files.example.com/key.pem"
run "$SB/busy-cf" deploy --yes; rc=$?
check 'nginx проксирует на 8081' has_line "$NGINX_DIR/sites-available/rust-file-manager-ssl" 'proxy_pass http://127.0.0.1:8081;'
check 'на 8080 nginx не ведёт' no_line "$NGINX_DIR/sites-available/rust-file-manager-ssl" '127.0.0.1:8080'

echo '3в. Занятый порт HTTPS по-прежнему останавливает до установки'
new_sandbox
vpn_listeners
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=certbot ENABLE_UFW=no run "$SB/busy443" deploy --yes; rc=$?
check 'certbot при занятом 443 — ошибка' test "$rc" -eq 1
check 'предложен другой способ HTTPS' has_line "$SB/busy443" 'HTTPS_MODE=cloudflare'
check 'пакеты не ставились' no_line "$SB/calls.log" 'apt-get'
check 'сборка не запускалась' test ! -e "$BUILD_ROOT/rfm-build"

echo '4. Обновление на маленьком VPS: подготовка'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/fm" deploy --yes; rc=$?
check 'FM установлен' test "$rc" -eq 0

echo '5. Swap перед сборкой на маленьком VPS'
printf 'MemTotal:        1000000 kB\n' >"$SB/meminfo"
export MEMINFO=$SB/meminfo
stub swapon 'exit 0'
stub swapoff 'exit 0'
stub fallocate 'touch "${!#}"'
stub df 'printf "Filesystem 1M-blocks Used Available Capacity Mounted\n/dev/vda1 20000 5000 %s 25%% /\n" "${DF_AVAIL:-15000}"'
rfm_release 1.8.0
: >"$SB/calls.log"
run "$SB/swap" post_deploy --yes; rc=$?
check 'обновление FM успешно' test "$rc" -eq 0
check 'в плане указан swap' has_line "$SB/swap" 'swap 2 ГБ'
check 'swap создан до сборки' line_before "$SB/calls.log" 'mkswap' 'cargo build'
check 'swap записан в fstab' has_line "$FSTAB" "$SWAP_FILE none swap"
rm -f "$SWAP_FILE"; : >"$FSTAB"
rfm_release 1.9.0
: >"$SB/calls.log"
DF_AVAIL=1000 run "$SB/no-space" post_deploy --yes; rc=$?
check 'мало места: обновление всё равно успешно' test "$rc" -eq 0
check 'мало места: swap не создаётся' no_line "$SB/calls.log" 'fallocate'
check 'мало места: предупреждение' has_line "$SB/no-space" 'swap не создаю'
stub mkswap 'exit 1'
rfm_release 1.9.1
run "$SB/swap-fail" post_deploy --yes; rc=$?
check 'сбой mkswap не мешает обновлению' test "$rc" -eq 0
check 'недописанный swap-файл удалён' test ! -e "$SWAP_FILE"
unset MEMINFO

only_own_units() {  # журнал вызовов: systemctl трогал только свои службы и nginx
  ! grep -E '^systemctl (restart|stop|start|reload|enable|disable|mask)' "$1" |
    grep -vE 'rust-file-manager|nginx|rfm-internal-bridge|telegramonly' | grep -q .
}
df_avail() {  # df показывает ${DF_AVAIL:-15000} МБ свободного места
  stub df 'printf "Filesystem 1M-blocks Used Available Capacity Mounted\n/dev/vda1 9600 9100 %s 95%% /\n" "${DF_AVAIL:-15000}"'
}
mesh_listeners() {  # VPN + Tailscale/DERP/Headplane + Headscale на 127.0.0.1:8080 + WireGuard ядра
  vpn_listeners
  printf 'tcp LISTEN 0 4096 127.0.0.1:8080 0.0.0.0:* users:(("docker-proxy",pid=17,fd=8))\n' >>"$SB/state/ss.txt"
  printf 'udp UNCONN 0 0 0.0.0.0:51820 0.0.0.0:*\n' >>"$SB/state/ss.txt"
}

echo '6. Tailscale, координатор Headscale и DERP: deploy и post_deploy их не трогают'
new_sandbox
mesh_listeners
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=cloudflare ENABLE_UFW=no run "$SB/mesh-install" deploy --yes; rc=$?
check 'установка рядом с mesh успешна' test "$rc" -eq 0
check 'после установки службы сверены' has_line "$SB/mesh-install" 'другие службы сервера на месте (порты: 11)'
rfm_release 1.8.0
: >"$SB/calls.log"
run "$SB/mesh" post_deploy --yes; rc=$?
check 'обновление успешно' test "$rc" -eq 0
check 'в итоге: другие службы на месте' has_line "$SB/mesh" 'Другие службы сервера (VPN, Tailscale и др.): на месте'
check 'ufw не меняется' no_line "$SB/calls.log" 'ufw allow'
check 'ufw не включается' no_line "$SB/calls.log" 'ufw --force'
check 'sysctl не меняется' no_line "$SB/calls.log" 'sysctl'
check 'пакеты не ставятся' no_line "$SB/calls.log" 'apt-get'
check 'перезапускались только FM и nginx' only_own_units "$SB/calls.log"
check 'Docker не вызывается' no_line "$SB/calls.log" 'docker'

echo '7. Служба пропала за время обновления — это видно в итоге'
# socat-мост на шлюзе docker-сети (BotCriptoM и др.) тоже сверяется.
printf 'tcp LISTEN 0 5 172.18.0.1:8787 0.0.0.0:* users:(("socat",pid=21,fd=5))\n' >>"$SB/state/ss.txt"
printf 'sed -i -e "/derper/d" -e "/socat/d" "$SB/state/ss.txt"\n' >>"$SB/bin/cargo"
rfm_release 1.9.0
WAIT_SECONDS=0 run "$SB/lost" post_deploy --yes; rc=$?
check 'пропажа — ненулевой код' test "$rc" -eq 1
check 'названы порт и процесс' has_line "$SB/lost" 'пропало: 3478/udp derper'
check 'пропавший мост socat назван' has_line "$SB/lost" '8787/tcp socat'
check 'FM при этом обновлён' has_line "$RFM_BIN" 'version=1.9.0'

echo '8. Мало места на диске: сборка не начинается, программа прежняя'
new_sandbox
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --yes
df_avail
rfm_release 1.8.0
bin_before=$(sha256sum <"$RFM_BIN")
: >"$SB/calls.log"
DF_AVAIL=500 run "$SB/disk" post_deploy --yes; rc=$?
check 'мало места — ненулевой код' test "$rc" -eq 1
check 'причина названа' has_line "$SB/disk" 'Сборка файлового менеджера: на диске свободно 500 МБ'
check 'сборка не запускалась' no_line "$SB/calls.log" 'cargo build'
check 'программа не заменена' test "$(sha256sum <"$RFM_BIN")" = "$bin_before"
check 'служба работает' test -f "$SB/state/active-$RFM_UNIT"
new_sandbox
df_avail
printf 'Secret-pass-1\nSecret-pass-1\n' |
  DF_AVAIL=1500 RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/disk-fresh" deploy --yes; rc=$?
check 'первая сборка при 1500 МБ — отказ до установки' test "$rc" -eq 1
check 'пакеты не ставились' no_line "$SB/calls.log" 'apt-get'

printf '\nПройдено: %s, ошибок: %s\n' "$PASS" "$FAILED"
(( FAILED == 0 ))
