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
for spec in 443/tcp 443/udp 29999/tcp; do
  check "порт $spec разрешён до включения ufw" line_before "$SB/calls.log" "ufw allow $spec" 'ufw --force enable'
done
check 'loopback-порт не открывается' no_line "$SB/calls.log" 'ufw allow 8000'
check 'DHCP-клиент не открывается' no_line "$SB/calls.log" 'ufw allow 68/udp'

echo '3. Порт файлового менеджера занят VPN: остановка до пакетов и сборки'
new_sandbox
vpn_listeners
printf 'tcp LISTEN 0 4096 0.0.0.0:8080 0.0.0.0:* users:(("xray",pid=17,fd=8))\n' >>"$SB/state/ss.txt"
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=cloudflare ENABLE_UFW=no run "$SB/busy" deploy --yes; rc=$?
check 'занятый порт — ошибка' test "$rc" -eq 1
check 'названы порт и процесс' has_line "$SB/busy" 'Порт 8080 занят (xray)'
check 'пакеты не ставились' no_line "$SB/calls.log" 'apt-get'
check 'сборка не запускалась' test ! -e "$BUILD_ROOT/rfm-build"
sed -i '/:8080 /d' "$SB/state/ss.txt"
printf 'Secret-pass-1\nSecret-pass-1\n' |
  RFM_DOMAIN=files.example.com HTTPS_MODE=certbot ENABLE_UFW=no run "$SB/busy443" deploy --yes; rc=$?
check 'certbot при занятом 443 — ошибка' test "$rc" -eq 1
check 'предложен другой способ HTTPS' has_line "$SB/busy443" 'HTTPS_MODE=cloudflare'

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

printf '\nПройдено: %s, ошибок: %s\n' "$PASS" "$FAILED"
(( FAILED == 0 ))
