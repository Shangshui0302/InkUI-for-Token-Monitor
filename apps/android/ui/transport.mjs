import { publicConnection } from './connection.mjs';
const hubErrors = {
  hub_unauthorized: ['认证失败', 'Hub 认证失败，请在连接设置中更新密钥。'],
  hub_timeout: ['读取超时', 'Hub 响应超时，请检查电脑是否在线、端口和防火墙。'],
  hub_host_not_found: ['找不到主机', '无法解析 Hub 主机名。请检查名称或 DNS，当前网络不支持时可使用电脑的局域网 IP。'],
  hub_connection_refused: ['连接被拒绝', '电脑拒绝连接，请确认 Hub 已启动、端口正确，并允许局域网访问。'],
  hub_network_error: ['连接失败', '无法连接 Hub，请检查设备网络、电脑地址和防火墙。'],
  hub_unavailable: ['连接失败', '无法连接 Hub，请检查设备网络、电脑地址和防火墙。'],
  upstream_redirect_blocked: ['地址需要修改', 'Hub 要求跳转，请在连接设置中填写最终地址。'],
  invalid_response: ['数据格式错误', '返回的数据不符合 Hub 接口，请确认地址指向 Token Monitor Hub。'],
  response_too_large: ['数据过大', 'Hub 返回的数据超过读取上限，请检查 Hub 后重试。']
};
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
      if (!['/api/stats', '/api/history'].includes(path)) throw new Error('不支持的 Hub 接口。');
      const controller = new AbortController(); active.add(controller);
      const timeout = setTimeout(() => controller.abort(), 10000);
      try {
        // Android supplies credentials to the configured source; the UI never reads them.
        const response = await fetcher(path, { signal: controller.signal, cache: 'no-store', credentials: 'omit' });
        if (!response.ok) {
          let body = {};
          try { body = await response.json(); } catch (_) { /* Keep fallback. */ }
          const known = hubErrors[body.error];
          const error = new Error(known?.[1] || (response.status === 401 ? 'Hub 认证失败，请更新密钥。' : '服务暂时不可用'));
          error.status = response.status; error.code = body.error;
          error.kind = known?.[0] || (response.status === 401 ? '认证失败' : response.status >= 500 ? '上游或服务错误' : '读取失败'); throw error;
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
      if (pending || payload?.source !== 'hub') return false;
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
