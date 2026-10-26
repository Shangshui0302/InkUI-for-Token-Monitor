#!/usr/bin/env python3
"""Restore CI signing inputs in a private temporary directory, without logging them."""

import base64
import binascii
import os
from pathlib import Path
import sys


def property_value(value):
    """Encode a value for java.util.Properties.load(InputStream), which reads Latin-1."""
    result = []
    escapes = {"\\": "\\\\", "\t": "\\t", "\n": "\\n", "\r": "\\r", "\f": "\\f"}
    for character in value:
        if character in escapes:
            result.append(escapes[character])
        elif character in " =:#!":
            result.append("\\" + character)
        elif 0x20 <= ord(character) <= 0x7e:
            result.append(character)
        else:
            encoded = character.encode("utf-16-be")
            result.extend("\\u" + encoded[index:index + 2].hex() for index in range(0, len(encoded), 2))
    return "".join(result)


def private_write(path, content):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as destination:
        destination.write(content)


def prepare_signing(destination, environment):
    names = ("INKUI_KEYSTORE_BASE64", "INKUI_STORE_PASSWORD", "INKUI_KEY_ALIAS", "INKUI_KEY_PASSWORD")
    if any(not environment.get(name) for name in names):
        raise ValueError("Missing CI signing input")
    try:
        keystore = base64.b64decode("".join(environment[names[0]].split()), validate=True)
    except (ValueError, binascii.Error) as error:
        raise ValueError("Invalid CI keystore encoding") from error
    if not keystore:
        raise ValueError("Empty CI keystore")
    destination = Path(destination).resolve()
    destination.mkdir(mode=0o700)
    keystore_path = destination / "release.keystore"
    private_write(keystore_path, keystore)
    properties = {
        "storeFile": str(keystore_path),
        "storePassword": environment["INKUI_STORE_PASSWORD"],
        "keyAlias": environment["INKUI_KEY_ALIAS"],
        "keyPassword": environment["INKUI_KEY_PASSWORD"],
    }
    content = "".join(key + "=" + property_value(value) + "\n" for key, value in properties.items())
    private_write(destination / "release.properties", content.encode("ascii"))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage: prepare-ci-signing.py PRIVATE_TEMP_DIRECTORY")
    try:
        prepare_signing(sys.argv[1], os.environ)
    except (ValueError, OSError, UnicodeError):
        # Never include exception contents: paths or malformed inputs could contain secrets.
        sys.exit("Could not prepare private CI signing files")
