import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { normalizeStats, projectStatsForCache } from './model.mjs';
import { quotaPresentation, quotaWindowPairs, providerQuotaPairs } from './quota-presentation.mjs';

// Invented source-contract values, never recorded provider/account responses.
const fixture = JSON.parse(readFileSync(new URL('./fixtures/provider-limits-v0.78.json', import.meta.url), 'utf8'));
const byId = id => {
  const entry = fixture.cases.find(entry => entry.id === id);
  assert.ok(entry, `Missing synthetic branch ${id}`);
  return normalizeStats(entry.stats)[0];
};
const windowBy = (provider, label) => {
  const window = provider.windows.find(window => window.label === label);
  assert.ok(window, `Missing ${provider.provider} window ${label}`);
  return window;
};
const reading = (provider, label) => quotaPresentation(provider, windowBy(provider, label));

test('v0.78 source fixture covers every quota registry provider and four third-party adapters', () => {
  assert.equal(fixture.upstreamTag, 'v0.78.0');
  assert.equal(fixture.provenance.kind, 'synthetic-source-contract');
  const catalog = fixture.provenance.providerCatalog;
  assert.equal(catalog.length, 30);
  assert.equal(new Set(catalog.map(entry => entry.id)).size, 30);
  assert.deepEqual([...new Set(fixture.cases.map(entry => entry.provider))].sort(), catalog.map(entry => entry.id).sort());
  for (const { id, name } of catalog) {
    const entry = fixture.cases.find(entry => entry.provider === id);
    assert.equal(normalizeStats(entry.stats)[0].name, name);
    assert.ok(entry.sources.every(source => source.lines[0] > 0 && source.lines[1] >= source.lines[0]));
  }
  const thirdparty = fixture.cases.filter(entry => entry.provider === 'thirdparty');
  assert.deepEqual([...new Set(thirdparty.map(entry => entry.stats.limits.providers[0].adapterId))].sort(),
    ['custom', 'newapi-account', 'newapi-token', 'sub2api']);
});

test('every provider branch retains quota fields through persistence and normalization', () => {
  for (const entry of fixture.cases) {
    const raw = entry.stats.limits.providers[0];
    const cached = JSON.parse(JSON.stringify(projectStatsForCache(entry.stats)));
    const stored = cached.limits.providers[0];
    const live = normalizeStats(entry.stats)[0];
    assert.equal(stored.windows.length, raw.windows.length, entry.id);
    // This checks the actual projection boundary, including null/zero/false.
    // Account identifiers deliberately do not belong to the display cache.
    for (const [index, window] of raw.windows.entries()) {
      for (const [key, value] of Object.entries(window)) {
        assert.deepEqual(stored.windows[index][key], value, `${entry.id}.window[${index}].${key}`);
        if (key !== 'label') assert.deepEqual(live.windows[index][key], value, `${entry.id}.live[${index}].${key}`);
      }
    }
    for (const field of ['balance', 'resetCredits', 'usageSummary']) {
      const assertPresentLeaves = (input, output, prefix) => {
        if (input === null || typeof input !== 'object') return assert.deepEqual(output, input, prefix);
        for (const [key, value] of Object.entries(input)) assertPresentLeaves(value, output?.[key], `${prefix}.${key}`);
      };
      if (raw[field]) assertPresentLeaves(raw[field], stored[field], `${entry.id}.${field}`);
    }
    assert.deepEqual(normalizeStats(cached), normalizeStats(entry.stats), entry.id);
    assert.equal(Object.hasOwn(stored, 'accountKey'), false);
    assert.equal(Object.hasOwn(stored, 'accountKeyAliases'), false);
  }
});

test('money, point balances and spend retain their values independently of meter percentages', () => {
  const router = byId('openrouter-1');
  assert.equal(reading(router, 'Credits').value, 42);
  assert.equal(reading(router, 'Credits').unit, 'USD');
  assert.equal(reading(router, 'Credits').meter, 84);
  const cline = byId('cline-1');
  assert.equal(reading(cline, 'Credits').value, 0.5);
  assert.equal(reading(cline, 'Credits').unit, 'CREDITS');
  assert.equal(reading(cline, 'Credits').meter, null);
  assert.equal(reading(cline, 'Usage credits').value, 2.3);
  assert.equal(reading(cline, 'Usage credits').type, 'spend');
  assert.equal(reading(cline, 'Usage credits').unit, 'USD');
  const unknownAllowance = byId('commandcode-2');
  assert.equal(reading(unknownAllowance, 'Monthly').value, 47.42);
  assert.equal(reading(unknownAllowance, 'Monthly').meter, null);
  assert.equal(windowBy(unknownAllowance, 'Monthly').limit, null);
});

