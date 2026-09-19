import json
import os
import sqlite3
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


import importlib.util


MODULE_PATH = Path(__file__).with_name("codex-status-server.py")
SPEC = importlib.util.spec_from_file_location("codex_status_server", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MODULE)


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.home = Path(self.temp.name) / ".codex"
        self.home.mkdir()
        self.workspace = Path(self.temp.name) / "workspace"
        self.workspace.mkdir()
        self.rollout = self.home / "sessions" / "rollout.jsonl"
        self.rollout.parent.mkdir()
        self.rollout.write_text("\n".join([
            json.dumps({"timestamp": "2026-09-19T08:00:00Z", "type": "event_msg", "payload": {"type": "user_message", "item": {"type": "UserMessage", "content": [{"text": "hello"}]}}}),
            json.dumps({"timestamp": "2026-09-19T08:01:00Z", "type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "AgentMessage", "content": [{"text": "done"}]}}}),
            json.dumps({"timestamp": "2026-09-19T08:01:01Z", "type": "event_msg", "payload": {"type": "token_count", "info": {}, "rate_limits": {"limit_id": "codex", "primary": {"used_percent": 25, "resets_at": 1790000000}, "secondary": {"used_percent": 50, "resets_at": 1791000000}}}}),
        ]) + "\n")
        connection = sqlite3.connect(self.home / "state_5.sqlite")
        connection.executescript("""
            CREATE TABLE threads (
              id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL, source TEXT NOT NULL, model_provider TEXT NOT NULL,
              cwd TEXT NOT NULL, title TEXT NOT NULL, sandbox_policy TEXT NOT NULL,
              approval_mode TEXT NOT NULL, tokens_used INTEGER NOT NULL DEFAULT 0,
              archived INTEGER NOT NULL DEFAULT 0, updated_at_ms INTEGER,
              thread_source TEXT, preview TEXT, model TEXT, reasoning_effort TEXT,
              name TEXT
            );
        """)
        connection.execute("INSERT INTO threads VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", (
            "thread-1", str(self.rollout), 1, 1, "user", "openai", str(self.workspace),
            "Test task", "{}", "never", 12, 0, 1789804800000, "user", "hello", "codex", "low", "Test task",
        ))
        connection.commit()
        connection.close()

    def tearDown(self):
        self.temp.cleanup()

    def test_build_status_reads_local_files_without_network(self):
        with patch.object(MODULE, "codex_home", return_value=self.home), patch.object(MODULE, "workspace", return_value=self.workspace):
            status = MODULE.build_status()
        self.assertEqual(status["task"]["thread_id"], "thread-1")
        self.assertEqual(status["quota"]["windows"][0]["remaining_percent"], 75.0)
        self.assertGreaterEqual(len(status["activity"]["items"]), 2)
        self.assertEqual(status["sessions"]["count"], 1)

    def test_domain_failure_does_not_hide_sessions(self):
        self.rollout.unlink()
        with patch.object(MODULE, "codex_home", return_value=self.home), patch.object(MODULE, "workspace", return_value=self.workspace):
            status = MODULE.build_status()
        self.assertEqual(status["sessions"]["count"], 1)
        self.assertEqual(status["quota"]["state"], "unavailable")
        self.assertTrue(status["errors"])


if __name__ == "__main__":
    unittest.main()
