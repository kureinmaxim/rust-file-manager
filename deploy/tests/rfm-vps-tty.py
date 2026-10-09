"""Проверка вопросов через настоящий PTY, только с синтетическими данными."""
import errno
import os
import pty
import select
import subprocess
import sys
import time
from pathlib import Path

script, scenario = sys.argv[1:]
cases = {
    "install_fm": (["deploy"], [("Придумайте пароль", "Secret-pass-1"),
                                 ("Повторите пароль:", "Secret-pass-1"),
                                 ("Начинаем?", "y")], 0),
    "cancel_fm": (["post_deploy"], [("Сохранить архив данных", "n"),
                                    ("Обновляем по этому плану?", "n")], 1),
    "update_fm": (["post_deploy"], [("Сохранить архив данных", "n"),
                                    ("Обновляем по этому плану?", "y")], 0),
}
args, replies, expected = cases[scenario]
master, slave = pty.openpty()
process = subprocess.Popen(["bash", script, *args], stdin=slave,
                           stdout=slave, stderr=slave, close_fds=True)
os.close(slave)
output = ""
pending = ""
index = 0
deadline = time.monotonic() + 60
try:
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master], [], [], 0.1)
        if ready:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                break
            if not data:
                break
            text = data.decode("utf-8", errors="replace")
            output += text
            pending += text
            if index < len(replies) and replies[index][0] in pending:
                # Даём read -s переключить echo после записи приглашения.
                time.sleep(0.05)
                os.write(master, (replies[index][1] + "\n").encode())
                index += 1
                pending = ""
        elif process.poll() is not None:
            break
    if process.poll() is None and time.monotonic() >= deadline:
        process.kill()
        raise AssertionError("опрос завис")
    rc = process.wait(timeout=5)
    Path(os.environ["SB"], f"tty-{scenario}.log").write_text(output, encoding="utf-8")
    assert index == len(replies), f"получено вопросов {index}/{len(replies)}"
    assert rc == expected, f"код {rc}, ожидался {expected}"
    assert "Secret-pass-1" not in output, "пароль попал в терминал"
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
