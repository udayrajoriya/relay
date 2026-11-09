"""Package a built native binary; no Node.js, browser runtime, or FUSE required."""
import argparse
from pathlib import Path
import platform
import shutil
import tarfile
import tempfile
import tomllib
import zipfile

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument("--binary", type=Path)
parser.add_argument("--output", type=Path, default=root / "dist")
args = parser.parse_args()
version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
system = platform.system()
if system not in ("Linux", "Windows"):
    parser.error("Packaging is supported on Linux and Windows")
arch = {"x86_64": "x64", "AMD64": "x64", "aarch64": "arm64", "ARM64": "arm64"}.get(platform.machine(), platform.machine())
binary_name = "relay.exe" if system == "Windows" else "relay"
binary = (args.binary or root / "target" / "release" / binary_name).resolve()
if not binary.is_file():
    parser.error(f"{binary} does not exist; run cargo build --release first")
args.output.mkdir(parents=True, exist_ok=True)
name = f"Relay-{version}-{system}-{arch}"
with tempfile.TemporaryDirectory(prefix="relay-package-") as temporary:
    stage = Path(temporary) / name
    stage.mkdir()
    shutil.copy2(binary, stage / binary_name)
    for filename in ("README.md", "LICENSE"):
        shutil.copy2(root / filename, stage / filename)
    if system == "Linux":
        shutil.copy2(root / "packaging/local.relay.downloads.desktop", stage)
        shutil.copy2(root / "src/assets/icon.png", stage / "relay.png")
        artifact = args.output / f"{name}.tar.gz"
        with tarfile.open(artifact, "w:gz") as archive:
            archive.add(stage, arcname=name)
    else:
        artifact = args.output / f"{name}.zip"
        with zipfile.ZipFile(artifact, "w", zipfile.ZIP_DEFLATED) as archive:
            for file in sorted(stage.rglob("*")):
                if file.is_file():
                    archive.write(file, file.relative_to(stage.parent))
print(artifact.resolve())
