import test from 'node:test';
import assert from 'node:assert/strict';
import { publicConnection, connectionPayload, sourceKey, createSourceFence } from './connection.mjs';
import { nativeConnection, saveNativeConnection, createTransport } from './transport.mjs';
test('native projection cannot expose extra credential fields', () => {
  const value = publicConnection({ hubUrl: 'http://hub:9000', hubSecret: 'secret', hasHubSecret: true, archiveSecret: 'other', revision: '2' });
  assert.deepEqual(Object.keys(value), ['hubUrl', 'hasHubSecret', 'archiveUrl', 'hasArchiveSecret', 'revision']);
  assert.equal(JSON.stringify(value).includes('secret'), false);
  assert.equal(nativeConnection({ getConnection: () => JSON.stringify(value) }).hasHubSecret, true);
});
test('untouched credentials preserve, explicit clear removes, typed replacement wins', () => {
  assert.deepEqual(connectionPayload(' http://hub ', '', false, '', '', false), { hubUrl: 'http://hub', hubSecret: null, archiveUrl: '', archiveSecret: null });
  assert.equal(connectionPayload('', 'new', false, '', '', false).hubSecret, 'new');
  assert.equal(connectionPayload('', 'new', true, '', '', true).hubSecret, '');
  let sent;
  saveNativeConnection({ saveConnection: json => { sent = JSON.parse(json); return '{"ok":true,"connection":{"revision":"3"}}'; } }, connectionPayload('', '', false, '', '', true));
  assert.equal(sent.hubSecret, null); assert.equal(sent.archiveSecret, '');
});
test('source switch excludes all older responses even after switching back', () => {
  const a = sourceKey(publicConnection({ hubUrl: 'http://a', revision: '1' }));
  const fence = createSourceFence(a), old = fence.capture();
  assert.equal(fence.change(a), false); assert.equal(fence.accepts(old), true);
  fence.change(sourceKey(publicConnection({ hubUrl: 'http://b', revision: '2' })));
  fence.change(a); assert.equal(fence.accepts(old), false);
  const current = fence.capture(); fence.invalidate(); assert.equal(fence.accepts(current), false);
});
test('APK UI requests carry no credentials or cookie authority', async () => {
  const requests = [];
  const fetcher = async (path, init) => { requests.push(init); return { ok: true, json: async () => ({}) }; };
  await createTransport({ fetcher }).getJSON('/api/stats');
  assert.equal(requests[0].headers, undefined);
  assert.equal(requests[0].credentials, 'omit');
});

test('native auth codes preserve independent Hub and archive failure identity', async () => {
  for (const code of ['hub_unauthorized', 'archive_unauthorized']) {
    const transport = createTransport({ fetcher: async () => ({ ok: false, status: 502, json: async () => ({ error: code }) }) });
    await assert.rejects(transport.getJSON(code.startsWith('archive') ? '/api/session-archive' : '/api/stats'), error => error.code === code && error.kind === '认证失败');
  }
});

