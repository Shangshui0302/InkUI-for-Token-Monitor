#!/usr/bin/env python3
"""Focused checks for CI secret handling; no signing credentials or Android SDK needed."""

import base64
import importlib.util
from pathlib import Path
import stat
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("ci_signing", Path(__file__).with_name("prepare-ci-signing.py"))
signing = importlib.util.module_from_spec(spec)
spec.loader.exec_module(signing)


class SigningPreparationTest(unittest.TestCase):
    def environment(self):
        return {"INKUI_KEYSTORE_BASE64": base64.b64encode(b"synthetic-keystore").decode(),
                "INKUI_STORE_PASSWORD": " leading space\\password\n中文",
                "INKUI_KEY_ALIAS": "alias:=#!", "INKUI_KEY_PASSWORD": "🔑\tpass "}

    def test_properties_escapes_java_inputstream_values(self):
        self.assertEqual(signing.property_value(" leading \\ \t\n\r\f=:#!中文🔑"),
                         r"\ leading\ \\\ \t\n\r\f\=\:\#\!\u4e2d\u6587\ud83d\udd11")

    def test_private_files_preserve_binary_and_escaped_values(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "signing"
            environment = self.environment()
            signing.prepare_signing(directory, environment)
            self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)
            self.assertEqual((directory / "release.keystore").read_bytes(), b"synthetic-keystore")
            properties = (directory / "release.properties").read_text(encoding="ascii")
            for key, secret in (("storePassword", "INKUI_STORE_PASSWORD"),
                                ("keyAlias", "INKUI_KEY_ALIAS"), ("keyPassword", "INKUI_KEY_PASSWORD")):
                self.assertIn(key + "=" + signing.property_value(environment[secret]) + "\n", properties)
            for path in directory.iterdir():
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            with self.assertRaises(FileExistsError):
                signing.prepare_signing(directory, environment)

    def test_reject_missing_or_invalid_inputs_before_creating_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            for index, change in enumerate(({"INKUI_STORE_PASSWORD": ""}, {"INKUI_KEYSTORE_BASE64": "%%"},
                                            {"INKUI_KEYSTORE_BASE64": " "})):
                directory = Path(temporary) / str(index)
                with self.assertRaises(ValueError):
                    signing.prepare_signing(directory, self.environment() | change)
                self.assertFalse(directory.exists())


if __name__ == "__main__":
    unittest.main()
