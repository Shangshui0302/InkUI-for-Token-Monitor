import { absoluteTime } from './model.mjs';

const finite = value => typeof value === 'number' && Number.isFinite(value);
const present = value => Array.isArray(value) ? value.length > 0 : value !== null && value !== undefined && value !== '';
const first = (...values) => values.find(finite) ?? null;
const clamp = n => Math.max(0, Math.min(100, n));
const format = new Intl.NumberFormat('zh-CN', { maximumSignificantDigits: 21 });
export const quotaNumber = value => finite(value) ? format.format(value) : '—';
const compact = value => !finite(value) ? '—' : value !== 0 && Math.abs(value) < .01 ? Number(value.toPrecision(3)).toString() : Math.abs(value) >= 1e9 ? +(value / 1e9).toFixed(2) + 'B' : Math.abs(value) >= 1e6 ? +(value / 1e6).toFixed(2) + 'M' : Math.abs(value) >= 1e4 ? +(value / 1e3).toFixed(2) + 'K' : new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 2 }).format(value);
export const quotaCompactNumber = compact;
const fixedFigure = new Intl.NumberFormat('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const percentFigure = new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 2 });
// The card format is tied to the metric, rather than the current value. Keep
// percentages compact and money at two decimals, preserving tiny amounts.
export function quotaFigure(reading) {
  const symbol = { CNY: '¥', USD: '$', EUR: '€', GBP: '£', HKD: 'HK$' }[reading.unit] || '';
  const unit = reading.unit && !symbol && !reading.text ? reading.unit === 'CREDITS' ? '点' : reading.unit : '';
  if (reading.text) return { text: reading.text, reserveText: reading.text, unit };
  const decimal = Boolean(symbol);
  let amount = reading.unit === '%' && finite(reading.value) ? percentFigure.format(reading.value) : compact(reading.value);
  if (reading.unit !== '%' && finite(reading.value) && (Math.abs(reading.value) >= 1e12 || amount.length > 9 && Math.abs(reading.value) < .01)) {
    const [mantissa, exponent] = reading.value.toExponential(2).split('e');
    amount = Number(mantissa) + 'e' + exponent;
  } else if (finite(reading.value) && decimal && (reading.value === 0 || Math.abs(reading.value) >= .01)) {
    const scale = Math.abs(reading.value) >= 1e9 ? 1e9 : Math.abs(reading.value) >= 1e6 ? 1e6 : Math.abs(reading.value) >= 1e4 ? 1e3 : 1;
    amount = fixedFigure.format(reading.value / scale) + (scale === 1e9 ? 'B' : scale === 1e6 ? 'M' : scale === 1e3 ? 'K' : '');
  }
  return { text: symbol + amount, unit,
    // Cover the complete ordinary compact format, including a sign and a
    // suffix, so crossing 9 / 10 / 100 / 1,000 does not resize the account.
    reserveText: reading.unit === '%' ? '100.00' : symbol + '-9,999.99B' };
}
const sourceNames = { oauth: 'OAuth', api: 'API', cli: 'CLI', web: '网页', rpc: 'RPC', local: '本地' };
const kindNames = { session: '会话额度', daily: '每日额度', weekly: '每周额度', billing: '账期额度', resetCredits: '重置券' };
const boundaryNames = { reset: '重置', expiry: '到期', mixed: '重置 / 到期' };
const symbols = { USD: '$', CNY: '¥', EUR: '€', GBP: '£', JPY: 'JPY ', HKD: 'HK$' };
const unitName = code => code === 'CREDITS' ? '点' : code || '';
export function quotaAmount(value, unit = '', short = false) {
  if (!finite(value)) return '—';
  const text = short ? compact(value) : quotaNumber(value);
  return symbols[unit] ? symbols[unit] + text : text + (unit ? ' ' + unitName(unit) : '');
}
function remainingPercent(w) {
  if (finite(w.remainingPercent)) return clamp(w.remainingPercent);
  if (finite(w.usedPercent)) return clamp(100 - w.usedPercent);
  if (finite(w.used) && w.limit > 0) return clamp(100 - w.used / w.limit * 100);
  return null;
}
function moneyUnit(p, w) { return w.currency || p.balance?.currency || ''; }
function countUnit(p, w) {
  const id = p.provider, label = w.label || '';
  if (id === 'kiro' || id === 'qoder' || id === 'notion' || id === 'copilot' && /credit/i.test(label)) return '点';
  if (id === 'zed' && (w.limitId === 'zed.edit-predictions' || /edit predictions/i.test(label))) return '次';
  if (w.currency) return unitName(w.currency);
  if (id === 'opencode' && w.source === 'local' || id === 'cursor' && /^(Overall|Team pool)$/.test(label) || id === 'commandcode' && w.kind !== 'billing' || id === 'openrouter') return 'USD';
  if ((id === 'zai' || id === 'zaiteam') && (w.kind === 'daily' || w.kind === 'billing' && w.limitId)) return 'tokens';
  if (id === 'mimo' && label === 'Token Plan' || id === 'factory' && w.kind === 'billing' && /^(Standard|Premium)$/.test(label)) return 'tokens';
  // Ark's Requests fallback can contain tokens; the wire lacks a unit marker.
  // Keep the observation without claiming it is a number of requests or tokens.
  if (id === 'volcengine' && label === 'Requests' && w.showMeter === false && !finite(w.limit)) return '';
  if (/requests|predictions|MCP|prompts/i.test(label) || id === 'copilot') return '次';
  return '';
}
export function providerQuotaEntries(p) {
  const entries = p.windows.map(w => ({ ...w }));
  if (finite(p.balance?.amount) && !entries.some(w => (w.metric === 'credits' || /balance/i.test(w.label || '')) && first(w.remaining, p.balance.amount) === p.balance.amount && (w.currency || p.balance.currency) === p.balance.currency)) {
    entries.push({ kind: 'billing', metric: 'credits', label: '账号余额', remaining: p.balance.amount, currency: p.balance.currency, showMeter: false, boundaryKind: 'expiry', resetsAt: p.balance.expiresAt });
  }
  if (p.resetCredits) entries.push({ kind: 'resetCredits', label: '重置券', remaining: p.resetCredits.availableCount, resetsAt: p.resetCredits.nextExpiresAt, boundaryKind: 'expiry', showMeter: false });
  if (!entries.length && finite(p.balanceUsd)) entries.push({ kind: 'billing', metric: 'credits', label: '余额（USD）', remaining: p.balanceUsd, currency: 'USD', showMeter: false });
  if (!entries.length) entries.push(null);
  return entries;
}
// Interpret the normalized Hub contract, keeping monetary and count observations
// independent from a meter. No provider calls or inferred starting balances.
export function quotaPresentation(p, w) {
  if (!w) return { value: null, unit: '', label: p.status, type: 'unknown', meter: null, detail: '', boundary: '', title: '账号资料' };
  const id = p.provider;
  const explicitLabel = Object.hasOwn(w, 'rawLabel') ? w.rawLabel : w.label;
  const duration = !explicitLabel && w.kind === 'session' && w.windowMinutes > 0 ? (w.windowMinutes % 60 === 0 ? w.windowMinutes / 60 + '小时额度' : w.windowMinutes + '分钟额度') : '';
  const overage = id === 'kiro' && w.kind === 'billing' && w.label === 'Overage' && w.showMeter === false;
  const spend = w.metric === 'spend' || !w.metric && w.kind === 'billing' && w.label === 'Usage credits';
  const unlimited = w.showMeter === false && /\bunlimited\b|不限/i.test(w.detail || '');
  const percent = remainingPercent(w);
  const result = { value: null, unit: '', label: '未提供额度', type: 'unknown', meter: w.showMeter === false ? null : percent, detail: '', boundary: boundaryNames[w.boundaryKind] || '重置', title: duration || w.label || kindNames[w.kind] || '额度窗口' };
  if (unlimited) return { ...result, type: 'unlimited', text: '不限', label: '不限额', detail: finite(w.used) ? '已用 ' + quotaAmount(w.used, countUnit(p, w) || w.currency || '') : w.detail || '', meter: null };
  if (overage) return { ...result, value: first(w.used), unit: '点', type: 'overage', label: '超额已用', detail: finite(w.remaining) ? '预估费用 ' + quotaAmount(w.remaining, 'USD') : '', meter: null };
  if (w.kind === 'resetCredits') return { ...result, value: first(w.remaining), unit: '张', type: 'coupons', label: '可用重置券', meter: null };
  if (spend) {
    const unit = moneyUnit(p, w);
    return { ...result, value: first(w.used), unit, type: 'spend', label: '已消费', detail: finite(w.limit) ? '上限 ' + quotaAmount(w.limit, unit) : '', meter: result.meter };
  }
  // A legacy balance without a metric may still be named Balance by the Hub.
  if (w.metric === 'credits' || !w.metric && /^(Balance|Credits|Token quota|账号余额|余额（USD）)$/i.test(w.label || '') && Boolean(w.currency)) {
    const unit = moneyUnit(p, w);
    return { ...result, value: first(w.remaining, p.balance?.amount), unit, type: unit === 'CREDITS' ? 'points' : 'balance', label: unit === 'CREDITS' ? '剩余点数' : '余额', detail: finite(w.used) && finite(w.limit) ? '已用 ' + quotaAmount(w.used, unit) + ' / ' + quotaAmount(w.limit, unit) : finite(w.limit) ? '总额 ' + quotaAmount(w.limit, unit) : '' };
  }
  const unit = countUnit(p, w);
  const remaining = first(w.remaining, finite(w.used) && finite(w.limit) ? Math.max(0, w.limit - w.used) : null);
  const absolute = finite(w.used) || finite(w.limit) || finite(remaining);
  if (unit && absolute) {
    const value = finite(remaining) ? remaining : first(w.used, w.limit);
    return { ...result, value, unit, type: 'count', label: finite(remaining) ? '剩余' : finite(w.used) ? '已用' : '上限', detail: finite(w.limit) ? quotaAmount(finite(remaining) ? remaining : w.used, unit) + ' / ' + quotaAmount(w.limit, unit) : '' };
  }
  if (finite(percent)) return { ...result, value: percent, unit: '%', type: 'percent', label: '剩余', detail: absolute ? (finite(w.used) ? '已用 ' + quotaNumber(w.used) : '剩余 ' + quotaNumber(remaining)) + (finite(w.limit) ? ' / ' + quotaNumber(w.limit) : '') : w.detail || '' };
  if (absolute) return { ...result, value: finite(remaining) ? remaining : first(w.used, w.limit), unit, type: 'count', label: finite(remaining) ? '剩余' : finite(w.used) ? '已用' : '上限', detail: finite(w.limit) ? '上限 ' + quotaNumber(w.limit) : id === 'volcengine' && w.label === 'Requests' ? '上游未标注单位' : '' };
  if (w.detail && w.showMeter === false) return { ...result, text: w.detail, type: 'text', label: '额度说明', meter: null };
  return result;
}
const yesNo = value => value === true ? '是' : value === false ? '否' : '未提供';
function pairsFor(object, definitions, prefix = '') {
  if (!object) return [];
  return definitions.filter(([key]) => present(object[key])).map(([key, label, formatter]) => [prefix + label, formatter ? formatter(object[key]) : typeof object[key] === 'number' ? quotaNumber(object[key]) : String(object[key])]);
}
export function providerQuotaPairs(p) {
  const pairs = [['服务商', p.name], ['服务商标识', p.provider], ['状态', p.status], ['来源过期', yesNo(p.stale)]];
  pairs.push(...pairsFor(p, [
    ['accountLabel', '账号'], ['accountName', '账号姓名'], ['accountEmail', '账号邮箱'], ['planLabel', '套餐'], ['plan', '套餐（兼容字段）'], ['planType', '套餐类型'], ['subscriptionPlan', '订阅套餐'],
    ['source', '来源', x => sourceNames[x] || x], ['sourceDetail', '采集端'], ['updatedAt', '采集时间', absoluteTime], ['adapterId', '适配器'], ['workspaceKind', '工作区'], ['region', '区域'],
    ['actionRequired', '所需操作', x => ({ accountVerification: '需在服务商处验证账号', appSessionEncrypted: '服务商应用会话已加密' }[x] || x)], ['balanceUsd', '余额（USD，兼容字段）', x => quotaAmount(x, 'USD')]
  ]));
  const currency = p.balance?.currency || '';
  const money = x => quotaAmount(x, currency);
  pairs.push(...pairsFor(p.balance, [
    ['amount', '余额', money], ['currency', '余额单位'], ['todaySpend', '今日消费', money], ['weekSpend', '本周消费', money], ['monthSpend', '本月消费', money], ['allTimeSpend', '累计消费', money],
    ['requestCount', '请求次数'], ['quotaGroup', '额度组'], ['expiresAt', '余额有效期', absoluteTime], ['trackingSince', '开始跟踪', absoluteTime], ['monthSinceTracking', '本月消费仅覆盖跟踪以来', yesNo],
    ['giftBalance', '赠送余额', money], ['cashBalance', '现金余额', money], ['planUsed', '套餐已用'], ['planLimit', '套餐上限'], ['planPercent', '套餐使用率（%）'], ['planStatus', '套餐状态', x => ({ active: '有效', expired: '已到期' }[x] || x)],
    ['todayTokenTotal', '今日 tokens'], ['todayUsageDate', '今日用量日期'], ['latestModelUsageDate', '最新模型用量日期'], ['todayUsageBasis', '今日用量依据'], ['snapshotDate', '余额快照日期']
  ]));
  (p.balance?.tranches || []).forEach((t, i) => pairs.push(...pairsFor(t, [['amount', '余额', x => quotaAmount(x, t.currency || currency)], ['currency', '单位'], ['expiresAt', '到期', absoluteTime]], '余额批次 ' + (i + 1) + ' · ')));
  pairs.push(...pairsFor(p.resetCredits, [['availableCount', '可用重置券'], ['nextExpiresAt', '最近重置券到期', absoluteTime]]));
  (p.resetCredits?.expirations || []).forEach((t, i) => pairs.push(['重置券有效期 ' + (i + 1), absoluteTime(t)]));
  (p.resetCredits?.grants || []).forEach((g, i) => pairs.push(...pairsFor(g, [
    ['id', '编号'], ['label', '名称'], ['resetsLeft', '剩余次数'], ['resetsTotal', '总次数'], ['startsAt', '开始时间', absoluteTime], ['endsAt', '结束时间', absoluteTime], ['clears', '清除范围', x => x.join(' / ')], ['usableNow', '当前可用', yesNo], ['useRequiresLimit', '使用要求额度受限', yesNo], ['paused', '已暂停', yesNo]
  ], '重置券 ' + (i + 1) + ' · ')));
  // Provider-side observations are shown as their own data, never added into
  // the separate Hub/session summary. Costs retain the provider balance unit.
  pairs.push(...pairsFor(p.usageSummary, [
    ['period', '时段', x => ({ today: '今日', week: '本周', month: '本月', allTime: '累计' }[x] || x)], ['requests', '请求次数'], ['todayTokens', '今日 tokens'], ['weekTokens', '本周 tokens'], ['inputTokens', '输入 tokens'], ['outputTokens', '输出 tokens'],
    ['cacheReadTokens', '缓存读取 tokens'], ['cacheCreationTokens', '缓存写入 tokens'], ['totalTokens', '总 tokens'], ['standardCost', '标准费用', money], ['actualCost', '实际费用', money], ['averageDurationMs', '平均耗时（ms）']
  ], '服务商用量 · '));
  return pairs;
}
export function quotaWindowPairs(p, w) {
  if (!w) return providerQuotaPairs(p);
  const reading = quotaPresentation(p, w);
  const pairs = [['窗口', reading.title], [reading.label, reading.text || quotaAmount(reading.value, reading.unit)]];
  pairs.push(...pairsFor(w, [
    ['kind', '周期', x => kindNames[x] || x], ['metric', '额度类型', x => ({ credits: '余额 / 点数', spend: '消费额' }[x] || x)], ['used', reading.type === 'overage' ? '超额已用点数' : '已用'], ['limit', '上限'], ['remaining', reading.type === 'overage' ? '预估费用（USD）' : '剩余'],
    ['usedPercent', '已用（%）'], ['remainingPercent', '剩余（%）'], ['currency', '单位'], ['resetsAt', reading.boundary || '边界时间', absoluteTime], ['windowMinutes', '周期时长（分钟）'], ['resetDescription', '重置说明'], ['boundaryKind', '边界类型', x => boundaryNames[x] || x],
    ['limitId', '额度窗口标识'], ['additional', '附加额度', yesNo], ['source', '窗口来源', x => sourceNames[x] || x], ['showMeter', '上游提供刻度图', yesNo], ['detail', '窗口说明']
  ]));
  return pairs.concat(providerQuotaPairs(p));
}
