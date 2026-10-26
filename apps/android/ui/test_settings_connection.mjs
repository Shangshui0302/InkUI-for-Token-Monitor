import test from 'node:test';
import assert from 'node:assert/strict';
import { publicConnection } from './connection.mjs';
import { createSettingsConnectionFlow, settingsConnectionPayload } from './settings-connection.mjs';

const initial = () => publicConnection({ hubUrl: 'http://old-hub:17321', hasHubSecret: true, revision: '1' });
const draft = (extra = {}) => ({ source: 'hub', url: 'new-hub', secret: '', removeSecret: false, ...extra });
function harness(options = {}) {
  let config = initial(), cancelled = 0, timerId = 0;
  const requests = [], saves = [], states = [], timers = new Map();
  const bridge = {
    testConnection(json, id) { requests.push({ payload: JSON.parse(json), id }); },
    cancelConnectionTest() { cancelled++; }
  };
  const flow = createSettingsConnectionFlow({
    bridge, getConfig: () => config,
    save(payload) {
      if (options.saveError) throw new Error('synthetic-sensitive-error');
      saves.push(payload);
      config = publicConnection({ ...config, ...payload, revision: String(Number(config.revision) + 1) });
      return config;
    },
    onState: value => states.push(value),
    testOptions: {
      setTimer(callback) { timers.set(++timerId, callback); return timerId; },
      clearTimer(id) { timers.delete(id); }
    }
  });
  const reply = (detail = {}) => {
    const request = requests.at(-1);
    return flow.accept({ id: request.id, source: request.payload.source, origin: request.payload.hubUrl, ok: true, ...detail });
  };
  return { flow, requests, saves, states, timers, reply, cancelled: () => cancelled, setConfig(value) { config = value; } };
}

test('candidate contains only Hub fields and leaves secret reuse to native policy', () => {
  const hub = settingsConnectionPayload(draft());
  assert.deepEqual(hub, { payload: { hubUrl: 'http://new-hub:17321', hubSecret: null }, source: 'hub', origin: 'http://new-hub:17321' });
  assert.equal(settingsConnectionPayload(draft({ secret: 'ignored', removeSecret: true })).payload.hubSecret, '');
  assert.equal(settingsConnectionPayload(draft({ secret: 'ignored\n', removeSecret: true })).payload.hubSecret, '');
});

test('invalid addresses, missing Hub and invalid secrets never test or save', () => {
  for (const input of [draft({ url: '' }), draft({ url: 'http://user:sensitive@host' }), draft({ url: 'host/path' }), draft({ secret: 'sensitive\n' }), draft({ source: 'other' })]) {
    const h = harness();
    assert.equal(h.flow.submit(input), false);
    assert.equal(h.requests.length, 0); assert.equal(h.saves.length, 0);
    assert.equal(h.flow.state.phase, 'error');
    assert.equal(JSON.stringify(h.states).includes('sensitive'), false);
  }
});

test('successful test saves exactly once, refuses duplicate submit and exposes only safe state', () => {
  const h = harness();
  assert.equal(h.flow.submit(draft({ secret: 'synthetic-secret' })), true);
  assert.equal(h.flow.pending, true);
  assert.equal(h.flow.submit(draft()), false);
  assert.equal(h.requests.length, 1); assert.equal(h.saves.length, 0);
  assert.equal(h.requests[0].payload.hubSecret, 'synthetic-secret');
  assert.equal(h.reply({ secret: 'synthetic-secret', body: 'synthetic-secret' }), true);
  assert.equal(h.flow.pending, false); assert.equal(h.saves.length, 1);
  assert.equal(h.reply(), false); assert.equal(h.flow.saveOffline(), false);
  assert.equal(h.saves.length, 1); assert.equal(h.flow.state.message, '已保存并连接');
  assert.equal(h.flow.state.verified, true);
  assert.equal(JSON.stringify(h.states).includes('synthetic-secret'), false);
  assert.ok(h.states.every(value => !('payload' in value) && !('origin' in value)));
  assert.ok(Object.isFrozen(h.flow.state));
});

