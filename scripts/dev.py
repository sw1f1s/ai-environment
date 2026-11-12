#!/usr/bin/env python3
"""Build/package or check this Rust repository with the available local toolchain."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]

def prepare_runtimes(env):
    """Record trusted host tools in the tool bundle, never target-project config."""
    runtimes = {}
    node = env.get("AI_ENV_NODE") or shutil.which("node", path=env.get("PATH"))
    local_node = ROOT / ".tools/node/current/bin/node"
    if not node and local_node.is_file():
        node = str(local_node)
    if node:
        runtimes["node"] = str(Path(node).resolve())
        env["AI_ENV_NODE"] = runtimes["node"]
    analyzer = env.get("AI_ENV_RUST_ANALYZER")
    rustup = shutil.which("rustup", path=env.get("PATH"))
    if not analyzer and rustup:
        result = subprocess.run([rustup, "which", "rust-analyzer"], env=env, cwd=ROOT, capture_output=True, text=True)
        if result.returncode == 0:
            analyzer = result.stdout.strip()
    analyzer = analyzer or shutil.which("rust-analyzer", path=env.get("PATH"))
    if analyzer:
        # A rustup proxy needs its invocation basename, even when it is a symlink.
        runtimes["rust_analyzer"] = os.path.abspath(analyzer)
        env["AI_ENV_RUST_ANALYZER"] = runtimes["rust_analyzer"]
    sysroot = env.get("AI_ENV_RUST_SYSROOT")
    rustc = shutil.which("rustc", path=env.get("PATH"))
    if not sysroot and rustc:
        result = subprocess.run([rustc, "--print", "sysroot"], env=env, cwd=ROOT, capture_output=True, text=True)
        if result.returncode == 0:
            sysroot = result.stdout.strip()
    if sysroot:
        runtimes["rust_sysroot"] = str(Path(sysroot).resolve())
        env["AI_ENV_RUST_SYSROOT"] = runtimes["rust_sysroot"]
    output = ROOT / "skills/ai-env/scripts/bin/runtimes.json"
    output.parent.mkdir(exist_ok=True)
    encoded = json.dumps(runtimes, indent=2) + "\n"
    if not output.exists() or output.read_text() != encoded:
        output.write_text(encoded)
    return runtimes


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["build", "check"])
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--without-csharp", action="store_true", help="build only: omit the optional Roslyn helper")
    args = parser.parse_args()
    env = os.environ.copy()
    local = ROOT / ".tools/cargo/bin/cargo"
    cargo = str(local) if local.exists() else shutil.which("cargo")
    if not cargo:
        parser.error("Rust cargo is required")
    if local.exists():
        env["CARGO_HOME"] = str(ROOT / ".tools/cargo")
        env["RUSTUP_HOME"] = str(ROOT / ".tools/rustup")
        env["PATH"] = str(local.parent) + os.pathsep + env.get("PATH", "")
    prepare_runtimes(env)
    common = ["--locked"] + (["--offline"] if args.offline else [])
    if args.without_csharp and args.command != "build":
        parser.error("--without-csharp applies only to build; check exercises all shipped adapters")
    subprocess.run([sys.executable, str(ROOT / "scripts/vendor.py")], cwd=ROOT, env=env, check=True)
    if args.command == "check":
        subprocess.run([cargo, "fmt", "--", "--check"], cwd=ROOT, env=env, check=True)
        # A successful product check must exercise the actual compilers, not skipped fixtures.
        env["AI_ENV_REQUIRE_SEMANTIC_BACKENDS"] = "1"
        subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py", "-v"], cwd=ROOT, env=env, check=True)
        subprocess.run([cargo, "test", *common, "--all-targets"], cwd=ROOT, env=env, check=True)
    else:
        subprocess.run([cargo, "build", "--release", *common], cwd=ROOT, env=env, check=True)
        destination = ROOT / "skills/ai-env/scripts/bin/aienv"
        destination.parent.mkdir(exist_ok=True)
        shutil.copy2(ROOT / "target/release/aienv", destination)
        destination.chmod(0o755)
        if not args.without_csharp:
            subprocess.run([sys.executable, str(ROOT / "skills/ai-env/scripts/analyzers/csharp/build.py")], cwd=ROOT, env=env, check=True)
        else:
            # Do not accidentally ship an earlier helper when C# was omitted.
            shutil.rmtree(destination.parent / "roslyn", ignore_errors=True)
        print(destination)


if __name__ == "__main__":
    main()
