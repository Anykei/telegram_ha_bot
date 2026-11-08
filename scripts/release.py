#!/usr/bin/env python3
"""Deterministic release planning and narrowly scoped version updates (no publishing)."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z")


def semver(value):
    match = VERSION.fullmatch(value)
    if not match:
        raise ValueError(f"Expected a stable MAJOR.MINOR.PATCH version: {value!r}")
    return tuple(map(int, match.groups()))


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def release_type(messages):
    bump = "patch"
    for message in messages:
        subject = message.split("\n", 1)[0]
        if re.match(r"^[a-zA-Z][\w-]*(?:\([^\r\n)]+\))?!:", subject) or re.search(
            r"^BREAKING[ -]CHANGE:\s*\S", message, re.MULTILINE
        ):
            return "major"
        if re.match(r"^feat(?:\([^\r\n)]+\))?:", subject):
            bump = "minor"
    return bump


def plan(repo):
    if git(repo, "rev-parse", "--is-shallow-repository") == "true":
        raise ValueError("Release planning requires full history and all tags")
    head = git(repo, "rev-parse", "HEAD")
    tags = []
    for tag in git(repo, "tag", "--list").splitlines():
        raw = tag.removeprefix("v")
        if VERSION.fullmatch(raw):
            tags.append((semver(raw), tag, git(repo, "rev-list", "-1", tag)))
    previous = max(tags, default=None)
    if previous:
        version, tag, commit = previous
        if subprocess.run(["git", "-C", str(repo), "merge-base", "--is-ancestor", commit, head]).returncode:
            raise ValueError("HEAD predates or diverges from the latest release; refusing a rollback")
        # Re-running a tagged commit repairs a partial deployment, never bumps again.
        if head == commit:
            return dict(version=".".join(map(str, version)), tag=tag, sha=head, bump="none", reused=True)
        history = f"{commit}..HEAD"
    else:
        version, history = (0, 0, 0), "HEAD"
    messages = git(repo, "log", "--format=%B%x00", history).split("\0")
    bump = release_type([m.strip() for m in messages if m.strip()])
    major, minor, patch = version
    updated = {"major": (major + 1, 0, 0), "minor": (major, minor + 1, 0), "patch": (major, minor, patch + 1)}[bump]
    value = ".".join(map(str, updated))
    return dict(version=value, tag=value, sha=head, bump=bump, reused=False)


def stamp(repo, version):
    """Stamp only the root package in the ephemeral release checkout, not dependencies."""
    semver(version)
    manifest_path, lock_path = Path(repo) / "Cargo.toml", Path(repo) / "Cargo.lock"
    manifest, lock = manifest_path.read_text(), lock_path.read_text()
    package = tomllib.loads(manifest)["package"]
    name = package["name"]
    matches = [p for p in tomllib.loads(lock)["package"] if p["name"] == name and "source" not in p]
    if len(matches) != 1 or matches[0]["version"] != package["version"]:
        raise ValueError("Root package and lockfile versions do not match")
    manifest, count = re.subn(r'(?ms)(^\[package\]\s*\n(?:(?!^\[).)*?^version\s*=\s*)"[^"]+"',
                             lambda m: m[1] + json.dumps(version), manifest, count=1)
    pattern = r'(?ms)(^\[\[package\]\]\s*\nname = "' + re.escape(name) + r'"\nversion = )"[^"]+"'
    lock, lock_count = re.subn(pattern, lambda m: m[1] + json.dumps(version), lock)
    if count != 1 or lock_count != 1:
        raise ValueError("Cannot identify exactly one root package version")
    # Validate both outputs before writing either file.
    assert tomllib.loads(manifest)["package"]["version"] == version
    assert next(p for p in tomllib.loads(lock)["package"] if p["name"] == name)["version"] == version
    manifest_path.write_text(manifest)
    lock_path.write_text(lock)


def update_store(path, version, image):
    """Update a single top-level YAML version without reformatting the add-on config."""
    semver(version)
    path = Path(path)
    text = path.read_text()
    def scalar(key):
        values = re.findall(r"^" + key + r":\s*([^\n#]+?)(?:\s+#.*)?$", text, re.MULTILINE)
        if len(values) != 1:
            raise ValueError(f"Expected exactly one top-level {key} in add-on config")
        return values[0].strip().strip("\"'")
    old = scalar("version")
    if semver(old) > semver(version):
        raise ValueError("Refusing to downgrade the add-on store")
    if scalar("image") != image:
        raise ValueError("Add-on image does not match the image built by this repository")
    updated = re.sub(r"^version:[^\n]*", "version: " + json.dumps(version), text, count=1, flags=re.MULTILINE)
    if updated != text:
        path.write_text(updated)
    return old != version


def verify_image(manifest, version, sha):
    semver(version)
    annotations = manifest.get("annotations", {})
    for key, expected in [("version", version), ("revision", sha)]:
        if annotations.get("org.opencontainers.image." + key) != expected:
            raise ValueError(f"Existing image has a different or missing {key}; refusing to overwrite")
    platforms = {(m.get("platform", {}).get("os"), m.get("platform", {}).get("architecture"))
                 for m in manifest.get("manifests", [])}
    if not {("linux", "amd64"), ("linux", "arm64")} <= platforms:
        raise ValueError("Release image is missing a required architecture")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("plan")
    p.add_argument("--repo", default=".")
    p.add_argument("--github-output", action="store_true")
    p = commands.add_parser("stamp")
    p.add_argument("--repo", default=".")
    p.add_argument("--version", required=True)
    p = commands.add_parser("store")
    p.add_argument("--path", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--image", required=True)
    p = commands.add_parser("verify-image")
    p.add_argument("--path", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--sha", required=True)
    args = parser.parse_args()
    try:
        if args.command == "plan":
            result = plan(args.repo)
            print(json.dumps(result))
            if args.github_output:
                with open(os.environ["GITHUB_OUTPUT"], "a") as out:
                    for key, value in result.items():
                        out.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
        elif args.command == "stamp":
            stamp(args.repo, args.version)
        elif args.command == "store":
            update_store(args.path, args.version, args.image)
        else:
            verify_image(json.loads(Path(args.path).read_text()), args.version, args.sha)
    except (ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Release error: {error}\n")


if __name__ == "__main__":
    main()
