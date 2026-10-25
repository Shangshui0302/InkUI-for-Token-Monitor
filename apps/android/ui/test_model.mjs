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

test('public plan labels survive projection without persisting arbitrary subscription data', () => {
  const input = stats([
    provider({ plan: 'Pro 长套餐', planType: 'Team', subscriptionPlan: 'Annual', accountKey: 'private-key', subscription: { token: 'private-token' } }),
    provider({ plan: { label: 'private nested data' }, planType: 99, subscriptionPlan: false })
  ]);
  const projected = projectStatsForCache(input);
  assert.deepEqual(projected.limits.providers.map(({ plan, planType, subscriptionPlan }) => ({ plan, planType, subscriptionPlan })), [
    { plan: 'Pro 长套餐', planType: 'Team', subscriptionPlan: 'Annual' },
    { plan: null, planType: null, subscriptionPlan: null }
  ]);
  assert.equal(JSON.stringify(projected).includes('private'), false);
  assert.deepEqual(projectStatsForCache(projected), projected);
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

test('all 30 Hub providers have catalog display names and unknown providers remain usable', () => {
  const catalog = {
    claude: 'Claude', codex: 'Codex', opencode: 'OpenCode', cursor: 'Cursor', antigravity: 'Antigravity',
    cline: 'Cline', factory: 'Factory Droid', kimi: 'Kimi', grok: 'Grok', copilot: 'GitHub Copilot',
    zed: 'Zed', commandcode: 'Command Code', mimo: 'Xiaomi MiMo', zai: 'GLM', zaiteam: 'GLM Team',
    kiro: 'Kiro', workbuddy: 'WorkBuddy', qoder: 'Qoder', deepseek: 'DeepSeek', devin: 'Devin',
    minimax: 'Minimax', typesafe: 'TypeSafe', openrouter: 'OpenRouter', volcengine: 'Volcengine',
    ollama: 'Ollama', trae: 'Trae CN', alibaba: 'Alibaba Cloud', stepfun: 'StepFun', notion: 'Notion AI',
    thirdparty: 'Third-party APIs'
  };
  const result = normalizeStats(stats(Object.keys(catalog).map(id => provider({ provider: id }))));
  assert.equal(result.length, 30);
  for (const item of result) assert.equal(item.name, catalog[item.provider]);
  assert.equal(normalizeStats(stats([provider({ provider: 'future-provider' })]))[0].name, 'future-provider');
});

test('complete typed quota details survive live projection and JSON round trip without credentials', () => {
  const secret = { accountKey: 'PRIVATE-key', webAccountKey: 'PRIVATE-web', accountKeyAliases: ['PRIVATE-alias'],
    token: 'PRIVATE-token', accessToken: 'PRIVATE-access', refreshToken: 'PRIVATE-refresh', apiKey: 'PRIVATE-api',
    clientSecret: 'PRIVATE-secret', password: 'PRIVATE-password', authorization: 'PRIVATE-header',
    cookie: 'PRIVATE-cookie', raw: { nested: 'PRIVATE-raw' }, sourceDeviceId: 'PRIVATE-device',
    path: 'PRIVATE-path', env: { value: 'PRIVATE-env' } };
  const input = stats([provider({
    ...secret, provider: 'thirdparty', adapterId: 'new-api', accountLabel: '个人', accountName: '示例',
    accountEmail: 'example@example.invalid', planLabel: 'Pro', workspaceKind: 'personal',
    status: 'sourceRateLimited', source: 'web', sourceDetail: 'managed', actionRequired: 'accountVerification', region: 'cn-personal',
    balanceUsd: 0,
    balance: { ...secret, amount: 0, currency: 'CNY', todaySpend: 0, weekSpend: 1, monthSpend: 2,
      allTimeSpend: 3, requestCount: 0, quotaGroup: '套餐组', expiresAt: null, trackingSince: '2026-10-01T00:00:00Z',
      monthSinceTracking: false, giftBalance: 0, cashBalance: 0, planUsed: 0, planLimit: 100, planPercent: 0,
      planStatus: 'active', todayTokenTotal: 0, todayUsageDate: '2026-10-25', latestModelUsageDate: '2026-10-24',
      todayUsageBasis: 'provider', snapshotDate: '2026-10-25', tranches: [{ ...secret, amount: 0, currency: 'CNY', expiresAt: null }] },
    resetCredits: { ...secret, availableCount: 0, nextExpiresAt: null, expirations: ['2026-10-31T00:00:00Z', secret],
      grants: [{ ...secret, id: 'semantic-coupon', label: '补偿', resetsLeft: 0, resetsTotal: 1,
        startsAt: null, endsAt: '2026-10-31T00:00:00Z', usableNow: false, useRequiresLimit: true, paused: false,
        clears: ['weekly', secret] }] },
    usageSummary: { ...secret, period: 'month', requests: 0, todayTokens: 0, weekTokens: 1, inputTokens: 2,
      outputTokens: 3, cacheReadTokens: 4, cacheCreationTokens: 5, totalTokens: 14, standardCost: 0, actualCost: 0, averageDurationMs: 0 },
    windows: [{ ...secret, kind: 'daily', label: '', source: 'local', metric: 'spend', limitId: 'semantic-limit',
      boundaryKind: 'expiry', additional: false, used: 0, limit: 100, remaining: 100, usedPercent: 0,
      remainingPercent: 100, windowMinutes: 1440, resetsAt: null, resetDescription: '月底', detail: '测试', currency: 'CNY', showMeter: false }]
  })]);
  input.limits.updatedAt = '2026-10-25T00:00:00Z'; input.limits.refreshMs = 300000;
  const projected = projectStatsForCache(input);
  const cached = JSON.parse(JSON.stringify(projected));
  assert.equal(JSON.stringify(cached).includes('PRIVATE'), false);
  assert.deepEqual(projectStatsForCache(cached), cached);
  assert.deepEqual(normalizeStats(cached), normalizeStats(input));
  assert.equal(cached.limits.updatedAt, input.limits.updatedAt);
  assert.equal(cached.limits.refreshMs, 300000);
  const row = normalizeStats(cached)[0];
  assert.equal(row.provider, 'thirdparty'); assert.equal(row.statusCode, 'sourceRateLimited');
  assert.equal(row.status, '采集受限'); assert.equal(row.source, 'web');
  assert.equal(row.accountEmail, 'example@example.invalid'); assert.equal(row.planLabel, 'Pro');
  assert.equal(row.balance.amount, 0); assert.equal(row.balance.monthSinceTracking, false);
  assert.equal(row.balance.tranches[0].amount, 0);
  assert.equal(row.resetCredits.grants[0].id, 'semantic-coupon');
  assert.equal(row.resetCredits.grants[0].usableNow, false);
  assert.deepEqual(row.resetCredits.grants[0].clears, ['weekly']);
  assert.equal(row.usageSummary.actualCost, 0);
  const window = row.windows[0];
  assert.equal(window.label, '每日额度'); assert.equal(window.metric, 'spend'); assert.equal(window.used, 0);
  assert.equal(window.limitId, 'semantic-limit'); assert.equal(window.boundaryKind, 'expiry');
  assert.equal(window.showMeter, false); assert.equal(window.additional, false);
  assert.equal(window.windowMinutes, 1440); assert.equal(window.resetDescription, '月底'); assert.equal(window.detail, '测试');
});

test('typed detail projection rejects arrays, nonfinite numbers and coercion without turning unknown into zero', () => {
  const input = stats([provider({ balance: [], resetCredits: [], usageSummary: [],
    windows: [{ kind: 'daily', used: '0', limit: Infinity, remaining: NaN, showMeter: 'false', additional: 0 }] })]);
  const projected = projectStatsForCache(input);
  const row = projected.limits.providers[0];
  assert.equal(row.balance, null); assert.equal(row.resetCredits, null); assert.equal(row.usageSummary, null);
  assert.equal(row.windows[0].used, null); assert.equal(row.windows[0].limit, null);
  assert.equal(row.windows[0].remaining, null); assert.equal(row.windows[0].showMeter, null);
  assert.equal(row.windows[0].additional, null);
  assert.deepEqual(projectStatsForCache(projected), projected);
  assert.equal(normalizeStats(projected)[0].windows[0].value, null);
});
