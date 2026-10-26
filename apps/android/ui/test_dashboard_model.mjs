import test from 'node:test';
import assert from 'node:assert/strict';
import { dashboardView, projectDashboardStats, projectHistoryForCache } from './dashboard-model.mjs';

const now = new Date(2026, 9, 23, 12).getTime();
const time = delta => new Date(now - delta).toISOString();
const base = () => ({
  updatedAt: time(0),
  periods: {
    today: { totalTokens: 100, costUsd: 2, outputTokens: 0, models: { a: 60 }, modelCosts: { a: 1 }, clients: { codex: 100 }, clientCosts: { codex: 2 }, sessions: {}, projects: {} },
    month: { totalTokens: 1000, costUsd: 20, models: { a: 1000 }, sessions: {}, projects: {} },
    allTime: { totalTokens: 9000, costUsd: 200, sessions: {}, projects: {} }
  },
  devices: [{ hostname: 'laptop', platform: 'linux', updatedAt: time(0), periods: { today: { totalTokens: 100, costUsd: 2 } } }]
});
const history = () => ({ daily: [{ date: '2026-10-01', tokens: 50, cost: 1 }], monthly: [{ month: '2026-10', tokens: 50, cost: 1 }], summary: { totalTokens: 9000, peakDayTokens: 40, activeDays: 500 } });

test('periods use aggregate totals, distributions use the whole total, missing stays distinct from zero', () => {
  const raw = base();
  raw.devices.push(raw.devices[0]);
  const today = dashboardView(raw, null, 'today', now);
  assert.equal(today.summary.totalTokens, 100);
  assert.equal(today.summary.inputTokens, null);
  assert.equal(today.summary.outputTokens, 0);
  assert.deepEqual(today.models, [{ name: 'a', tokens: 60, share: .6, costUsd: 1 }, { name: '未分类', tokens: 40, share: .4, costUsd: 1 }]);
  assert.equal(dashboardView(raw, null, 'month', now).summary.totalTokens, 1000);
  const all = dashboardView(raw, null, 'allTime', now);
  assert.equal(all.summary.totalTokens, 9000);
  assert.equal(all.sessionsUnavailable, true);
  assert.deepEqual(all.sessions, []);
});

test('live status, archive flags, exact ten minute boundary, and context agree across periods', () => {
  const raw = base();
  const session = extra => ({ title: 'display title', projectLabel: 'project', client: 'codex', models: { a: 1 }, lastUsedAt: time(1), contextTokens: 101, contextWindow: 200, ...extra });
  raw.periods.month.sessions = {
    running: session({ lastUsedAt: time(600_000) }),
    ended: session({ turnEnded: true }),
    quiet: session({ lastUsedAt: time(600_001) }),
    archived: session({ archived: true }), deleted: session({ deleted: true }), sourceDeleted: session({ sourceDeleted: true }),
    background: session({ sessionKind: 'background-review' }), invalid: session({ lastUsedAt: 'bad' })
  };
  raw.periods.today.sessions = { running: session({ totalTokens: 999, turnEnded: true }), extra: session({ contextTokens: 0 }) };
  const view = dashboardView(raw, null, 'allTime', now);
  assert.deepEqual(view.runningSessions.map(s => s.key).sort(), ['extra', 'running']);
  assert.equal(view.runningSessions.find(s => s.key === 'running').contextPercent, 50);
  assert.equal(view.runningSessions.find(s => s.key === 'extra').contextPercent, null);
  assert.equal(view.recentSessions.find(s => s.key === 'ended').state, 'ended');
  assert.equal(view.recentSessions.find(s => s.key === 'ended').contextPercent, 50);
  const month = dashboardView(raw, null, 'month', now);
  for (const key of ['quiet', 'archived', 'deleted', 'sourceDeleted']) {
    assert.equal(month.sessions.find(s => s.key === key).state, 'idle');
    assert.equal(month.sessions.find(s => s.key === key).contextPercent, null);
  }
  assert.equal(month.sessions.some(s => ['background', 'invalid'].includes(s.key)), false);
});

test('recent sessions keep every running row beyond five and retain chronological ordering', () => {
  const raw = base();
  raw.periods.month.sessions = Object.fromEntries(Array.from({ length: 8 }, (_, i) => [String(i), { lastUsedAt: time(i * 1000), turnEnded: i < 6 }]));
  const view = dashboardView(raw, null, 'today', now);
  assert.deepEqual(view.recentSessions.map(s => s.key), ['0', '1', '2', '3', '4', '6', '7']);
});

test('full history wins preview, today overrides stale history, authoritative summary survives the retained window', () => {
  const raw = base(); raw.historyPreview = history();
  const full = history(); full.daily.push({ date: '2026-10-22', tokens: 200, cost: 3 }, { date: '2026-10-23', tokens: 1, cost: .1 });
  const result = dashboardView(raw, full, 'month', now).history;
  assert.equal(result.source, 'full');
  assert.equal(result.daily.length, 3);
  assert.equal(result.daily.at(-1).tokens, 100);
  assert.equal(result.summary.activeDays, 500);
  assert.equal(result.summary.peakDayTokens, 200);
  assert.equal(result.summary.totalTokens, 9000);
  assert.equal(dashboardView(raw, null, 'today', now).history.source, 'preview');
  delete raw.historyPreview;
  assert.equal(dashboardView(raw, null, 'today', now).history.available, false);
});

