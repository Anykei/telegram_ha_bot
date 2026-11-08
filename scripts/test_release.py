import copy
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

from release import plan, release_type, semver, stamp, update_store, verify_image


class ReleasePlanTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "ci@example.invalid")
        self.git("config", "user.name", "CI test")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", "/dev/null")
        self.commit("initial")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], text=True).strip()

    def commit(self, message):
        self.git("commit", "--allow-empty", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def test_initial_default_patch(self):
        self.assertEqual(plan(self.repo)["version"], "0.0.1")
        self.assertEqual(self.git("tag"), "")  # planning never creates tags

    def test_numeric_order_and_legacy_prefix(self):
        self.git("tag", "0.2.9")
        self.commit("fix: previous")
        self.git("tag", "v0.2.10")
        self.git("tag", "20.0.0-rc.1")  # not a stable release
        self.commit("fix: current")
        self.assertEqual(plan(self.repo)["version"], "0.2.11")

    def test_annotated_tags_and_minor_bump(self):
        self.git("tag", "-a", "0.2.3", "-m", "Release")
        self.commit("feat(ui): help")
        self.commit("fix: navigation")
        self.assertEqual(plan(self.repo)["version"], "0.3.0")

    def test_breaking_change_resets_minor_and_patch(self):
        self.git("tag", "0.2.3")
        self.commit("feat!: changed protocol")
        self.assertEqual(plan(self.repo)["version"], "1.0.0")

    def test_retry_after_tag_reuses_exact_version(self):
        self.git("tag", "0.2.3")
        self.commit("fix: camera")
        before = plan(self.repo)
        self.git("tag", before["tag"])
        after = plan(self.repo)
        self.assertEqual(after["version"], before["version"])
        self.assertEqual(after["sha"], before["sha"])
        self.assertTrue(after["reused"])
        self.assertEqual(after["bump"], "none")
        self.commit("fix: next")
        self.assertEqual(plan(self.repo)["version"], "0.2.5")

    def test_retry_old_commit_cannot_downgrade(self):
        old = self.git("rev-parse", "HEAD")
        self.git("tag", "0.2.3")
        self.commit("fix: next")
        self.git("tag", "0.2.4")
        self.git("checkout", "-q", "--detach", old)
        with self.assertRaisesRegex(ValueError, "refusing a rollback"):
            plan(self.repo)

    def test_failed_run_before_tag_is_deterministic(self):
        self.git("tag", "0.2.3")
        self.commit("chore: build")
        self.assertEqual(plan(self.repo), plan(self.repo))
        self.assertEqual(plan(self.repo)["version"], "0.2.4")

    def test_shallow_clone_is_rejected(self):
        destination = self.repo / "clone"
        subprocess.run(["git", "clone", "-q", "--depth=1", self.repo.as_uri(), str(destination)], check=True)
        with self.assertRaisesRegex(ValueError, "full history"):
            plan(destination)


class VersionRulesTest(unittest.TestCase):
    def test_conventional_commit_rules(self):
        cases = [
            (["docs: typo"], "patch"), (["fix(api): bug"], "patch"),
            (["feat(ui): screen", "fix: bug"], "minor"),
            (["refactor!: API"], "major"), (["feat(api)!: API"], "major"),
            (["feat: API\n\nBREAKING CHANGE: changed shape"], "major"),
            (["fix: API\n\nBREAKING-CHANGE: removed field"], "major"),
            (["docs: explain feat! without changing an API"], "patch"),
        ]
        for messages, expected in cases:
            with self.subTest(messages=messages):
                self.assertEqual(release_type(messages), expected)

    def test_version_input_rejects_prereleases_and_shell_metacharacters(self):
        for value in ["v1.2.3", "1.2.3-rc.1", "01.2.3", "1.2", "1.2.3\n", "$(id)", "1.2.3;exit"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                semver(value)


class ReleaseFilesTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)

    def test_stamp_updates_root_manifest_and_lock_without_touching_dependencies(self):
        root = Path(__file__).resolve().parents[1]
        for filename in ["Cargo.toml", "Cargo.lock"]:
            (self.repo / filename).write_bytes((root / filename).read_bytes())
        before = tomllib.loads((self.repo / "Cargo.lock").read_text())
        stamp(self.repo, "0.3.0")
        after = tomllib.loads((self.repo / "Cargo.lock").read_text())
        self.assertEqual(tomllib.loads((self.repo / "Cargo.toml").read_text())["package"]["version"], "0.3.0")
        for old, new in zip(before["package"], after["package"], strict=True):
            if old["name"] == "telegram_ha_bot":
                self.assertEqual(new["version"], "0.3.0")
            else:
                self.assertEqual(old, new)
        stamp(self.repo, "0.3.0")  # repeat is safe
        self.assertEqual(tomllib.loads((self.repo / "Cargo.lock").read_text()), after)

    def store(self, contents='version: "0.2.3"\nimage: "ghcr.io/anykei/telegram_ha_bot"\noptions:\n  token: ""\n'):
        path = self.repo / "config.yaml"
        path.write_text(contents)
        return path

    def test_store_preserves_other_properties_and_is_retry_safe(self):
        path = self.store()
        self.assertTrue(update_store(path, "0.3.0", "ghcr.io/anykei/telegram_ha_bot"))
        updated = path.read_text()
        self.assertIn('version: "0.3.0"', updated)
        self.assertTrue(updated.endswith('options:\n  token: ""\n'))
        self.assertFalse(update_store(path, "0.3.0", "ghcr.io/anykei/telegram_ha_bot"))
        self.assertEqual(path.read_text(), updated)

    def test_bad_store_config_is_not_changed(self):
        cases = [
            ('version: "0.2.3"\nversion: "0.2.4"\nimage: expected\n', "0.3.0", "expected"),
            ('name: test\nimage: expected\n', "0.3.0", "expected"),
            ('version: "0.4.0"\nimage: expected\n', "0.3.0", "expected"),
            ('version: "0.2.3"\nimage: wrong\n', "0.3.0", "expected"),
        ]
        for text, version, image in cases:
            path = self.store(text)
            with self.subTest(text=text), self.assertRaises(ValueError):
                update_store(path, version, image)
            self.assertEqual(path.read_text(), text)

    def test_registry_image_requires_matching_source_version_and_both_architectures(self):
        manifest = {
            "annotations": {"org.opencontainers.image.version": "0.3.0", "org.opencontainers.image.revision": "a" * 40},
            "manifests": [{"platform": {"os": "linux", "architecture": arch}} for arch in ["amd64", "arm64"]],
        }
        verify_image(manifest, "0.3.0", "a" * 40)
        bad = copy.deepcopy(manifest)
        bad["manifests"].pop()
        for data, version, sha in [(manifest, "0.4.0", "a" * 40), (manifest, "0.3.0", "b" * 40), (bad, "0.3.0", "a" * 40)]:
            with self.subTest(version=version, sha=sha), self.assertRaises(ValueError):
                verify_image(data, version, sha)


if __name__ == "__main__":
    unittest.main()
