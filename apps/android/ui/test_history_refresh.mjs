import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import * as connection from './connection.mjs';
import * as transport from './transport.mjs';
import * as model from './dashboard-model.mjs';
import * as theme from './theme.mjs';
import * as device from './device-status.mjs';
import * as recovery from './connection-recovery.mjs';
import { createHistoryRecovery, historyRevisionMatches } from './history-recovery.mjs';

// Execute the actual application orchestration with an inert DOM/dashboard and
// synthetic transport. No browser, native bridge, network or persistent state.
const source = readFileSync(new URL('./app.js', import.meta.url), 'utf8').replace(/^import .*;\n/gm, '');
const stats = revision => ({ periods: {}, limits: { providers: [] }, historyRevision: revision });
const history = () => ({ daily: [], monthly: [], summary: {} });
const ok = body => ({ ok: true, status: 200, json: async () => body });
const failure = code => ({ ok: false, status: 503, json: async () => ({ error: code }) });
const drain = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

async function harness({ cached = false, interval = '60', revision = 'r1', respond } = {}) {
  let now = Date.parse('2026-10-28T01:00:00Z'), serial = 0, settings;
  const timers = new Map(), elements = new Map(), events = new Map(), values = new Map();
  const calls = [], updates = [];
  let config = { hubUrl: 'http://synthetic.invalid', hasHubSecret: true, revision: '1', storageState: 'ready' };
  const node = id => {
    if (!elements.has(id)) elements.set(id, {
      value: id === 'interval' ? interval : id === 'period' ? 'today' : '',
      hidden: id === 'settings', textContent: '', dataset: {}, listeners: {}, style: { setProperty() {} },
      setAttribute() {}, appendChild() {}, replaceChildren() {}, closest: () => null, getClientRects: () => [],
      addEventListener(name, fn) { this.listeners[name] = fn; },
      dispatchEvent(event) { this.listeners[event.type]?.(event); }, click() { this.listeners.click?.(); }
    });
    return elements.get(id);
  };
  const setTimer = (fn, delay) => { const id = ++serial; timers.set(id, { fn, at: now + delay }); return id; };
  const clearTimer = id => timers.delete(id);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
  values.set('token-monitor-eink-native-source-v1', connection.sourceKey(config));
  values.set('token-monitor-eink-interval-v1', interval);
  const cachedAt = '2026-10-27T01:00:00Z';
  if (cached) for (const [key, data] of [
    ['token-monitor-eink-snapshot-v1', { stats: stats(revision) }],
    ['token-monitor-eink-history-v1', { history: history(), revision }]
  ]) values.set(key, JSON.stringify({ ...data, receivedAt: cachedAt, _connectionSource: connection.sourceKey(config) }));
  const bridge = {
    getConnection: () => JSON.stringify(config), getDeviceStatus: () => '{"network":"wifi","networkRevision":"1"}',
    saveConnection(payload) {
      config = { ...config, hubUrl: JSON.parse(payload).hubUrl, revision: String(Number(config.revision) + 1) };
      return JSON.stringify({ ok: true, connection: config });
    }
  };
  const window = { TokenMonitorAndroid: bridge, localStorage: storage,
    addEventListener: (name, fn) => events.set(name, fn) };
  const sandbox = {
    ...connection, ...transport, ...model, ...theme, ...device, ...recovery, historyRevisionMatches,
    createHistoryRecovery: () => createHistoryRecovery(() => now),
    createTransport: options => transport.createTransport({ ...options, setTimer, clearTimer,
      fetcher: async path => {
        calls.push({ path, at: now, origin: config.hubUrl });
        return respond ? respond(path, calls) : ok(path === '/api/stats' ? stats(revision) : history());
      } }),
    createDashboard: () => ({ update: (stats, history, context) => updates.push({ stats, history, ...context }), setOrderSource() {}, clear() {} }),
    createSettings: options => { settings = options; return { update() {}, resize() {}, open() {}, cancelTests() {} }; },
    createDisplayRefresh: () => ({ request() {}, cancel() {} }), installPageSwipe: () => () => {}, icon: () => node('icon'),
    window, localStorage: storage, innerHeight: 800,
    document: { hidden: false, documentElement: node('html'), getElementById: node,
      querySelectorAll: () => [], querySelector: node, createElement: node, createTextNode: node, createElementNS: (_, name) => node(name),
      addEventListener: (name, fn) => events.set(name, fn) },
    setTimeout: setTimer, clearTimeout: clearTimer,
    Date: class extends Date { constructor(...args) { super(...(args.length ? args : [now])); } static now() { return now; } },
    SyntaxError, Event
  };
  vm.runInNewContext(source, sandbox, { filename: 'app.js' });
  await drain();
  return {
    calls, updates, values, cachedAt, state: () => updates.at(-1), label: () => node('connection').textContent,
    async advance(ms) {
      const end = now + ms;
      for (;;) {
        const next = [...timers].filter(([, item]) => item.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) break;
        now = next[1].at; timers.delete(next[0]); next[1].fn(); await drain();
      }
      now = end; await drain();
    },
    async retry() { node('refresh').click(); await drain(); },
    async save(hubUrl) { settings.saveConnection({ hubUrl, hubSecret: 'synthetic-only' }); await drain(); },
    async event(name, detail) { events.get(name)?.({ detail }); await drain(); },
    async clear() { settings.clearCache(); await drain(); }
  };
}
const historyCalls = h => h.calls.filter(call => call.path === '/api/history');

