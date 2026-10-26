import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const source = readFileSync(new URL('./boot.js', import.meta.url), 'utf8');
function harness(unsupported = false, missingHasOwn = false) {
  const handlers = {}, failures = [];
  const element = { replaceChildren() {}, toggleAttribute() {}, closest() {} };
  const window = { TokenMonitorAndroid: { uiFailed: category => failures.push(category) },
    addEventListener: (name, fn) => { handlers[name] = fn; },
    fetch() {}, AbortController, Promise, URL, Map, Set, queueMicrotask, requestAnimationFrame() {}, CustomEvent() {}, CSS: { supports: () => true } };
  if (unsupported) delete window.AbortController;
  const context = vm.createContext({ window, document: { documentElement: element, createElement: () => ({ noModule: false }) } });
  if (missingHasOwn) vm.runInContext('Object.hasOwn = undefined', context);
  vm.runInContext(source, context);
  return { window, handlers, failures };
}
test('bootstrap reports unsupported required capabilities with a fixed safe category', () => {
  const h = harness(true); assert.deepEqual(h.failures, ['unsupported_webview']);
  assert.equal(h.window.TokenMonitorBoot.ready(), false);
});
test('bootstrap identifies failed module resources without forwarding URL or error text', () => {
  const h = harness(); h.handlers.error({ target: { tagName: 'SCRIPT', src: 'private' }, message: 'private' });
  h.handlers.unhandledrejection({ reason: 'private' }); assert.deepEqual(h.failures, ['module_load_error']);
});
test('bootstrap catches parse/runtime errors and stops observing startup failures after readiness', () => {
  const failed = harness(); failed.handlers.error({ message: 'private stack' });
  assert.deepEqual(failed.failures, ['bootstrap_error']);
  const ready = harness(); assert.equal(ready.window.TokenMonitorBoot.ready(), true);
  ready.handlers.error({ message: 'later error' }); ready.handlers.unhandledrejection({}); assert.deepEqual(ready.failures, []);
});

test('bootstrap detects WebViews missing Object.hasOwn before the module uses it', () => {
  const h = harness(false, true); assert.deepEqual(h.failures, ['unsupported_webview']);
});
