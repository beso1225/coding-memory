import json
import os
import subprocess
import signal
import sys
import time
import runtime as p

binary = sys.argv[1]
version = subprocess.check_output([binary, "--version"], text=True).strip().split()[-1]
result = {
    "binary": binary,
    "runtime_version": version,
    "root": str(p.ROOT),
    "cases": {},
}


def text(s):
    return [{"type": "text", "text": s}]


def run_exec(name, plans, args):
    for plan in plans:
        p.PLANS.put(plan)
    r = subprocess.run(
        [binary, "exec", "--skip-git-repo-check", "--json", *args],
        cwd=p.WORK,
        env={**os.environ, "CODEX_HOME": str(p.HOME_DIR)},
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=45,
    )
    lines = [json.loads(x) for x in r.stdout.splitlines()]
    result["cases"][name] = {"exit": r.returncode, "events": lines}
    (p.ROOT / (name + ".stderr")).write_text(r.stderr)
    return lines[0]["thread_id"]


normal = run_exec("exec_normal", [{"text": "first answer"}], ["first instruction"])
failed = run_exec("exec_failure", ["fail"], ["synthetic failure instruction"])
request_count = len(p.EVENTS)
p.PLANS.put({"delay": 0.5, "text": "unreachable exec answer"})
proc = subprocess.Popen(
    [
        binary,
        "exec",
        "--skip-git-repo-check",
        "--json",
        "synthetic interrupt instruction",
    ],
    cwd=p.WORK,
    env={**os.environ, "CODEX_HOME": str(p.HOME_DIR)},
    stdin=subprocess.DEVNULL,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
)
deadline = time.monotonic() + 20
while len(p.EVENTS) == request_count and time.monotonic() < deadline:
    time.sleep(0.01)
if len(p.EVENTS) == request_count:
    proc.kill()
    proc.communicate()
    raise TimeoutError("exec never reached the synthetic provider")
proc.send_signal(signal.SIGINT)
stdout, stderr = proc.communicate(timeout=20)
events = [json.loads(line) for line in stdout.splitlines()]
interrupted = events[0]["thread_id"]
result["cases"]["exec_interruption"] = {"exit": proc.returncode, "events": events}
(p.ROOT / "exec_interruption.stderr").write_text(stderr)
p.ROOT.joinpath("continue").touch()
stop = run_exec(
    "exec_stop",
    [{"text": "pre-continuation answer"}, {"text": "post-continuation answer"}],
    ["--dangerously-bypass-hook-trust", "continue once"],
)
p.ROOT.joinpath("continue").unlink()
client = p.Client(binary)
for name, thread_id in [
    ("exec_normal", normal),
    ("exec_failure", failed),
    ("exec_interruption", interrupted),
    ("exec_stop", stop),
]:
    result["cases"][name]["read"] = client.call(
        "thread/read", {"threadId": thread_id, "includeTurns": True}
    )


def start(name, plans, steering=False, interrupt=False):
    t = client.call(
        "thread/start", {"cwd": str(p.WORK), "config": {"bypass_hook_trust": True}}
    )["thread"]
    tid = t["id"]
    for plan in plans:
        p.PLANS.put(plan)
    turn = client.call(
        "turn/start", {"threadId": tid, "input": text(name + " instruction")}
    )["turn"]
    turn_id = turn["id"]
    extra = {}
    if steering:
        time.sleep(0.1)
        p.PLANS.put({"text": "after steering"})
        extra["steer"] = client.call(
            "turn/steer",
            {
                "threadId": tid,
                "expectedTurnId": turn_id,
                "input": text("synthetic steering instruction"),
            },
        )
    if interrupt:
        time.sleep(0.1)
        observer = p.Client(binary)
        extra["observer_read_before_interrupt"] = observer.call(
            "thread/read", {"threadId": tid, "includeTurns": True}
        )
        observer.close()
        extra["interrupt"] = client.call(
            "turn/interrupt", {"threadId": tid, "turnId": turn_id}
        )
    term = client.terminal(turn_id)
    result["cases"][name] = {
        "thread_id": tid,
        "turn_id": turn_id,
        "terminal": term,
        **extra,
        "read": client.call("thread/read", {"threadId": tid, "includeTurns": True}),
    }
    return tid, turn_id


tool_plan = {
    "items": [
        {
            "type": "function_call",
            "name": "exec_command",
            "call_id": "synthetic-call",
            "arguments": json.dumps(
                {"cmd": "printf synthetic-tool-result", "yield_time_ms": 1000}
            ),
        }
    ]
}
tool_thread, tool_turn = start("tools", [tool_plan, {"text": "tool answer"}])