test('a current revision verifies cached history without changing its original read time', async () => {
  const h = await harness({ cached: true });
  assert.deepEqual(h.calls.map(c => c.path), ['/api/stats']);
  assert.equal(h.state().historyCached, false);
  assert.equal(h.state().historyReceivedAt, h.cachedAt);
  assert.equal(h.label(), '在线');
});

test('stats failure and recovery revalidate matching retained history', async () => {
  const h = await harness({ cached: true, respond: (path, calls) => calls.length === 1
    ? failure('hub_network_error') : ok(stats('r1')) });
  assert.equal(h.state().historyCached, true);
  await h.advance(15_000);
  assert.equal(h.state().historyCached, false);
  assert.equal(h.state().historyReceivedAt, h.cachedAt);
  assert.equal(historyCalls(h).length, 0);
});

test('history authentication and contract errors pause only history until explicit retry', async () => {
  for (const code of ['hub_unauthorized', 'upstream_redirect_blocked', 'invalid_response', 'response_too_large', 'bad-schema']) {
    let broken = true;
    const h = await harness({ respond: path => path === '/api/stats' ? ok(stats('r1'))
      : broken ? code === 'bad-schema' ? ok({ invalid: true }) : failure(code) : ok(history()) });
    await h.advance(180_000);
    assert.equal(h.calls.filter(c => c.path === '/api/stats').length, 4, code);
    assert.equal(historyCalls(h).length, 1, code);
    assert.equal(h.state().historyCached, true, code);
    assert.ok(h.state().historyError, code);
    assert.equal(h.label(), '部分缓存', code);
    broken = false; await h.retry();
    assert.equal(historyCalls(h).length, 2, code);
    assert.equal(h.state().historyCached, false, code);
    assert.equal(h.state().historyError, '', code);
  }
});

test('history backoff does not slow stats or add a polling timer', async () => {
  const h = await harness({ respond: path => path === '/api/stats' ? ok(stats('r1')) : failure('hub_timeout') });
  const start = h.calls[0].at;
  await h.advance(600_000);
  assert.equal(h.calls.filter(c => c.path === '/api/stats').length, 11);
  assert.deepEqual(historyCalls(h).map(c => c.at - start), [0, 60_000, 120_000, 180_000, 300_000, 600_000]);
});

test('manual mode has no background retry and network events do not unlock permanent history errors', async () => {
  const h = await harness({ interval: '0', respond: path => path === '/api/stats' ? ok(stats('r1')) : failure('hub_unauthorized') });
  await h.advance(600_000);
  assert.equal(h.calls.length, 2);
  await h.event('tokenmonitor:device-status', { network: 'wifi', networkRevision: '2' });
  await h.advance(1000);
  assert.equal(h.calls.length, 3);
  assert.equal(historyCalls(h).length, 1);
});

test('older Hubs without history revisions still fetch history on each successful refresh', async () => {
  for (const revision of [null, '']) {
    const h = await harness({ cached: true, revision });
    await h.advance(60_000);
    assert.equal(historyCalls(h).length, 2);
    assert.equal(h.state().historyCached, false);
  }
});

test('changing the connection clears the old history error and its blocked retry state', async () => {
  const h = await harness({ respond: (path, calls) => path === '/api/stats' ? ok(stats('r1'))
    : calls.at(-1).origin === 'http://synthetic.invalid' ? failure('hub_unauthorized') : ok(history()) });
  await h.save('http://other-synthetic.invalid');
  assert.equal(historyCalls(h).length, 2);
  assert.equal(h.state().historyCached, false);
  assert.equal(h.state().historyError, '');
});

test('a cancelled old history response cannot repopulate cleared cache or block the next read', async () => {
  let finish;
  const h = await harness({ respond: (path, calls) => path === '/api/stats' ? ok(stats('r1'))
    : calls.filter(c => c.path === '/api/history').length === 1 ? new Promise(resolve => { finish = resolve; }) : ok(history()) });
  await h.clear();
  finish(failure('hub_unauthorized')); await drain();
  assert.equal(h.values.has('token-monitor-eink-history-v1'), false);
  await h.retry();
  assert.equal(h.state().historyCached, false);
  assert.equal(h.state().historyError, '');
});
