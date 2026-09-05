#!/usr/bin/env python3
"""Exercise the full Linux service over the same framed transport as Windows."""
import base64
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest

BINARY = str(Path(sys.argv.pop(1)).resolve())

class WorkspaceTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aiterm-service-test-")
        self.home = Path(self.temp.name)
        env = dict(os.environ, HOME=str(self.home), SHELL="/bin/bash", XDG_CONFIG_HOME=str(self.home / ".config"),
                   XDG_DATA_HOME=str(self.home / ".local/share"), XDG_CACHE_HOME=str(self.home / ".cache"))
        self.proc = subprocess.Popen([BINARY, "--service"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, text=True)
        self.messages = queue.Queue()
        self.events = []
        self.output = bytearray()
        self.seq = 0
        def read():
            for line in self.proc.stdout:
                self.messages.put(json.loads(line))
        threading.Thread(target=read, daemon=True).start()
        self.until(lambda f: f.get("type") == "ready")

    def tearDown(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            self.fail("Workspace did not shut down on EOF")
        finally:
            self.proc.stdout.close()
            self.temp.cleanup()

    def send(self, frame):
        self.proc.stdin.write(json.dumps(frame) + "\n")
        self.proc.stdin.flush()

    def until(self, predicate, ack=True):
        if predicate({}): return {}
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            try: frame = self.messages.get(timeout=max(.01, deadline-time.monotonic()))
            except queue.Empty: self.fail(f"Workspace timed out. Terminal output: {self.output[-2000:]!r}")
            if frame.get("type") == "channel":
                data = base64.b64decode(frame["data"])
                self.output.extend(data)
                if ack:
                    self.send({"type":"ack", "channel":frame["id"], "bytes":len(data)})
            elif frame.get("type") == "event":
                self.events.append(frame)
            if predicate(frame):
                return frame
        self.fail("Timed out waiting for workspace response")

    def rpc(self, command, **args):
        self.seq += 1
        self.send({"type":"call", "id":self.seq, "command":command, "args":args})
        reply = self.until(lambda f: f.get("type") == "reply" and f["id"] == self.seq)
        self.assertNotIn("error", reply, reply)
        return reply["value"]

    def terminal(self):
        tab = self.rpc("tab_open", launch={"title":"Parity test", "slotId":"test", "cwd":str(self.home), "size":{"cols":100,"rows":32}})
        attachment = self.rpc("tab_attach_desktop", tabId=tab["id"], onOutput="__CHANNEL__:7")
        self.rpc("tab_take_focus", tabId=tab["id"], attachmentId=attachment, cols=100, rows=32)
        return tab["id"], attachment

    def test_sessions_terminal_and_registry_share_one_service(self):
        self.assertEqual(self.rpc("tab_list"), [])
        tab, attachment = self.terminal()
        self.assertEqual(self.rpc("tab_list")[0]["id"], tab)
        self.rpc("tab_resize", tabId=tab, attachmentId=attachment, cols=93, rows=27)
        self.rpc("tab_write", tabId=tab, attachmentId=attachment,
                 data="printf '\\101\\111\\124\\105\\122\\115\\137\\120\\101\\122\\111\\124\\131\\n'; stty size\n")
        self.until(lambda _: b"AITERM_PARITY" in self.output and b"27 93" in self.output)
        self.rpc("tab_close", tabId=tab)
        self.assertEqual(self.rpc("tab_list"), [])
        self.assertTrue(any(e["name"] == "tab://registry" for e in self.events))

    def test_large_file_and_structured_errors(self):
        path = self.home / "large.txt"
        path.write_text("a" * (1500 * 1024))
        result = self.rpc("read_text_file", path=str(path))
        self.assertEqual(len(result["content"]), 1500 * 1024)
        config = self.home / "settings.json"
        config.write_text('{}')
        self.seq += 1
        self.send({"type":"call","id":self.seq,"command":"claude_save_layer","args":{"path":str(config),"newText":"{}","loadedText":"changed"}})
        reply = self.until(lambda f: f.get("type") == "reply" and f["id"] == self.seq)
        self.assertEqual(reply["error"]["kind"], "collision")
        self.assertEqual(config.read_text(), '{}')

    def test_slow_terminal_reader_does_not_block_commands(self):
        tab, attachment = self.terminal()
        self.rpc("tab_write", tabId=tab, attachmentId=attachment, data="head -c 700000 /dev/zero | tr '\\0' x\n")
        received = 0
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try: frame = self.messages.get(timeout=.2)
            except queue.Empty: continue
            if frame.get("type") == "channel": received += len(base64.b64decode(frame["data"]))
        self.assertGreater(received, 0)
        self.assertLessEqual(received, 256 * 1024)
        self.assertEqual(self.rpc("tab_list")[0]["id"], tab)
        self.send({"type":"channel_close", "channel":7})
        self.rpc("tab_close", tabId=tab)

if __name__ == "__main__": unittest.main()
