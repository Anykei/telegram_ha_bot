#!/usr/bin/env python3
"""Deterministic release planning, version stamping and add-on metadata (no publishing)."""
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


def changelog_sections(text):
    """Parse the intentionally small HA release-notes format: ## MAJOR.MINOR.PATCH."""
    headings = list(re.finditer(r"^## (.+)$", text, re.MULTILINE))
    if not headings:
        raise ValueError("Changelog must contain version sections")
    sections = {}
    for index, heading in enumerate(headings):
        version = heading[1].strip()
        semver(version)
        if version in sections:
            raise ValueError(f"Duplicate changelog version: {version}")
        end = headings[index + 1].start() if index + 1 < len(headings) else len(text)
        body = text[heading.end():end].strip()
        if not re.search(r"^\s*[-*] \S", body, re.MULTILINE):
            raise ValueError(f"Empty changelog section: {version}")
        sections[version] = body
    return sections


def render_changelog(source, version, existing=None):
    """Publish only notes through this release; retain older store-only history."""
    target = semver(version)
    incoming = changelog_sections(source)
    if version not in incoming:
        raise ValueError(f"Missing changelog section for release {version}")
    sections = changelog_sections(existing) if existing is not None else {}
    if any(semver(old) > target for old in sections):
        raise ValueError("Refusing to downgrade the add-on changelog")
    sections.update({v: body for v, body in incoming.items() if semver(v) <= target})
    return "# История изменений Telegram Bot\n\n" + "\n\n".join(
        f"## {v}\n\n{sections[v]}" for v in sorted(sections, key=semver, reverse=True)
    ) + "\n"


def config_scalar(text, key):
    values = re.findall(r"^" + re.escape(key) + r":\s*([^\n#]+?)(?:\s+#.*)?$", text, re.MULTILINE)
    if len(values) != 1:
        raise ValueError(f"Expected exactly one top-level {key} in add-on config")
    return values[0].strip().strip("\"'")


def update_store(path, version, image, changelog=None, check=False, config_source=None):
    """Validate both files before updating; the workflow commits them together."""
    semver(version)
    path = Path(path)
    text = path.read_text(encoding="utf-8")
    old = config_scalar(text, "version")
    if semver(old) > semver(version):
        raise ValueError("Refusing to downgrade the add-on store")
    if config_scalar(text, "image") != image:
        raise ValueError("Add-on image does not match the image built by this repository")
    source = text
    if config_source is not None:
        source = Path(config_source).read_text(encoding="utf-8")
        semver(config_scalar(source, "version"))
        if config_scalar(source, "image") != image:
            raise ValueError("Source add-on image does not match the release image")
        if config_scalar(source, "slug") != config_scalar(text, "slug"):
            raise ValueError("Refusing to change the installed add-on slug")
    updated = re.sub(r"^version:[^\n]*", "version: " + json.dumps(version), source, count=1, flags=re.MULTILINE)
    notes_path = path.with_name("CHANGELOG.md")
    previous_notes = notes_path.read_text(encoding="utf-8") if notes_path.exists() else None
    notes = previous_notes
    if changelog is not None:
        notes = render_changelog(Path(changelog).read_text(encoding="utf-8"), version, previous_notes)
    changed = updated != text or notes != previous_notes
    if not check:
        if notes != previous_notes:
            notes_path.write_text(notes, encoding="utf-8")
        if updated != text:
            path.write_text(updated, encoding="utf-8")
    return changed


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
    p.add_argument("--changelog", help="Versioned HA release notes from the source repository")
    p.add_argument("--config-source", help="Authoritative add-on config; version is stamped for this release")
    p.add_argument("--check", action="store_true", help="Validate without modifying store files")
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
            update_store(args.path, args.version, args.image, args.changelog, args.check, args.config_source)
        else:
            verify_image(json.loads(Path(args.path).read_text()), args.version, args.sha)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Release error: {error}\n")


if __name__ == "__main__":
    main()