test('persistence projects only display fields, preserves history detail and is idempotent', () => {
  const raw = base(); raw.secret = 'secret'; raw.accountEmail = 'secret';
  raw.periods.today.sessions.x = { title: 'permitted title', projectLabel: 'label', path: '/private/transcript', lastUsedAt: time(0), accountEmail: 'secret', token: 'secret' };
  raw.periods.today.projects['/private/workspace'] = { label: 'public label', tokens: 100, path: '/private/workspace' };
  raw.periods.today.sessionDetailsOmitted = true;
  const projected = projectDashboardStats(raw);
  const text = JSON.stringify(projected);
  assert.equal(text.includes('secret'), false);
  assert.equal(text.includes('/private'), false);
  assert.equal(projected.periods.today.sessions.x.title, 'permitted title');
  assert.deepEqual(projectDashboardStats(projected), projected);
  const h = history(); h.daily[0].perModel = { a: { tokens: 50, cost: 1, secret: 'secret' } }; h.daily[0].path = '/private';
  const ph = projectHistoryForCache(h);
  assert.equal(ph.daily[0].perModel.a.tokens, 50);
  assert.equal(JSON.stringify(ph).includes('secret'), false);
  assert.equal(dashboardView(raw, null, 'today', now).sessionsIncomplete, true);
  assert.throws(() => projectHistoryForCache({ daily: [], monthly: 'bad', summary: {} }));
  assert.throws(() => projectHistoryForCache({ daily: [{ date: 'bad' }], monthly: [], summary: {} }));
});

test('throughput uses timed observations and partial pricing remains explicit', () => {
  const raw = base(); Object.assign(raw.periods.today, { timedTokens: 100, timedOutputTokens: 40, timedDurationMs: 2000, unpricedTokens: 5 });
  const result = dashboardView(raw, null, 'today', now);
  assert.equal(result.summary.tokensPerSecond, 50);
  assert.equal(result.summary.outputTokensPerSecond, 20);
  assert.equal(result.summary.costPartial, true);
  assert.equal(result.summary.costUsd, 2);
});

test('omission counts are period scoped and survive caching', () => {
  const raw = base(); raw.sessionDetailsOmitted = { month: 3 }; raw.periodProjectsOmitted = { allTime: 2 };
  const cache = projectDashboardStats(raw);
  assert.deepEqual(cache.sessionDetailsOmitted, { month: 3 });
  assert.equal(dashboardView(cache, null, 'today', now).sessionsIncomplete, false);
  assert.equal(dashboardView(cache, null, 'month', now).sessionsIncomplete, true);
  assert.equal(dashboardView(cache, null, 'allTime', now).projectsIncomplete, true);
});


test('a short preview preserves authoritative lifetime activeDays', () => {
  const raw = base(); raw.historyPreview = history(); raw.historyPreview.summary.activeDays = 22;
  assert.equal(dashboardView(raw, null, 'today', now).history.summary.activeDays, 22);
});

test('multi-model sessions retain every model name', () => {
  const raw = base(); raw.periods.month.sessions.x = { lastUsedAt: time(0), models: { alpha: 10, beta: 50 } };
  assert.equal(dashboardView(raw, null, 'month', now).sessions[0].model, 'beta / alpha');
});

test('an old or undated snapshot cannot move yesterday usage to today', () => {
  const raw = base(); raw.historyPreview = history();
  raw.updatedAt = new Date(2026, 9, 22, 23, 59).toISOString();
  assert.deepEqual(dashboardView(raw, null, 'today', now).history.daily.map(d => d.date), ['2026-10-01']);
  delete raw.updatedAt;
  assert.deepEqual(dashboardView(raw, null, 'today', now).history.daily.map(d => d.date), ['2026-10-01']);
  raw.updatedAt = time(0);
  assert.equal(dashboardView(raw, null, 'today', now).history.daily.at(-1).tokens, 100);
});


test('cumulative sessions use only the Hub cumulative period and retain Hub archive flags', () => {
  const raw = base();
  raw.periods.allTime.sessions.hub = { title: 'Hub', lastUsedAt: time(1), totalTokens: 400, costUsd: 8, models: { hub: 400 }, archived: true };
  raw.periods.month.sessions.recent = { title: 'recent', lastUsedAt: time(1), totalTokens: 50 };
  const projected = projectDashboardStats(raw);
  assert.equal(projected.periods.allTime.sessions.hub.archived, true);
  const view = dashboardView(projected, null, 'allTime', now);
  assert.equal(view.summary.totalTokens, 9000);
  assert.equal(view.summary.costUsd, 200);
  assert.equal(view.sessionsUnavailable, false);
  assert.deepEqual(view.sessions.map(s => s.key), ['hub']);
  assert.equal(view.sessions[0].costUsd, 8);
  assert.equal(view.sessions[0].archived, true);
  assert.equal(view.sessions[0].state, 'idle');
  assert.deepEqual(view.runningSessions.map(s => s.key), ['recent']);
});
