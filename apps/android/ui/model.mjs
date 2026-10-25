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
const names = { codex: 'Codex', deepseek: 'DeepSeek', antigravity: 'Antigravity' };
const windowNames = { weekly: '每周额度', session: '会话额度', billing: '账单额度' };
const providerOrder = { codex: 0, deepseek: 1, antigravity: 2 };
const finite = value => typeof value === 'number' && Number.isFinite(value);
const stringOrNull = value => typeof value === 'string' ? value : null;
const numberOrNull = value => finite(value) ? value : null;

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
  if (!stats || typeof stats !== 'object' || Array.isArray(stats) ||
      !stats.limits || !Array.isArray(stats.limits.providers)) {
    throw new Error('额度数据格式不正确');
  }
  const providers = stats.limits.providers.map((provider, index) => {
    if (!provider || typeof provider !== 'object' ||
        typeof provider.provider !== 'string' || !Array.isArray(provider.windows)) {
      throw new Error('额度数据格式不正确');
    }
    return {
      key: index,
      order: Object.prototype.hasOwnProperty.call(providerOrder, provider.provider) ? providerOrder[provider.provider] : 3,
      name: names[provider.provider] || provider.provider,
      account: typeof provider.accountLabel === 'string' ? provider.accountLabel : '',
      status: statuses[provider.status] || '状态未知',
      ok: provider.status === 'ok',
      stale: provider.stale === true,
      updatedAt: provider.updatedAt,
      source: provider.source === 'oauth' ? 'OAuth' : provider.source === 'api' ? 'API' : '',
      windows: provider.windows.map(window => {
        if (!window || typeof window !== 'object') {
          throw new Error('额度数据格式不正确');
        }
        return {
          label: typeof window.label === 'string' && window.label.trim() ? window.label : windowNames[window.kind] || '额度窗口',
          ...windowValue(window),
          resetsAt: window.resetsAt
        };
      })
    };
  });
  // The input index breaks ties explicitly, preserving multiple accounts and
  // unknown-provider ordering even without relying on stable Array.sort.
  providers.sort((left, right) => left.order - right.order || left.key - right.key);
  return providers;
}

// Only this explicit projection is persisted; the Hub snapshot can contain
// unrelated account identifiers, devices and usage history.
export function projectStatsForCache(stats) {
  normalizeStats(stats);
  return {
    limits: {
      providers: stats.limits.providers.map(provider => ({
        provider: provider.provider,
        accountLabel: stringOrNull(provider.accountLabel),
        status: stringOrNull(provider.status),
        stale: provider.stale === true,
        updatedAt: stringOrNull(provider.updatedAt),
        source: stringOrNull(provider.source),
        windows: provider.windows.map(window => ({
          kind: stringOrNull(window.kind),
          label: stringOrNull(window.label),
          remainingPercent: numberOrNull(window.remainingPercent),
          usedPercent: numberOrNull(window.usedPercent),
          remaining: numberOrNull(window.remaining),
          currency: stringOrNull(window.currency),
          resetsAt: stringOrNull(window.resetsAt)
        }))
      }))
    }
  };
}

export function absoluteTime(value) {
  if (typeof value !== 'string' || !Number.isFinite(Date.parse(value))) {
    return '未提供';
  }
  return new Date(value).toLocaleString('zh-CN', { hour12: false });
}
