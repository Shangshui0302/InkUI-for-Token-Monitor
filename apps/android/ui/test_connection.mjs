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
