// Compare the current page's inputs before building or measuring a replacement
// tree. Only the last signature is retained by the dashboard, in memory.
const summaryFields = ['totalTokens', 'costUsd', 'inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens', 'unclassifiedTokens', 'unpricedTokens', 'timedTokens', 'timedOutputTokens', 'timedDurationMs', 'tokensPerSecond', 'outputTokensPerSecond'];
const pick = (value, fields) => fields.map(key => value?.[key]);
const parsedTime = value => typeof value === 'string' ? Date.parse(value) : NaN;
const localDate = now => {
  const d = new Date(now);
  return [d.getFullYear(), d.getMonth(), d.getDate()].join('-');
};

export function resetCountdown(value, now = Date.now()) {
  const at = Date.parse(value);
  if (!Number.isFinite(at)) return '';
  const minutes = Math.max(0, Math.ceil((at - now) / 60000));
  if (!minutes) return '待重置';
  if (minutes >= 1440) return Math.floor(minutes / 1440) + '天' + (Math.floor(minutes % 1440 / 60) ? Math.floor(minutes % 1440 / 60) + '时' : '') + '后';
  return minutes >= 60 ? Math.floor(minutes / 60) + '时' + (minutes % 60 ? minutes % 60 + '分' : '') + '后' : minutes + '分后';
}

function sessionTime(sessions, now) {
  return Object.values(sessions || {}).map(s => now - parsedTime(s.lastUsedAt) <= 600_000);
}
function runningCount(stats, now) {
  return Object.values({ ...stats.periods?.today?.sessions, ...stats.periods?.month?.sessions }).filter(s =>
    s.sessionKind !== 'background-review' && s.archived !== true && s.deleted !== true && s.sourceDeleted !== true && s.turnEnded !== true &&
    now - parsedTime(s.lastUsedAt) <= 600_000).length;
}
function historyInput(stats, history, now) {
  const current = localDate(Date.parse(stats.updatedAt || '')) === localDate(now);
  return [history || stats.historyPreview, Boolean(history), localDate(now), current,
    current ? pick(stats.periods?.today, ['totalTokens', 'costUsd']) : null];
}
function scope(stats, period) {
  return [stats.sessionDetailsOmitted, stats.periodProjectsOmitted, stats.projectsIncomplete,
    pick(stats.periods?.[period], ['sessionDetailsOmitted', 'periodProjectsOmitted', 'projectsIncomplete'])];
}

export function dashboardPageInput(stats, history, state, now = Date.now()) {
  if (state.orderEditor) return state.orderEditor;
  if (state.drill) return state.drill;
  if (!stats) return null;
  const value = stats.periods?.[state.period] || {};
  if (state.page === 'limits') {
    const providers = stats.limits?.providers || [];
    const visible = state.providerKey === null ? providers : [providers[state.providerKey]].filter(Boolean);
    const countdowns = state.providerPicker ? [] : visible.map(p => [
      ...(p.windows || []).map(w => resetCountdown(w.resetsAt, now)),
      resetCountdown(p.balance?.expiresAt, now), resetCountdown(p.resetCredits?.nextExpiresAt, now)
    ]);
    // Include detail-only provider fields so an unchanged-looking card cannot
    // keep an old click closure when a new account snapshot arrives.
    return [providers, countdowns];
  }
  if (state.page === 'sessions') return [value.sessions, sessionTime(value.sessions, now), scope(stats, state.period)];
  if (state.page === 'activity') return historyInput(stats, history, now);
  if (state.detailKind === 'devices') return [stats.devices, stats.staleAfterMs, (stats.devices || []).map(d =>
    stats.staleAfterMs > 0 && now - Date.parse(d.receivedAt || d.updatedAt || '') > stats.staleAfterMs)];
  if (state.detailKind === 'usage') return [pick(value, summaryFields), runningCount(stats, now)];
  if (state.detailKind === 'models') return pick(value, ['models', 'modelCosts', 'totalTokens', 'costUsd']);
  if (state.detailKind === 'clients') return pick(value, ['clients', 'clientCosts', 'totalTokens', 'costUsd']);
  if (state.detailKind === 'projects') return pick(value, ['projects', 'totalTokens', 'costUsd']);
  // The source buttons read the latest snapshot lazily on click. Only the
  // history total is visible while the source details remain closed.
  return (history || stats.historyPreview)?.summary?.totalTokens ?? null;
}

export function dashboardRenderSignature(stats, history, state, now = Date.now()) {
  return JSON.stringify([state, dashboardPageInput(stats, history, state, now)]);
}

// One projected model is enough for pagination, resize and theme changes. Its
// clock token covers every time-derived field in dashboardView, without using
// minute buckets which can miss a boundary crossed within the same minute.
export function dashboardViewClock(stats, now = Date.now()) {
  return JSON.stringify([localDate(now),
    ...['today', 'month', 'allTime'].map(period => sessionTime(stats.periods?.[period]?.sessions, now)),
    (stats.devices || []).map(d => stats.staleAfterMs > 0 && now - Date.parse(d.receivedAt || d.updatedAt || '') > stats.staleAfterMs)]);
}
