import test from 'node:test';
import assert from 'node:assert/strict';
import { providerOrderIds, orderedProviderIndices, mergeProviderOrder, createProviderOrderStore } from './provider-order.mjs';

const account = (email, extra = {}) => ({ provider: 'codex', accountEmail: email, ...extra });
const accounts = () => [account('a@example.invalid'), account('b@example.invalid'), account('c@example.invalid')];
function memoryStorage() {
  const data = new Map();
  return { data, getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value), removeItem: key => data.delete(key) };
}

test('preferred identity is stable through quota, plan, status and source reorder changes', () => {
  const raw = accounts(), ids = providerOrderIds(raw);
  assert.equal(new Set(ids).size, 3);
  const changed = [raw[2], raw[0], raw[1]].map(p => ({ ...p, accountLabel: 'renamed', accountName: 'renamed', plan: 'pro', status: 'offline', windows: [{ remainingPercent: 1 }], secret: 'do-not-store' }));
  assert.deepEqual(providerOrderIds(changed), [ids[2], ids[0], ids[1]]);
  assert.deepEqual(orderedProviderIndices(changed, [ids[1], ids[2], ids[0]]), [2, 0, 1]);
  assert.equal(JSON.stringify(providerOrderIds(changed)).includes('do-not-store'), false);
});

test('fallback identities and provider, adapter and region remain distinct', () => {
  const raw = [account('', { accountName: 'Alice', accountLabel: 'label' }), account('', { accountLabel: 'Alice' }), account('a', { adapterId: 'x' }), account('a', { region: 'eu' }), account('a', { provider: 'other' })];
  const ids = providerOrderIds(raw);
  assert.equal(new Set(ids).size, raw.length);
  assert.equal(ids[0], providerOrderIds([{ ...raw[0], accountLabel: 'different' }])[0]);
  assert.deepEqual(providerOrderIds(null), []);
  assert.deepEqual(providerOrderIds([null, {}, 4]), [null, null, null]);
});

test('fully repeated identities are ambiguous and cannot inherit saved ordering', () => {
  const one = account('same', { plan: 'pro' }), other = account('other');
  const oneId = providerOrderIds([one])[0], otherId = providerOrderIds([other])[0];
  const raw = [one, other, { ...one, plan: 'free', status: 'error' }];
  assert.deepEqual(providerOrderIds(raw), [null, otherId, null]);
  assert.deepEqual(orderedProviderIndices(raw, [oneId, otherId]), [1, 0, 2]);
  assert.deepEqual(providerOrderIds([{ provider: 'codex' }, { provider: 'codex' }]), [null, null]);
});

test('natural source order, new accounts and invalid saved entries never lose rows', () => {
  const raw = accounts(), ids = providerOrderIds(raw);
  assert.deepEqual(orderedProviderIndices(raw, []), [0, 1, 2]);
  assert.deepEqual(orderedProviderIndices(raw, null), [0, 1, 2]);
  assert.deepEqual(orderedProviderIndices(raw, [null, ids[1], ids[1], 'gone', {}, ids[0]]), [1, 0, 2]);
});

test('editing preserves offline slots and visible sequence, then restoration follows saved order', () => {
  const [a, b, c] = accounts(), d = account('d@example.invalid');
  const [ai, bi, ci, di] = providerOrderIds([a, b, c, d]);
  const saved = [ai, bi, ci], visible = [ci, di, ai];
  const merged = mergeProviderOrder(saved, visible);
  assert.deepEqual(merged, [ci, bi, di, ai]);
  assert.deepEqual(orderedProviderIndices([d, a, b, c], merged), [3, 2, 0, 1]);
  assert.deepEqual(saved, [ai, bi, ci]); assert.deepEqual(visible, [ci, di, ai]);
  assert.deepEqual(mergeProviderOrder([ai, null, bi, ai], [bi, null, bi, ci]), [ai, bi, ci]);
  assert.deepEqual(mergeProviderOrder(saved, []), saved);
  assert.deepEqual(mergeProviderOrder(null, [ai, ai, null, bi]), [ai, bi]);
});

