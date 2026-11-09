"""Exercise publication/verification/deletion ordering without contacting GitHub."""
from pathlib import Path
import tempfile
import unittest

from scripts.release import Publisher

SHA = "a" * 40


class FakeGitHub:
    def __init__(self):
        self.current_release = None
        self.reference = None
        self.assets = {}
        self.artifacts = [{"id": 1, "name": "Relay-Linux"}, {"id": 2, "name": "Relay-Windows"}, {"id": 3, "name": "unrelated"}]
        self.events = []
        self.corrupt_download = False
        self.fail_upload = False

    def api(self, path, method="GET", missing_ok=False):
        if method == "DELETE":
            self.events.append(("delete", path))
            return None
        if path.startswith("releases/tags/"):
            if self.current_release is not None:
                return {**self.current_release, "assets": [{"name": name, "size": len(data)} for name, data in self.assets.items()]}
            return None
        if path.startswith("git/ref/"):
            return self.reference
        if path.startswith("actions/runs/"):
            return {"artifacts": self.artifacts}
        raise AssertionError(path)

    def gh(self, *args):
        self.events.append((args[0], args))
        if args[0] == "create":
            self.current_release = {"draft": True, "target_commitish": args[args.index("--target") + 1]}
        elif args[0] == "upload":
            if self.fail_upload:
                raise RuntimeError("simulated upload failure")
            for name in args[2:-1]:
                path = Path(name)
                self.assets[path.name] = path.read_bytes()
        elif args[0] == "download":
            directory = Path(args[args.index("--dir") + 1])
            for name, data in self.assets.items():
                (directory / name).write_bytes(b"corrupt" if self.corrupt_download else data)
        elif args[0] == "edit":
            self.current_release["draft"] = False
        else:
            raise AssertionError(args)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.github = FakeGitHub()
        self.publisher = Publisher(self.github, "0.2.0", SHA)
        for name in self.publisher.names:
            (self.directory / name).write_bytes(name.encode())

    def test_only_master_pushes_or_manual_runs_can_publish(self):
        for event, ref in [("pull_request", "refs/heads/master"), ("push", "refs/heads/feature"), ("push", "refs/tags/v0.2.0")]:
            self.assertFalse(self.publisher.plan(event, ref))
        self.assertTrue(self.publisher.plan("push", "refs/heads/master"))
        self.assertTrue(self.publisher.plan("workflow_dispatch", "refs/heads/master"))

    def test_existing_published_version_is_skipped_and_never_overwritten(self):
        self.github.current_release = {"draft": False, "target_commitish": "old"}
        self.assertFalse(self.publisher.plan("push", "refs/heads/master"))
        with self.assertRaisesRegex(RuntimeError, "Refusing to overwrite"):
            self.publisher.publish(self.directory)
        self.assertEqual(self.github.events, [])

    def test_tag_or_draft_for_another_commit_is_rejected(self):
        self.github.reference = {"object": {"type": "commit", "sha": "b" * 40}}
        with self.assertRaisesRegex(RuntimeError, "another commit"):
            self.publisher.plan("push", "refs/heads/master")
        self.github.reference = None
        self.github.current_release = {"draft": True, "target_commitish": "old"}
        with self.assertRaisesRegex(RuntimeError, "another commit"):
            self.publisher.publish(self.directory)

    def test_publish_verifies_before_publication_then_cleanup_is_limited_to_this_run(self):
        self.publisher.publish(self.directory)
        self.assertEqual([event[0] for event in self.github.events], ["create", "upload", "download", "edit"])
        self.assertEqual(set(self.github.assets), {*self.publisher.names, "SHA256SUMS"})
        self.publisher.cleanup("123")
        deleted = [path for event, path in self.github.events if event == "delete"]
        self.assertEqual(deleted, ["actions/artifacts/1", "actions/artifacts/2"])

    def test_same_commit_draft_can_be_retried(self):
        self.github.current_release = {"draft": True, "target_commitish": SHA}
        self.assertTrue(self.publisher.plan("push", "refs/heads/master"))
        self.publisher.publish(self.directory)
        self.assertNotIn("create", [event[0] for event in self.github.events])

    def test_missing_package_cannot_create_a_release(self):
        (self.directory / self.publisher.names[1]).unlink()
        with self.assertRaisesRegex(RuntimeError, "Both Linux and Windows"):
            self.publisher.publish(self.directory)
        self.assertEqual(self.github.events, [])

    def test_failed_upload_keeps_draft_and_temporary_artifacts(self):
        self.github.fail_upload = True
        with self.assertRaisesRegex(RuntimeError, "upload failure"):
            self.publisher.publish(self.directory)
        self.assertTrue(self.github.current_release["draft"])
        with self.assertRaisesRegex(RuntimeError, "keeping temporary artifacts"):
            self.publisher.cleanup("123")
        self.assertFalse(any(event[0] == "delete" for event in self.github.events))

    def test_bad_download_cannot_publish_or_delete_artifacts(self):
        self.github.corrupt_download = True
        with self.assertRaisesRegex(RuntimeError, "failed verification"):
            self.publisher.publish(self.directory)
        self.assertTrue(self.github.current_release["draft"])
        self.assertNotIn("edit", [event[0] for event in self.github.events])
        with self.assertRaises(RuntimeError):
            self.publisher.cleanup("123")
        self.assertFalse(any(event[0] == "delete" for event in self.github.events))

    def test_cleanup_requires_complete_published_release(self):
        with self.assertRaises(RuntimeError):
            self.publisher.cleanup("123")
        self.github.current_release = {"draft": False, "target_commitish": SHA}
        self.github.assets = {self.publisher.names[0]: b"linux"}
        with self.assertRaises(RuntimeError):
            self.publisher.cleanup("123")
        self.assertFalse(any(event[0] == "delete" for event in self.github.events))

    def test_prerelease_is_not_marked_latest(self):
        publisher = Publisher(self.github, "0.3.0-rc.1", SHA)
        for name in publisher.names:
            (self.directory / name).write_bytes(b"prerelease")
        publisher.publish(self.directory)
        self.assertIn("--prerelease", self.github.events[0][1])
        self.assertIn("--latest=false", self.github.events[-1][1])


if __name__ == "__main__":
    unittest.main()
