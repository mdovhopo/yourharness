"""A small JSON API over http.server."""

import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse

from .places import search


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/places":
            q = parse_qs(url.query)
            near = None
            if "lat" in q and "lon" in q:
                near = (float(q["lat"][0]), float(q["lon"][0]))
            places = search(q.get("q", [""])[0], near, int(q.get("limit", ["10"])[0]))
            self.reply(200, [p.__dict__ for p in places])
        elif url.path == "/health":
            self.reply(200, {"ok": True})
        else:
            self.reply(404, {"error": "not found"})

    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    print("atlas-api on http://localhost:8080")
    HTTPServer(("127.0.0.1", 8080), Handler).serve_forever()
