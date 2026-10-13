import atexit
import http.server
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(
    tempfile.mkdtemp(prefix="coding-memory-capture-", dir="/private/tmp")
).resolve()
HOME_DIR = ROOT / "state"
HOME_DIR.mkdir()
WORK = ROOT / "project"
WORK.mkdir()
EVENTS = []
PLANS = queue.Queue()


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"data":[]}')

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        EVENTS.append(
            {
                "kind": "request",
                "path": self.path,
                "time": time.monotonic(),
                "tool_names": [x.get("name") for x in body.get("tools", [])],
            }
        )
        plan = PLANS.get(timeout=15)
        if plan == "fail":
            self.send_response(400)
            self.end_headers()
            self.wfile.write(
                b'{"error":{"message":"synthetic failure","type":"invalid_request_error"}}'
            )
            return
        delay = plan.get("delay", 0)
        if delay:
            time.sleep(delay)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        response_id = "synthetic-" + str(len(EVENTS))
        items = plan.get(
            "items",
            [
                {
                    "type": "message",
                    "id": "msg-" + response_id,
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": plan.get("text", "synthetic answer"),
                        }
                    ],
                }
            ],
        )
        events = [{"type": "response.created", "response": {"id": response_id}}]
        events += [
            {"type": "response.output_item.done", "item": item} for item in items
        ]
        events += [
            {
                "type": "response.completed",
                "response": {
                    "id": response_id,
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
                },
            }
        ]
        try:
            for event in events:
                self.wfile.write(
                    (
                        "event: "
                        + event["type"]
                        + "\ndata: "
                        + json.dumps(event)
                        + "\n\n"
                    ).encode()
                )
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


SERVER = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
threading.Thread(target=SERVER.serve_forever, daemon=True).start()

HOOK = ROOT / "hook.py"
HOOK.write_text("""import json,sys,time,pathlib
root=pathlib.Path(sys.argv[1])
event=json.loads(sys.argv[2]) if len(sys.argv)>2 else json.load(sys.stdin)
paths=list((root/'state'/'sessions').rglob('*.jsonl'))
tails={str(p):p.read_text().splitlines()[-3:] for p in paths}
with (root/'hooks.jsonl').open('a') as f:
 f.write(json.dumps({'time':time.monotonic(),'event':event,'tails':tails})+'\\n')
if (root/'continue').exists() and event.get('hook_event_name')=='Stop' and not event.get('stop_hook_active'):
 print(json.dumps({'decision':'block','reason':'Synthetic one-time continuation.'}))
""")
CONFIG = f"""model = "capture-test"
model_provider = "mock"
approval_policy = "never"
sandbox_mode = "read-only"
project_doc_max_bytes = 0
notify = [{json.dumps(sys.executable)}, {json.dumps(str(HOOK))}, {json.dumps(str(ROOT))}]
[features]
hooks = true
plugins = false
[model_providers.mock]
name = "Loopback synthetic provider"
base_url = "http://127.0.0.1:{SERVER.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
"""
(HOME_DIR / "config.toml").write_text(CONFIG)
(HOME_DIR / "hooks.json").write_text(
    json.dumps(
        {
            "hooks": {
                event: [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": f"{sys.executable} {HOOK} {ROOT}",
                                "timeout": 10,
                            }
                        ]
                    }
                ]
                for event in [
                    "SessionStart",
                    "UserPromptSubmit",
                    "Stop",
                    "Interrupt",
                    "PostToolUse",
                    "PreCompact",
                    "PostCompact",
                ]
            }
        }
    )
)


class Client:
    def __init__(self, binary):
        self.lines = queue.Queue()
        self.events = []
        self.counter = 0
        self.stderr = (ROOT / ("stderr-" + str(len(EVENTS)) + ".log")).open("w")
        self.proc = subprocess.Popen(
            [binary, "app-server"],
            cwd=WORK,
            env={**os.environ, "CODEX_HOME": str(HOME_DIR)},
            text=True,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr,
        )
        atexit.register(self.close)

        def read():
            for line in self.proc.stdout:
                value = json.loads(line)
                self.events.append(value)
                self.lines.put(value)

        threading.Thread(target=read, daemon=True).start()
        self.call(
            "initialize",
            {
                "clientInfo": {"name": "coding_memory_capture_probe", "version": "0.1"},
                "capabilities": {"experimentalApi": True},
            },
        )
        self.send({"method": "initialized", "params": {}})

    def send(self, value):
        self.proc.stdin.write(json.dumps(value) + "\n")
        self.proc.stdin.flush()

    def call(self, method, params):
        self.counter += 1
        ident = self.counter
        self.send({"id": ident, "method": method, "params": params})
        while True:
            value = self.lines.get(timeout=30)
            if value.get("id") == ident:
                if "error" in value:
                    raise RuntimeError(value["error"])
                return value["result"]

    def terminal(self, tid):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            for value in self.events:
                if (
                    value.get("method") == "turn/completed"
                    and value["params"]["turn"]["id"] == tid
                ):
                    return value["params"]
            time.sleep(0.01)
        raise TimeoutError(tid)

    def close(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.stderr.close()
