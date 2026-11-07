"""Portable deployment reporting tests: real temporary Git, mocked services only."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
FULL = (ROOT / 'scripts/tgo-vps.sh').exists()
if not FULL:
    ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / ('scripts/tgo-vps.sh' if FULL else 'deploy/rfm-vps.sh')


def function(name):
    lines = SCRIPT.read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if re.match(r'^' + name + r'\(\)\s*\{', line))
    if lines[start].rstrip().endswith('}'):
        return lines[start] + '\n'
    end = next(i for i in range(start + 1, len(lines)) if lines[i] == '}')
    return '\n'.join(lines[start:end + 1]) + '\n'


class DeploymentReportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='post-deploy-report-')
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.git('init', '-q', '-b', 'main')
        (self.repo / 'pyproject.toml').write_text('version = "1.0.0"\n')
        (self.repo / 'Cargo.toml').write_text('version = "1.0.0"\n')
        (self.repo / 'tracked.py').write_text('original\n')
        self.git('add', '.')
        self.git('-c', 'user.name=Synthetic test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'initial')
        self.head = self.git('rev-parse', 'HEAD').strip()
        self.git('update-ref', 'refs/remotes/origin/main', self.head)
        (self.repo / '.process').write_text('a' * 32)
        pip = self.repo / 'venv/bin/pip'
        pip.parent.mkdir(parents=True)
        pip.write_text('#!/bin/sh\nexit 0\n')
        pip.chmod(0o700)

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], text=True)

    def shell(self, body, extra=()):
        names = ['manual_changes_help', 'fm_checkout', 'final_checks', 'print_summary']
        if FULL:
            names += ['bot_code_commit', 'bot_process_identity', 'bot_verification_help', 'bot_update']
        prelude = '''set -uo pipefail
SUMMARY=(); TODO=(); FM_UPDATE_STATE=''; BOT_UPDATE_STATE=''; C_B=''; C_0=''
BOT_UNIT=telegramonly.service; BOT_CONTAINER=telegram-helper-lite
BOT_RESTARTED=0; BOT_API_PENDING=0; WAIT_SECONDS=0; FORCE=1; ONLY=''; INSTALLED_NOW=0
RFM_UNIT=rust-file-manager.service; BOT_SOURCE=GitHub
say() { printf '%s\\n' "$*"; }
warn() { printf 'WARN %s\\n' "$*"; }
fail() { printf 'FAIL %s\\n' "$*"; }
step() { printf '%s\\n' "$*"; }
ok() { printf 'OK %s\\n' "$*"; }
bot_flavor() { echo systemd; }
bot_present() { return 0; }
fm_present() { return 1; }
bot_healthy() { return 0; }
bot_repo_version() { echo 1.0.0; }
bot_running_version() { echo 1.0.0; }
bot_fetch_newest() { return 0; }
primary_src() { return 0; }
others_check() { return 0; }
fm_version() { echo 1.0.0; }
fm_commit() { echo abc1234; }
sleep() { :; }
systemctl() {
  case $1 in
    show) cat "$BOT_DIR/.process" ;;
    restart) printf 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' > "$BOT_DIR/.process" ;;
    is-active) return 0 ;;
  esac
}
'''
        script = prelude + ''.join(function(n) for n in names)
        script += '\n'.join(extra) + '\n' + body
        env = {**os.environ, 'BOT_DIR': str(self.repo)}
        return subprocess.run(['bash', '-c', script], env=env, text=True, capture_output=True)

    @unittest.skipUnless(FULL, 'bot reporting belongs to TelegramOnly')
    def test_dirty_bot_stops_even_with_force_and_keeps_manual_changes(self):
        (self.repo / 'tracked.py').write_text('manual change\n')
        result = self.shell('bot_update; code=$?; final_checks; print_summary; exit "$code"')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('обновление НЕ выполнено', result.stdout)
        self.assertIn('прежняя служба', result.stdout)
        self.assertIn('служба доступна, но обновление НЕ выполнено', result.stdout)
        self.assertIn('tracked-changes.patch', result.stdout)
        self.assertIn('systemctl is-active telegramonly.service', result.stdout)
        self.assertNotIn('docker inspect', result.stdout)
        self.assertEqual((self.repo / 'tracked.py').read_text(), 'manual change\n')
        self.assertEqual(self.git('rev-parse', 'HEAD').strip(), self.head)
        self.assertEqual((self.repo / '.process').read_text(), 'a' * 32)

    def test_dirty_fm_stops_before_fetch_and_keeps_changes(self):
        (self.repo / 'tracked.py').write_text('manual change\n')
        result = self.shell('FM_UPDATE_STATE=failed; fm_checkout "$BOT_DIR" main; code=$?; final_checks; print_summary; exit "$code"')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('обновление НЕ выполнено', result.stdout)
        self.assertIn('tracked-changes.patch', result.stdout)
        self.assertIn('прежняя программа оставлена', result.stdout)
        self.assertNotIn('удалите каталог', result.stdout)
        self.assertEqual((self.repo / 'tracked.py').read_text(), 'manual change\n')
        self.assertEqual(self.git('rev-parse', 'HEAD').strip(), self.head)

    def test_backup_guidance_creates_usable_patch_without_changing_source(self):
        (self.repo / 'tracked.py').write_text('manual change\n')
        result = self.shell('manual_changes_help "$BOT_DIR"')
        commands = [line.strip() for line in result.stdout.splitlines() if line.startswith('  ')]
        commands[0] = 'update_backup_dir="$BOT_DIR/backup"; mkdir -m 700 "$update_backup_dir"'
        result = self.shell('\n'.join(commands))
        self.assertEqual(result.returncode, 0, result.stderr)
        patch = self.repo / 'backup/tracked-changes.patch'
        self.assertIn('+manual change', patch.read_text())
        self.assertEqual(patch.stat().st_mode & 0o777, 0o600)
        self.assertEqual((self.repo / 'tracked.py').read_text(), 'manual change\n')

    @unittest.skipUnless(FULL, 'bot reporting belongs to TelegramOnly')
    def test_systemd_restart_reports_new_process_and_disk_commit(self):
        result = self.shell('bot_update; code=$?; print_summary; exit "$code"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('перезапуск подтверждён сменой процесса', result.stdout)
        self.assertIn(self.head[:7], result.stdout)
        self.assertIn('systemd', result.stdout)

    @unittest.skipUnless(FULL, 'bot reporting belongs to TelegramOnly')
    def test_active_service_with_unchanged_process_is_not_update_success(self):
        result = self.shell('bot_update; code=$?; final_checks; print_summary; exit "$code"',
            ['systemctl() { [[ $1 != show ]] || cat "$BOT_DIR/.process"; return 0; }'])
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('обновление НЕ подтверждено', result.stdout)
        self.assertNotIn('перезапуск подтверждён сменой процесса', result.stdout)

    @unittest.skipUnless(FULL, 'bot reporting belongs to TelegramOnly')
    def test_no_restart_reports_no_restart_and_force_hint(self):
        result = self.shell('FORCE=0; bot_update; code=$?; print_summary; exit "$code"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('перезапуска не было', result.stdout)
        self.assertIn('post_deploy --only bot --force --yes', result.stdout)

    @unittest.skipUnless(FULL, 'bot reporting belongs to TelegramOnly')
    def test_unknown_identity_is_not_reported_as_confirmed(self):
        result = self.shell('bot_update; code=$?; print_summary; exit "$code"',
            ['bot_process_identity() { :; }'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('смену процесса подтвердить не удалось', result.stdout)
        self.assertNotIn('перезапуск подтверждён сменой процесса', result.stdout)

    @unittest.skipUnless(FULL, 'bot reporting belongs to TelegramOnly')
    def test_docker_help_and_identity_use_container_not_systemd(self):
        identity = 'd' * 64 + ':2026-11-07T12:00:00Z'
        result = self.shell('bot_process_identity; bot_verification_help; printf \'%s\\n\' "${TODO[@]}"', [
            'bot_flavor() { echo docker; }', 'docker() { echo "' + identity + '"; }'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(identity, result.stdout)
        self.assertIn('docker inspect', result.stdout)
        self.assertNotIn('systemctl is-active', result.stdout)


    def test_failed_fm_update_is_not_green_when_previous_service_is_active(self):
        result = self.shell('FM_UPDATE_STATE=failed; final_checks', [
            'fm_present() { return 0; }', 'bot_present() { return 1; }'])
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('служба работает, но обновление НЕ выполнено', result.stdout)
        self.assertNotIn('OK файловый менеджер', result.stdout)

    def test_fm_check_reports_running_commit_and_systemd_unit(self):
        result = self.shell('FM_UPDATE_STATE=updated; final_checks', [
            'fm_present() { return 0; }', 'bot_present() { return 1; }'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('abc1234', result.stdout)
        self.assertIn('systemd: rust-file-manager.service', result.stdout)

    @unittest.skipUnless(FULL, 'bot reporting belongs to TelegramOnly')
    def test_restart_command_failure_is_not_reported_as_update(self):
        result = self.shell('bot_update; code=$?; final_checks; exit "$code"', [
            'systemctl() { if [[ $1 == restart ]]; then return 1; elif [[ $1 == show ]]; then cat "$BOT_DIR/.process"; else return 0; fi; }'])
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('не перезапустился', result.stdout)
        self.assertIn('обновление НЕ выполнено', result.stdout)

if __name__ == '__main__':
    unittest.main()
