// The web layer only receives the public native connection projection.
export function publicConnection(value = {}) {
  return Object.freeze({
    hubUrl: typeof value.hubUrl === 'string' ? value.hubUrl : '',
    hasHubSecret: value.hasHubSecret === true,
    revision: typeof value.revision === 'string' ? value.revision : ''
  });
}
export function connectionPayload(hubUrl, hubInput, clearHub) {
  // Never retain secret text in state, storage, status, or the returned projection.
  return {
    hubUrl: hubUrl.trim(), hubSecret: clearHub ? '' : hubInput || null
  };
}
export function sourceKey(connection) {
  return JSON.stringify([connection.hubUrl, connection.revision]);
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

// Only a bare Hub host receives the documented default port. Explicit ports,
// schemes and paths are never replaced with a different destination.
export function normalizeConnectionAddress(input) {
  const raw = String(input || '').trim();
  if (!raw) return { value: '', origin: '', error: '', loopback: false };
  const invalid = message => ({ value: '', origin: '', error: message, loopback: false });
  if (raw.length > 2048 || /\s|[\\\u0000-\u001f\u007f]/.test(raw)) return invalid('地址含空格或无效字符，请只填写主机地址。');
  const explicitScheme = /^[a-z][a-z\d+.-]*:\/\//i.test(raw);
  if (explicitScheme && !/^https?:\/\//i.test(raw)) return invalid('请使用 http:// 或 https:// 地址。');
  let value = explicitScheme ? raw : 'http://' + raw;
  let parsed;
  try { parsed = new URL(value); } catch (_) { return invalid('地址无法识别，请填写 IP 或 主机名:端口。'); }
  if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname || parsed.username || parsed.password)
    return invalid('地址不能包含账号或密钥，请将密钥填写在下方。');
  // URL normalizes dot paths and default ports; inspect the original suffix too.
  const authority = value.slice(value.indexOf('://') + 3).split(/[/?#]/)[0];
  const suffix = value.slice(value.indexOf('://') + 3 + authority.length);
  if (authority.endsWith(':') || !/^[a-z\d.\-:[\]]+$/i.test(authority)) return invalid('主机名或端口无效，请检查地址。');
  if (suffix && suffix !== '/') return invalid('这里只接受来源地址。请去掉路径、查询参数或 # 后缀，再测试。');
  if (/:0$/.test(authority)) return invalid('端口必须在 1–65535 之间。');
  const hasPort = authority.startsWith('[') ? /\]:\d+$/.test(authority) : /:\d+$/.test(authority);
  const defaultPort = !explicitScheme && !hasPort;
  value = parsed.protocol + '//' + authority.toLowerCase() + (defaultPort ? ':17321' : '');
  // Match the native URI projection, which keeps literal IPv6 spelling.
  const origin = parsed.protocol === 'http:' ? value.replace(/:80$/, '') : value.replace(/:443$/, '');
  const hostname = parsed.hostname.replace(/^\[|\]$/g, '');
  const loopback = hostname === 'localhost' || hostname.endsWith('.localhost') || /^127\./.test(hostname) || hostname === '::1' || hostname === '0.0.0.0';
  return { value, origin, error: '', loopback, defaultPort };
}

export function connectionTestMessage(result) {
  if (result.ok) return 'Hub 连接测试成功。' + (Number.isFinite(result.latencyMs) ? '用时 ' + Math.max(0, Math.round(result.latencyMs)) + ' 毫秒。' : '');
  return ({
    invalid_connection: '地址或密钥格式无效，请检查当前输入后重试。',
    unconfigured: '请先填写 Hub 地址。',
    unauthorized: '认证失败，请填写有效密钥；更换来源地址后需重新输入密钥。',
    timeout: '测试超时，请检查电脑是否在线、端口和防火墙是否允许局域网访问。',
    host_not_found: '找不到主机。请检查名称，或改填电脑的局域网 IP。',
    connection_refused: '连接被拒绝。请确认服务已启动、端口正确，并允许局域网访问。',
    invalid_response: '服务返回的数据不符合接口要求，请确认这是 Token Monitor 来源地址。',
    http_error: '服务返回错误，请检查服务状态和端口后重试。',
    redirect_blocked: '服务要求跳转。请直接填写最终服务来源地址后重试。',
    cancelled: '测试已取消，可以修改输入后重新测试。',
    connection_changed: '已保存的连接发生变化，请重新测试当前输入。',
    network_error: '无法连接，请检查设备网络、电脑地址和防火墙。'
  })[result.error] || '测试失败，请检查设备网络和服务地址后重试。';
}
