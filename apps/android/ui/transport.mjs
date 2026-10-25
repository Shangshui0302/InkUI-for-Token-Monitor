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
