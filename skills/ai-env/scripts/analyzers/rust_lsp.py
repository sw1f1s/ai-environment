"""Bounded stdio JSON-RPC client for rust-analyzer; never invokes project tools."""
import json
import queue
import subprocess
import threading
import time


class Lsp:
    def __init__(self, argv, cwd, env):
        self.proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.messages = queue.Queue()
        self.sequence = 0
        self.notifications = []
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.proc.stdout.readline()
                    if not line:
                        raise EOFError("rust-analyzer stopped")
                    if line in (b'\r\n', b'\n'):
                        break
                    k, v = line.decode('ascii').split(':', 1)
                    headers[k.lower()] = v.strip()
                length = int(headers.get('content-length', 0))
                if length > 32 * 1024 * 1024:
                    raise ValueError("rust-analyzer response exceeds 32 MiB")
                raw = self.proc.stdout.read(length)
                self.messages.put(json.loads(raw))
        except Exception as exc:
            self.messages.put(exc)

    def send(self, value):
        data = json.dumps(value, ensure_ascii=False).encode('utf8')
        self.proc.stdin.write(('Content-Length: %d\r\n\r\n' % len(data)).encode() + data)
        self.proc.stdin.flush()

    def notify(self, method, params):
        self.send(dict(jsonrpc='2.0', method=method, params=params))

    def request(self, method, params, timeout=35):
        self.sequence += 1
        sequence = self.sequence
        self.send(dict(jsonrpc='2.0', id=sequence, method=method, params=params))
        deadline = time.monotonic() + timeout
        while True:
            remaining=deadline-time.monotonic()
            if remaining<=0:
                raise TimeoutError('rust-analyzer request timed out: '+method)
            try:
                value = self.messages.get(timeout=remaining)
            except queue.Empty:
                raise TimeoutError('rust-analyzer request timed out: '+method) from None
            if isinstance(value, Exception):
                raise value
            if 'method' in value and 'id' in value:
                # No target configuration/commands are accepted by this client.
                result = [] if value['method'] == 'workspace/configuration' else None
                self.send(dict(jsonrpc='2.0', id=value['id'], result=result))
            elif value.get('id') == sequence:
                if 'error' in value:
                    raise RuntimeError('%s: %s' % (method, value['error']))
                return value.get('result')
            else:
                self.notifications.append(value)

    def close(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
