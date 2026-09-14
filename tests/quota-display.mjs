import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../web/app.js", import.meta.url), "utf8");
const remainingSource = source.match(/function remainingPercent\(usedPercent\) \{[\s\S]*?\n\}/);
const quotaSource = source.match(/function quotaWindows\(bucket\) \{[\s\S]*?\n\}/);

assert.ok(remainingSource, "remainingPercent must remain a pure function in app.js");
assert.ok(quotaSource, "quotaWindows must remain a pure function in app.js");

const remainingPercent = vm.runInNewContext(`(${remainingSource[0]})`);
const quotaWindows = vm.runInNewContext(`(${quotaSource[0]})`);

const proLiteWeek = { usedPercent: 37, windowDurationMins: 10080 };
const proLiteWindows = quotaWindows({ primary: proLiteWeek });
assert.equal(proLiteWindows.fiveHour, null);
assert.equal(proLiteWindows.week, proLiteWeek);
assert.equal(remainingPercent(proLiteWeek.usedPercent), 63);

const sparkFiveHour = { usedPercent: 100, windowDurationMins: 300 };
const sparkWeek = { usedPercent: 47, windowDurationMins: 10080 };
const sparkWindows = quotaWindows({ primary: sparkFiveHour, secondary: sparkWeek });
assert.equal(remainingPercent(sparkWindows.fiveHour.usedPercent), 0);
assert.equal(remainingPercent(sparkWindows.week.usedPercent), 53);
assert.equal(remainingPercent(-10), 100);
assert.equal(remainingPercent(110), 0);
assert.ok(Number.isNaN(remainingPercent("not-a-number")));

console.log("Quota display checks passed");
