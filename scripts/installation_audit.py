#!/usr/bin/env python3
"""Read-only archive reconciliation and legacy ACL report. Never removes files."""
import argparse
import json
import os
from pathlib import Path
import sqlite3


def inspect(database, storage_root):
    root = storage_root.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("storage root must be a directory")
    connection = sqlite3.connect(database.resolve(strict=True).as_uri() + "?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute("PRAGMA query_only = ON")
        connection.execute("BEGIN")
        tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master")}
        known = {}
        required = set()
        def owned_path(value):
            path = Path(value)
            return (path if path.is_absolute() else root / path).resolve()
        if "recording_files" in tables:
            for row in connection.execute("SELECT * FROM recording_files"):
                path = owned_path(row["path"])
                known[path] = dict(row)
                if row["size_bytes"] > 0 and not row["delete_pending"]:
                    required.add(path)
        for row in connection.execute(
            "SELECT id AS segment_id, session_id, file_path AS path, size_bytes, status, deleted_at "
            "FROM camera_recording_segments WHERE file_path IS NOT NULL"
        ):
            path = owned_path(row["path"])
            known.setdefault(path, dict(row))
            if row["status"] == "ready" and row["deleted_at"] is None:
                required.add(path)
        report = {"database": str(database), "storage_root": str(root), "orphans": [],
                  "missing": [], "outside_root": [], "symlinks": [], "errors": [],
                  "orphan_bytes": 0, "known_bytes_on_disk": 0,
                  "reserved_or_pending_paths_absent": 0,
                  "legacy_acl_review": [], "acl_review_required": True}
        if "legacy_device_acl_report" in tables:
            report["legacy_acl_review"] = [dict(row) for row in connection.execute(
                "SELECT * FROM legacy_device_acl_report")]
        for path, owner in known.items():
            if not path.is_relative_to(root):
                report["outside_root"].append(owner)
            elif not path.exists():
                if path in required:
                    report["missing"].append(owner)
                else:
                    report["reserved_or_pending_paths_absent"] += 1
        def onerror(error):
            report["errors"].append(str(error))
        for directory, dirs, files in os.walk(root, followlinks=False, onerror=onerror):
            for name in dirs + files:
                path = Path(directory) / name
                if path.is_symlink():
                    report["symlinks"].append(str(path))
            for name in files:
                path = Path(directory) / name
                if path.is_symlink():
                    continue
                try:
                    size = path.stat().st_size
                except OSError as error:
                    onerror(error)
                    continue
                if path.resolve() in known:
                    report["known_bytes_on_disk"] += size
                else:
                    report["orphans"].append({"path": str(path), "size_bytes": size})
                    report["orphan_bytes"] += size
        return report
    finally:
        connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", type=Path, required=True)
    parser.add_argument("--storage-root", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(inspect(args.database, args.storage_root), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
