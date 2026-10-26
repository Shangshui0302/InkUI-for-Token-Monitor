import { projectStatsForCache } from './model.mjs';

const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const number = value => typeof value === 'number' && Number.isFinite(value) ? value : null;
const string = value => typeof value === 'string' ? value : null;
const numericFields = ['totalTokens', 'costUsd', 'inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens', 'unclassifiedTokens', 'unpricedTokens', 'timedTokens', 'timedOutputTokens', 'timedDurationMs', 'tokensPerSecond', 'outputTokensPerSecond'];
const historyFields = ['tokens', 'cost', 'messages', 'activeTimeMs', 'inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens', 'unclassifiedTokens', 'unpricedTokens', 'tokenIntensity', 'costIntensity', 'intensity'];
const summaryFields = ['totalTokens', 'totalCost', 'activeDays', 'currentStreak', 'longestStreak', 'peakDayTokens', 'messages', 'activeTimeMs'];
function pickNumbers(value, fields) { return Object.fromEntries(fields.map(key => [key, number(value?.[key])])); }
function numericMap(value) {
  return object(value) ? Object.fromEntries(Object.entries(value).filter(([, v]) => number(v) !== null)) : {};
}
function recordMap(value, project) {
  return object(value) ? Object.fromEntries(Object.entries(value).filter(([, v]) => object(v)).map(([k, v]) => [k, project(v)])) : {};
}
function omissionProjection(value) {
  if (typeof value === 'boolean') return value;
  return Object.fromEntries(['today', 'month', 'allTime'].filter(key => number(value?.[key]) !== null).map(key => [key, value[key]]));
}
function omitted(value, period) { return value === true || (object(value) && value[period] > 0); }
function sessionProjection(session) {
  return {
    ...pickNumbers(session, [...numericFields, 'messageCount', 'contextTokens', 'contextWindow']),
    ...Object.fromEntries(['title', 'projectLabel', 'client', 'model', 'lastUsedAt', 'startedAt', 'sessionKind'].map(key => [key, string(session[key])])),
    ...Object.fromEntries(['turnEnded', 'archived', 'deleted', 'sourceDeleted'].map(key => [key, session[key] === true])),
    models: numericMap(session.models), modelCosts: numericMap(session.modelCosts)
  };
}
function periodProjection(period) {
  return {
    ...pickNumbers(period, numericFields),
    models: numericMap(period?.models), modelCosts: numericMap(period?.modelCosts),
    clients: numericMap(period?.clients), clientCosts: numericMap(period?.clientCosts),
    // Project map keys can contain local paths. Replace them with opaque indices;
    // only the explicitly public display label reaches persistent storage.
    projects: Object.fromEntries(Object.values(object(period?.projects) ? period.projects : {}).filter(object).map((p, i) => [String(i), {
      label: string(p.label), tokens: number(p.tokens), costUsd: number(p.costUsd)
    }])),
    sessions: recordMap(period?.sessions, sessionProjection),
    sessionDetailsOmitted: period?.sessionDetailsOmitted === true,
    periodProjectsOmitted: period?.periodProjectsOmitted === true,
    projectsIncomplete: period?.projectsIncomplete === true
  };
}
function historyRecord(row) {
  return {
    ...pickNumbers(row, historyFields),
    tokenComponentsAvailable: typeof row.tokenComponentsAvailable === 'boolean' ? row.tokenComponentsAvailable : null,
    perModel: recordMap(row.perModel, v => pickNumbers(v, historyFields)),
    perClient: recordMap(row.perClient, v => pickNumbers(v, historyFields))
  };
}
export function projectHistoryForCache(raw) {
  if (!object(raw) || !Array.isArray(raw.daily) || !Array.isArray(raw.monthly) || !object(raw.summary)) throw new Error('历史数据格式不正确');
  const daily = raw.daily.map(row => {
    if (!object(row) || typeof row.date !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(row.date)) throw new Error('每日历史数据格式不正确');
    return { date: row.date, ...historyRecord(row) };
  });
  const monthly = raw.monthly.map(row => {
    if (!object(row) || typeof row.month !== 'string' || !/^\d{4}-\d{2}$/.test(row.month)) throw new Error('每月历史数据格式不正确');
    return { month: row.month, ...historyRecord(row) };
  });
  return { daily, monthly, summary: { ...pickNumbers(raw.summary, summaryFields), favoriteModel: string(raw.summary.favoriteModel) } };
}
export function projectDashboardStats(raw) {
  if (!object(raw) || !object(raw.periods)) throw new Error('用量数据格式不正确');
  const limits = raw.limits ? projectStatsForCache(raw).limits : { providers: [] };
  return {
    updatedAt: string(raw.updatedAt),
    periods: Object.fromEntries(['today', 'month', 'allTime'].map(key => [key, periodProjection(raw.periods[key])])),
    devices: (Array.isArray(raw.devices) ? raw.devices : []).filter(object).map(device => ({
      hostname: string(device.hostname), platform: string(device.platform), osName: string(device.osName),
      updatedAt: string(device.updatedAt), receivedAt: string(device.receivedAt), stale: device.stale === true,
      periods: Object.fromEntries(['today', 'month', 'allTime'].map(key => [key, pickNumbers(device.periods?.[key], numericFields)]))
    })),
    historyPreview: raw.historyPreview ? projectHistoryForCache(raw.historyPreview) : null,
    historyRevision: string(raw.historyRevision), staleAfterMs: number(raw.staleAfterMs),
    sessionDetailsOmitted: omissionProjection(raw.sessionDetailsOmitted), periodProjectsOmitted: omissionProjection(raw.periodProjectsOmitted),
    projectsIncomplete: raw.projectsIncomplete === true, limits
  };
}
function summaryView(period) {
  const result = pickNumbers(period, numericFields.filter(key => !key.startsWith('timed')));
  const duration = number(period.timedDurationMs);
  for (const [key, numerator] of [['tokensPerSecond', 'timedTokens'], ['outputTokensPerSecond', 'timedOutputTokens']]) {
    if (result[key] === null && duration > 0 && number(period[numerator]) !== null) result[key] = period[numerator] * 1000 / duration;
  }
  result.costPartial = result.unpricedTokens > 0;
  return result;
}
function distribution(tokens, costs, total, totalCost) {
  const rows = Object.entries(tokens).map(([name, count]) => ({ name, tokens: count, share: total > 0 ? count / total : total === 0 ? 0 : null, costUsd: number(costs[name]) }));
  const sum = rows.reduce((n, row) => n + row.tokens, 0);
  const missing = total !== null ? Math.max(0, total - sum) : 0;
  if (missing > 0) {
    const knownCost = Object.values(costs).reduce((n, value) => n + value, 0);
    rows.push({ name: '未分类', tokens: missing, share: total > 0 ? missing / total : 0, costUsd: totalCost === null ? null : Math.max(0, totalCost - knownCost) });
  }
  return rows.sort((a, b) => b.tokens - a.tokens || a.name.localeCompare(b.name));
}
function sessionsView(sessions, now) {
  return Object.entries(sessions).filter(([, s]) => s.sessionKind !== 'background-review' && Number.isFinite(Date.parse(s.lastUsedAt || s.startedAt || ''))).map(([key, s]) => {
    const archived = s.archived || s.deleted || s.sourceDeleted;
    const recent = Number.isFinite(Date.parse(s.lastUsedAt || '')) && now - Date.parse(s.lastUsedAt) <= 600_000;
    const state = archived || !recent ? 'idle' : s.turnEnded ? 'ended' : 'running';
    const hasContext = state !== 'idle' && s.contextTokens > 0 && s.contextWindow > 0;
    const models = Object.entries(s.models || {}).sort((a, b) => b[1] - a[1]);
    return {
      key, title: s.title || '未命名会话', project: s.projectLabel || '未分类', client: s.client || '未知客户端',
      model: s.model || (models.length ? models.map(([name]) => name).join(' / ') : '未知模型'),
      ...pickNumbers(s, ['totalTokens', 'costUsd', 'inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens', 'messageCount', 'unpricedTokens']),
      lastUsedAt: s.lastUsedAt || s.startedAt,
      contextPercent: hasContext ? 100 - Math.max(0, Math.round((s.contextWindow - s.contextTokens) / s.contextWindow * 100)) : null,
      contextTokens: hasContext ? s.contextTokens : null, contextWindow: hasContext ? s.contextWindow : null,
      state, archived: Boolean(archived), costPartial: s.unpricedTokens > 0
    };
  }).sort((a, b) => Date.parse(b.lastUsedAt) - Date.parse(a.lastUsedAt));
}
function localDateKey(now) {
  const date = new Date(now);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}
