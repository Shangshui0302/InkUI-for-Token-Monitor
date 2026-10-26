import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeDeviceStatus, networkLabel } from './device-status.mjs';

test('device status keeps real zero and unknown distinct and projects only display fields', () => {
  const value = normalizeDeviceStatus({ fullscreen:true,time:'00:00',batteryPercent:0,charging:false,network:'wifi',networkValidated:false,ssid:'private',secret:'private' });
  assert.deepEqual(value, { fullscreen:true,time:'00:00',batteryPercent:0,charging:false,network:'wifi',networkValidated:false,networkRevision:'' });
  assert.equal(normalizeDeviceStatus({}).batteryPercent, null);
  for (const batteryPercent of [NaN,Infinity,-1,101,'50']) assert.equal(normalizeDeviceStatus({batteryPercent}).batteryPercent,null);
});
test('network changes expose only an opaque counter, including Wi-Fi-to-Wi-Fi changes', () => {
  assert.equal(normalizeDeviceStatus({ networkRevision:'42', ssid:'private', ip:'private' }).networkRevision, '42');
  for (const networkRevision of [42, -1, 'ssid', '10.0.0.1', '1'.repeat(21)])
    assert.equal(normalizeDeviceStatus({ networkRevision }).networkRevision, '');
});
test('invalid clock and link states never look like an observed disconnection', () => {
  for (const time of ['24:00','12:60','1:23',null]) assert.equal(normalizeDeviceStatus({time}).time,'—:—');
  assert.equal(normalizeDeviceStatus({network:'unexpected'}).network,'unknown');
  assert.equal(normalizeDeviceStatus({network:'offline'}).network,'offline');
  assert.match(networkLabel(normalizeDeviceStatus({network:'wifi',networkValidated:false})),/局域网仍可能可用/);
  assert.equal(networkLabel(normalizeDeviceStatus({network:'offline',networkValidated:false})),'设备未联网');
});
