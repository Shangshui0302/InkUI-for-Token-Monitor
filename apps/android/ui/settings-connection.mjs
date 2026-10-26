import { connectionPayload, connectionTestMessage, normalizeConnectionAddress, sourceKey } from './connection.mjs';
import { createConnectionTester } from './transport.mjs';

const offlineErrors = new Set(['timeout', 'host_not_found', 'connection_refused', 'network_error']);
const testErrors = new Set([...offlineErrors, 'invalid_connection', 'unconfigured', 'unauthorized', 'invalid_response', 'http_error', 'redirect_blocked', 'cancelled', 'connection_changed']);

export function settingsConnectionPayload(draft) {
  if (draft?.source !== 'hub') throw new Error('这里只支持 Hub 连接。');
  const address = normalizeConnectionAddress(draft.url);
  if (address.error) throw new Error(address.error);
  if (!address.value) throw new Error('请先填写 Hub 地址。');
  const secret = draft.removeSecret ? '' : (draft.secret ?? '');
  if (typeof secret !== 'string' || secret.length > 8192 || /[^\x20-\x7e]/.test(secret))
    throw new Error('密钥格式无效，请检查后重试。');
  return { payload: connectionPayload(address.value, secret, draft.removeSecret), origin: address.origin, source: 'hub' };
}

// Candidate credentials live only in this closure until save, cancellation or a
// non-recoverable test failure. Public state contains a fixed, safe vocabulary.
export function createSettingsConnectionFlow({ bridge, getConfig, save, onState = () => {}, testOptions }) {
  let candidate = null;
  let state = Object.freeze({ phase: 'idle', source: 'hub', message: '', canSaveOffline: false });
  const publish = next => { state = Object.freeze(next); onState(state); };
  const fail = (source, error, message = connectionTestMessage({ source, error }), canSaveOffline = false) => {
    publish({ phase: 'error', source, message, error, canSaveOffline });
  };
  const commit = (offline = false) => {
    const current = candidate;
    if (!current) return false;
    candidate = null;
    try {
      if (sourceKey(getConfig()) !== current.savedKey) {
        fail(current.source, 'connection_changed');
        return false;
      }
      save(current.payload);
      publish({ phase: 'saved', source: current.source, verified: !offline, message: offline ? '已保存，待连接' : '已保存并连接', canSaveOffline: false });
      return true;
    } catch (_) {
      fail(current.source, 'connection_save_failed', '连接配置未保存，请重试。');
      return false;
    }
  };
  const tester = createConnectionTester(bridge, result => {
    if (!candidate) return;
    if (result.ok) { commit(); return; }
    const error = testErrors.has(result.error) ? result.error : 'test_failed';
    const canSaveOffline = offlineErrors.has(error);
    if (!canSaveOffline) candidate = null;
    fail(result.source, error, undefined, canSaveOffline);
  }, testOptions);
  return {
    get pending() { return tester.pending; },
    get state() { return state; },
    submit(draft) {
      if (tester.pending) return false;
      candidate = null;
      let next, savedKey, config;
      try {
        config = getConfig();
        savedKey = sourceKey(config);
      } catch (_) {
        fail('hub', 'connection_save_failed', '连接配置无法读取，请重试。');
        return false;
      }
      try { next = settingsConnectionPayload(draft); }
      catch (error) {
        fail('hub', 'invalid_connection', error.message);
        return false;
      }
      candidate = { ...next, savedKey };
      publish({ phase: 'testing', source: next.source, message: '正在测试连接…', canSaveOffline: false });
      if (!tester.start({ ...next.payload, source: next.source }, next.origin)) {
        candidate = null;
        fail(next.source, 'test_unavailable', '当前版本无法测试连接，请更新应用后重试。');
        return false;
      }
      return true;
    },
    saveOffline() {
      if (tester.pending || !state.canSaveOffline) return false;
      return commit(true);
    },
    cancel() {
      candidate = null;
      tester.cancel();
      publish({ phase: 'idle', source: state.source, message: '', canSaveOffline: false });
    },
    accept(detail) { return tester.accept(detail); }
  };
}
