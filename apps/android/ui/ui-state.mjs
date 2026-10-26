import { providerOrderIds } from './provider-order.mjs';
const prefix = 'inkui.reading-state.v1:';
const choices = (value, allowed, fallback) => allowed.includes(value) ? value : fallback;
export function normalizeReadingState(value = {}) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) value = {};
  return {
    page: choices(value.page, ['limits', 'sessions', 'activity', 'details'], 'limits'),
    period: choices(value.period, ['today', 'month', 'allTime'], 'today'),
    activityKind: choices(value.activityKind, ['heat', 'trend', 'day', 'month'], 'trend'),
    detailKind: choices(value.detailKind, ['usage', 'models', 'clients', 'projects', 'devices', 'source'], 'usage'),
    runningOnly: value.runningOnly === true,
    providerId: typeof value.providerId === 'string' && /^provider-state:v1:[a-f0-9]{16}$/.test(value.providerId) ? value.providerId : null,
    index: Number.isInteger(value.index) && value.index >= 0 ? Math.min(value.index, 1000000) : 0
  };
}
function fingerprint(value) {
  let a = 2166136261, b = 3339675911;
  for (let i = 0; i < value.length; i++) {
    a = Math.imul(a ^ value.charCodeAt(i), 16777619);
    b = Math.imul(b ^ value.charCodeAt(i), 2246822519);
  }
  return 'provider-state:v1:' + [a, b].map(n => (n >>> 0).toString(16).padStart(8, '0')).join('');
}
// Reuse the account-order identity policy, but persist only an opaque fingerprint
// in reading state. Account labels, emails, quota data and source indices stay out.
export function readingProviderIds(providers) {
  const ids = providerOrderIds(providers).map(id => id === null ? null : fingerprint(id));
  const counts = new Map();
  ids.forEach(id => { if (id) counts.set(id, (counts.get(id) || 0) + 1); });
  return ids.map(id => id && counts.get(id) === 1 ? id : null);
}
export function restoreReadingProvider(providerId, providers) {
  if (!providerId) return null;
  const ids = readingProviderIds(providers), index = ids.indexOf(providerId);
  return index < 0 ? null : index;
}
function storageKey(hubUrl) {
  try {
    const url = new URL(hubUrl);
    return ['http:', 'https:'].includes(url.protocol) && url.hostname ? prefix + url.origin : null;
  } catch (_) { return null; }
}
export function createReadingStateStore(storage) {
  return {
    load(hubUrl) {
      const key = storageKey(hubUrl);
      if (!key) return normalizeReadingState();
      try { return normalizeReadingState(JSON.parse(storage.getItem(key))); } catch (_) { return normalizeReadingState(); }
    },
    save(hubUrl, state) {
      const key = storageKey(hubUrl);
      if (!key) return false;
      try {
        const payload = JSON.stringify(normalizeReadingState(state));
        // Polls and layout-only changes never churn persistent storage.
        if (storage.getItem(key) !== payload) storage.setItem(key, payload);
        return storage.getItem(key) === payload;
      } catch (_) { return false; }
    }
  };
}
