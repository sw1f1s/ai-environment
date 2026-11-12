"""Exercise the standalone verifier against an actual copied dependency tree."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class VendorIntegrityTests(unittest.TestCase):
    def test_shipped_dependencies_match_lock(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/vendor.py")], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(json.loads(result.stdout)["packages"]), 3)

    def test_changed_missing_and_unlisted_files_are_rejected(self):
        with tempfile.TemporaryDirectory(prefix="ai-env-vendor-test-") as temporary:
            base = Path(temporary)
            vendor = base / "vendor"
            vendor.mkdir()
            shutil.copytree(ROOT / "skills/ai-env/scripts/vendor/parso", vendor / "parso", ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
            shutil.copytree(ROOT / "skills/ai-env/scripts/vendor/parso-0.8.7.dist-info", vendor / "parso-0.8.7.dist-info")
            lock = json.loads((ROOT / "skills/ai-env/vendor-lock.json").read_text())
            lock["packages"] = [p for p in lock["packages"] if p["name"] == "parso"]
            lock_path = base / "lock.json"
            lock_path.write_text(json.dumps(lock))

            def verify():
                return subprocess.run([sys.executable, str(ROOT / "scripts/vendor.py"), "--vendor", str(vendor), "--lock", str(lock_path)], capture_output=True, text=True)

            self.assertEqual(verify().returncode, 0)
            source = vendor / "parso/__init__.py"
            original = source.read_bytes()
            source.write_bytes(original + b"\n# unexpected modification\n")
            self.assertNotEqual(verify().returncode, 0)
            source.write_bytes(original)
            source.unlink()
            self.assertNotEqual(verify().returncode, 0)
            source.write_bytes(original)
            unknown = vendor / "unexpected.py"
            unknown.write_text("raise RuntimeError('must never be imported')\n")
            self.assertNotEqual(verify().returncode, 0)
            unknown.unlink()
            license_path = vendor / "parso-0.8.7.dist-info/LICENSE.txt"
            license_path.unlink()
            result = verify()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Missing license", result.stderr)


if __name__ == "__main__":
    unittest.main()
