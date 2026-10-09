"""Real nginx routing tests with an isolated HTTP upstream and synthetic data.

Run: NGINX_BIN=/path/to/nginx python3 deploy/tests/nginx-upload.test.py
No system nginx files, working credentials or external services are used.
"""
import contextlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]
NGINX = os.environ.get("NGINX_BIN") or shutil.which("nginx")
AUTH = "Bearer synthetic-nginx-test-token"
PHOTO = b"synthetic-photo" * 165000  # 2.31 MB, comparable to a phone photo.


class Upstream(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def handle_upload(self):
        path = urlsplit(self.path).path
        if self.headers.get("Authorization") != AUTH:
            status, data = 401, {"code": "unauthorized"}
        elif path == "/api/v1/uploads/" and not self.server.alias:
            status, data = 404, {}
        elif self.command == "GET" and path.rstrip("/") == "/api/v1/uploads":
            status, data = 200, {"uploads": []}
        elif self.command == "POST" and path.rstrip("/") == "/api/v1/uploads":
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            status, data = 201, {"id": "synthetic-upload-id"}
        elif self.command == "PUT" and path == "/api/v1/uploads/synthetic-upload-id":
            self.server.received = self.rfile.read(int(self.headers["Content-Length"]))
            status, data = 200, {"offset": len(self.server.received)}
        elif self.command == "POST" and path.endswith("/synthetic-upload-id/complete"):
            status, data = 200, {"item": {"size": len(self.server.received)}}
        else:
            status, data = 404, {}
        body = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_GET = do_POST = do_PUT = handle_upload


def templates():
    example = (ROOT / "deploy/nginx.example.conf").read_text()
    script = (ROOT / "deploy/rfm-vps.sh").read_text().rsplit('\nmain "$@"; exit $?', 1)[0]
    cloudflare = subprocess.run(
        ["bash", "-s"], input=script + "\nnginx_cloudflare_site files.example.com\n",
        text=True, capture_output=True, check=True,
    ).stdout
    return {"certbot": example, "cloudflare": cloudflare}


def request(port, method, path, body=None, auth=True):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        headers = {"Authorization": AUTH} if auth else {}
        conn.request(method, path, body=body, headers=headers)
        res = conn.getresponse()
        return res.status, dict(res.getheaders()), res.read()
    finally:
        conn.close()


@contextlib.contextmanager
def proxy(template, alias=True):
    upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    upstream.alias = alias
    upstream.received = b""
    thread = threading.Thread(target=upstream.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="rfm-nginx-test-") as name:
            directory = Path(name)
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            # Exercise both production routing templates over local HTTP;
            # TLS certificate issuance is outside this routing test.
            site = re.sub(r"listen (?:80|2083 ssl);", f"listen 127.0.0.1:{port};", template)
            site = re.sub(r"^\s*ssl_certificate(?:_key)? .*;\s*$", "", site, flags=re.M)
            site = site.replace("127.0.0.1:8080", f"127.0.0.1:{upstream.server_port}")
            conf = directory / "nginx.conf"
            conf.write_text(
                f"master_process off; error_log stderr crit; pid {directory}/nginx.pid;\n"
                f"events {{ worker_connections 64; }}\nhttp {{ access_log off;\n"
                f"client_body_temp_path {directory}/body;\n"
                f"proxy_temp_path {directory}/proxy; fastcgi_temp_path {directory}/fastcgi;\n"
                f"uwsgi_temp_path {directory}/uwsgi; scgi_temp_path {directory}/scgi;\n{site}\n}}\n"
            )
            proc = subprocess.Popen(
                [NGINX, "-p", name + "/", "-c", str(conf), "-g", "daemon off;"],
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                for _ in range(50):
                    if proc.poll() is not None:
                        raise RuntimeError(proc.stderr.read().decode())
                    try:
                        request(port, "GET", "/api/v1/uploads", auth=False)
                        break
                    except OSError:
                        time.sleep(0.1)
                else:
                    raise RuntimeError("nginx did not listen")
                yield port, upstream
            finally:
                if proc.poll() is None:
                    proc.terminate()
                proc.communicate(timeout=5)
    finally:
        upstream.shutdown()
        upstream.server_close()
        thread.join(timeout=5)


class UploadRouting(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not NGINX:
            raise RuntimeError("Set NGINX_BIN or install nginx to run this regression test")
        cls.templates = templates()

    def test_old_location_reproduces_redirect_and_404(self):
        old = self.templates["certbot"].replace(
            "location /api/v1/uploads {", "location /api/v1/uploads/ {"
        )
        with proxy(old, alias=False) as (port, _):
            status, headers, _ = request(port, "GET", "/api/v1/uploads")
            self.assertEqual(status, 301)
            redirected = urlsplit(headers["Location"]).path
            self.assertEqual(redirected, "/api/v1/uploads/")
            self.assertEqual(request(port, "GET", redirected)[0], 404)

    def test_upload_collection_and_photo_without_redirect(self):
        for name, template in self.templates.items():
            with self.subTest(template=name), proxy(template) as (port, upstream):
                self.assertEqual(request(port, "GET", "/api/v1/uploads?resume=1")[0], 200)
                self.assertEqual(request(port, "GET", "/api/v1/uploads/")[0], 200)
                status, _, body = request(port, "POST", "/api/v1/uploads", b"{}")
                self.assertEqual(status, 201)
                item_id = json.loads(body)["id"]
                status, _, body = request(port, "PUT", f"/api/v1/uploads/{item_id}", PHOTO)
                self.assertEqual(status, 200)
                self.assertEqual(json.loads(body)["offset"], len(PHOTO))
                self.assertEqual(upstream.received, PHOTO)
                self.assertEqual(request(port, "POST", f"/api/v1/uploads/{item_id}/complete")[0], 200)

    def test_unauthenticated_requests_stay_unauthorized(self):
        for name, template in self.templates.items():
            with self.subTest(template=name), proxy(template) as (port, _):
                for path in ("/api/v1/uploads", "/api/v1/uploads/", "/api/v1/uploads/id"):
                    self.assertEqual(request(port, "GET", path, auth=False)[0], 401)


if __name__ == "__main__":
    unittest.main(verbosity=2)