test('only network failures allow one explicit offline save', () => {
  for (const error of ['timeout', 'host_not_found', 'connection_refused', 'network_error']) {
    const h = harness(); h.flow.submit(draft());
    h.reply({ ok: false, error });
    assert.equal(h.saves.length, 0); assert.equal(h.flow.state.canSaveOffline, true);
    assert.equal(h.flow.saveOffline(), true); assert.equal(h.saves.length, 1);
    assert.equal(h.flow.state.message, '已保存，待连接');
    assert.equal(h.flow.state.verified, false);
    assert.equal(h.flow.saveOffline(), false); assert.equal(h.saves.length, 1);
  }
});

test('authentication, API and format failures cannot bypass testing; arbitrary native errors are hidden', () => {
  for (const error of ['unauthorized', 'invalid_connection', 'invalid_response', 'http_error', 'redirect_blocked', 'cancelled', 'synthetic-sensitive-error']) {
    const h = harness(); h.flow.submit(draft()); h.reply({ ok: false, error });
    assert.equal(h.flow.state.canSaveOffline, false); assert.equal(h.flow.saveOffline(), false);
    assert.equal(h.saves.length, 0);
    assert.equal(JSON.stringify(h.states).includes('synthetic-sensitive-error'), false);
  }
});

test('editing or hiding cancellation excludes old completion and discards offline candidate', () => {
  for (const reason of ['edit', 'hide']) {
    const h = harness(); h.flow.submit(draft()); const old = h.requests[0];
    h.flow.cancel(); assert.equal(h.cancelled(), 1); assert.equal(h.flow.pending, false);
    assert.equal(h.flow.accept({ id: old.id, source: 'hub', origin: 'http://new-hub:17321', ok: true }), false);
    assert.equal(h.saves.length, 0, reason); assert.equal(h.flow.state.phase, 'idle');
    h.flow.submit(draft()); h.reply({ ok: false, error: 'network_error' });
    h.flow.cancel(); assert.equal(h.flow.saveOffline(), false); assert.equal(h.saves.length, 0);
  }
});

test('timeouts use existing tester and late results cannot save', () => {
  const h = harness(); h.flow.submit(draft());
  [...h.timers.values()][0]();
  assert.equal(h.cancelled(), 1); assert.equal(h.flow.state.error, 'timeout');
  assert.equal(h.flow.state.canSaveOffline, true); assert.equal(h.reply(), false);
  assert.equal(h.saves.length, 0); assert.equal(h.flow.saveOffline(), true);
});

test('save exceptions retain an actionable safe error with no offline bypass', () => {
  const h = harness({ saveError: true }); h.flow.submit(draft()); h.reply();
  assert.equal(h.flow.state.phase, 'error'); assert.equal(h.flow.state.canSaveOffline, false);
  assert.equal(h.flow.saveOffline(), false); assert.equal(h.saves.length, 0);
  assert.equal(JSON.stringify(h.states).includes('synthetic-sensitive-error'), false);
  assert.equal(h.flow.submit(draft()), true);
});

test('changed saved source or revision prevents both successful and offline commits', () => {
  for (const change of [{ hubUrl: 'http://other' }, { revision: '2' }]) {
    for (const offline of [true, false]) {
      const h = harness(); h.flow.submit(draft());
      h.setConfig(publicConnection({ ...initial(), ...change }));
      h.reply(offline ? { ok: false, error: 'network_error' } : {});
      if (offline) assert.equal(h.flow.saveOffline(), false);
      assert.equal(h.saves.length, 0); assert.equal(h.flow.state.error, 'connection_changed');
      assert.equal(h.flow.state.canSaveOffline, false);
    }
  }
});

test('missing native test capability cannot save and synchronous bridge failure remains explicit', () => {
  let writes = 0;
  const options = { getConfig: initial, save() { writes++; } };
  const old = createSettingsConnectionFlow({ ...options, bridge: {} });
  assert.equal(old.submit(draft()), false); assert.equal(old.saveOffline(), false);
  const broken = createSettingsConnectionFlow({ ...options, bridge: { testConnection() { throw new Error('sensitive'); } } });
  assert.equal(broken.submit(draft()), true); assert.equal(broken.state.error, 'network_error');
  assert.equal(writes, 0); assert.equal(broken.saveOffline(), true); assert.equal(writes, 1);
});
