import hashlib
from contextlib import closing
from pathlib import Path
import sqlite3
import tempfile
import unittest

from installation_audit import inspect


class AuditTest(unittest.TestCase):
    def test_reports_orphans_missing_and_external_paths_without_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "archive"
            root.mkdir()
            known = root / "known.mp4"
            known.write_bytes(b"original")
            orphan = root / "orphan.mp4"
            orphan.write_bytes(b"orphan")
            outside = Path(directory) / "outside.mp4"
            outside.write_bytes(b"outside")
            (root / "link.mp4").symlink_to(outside)
            database = Path(directory) / "test.db"
            with closing(sqlite3.connect(database)) as connection:
                connection.execute("CREATE TABLE camera_recording_segments(id,session_id,file_path,size_bytes,status DEFAULT 'ready',deleted_at)")
                connection.executemany("INSERT INTO camera_recording_segments(id,session_id,file_path,size_bytes) VALUES(?,?,?,?)", [
                    (1, 1, str(known), 8), (2, 1, str(root / "missing.mp4"), 10),
                    (3, 2, str(outside), 7),
                ])
                connection.execute("CREATE TABLE recording_files(path,session_id,segment_id,size_bytes,delete_pending)")
                connection.execute("INSERT INTO recording_files VALUES(?,?,?,?,1)", (known.name, 1, 1, 8))
                connection.commit()
            before = hashlib.sha256(database.read_bytes()).digest()
            report = inspect(database, root)
            self.assertEqual(report["orphan_bytes"], 6)
            self.assertEqual(report["known_bytes_on_disk"], 8)
            self.assertEqual(len(report["missing"]), 1)
            self.assertEqual(len(report["outside_root"]), 1)
            self.assertEqual(len(report["symlinks"]), 1)
            self.assertEqual(report["orphans"][0]["path"], str(orphan))
            self.assertEqual(hashlib.sha256(database.read_bytes()).digest(), before)
            self.assertEqual(known.read_bytes(), b"original")
            self.assertEqual(orphan.read_bytes(), b"orphan")

    def test_backup_api_preserves_wal_and_restore_integrity(self):
        with tempfile.TemporaryDirectory() as directory:
            original = sqlite3.connect(Path(directory) / "live.db")
            original.execute("PRAGMA journal_mode=WAL")
            original.execute("CREATE TABLE example(id PRIMARY KEY, value)")
            original.execute("INSERT INTO example VALUES(1,'before backup')")
            original.commit()
            backup_path = Path(directory) / "backup.db"
            with closing(sqlite3.connect(backup_path)) as backup:
                original.backup(backup)
                self.assertEqual(backup.execute("PRAGMA integrity_check").fetchone()[0], "ok")
            original.execute("UPDATE example SET value='after backup'")
            original.commit()
            original.close()
            with closing(sqlite3.connect(backup_path)) as restored:
                self.assertEqual(restored.execute("SELECT value FROM example").fetchone()[0], "before backup")


if __name__ == "__main__":
    unittest.main()