// These cases protect the destination typed by the user, especially custom ports.
import { normalizeConnectionAddress, connectionTestMessage } from './connection.mjs';
import { createConnectionTester } from './transport.mjs';
test('bare Hub hosts get the documented port; explicit origins and archive hosts keep their ports', () => {
  for (const [raw, source, expected] of [
    ['192.168.1.22', 'hub', 'http://192.168.1.22:17321'],
    [' my-pc ', 'hub', 'http://my-pc:17321'],
    ['my-pc:9000', 'hub', 'http://my-pc:9000'],
    ['my-pc:80', 'hub', 'http://my-pc:80'],
    ['http://my-pc', 'hub', 'http://my-pc'],
    ['https://my-pc:443/', 'hub', 'https://my-pc:443'],
    ['my-pc', 'archive', 'http://my-pc'],
    ['[2001:db8::1]', 'hub', 'http://[2001:db8::1]:17321']
  ]) assert.equal(normalizeConnectionAddress(raw, source).value, expected, raw);
  assert.equal(normalizeConnectionAddress('http://host:80').origin, 'http://host');
  assert.equal(normalizeConnectionAddress('https://host:443').origin, 'https://host');
  assert.equal(normalizeConnectionAddress('[2001:db8:0::1]:9000').origin, 'http://[2001:db8:0::1]:9000');
  assert.equal(normalizeConnectionAddress('').value, '');
  assert.equal(normalizeConnectionAddress('localhost').loopback, true);
  assert.equal(normalizeConnectionAddress('127.0.0.1:17321').loopback, true);
  assert.equal(normalizeConnectionAddress('[::1]').loopback, true);
  assert.equal(normalizeConnectionAddress('192.168.1.22').loopback, false);
});
test('invalid or path-bearing addresses produce actionable errors without disclosing credentials', () => {
  for (const raw of ['my-pc:0', 'my-pc:65536', 'my-pc:', 'http://user:private@my-pc', 'my-pc/stats', 'my-pc/../', 'my-pc?token=private', 'my-pc#private', 'ftp://my-pc', 'my pc', 'my-pc\\evil']) {
    const result = normalizeConnectionAddress(raw);
    assert.ok(result.error, raw); assert.equal(result.value, '');
    assert.equal(JSON.stringify(result).includes('private'), false);
  }
});
function testerHarness() {
  const calls = [], results = [], timers = new Map(); let timerId = 0, cancelled = 0;
  const bridge = { testConnection: (payload, id) => calls.push({ payload: JSON.parse(payload), id }), cancelConnectionTest: () => cancelled++ };
  const tester = createConnectionTester(bridge, value => results.push(value), {
    setTimer(callback) { timers.set(++timerId, callback); return timerId; },
    clearTimer(id) { timers.delete(id); }
  });
  return { tester, calls, results, timers, cancelled: () => cancelled };
}
test('test bridge receives transient inputs without saving and rejects duplicate submissions', () => {
  const h = testerHarness(), payload = { ...connectionPayload('http://hub', 'temporary', false, '', '', false), source: 'hub' };
  assert.equal(h.tester.start(payload, 'http://hub'), true);
  assert.equal(h.tester.start(payload, 'http://hub'), false);
  assert.equal(h.calls.length, 1); assert.equal(h.calls[0].payload.hubSecret, 'temporary');
  assert.equal(h.tester.pending, true);
  assert.equal(h.tester.accept({ id: h.calls[0].id, source: 'hub', origin: 'http://hub', ok: true, latencyMs: 12, body: 'private', secret: 'private' }), true);
  assert.equal(h.tester.pending, false); assert.equal(h.timers.size, 0);
  assert.equal(JSON.stringify(h.results).includes('private'), false);
});
test('editing, switching source or leaving cancels a test and excludes delayed replies', () => {
  const h = testerHarness(), payload = { hubUrl: 'http://hub', source: 'hub' };
  h.tester.start(payload, 'http://hub'); const oldId = h.calls[0].id;
  h.tester.cancel(); assert.equal(h.cancelled(), 1);
  h.tester.start(payload, 'http://hub'); const id = h.calls[1].id;
  assert.notEqual(oldId, id);
  assert.equal(h.tester.accept({ id: oldId, source: 'hub', origin: 'http://hub', ok: true }), false);
  assert.equal(h.tester.accept({ id, source: 'archive', origin: 'http://hub', ok: true }), false);
  assert.equal(h.tester.accept({ id, source: 'hub', origin: 'http://other', ok: true }), false);
  assert.equal(h.results.length, 0);
  assert.equal(h.tester.accept({ id, source: 'hub', origin: 'http://hub', ok: true }), true);
});
test('native validation failures with no valid origin remain actionable and clear the pending request', () => {
  const h = testerHarness(); h.tester.start({ source: 'hub' }, 'http://hub');
  const id = h.calls[0].id;
  assert.equal(h.tester.accept({ id, source: 'hub', origin: '', ok: false, error: 'invalid_connection' }), true);
  assert.equal(h.tester.pending, false); assert.equal(h.results[0].error, 'invalid_connection');
});
test('timeout cancels native request and an older bridge never falls back to saving', () => {
  const h = testerHarness(); h.tester.start({ source: 'hub' }, 'http://hub');
  [...h.timers.values()][0]();
  assert.equal(h.tester.pending, false); assert.equal(h.cancelled(), 1);
  assert.equal(h.results[0].error, 'timeout');
  let saved = false;
  const old = createConnectionTester({ saveConnection: () => { saved = true; } }, () => assert.fail('unexpected callback'));
  assert.equal(old.start({ source: 'hub' }, 'http://hub'), false);
  assert.equal(old.pending, false); assert.equal(saved, false);
});
test('test result copy distinguishes successful unsaved input and actionable failures', () => {
  assert.match(connectionTestMessage({ ok: true, source: 'hub' }), /测试不会保存/);
  assert.match(connectionTestMessage({ error: 'unauthorized' }), /重新输入密钥/);
  assert.match(connectionTestMessage({ error: 'host_not_found' }), /局域网 IP/);
  assert.match(connectionTestMessage({ error: 'connection_refused' }), /服务已启动/);
  assert.equal(connectionTestMessage({ error: 'private-data' }).includes('private-data'), false);
});
