import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

LABELS = "/tmp/rtk-cowork/labels.json"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, code, body):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(code)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.reply(200, b"ok")

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)))
            labels = json.load(open(LABELS))
        except (OSError, ValueError):
            return self.reply(400, {"error": "no labels"})
        answers = {}
        for q in body.get("questions") or {}:
            if q in labels:
                answers[q] = {"type": "choice", "choice": labels[q].get("choice"), "confidence": labels[q].get("confidence", 0.0)}
        self.reply(200, {"model": "laya", "answers": answers})


if __name__ == "__main__":
    try:
        HTTPServer(("127.0.0.1", 41733), Handler).serve_forever()
    except OSError:
        pass
