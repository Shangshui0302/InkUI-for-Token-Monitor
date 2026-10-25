// The web layer only receives the public native connection projection.
export function publicConnection(value = {}) {
  return Object.freeze({
    hubUrl: typeof value.hubUrl === 'string' ? value.hubUrl : '',
    hasHubSecret: value.hasHubSecret === true,
    archiveUrl: typeof value.archiveUrl === 'string' ? value.archiveUrl : '',
    hasArchiveSecret: value.hasArchiveSecret === true,
    revision: typeof value.revision === 'string' ? value.revision : ''
  });
}
export function connectionPayload(hubUrl, hubInput, clearHub, archiveUrl, archiveInput, clearArchive) {
  // Never retain secret text in state, storage, status, or the returned projection.
  return {
    hubUrl: hubUrl.trim(), hubSecret: clearHub ? '' : hubInput || null,
    archiveUrl: archiveUrl.trim(), archiveSecret: clearArchive ? '' : archiveInput || null
  };
}
export function sourceKey(connection) {
  return JSON.stringify([connection.hubUrl, connection.archiveUrl, connection.revision]);
}
export function createSourceFence(initial = '') {
  let key = initial, epoch = 0;
  return {
    capture: () => epoch,
    accepts: value => value === epoch,
    invalidate: () => ++epoch,
    change(next) { if (next === key) return false; key = next; epoch++; return true; }
  };
}
