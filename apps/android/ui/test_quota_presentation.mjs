import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeStats } from './model.mjs';
import { providerQuotaEntries, quotaPresentation, providerQuotaPairs, quotaWindowPairs, quotaNumber, quotaCompactNumber, quotaFigure } from './quota-presentation.mjs';
const p = (provider, windows = [], extra = {}) => normalizeStats({ limits: { providers: [{ provider, status: 'ok', windows, ...extra }] } })[0];

test('small positive amounts keep their magnitude; negatives and zero remain distinct', () => {
  assert.equal(quotaNumber(0), '0');
  assert.notEqual(quotaCompactNumber(0.00000012), '0');
  assert.equal(Number(quotaNumber(0.00000012)), 0.00000012);
  const account = p('deepseek', [{ metric: 'credits', remaining: -1.25, currency: 'CNY', showMeter: false }]);
  assert.equal(quotaPresentation(account, account.windows[0]).value, -1.25);
  assert.equal(quotaPresentation(account, account.windows[0]).meter, null);
});

test('amount-only wallets have no invented percentage and hideMeter suppresses known percentages', () => {
  const account = p('openrouter', [
    { metric: 'credits', remaining: 20, currency: 'USD' },
    { kind: 'billing', label: 'On-demand spend', metric: 'spend', used: 5, limit: 20, remainingPercent: 75, currency: 'USD', showMeter: false }
  ], { balance: { amount: 20, currency: 'USD', monthSpend: 80 } });
  assert.equal(quotaPresentation(account, account.windows[0]).meter, null);
  assert.equal(quotaPresentation(account, account.windows[1]).meter, null);
  assert.equal(quotaPresentation(account, account.windows[1]).value, 5);
});

test('count pools retain denominators and source-supported currency rules without calling all counts balances', () => {
  for (const [id, label, unit] of [['qoder','Credits','点'], ['mimo','Token Plan','tokens'], ['cursor','Team pool','USD'], ['openrouter','Daily limit','USD']]) {
    const account = p(id, [{ kind: 'billing', label, used: 30, limit: 100, remaining: 70, remainingPercent: 70, showMeter: true }]);
    const reading = quotaPresentation(account, account.windows[0]);
    assert.equal(reading.value, 70); assert.equal(reading.unit, unit); assert.equal(reading.type, 'count');
    assert.ok(reading.detail.includes('100'));
  }
  const ark = p('volcengine', [{ kind: 'session', label: 'Requests', used: 12345, showMeter: false }]);
  const reading = quotaPresentation(ark, ark.windows[0]);
  assert.equal(reading.unit, ''); assert.equal(reading.value, 12345); assert.match(reading.detail, /未标注单位/);
});

test('explicit unlimited differs from a missing cap or missing meter reading', () => {
  const account = p('copilot', [{ kind: 'billing', label: 'Chat', showMeter: false }]);
  assert.equal(quotaPresentation(account, account.windows[0]).type, 'unknown');
  const text = p('future', [{ showMeter: false, detail: 'Ask the administrator' }]);
  assert.equal(quotaPresentation(text, text.windows[0]).type, 'text');
  assert.equal(quotaPresentation(text, text.windows[0]).text, 'Ask the administrator');
});

test('actual window duration survives empty labels instead of labeling every session five hours', () => {
  for (const minutes of [60,300,360]) {
    const account = p('ollama', [{ kind:'session', windowMinutes:minutes, remainingPercent:40 }]);
    assert.equal(quotaPresentation(account, account.windows[0]).title, minutes / 60 + '小时额度');
  }
});

test('metadata-only accounts and reset coupons have reachable entries without replacing real windows', () => {
  const account = p('claude', [{ kind:'weekly', remainingPercent:50 }], { balance:{ amount:0, currency:'USD' }, resetCredits:{availableCount:0,nextExpiresAt:'2026-10-26T00:00:00Z'} });
  const entries=providerQuotaEntries(account);
  assert.equal(entries.length,3);
  const coupon=quotaPresentation(account,entries[2]);
  assert.equal(coupon.type,'coupons'); assert.equal(coupon.value,0); assert.equal(coupon.boundary,'到期');
  const metadata=p('mimo',[],{balance:{giftBalance:2,cashBalance:3,planStatus:'expired'}});
  assert.deepEqual(providerQuotaEntries(metadata),[null]);
  assert.ok(quotaWindowPairs(metadata,null).some(([label,value])=>label==='套餐状态'&&value==='已到期'));
});

test('unknown values stay absent in details, while real zero and false observations remain readable', () => {
  const account=p('typesafe',[],{balance:{amount:null,monthSpend:0,monthSinceTracking:false,currency:'USD',tranches:[{amount:0,expiresAt:'2026-10-26T00:00:00Z'}]},resetCredits:{availableCount:0,grants:[{label:'Grant',usableNow:false,paused:false}]}});
  const pairs=providerQuotaPairs(account);
  assert.ok(pairs.some(([label,value])=>label==='本月消费'&&value==='$0'));
  assert.ok(pairs.some(([label,value])=>label.includes('当前可用')&&value==='否'));
  assert.ok(pairs.some(([label,value])=>label==='余额批次 1 · 余额'&&value==='$0'));
  assert.ok(!pairs.some(([label])=>label==='现金余额'));
});

test('a limit without usage is shown as a limit, never as consumption or an unknown number', () => {
  for(const [id,label] of [['qoder','Credits'],['alibaba','Monthly']]) {
    const account=p(id,[{kind:'billing',label,limit:100,used:null,remaining:null,showMeter:false}]);
    const reading=quotaPresentation(account,account.windows[0]);
    assert.equal(reading.value,100); assert.equal(reading.label,'上限'); assert.equal(reading.meter,null);
  }
});

test('card percentages stay compact and money keeps two decimals across value changes', () => {
  assert.deepEqual([98.09, 100, 9, 0].map(value => quotaFigure({ value, unit: '%' }).text),
    ['98.09', '100', '9', '0']);
  assert.deepEqual([1, 10.5, 9999.99, 10000].map(value => quotaFigure({ value, unit: 'USD' }).text),
    ['$1.00', '$10.50', '$9,999.99', '$10.00K']);
  assert.deepEqual([0, 9, 10.5].map(value => quotaFigure({ value, unit: '点' }).text), ['0', '9', '10.5']);
  for (const unit of ['%', 'USD', 'tokens', '点', '']) {
    assert.equal(new Set([0, 9, 100, 9999, 10000, -1].map(value => quotaFigure({ value, unit }).reserveText)).size, 1);
  }
});

test('card figures preserve tiny nonzero values, missing values and readable extreme magnitudes', () => {
  assert.notEqual(quotaFigure({ value: 0.00000012, unit: 'USD' }).text, '$0.00');
  assert.equal(quotaFigure({ value: null, unit: '%' }).text, '—');
  assert.equal(quotaFigure({ value: -1.25, unit: 'CNY' }).text, '¥-1.25');
  assert.equal(quotaFigure({ value: 1e308, unit: 'USD' }).text, '$1e+308');
  assert.equal(quotaFigure({ value: 12345, unit: 'tokens' }).text, '12.35K');
  assert.equal(quotaFigure({ text: '不限', unit: '', value: null }).text, '不限');
});
