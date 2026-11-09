"""Launch the real native app with an isolated profile and verify rendered views."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("executable", nargs="?", default="target/debug/relay.exe" if os.name == "nt" else "target/debug/relay")
parser.add_argument("--artifacts", type=Path)
args = parser.parse_args()
executable = Path(args.executable).resolve()
with tempfile.TemporaryDirectory(prefix="relay-native-smoke-") as temporary:
    root = Path(temporary)
    artifacts = (args.artifacts or root).resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    marker = artifacts / "native-smoke.txt"
    subprocess.run([str(executable), "--profile", str(root / "profile"), "--downloads", str(root / "downloads"), "--smoke-test", str(marker)], check=True, timeout=45)
    assert "native UI rendered" in marker.read_text()
    for view in ("main", "add", "settings"):
        screenshot = marker.with_suffix(f".{view}.png")
        assert screenshot.stat().st_size > 1000, f"No native {view} screenshot"
    profile = root / "profile"
    assert len((profile / "browser-token").read_text()) == 64
    state = json.loads((profile / "state.json").read_text())
    assert state["jobs"] == []
    assert Path(state["settings"]["downloadDir"]) == root / "downloads"
    for browser in ("chromium", "firefox"):
        assert list((profile / "extensions").glob(f"*/{browser}/manifest.json"))
print("Native smoke passed: window, downloads view, add form, settings, profile persistence, embedded companions, clean shutdown.")
