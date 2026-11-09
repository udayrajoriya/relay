"""Publish verified native packages, then remove only this run's temporary artifacts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parent.parent
ARTIFACT_NAMES = {"Relay-Linux", "Relay-Windows"}


class GitHub:
    def __init__(self):
        self.repo = os.environ["GITHUB_REPOSITORY"]
        self.base = os.environ.get("GITHUB_API_URL", "https://api.github.com")

    def api(self, path, method="GET", missing_ok=False):
        request = Request(f"{self.base}/repos/{self.repo}/{path}", method=method, headers={
            "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        })
        try:
            with urlopen(request, timeout=60) as response:
                body = response.read()
                return json.loads(body) if body else None
        except HTTPError as error:
            if missing_ok and error.code == 404:
                return None
            raise RuntimeError(f"GitHub {method} {path} failed: HTTP {error.code}") from error

    def gh(self, *arguments):
        subprocess.run(["gh", "release", *arguments, "--repo", self.repo], check=True)


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


class Publisher:
    def __init__(self, github, version, sha):
        if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version):
            raise ValueError("Cargo.toml must contain a semantic version")
        self.github, self.version, self.sha = github, version, sha
        self.tag = f"v{version}"
        self.prerelease = "-" in version.split("+", 1)[0]
        self.names = [f"Relay-{version}-Linux-x64.tar.gz", f"Relay-{version}-Windows-x64.zip"]

    def release(self):
        return self.github.api(f"releases/tags/{quote(self.tag, safe='')}", missing_ok=True)

    def validate_target(self, release=None):
        reference = self.github.api(f"git/ref/tags/{quote(self.tag, safe='')}", missing_ok=True)
        if reference:
            target = reference["object"]
            for _ in range(8):
                if target["type"] != "tag":
                    break
                target = self.github.api(f"git/tags/{target['sha']}")["object"]
            if target["type"] != "commit" or target["sha"] != self.sha:
                raise RuntimeError(f"{self.tag} points to another commit; bump the package version")
        if release and release["target_commitish"] != self.sha:
            raise RuntimeError(f"The draft for {self.tag} targets another commit; bump the version or resolve the draft")

    def plan(self, event, ref):
        if ref != "refs/heads/master" or event not in {"push", "workflow_dispatch"}:
            return False
        release = self.release()
        if release and not release["draft"]:
            print(f"{self.tag} is already published; checks will run without uploading artifacts")
            return False
        self.validate_target(release)
        return True

    def publish(self, directory):
        files = [directory / name for name in self.names]
        if any(not path.is_file() or path.stat().st_size == 0 for path in files):
            raise RuntimeError("Both Linux and Windows packages must exist and be nonempty")
        checksums = directory / "SHA256SUMS"
        checksums.write_text("".join(f"{digest(path)}  {path.name}\n" for path in files))
        files.append(checksums)
        release = self.release()
        if release and not release["draft"]:
            raise RuntimeError(f"Refusing to overwrite published release {self.tag}")
        self.validate_target(release)
        if release is None:
            options = ["create", self.tag, "--draft", "--target", self.sha,
                       "--title", f"Relay {self.version}", "--generate-notes"]
            if self.prerelease:
                options.append("--prerelease")
            self.github.gh(*options)
        # Clobber is restricted to an unpublished draft belonging to this commit.
        self.github.gh("upload", self.tag, *(str(path) for path in files), "--clobber")
        with tempfile.TemporaryDirectory(prefix="relay-release-verify-") as temporary:
            options = ["download", self.tag, "--dir", temporary]
            for path in files:
                options += ["--pattern", path.name]
            self.github.gh(*options)
            for path in files:
                downloaded = Path(temporary) / path.name
                if not downloaded.is_file() or digest(downloaded) != digest(path):
                    raise RuntimeError(f"Uploaded {path.name} failed verification; leaving the release as a draft")
        self.github.gh("edit", self.tag, "--draft=false", "--latest=false" if self.prerelease else "--latest")
        self.require_published()
        print(f"Published {self.tag} with verified Linux, Windows, and checksum assets")

    def require_published(self):
        release = self.release()
        expected = {*self.names, "SHA256SUMS"}
        uploaded = {asset["name"] for asset in (release or {}).get("assets", []) if asset.get("size", 0) > 0}
        if not release or release["draft"] or not expected <= uploaded:
            raise RuntimeError("Release is not published with all assets; keeping temporary artifacts")

    def cleanup(self, run_id):
        if not run_id.isdecimal():
            raise ValueError("GITHUB_RUN_ID must be numeric")
        self.require_published()
        page = 1
        artifacts = []
        while True:
            batch = self.github.api(f"actions/runs/{run_id}/artifacts?per_page=100&page={page}")["artifacts"]
            artifacts.extend(batch)
            if len(batch) < 100:
                break
            page += 1
        for artifact in artifacts:
            if artifact["name"] in ARTIFACT_NAMES:
                self.github.api(f"actions/artifacts/{int(artifact['id'])}", method="DELETE")
                print(f"Deleted temporary artifact {artifact['name']} ({artifact['id']})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["plan", "publish", "cleanup"])
    parser.add_argument("--directory", type=Path, default=Path("release-assets"))
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    publisher = Publisher(GitHub(), version, os.environ["GITHUB_SHA"])
    if args.command == "plan":
        publish = publisher.plan(os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REF"])
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"tag={publisher.tag}\npublish={str(publish).lower()}\n")
    elif args.command == "publish":
        publisher.publish(args.directory)
    else:
        publisher.cleanup(os.environ["GITHUB_RUN_ID"])


if __name__ == "__main__":
    main()
