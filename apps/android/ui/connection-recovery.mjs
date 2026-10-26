const retryDelays = [15_000, 30_000, 60_000, 120_000, 300_000];
const needsEditing = new Set(['hub_unauthorized', 'upstream_redirect_blocked', 'invalid_response',
  'response_too_large', 'hub_unconfigured', 'request_blocked']);

// No endpoints or credentials live here. Recovery always uses the native saved Hub.
export function createConnectionRecovery() {
  let failures = 0, blocked = false;
  return {
    get automaticAllowed() { return !blocked; },
    reset() { failures = 0; blocked = false; },
    failed(error = {}) {
      blocked = needsEditing.has(error.code) || error.status === 401 || error.status === 403
        || error instanceof SyntaxError;
      failures = Math.min(failures + 1, retryDelays.length);
    },
    delay(intervalSeconds) {
      if (blocked || ![60, 300].includes(intervalSeconds)) return null;
      return failures ? retryDelays[failures - 1] : intervalSeconds * 1000;
    }
  };
}

// Validation of public internet access is deliberately irrelevant to a LAN Hub.
// The native revision is an opaque change counter, never an SSID or IP address.
export function networkRecovered(previous, next) {
  if (next.network === 'offline' || next.network === 'unknown') return false;
  return previous.network !== next.network || Boolean(previous.networkRevision && next.networkRevision
    && previous.networkRevision !== next.networkRevision);
}