test('new accounts can be edited to the front, middle or end with offline accounts retaining slots', () => {
  const [a, b, c] = accounts(), d = account('d@example.invalid'), e = account('e@example.invalid');
  const [ai, bi, ci, di, ei] = providerOrderIds([a, b, c, d, e]);
  for (const visible of [[di, bi, ai], [bi, di, ai], [bi, ai, di]]) {
    const merged = mergeProviderOrder([ai, bi], visible);
    assert.deepEqual(merged, visible);
    const raw = [a, b, d];
    assert.deepEqual(orderedProviderIndices(raw, merged).map(index => providerOrderIds(raw)[index]), visible);
  }
  for (const visible of [[di, ci, ei, ai], [ci, di, ai, ei], [ci, ai, di, ei]]) {
    const merged = mergeProviderOrder([ai, bi, ci], visible);
    assert.equal(merged[1], bi);
    assert.deepEqual(merged.filter(id => id !== bi), visible);
    const raw = [a, c, d, e];
    assert.deepEqual(orderedProviderIndices(raw, merged).map(index => providerOrderIds(raw)[index]), visible);
  }
  assert.deepEqual(mergeProviderOrder([bi, ei], [di, ci, ai]), [bi, ei, di, ci, ai]);
});

test('store persists across instances, isolates Hub origins and normalizes default ports', () => {
  const storage = memoryStorage(), store = createProviderOrderStore(storage), ids = providerOrderIds(accounts());
  assert.equal(store.save(' HTTPS://Hub.Example:443/ ', ids), true);
  assert.deepEqual(createProviderOrderStore(storage).load('https://hub.example'), ids);
  assert.deepEqual(store.load('http://hub.example'), []);
  assert.equal(store.save('https://other.example:9443', [ids[1]]), true);
  assert.deepEqual(store.load('https://other.example:9443'), [ids[1]]);
  assert.deepEqual(store.load('https://other.example'), []);
  assert.equal(store.save('https://user:private@hub.example/path?token=private#private', ids), true);
  assert.equal([...storage.data.keys()].some(key => /private|path|token/.test(key)), false);
  assert.deepEqual([...storage.data.values()].map(JSON.parse), [ids, [ids[1]]]);
  assert.equal(store.reset('https://hub.example:443/'), true);
  assert.deepEqual(store.load('https://hub.example'), []);
  assert.deepEqual(store.load('https://other.example:9443'), [ids[1]]);
});

test('unconfigured or invalid Hub never accesses storage', () => {
  const storage = new Proxy({}, { get() { assert.fail('must not access storage'); } });
  const store = createProviderOrderStore(storage);
  for (const url of ['', null, undefined, 'hub', 'file:///private', 'not a URL']) {
    assert.deepEqual(store.load(url), []);
    assert.equal(store.save(url, ['a']), false);
    assert.equal(store.reset(url), false);
  }
});

test('malformed JSON and fields are ignored; only unique string IDs are retained', () => {
  const storage = memoryStorage(), store = createProviderOrderStore(storage), url = 'http://hub';
  store.save(url, ['a']); const key = [...storage.data.keys()][0];
  for (const value of ['{', '{}', 'null', '4', '"a"']) {
    storage.data.set(key, value); assert.deepEqual(store.load(url), []);
  }
  storage.data.set(key, JSON.stringify(['a', null, 9, { secret: 'private' }, '', ' ', 'a', 'b']));
  assert.deepEqual(store.load(url), ['a', 'b']);
  assert.equal(store.save(url, { secret: 'private' }), false);
  assert.equal(store.save(url, ['a', null, 'a', 'b']), true);
  assert.equal(storage.data.get(key), '["a","b"]');
});

test('storage denial, failed writes, failed deletions and failed readback are explicit', () => {
  const denied = new Proxy({}, { get() { throw Error('denied'); } });
  for (const storage of [undefined, null, denied]) {
    const store = createProviderOrderStore(storage);
    assert.deepEqual(store.load('http://hub'), []);
    assert.equal(store.save('http://hub', ['a']), false);
    assert.equal(store.reset('http://hub'), false);
  }
  const noWrite = createProviderOrderStore({ setItem() {}, getItem: () => null, removeItem() {} });
  assert.equal(noWrite.save('http://hub', ['a']), false);
  const noDelete = createProviderOrderStore({ removeItem() {}, getItem: () => '["a"]' });
  assert.equal(noDelete.reset('http://hub'), false);
  const noReadback = createProviderOrderStore({ setItem() {}, removeItem() {}, getItem() { throw Error('denied'); } });
  assert.equal(noReadback.save('http://hub', ['a']), false);
  assert.equal(noReadback.reset('http://hub'), false);
});
