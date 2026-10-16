"""Build a conservative, locked-target license bundle using only local sources."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
TARGET = "aarch64-apple-darwin"
REVIEWED = {
    "MIT", "Apache-2.0", "MIT OR Apache-2.0", "Apache-2.0 OR MIT",
    "MIT/Apache-2.0", "Zlib", "Unlicense OR MIT", "Apache-2.0 OR BSL-1.0",
    "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
    "(MIT OR Apache-2.0) AND Unicode-3.0",
}
SDK_REVISION = "79437f291b2c44053d00dcd5db969fd0cca7c887"
SDK_LICENSE_HASH = "0382b0057770ca05e9c350a50aa3b1c1fea84da0bc81d723bf00b9aa841be58a"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def package_notices(package):
    name, version = package["name"], package["version"]
    if package["license"] not in REVIEWED:
        raise ValueError(f"Unreviewed license: {name} {version}: {package['license']}")
    root = Path(package["manifest_path"]).parent
    files = []
    for file in sorted(root.rglob("*")):
        relative = file.relative_to(root)
        # The current target uses bundled SQLite, not the SQLCipher source tree.
        if name == "libsqlite3-sys" and relative.parts[0] == "sqlcipher":
            continue
        if file.is_file() and file.name.upper().startswith(
            ("LICENSE", "LICENCE", "NOTICE", "COPYING", "COPYRIGHT")
        ):
            files.append((relative.as_posix(), file.read_bytes()))
    if name in {"rmcp", "rmcp-macros"}:
        vcs = json.loads((root / ".cargo_vcs_info.json").read_text())
        if version != "3.5.1" or vcs["git"]["sha1"] != SDK_REVISION:
            raise ValueError(f"SDK source revision needs review: {name} {version}")
        supplement = (ROOT / "third-party/rmcp-LICENSE").read_bytes()
        if digest(supplement) != SDK_LICENSE_HASH:
            raise ValueError("SDK license supplement hash changed; review required")
        files.append((f"upstream/{SDK_REVISION}/LICENSE", supplement))
    if not any("LICEN" in name.upper() or "COPYING" in name.upper() for name, _ in files):
        raise ValueError(f"Missing license text: {name} {version}")
    if "Unicode-3.0" in package["license"] and not any("UNICODE" in name.upper() for name, _ in files):
        raise ValueError(f"Missing license text for Unicode: {name} {version}")
    text = f"\n{'=' * 72}\n{name} {version}\nDeclared license: {package['license']}\n"
    inventory = []
    for filename, data in files:
        text += f"\n--- {filename} ---\n{data.decode('utf-8')}\n"
        inventory.append({"path": filename, "sha256": digest(data)})
    return text, inventory


def generate(output):
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--offline", "--filter-platform", TARGET,
        "--format-version", "1",
    ], cwd=ROOT))
    package = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    compiler = subprocess.check_output(["rustc", "--version"], text=True).strip()
    if compiler.split()[1] != toolchain:
        raise ValueError(f"Expected Rust {toolchain}, found {compiler}")
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    sqlite = next(node for node in nodes.values() if "libsqlite3-sys@" in node["id"])
    if any("sqlcipher" in feature for feature in sqlite["features"]):
        raise ValueError("SQLCipher features require a new license review")
    header = (
        "Third-party notices for Coding Memory\n\n"
        "Original project code: MIT OR Apache-2.0. Third-party components retain\n"
        "their own terms below. This conservative inventory also includes build\n"
        "tools and procedural macros; it is not an assertion that every component\n"
        "is linked into the executable. Alternative license texts are retained.\n\n"
        "Bundled SQLite is dedicated to the public domain by its authors.\n"
        "See https://sqlite.org/copyright.html . Rust wrapper notices follow.\n"
        "Rust library attributions are supplied separately in rust-runtime/.\n"
    )
    entries = []
    for item in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if item["id"] == metadata["resolve"]["root"]:
            continue
        text, files = package_notices(item)
        header += text
        entries.append({"name": item["name"], "version": item["version"],
                        "license": item["license"], "source": item["source"],
                        "repository": item["repository"], "notices": files})
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    rust_docs = sysroot / "share/doc/rust"
    copyright_file = rust_docs / "COPYRIGHT-library.html"
    if not copyright_file.is_file() or not (rust_docs / "licenses").is_dir():
        raise ValueError("Missing Rust runtime notices; install the rust-docs component")
    output.mkdir(parents=True, exist_ok=True)
    runtime = output / "rust-runtime"
    if runtime.exists():
        # Nix store directories are read-only; do not retain their modes when
        # preparing an ordinary, repeatable build output.
        for directory in [runtime, *[p for p in runtime.rglob("*") if p.is_dir()]]:
            directory.chmod(0o755)
        shutil.rmtree(runtime)
    runtime.mkdir()
    shutil.copyfile(copyright_file, runtime / copyright_file.name)
    for source in sorted((rust_docs / "licenses").rglob("*")):
        destination = runtime / "licenses" / source.relative_to(rust_docs / "licenses")
        if source.is_dir():
            destination.mkdir(parents=True, exist_ok=True)
        elif source.is_file():
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
    (output / "THIRD-PARTY-NOTICES.txt").write_text(header)
    inventory = {"version": package["version"], "target": TARGET,
                 "cargo_lock_sha256": digest((ROOT / "Cargo.lock").read_bytes()),
                 "compiler": compiler, "packages": entries,
                 "rust_runtime_notices": [
                     {"path": f.relative_to(output).as_posix(), "sha256": digest(f.read_bytes())}
                     for f in sorted(runtime.rglob("*")) if f.is_file()
                 ]}
    (output / "inventory.json").write_text(json.dumps(inventory, indent=2) + "\n")
    print(f"Prepared {len(entries)} dependency notices plus Rust runtime notices in {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/release-notices")
    generate(parser.parse_args().output.resolve())
