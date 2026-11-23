#!/usr/bin/env python3
"""Serves game-tree/ at http://127.0.0.1:8781/game-tree.html for testing the explorer.

POST /save/<name>.png writes a picture into game-tree/screenshots/full/ (not committed; smaller JPEGs of the
chosen ones go in game-tree/screenshots/).
Only this computer can reach it (it listens on 127.0.0.1).

    python game-tree/serve.py [port]
"""
import functools
import http.server
import os
import re
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))


class Handler(http.server.SimpleHTTPRequestHandler):
    def do_POST(self):
        m = re.fullmatch(r"/save/([A-Za-z0-9_-]{1,64}\.png)", self.path)
        if not m:
            self.send_error(404)
            return
        n = int(self.headers.get("Content-Length", "0"))
        if n <= 0 or n > 60_000_000:
            self.send_error(400)
            return
        data = self.rfile.read(n)
        if not data.startswith(b"\x89PNG"):
            self.send_error(400, "not a PNG")
            return
        folder = os.path.join(ROOT, "screenshots", "full")
        os.makedirs(folder, exist_ok=True)
        with open(os.path.join(folder, m.group(1)), "wb") as f:
            f.write(data)
        self.send_response(204)
        self.end_headers()

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


class Server(http.server.ThreadingHTTPServer):
    allow_reuse_address = False  # on Windows, reuse would let two servers share a port


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8781
    server = Server(("127.0.0.1", port), functools.partial(Handler, directory=ROOT))
    print(f"serving {ROOT} at http://127.0.0.1:{port}/", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
