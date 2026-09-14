import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../web/app.js", import.meta.url), "utf8");

function helper(name, end = "\\n\\}") {
  const match = source.match(new RegExp(`function ${name}\\([\\s\\S]*?${end}`));
  assert.ok(match, `${name} must remain a pure helper in app.js`);
  return match[0];
}

const context = {
  ACCENT_PALETTES: {
    red: {}, orange: {}, yellow: {}, green: {}, cyan: {}, blue: {}, purple: {},
  },
};
const normalizeAccent = vm.runInNewContext(`(${helper("normalizeAccent")})`, context);
const daysInMonth = vm.runInNewContext(`(${helper("daysInMonth")})`, {});
const monthCalendar = vm.runInNewContext(`(${helper("monthCalendar")})`, { daysInMonth });
const dailyMap = vm.runInNewContext(`(${helper("dailyMap")})`, {});
const dailyLevel = vm.runInNewContext(`(${helper("dailyLevel")})`, {});
const isSparkBucket = vm.runInNewContext(`(${helper("isSparkBucket")})`, {});
const visibleRateLimits = vm.runInNewContext(`(${helper("visibleRateLimits")})`, { isSparkBucket });
const otherPeerDevices = vm.runInNewContext(`(${helper("otherPeerDevices")})`, {});
const normalizePairingCode = vm.runInNewContext(`(${helper("normalizePairingCode")})`, {});
const validPairingCode = vm.runInNewContext(`(${helper("validPairingCode")})`, {});

const february = monthCalendar("2021-02");
assert.equal(february.length, 28, "February 2021 should start on Monday and occupy four weeks");
assert.equal(february[0].date, "2021-02-01");
assert.equal(february[27].date, "2021-02-28");
const november = monthCalendar("2026-11");
assert.equal(november.length, 42, "a six-week month should retain all calendar rows");
assert.equal(november.filter((cell) => cell.outside).length, 12);
assert.equal(daysInMonth("2024-02"), 29);

const usage = dailyMap({ dailyUsage: [{ startDate: "2026-09-01", tokens: 0 }] });
assert.equal(usage.has("2026-09-01"), true, "a returned zero must remain present");
assert.equal(usage.get("2026-09-01"), 0);
assert.equal(usage.has("2026-09-02"), false, "a missing date must not be synthesized");
assert.equal(dailyLevel(0, 100, true), 1);
assert.equal(dailyLevel(0, 100, false), 0);

const spark = { limitId: "codex_bengalfox", limitName: "GPT-5.3-Codex-Spark" };
assert.equal(isSparkBucket(spark), true);
const buckets = [{ limitId: "codex", limitName: "Codex" }, spark, { limitId: "other", limitName: "Other" }];
assert.deepEqual(visibleRateLimits({ rateLimits: buckets }), [buckets[0], buckets[2]]);
assert.deepEqual(buckets, [{ limitId: "codex", limitName: "Codex" }, spark, { limitId: "other", limitName: "Other" }]);
assert.deepEqual(otherPeerDevices([
  { nodeId: "self" },
  { nodeId: "pc-a" },
  { nodeId: "pc-a" },
  { nodeId: "pc-b" },
], "self"), [{ nodeId: "pc-a" }, { nodeId: "pc-b" }]);
assert.equal(normalizePairingCode("  aB12cd34  "), "aB12cd34", "only outer whitespace is trimmed; case is preserved");
assert.equal(validPairingCode("aB12cd34"), true);
assert.equal(validPairingCode("AB12CD34"), true);
assert.equal(validPairingCode("AB12CD3"), false);
assert.equal(validPairingCode("AB12CD3!"), false);

assert.equal(normalizeAccent("purple"), "purple");
assert.equal(normalizeAccent("unknown"), "green");
assert.match(source, /writeStorage\(ACCENT_STORAGE_KEY, accent\)/);
assert.match(source, /applyAccent\(readStorage\(ACCENT_STORAGE_KEY\), false\)/);
assert.match(source, /pairingCodeRequestGeneration/);
assert.match(source, /pairingCodeToggle\.disabled = false/);
assert.match(source, /pendingPairNodeIds/);
assert.match(source, /timeoutMs: 25_000/);
assert.doesNotMatch(source, /pairingCode\.inputMode = "numeric"/);
assert.match(source, /paired: "已配对"/);
assert.match(source, /pairingCode\.maxLength = 8/);
assert.match(source, /pairingCode\.minLength = 8/);
assert.match(source, /pairingCode\.pattern = "\[A-Za-z0-9\]\{8\}"/);
assert.match(source, /pairingCode\.setAttribute\("autocapitalize", "off"\)/);
assert.match(source, /currentEndpointOnline \? "connected" : "unknown"/);
assert.match(source, /requestEndpointGeneration !== endpointGeneration/);
assert.match(fs.readFileSync(new URL("../web/sw.js", import.meta.url), "utf8"), /codex-monitor-shell-v21/);
assert.match(source, /kind === "face" \? "peeking" : kind/);
assert.doesNotMatch(source, /createElementNS\("http:\/\/www\.w3\.org\/2000\/svg", "svg"\)/);
assert.match(fs.readFileSync(new URL("../web/styles.css", import.meta.url), "utf8"), /background-image: url\("\/cat-sprites\.png"\)/);

console.log("PWA visual logic checks passed");
