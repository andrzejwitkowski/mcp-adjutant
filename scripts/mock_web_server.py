#!/usr/bin/env python3
"""Local mock DDG + example.com pages for web_fetch integration tests."""
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

ROOT = Path(__file__).resolve().parents[1]
DDG_HTML = """<html><body>
<div class="result">
  <h2 class="result__title"><a href="http://127.0.0.1:{port}/tokio/docs" class="result__a">Tokio Async Runtime</a></h2>
  <a class="result__snippet" href="http://127.0.0.1:{port}/tokio/docs">Official Tokio async runtime documentation covering spawn, channels, and timers.</a>
</div>
<div class="result">
  <h2 class="result__title"><a href="http://127.0.0.1:{port}/rust-async/book" class="result__a">Asynchronous Programming in Rust</a></h2>
  <a class="result__snippet" href="http://127.0.0.1:{port}/rust-async/book">Guide to async Rust including executors and futures.</a>
</div>
</body></html>"""
PAGES = {
    "/tokio/docs": "# Tokio docs\n\nUse `tokio::spawn` for async tasks, `spawn_blocking` for CPU/blocking work, and channels (`mpsc`, `oneshot`) to communicate between tasks.\n",
    "/rust-async/book": "# Async Rust book\n\nCovers executors, futures, and when to prefer message passing over shared state.\n",
}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/html/":
            body = DDG_HTML.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        for suffix, markdown in PAGES.items():
            if parsed.path == suffix:
                html = f"<html><body><h1>Mock page</h1><pre>{markdown}</pre></body></html>"
                body = html.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return

        self.send_response(404)
        self.end_headers()


def main():
    port = int(__import__("os").environ.get("MOCK_WEB_PORT", "8765"))
    global DDG_HTML
    DDG_HTML = DDG_HTML.format(port=port)
    server = HTTPServer(("127.0.0.1", port), Handler)
    print(f"mock web server on http://127.0.0.1:{port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
