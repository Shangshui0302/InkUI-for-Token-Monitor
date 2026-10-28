import test from 'node:test';
import assert from 'node:assert/strict';
import { dashboardPageInput, dashboardRenderSignature, dashboardViewClock, resetCountdown } from './dashboard-render.mjs';
import { dashboardView } from './dashboard-model.mjs';

const now = new Date(2026, 9, 28, 12, 0, 15).getTime();
const at = milliseconds => new Date(now + milliseconds).toISOString();
const state = extra => ({ page: 'limits', period: 'today', detailKind: 'usage', providerKey: null,
  providerPicker: false, width: 600, height: 800, index: 0, ...extra });
const stats = () => ({ updatedAt: at(0), limits: { updatedAt: at(0), refreshMs: 1000,
  providers: [{ provider: 'codex', status: 'ok', windows: [{ remainingPercent: 50, resetsAt: at(90_000) }] }] },
  periods: { today: { totalTokens: 100, costUsd: 1, sessions: {}, models: {}, projects: {} },
    month: { sessions: {} }, allTime: { sessions: {} } }, devices: [] });
const signature = (raw, selected = state(), time = now, history = null) => dashboardRenderSignature(raw, history, selected, time);

test('quota ignores polling timestamps, history and usage for another page', () => {
  const raw = stats(), original = signature(raw);
  raw.updatedAt = at(1000); raw.limits.updatedAt = at(1000); raw.limits.refreshMs = 2000;
  raw.periods.today.totalTokens++; raw.devices.push({ hostname: 'another page' });
  assert.equal(signature(raw), original);
  assert.equal(signature(raw, state(), now, { daily: [], monthly: [], summary: {} }), original);
  assert.equal(signature(structuredClone(raw)), original, 'response identity alone does not rebuild');
});

test('quota includes detail-only changes even when the card value is unchanged', () => {
  for (const change of [raw => { raw.limits.providers[0].updatedAt = at(1); },
    raw => { raw.limits.providers[0].windows[0].detail = 'new explanation'; },
    raw => { raw.limits.providers[0].accountEmail = 'new account'; }]) {
    const raw = stats(), original = signature(raw); change(raw);
    assert.notEqual(signature(raw), original);
  }
});

test('countdowns invalidate at the real second boundary within the same minute', () => {
  const raw = stats(); raw.limits.providers[0].windows[0].resetsAt = at(10_000);
  assert.equal(signature(raw, state(), now + 9_999), signature(raw));
  assert.notEqual(signature(raw, state(), now + 10_000), signature(raw));
  assert.equal(resetCountdown(at(10_000), now + 9_999), '1分后');
  assert.equal(resetCountdown(at(10_000), now + 10_000), '待重置');
  raw.limits.providers[0].balance = { amount: 1, expiresAt: at(10_000) };
  raw.limits.providers[0].resetCredits = { availableCount: 1, nextExpiresAt: at(10_000) };
  assert.notEqual(signature(raw, state(), now + 10_000), signature(raw));
  assert.equal(signature(raw, state({ providerPicker: true }), now + 10_000), signature(raw, state({ providerPicker: true })));
});

test('long countdowns use the displayed hour/day boundary rather than every minute', () => {
  const raw = stats(); raw.limits.providers[0].windows[0].resetsAt = at(2 * 86400000 + 3599000);
  assert.equal(resetCountdown(raw.limits.providers[0].windows[0].resetsAt, now), '2天1时后');
  assert.equal(signature(raw, state(), now + 30_000), signature(raw));
  assert.notEqual(signature(raw, state(), now + 59_000), signature(raw));
});

test('session and usage status expire strictly after ten minutes, including malformed booleans', () => {
  const raw = stats(); raw.periods.today.sessions.s = { lastUsedAt: at(-590_000), title: 'session', archived: 'false', turnEnded: 'false' };
  for (const selected of [state({ page: 'sessions' }), state({ page: 'details', detailKind: 'usage' })]) {
    assert.equal(signature(raw, selected, now + 10_000), signature(raw, selected));
    assert.notEqual(signature(raw, selected, now + 10_001), signature(raw, selected));
  }
  assert.equal(dashboardView(raw, null, 'today', now + 10_000).runningSessions.length, 1);
  assert.equal(dashboardView(raw, null, 'today', now + 10_001).runningSessions.length, 0);
  assert.notEqual(dashboardViewClock(raw, now + 10_000), dashboardViewClock(raw, now + 10_001));
  const selected = state({ page: 'sessions' }), original = signature(raw, selected);
  raw.periods.today.sessions.s.messageCount = 99;
  assert.notEqual(signature(raw, selected), original, 'session details refresh even if the summary card is unchanged');
});

test('device expiry and local midnight invalidate at their actual boundary', () => {
  const raw = stats(), selected = state({ page: 'details', detailKind: 'devices' });
  raw.staleAfterMs = 60_000; raw.devices.push({ receivedAt: at(-50_000), periods: {} });
  assert.equal(signature(raw, selected, now + 10_000), signature(raw, selected));
  assert.notEqual(signature(raw, selected, now + 10_001), signature(raw, selected));
  const midnight = new Date(2026, 9, 29).getTime(), activity = state({ page: 'activity' });
  assert.notEqual(signature(raw, activity, midnight), signature(raw, activity, midnight - 1));
  assert.notEqual(dashboardViewClock(raw, midnight), dashboardViewClock(raw, midnight - 1));
});

test('activity ignores within-day polling time but tracks today override eligibility and detail fields', () => {
  const raw = stats(), activity = state({ page: 'activity' });
  const history = { daily: [{ date: '2026-10-27', tokens: 1 }], monthly: [], summary: {} };
  const original = signature(raw, activity, now, history);
  raw.updatedAt = at(1000);
  assert.equal(signature(raw, activity, now, history), original);
  raw.updatedAt = new Date(2026, 9, 27, 12).toISOString();
  assert.notEqual(signature(raw, activity, now, history), original);
  history.daily[0].messages = 55;
  assert.notEqual(signature(raw, activity, now, history), signature(raw, activity, now, { ...history, daily: [{ date: '2026-10-27', tokens: 1 }] }));
});

test('navigation, page position, geometry, appearance, order draft and drill remain inputs', () => {
  const raw = stats(), original = signature(raw);
  for (const extra of [{ index: 1 }, { width: 601 }, { height: 799 }, { theme: 'color' }, { accent: 'green' },
    { providerKey: 0 }, { providerPicker: true }, { savedOrder: ['new order'] }, { period: 'month' },
    { orderEditor: { rows: [], message: 'unsaved' } }, { drill: { title: 'detail', pairs: [['field', 'value']] } }]) {
    assert.notEqual(signature(raw, state(extra)), original, JSON.stringify(extra));
  }
  const drill = { title: 'detail', pairs: [['field', 'value']] };
  assert.deepEqual(dashboardPageInput(raw, null, state({ drill })), drill);
});
