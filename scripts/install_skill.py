#!/usr/bin/env python3
"""Copy the built, self-contained skill into one project's discovery directory."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "skills/ai-env"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", type=Path, required=True)
    args = parser.parse_args()
    project = args.project.expanduser().resolve(strict=True)
    if not project.is_dir():
        parser.error("Project must be a directory")
    binary = SOURCE / "scripts/bin/aienv"
    if not binary.is_file():
        parser.error("Run python3 scripts/dev.py build first")
    subprocess.run([sys.executable, str(ROOT / "scripts/vendor.py")], stdout=subprocess.DEVNULL, check=True)
    parent = project / ".agents/skills"
    for path in [project / ".agents", parent]:
        if path.is_symlink() or path.exists() and not path.is_dir():
            parser.error("Skill discovery directory must not be linked or a file")
    parent.mkdir(parents=True, exist_ok=True)
    destination = parent / "ai-env"
    if destination.exists() or destination.is_symlink():
        parser.error(f"Refusing to overwrite an existing skill: {destination}")
    with tempfile.TemporaryDirectory(prefix=".ai-env-install-", dir=parent) as temporary:
        staged = Path(temporary) / "ai-env"
        shutil.copytree(SOURCE, staged, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        assert (staged / "SKILL.md").is_file()
        assert hashlib.sha256((staged / "scripts/bin/aienv").read_bytes()).digest() == hashlib.sha256(binary.read_bytes()).digest()
        staged.rename(destination)
    print(json.dumps({"installed": str(destination), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}))


if __name__ == "__main__":
    main()
