#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { constants, accessSync, readFileSync, readdirSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function fail(message) {
  process.stderr.write(`release metadata check failed: ${message}\n`);
  process.exit(1);
}

const args = process.argv.slice(2);
let requireClean = false;
let androidApk;
let expectedAndroidCertSha256;
let nixOutput;
const artifactArgs = [];
for (let index = 0; index < args.length; index += 1) {
  const argument = args[index];
  if (argument === "--require-clean") {
    requireClean = true;
  } else if (["--android-apk", "--expected-android-cert-sha256", "--nix-output"].includes(argument)) {
    const value = args[index + 1];
    if (!value || value.startsWith("--")) fail(`${argument} requires a value`);
    index += 1;
    if (argument === "--android-apk") androidApk = value;
    else if (argument === "--nix-output") nixOutput = value;
    else expectedAndroidCertSha256 = value;
  } else if (argument.startsWith("--")) {
    fail(`unknown option: ${argument}`);
  } else {
    artifactArgs.push(argument);
  }
}
if (expectedAndroidCertSha256 && !androidApk) {
  fail("--expected-android-cert-sha256 requires --android-apk");
}
if (androidApk && !artifactArgs.includes(androidApk)) artifactArgs.push(androidApk);

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

function executable(path) {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function findBuildTool(name) {
  for (const directory of (process.env.PATH ?? "").split(delimiter)) {
    const candidate = join(directory, name);
    if (directory && executable(candidate)) return candidate;
  }
  const sdkRoots = [
    process.env.ANDROID_SDK_ROOT,
    process.env.ANDROID_HOME,
    join(homedir(), "Android", "Sdk"),
    "/opt/android-sdk",
  ].filter(Boolean);
  for (const sdkRoot of [...new Set(sdkRoots)]) {
    let versions;
    try {
      versions = readdirSync(join(sdkRoot, "build-tools"), { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort((left, right) => right.localeCompare(left, undefined, { numeric: true }));
    } catch {
      continue;
    }
    for (const version of versions) {
      const candidate = join(sdkRoot, "build-tools", version, name);
      if (executable(candidate)) return candidate;
    }
  }
  fail(`Android SDK build tool is unavailable: ${name}`);
}

function runTool(tool, toolArgs, label) {
  try {
    return execFileSync(tool, toolArgs, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    const detail = String(error.stderr || error.message || error).trim();
    fail(`${label} failed${detail ? `: ${detail}` : ""}`);
  }
}

function normalizeCertificateDigest(value) {
  const normalized = value.replace(/[^0-9a-f]/gi, "").toLowerCase();
  if (normalized.length !== 64) fail(`invalid SHA-256 certificate digest: ${value}`);
  return normalized;
}

function verifyAndroidArtifact(argument) {
  const path = resolve(process.cwd(), argument);
  const aapt = findBuildTool("aapt");
  const apksigner = findBuildTool("apksigner");
  const badging = runTool(aapt, ["dump", "badging", path], "Android manifest inspection");
  const packageLine = badging.split("\n").find((line) => line.startsWith("package:"));
  const identifier = packageLine?.match(/\bname='([^']+)'/)?.[1];
  const versionCode = packageLine?.match(/\bversionCode='([^']+)'/)?.[1];
  const versionName = packageLine?.match(/\bversionName='([^']+)'/)?.[1];
  if (!identifier || !versionCode || !versionName) fail("unable to read Android package metadata");
  if (identifier !== tauri.identifier) {
    fail(`Android identifier mismatch: expected ${tauri.identifier}, got ${identifier}`);
  }
  if (versionCode !== String(androidVersionCode)) {
    fail(`Android versionCode mismatch: expected ${androidVersionCode}, got ${versionCode}`);
  }
  if (versionName !== tauriVersion) {
    fail(`Android versionName mismatch: expected ${tauriVersion}, got ${versionName}`);
  }

  const signature = runTool(
    apksigner,
    ["verify", "--verbose", "--print-certs", path],
    "Android signature verification",
  );
  const signerCertSha256 = [...signature.matchAll(/Signer #\d+ certificate SHA-256 digest:\s*([0-9a-f]+)/gi)]
    .map((match) => normalizeCertificateDigest(match[1]));
  if (!signerCertSha256.length) fail("Android signature has no SHA-256 signer certificate digest");
  if (expectedAndroidCertSha256) {
    const expected = normalizeCertificateDigest(expectedAndroidCertSha256);
    if (!signerCertSha256.includes(expected)) {
      fail(`Android signer certificate mismatch: expected ${expected}`);
    }
  }
  return {
    path: argument,
    identifier,
    versionCode: Number(versionCode),
    versionName,
    signerCertSha256,
  };
}

function verifyNixOutput(argument) {
  const path = resolve(process.cwd(), argument);
  let stat;
  try {
    stat = statSync(path);
  } catch (error) {
    fail(`unable to inspect Nix output ${argument}: ${error.message}`);
  }
  if (!stat.isDirectory()) fail(`Nix output is not a directory: ${argument}`);

  const program = join(path, "bin", "nekodex-shell");
  const desktop = join(path, "share", "applications", "nekodex.desktop");
  const icon = join(path, "share", "icons", "hicolor", "scalable", "apps", "nekodex.svg");
  const license = join(path, "share", "licenses", "nekodex", "LICENSE");
  if (!executable(program)) fail(`Nix output main program is missing or not executable: ${program}`);
  for (const [label, requiredPath] of [["desktop entry", desktop], ["icon", icon], ["license", license]]) {
    try {
      if (!statSync(requiredPath).isFile()) fail(`Nix output ${label} is not a regular file: ${requiredPath}`);
    } catch {
      fail(`Nix output is missing ${label}: ${requiredPath}`);
    }
  }
  if (!readFileSync(license).equals(readFileSync(resolve(root, "LICENSE")))) {
    fail("Nix output license does not match the repository LICENSE");
  }

  const pathInfoText = runTool("nix", ["path-info", "--json", path], "Nix output metadata inspection");
  let pathInfo;
  try {
    pathInfo = JSON.parse(pathInfoText);
  } catch {
    fail("Nix path-info returned invalid JSON");
  }
  const entries = Object.entries(pathInfo);
  if (entries.length !== 1) fail("Nix path-info did not return exactly one output");
  const [storePath, metadata] = entries[0];
  if (typeof metadata.narHash !== "string" || typeof metadata.narSize !== "number") {
    fail("Nix path-info omitted narHash or narSize");
  }
  return {
    path: argument,
    storePath,
    narHash: metadata.narHash,
    narBytes: metadata.narSize,
    program: "bin/nekodex-shell",
    desktop: "share/applications/nekodex.desktop",
    icon: "share/icons/hicolor/scalable/apps/nekodex.svg",
    license: "share/licenses/nekodex/LICENSE",
  };
}

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
const android = androidApk ? verifyAndroidArtifact(androidApk) : null;
const nix = nixOutput ? verifyNixOutput(nixOutput) : null;

process.stdout.write(`${JSON.stringify({
  product: tauri.productName,
  identifier: tauri.identifier,
  version: tauriVersion,
  androidVersionCode,
  commit: git("rev-parse", "HEAD"),
  artifacts,
  android,
  nix,
}, null, 2)}\n`);