function historyView(stats, history, now) {
  const source = history ? 'full' : stats.historyPreview ? 'preview' : 'none';
  const value = history ? projectHistoryForCache(history) : stats.historyPreview;
  if (!value) return { daily: [], monthly: [], summary: {}, available: false, source };
  const daily = value.daily.map(row => ({ ...row }));
  const date = localDateKey(now);
  const today = stats.periods.today;
  const updatedMs = Date.parse(stats.updatedAt || '');
  const currentSnapshot = Number.isFinite(updatedMs) && localDateKey(updatedMs) === date;
  if (currentSnapshot && (today.totalTokens !== null || today.costUsd !== null)) {
    let row = daily.find(row => row.date === date);
    if (!row) { row = { date }; daily.push(row); }
    row.tokens = today.totalTokens; row.cost = today.costUsd;
  }
  daily.sort((a, b) => a.date.localeCompare(b.date));
  const maxTokens = daily.reduce((max, row) => Math.max(max, row.tokens || 0), 0);
  return { daily, monthly: value.monthly, summary: { ...value.summary,
    activeDays: value.summary.activeDays,
    peakDayTokens: Math.max(value.summary.peakDayTokens || 0, maxTokens)
  }, available: true, source };
}
export function dashboardView(raw, history = null, period = 'today', now = Date.now()) {
  if (!['today', 'month', 'allTime'].includes(period)) throw new Error('未知用量时段');
  const stats = projectDashboardStats(raw);
  const value = stats.periods[period];
  const summary = summaryView(value);
  const sessions = sessionsView(value.sessions, now);
  const liveSource = { ...stats.periods.today.sessions, ...stats.periods.month.sessions };
  const recent = sessionsView(liveSource, now);
  const projects = Object.values(value.projects).map(p => ({ name: p.label || '未分类', tokens: p.tokens, costUsd: p.costUsd,
    share: summary.totalTokens > 0 && p.tokens !== null ? p.tokens / summary.totalTokens : summary.totalTokens === 0 ? 0 : null
  })).sort((a, b) => (b.tokens ?? -1) - (a.tokens ?? -1));
  return { period, summary,
    models: distribution(value.models, value.modelCosts, summary.totalTokens, summary.costUsd),
    clients: distribution(value.clients, value.clientCosts, summary.totalTokens, summary.costUsd), projects,
    devices: stats.devices.map(d => ({ name: d.hostname || '未命名设备', platform: d.osName || d.platform || '未知平台',
      stale: d.stale || (stats.staleAfterMs > 0 && Number.isFinite(Date.parse(d.receivedAt || d.updatedAt)) && now - Date.parse(d.receivedAt || d.updatedAt) > stats.staleAfterMs),
      updatedAt: d.updatedAt, totalTokens: d.periods[period].totalTokens, costUsd: d.periods[period].costUsd })),
    sessions, runningSessions: recent.filter(s => s.state === 'running'),
    recentSessions: recent.filter((s, i) => i < 5 || s.state === 'running'),
    sessionsUnavailable: period === 'allTime' && sessions.length === 0,
    sessionsIncomplete: omitted(stats.sessionDetailsOmitted, period) || value.sessionDetailsOmitted,
    projectsIncomplete: stats.projectsIncomplete || omitted(stats.periodProjectsOmitted, period) || value.projectsIncomplete || value.periodProjectsOmitted,
    history: historyView(stats, history, now)
  };
}
