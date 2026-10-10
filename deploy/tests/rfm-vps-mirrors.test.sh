#!/usr/bin/env bash
# Запасные источники кода: GitHub недоступен — своё зеркало (RFM_MIRRORS) и bundle,
# присланный с локальной машины. Все «серверы» — локальные git-репозитории песочницы.
#   bash deploy/tests/rfm-vps-mirrors.test.sh
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source <(sed '/^# =====/q' "$HERE/rfm-vps.test.sh")

mirror_sandbox() {
  new_sandbox
  export BUNDLE_DIR=$SB/var/cache/rfm-vps RFM_MIRRORS="file://$SB/mirror/rust-file-manager.git"
  mkdir -p "$SB/mirror"
  git clone -q --mirror "$SB/repos/rfm.git" "$SB/mirror/rust-file-manager.git"
}

mirror_sync() { git -C "$SB/mirror/rust-file-manager.git" fetch -q "$SB/repos/rfm.git" '+refs/heads/*:refs/heads/*'; }
github_down() { mv "$SB/repos/rfm.git" "$SB/repos/rfm.off"; }

make_bundle() {  # root, 0700/0600 — как в инструкции «Без GitHub»
  install -d -m 0700 "$BUNDLE_DIR"
  git -C "$SB/src/rfm" bundle create -q "$BUNDLE_DIR/rust-file-manager.bundle" main --tags
  chmod 600 "$BUNDLE_DIR/rust-file-manager.bundle"
}

install_fm() {
  printf 'Secret-pass-1\nSecret-pass-1\n' |
    RFM_DOMAIN=files.example.com HTTPS_MODE=none ENABLE_UFW=no run "$SB/install" deploy --yes
}

# ============================================================================
echo '1. GitHub недоступен — обновление из своего зеркала'
mirror_sandbox
install_fm; rc=$?
check 'установка с GitHub' test "$rc" -eq 0
check 'при живом GitHub зеркало не упоминается' no_line "$SB/install" 'взят из источника'
rfm_release 1.8.0; mirror_sync; github_down
run "$SB/o1" post_deploy --yes; rc=$?
check 'post_deploy успешен' test "$rc" -eq 0
check 'код из зеркала' has_line "$SB/o1" 'код rust-file-manager взят из источника «зеркало»'
check 'FM 1.8.0' has_line "$RFM_BIN" 'version=1.8.0'
check 'origin сборки остался GitHub' test "$(git -C "$(val "$STATE_FILE" RFM_BUILD_DIR)" remote get-url origin)" = "$RFM_REPO_URL"
check 'временные ссылки убраны' test -z "$(git -C "$(val "$STATE_FILE" RFM_BUILD_DIR)" for-each-ref refs/rfm-vps/)"
run "$SB/o1c" post_deploy --check; rc=$?
check '--check сверяет с зеркалом' has_line "$SB/o1c" 'сверяю с источником «зеркало»'

echo '2. Зеркало отстало, bundle новее — берётся bundle'
mirror_sandbox
install_fm
rfm_release 1.8.0  # в зеркало ещё не пришло
github_down; make_bundle
run "$SB/o2" post_deploy --yes; rc=$?
check 'post_deploy успешен' test "$rc" -eq 0
check 'код из bundle' has_line "$SB/o2" 'код rust-file-manager взят из источника «bundle»'
check 'FM 1.8.0' has_line "$RFM_BIN" 'version=1.8.0'

echo '3. Отставший источник не понижает версию, незащищённый bundle не используется'
mirror_sandbox
install_fm
rfm_release 1.8.0
run "$SB/o3a" post_deploy --yes  # с GitHub; зеркало осталось на 1.7.0
github_down
run "$SB/o3" post_deploy --yes; rc=$?
check 'post_deploy успешен' test "$rc" -eq 0
check 'понижение отклонено' has_line "$SB/o3" 'версия старее работающей — файловый менеджер не трогаю'
check 'FM остался 1.8.0' has_line "$RFM_BIN" 'version=1.8.0'
sed -i 's/^version = .*/version = "1.9.0"/' "$SB/src/rfm/Cargo.toml"; git_commit_all "$SB/src/rfm" 1.9.0
make_bundle; chmod 755 "$BUNDLE_DIR"; chmod 666 "$BUNDLE_DIR/rust-file-manager.bundle"
run "$SB/o3b" post_deploy --yes
check 'bundle с записью для всех отвергнут' has_line "$SB/o3b" 'не используется: файл или каталог чужой'
check 'из него не обновлено' has_line "$RFM_BIN" 'version=1.8.0'
chmod 700 "$BUNDLE_DIR"; chmod 600 "$BUNDLE_DIR/rust-file-manager.bundle"
run "$SB/o3c" post_deploy --yes; rc=$?
check 'защищённый bundle принят' test "$rc" -eq 0
check 'FM 1.9.0 из bundle' has_line "$RFM_BIN" 'version=1.9.0'

echo '4. Установка с нуля без GitHub — из bundle'
mirror_sandbox
github_down; export RFM_MIRRORS=''
make_bundle
install_fm; rc=$?
check 'deploy успешен' test "$rc" -eq 0
check 'код из bundle' has_line "$SB/install" 'код rust-file-manager взят из источника «bundle»'
check 'FM 1.7.0' has_line "$RFM_BIN" 'version=1.7.0'
check 'origin сборки — GitHub для следующих обновлений' test "$(git -C "$(val "$STATE_FILE" RFM_BUILD_DIR)" remote get-url origin)" = "$RFM_REPO_URL"

echo '5. GitHub отвечает — он главный, даже если зеркало новее'
mirror_sandbox
install_fm
rfm_release 1.8.0; mirror_sync
git clone -q "$SB/mirror/rust-file-manager.git" "$SB/m"
sed -i 's/^version = .*/version = "1.9.0"/' "$SB/m/Cargo.toml"; git_commit_all "$SB/m" 1.9.0
git -C "$SB/m" push -q origin main
run "$SB/o5" post_deploy --yes; rc=$?
check 'post_deploy успешен' test "$rc" -eq 0
check 'установлена версия с GitHub' has_line "$RFM_BIN" 'version=1.8.0'
check 'зеркало не использовалось' no_line "$SB/o5" 'взят из источника'

echo '6. Нет ни одного источника — понятная ошибка, программа не тронута'
mirror_sandbox
install_fm
github_down; rm -rf "$SB/mirror"
run "$SB/o6" post_deploy --yes; rc=$?
check 'post_deploy с ошибкой' test "$rc" -ne 0
check 'названы опрошенные источники' has_line "$SB/o6" 'ни из одного источника (основной, зеркало)'
check 'FM остался 1.7.0 и работает' has_line "$RFM_BIN" 'version=1.7.0'
check 'служба активна' test -f "$SB/state/active-$RFM_UNIT"

echo '7. Зеркало из /etc/rfm-vps.conf'
mirror_sandbox
install_fm
echo "RFM_MIRRORS=$RFM_MIRRORS" >>"$STATE_FILE"; export RFM_MIRRORS=''
rfm_release 1.8.0; mirror_sync; github_down
run "$SB/o7" post_deploy --yes; rc=$?
check 'post_deploy успешен' test "$rc" -eq 0
check 'код из зеркала из файла состояния' has_line "$SB/o7" 'взят из источника «зеркало»'
check 'FM 1.8.0' has_line "$RFM_BIN" 'version=1.8.0'

echo
echo "Пройдено: $PASS, ошибок: $FAILED"
(( FAILED == 0 ))
