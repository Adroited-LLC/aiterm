#!/usr/bin/env python3
"""Invite-authenticated cache of verified private GitHub release assets.
Only bind loopback; expose through the existing HTTPS proxy.
"""
import hashlib, hmac, json, os, pathlib, re, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CODE = re.compile(r'aiterm_[A-Za-z0-9_-]{43}\Z')
NAME = re.compile(r'[A-Za-z0-9][A-Za-z0-9._-]{0,180}\Z')

def authorized(header, invite_file):
    if not header.startswith('Bearer '): return False
    token = header[7:]
    if not CODE.fullmatch(token): return False
    digest = hashlib.sha256(token.encode()).hexdigest()
    try:
        records = json.loads(pathlib.Path(invite_file).read_text())
        return any(hmac.compare_digest(digest, value) for value in records.values())
    except (OSError, ValueError, TypeError): return False

def release_files(root):
    root = pathlib.Path(root).resolve(strict=True)
    manifest = json.loads((root/'updates.json').read_text())
    if manifest.get('schema') != 1: raise ValueError('Invalid manifest schema')
    names = {}
    for package in manifest['platforms'].values():
        name = package['asset']
        if not NAME.fullmatch(name): raise ValueError('Invalid asset name')
        path = root/name
        if path.is_symlink() or not path.is_file() or path.stat().st_size != package['size']:
            raise ValueError('Invalid asset file')
        names[name] = path
    return root/'updates.json', names

class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_): pass  # Never log request paths or credentials.
    def setup(self):
        super().setup(); self.connection.settimeout(30)
    def reply(self, status, body=b'', kind='text/plain'):
        self.send_response(status)
        self.send_header('Content-Type', kind)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'private, no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.end_headers()
        if body: self.wfile.write(body)
    def do_GET(self):
        if not authorized(self.headers.get('Authorization',''), self.server.invite_file):
            return self.reply(401, b'Invalid or revoked invite code')
        if self.path == '/updates/access': return self.reply(200, b'{"authorized":true}', 'application/json')
        streaming = False
        try:
            manifest, assets = release_files(self.server.release_root)
            if self.path == '/updates/latest': return self.reply(200, manifest.read_bytes(), 'application/json')
            prefix = '/updates/assets/'
            name = self.path[len(prefix):] if self.path.startswith(prefix) else ''
            path = assets.get(name)
            if path is None: return self.reply(404)
            with path.open('rb') as f:
                self.send_response(200)
                self.send_header('Content-Type','application/octet-stream')
                self.send_header('Content-Length',str(os.fstat(f.fileno()).st_size))
                self.send_header('Cache-Control','private, no-store')
                self.send_header('X-Content-Type-Options','nosniff')
                self.end_headers()
                streaming = True
                while chunk := f.read(65536): self.wfile.write(chunk)
        except (OSError, ValueError, KeyError, TypeError):
            # Missing/incomplete release must not look like "up to date".
            if not streaming: self.reply(503, b'Updates temporarily unavailable')
            else: self.close_connection = True

class Server(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 16
    def __init__(self, address, root, invites):
        self.release_root = root; self.invite_file = invites
        self.slots = threading.BoundedSemaphore(16)
        super().__init__(address, Handler)
    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False): self.shutdown_request(request); return
        try: super().process_request(request, address)
        except Exception: self.slots.release(); raise
    def process_request_thread(self, request, address):
        try: super().process_request_thread(request, address)
        finally: self.slots.release()
    def handle_error(self, *_): pass
if __name__ == '__main__':
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument('--root',required=True); p.add_argument('--invites',required=True); p.add_argument('--port',type=int,default=8090)
    a = p.parse_args()
    Server(('127.0.0.1',a.port),a.root,a.invites).serve_forever()
