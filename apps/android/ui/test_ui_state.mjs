import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeReadingState, readingProviderIds, restoreReadingProvider, createReadingStateStore } from './ui-state.mjs';
const providers = [{ provider: 'a', accountEmail: 'private-a@example.test' }, { provider: 'b', accountName: 'private-b' }];
function harness() {
  const values = new Map(); let writes = 0;
  return { values, store: createReadingStateStore({ getItem: key => values.get(key) || null,
    setItem: (key, value) => { writes++; values.set(key, value); } }), writes: () => writes };
}
test('reading state keeps only validated structural preferences', () => {
  const value = normalizeReadingState({ page: 'activity', period: 'month', activityKind: 'day', detailKind: 'devices', runningOnly: true,
    providerKey: 5, providerId: 'private-b', index: 7, hubSecret: 'secret', data: providers, orderEditor: providers, drill: { title: 'private' } });
  assert.deepEqual(value, { page: 'activity', period: 'month', activityKind: 'day', detailKind: 'devices', runningOnly: true, providerId: null, index: 7 });
  for (const input of [null, [], {}, { page: 'invalid', period: 'invalid', index: -1 }, { index: Infinity }])
    assert.deepEqual(normalizeReadingState(input), normalizeReadingState());
});
test('normalized Hub origins isolate states and omit paths, credentials and query text', () => {
  const h = harness(), state = { page: 'details', detailKind: 'projects', period: 'allTime', index: 12 };
  assert.equal(h.store.save('HTTP://HUB:80/path?secret=private', state), true);
  assert.equal(h.store.load('http://hub/').index, 12);
  assert.equal(h.store.load('https://hub').index, 0);
  assert.equal(h.store.load('http://other').page, 'limits');
  assert.equal([...h.values.keys()][0], 'inkui.reading-state.v1:http://hub');
  assert.equal(h.store.save('', state), false);
});
test('stable provider selection survives source reorder and plan changes without storing account fields', () => {
  const ids = readingProviderIds(providers), changed = [{ ...providers[1], plan: 'new' }, providers[0]];
  assert.equal(restoreReadingProvider(ids[1], changed), 0);
  const h = harness(); h.store.save('http://hub', { providerId: ids[1], index: 3 });
  const payload = [...h.values.values()][0];
  assert.equal(payload.includes('private'), false); assert.equal(payload.includes('account'), false);
  assert.equal(h.store.load('http://hub').providerId, ids[1]);
  assert.equal(restoreReadingProvider(ids[1], [providers[0]]), null);
  assert.equal(restoreReadingProvider(ids[1], [providers[1], providers[1]]), null);
  assert.deepEqual(readingProviderIds([{ provider: 'a' }, { provider: 'a' }]), [null, null]);
});
test('unchanged polls do not rewrite storage and a pending index survives loading until real data is available', () => {
  const h = harness(), state = { page: 'sessions', runningOnly: true, index: 9 };
  h.store.save('http://hub', state);
  const desired = h.store.load('http://hub');
  // Loading has no row count yet; reading the preference must not normalize it to 0.
  assert.equal(desired.index, 9);
  for (let i = 0; i < 10; i++) h.store.save('http://hub', desired);
  assert.equal(h.writes(), 1);
  h.store.save('http://hub', { ...desired, index: Math.min(desired.index, 3 - 1) });
  assert.equal(h.store.load('http://hub').index, 2); assert.equal(h.writes(), 2);
});
test('corrupt and unavailable storage uses safe defaults', () => {
  const bad = createReadingStateStore({ getItem() { throw Error('unavailable'); }, setItem() { throw Error('unavailable'); } });
  assert.deepEqual(bad.load('http://hub'), normalizeReadingState()); assert.equal(bad.save('http://hub', {}), false);
});
