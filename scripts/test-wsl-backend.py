"""Exercise a real Linux PTY over the desktop protocol, without a GUI.

Usage: python3 scripts/test-wsl-backend.py wsl-backend/target/debug/aiterm-wsl-backend
"""
import base64
import json
import os
import queue
import subprocess
import sys
import threading
import time
import unittest

BINARY = os.path.abspath(sys.argv.pop(1))


class Companion:
    def __init__(self, version=1):
        self.proc = subprocess.Popen([BINARY], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     env={**os.environ, "SHELL": "/bin/bash"})
        self.events = queue.Queue()
        self.output = bytearray()
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        self.send(type="start", version=version, cols=91, rows=27)

    def read(self):
        for line in self.proc.stdout:
            self.events.put(json.loads(line))
        self.events.put({"type": "eof"})

    def send(self, **request):
        self.proc.stdin.write(json.dumps(request).encode() + b"\n")
        self.proc.stdin.flush()

    def command(self, text):
        self.send(type="input", data=base64.b64encode(text.encode()).decode())

    def event(self, timeout=10, ack=True):
        event = self.events.get(timeout=timeout)
        if event["type"] == "output":
            self.output.extend(base64.b64decode(event["data"]))
            if ack:
                self.send(type="ack", sequence=event["sequence"])
        return event

    def until(self, predicate):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            try:
                event = self.event()
            except queue.Empty:
                raise AssertionError(f"Timed out; terminal output: {self.output[-2000:]!r}") from None
            if predicate(event):
                return event
            if event["type"] in ("error", "eof"):
                raise AssertionError(event)
        raise AssertionError("Timed out waiting for terminal output")

    def close(self):
        if not self.proc.stdin.closed:
            self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            raise AssertionError("Backend did not stop on transport EOF")
        self.reader.join(timeout=2)
        self.proc.stdout.close()


class BackendTests(unittest.TestCase):
    def start(self):
        p = Companion()
        self.addCleanup(p.close)
        ready = p.event()
        self.assertEqual(ready["type"], "ready")
        self.assertEqual(ready["version"], 1)
        return p, ready

    def test_unicode_resize_interrupt_and_exit_order(self):
        p, _ = self.start()
        p.command("stty -echo; PS1='__READY__ '; printf '\\123\\105\\124\\125\\120\\137\\117\\113\\n'\n")
        p.until(lambda _: b"SETUP_OK" in p.output and p.output.endswith(b"__READY__ "))
        p.output.clear()
        p.send(type="resize", cols=113, rows=41)
        p.command("stty size; printf 'héllo 世界\\n'; sh -c 'printf SLEEP_READY; exec sleep 30'\n")
        p.until(lambda _: "héllo 世界".encode() in p.output and b"41 113" in p.output and b"SLEEP_READY" in p.output)
        p.command("\x03")
        # Ctrl+C flushes pending terminal input. Wait for the shell to regain
        # the foreground before submitting the next command.
        p.until(lambda _: b"__READY__ " in p.output)
        p.command("printf 'FINAL_OUTPUT\\n'; exit 7\n")
        exit_event = p.until(lambda e: e["type"] == "exit")
        self.assertEqual(exit_event["code"], 7)
        self.assertIn(b"FINAL_OUTPUT", p.output)

    def test_disconnect_releases_shell(self):
        p, ready = self.start()
        p.close()
        deadline = time.monotonic() + 3
        while os.path.exists(f'/proc/{ready["pid"]}') and time.monotonic() < deadline:
            time.sleep(.05)
        # A reparented zombie has already exited and holds no execution resources.
        path = f'/proc/{ready["pid"]}/stat'
        if os.path.exists(path):
            with open(path) as f:
                self.assertEqual(f.read().split(") ", 1)[1].split()[0], "Z")

    def test_output_backpressure_is_bounded_and_resumes(self):
        p, _ = self.start()
        p.command("stty -echo; head -c 1048576 /dev/zero | tr '\\0' x; printf '\\nDONE\\n'; exit\n")
        total = 0
        sequence = 0
        while True:
            try:
                e = p.event(timeout=1, ack=False)
            except queue.Empty:
                break
            self.assertEqual(e["type"], "output")
            total += len(base64.b64decode(e["data"]))
            sequence = e["sequence"]
        self.assertGreater(total, 200000)
        self.assertLessEqual(total, 256 * 1024)
        p.send(type="ack", sequence=sequence)
        p.until(lambda e: e["type"] == "exit")
        self.assertGreaterEqual(p.output.count(b"x"), 1048576)
        self.assertIn(b"DONE", p.output)

    def test_disconnect_stops_foreground_job_ignoring_hangup(self):
        p, _ = self.start()
        p.command("stty -echo; sh -c 'trap \"\" HUP; echo JOB_PID=$$; exec sleep 300'\n")
        import re
        p.until(lambda _: re.search(rb"JOB_PID=(\d+)\r?\n", p.output) is not None)
        pid = int(re.search(rb"JOB_PID=(\d+)\r?\n", p.output).group(1))
        p.close()
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                with open(f"/proc/{pid}/stat") as f:
                    state = f.read().split(") ", 1)[1].split()[0]
                if state == "Z":
                    return
            except FileNotFoundError:
                return
            time.sleep(.05)
        self.fail("Foreground job survived desktop disconnect")

    def test_version_mismatch_does_not_spawn_shell(self):
        p = Companion(version=999)
        self.addCleanup(p.close)
        self.assertEqual(p.event()["type"], "error")
        self.assertNotEqual(p.proc.wait(timeout=5), 0)


if __name__ == "__main__":
    unittest.main()
