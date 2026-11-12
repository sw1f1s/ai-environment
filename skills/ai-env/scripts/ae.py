#!/usr/bin/env python3
"""Run the packaged AI Env CLI without loading implementation into model context."""
import os
from pathlib import Path
import shutil
import sys


def main():
    explicit = os.environ.get("AI_ENV_BIN")
    bundled = Path(__file__).resolve().parent / "bin" / "aienv"
    binary = Path(explicit).expanduser() if explicit else bundled
    if not explicit and not binary.is_file():
        installed = shutil.which("aienv")
        if installed:
            binary = Path(installed)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        sys.exit("AI Env binary unavailable. Build/package this skill or set AI_ENV_BIN to an executable aienv.")
    os.execv(str(binary.resolve()), [str(binary.resolve()), *sys.argv[1:]])


if __name__ == "__main__":
    main()
