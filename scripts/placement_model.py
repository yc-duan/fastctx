"""A loopback Chat Completions fixture for the placement drill's goal sentinel."""
import json
import shlex
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class GoalMock:
    def __init__(self):
        self.requests = []
        self.errors = []
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
        results = [json.loads(message["content"]) for message in messages if message["role"] == "tool"]
        if not results:
            code = ("import json,os,sys\np=sys.argv[1];x=json.load(open(p));readonly=False\n"
                    "try: open(p+'.write','w').write('unexpected')\nexcept OSError: readonly=True\n"
                    "print(json.dumps({'objective':x['objective'],'evidence':(x.get('world_actions') or {}).get('full_path'),"
                    "'readonly':readonly,'world_hidden':not os.path.exists(p.split('/world/conversations/')[0]+'/world/node.sock')}))")
            name, arguments = "look", {"machine": full["machine"], "command": "python3 -c " + shlex.quote(code) + " " + shlex.quote(full["path"])}
        else:
            result = results[-1]
            if result.get("exit_code") != 0:
                raise AssertionError("the sentinel could not read its working-computer material: " + str(result))
            observed = json.loads(result["text"])
            if len(results) == 1 and (not observed.get("readonly") or not observed.get("world_hidden")):
                raise AssertionError("the original read-only observation walls must remain in force")
            if len(results) == 1 and observed.get("evidence"):
                code = "import json,sys;x=json.load(open(sys.argv[1]));print(json.dumps({'turns':len(x['turns']),'jobs':len(x['jobs']),'marker':'WORLD_FACTS_READ'}))"
                name, arguments = "look", {"machine": full["machine"], "command": "python3 -c " + shlex.quote(code) + " " + shlex.quote(observed["evidence"])}
            else:
                name, arguments = "report", {"sound": True, "reason": "PLACEMENT_REVIEW_CANARY"}
        return {"choices": [{"message": {"role": "assistant", "content": None, "tool_calls": [
            {"id": f"placement-{len(results)}", "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}
        ]}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 10, "completion_tokens": 10}}
