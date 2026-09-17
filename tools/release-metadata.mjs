#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const requireClean = args.includes("--require-clean");
const artifactArgs = args.filter((argument) => argument !== "--require-clean");

function fail(message) {
  process.stderr.write(`release metadata check failed: ${message}\n`);
  process.exit(1);
}

function section(text, name) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = text.match(new RegExp(`^\\[${escaped}\\]\\s*\\n([\\s\\S]*?)(?=^\\[|\\Z)`, "m"));
  return match?.[1] ?? fail(`missing [${name}] section`);
}

function quotedVersion(text, label) {
  const match = text.match(/^version\s*=\s*"([^"]+)"\s*$/m);
  return match?.[1] ?? fail(`missing ${label} version`);
}

const tauri = JSON.parse(readFileSync(resolve(root, "apps/shell/src-tauri/tauri.conf.json"), "utf8"));
const tauriVersion = tauri.version;
if (typeof tauriVersion !== "string") fail("tauri.conf.json version is not a string");

const cargoText = readFileSync(resolve(root, "Cargo.toml"), "utf8");
const cargoVersion = quotedVersion(section(cargoText, "workspace.package"), "workspace");

const flakeText = readFileSync(resolve(root, "flake.nix"), "utf8");
const flakeMatch = flakeText.match(
  /nekodex\s*=\s*pkgs\.rustPlatform\.buildRustPackage\s*\{[\s\S]*?pname\s*=\s*"nekodex";\s*version\s*=\s*"([^"]+)";/,
);
const flakeVersion = flakeMatch?.[1] ?? fail("missing nekodex Nix package version");

const versions = new Set([tauriVersion, cargoVersion, flakeVersion]);
if (versions.size !== 1) {
  fail(`version mismatch: tauri=${tauriVersion}, cargo=${cargoVersion}, nix=${flakeVersion}`);
}

const semver = tauriVersion.match(/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);
if (!semver) fail(`release version must be numeric major.minor.patch, got ${tauriVersion}`);
const [major, minor, patch] = semver.slice(1).map(Number);
if (minor >= 1000 || patch >= 1000) fail("minor and patch must stay below 1000 for Android versionCode mapping");
const androidVersionCode = major * 1_000_000 + minor * 1_000 + patch;
if (!Number.isSafeInteger(androidVersionCode) || androidVersionCode <= 0 || androidVersionCode > 2_100_000_000) {
  fail(`derived Android versionCode is out of range: ${androidVersionCode}`);
}

const git = (...gitArgs) => execFileSync("git", gitArgs, { cwd: root, encoding: "utf8" }).trim();
if (requireClean && git("status", "--porcelain").length > 0) fail("Git worktree is not clean");

const artifacts = artifactArgs.map((argument) => {
  const path = resolve(process.cwd(), argument);
  const stat = statSync(path);
  if (!stat.isFile()) fail(`artifact is not a regular file: ${argument}`);
  return {
    path: argument,
    bytes: stat.size,
    sha256: createHash("sha256").update(readFileSync(path)).digest("hex"),
  };
});

process.stdout.write(`${JSON.stringify({
  product: tauri.productName,
  identifier: tauri.identifier,
  version: tauriVersion,
  androidVersionCode,
  commit: git("rev-parse", "HEAD"),
  artifacts,
}, null, 2)}\n`);
