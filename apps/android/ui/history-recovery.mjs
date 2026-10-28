import { createConnectionRecovery } from './connection-recovery.mjs';

export function historyRevisionMatches(history, savedRevision, currentRevision) {
  return Boolean(history && typeof currentRevision === 'string' && currentRevision.length
    && savedRevision === currentRevision);
}

// History failures must not change the cadence or availability of current stats.
// Retry only during a normal refresh after backoff; this owns no extra timer.
export function createHistoryRecovery(now = () => Date.now()) {
  const recovery = createConnectionRecovery();
  let retryAt = 0;
  return {
    ready() { return recovery.automaticAllowed && now() >= retryAt; },
    reset() { recovery.reset(); retryAt = 0; },
    networkChanged() { if (recovery.automaticAllowed) retryAt = 0; },
    failed(error) {
      recovery.failed(error);
      const delay = recovery.delay(60);
      retryAt = delay === null ? Infinity : now() + delay;
    }
  };
}
