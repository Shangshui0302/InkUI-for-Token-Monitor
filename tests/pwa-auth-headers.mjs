import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const appPath = new URL("../web/app.js", import.meta.url);
const source = fs.readFileSync(appPath, "utf8");
const helper = source.match(/function readRequestHeaders\(token\) \{[\s\S]*?\n\}/);

assert.ok(helper, "readRequestHeaders must remain a pure function in app.js");
const readRequestHeaders = vm.runInNewContext(`(${helper[0]})`);

assert.deepEqual({ ...readRequestHeaders("read-secret") }, {
  Accept: "application/json",
  Authorization: "Bearer read-secret",
});
const emptyHeaders = readRequestHeaders("");
assert.deepEqual({ ...emptyHeaders }, { Accept: "application/json" });
assert.equal(Object.hasOwn(emptyHeaders, "Authorization"), false);
assert.match(source, /const headers = readRequestHeaders\(clientToken\);/);
assert.match(source, /if \(typeof Element\.prototype\.replaceChildren !== "function"\)/);
assert.match(source, /Element\.prototype\.replaceChildren = function \(\.\.\.nodes\)/);
assert.match(source, /node instanceof Node \? node : document\.createTextNode\(String\(node\)\)/);

console.log("PWA read-token header checks passed");
