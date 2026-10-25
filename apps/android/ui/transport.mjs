import { publicConnection } from './connection.mjs';
export function nativeConnection(bridge) {
  return publicConnection(JSON.parse(bridge.getConnection()));
}
export function saveNativeConnection(bridge, payload) {
  const result = JSON.parse(bridge.saveConnection(JSON.stringify(payload)));
  if (!result.ok) throw new Error(result.error === 'invalid_connection' ? '地址或密钥格式不正确，请检查后重试。' : '连接配置未保存，请重试。');
  return result.connection ? publicConnection(result.connection) : nativeConnection(bridge);
}
export function createTransport({ fetcher = (...args) => fetch(...args) } = {}) {
  const active = new Set();
  return {
    cancel() { for (const controller of active) controller.abort(); active.clear(); },
    async getJSON(path) {
      const controller = new AbortController(); active.add(controller);
      const timeout = setTimeout(() => controller.abort(), 10000);
      try {
        // Android supplies credentials to the configured source; the UI never reads them.
        const response = await fetcher(path, { signal: controller.signal, cache: 'no-store', credentials: 'omit' });
        if (!response.ok) {
          let body = {};
          try { body = await response.json(); } catch (_) { /* Keep fallback. */ }
          const error = new Error(typeof body.message === 'string' ? body.message : response.status === 401 ? (path === '/api/session-archive' ? '归档认证失败，请更新查看令牌。' : 'Hub 认证失败，请更新密钥。') : '服务暂时不可用');
          error.status = response.status; error.code = body.error;
          if (body.error === 'hub_unauthorized') error.message = 'Hub 认证失败，请更新密钥。';
          if (body.error === 'archive_unauthorized') error.message = '归档认证失败，请更新归档查看令牌。';
          error.kind = response.status === 401 || body.error === 'hub_unauthorized' || body.error === 'archive_unauthorized' ? '认证失败' : response.status >= 500 ? '上游或服务错误' : '读取失败'; throw error;
        }
        return await response.json();
      } finally { clearTimeout(timeout); active.delete(controller); }
    }
  };
}

// Keep only public request identity while native code tests the transient input.
// Late replies cannot confirm edited fields or a different source.
export function createConnectionTester(bridge, onResult, { setTimer = setTimeout, clearTimer = clearTimeout, timeoutMs = 12000 } = {}) {
  let sequence = 0, pending = null;
  const cancel = () => {
    if (!pending) return;
    clearTimer(pending.timer); pending = null;
    try { bridge.cancelConnectionTest?.(); } catch (_) { /* Identity is invalidated even on older bridges. */ }
  };
  return {
    get pending() { return pending !== null; },
    cancel,
    start(payload, origin) {
      if (pending) return false;
      if (typeof bridge.testConnection !== 'function') return false;
      const id = 'connection-' + (++sequence);
      const source = payload.source;
      pending = { id, source, origin, timer: null };
      pending.timer = setTimer(() => {
        if (pending?.id !== id) return;
        cancel(); onResult({ id, source, origin, ok: false, error: 'timeout' });
      }, timeoutMs);
      try { bridge.testConnection(JSON.stringify(payload), id); }
      catch (_) { cancel(); onResult({ id, source, origin, ok: false, error: 'network_error' }); }
      return true;
    },
    accept(detail) {
      if (!pending || !detail || detail.id !== pending.id || detail.source !== pending.source || (detail.ok === true && detail.origin !== pending.origin)) return false;
      clearTimer(pending.timer); pending = null;
      // Do not forward any native fields that could contain response/secret data.
      onResult({ id: detail.id, source: detail.source, origin: detail.origin, ok: detail.ok === true, error: typeof detail.error === 'string' ? detail.error : '', latencyMs: detail.latencyMs });
      return true;
    }
  };
}
