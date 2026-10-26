import test from 'node:test';
import assert from 'node:assert/strict';
import { createTransport, reloadNativeConnection } from './transport.mjs';
const origin = 'https://appassets.androidplatform.net';
function harness(options = {}) {
  const requests = [], cancelled = [], timers = new Map(); let timerId = 0;
  const transport = createTransport({ origin, bridge: { cancelDataRequest: id => cancelled.push(id) },
    setTimer: (callback, delay) => { timers.set(++timerId, { callback, delay }); return timerId; },
    clearTimer: id => timers.delete(id), fetcher: (path, init) => new Promise((resolve, reject) => {
      requests.push({ path, init, resolve, reject });
      init.signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
    }), ...options });
  return { transport, requests, cancelled, timers };
}
test('packaged data reads carry unique public native IDs and cancel both active sockets immediately', async () => {
  const h = harness(), a = h.transport.getJSON('/api/stats'), b = h.transport.getJSON('/api/history');
  const ids = h.requests.map(r => r.init.headers['X-InkUI-Request-Id']);
  assert.notEqual(ids[0], ids[1]); ids.forEach(id => assert.match(id, /^read-[A-Za-z0-9_-]{1,59}$/));
  assert.equal([...h.timers.values()][0].delay, 10000);
  h.transport.cancel(); assert.deepEqual(h.cancelled, ids);
  await assert.rejects(a, { name: 'AbortError' }); await assert.rejects(b, { name: 'AbortError' });
  assert.equal(h.timers.size, 0); h.transport.cancel(); assert.deepEqual(h.cancelled, ids);
});
test('timeout immediately cancels its native request and clears the pending timer', async () => {
  const h = harness(), pending = h.transport.getJSON('/api/stats');
  [...h.timers.values()][0].callback();
  assert.deepEqual(h.cancelled, [h.requests[0].init.headers['X-InkUI-Request-Id']]);
  await assert.rejects(pending, { name: 'AbortError' }); assert.equal(h.timers.size, 0);
});
test('late fetch completion cannot resurrect a cancelled read even with a fetch mock ignoring AbortSignal', async () => {
  let resolve, request;
  const h = harness({ fetcher: (path, options) => { request = options; return new Promise(done => { resolve = done; }); } });
  const pending = h.transport.getJSON('/api/stats'); h.transport.cancel();
  assert.equal(request.signal.aborted, true);
  resolve({ ok: true, json: async () => ({ private: 'old' }) });
  await assert.rejects(pending, { name: 'AbortError' });
});
test('browser mocks and other origins never receive native request headers', async () => {
  for (const options of [{}, { bridge: {}, origin: 'http://localhost:8000' }, { origin }]) {
    let request;
    const transport = createTransport({ ...options, fetcher: async (path, init) => { request = init; return { ok: true, json: async () => ({}) }; } });
    await transport.getJSON('/api/stats'); assert.equal(request.headers, undefined); assert.equal(request.credentials, 'omit');
  }
});
test('successful reads release IDs without native cancellation; reloading returns only safe public config', async () => {
  const h = harness(); const pending = h.transport.getJSON('/api/stats');
  h.requests[0].resolve({ ok: true, json: async () => ({ ok: true }) }); await pending;
  h.transport.cancel(); assert.deepEqual(h.cancelled, []);
  const config = reloadNativeConnection({ reloadConnection: () => JSON.stringify({ ok: false, connection: { storageState: 'key_unavailable', hubSecret: 'private' } }) });
  assert.equal(config.storageState, 'key_unavailable'); assert.equal(JSON.stringify(config).includes('private'), false);
  assert.throws(() => reloadNativeConnection({ reloadConnection: () => '{}' }));
});
