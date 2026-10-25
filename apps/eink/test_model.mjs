import test from 'node:test';
import assert from 'node:assert/strict';
import { windowValue, normalizeStats, projectStatsForCache, absoluteTime } from './model.mjs';

const stats = providers => ({ limits: { providers } });
const provider = overrides => ({
  provider: 'codex', status: 'ok', stale: false, windows: [], ...overrides
});

test('remaining percent preserves zero and null never becomes zero', () => {
  assert.equal(windowValue({ remainingPercent: 0, usedPercent: 50 }).value, 0);
  assert.equal(windowValue({ remainingPercent: null, usedPercent: null }).value, null);
  assert.equal(windowValue({ usedPercent: 0 }).value, 100);
  assert.equal(windowValue({ usedPercent: 100 }).value, 0);
  assert.equal(windowValue({ remainingPercent: NaN }).value, null);
});

test('zero currency balance is available', () => {
  assert.deepEqual(windowValue({ remaining: 0, currency: 'CNY' }), {
    value: 0, unit: 'CNY', valueLabel: '剩余余额'
  });
});

test('same provider accounts stay independent and window label survives', () => {
  const result = normalizeStats(stats([
    provider({ accountLabel: '测试甲', windows: [{ label: '每周', remainingPercent: 0 }] }),
    provider({ accountLabel: '测试乙' })
  ]));
  assert.equal(result.length, 2);
  assert.equal(result[0].account, '测试甲');
  assert.equal(result[1].account, '测试乙');
  assert.equal(result[0].windows[0].label, '每周');
});

test('provider errors and upstream stale remain explicit', () => {
  const result = normalizeStats(stats([
    provider({ status: 'unauthorized', stale: true }),
    provider({ status: 'notConfigured' })
  ]));
  assert.equal(result[0].ok, false);
  assert.equal(result[0].status, '来源认证失败');
  assert.equal(result[0].stale, true);
  assert.equal(result[1].status, '未配置');
  assert.equal(result[1].stale, false);
});

test('malformed payload rejected; empty provider list valid', () => {
  const malformed = [null, [], {}, stats([null]), stats([{}]), stats([provider({ windows: [null] })])];
  for (const value of malformed) {
    assert.throws(() => normalizeStats(value), /格式/);
  }
  assert.deepEqual(normalizeStats(stats([])), []);
  assert.equal(absoluteTime(null), '未提供');
});

test('persisted projection omits history and identifiers while preserving displayed values', () => {
  const input = {
    history: ['private history'],
    devices: [{ userId: 'private device identifier' }],
    ...stats([provider({
      accountLabel: '测试账号',
      accountKey: 'private key',
      userId: 'private user identifier',
      extra: { arbitrary: 'private content' },
      updatedAt: '2026-01-01T00:00:00Z',
      source: 'oauth',
      stale: true,
      windows: [{
        kind: 'weekly',
        label: '每周',
        remainingPercent: 0,
        remaining: 0,
        currency: 'CNY',
        resetsAt: '2026-01-08T00:00:00Z',
        history: ['private window history']
      }]
    })])
  };
  const projected = projectStatsForCache(input);
  const persisted = JSON.parse(JSON.stringify(projected));
  assert.deepEqual(normalizeStats(persisted), normalizeStats(input));
  assert.deepEqual(Object.keys(persisted), ['limits']);
  assert.equal(persisted.limits.providers[0].accountLabel, '测试账号');
  assert.equal(persisted.limits.providers[0].windows[0].remainingPercent, 0);
  assert.equal(persisted.limits.providers[0].windows[0].remaining, 0);
  assert.equal(JSON.stringify(persisted).includes('private'), false);
  assert.equal(Object.hasOwn(persisted.limits.providers[0], 'accountKey'), false);
  assert.equal(Object.hasOwn(persisted.limits.providers[0], 'userId'), false);
});


test('blank labels fall back to the window kind while explicit labels survive', () => {
  const result = normalizeStats(stats([provider({ windows: [
    { kind: 'weekly', label: '' },
    { kind: 'session', label: '  \t' },
    { kind: 'billing', label: '本月预算' },
    { kind: 'unknown', label: '' }
  ] })]));
  assert.deepEqual(result[0].windows.map(window => window.label), [
    '每周额度', '会话额度', '本月预算', '额度窗口'
  ]);
});

test('priority preserves every account and the relative order of unknown providers', () => {
  const input = [
    provider({ provider: 'other-a', accountLabel: '其他甲' }),
    provider({ provider: 'antigravity', accountLabel: '反重力' }),
    provider({ provider: 'codex', accountLabel: '甲' }),
    provider({ provider: 'other-b', accountLabel: '其他乙' }),
    provider({ provider: 'deepseek', accountLabel: '深度' }),
    provider({ provider: 'codex', accountLabel: '乙' })
  ];
  assert.deepEqual(normalizeStats(stats(input)).map(item => item.account), [
    '甲', '乙', '深度', '反重力', '其他甲', '其他乙'
  ]);
  assert.equal(normalizeStats(stats([provider({ source: 'other' })]))[0].source, '');
});
