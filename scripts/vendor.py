#!/usr/bin/env python3
"""Verify the exact vendored source trees without downloading or importing them."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_VENDOR = ROOT / "skills/ai-env/scripts/vendor"
DEFAULT_LOCK = ROOT / "skills/ai-env/vendor-lock.json"


def inventory(vendor, packages):
    """Hash names and contents, with unambiguous length-prefixed records."""
    if vendor.is_symlink() or not vendor.is_dir():
        raise ValueError("Vendor directory must be a real directory")
    roots = {}
    for package in packages:
        for name in package["paths"]:
            path = Path(name)
            if path.is_absolute() or len(path.parts) != 1 or name in roots:
                raise ValueError(f"Invalid or duplicate vendor root: {name}")
            roots[name] = package["name"]
            if not (vendor / name).is_dir() or (vendor / name).is_symlink():
                raise ValueError(f"Missing vendor directory: {name}")
    records = {package["name"]: [] for package in packages}
    if len(records) != len(packages):
        raise ValueError("Duplicate package name")
    for path in sorted(vendor.rglob("*"), key=lambda path: path.relative_to(vendor).as_posix()):
        relative = path.relative_to(vendor)
        if path.is_symlink():
            raise ValueError(f"Linked vendor entry: {relative.as_posix()}")
        if "__pycache__" in relative.parts or path.suffix == ".pyc":
            continue
        if relative.parts[0] not in roots:
            raise ValueError(f"Unlisted vendor entry: {relative.as_posix()}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"Unsupported vendor entry: {relative.as_posix()}")
        content = path.read_bytes()
        records[roots[relative.parts[0]]].append((relative.as_posix(), content))
    result = {}
    for package in packages:
        for license_path in package["licenses"]:
            if not any(name == license_path for name, _ in records[package["name"]]):
                raise ValueError(f"Missing license for {package['name']}: {license_path}")
        digest = hashlib.sha256()
        size = 0
        for name, content in records[package["name"]]:
            encoded = name.encode("utf-8")
            digest.update(len(encoded).to_bytes(8, "big"))
            digest.update(encoded)
            digest.update(len(content).to_bytes(8, "big"))
            digest.update(hashlib.sha256(content).digest())
            size += len(content)
        result[package["name"]] = {
            "files": len(records[package["name"]]),
            "bytes": size,
            "sha256": digest.hexdigest(),
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vendor", type=Path, default=DEFAULT_VENDOR)
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK)
    parser.add_argument("--refresh", action="store_true", help="explicitly accept current trees after reviewing an intentional dependency update")
    args = parser.parse_args()
    try:
        lock = json.loads(args.lock.read_text(encoding="utf-8"))
        if lock["schema_version"] != 1 or lock["tree_format"] != "path-length+utf8-path+content-length+sha256-bytes;u64be;sorted-paths":
            raise ValueError("Unsupported vendor lock format")
        actual = inventory(args.vendor, lock["packages"])
        for package in lock["packages"]:
            if args.refresh:
                package["tree"] = actual[package["name"]]
            elif package["tree"] != actual[package["name"]]:
                raise ValueError(f"Vendor integrity mismatch: {package['name']} (missing, added or changed files)")
        if args.refresh:
            args.lock.write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"vendor_integrity": "refreshed" if args.refresh else "verified", "packages": actual}, sort_keys=True))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Vendor verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
