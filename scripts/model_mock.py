"""A stand-in for a model vendor, for drills that must send no request to one: an HTTP server on
127.0.0.1 that answers the Responses API as a streaming endpoint, the way Codex asks a model provider
whose `base_url` points at it.

What the "model" does is said in the latest user input, by markers:

    [[run: COMMAND]]   call the World's `run` tool with COMMAND, then answer once its output comes
    [[slow: SECONDS]]  wait that long before answering
    [[hang]]           never answer: the stream opens and nothing comes on it
    [[fail]]           answer with an HTTP 500

Without a marker it answers "ok". Every request is kept (`requests`) so a drill can see what a
session sent: the tools it was given, the inputs it carried.

    python scripts/model_mock.py [PORT]   runs one in the foreground, for trying it by hand
"""
import json
import re
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MARKER = re.compile(r"\[\[(run|slow|hang|fail)(?::\s*(.*?))?\]\]", re.S)


class Quiet(ThreadingHTTPServer):
    """A server that says nothing when a client goes before its answer is written."""

    def handle_error(self, request, client_address):
        if not isinstance(sys.exc_info()[1], (ConnectionError, TimeoutError)):
            super().handle_error(request, client_address)


class Mock:
    def __init__(self, port=0):
        self.requests = []
        self.lock = threading.Lock()
        mock = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def do_GET(self):
                if self.path.rstrip("/").endswith("/models"):
                    body = json.dumps({"object": "list", "data": [{"id": "mock-model", "object": "model"}]}).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                self.send_error(404)

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(length) if length else b""
                try:
                    request = json.loads(raw or b"{}")
                except ValueError:
                    request = {}
                with mock.lock:
                    mock.requests.append({"path": self.path, "body": request})
                if not self.path.rstrip("/").endswith("/responses"):
                    self.send_error(404)
                    return
                mock.respond(self, request)

        self.server = Quiet(("127.0.0.1", port), Handler)
        self.server.daemon_threads = True
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def base_url(self):
        return f"http://127.0.0.1:{self.port}/v1"

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def respond(self, handler, request):
        items = request.get("input") or []
        # Once the World's tool has answered, the turn's answer follows it.
        if items and isinstance(items[-1], dict) and items[-1].get("type") == "function_call_output":
            return stream(handler, [message("done")])
        text = last_user_text(items)
        found = MARKER.search(text or "")
        if not found:
            return stream(handler, [message("ok")])
        kind, argument = found.group(1), (found.group(2) or "").strip()
        if kind == "fail":
            body = b'{"error":{"message":"the mock was told to fail"}}'
            handler.send_response(500)
            handler.send_header("Content-Type", "application/json")
            handler.send_header("Content-Length", str(len(body)))
            handler.end_headers()
            handler.wfile.write(body)
            return
        if kind == "hang":
            # Nothing comes on the stream: the session's own idle limit ends it.
            start(handler)
            time.sleep(600)
            return
        if kind == "slow":
            time.sleep(float(argument or "5"))
            return stream(handler, [message("ok")])
        tool = run_tool(request.get("tools") or [])
        if tool is None:
            return stream(handler, [message("no run tool was offered")])
        namespace, name = tool
        call = {"type": "function_call", "call_id": f"call_{uuid.uuid4().hex[:12]}", "name": name,
                "arguments": json.dumps({"command": argument, "wait": 600})}
        if namespace:
            call["namespace"] = namespace
        return stream(handler, [call])


def last_user_text(items):
    for item in reversed(items):
        if not isinstance(item, dict) or item.get("role") != "user":
            continue
        content = item.get("content")
        if isinstance(content, str):
            return content
        parts = [part.get("text", "") for part in content or [] if isinstance(part, dict)]
        return "\n".join(parts)
    return ""


def run_tool(tools):
    """The World's `run` tool as this request offers it: its namespace, when it is in one, and its name."""
    for tool in tools:
        if not isinstance(tool, dict):
            continue
        if tool.get("type") == "namespace":
            for inner in tool.get("tools") or []:
                if isinstance(inner, dict) and inner.get("name") == "run":
                    return tool.get("name"), "run"
            continue
        name = tool.get("name") or (tool.get("function") or {}).get("name") or ""
        if name == "run" or name.endswith("__run"):
            return None, name
    return None


def message(text):
    return {"type": "message", "role": "assistant", "id": f"msg_{uuid.uuid4().hex[:12]}",
            "content": [{"type": "output_text", "text": text}]}


def start(handler):
    handler.send_response(200)
    handler.send_header("Content-Type", "text/event-stream")
    handler.send_header("Cache-Control", "no-cache")
    handler.send_header("Connection", "close")
    handler.end_headers()


def stream(handler, items):
    response = f"resp_{uuid.uuid4().hex[:12]}"
    events = [{"type": "response.created", "response": {"id": response}}]
    for item in items:
        events.append({"type": "response.output_item.added", "output_index": 0, "item": item})
        events.append({"type": "response.output_item.done", "output_index": 0, "item": item})
    events.append({"type": "response.completed", "response": {
        "id": response,
        "usage": {"input_tokens": 10, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 2,
                  "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 12}}})
    start(handler)
    try:
        for event in events:
            handler.wfile.write(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n".encode())
            handler.wfile.flush()
    except OSError:
        pass
    handler.close_connection = True


if __name__ == "__main__":
    mock = Mock(int(sys.argv[1]) if len(sys.argv) > 1 else 0)
    print(f"model mock at {mock.base_url}", flush=True)
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        mock.close()
