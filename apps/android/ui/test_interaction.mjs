import test from 'node:test';
import assert from 'node:assert/strict';
import { swipeDirection } from './gestures.mjs';
import { createDisplayRefresh } from './display.mjs';

test('a deliberate horizontal stroke turns one page in its reading direction', () => {
  const start = { x: 300, y: 120, time: 100 };
  assert.equal(swipeDirection(start, { x: 180, y: 130, time: 500 }, 662), 1);
  assert.equal(swipeDirection(start, { x: 420, y: 110, time: 500 }, 662), -1);
  assert.equal(swipeDirection({ x: 90, y: 50, time: 0 }, { x: 50, y: 51, time: 400 }, 128), 1);
});

test('taps, short drags, diagonal/vertical movements and long holds do not turn pages', () => {
  const start = { x: 300, y: 120, time: 100 };
  for (const end of [{ x: 300, y: 120, time: 200 }, { x: 280, y: 120, time: 400 },
    { x: 200, y: 200, time: 500 }, { x: 295, y: 300, time: 500 },
    { x: 180, y: 120, time: 1200 }]) assert.equal(swipeDirection(start, end, 662), 0);
});

function displayHarness() {
  const calls = [], frames = new Map(); let serial = 0, hidden = false;
  const display = createDisplayRefresh({ requestDisplayRefresh: value => calls.push(value) }, () => hidden,
    fn => { frames.set(++serial, fn); return serial; }, id => frames.delete(id));
  return { display, calls, frames, hide: () => { hidden = true; }, flush: () => { const ready = [...frames.values()]; frames.clear(); ready.forEach(fn => fn()); } };
}
test('screen requests wait for a frame and combine by strongest refresh', () => {
  const h = displayHarness();
  h.display.request('content'); h.display.request('page'); h.display.request('content');
  assert.equal(h.frames.size, 1); assert.deepEqual(h.calls, []);
  h.flush(); assert.deepEqual(h.calls, ['page']);
  h.display.request('full'); h.display.request('page'); h.flush();
  assert.deepEqual(h.calls, ['page', 'full']);
});
test('hidden and canceled work never refreshes the panel; old APK bridge remains compatible', () => {
  const h = displayHarness(); h.display.request('page'); h.display.cancel(); h.flush();
  assert.deepEqual(h.calls, []);
  h.display.request('page'); h.hide(); h.flush(); h.display.request('full');
  assert.deepEqual(h.calls, []); assert.equal(h.frames.size, 0);
  let callback; const legacy = createDisplayRefresh({}, () => false, fn => { callback = fn; return 1; }, () => {});
  legacy.request('page'); assert.doesNotThrow(() => callback());
});
