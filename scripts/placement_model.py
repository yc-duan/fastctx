"""A loopback Chat Completions fixture for the placement drill's goal sentinel."""
import json
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class GoalMock:
    def __init__(self):
        self.requests = []
        self.errors = []
        self.evidence = None
        self.world_facts_read = False
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                owner.requests.append(request)
                try:
                    body = owner.answer(request)
                except Exception as error:
                    owner.errors.append(str(error))
                    self.send_error(500, str(error))
                    return
                data = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.endpoint = f"http://127.0.0.1:{self.server.server_address[1]}/v1/chat/completions"

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def answer(self, request):
        messages = request["messages"]
        inputs = [json.loads(message["content"]) for message in messages if message["role"] == "user"]
        review = inputs[-1]
        full = review["full"]
        if full["machine"] != "alice-two":
            raise AssertionError("the goal's complete materials must be on its working computer")
        results = [observed(message["content"]) for message in messages if message["role"] == "tool"]
        machine = full["machine"]
        if not results:
            name, arguments = "read_file", {"machine": machine, "path": full["path"]}
        elif len(results) == 1:
            material = results[0]
            if "PLACEMENT_GOAL_CANARY" not in material:
                raise AssertionError("the sentinel could not read its working-computer material: " + material[:500])
            found = re.search(r'"full_path"\s*:\s*"([^"]+)"', material)
            self.evidence = found.group(1) if found else None
            world = full["path"].split("/world/conversations/")[0] + "/world"
            name, arguments = "list_directory", {"machine": machine, "path": world}
        elif len(results) == 2:
            if "World's own directory" not in results[1]:
                raise AssertionError("a look read the World's own directory: " + results[1][:500])
            if self.evidence:
                name, arguments = "read_file", {"machine": machine, "path": self.evidence, "matching": "turns"}
            else:
                name, arguments = "report", {"sound": True, "reason": "PLACEMENT_REVIEW_CANARY"}
        else:
            if '"turns"' not in results[2] or '"jobs"' not in results[2]:
                raise AssertionError("the sentinel could not read the World's facts of the work: " + results[2][:500])
            self.world_facts_read = True
            name, arguments = "report", {"sound": True, "reason": "PLACEMENT_REVIEW_CANARY"}
        return {"choices": [{"message": {"role": "assistant", "content": None, "tool_calls": [
            {"id": f"placement-{len(results)}", "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}
        ]}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 10, "completion_tokens": 10}}


def observed(content):
    """What a look saw, as its tool answer carries it, or why it was refused."""
    try:
        answer = json.loads(content)
    except ValueError:
        return content
    return answer.get("text", content) if isinstance(answer, dict) else content
