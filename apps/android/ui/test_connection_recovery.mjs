import test from 'node:test';
import assert from 'node:assert/strict';
import { createConnectionRecovery, networkRecovered } from './connection-recovery.mjs';

test('temporary failures retry with bounded backoff and success restores the selected interval', () => {
  const recovery = createConnectionRecovery();
  assert.equal(recovery.delay(60), 60_000);
  assert.equal(recovery.delay(300), 300_000);
  const actual = Array.from({ length: 8 }, () => { recovery.failed({ code: 'hub_timeout' }); return recovery.delay(60); });
  assert.deepEqual(actual, [15_000, 30_000, 60_000, 120_000, 300_000, 300_000, 300_000, 300_000]);
  recovery.reset(); assert.equal(recovery.delay(300), 300_000);
});
test('manual refresh mode never starts a periodic retry', () => {
  const recovery = createConnectionRecovery();
  for (const interval of [0, -1, undefined, NaN, '60']) assert.equal(recovery.delay(interval), null);
  recovery.failed({ code: 'hub_network_error' });
  assert.equal(recovery.delay(0), null);
  assert.equal(recovery.automaticAllowed, true); // Foreground/network recovery may still read once.
});
test('credential, redirect and response contract failures await an explicit retry or edited connection', () => {
  for (const error of [{ code: 'hub_unauthorized' }, { status: 401 }, { status: 403 },
    { code: 'upstream_redirect_blocked' }, { code: 'invalid_response' },
    { code: 'response_too_large' }, new SyntaxError('malformed JSON')]) {
    const recovery = createConnectionRecovery(); recovery.failed(error);
    assert.equal(recovery.automaticAllowed, false); assert.equal(recovery.delay(60), null);
    recovery.reset(); assert.equal(recovery.automaticAllowed, true);
    assert.equal(recovery.delay(60), 60_000);
  }
});
test('LAN return and Wi-Fi-to-Wi-Fi changes trigger recovery without requiring internet validation', () => {
  const wifi = { network: 'wifi', networkValidated: false, networkRevision: '3' };
  assert.equal(networkRecovered({ network: 'offline', networkRevision: '2' }, wifi), true);
  assert.equal(networkRecovered(wifi, { ...wifi, networkRevision: '4' }), true);
  assert.equal(networkRecovered(wifi, { ...wifi, networkValidated: true }), false);
  assert.equal(networkRecovered(wifi, { ...wifi }), false);
  assert.equal(networkRecovered(wifi, { network: 'offline', networkRevision: '4' }), false);
  assert.equal(networkRecovered(wifi, { network: 'unknown', networkRevision: '4' }), false);
  assert.equal(networkRecovered({ network: 'unknown' }, wifi), true);
  assert.equal(networkRecovered(wifi, { network: 'wifi' }), false); // Older bridge, no revision.
});
