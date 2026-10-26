#!/usr/bin/env python3
"""Verify the delivered APK itself; no private signing material is read.

Formal release example:
  tools/verify-eink-release.sh output/android/inkui-1.0.1-code14-release.apk \
      --tag v1.0.1 --previous-code 13 --expected-cert-sha256 <public fingerprint>

Binary XML layout follows Android's ResourceTypes.h (RES_XML_START_ELEMENT_TYPE,
ResXMLTree_attrExt, Res_value, ResStringPool_header). The small reader avoids
executing SDK native binaries on hosts such as NixOS.
https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/libs/androidfw/include/androidfw/ResourceTypes.h
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import zipfile


class VerificationError(ValueError):
    pass


ANDROID = "http://schemas.android.com/apk/res/android"
NONE = 0xFFFFFFFF


def binary_manifest(data):
    """Read typed attributes from a compiled Android binary XML manifest."""
    def unpack(fmt, offset):
        try:
            return struct.unpack_from("<" + fmt, data, offset)
        except struct.error as exc:
            raise VerificationError("Truncated binary manifest") from exc

    kind, header, total = unpack("HHI", 0)
    if kind != 3 or header != 8 or total != len(data):
        raise VerificationError("APK manifest is not a complete Android binary XML")
    strings = []
    elements = {}
    offset = header
    while offset < total:
        kind, header, size = unpack("HHI", offset)
        end = offset + size
        if header < 8 or size < header or end > total:
            raise VerificationError("Invalid manifest chunk bounds")
        if kind == 1:
            if header < 28:
                raise VerificationError("Invalid manifest string pool")
            count, _, flags, start, _ = unpack("IIIII", offset + 8)
            if header + count * 4 > size or start < header + count * 4:
                raise VerificationError("Invalid manifest string offsets")
            utf8 = bool(flags & 0x100)

            def length_at(position):
                if utf8:
                    first = unpack("B", position)[0]
                    if first & 0x80:
                        return ((first & 0x7F) << 8) | unpack("B", position + 1)[0], position + 2
                    return first, position + 1
                first = unpack("H", position)[0]
                if first & 0x8000:
                    return ((first & 0x7FFF) << 16) | unpack("H", position + 2)[0], position + 4
                return first, position + 2

            for i in range(count):
                position = offset + start + unpack("I", offset + header + 4 * i)[0]
                if utf8:
                    _, position = length_at(position)  # UTF-16 character length
                length, position = length_at(position)
                byte_length = length if utf8 else length * 2
                terminator = 1 if utf8 else 2
                if position + byte_length + terminator > end:
                    raise VerificationError("Manifest string exceeds pool")
                try:
                    strings.append(data[position:position + byte_length].decode("utf-8" if utf8 else "utf-16-le"))
                except UnicodeError as exc:
                    raise VerificationError("Invalid manifest string encoding") from exc
        elif kind == 0x0102:
            if header != 16 or size < 36:
                raise VerificationError("Invalid manifest start element")

            def string(index):
                if index >= len(strings):
                    raise VerificationError("Invalid manifest string index")
                return strings[index]

            _, name, attr_start, attr_size, attr_count = unpack("IIHHH", offset + header)
            position = offset + header + attr_start
            if attr_start < 20 or attr_size < 20 or position + attr_count * attr_size > end:
                raise VerificationError("Invalid manifest attribute bounds")
            attributes = {}
            for i in range(attr_count):
                at = position + i * attr_size
                namespace, attr_name, raw = unpack("III", at)
                value_size, _, value_type, value = unpack("HBBI", at + 12)
                if value_size != 8:
                    raise VerificationError("Invalid typed manifest value")
                if namespace != NONE and string(namespace) != ANDROID:
                    continue
                if value_type == 3:
                    value = string(value)
                elif raw != NONE:
                    value = string(raw)
                elif value_type == 0x12:
                    value = bool(value)
                elif value_type not in (0x10, 0x11):
                    continue  # Resource references are irrelevant to this gate.
                attributes[string(attr_name)] = value
            element_name = string(name)
            # The fields used here are unique top-level elements in this app.
            if element_name in ("manifest", "application", "uses-sdk"):
                if element_name in elements:
                    raise VerificationError("Duplicate manifest element: " + element_name)
                elements[element_name] = attributes
        offset = end
    if offset != total or "manifest" not in elements or "application" not in elements:
        raise VerificationError("Incomplete APK manifest")
    return elements


def source_config(repo):
    source = (repo / "apps/android/app/build.gradle").read_text()
    result = {}
    for name in ("versionName", "applicationId"):
        match = re.search(r"\b" + name + r"\s+['\"]([^'\"]+)['\"]", source)
        if not match:
            raise VerificationError("Missing source " + name)
        result[name] = match.group(1)
    for name in ("versionCode", "minSdk", "targetSdk"):
        match = re.search(r"\b" + name + r"\s+(\d+)\b", source)
        if not match:
            raise VerificationError("Missing source " + name)
        result[name] = int(match.group(1))
    return result


def verify_metadata(manifest, config, filename, variant, tag=None, previous_code=None):
    if previous_code is not None and not 0 <= previous_code <= 2100000000:
        raise VerificationError("Previous versionCode must be a nonnegative Android build number")
    root = manifest["manifest"]
    version = root.get("versionName")
    code = root.get("versionCode")
    if version != config["versionName"] or code != config["versionCode"]:
        raise VerificationError("APK versionName/versionCode differ from current source")
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-dev)?", str(version)):
        raise VerificationError("Version must be X.Y.Z or next-target X.Y.Z-dev")
    if not isinstance(code, int) or isinstance(code, bool) or not 0 < code <= 2100000000:
        raise VerificationError("Invalid Android versionCode")
    expected_package = config["applicationId"] + (".debug" if variant == "debug" else "")
    if root.get("package") != expected_package:
        raise VerificationError("APK applicationId does not match build variant")
    if bool(manifest["application"].get("debuggable", False)) != (variant == "debug"):
        raise VerificationError("APK debuggable flag does not match build variant")
    sdk = manifest.get("uses-sdk", {})
    if sdk.get("minSdkVersion") != config["minSdk"] or sdk.get("targetSdkVersion") != config["targetSdk"]:
        raise VerificationError("APK SDK levels differ from current source")
    expected_name = f"inkui-{version}-code{code}-{variant}.apk"
    if filename != expected_name:
        raise VerificationError("APK filename must be " + expected_name)
    if variant == "release" and not version.endswith("-dev") and tag is None:
        raise VerificationError("Stable release APK requires --tag, --previous-code, and --expected-cert-sha256")
    if previous_code is not None and code <= previous_code:
        raise VerificationError("versionCode must exceed the previous distributed build")
    if tag is not None:
        if not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
            raise VerificationError("Formal release tag must be vX.Y.Z")
        if variant != "release" or version != tag[1:]:
            raise VerificationError("Formal release tag must equal the release APK versionName")
        if previous_code is None:
            raise VerificationError("Formal release requires --previous-code")
    return {"versionName": version, "versionCode": code, "applicationId": expected_package, "variant": variant}


def verify_assets(apk, ui_dir):
    expected = {"assets/" + path.name: path.read_bytes() for path in ui_dir.iterdir()
                if path.is_file() and path.suffix in (".html", ".css", ".js", ".mjs")
                and not (path.suffix == ".mjs" and path.name.startswith("test_"))}
    if not expected:
        raise VerificationError("No source UI assets found")
    actual = {name for name in apk.namelist() if name.startswith("assets/") and not name.endswith("/")}
    if actual != set(expected):
        raise VerificationError("APK UI asset list differs from source: " + repr(sorted(actual ^ set(expected))))
    for name, content in expected.items():
        if apk.read(name) != content:
            raise VerificationError("APK UI asset differs from source: " + name)
    return len(expected)


def fingerprint(value):
    normalized = value.replace(":", "").lower()
    if not re.fullmatch(r"[0-9a-f]{64}", normalized):
        raise VerificationError("Expected signing certificate must be a SHA-256 fingerprint")
    return normalized


def verify_signature_output(output, expected_cert=None):
    if not re.search(r"Verified using v2 scheme[^\n]*:\s*true", output):
        raise VerificationError("APK must have a valid v2 signature")
    signers = re.findall(r"Signer #\d+ certificate SHA-256 digest:\s*([0-9a-fA-F:]+)", output)
    if len(signers) != 1:
        # Diagnostic labels only: never print a DN, digest, or arbitrary prefix
        # from tool output. These additional formats remain rejected by the gate.
        labels = []
        for line in output.splitlines():
            if " certificate SHA-256 digest:" not in line:
                continue
            prefix = line.split(" certificate SHA-256 digest:", 1)[0]
            if re.fullmatch(r"Signer #\d+", prefix):
                label = "numbered-signer"
            elif re.fullmatch(r"Signer \(minSdkVersion=\d+(?: \(dev release=true\))?, maxSdkVersion=\d+\)", prefix):
                label = "SDK-range-signer"
            elif prefix in ("V1 Signer:", "V2 Signer:", "V3.0 Signer:", "V3.1 Signer:", "V4 Signer:"):
                label = prefix
            elif prefix == "Source Stamp Signer":
                label = "source-stamp-signer"
            else:
                label = "unrecognized"
            if label not in labels and len(labels) < 8:
                labels.append(label)
        raise VerificationError("Expected exactly one APK signing certificate "
                                f"(matches={len(signers)}; certificate-labels={labels!r})")
    cert = fingerprint(signers[0])
    if expected_cert is not None and cert != fingerprint(expected_cert):
        raise VerificationError("APK certificate differs from the expected release certificate")
    return cert


def apksigner_jar(explicit=None):
    if explicit:
        jar = Path(explicit)
        if not jar.is_file():
            raise VerificationError("Missing --apksigner-jar")
        return jar
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    # Match this project's pinned Build Tools rather than a hosted runner's
    # unrelated newer SDK packages; apksigner's text format is versioned too.
    jar = Path(sdk) / "build-tools/36.0.0/lib/apksigner.jar" if sdk else None
    if jar is None or not jar.is_file():
        raise VerificationError("SDK Build Tools 36.0.0 apksigner.jar is required; "
                                "set ANDROID_HOME or an explicit --apksigner-jar")
    return jar


def verify_apk(apk_path, repo, variant, tag=None, previous_code=None, expected_cert=None, signer_jar=None):
    if tag is not None and expected_cert is None:
        raise VerificationError("Formal release requires --expected-cert-sha256 (public fingerprint)")
    config = source_config(repo)
    with zipfile.ZipFile(apk_path) as apk:
        if len(apk.namelist()) != len(set(apk.namelist())):
            raise VerificationError("APK contains duplicate ZIP entries")
        manifest = binary_manifest(apk.read("AndroidManifest.xml"))
        metadata = verify_metadata(manifest, config, apk_path.name, variant, tag, previous_code)
        metadata["assetsChecked"] = verify_assets(apk, repo / "apps/android/ui")
    java = str(Path(os.environ["JAVA_HOME"]) / "bin/java") if os.environ.get("JAVA_HOME") else "java"
    result = subprocess.run([java, "-jar", str(apksigner_jar(signer_jar)), "verify", "--verbose", "--print-certs", str(apk_path)],
                            text=True, capture_output=True, check=False)
    if result.returncode != 0:
        raise VerificationError("apksigner rejected APK: " + (result.stderr or result.stdout).strip())
    metadata["signerSha256"] = verify_signature_output(result.stdout, expected_cert)
    metadata["sha256"] = hashlib.sha256(apk_path.read_bytes()).hexdigest()
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("apk", type=Path)
    parser.add_argument("--variant", choices=("debug", "release"), default="release")
    parser.add_argument("--tag", help="Formal release tag vX.Y.Z; rejects -dev builds")
    parser.add_argument("--previous-code", type=int, help="Highest previously distributed Android versionCode")
    parser.add_argument("--expected-cert-sha256", help="Public release signing certificate SHA-256; required with --tag")
    parser.add_argument("--apksigner-jar", help="Override SDK apksigner.jar path")
    args = parser.parse_args()
    try:
        result = verify_apk(args.apk, Path(__file__).resolve().parent.parent, args.variant,
                            args.tag, args.previous_code, args.expected_cert_sha256, args.apksigner_jar)
    except (VerificationError, OSError, KeyError, zipfile.BadZipFile) as exc:
        parser.exit(1, "APK verification failed: " + str(exc) + "\n")
    print(json.dumps(result, ensure_ascii=False, sort_keys=True))


if __name__ == "__main__":
    main()
