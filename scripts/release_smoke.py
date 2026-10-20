"""Test the unpacked distribution without developer tools or personal state."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import queue
import subprocess
import tarfile
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]


def extract(archive, destination):
    with tarfile.open(archive) as stream:
        for member in stream.getmembers():
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unsafe archive entry: {member.name}")
        stream.extractall(destination, filter="data")
    binaries = list(destination.rglob("coding-memory"))
    if len(binaries) != 1 or not binaries[0].is_file():
        raise ValueError("Archive must contain exactly one coding-memory binary")
    return binaries[0]


def smoke(archive, version):
    with tempfile.TemporaryDirectory(prefix="coding-memory-release-") as directory:
        root = Path(directory)
        binary = extract(archive, root / "unpacked")
        payload = binary.parent
        for name in ["LICENSE-MIT", "LICENSE-APACHE", "README.md", "install.md",
                     "release-notices/THIRD-PARTY-NOTICES.txt", "release-notices/inventory.json",
                     "release-notices/rust-runtime/COPYRIGHT-library.html"]:
            if not (payload / name).is_file():
                raise ValueError(f"Missing archive content: {name}")
        inventory = json.loads((payload / "release-notices/inventory.json").read_text())
        if inventory["version"] != version or inventory["target"] != "aarch64-apple-darwin":
            raise ValueError("Archive inventory version/target mismatch")
        lock_hash = hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest()
        if inventory["cargo_lock_sha256"] != lock_hash:
            raise ValueError("Archive inventory does not match Cargo.lock")
        for item in inventory["rust_runtime_notices"]:
            data = (payload / "release-notices" / item["path"]).read_bytes()
            if hashlib.sha256(data).hexdigest() != item["sha256"]:
                raise ValueError(f"Runtime notice hash mismatch: {item['path']}")
        arch = subprocess.check_output(["/usr/bin/lipo", "-archs", binary], text=True).strip()
        if arch != "arm64":
            raise ValueError(f"Unexpected binary architecture: {arch}")
        linkage = subprocess.check_output(["/usr/bin/otool", "-L", binary], text=True)
        for line in linkage.splitlines()[1:]:
            library = line.strip().split(" (", 1)[0]
            if not library.startswith(("/usr/lib/", "/System/Library/")):
                raise ValueError(f"Unexpected non-system library: {library}")
        # The DSH capture bridge ships as a runtime dependency; a release user
        # mounts it by absolute path, so it must be inside the version directory.
        # cargo-dist flattens each include entry to the archive root, so the
        # plugin lands beside the binary rather than under its source path.
        bridge = payload / "index.mjs"
        if not bridge.is_file():
            raise ValueError("Missing archive content: index.mjs (the DSH capture bridge)")
        if "export function apply" not in bridge.read_text():
            raise ValueError("Packaged bridge is not the capture plugin")
        print(linkage)
        build = subprocess.check_output(["/usr/bin/vtool", "-show-build", binary], text=True)
        minimum = [line.split()[1] for line in build.splitlines() if line.strip().startswith("minos ")]
        if minimum != ["15.0"]:
            raise ValueError(f"Expected macOS deployment target 15.0: {minimum}")
        home = root / "home"
        home.mkdir()
        env = {"HOME": str(home), "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TMPDIR": str(root), "LANG": "en_US.UTF-8"}

        def run(*args):
            return subprocess.check_output([binary, *map(str, args)], cwd=root, env=env, text=True, timeout=15)

        if run("--version").strip() != f"coding-memory {version}":
            raise ValueError("CLI version mismatch")
        run("--help")
        state, vault = root / "state", root / "vault"
        vault.mkdir()
        (vault / "queue.md").write_bytes((ROOT / "tests/fixtures/vault/queue.md").read_bytes())
        json.loads(run("--state-dir", state, "status"))
        run("--state-dir", state, "index-rebuild", "--vault", vault)
        result = json.loads(run("--state-dir", state, "search", "--vault", vault,
                               "--project-id", "alpha", "sync_all"))
        if result["results"][0]["metadata"]["id"] != "queue-leases":
            raise ValueError("Packaged SQLite FTS retrieval failed")
        with subprocess.Popen(
            [binary, "--state-dir", state, "mcp", "--vault", vault, "--project-id", "alpha"],
            cwd=root, env=env, text=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        ) as child:
            responses = queue.Queue()

            def read():
                for line in child.stdout:
                    responses.put(line)
                responses.put(None)

            threading.Thread(target=read, daemon=True).start()

            def send(message, response=True):
                child.stdin.write(json.dumps(message) + "\n")
                child.stdin.flush()
                if not response:
                    return None
                while True:
                    line = responses.get(timeout=10)
                    if line is None:
                        raise ValueError("MCP closed before responding")
                    value = json.loads(line)
                    if value.get("id") == message["id"]:
                        if "error" in value:
                            raise ValueError(f"MCP error: {value}")
                        return value["result"]

            try:
                send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": {"name": "release-smoke", "version": "1"}}})
                send({"jsonrpc": "2.0", "method": "notifications/initialized"}, response=False)
                tools = send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
                if {tool["name"] for tool in tools["tools"]} != {"memory_search", "memory_read"}:
                    raise ValueError("Unexpected MCP tool set")
                found = send({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                    "name": "memory_search", "arguments": {"query": "sync_all"}}})
                if found["structuredContent"]["results"][0]["metadata"]["id"] != "queue-leases":
                    raise ValueError("Packaged MCP search failed")
                child.stdin.close()
                if child.wait(timeout=10) != 0:
                    raise ValueError("MCP exited unsuccessfully")
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
        print(f"Archive smoke passed: {archive.name} ({version}); clean HOME and system-only PATH")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    smoke(args.archive.resolve(), args.version)