# Append another turn and deliberately read the entire later thread.
p.PLANS.put({"text": "later answer"})
later = client.call(
    "turn/start", {"threadId": tool_thread, "input": text("later instruction")}
)["turn"]["id"]
client.terminal(later)
result["cases"]["later_read"] = {
    "thread_id": tool_thread,
    "earlier_turn_id": tool_turn,
    "later_turn_id": later,
    "read": client.call("thread/read", {"threadId": tool_thread, "includeTurns": True}),
}
turn_page = client.call(
    "thread/turns/list", {"threadId": tool_thread, "limit": 1, "itemsView": "full"}
)
older = client.call(
    "thread/turns/list",
    {
        "threadId": tool_thread,
        "limit": 1,
        "itemsView": "full",
        "cursor": turn_page["nextCursor"],
    },
)
items = client.call(
    "thread/items/list", {"threadId": tool_thread, "turnId": tool_turn, "limit": 100}
)
result["cases"]["pagination"] = {
    "newest": turn_page,
    "older": older,
    "earlier_items": items,
    "earlier_turn_id": tool_turn,
    "later_turn_id": later,
}
start("steering", [{"delay": 0.3, "text": "before steering"}], steering=True)
start("failure", ["fail"])
start("interruption", [{"delay": 2, "text": "unreachable answer"}], interrupt=True)

fork = client.call(
    "thread/fork",
    {
        "threadId": tool_thread,
        "lastTurnId": tool_turn,
        "config": {"bypass_hook_trust": True},
    },
)["thread"]
p.PLANS.put({"text": "fork answer"})
fork_turn = client.call(
    "turn/start", {"threadId": fork["id"], "input": text("fork instruction")}
)["turn"]["id"]
client.terminal(fork_turn)
result["cases"]["fork"] = {
    "parent_thread_id": tool_thread,
    "inherited_turn_id": tool_turn,
    "new_turn_id": fork_turn,
    "read": client.call("thread/read", {"threadId": fork["id"], "includeTurns": True}),
}

client.close()
client = p.Client(binary)
result["cases"]["cold_read"] = {
    "read": client.call("thread/read", {"threadId": tool_thread, "includeTurns": True})
}
resume = client.call(
    "thread/resume", {"threadId": tool_thread, "config": {"bypass_hook_trust": True}}
)
p.PLANS.put({"text": "resumed answer"})
rt = client.call(
    "turn/start", {"threadId": tool_thread, "input": text("resume instruction")}
)["turn"]["id"]
client.terminal(rt)
result["cases"]["resume"] = {
    "new_turn_id": rt,
    "read": client.call("thread/read", {"threadId": tool_thread, "includeTurns": True}),
}
p.PLANS.put({"text": "synthetic compacted summary"})
client.call("thread/compact/start", {"threadId": tool_thread})
deadline = time.monotonic() + 20
while time.monotonic() < deadline:
    if any(
        v.get("method") == "item/completed"
        and v["params"].get("item", {}).get("type") == "contextCompaction"
        for v in client.events
    ):
        break
    time.sleep(0.02)
result["cases"]["compaction"] = {
    "events": [v for v in client.events if "ompact" in json.dumps(v)],
    "read": client.call("thread/read", {"threadId": tool_thread, "includeTurns": True}),
}
ct = result["cases"]["compaction"]["read"]["thread"]["turns"][-1]["id"]
result["cases"]["compaction"]["terminal"] = client.terminal(ct)
result["cases"]["compaction"]["final_read"] = client.call(
    "thread/read", {"threadId": tool_thread, "includeTurns": True}
)
client.close()
result["requests"] = p.EVENTS
time.sleep(0.2)
result["hooks"] = [
    json.loads(x) for x in (p.ROOT / "hooks.jsonl").read_text().splitlines()
]
result["rollouts"] = {
    str(f): [json.loads(x) for x in f.read_text().splitlines()]
    for f in p.HOME_DIR.rglob("rollout*.jsonl")
}
(p.ROOT / "result.json").write_text(json.dumps(result, indent=2))
print(p.ROOT)
for name, case in result["cases"].items():
    turns = case.get("read", {}).get("thread", {}).get("turns", [])
    print(
        name,
        case.get("exit"),
        [(x["id"], x["status"], [y["type"] for y in x["items"]]) for x in turns],
    )
