#!/usr/bin/env python3
"""Exercise the compiled TUI through a real PTY on macOS/Linux."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time


def git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.STDOUT).decode()


def fixture(root, files=3):
    git(root, "init", "-q", "-b", "main")
    for key, value in [("user.name", "Port Smoke"), ("user.email", "smoke@example.invalid"), ("commit.gpgsign", "false"), ("core.hooksPath", ".test-hooks")]:
        git(root, "config", key, value)
    (root / ".gitignore").write_text("*.log\n")
    for index in range(files):
        path = root / "files" / f"file-{index:06}.txt"
        path.parent.mkdir(exist_ok=True)
        path.write_text(f"line {index}\n")
    (root / "work.txt").write_text("base\n")
    git(root, "add", "--all")
    # Repeated Zs stay contiguous in Ratatui's incremental terminal updates.
    git(root, "commit", "-q", "-m", "fixture", "-m", "ZZZZZZZZZZZZ\n\nCommit inspection body.")


class Session:
    def __init__(self, binary, root, config):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
        env = os.environ.copy()
        env.update(TERM="xterm-256color", COLORTERM="truecolor", LC_ALL="en_US.UTF-8")

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)

        self.started = time.monotonic()
        self.process = subprocess.Popen([str(binary), "-p", str(root), "--use-config-dir", str(config)], stdin=self.slave, stdout=self.slave, stderr=self.slave, env=env, preexec_fn=controlling_terminal)
        self.data = b""

    def send(self, text):
        os.write(self.master, text.encode())

    def read(self, duration=0.05, stop_marker=None):
        deadline = time.monotonic() + duration
        chunk = b""
        while time.monotonic() < deadline:
            ready, _, _ = select.select([self.master], [], [], max(0, deadline - time.monotonic()))
            if not ready:
                break
            try:
                current = os.read(self.master, 65536)
            except OSError:
                break
            if not current:
                break
            chunk += current
            if b"\x1b[6n" in current:
                os.write(self.master, b"\x1b[1;1R")
            if stop_marker is not None and stop_marker in self.data + chunk:
                break
        self.data += chunk
        return chunk

    def until(self, marker, timeout=10):
        marker = marker.encode()
        deadline = time.monotonic() + timeout
        while marker not in self.data:
            self.read(stop_marker=marker)
            if self.process.poll() is not None:
                raise AssertionError(f"TUI exited early: {self.data[-4000:]!r}")
            if time.monotonic() >= deadline:
                raise AssertionError(f"Timed out waiting for {marker!r}: {self.data[-4000:]!r}")

    def finish(self):
        self.send("q")
        deadline = time.monotonic() + 5
        while self.process.poll() is None and time.monotonic() < deadline:
            self.read()
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGKILL)
            raise AssertionError("TUI did not exit")
        self.read()
        try:
            after = termios.tcgetattr(self.master)
        except termios.error:
            # macOS invalidates terminal ioctls after the controlling session exits.
            assert b"\x1b[?1049l" in self.data, "Alternate screen was not restored"
        else:
            assert after[3] & (termios.ICANON | termios.ECHO) == self.original[3] & (termios.ICANON | termios.ECHO), "Terminal mode was not restored"
        assert self.process.returncode == 0, self.data[-4000:]
        os.close(self.master)
        os.close(self.slave)

    def close(self):
        os.close(self.master)
        os.close(self.slave)
        if self.process.poll() is None:
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
            self.process.wait(timeout=5)


def eventually(check, timeout=5, pump=None):
    deadline = time.monotonic() + timeout
    while not check():
        if time.monotonic() >= deadline:
            raise AssertionError("Repository did not reach expected state")
        if pump is not None:
            pump(0.02)
        else:
            time.sleep(0.02)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", nargs="?", type=Path, default=Path(__file__).resolve().parents[1] / "target/release/pd-lazygit")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="pd-rust-smoke-") as temporary:
        root = Path(temporary) / "repo"
        config = Path(temporary) / "config"
        root.mkdir()
        config.mkdir()
        fixture(root)
        git(root, "branch", "feature/user/login")
        git(root, "branch", "feature/user/profile")
        git(root, "update-ref", "refs/remotes/origin/feature/user/login", "HEAD")
        (config / "config.yml").write_text("git:\n  autoFetch: false\n  autoRefresh: false\n")
        (root / "work.txt").write_text("updated\n" + "".join(f"large-hunk-line-{line:04}\n" for line in range(240)) + "QQQQQQQQQQQQ\n")
        session = Session(args.binary.resolve(), root, config)
        finished = False
        try:
            session.until("work.txt")
            ready_ms = round((time.monotonic() - session.started) * 1000, 2)
            original_head = git(root, "rev-parse", "HEAD")
            session.send("2/feature\r ")
            state_file = config / "rust-state.json"
            eventually(lambda: state_file.exists() and all(reference in json.loads(state_file.read_text())["visibleBranches"].get(str(root.resolve()), []) for reference in ["refs/heads/feature/user/login", "refs/heads/feature/user/profile"]), pump=session.read)
            assert git(root, "rev-parse", "HEAD") == original_head, "Graph toggle checked out a branch"
            session.read(0.15)
            session.send("5j")
            session.until("ZZZZZZZZZZZZ")
            session.send("\x1b[1;2A\x1b[1;2B")
            session.read(0.1)
            session.send("o")
            session.until("Actions")
            session.send("?")
            session.until("Numbered")
            session.send("\x1b")
            session.read(0.1)
            session.send("5\x1b[H7/work.txt\r")
            session.read(0.1)
            session.send("\r")
            session.read(0.1)
            assert b"QQQQQQQQQQQQ" not in session.data, "Large hunk fixture fits on screen"
            session.send("\x1b[B" * 270)
            session.until("QQQQQQQQQQQQ")
            session.send("\x1b[A")
            session.read(0.05)
            session.send("\x1b")
            session.read(0.1)
            session.send(" ")
            eventually(lambda: "work.txt" in git(root, "diff", "--cached", "--name-only"), pump=session.read)
            session.read(0.15)
            session.send("cRust port smoke commit\r")
            eventually(lambda: git(root, "log", "-1", "--format=%s").strip() == "Rust port smoke commit", pump=session.read)
            session.read(0.2)
            session.send("~")
            eventually(lambda: json.loads(state_file.read_text())["diffPreviewEnabled"] is False, pump=session.read)
            session.send("8:printf '%s%s\\n' LIVE _READY; sleep 2; printf '%s%s\\n' LIVE _DONE\r")
            session.until("LIVE_READY")
            assert b"LIVE_DONE" not in session.data, "Command output was buffered until completion"
            session.send("\x03")
            session.until("Cancelled")
            session.read(0.15)
            session.finish()
            finished = True
            print(json.dumps({"terminal_smoke": "passed", "ready_ms": ready_ms, "verified": ["nested branch group graph toggle without checkout", "persisted branch visibility", "automatic full commit inspection", "modified arrow navigation", "executable context menu", "working-tree navigation", "full-screen large hunk keyboard scrolling", "staging", "commit", "persisted preview toggle", "live output", "process cancellation", "clean terminal restoration"]}))
        except Exception:
            session.read(0.1)
            import re
            tail = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", session.data[-8000:]).decode(errors="replace")
            print(f"Terminal tail: {tail[-3000:]}", flush=True)
            raise
        finally:
            if not finished:
                session.close()


if __name__ == "__main__":
    main()
