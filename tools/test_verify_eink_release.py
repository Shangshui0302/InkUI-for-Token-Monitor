#!/usr/bin/env python3
"""Dependency-free positive and negative tests for APK delivery validation."""
import copy
import importlib.util
import io
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("release_gate", Path(__file__).with_name("verify-eink-release.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def manifest_fixture(utf8):
    strings = [gate.ANDROID, "manifest", "versionName", "1.0.1-dev", "versionCode", "package",
               "dev.tokenmonitor.eink.debug", "application", "debuggable", "uses-sdk", "minSdkVersion",
               "targetSdkVersion", "长" * 160]
    encoded = bytearray()
    offsets = []

    def length(n):
        if utf8:
            return bytes([n]) if n < 128 else bytes([0x80 | (n >> 8), n & 255])
        return struct.pack("<H", n)

    for string in strings:
        offsets.append(len(encoded))
        content = string.encode("utf-8" if utf8 else "utf-16-le")
        encoded += length(len(string))
        if utf8:
            encoded += length(len(content))
        encoded += content + (b"\0" if utf8 else b"\0\0")
    encoded += b"\0" * (-len(encoded) % 4)
    start = 28 + 4 * len(strings)
    pool = struct.pack("<HHIIIIII", 1, 28, start + len(encoded), len(strings), 0, 0x100 if utf8 else 0, start, 0)
    pool += struct.pack("<" + "I" * len(offsets), *offsets) + encoded

    def attribute(name, value, kind=3, namespace=True):
        return struct.pack("<IIIHBBI", 0 if namespace else gate.NONE, strings.index(name), gate.NONE,
                           8, 0, kind, strings.index(value) if kind == 3 else value)

    def element(name, attributes):
        return (struct.pack("<HHIII", 0x0102, 16, 36 + 20 * len(attributes), 1, gate.NONE)
                + struct.pack("<IIHHHHHH", gate.NONE, strings.index(name), 20, 20, len(attributes), 0, 0, 0)
                + b"".join(attributes))

    body = pool
    body += element("manifest", [attribute("versionName", "1.0.1-dev"), attribute("versionCode", 13, 0x10),
                                  attribute("package", "dev.tokenmonitor.eink.debug", namespace=False)])
    body += element("application", [attribute("debuggable", 1, 0x12)])
    body += element("uses-sdk", [attribute("minSdkVersion", 29, 0x10), attribute("targetSdkVersion", 36, 0x10)])
    return struct.pack("<HHI", 3, 8, len(body) + 8) + body


class ReleaseGateTest(unittest.TestCase):
    def setUp(self):
        self.config = {"versionName": "1.0.1-dev", "versionCode": 13, "applicationId": "dev.tokenmonitor.eink", "minSdk": 29, "targetSdk": 36}
        self.manifest = gate.binary_manifest(manifest_fixture(True))

    def verify(self, **kwargs):
        return gate.verify_metadata(self.manifest, self.config, "inkui-1.0.1-dev-code13-debug.apk", "debug", **kwargs)

    def test_utf8_and_utf16_binary_manifest(self):
        self.assertEqual(self.manifest, gate.binary_manifest(manifest_fixture(False)))
        self.assertEqual(self.verify()["versionCode"], 13)

    def test_reject_plain_or_truncated_xml(self):
        for data in (b"<manifest/>", manifest_fixture(True)[:-1], b"", struct.pack("<HHI", 3, 8, 8)):
            with self.subTest(data=data[:8]), self.assertRaises(gate.VerificationError):
                gate.binary_manifest(data)

    def test_reject_zero_length_chunk(self):
        data = bytearray(manifest_fixture(True))
        struct.pack_into("<I", data, 12, 0)
        with self.assertRaises(gate.VerificationError):
            gate.binary_manifest(data)

    def test_reject_source_version_mismatch(self):
        for field, value in (("versionName", "1.0.0"), ("versionCode", 12)):
            with self.subTest(field=field), self.assertRaises(gate.VerificationError):
                config = dict(self.config, **{field: value})
                gate.verify_metadata(self.manifest, config, "inkui-1.0.1-dev-code13-debug.apk", "debug")

    def test_reject_wrong_filename(self):
        with self.assertRaisesRegex(gate.VerificationError, "filename"):
            gate.verify_metadata(self.manifest, self.config, "inkui-release.apk", "debug")

    def test_reject_code_reuse_or_regression(self):
        for previous in (13, 14, -1):
            with self.subTest(previous=previous), self.assertRaises(gate.VerificationError):
                self.verify(previous_code=previous)
        self.verify(previous_code=12)

    def test_reject_wrong_package_or_debug_flag(self):
        for element, field, value in (("manifest", "package", "other.app"), ("application", "debuggable", False),
                                      ("uses-sdk", "targetSdkVersion", 35)):
            with self.subTest(field=field), self.assertRaises(gate.VerificationError):
                manifest = copy.deepcopy(self.manifest)
                manifest[element][field] = value
                gate.verify_metadata(manifest, self.config, "inkui-1.0.1-dev-code13-debug.apk", "debug")

    def test_reject_dev_as_formal_release(self):
        with self.assertRaises(gate.VerificationError):
            self.verify(tag="v1.0.1", previous_code=12)

    def formal(self, tag="v1.0.1", previous=12):
        manifest = copy.deepcopy(self.manifest)
        manifest["manifest"].update(versionName="1.0.1", package="dev.tokenmonitor.eink")
        manifest["application"]["debuggable"] = False
        config = dict(self.config, versionName="1.0.1")
        return gate.verify_metadata(manifest, config, "inkui-1.0.1-code13-release.apk", "release", tag, previous)

    def test_formal_release_matches_tag(self):
        self.assertEqual(self.formal()["versionName"], "1.0.1")

    def test_formal_rejects_missing_history_wrong_tag_and_dev_tag(self):
        for tag, previous in (("v1.0.2", 12), ("1.0.1", 12), ("v1.0.1-dev", 12), ("v1.0.1", None), (None, 12)):
            with self.subTest(tag=tag, previous=previous), self.assertRaises(gate.VerificationError):
                self.formal(tag, previous)

    def test_formal_release_needs_public_signer_pin(self):
        with self.assertRaisesRegex(gate.VerificationError, "expected-cert"):
            gate.verify_apk(Path("missing.apk"), Path("."), "release", "v1.0.1", 12)

    def test_assets_match_exactly(self):
        with tempfile.TemporaryDirectory() as directory:
            ui = Path(directory)
            (ui / "app.js").write_bytes(b"valid")
            (ui / "test_model.mjs").write_bytes(b"test only")
            for files in ({"assets/app.js": b"valid"}, {"assets/app.js": b"stale"}, {},
                          {"assets/app.js": b"valid", "assets/test_model.mjs": b"test only"}):
                with self.subTest(files=files):
                    output = io.BytesIO()
                    with zipfile.ZipFile(output, "w") as archive:
                        for name, content in files.items():
                            archive.writestr(name, content)
                    with zipfile.ZipFile(output) as archive:
                        if files == {"assets/app.js": b"valid"}:
                            self.assertEqual(gate.verify_assets(archive, ui), 1)
                        else:
                            with self.assertRaises(gate.VerificationError):
                                gate.verify_assets(archive, ui)

    def test_signature_requires_v2_single_signer_and_expected_cert(self):
        cert = "ab" * 32
        valid = "Verified using v2 scheme (APK Signature Scheme v2): true\nSigner #1 certificate SHA-256 digest: " + cert
        self.assertEqual(gate.verify_signature_output(valid, ":".join(["AB"] * 32)), cert)
        for output, expected in ((valid.replace("true", "false"), cert), (valid, "cd" * 32),
                                 (valid + "\nSigner #2 certificate SHA-256 digest: " + cert, cert), ("", cert)):
            with self.subTest(output=output, expected=expected), self.assertRaises(gate.VerificationError):
                gate.verify_signature_output(output, expected)


if __name__ == "__main__":
    unittest.main()