test('unlimited, unknown and exhausted quotas do not collapse into the same zero', () => {
  const unlimited = byId('workbuddy-2');
  assert.equal(reading(unlimited, 'Credits').type, 'unlimited');
  assert.equal(windowBy(unlimited, 'Credits').remaining, null);
  assert.equal(windowBy(unlimited, 'Credits').used, 32);
  const disabled = byId('antigravity-1');
  assert.equal(reading(disabled, 'Claude/GPT weekly').type, 'unknown');
  assert.equal(reading(disabled, 'Claude/GPT weekly').value, null);
  assert.equal(reading(disabled, 'Claude/GPT weekly').meter, null);
  const exhausted = byId('thirdparty-4');
  assert.equal(reading(exhausted, 'Balance').value, 0);
  assert.equal(reading(exhausted, 'Balance').type, 'balance');
  assert.equal(reading(exhausted, 'Balance').unit, 'USD');
  const spend = byId('claude-2');
  assert.equal(windowBy(spend, 'Usage credits').limit, null);
  assert.equal(reading(spend, 'Usage credits').value, 3.5);
  for (const [id, action, status] of [
    ['antigravity-3', 'accountVerification', 'unauthorized'],
    ['workbuddy-3', 'appSessionEncrypted', 'notConfigured']
  ]) {
    const account = byId(id);
    assert.equal(account.actionRequired, action);
    assert.equal(account.statusCode, status);
    assert.equal(account.ok, false);
    assert.deepEqual(account.windows, []);
    assert.equal(quotaPresentation(account, null).value, null);
  }
});

test('Kiro overage exposes estimated cost rather than calling it a remaining balance', () => {
  const kiro = byId('kiro-1');
  const overage = reading(kiro, 'Overage');
  assert.equal(overage.type, 'overage');
  assert.equal(overage.value, 12.5);
  assert.match(overage.detail, /3\.2/);
  assert.equal(overage.meter, null);
  const pairs = quotaWindowPairs(kiro, windowBy(kiro, 'Overage'));
  assert.ok(pairs.some(([label, value]) => label === '预估费用（USD）' && value === '3.2'));
});

test('source-specific cadence, count and composition metadata remain available in detail', () => {
  const codex = byId('codex-1');
  const extra = codex.windows.find(window => window.additional);
  assert.equal(extra.limitId, 'model-cap');
  assert.equal(extra.windowMinutes, 300);
  assert.ok(quotaWindowPairs(codex, extra).some(([label, value]) => label === '附加额度' && value === '是'));
  const code = byId('opencode-1');
  assert.deepEqual(code.windows.map(window => window.source), ['web', 'local', 'local', 'web']);
  const zcode = byId('zai-2');
  assert.deepEqual(zcode.windows.map(window => window.boundaryKind), ['reset', 'expiry', 'mixed']);
  const notion = byId('notion-1');
  assert.equal(windowBy(notion, 'Rolling').windowMinutes, 360);
  assert.equal(reading(notion, 'Rolling').unit, '点');
  const kimi = byId('kimi-1');
  assert.match(windowBy(kimi, 'Monthly').detail, /Code 120%/);
  assert.equal(windowBy(kimi, 'Monthly').usedPercent, 100);
  const zed = byId('zed-1');
  assert.equal(reading(zed, 'Edit Predictions').unit, '次');
  assert.equal(reading(zed, 'Token Spend').unit, 'USD');
});

test('product rows remain independent and coupon, wallet and usage riders reach details', () => {
  const consoleCase = fixture.cases.find(entry => entry.id === 'mimo-1');
  const memberCase = fixture.cases.find(entry => entry.id === 'mimo-2');
  const providers = normalizeStats({ limits: { providers: [
    ...consoleCase.stats.limits.providers, ...memberCase.stats.limits.providers
  ] } });
  assert.equal(providers.length, 2);
  assert.deepEqual(providers.map(provider => provider.accountLabel), ['Console', 'Desktop Membership']);
  assert.equal(providers[1].windows[0].windowMinutes, null);
  assert.ok(providerQuotaPairs(providers[0]).some(([label, value]) => label === '赠送余额' && value.includes('10')));
  const claude = byId('claude-1');
  const couponDetails = providerQuotaPairs(claude);
  assert.ok(couponDetails.some(([label, value]) => label.includes('当前可用') && value === '否'));
  assert.ok(couponDetails.some(([label, value]) => label.includes('已暂停') && value === '是'));
  const sub = byId('thirdparty-3');
  const usageDetails = providerQuotaPairs(sub);
  assert.ok(usageDetails.some(([label, value]) => label.includes('缓存写入') && value === '40'));
  assert.ok(usageDetails.some(([label, value]) => label.includes('实际费用') && value.includes('1.5')));
  const type = byId('typesafe-1');
  assert.ok(providerQuotaPairs(type).some(([label]) => label.startsWith('余额批次 1')));
});
