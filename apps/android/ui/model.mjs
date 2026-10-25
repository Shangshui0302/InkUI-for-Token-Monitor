const statuses = {
  ok: '可用',
  disabled: '已停用',
  notConfigured: '未配置',
  unauthorized: '来源认证失败',
  rateLimited: '额度受限',
  sourceRateLimited: '采集受限',
  unavailable: '来源不可用',
  error: '采集出错'
};
const names = {
  claude: 'Claude', codex: 'Codex', opencode: 'OpenCode', cursor: 'Cursor',
  antigravity: 'Antigravity', cline: 'Cline', factory: 'Factory Droid', kimi: 'Kimi',
  grok: 'Grok', copilot: 'GitHub Copilot', zed: 'Zed', commandcode: 'Command Code',
  mimo: 'Xiaomi MiMo', zai: 'GLM', zaiteam: 'GLM Team', kiro: 'Kiro',
  workbuddy: 'WorkBuddy', qoder: 'Qoder', deepseek: 'DeepSeek', devin: 'Devin',
  minimax: 'Minimax', typesafe: 'TypeSafe', openrouter: 'OpenRouter',
  volcengine: 'Volcengine', ollama: 'Ollama', trae: 'Trae CN', alibaba: 'Alibaba Cloud',
  stepfun: 'StepFun', notion: 'Notion AI', thirdparty: 'Third-party APIs'
};
const windowNames = { daily: '每日额度', weekly: '每周额度', session: '会话额度', billing: '账单额度' };
const providerOrder = { codex: 0, deepseek: 1, antigravity: 2 };
const finite = value => typeof value === 'number' && Number.isFinite(value);
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const stringOrNull = value => typeof value === 'string' ? value : null;
const numberOrNull = value => finite(value) ? value : null;
const booleanOrNull = value => typeof value === 'boolean' ? value : null;
const pick = (value, fields, convert) => Object.fromEntries(fields.map(key => [key, convert(value[key])]));
const strings = (value, fields) => pick(value, fields.split(' '), stringOrNull);
const numbers = (value, fields) => pick(value, fields.split(' '), numberOrNull);
const booleans = (value, fields) => pick(value, fields.split(' '), booleanOrNull);
const list = (value, project) => Array.isArray(value) ? value.filter(object).map(project) : [];
const windowProjection = window => ({
  ...strings(window, 'kind label source metric limitId boundaryKind resetsAt resetDescription detail currency'),
  ...numbers(window, 'used limit remaining usedPercent remainingPercent windowMinutes'),
  ...booleans(window, 'additional showMeter')
});
const balanceProjection = balance => !object(balance) ? null : ({
  ...strings(balance, 'currency quotaGroup expiresAt trackingSince planStatus todayUsageDate latestModelUsageDate todayUsageBasis snapshotDate'),
  ...numbers(balance, 'amount todaySpend weekSpend monthSpend allTimeSpend requestCount giftBalance cashBalance planUsed planLimit planPercent todayTokenTotal'),
  ...booleans(balance, 'monthSinceTracking'),
  tranches: list(balance.tranches, tranche => ({ ...numbers(tranche, 'amount'), ...strings(tranche, 'currency expiresAt') }))
});
const resetCreditsProjection = credits => !object(credits) ? null : ({
  ...numbers(credits, 'availableCount'), ...strings(credits, 'nextExpiresAt'),
  expirations: Array.isArray(credits.expirations) ? credits.expirations.filter(value => typeof value === 'string') : [],
  grants: list(credits.grants, grant => ({
    ...strings(grant, 'id label startsAt endsAt'), ...numbers(grant, 'resetsLeft resetsTotal'),
    ...booleans(grant, 'usableNow useRequiresLimit paused'),
    clears: Array.isArray(grant.clears) ? grant.clears.filter(value => typeof value === 'string') : []
  }))
});
const usageProjection = usage => !object(usage) ? null : ({
  ...strings(usage, 'period'),
  ...numbers(usage, 'requests todayTokens weekTokens inputTokens outputTokens cacheReadTokens cacheCreationTokens totalTokens standardCost actualCost averageDurationMs')
});
const providerProjection = provider => ({
  ...strings(provider, 'provider adapterId accountLabel accountName accountEmail planLabel workspaceKind plan planType subscriptionPlan status actionRequired source sourceDetail updatedAt region'),
  stale: provider.stale === true,
  ...numbers(provider, 'balanceUsd'),
  balance: balanceProjection(provider.balance),
  resetCredits: resetCreditsProjection(provider.resetCredits),
  usageSummary: usageProjection(provider.usageSummary),
  windows: provider.windows.map(windowProjection)
});
function validateStats(stats) {
  if (!object(stats) || !object(stats.limits) || !Array.isArray(stats.limits.providers) ||
      stats.limits.providers.some(provider => !object(provider) || typeof provider.provider !== 'string' ||
        !Array.isArray(provider.windows) || provider.windows.some(window => !object(window)))) {
    throw new Error('额度数据格式不正确');
  }
}

export function windowValue(window) {
  if (finite(window.remainingPercent)) {
    return {
      value: Math.max(0, Math.min(100, window.remainingPercent)),
      unit: '%',
      valueLabel: '剩余额度'
    };
  }
  if (finite(window.usedPercent)) {
    return {
      value: Math.max(0, Math.min(100, 100 - window.usedPercent)),
      unit: '%',
      valueLabel: '剩余额度'
    };
  }
  if (finite(window.remaining)) {
    return {
      value: window.remaining,
      unit: typeof window.currency === 'string' ? window.currency : '',
      valueLabel: '剩余余额'
    };
  }
  return { value: null, unit: '', valueLabel: '未提供额度' };
}

export function normalizeStats(stats) {
  validateStats(stats);
  const providers = stats.limits.providers.map((raw, index) => {
    const provider = providerProjection(raw);
    return {
      ...provider,
      key: index,
      order: Object.prototype.hasOwnProperty.call(providerOrder, provider.provider) ? providerOrder[provider.provider] : 3,
      name: names[provider.provider] || provider.provider,
      account: provider.accountLabel || '',
      statusCode: provider.status,
      status: statuses[provider.status] || '状态未知',
      ok: provider.status === 'ok',
      source: ['oauth', 'cli', 'web', 'rpc', 'local', 'api'].includes(provider.source) ? provider.source : '',
      windows: provider.windows.map(window => ({
        ...window,
        rawLabel: window.label,
        label: window.label?.trim() ? window.label : windowNames[window.kind] || '额度窗口',
        ...windowValue(window)
      }))
    };
  });
  // Keep independent accounts and upstream window order; index breaks priority ties.
  providers.sort((left, right) => left.order - right.order || left.key - right.key);
  return providers;
}

// Persist only typed display fields. Authenticated Hub responses also contain
// account identifiers and unrelated device/session data that must stay out.
export function projectStatsForCache(stats) {
  validateStats(stats);
  return {
    limits: {
      ...(Object.hasOwn(stats.limits, 'updatedAt') ? { updatedAt: stringOrNull(stats.limits.updatedAt) } : {}),
      ...(Object.hasOwn(stats.limits, 'refreshMs') ? { refreshMs: numberOrNull(stats.limits.refreshMs) } : {}),
      providers: stats.limits.providers.map(providerProjection)
    }
  };
}

export function absoluteTime(value) {
  if (typeof value !== 'string' || !Number.isFinite(Date.parse(value))) {
    return '未提供';
  }
  return new Date(value).toLocaleString('zh-CN', { hour12: false });
}
