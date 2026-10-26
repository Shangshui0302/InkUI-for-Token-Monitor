const STORAGE_PREFIX = 'inkui.provider-order.v1:';
const text = value => typeof value === 'string' ? value.trim() : '';

// Identity deliberately excludes quota, plan, status, source position and secrets.
// Preserve case: two distinct upstream identities must never collapse silently.
// When the preferred identity changes, the account is treated as a new account.
export function providerOrderIds(rawProviders) {
  if (!Array.isArray(rawProviders)) return [];
  const ids = rawProviders.map(provider => {
    if (!provider || typeof provider !== 'object' || Array.isArray(provider)) return null;
    const identityField = ['accountEmail', 'accountName', 'accountLabel'].find(key => text(provider[key]));
    const providerId = text(provider.provider), adapterId = text(provider.adapterId);
    if (!providerId && !adapterId) return null;
    return 'provider:v1:' + JSON.stringify([
      providerId, adapterId, text(provider.region), identityField || '', identityField ? text(provider[identityField]) : ''
    ]);
  });
  const counts = new Map();
  for (const id of ids) if (id !== null) counts.set(id, (counts.get(id) || 0) + 1);
  // All members of an ambiguous identity are unsortable, including the first.
  return ids.map(id => id !== null && counts.get(id) === 1 ? id : null);
}

function uniqueIds(value) {
  return Array.isArray(value) ? [...new Set(value.filter(id => typeof id === 'string' && id.trim()))] : [];
}

// Saved known accounts lead; newly seen and ambiguous accounts follow in source
// order. Every source index appears exactly once, even with corrupt saved data.
export function orderedProviderIndices(rawProviders, savedIds) {
  const ids = providerOrderIds(rawProviders), positions = new Map();
  ids.forEach((id, index) => { if (id !== null) positions.set(id, index); });
  const ordered = uniqueIds(savedIds).filter(id => positions.has(id)).map(id => positions.get(id));
  const used = new Set(ordered);
  return ordered.concat(ids.map((_, index) => index).filter(index => !used.has(index)));
}

// Replace only the slots belonging to currently visible saved accounts. Missing
// accounts keep their existing slots, so reconnecting accounts recover their
// position. Fill visible slots from the full edited sequence (including new
// accounts), then append its remainder. Thus the visible subsequence exactly
// matches the edit, including a new account moved to the front or middle. Nulls,
// non-strings and repeated IDs are ignored; neither input is modified.
export function mergeProviderOrder(savedIds, visibleIds) {
  const saved = uniqueIds(savedIds), visible = uniqueIds(visibleIds);
  const visibleSet = new Set(visible);
  let cursor = 0;
  return saved.map(id => visibleSet.has(id) ? visible[cursor++] : id)
    .concat(visible.slice(cursor));
}

function storageKey(hubUrl) {
  if (typeof hubUrl !== 'string' || !hubUrl.trim()) return null;
  try {
    const url = new URL(hubUrl.trim());
    return ['http:', 'https:'].includes(url.protocol) && url.hostname
      ? STORAGE_PREFIX + url.origin : null;
  } catch (_) { return null; }
}

// One JSON array per normalized Hub origin. Credentials, paths, query strings,
// Connection revisions never enter keys or stored payloads.
// Inject storage to avoid reading real browser settings during tests.
export function createProviderOrderStore(storage) {
  return {
    load(hubUrl) {
      const key = storageKey(hubUrl);
      if (key === null) return [];
      try { return uniqueIds(JSON.parse(storage.getItem(key))); } catch (_) { return []; }
    },
    save(hubUrl, ids) {
      const key = storageKey(hubUrl);
      if (key === null || !Array.isArray(ids)) return false;
      try {
        const payload = JSON.stringify(uniqueIds(ids));
        storage.setItem(key, payload);
        return storage.getItem(key) === payload;
      } catch (_) { return false; }
    },
    reset(hubUrl) {
      const key = storageKey(hubUrl);
      if (key === null) return false;
      try { storage.removeItem(key); return storage.getItem(key) === null; } catch (_) { return false; }
    }
  };
}
