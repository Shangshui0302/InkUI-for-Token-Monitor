import { publicConnection, sourceKey, createSourceFence } from './connection.mjs';
import { nativeConnection, saveNativeConnection, reloadNativeConnection, createTransport } from './transport.mjs';
import { createSettings } from './settings.js';
import { projectDashboardStats, projectHistoryForCache } from './dashboard-model.mjs';
import { createDashboard } from './dashboard.js';
import { themeKey, accentKey, ACCENTS, normalizeTheme, normalizeAccent } from './theme.mjs';
import { installPageSwipe } from './gestures.mjs';
import { createDisplayRefresh } from './display.mjs';
import { icon } from './icons.mjs';
import { normalizeDeviceStatus, networkLabel } from './device-status.mjs';
import { createConnectionRecovery, networkRecovered } from './connection-recovery.mjs';
import { createHistoryRecovery, historyRevisionMatches } from './history-recovery.mjs';

const $ = id => document.getElementById(id);
const cacheKey = 'token-monitor-eink-snapshot-v1';
const historyCacheKey = 'token-monitor-eink-history-v1';
const intervalKey = 'token-monitor-eink-interval-v1';
const nativeBridge = window.TokenMonitorAndroid;
if (!nativeBridge || typeof nativeBridge.getConnection !== 'function' || typeof nativeBridge.saveConnection !== 'function') {
  $('connection').textContent = '应用初始化失败';
  throw new Error('Android connection bridge unavailable');
}
let connectionConfig = publicConnection();
let nativeHidden = false;
let detailMessage = '正在连接', settingsUI = null;
const sourceStampKey = 'token-monitor-eink-native-source-v1';
try { connectionConfig = nativeConnection(nativeBridge); } catch (_) { detailMessage = '无法读取连接配置，请重新保存。'; }
const sourceFence = createSourceFence(sourceKey(connectionConfig));
function isHidden() { return document.hidden || nativeHidden; }
const displayRefresh = createDisplayRefresh(nativeBridge, isHidden);
document.querySelectorAll('[data-icon]').forEach(node => node.replaceChildren(icon(node.dataset.icon)));
$('hub-state-icon').replaceChildren(icon('link'));
let deviceStatus = normalizeDeviceStatus(), deviceSignature = '', onNetworkChange = () => {};
function showDeviceStatus(value) {
  const next = normalizeDeviceStatus(value), signature = JSON.stringify(next);
  if (signature === deviceSignature) return;
  const previous = deviceStatus, first = !deviceSignature;
  const visibleChanged = next.time !== deviceStatus.time || next.batteryPercent !== deviceStatus.batteryPercent
    || next.charging !== deviceStatus.charging || next.network !== deviceStatus.network || next.fullscreen !== deviceStatus.fullscreen;
  deviceStatus = next; deviceSignature = signature;
  if (first || next.time !== previous.time) $('device-time').textContent = next.time;
  if (first || next.network !== previous.network) $('device-network').replaceChildren(icon(next.network));
  if (first || next.network !== previous.network || next.networkValidated !== previous.networkValidated) {
    $('device-network').title = networkLabel(next); $('device-network').setAttribute('aria-label', networkLabel(next));
  }
  if (first || next.batteryPercent !== previous.batteryPercent || next.charging !== previous.charging) {
    const battery = icon('battery');
    if (next.batteryPercent !== null) {
      const fill = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
      for (const [key, v] of Object.entries({ x:5, y:9, width:next.batteryPercent * .11, height:6, fill:'currentColor', stroke:'none' })) fill.setAttribute(key, v);
      battery.append(fill);
    }
    $('battery-symbol').replaceChildren(battery);
    if (next.charging) { const bolt = icon('bolt'); bolt.classList.add('charging-mark'); $('battery-symbol').append(bolt); }
    $('battery-value').textContent = next.batteryPercent === null ? '—' : next.batteryPercent + '%';
    const batteryLabel = (next.batteryPercent === null ? '电量未知' : '电量 ' + next.batteryPercent + '%') + (next.charging ? ' · 充电中' : '');
    $('device-battery').title = batteryLabel; $('device-battery').setAttribute('aria-label', batteryLabel);
  }
  if (first || next.fullscreen !== previous.fullscreen) {
    document.documentElement.dataset.fullscreen = String(next.fullscreen);
    const toggle = $('fullscreen-toggle'), label = next.fullscreen ? '退出全屏' : '进入全屏';
    toggle.replaceChildren(icon(next.fullscreen ? 'collapse' : 'expand'));
    toggle.setAttribute('aria-pressed', String(next.fullscreen)); toggle.setAttribute('aria-label', label); toggle.title = label;
    toggle.disabled = typeof nativeBridge.setFullscreen !== 'function';
  }
  settingsUI?.update();
  if (visibleChanged && $('settings').hidden) displayRefresh.request('content');
  onNetworkChange(previous, next);
}
try { showDeviceStatus(JSON.parse(nativeBridge.getDeviceStatus?.() || '{}')); } catch (_) { showDeviceStatus({}); }
window.addEventListener('tokenmonitor:device-status', event => showDeviceStatus(event.detail));
$('fullscreen-toggle').addEventListener('click', () => {
  try { nativeBridge.setFullscreen?.(!deviceStatus.fullscreen); } catch (_) { /* Preserve the last confirmed native mode. */ }
});

function showTheme(value, accentValue) {
  const theme = normalizeTheme(value), accent = normalizeAccent(accentValue);
  const changed = document.documentElement.dataset.theme !== theme || document.documentElement.dataset.accent !== accent;
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.accent = accent;
  document.querySelectorAll('[data-theme-choice]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.themeChoice === theme)));
  document.querySelectorAll('[data-accent-choice]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.accentChoice === accent)));
  $('accent-options').hidden = theme !== 'color';
  $('theme-caption').textContent = theme === 'color' ? ACCENTS.find(a => a.id === accent).label : '黑白';
  settingsUI?.resize();
  if (changed) displayRefresh.request('full');
}
ACCENTS.forEach(accent => {
  const button = document.createElement('button');
  button.type = 'button'; button.className = 'accent-choice'; button.dataset.accentChoice = accent.id;
  const dot = document.createElement('span'); dot.className = 'accent-dot'; dot.setAttribute('aria-hidden', 'true');
  button.appendChild(dot); button.appendChild(document.createTextNode(accent.label));
  button.addEventListener('click', () => {
    showTheme('color', accent.id);
    try { localStorage.setItem(accentKey, accent.id); localStorage.setItem(themeKey, 'color'); } catch (_) { /* Keep the in-memory choice. */ }
  });
  $('accent-options').appendChild(button);
});
let savedTheme = 'bw', savedAccent = 'blue';
try { savedTheme = localStorage.getItem(themeKey); savedAccent = localStorage.getItem(accentKey); } catch (_) { /* Storage is optional. */ }
showTheme(savedTheme, savedAccent);
document.querySelectorAll('[data-theme-choice]').forEach(b => b.addEventListener('click', () => {
  const theme = normalizeTheme(b.dataset.themeChoice);
  showTheme(theme, document.documentElement.dataset.accent);
  try { localStorage.setItem(themeKey, theme); } catch (_) { /* Keep the in-memory choice. */ }
}));
const dashboard = createDashboard($('dashboard'), reason => { if ($('settings').hidden) displayRefresh.request(reason); });
dashboard.setOrderSource(connectionConfig.hubUrl);
let inFlight = false;
let timer = null;
let recoveryTimer = null;
let snapshot = null;
let receivedAt = null;
let history = null;
let historyReceivedAt = null;
let historyRevision = null;
let historyError = '';
let historyCached = true;
let historyForceQueued = false;
let refreshQueued = false;
const transport = createTransport({ bridge: nativeBridge });
const recovery = createConnectionRecovery();
const historyRecovery = createHistoryRecovery();

function setText(node, text) {
  if (node.textContent !== text) {
    node.textContent = text;
    if (!node.closest('.sr-only') && node.getClientRects().length) displayRefresh.request('content');
  }
}
function render() {
  dashboard.update(snapshot, history, { receivedAt, historyReceivedAt, historyError, historyCached });
}
function notice(title, detail) {
  const healthy = title === '已连接' && !historyError;
  const label = healthy ? '在线' : title === '已连接' ? '部分缓存' : title === '本机缓存' ? '缓存' : title === '连接失败' ? '离线' : title === '当前快照已清除' ? '已清除' : title;
  const detailText = healthy ? '在线' : title === '已连接'
    ? '历史读取失败'
    : title === '当前快照已清除' && detail.includes('未允许') ? '部分缓存未清除'
    : label + (snapshot && !['本机缓存', '当前快照已清除'].includes(title) ? ' · 保留缓存' : '');
  setText($('connection'), label); detailMessage = detail || detailText;
  const connectionState = healthy ? 'online' : title === '正在连接' ? 'connecting' : ['本机缓存','已连接'].includes(title) ? 'cached' : 'error';
  if ($('status-toggle').dataset.state !== connectionState) {
    $('status-toggle').dataset.state = connectionState;
    $('hub-state-icon').replaceChildren(icon(connectionState === 'error' ? 'link-off' : 'link'));
    if ($('settings').hidden) displayRefresh.request('content');
  }
  $('status-toggle').title = 'Hub：' + label;
  $('status-toggle').setAttribute('aria-label', 'Hub：' + label + '，查看连接详情');
  setText($('last-time'), receivedAt ? new Date(receivedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }) : '');
  settingsUI?.update();
}
function showInterval() {
  const seconds = Number($('interval').value);
  setText($('interval-current'), seconds === 0 ? '手动刷新' : seconds === 60 ? '每 60 秒' : '每 5 分钟');
  document.querySelectorAll('[data-interval]').forEach(b => b.setAttribute('aria-pressed', String(Number(b.dataset.interval) === seconds)));
}
function schedule() {
  clearTimeout(timer); timer = null;
  const delay = recovery.delay(Number($('interval').value));
  if (delay !== null && recoveryTimer === null && connectionConfig.hubUrl && deviceStatus.network !== 'offline' && !isHidden())
    timer = setTimeout(() => refresh(), delay);
}
function retry() {
  recovery.reset(); clearTimeout(recoveryTimer); recoveryTimer = null;
  return refresh(true);
}
onNetworkChange = (previous, next) => {
  if (!connectionConfig.hubUrl) return;
  if (next.network !== 'offline' && !networkRecovered(previous, next)) return;
  clearTimeout(timer); timer = null;
  clearTimeout(recoveryTimer); recoveryTimer = null;
  // An old socket/result must not confirm the newly selected Wi-Fi network.
  sourceFence.invalidate(); transport.cancel(); refreshQueued = false; historyForceQueued = false;
  if (next.network === 'offline') {
    historyCached = true; render();
    if (recovery.automaticAllowed) notice('连接失败', '设备未联网，已保留 Hub 配置；网络恢复后会重新连接。');
  } else if (!isHidden() && recovery.automaticAllowed) {
    historyRecovery.networkChanged();
    recoveryTimer = setTimeout(() => { recoveryTimer = null; refresh(); }, 1000);
  }
};
function validTime(value) { return typeof value === 'string' && Number.isFinite(Date.parse(value)); }
function save(key, value) { try { localStorage.setItem(key, JSON.stringify({ ...value, _connectionSource: sourceKey(connectionConfig) })); } catch (_) { /* Reading works without persistent storage. */ } }
function readCache() {
  try {
    const value = JSON.parse(localStorage.getItem(historyCacheKey));
    if (value && value._connectionSource === sourceKey(connectionConfig) && validTime(value.receivedAt)) {
      history = projectHistoryForCache(value.history); historyReceivedAt = value.receivedAt;
      historyRevision = typeof value.revision === 'string' ? value.revision : null;
      save(historyCacheKey, { history, receivedAt: historyReceivedAt, revision: historyRevision });
    }
  } catch (_) { /* Old or invalid cache is ignored. */ }
  try {
    const value = JSON.parse(localStorage.getItem(cacheKey));
    if (value && value._connectionSource === sourceKey(connectionConfig) && validTime(value.receivedAt)) {
      snapshot = projectDashboardStats(value.stats); receivedAt = value.receivedAt;
      save(cacheKey, { stats: snapshot, receivedAt }); render();
      notice('本机缓存', '正在连接；当前显示上次成功读取的用量快照。');
    }
  } catch (_) { /* Older quota-only cache is replaced after a successful fetch. */ }
}
function requestFailure(error) {
  if (error.kind) return { title: error.kind, detail: error.message };
  if (error.name === 'AbortError') return { title: '读取超时', detail: '请求超过 10 秒，请稍后刷新。' };
  if (error instanceof SyntaxError || /数据格式不正确/.test(error.message)) return { title: '数据格式错误', detail: '返回的数据无法读取。' };
  return { title: '连接失败', detail: '无法连接服务，请检查网络。' };
}
async function getJSON(path) { return transport.getJSON(path); }
function handleHubFailure(error) {
  // Background failures update the summary; they never interrupt editing or
  // force the reader away from another dashboard/settings page.
  if (error.status === 401 || error.code === 'hub_unauthorized')
    detailMessage = 'Hub 认证失败，请在连接设置中更换密钥。';
}
async function readHistory(stats, epoch) {
  try {
    const nextHistory = projectHistoryForCache(await getJSON('/api/history'));
    if (!sourceFence.accepts(epoch)) return;
    history = nextHistory; historyReceivedAt = new Date().toISOString(); historyRevision = stats.historyRevision;
    historyError = ''; historyCached = false;
    historyRecovery.reset();
    save(historyCacheKey, { history, receivedAt: historyReceivedAt, revision: historyRevision });
  } catch (error) {
    if (!sourceFence.accepts(epoch)) return;
    if (error instanceof SyntaxError || /数据格式不正确/.test(error.message)) error.code = 'invalid_response';
    historyRecovery.failed(error);
    historyError = requestFailure(error).detail; historyCached = true;
    if (error.status === 401 || error.code === 'hub_unauthorized') handleHubFailure(error);
  }
  render();
}
async function refresh(forceHistory = false) {
  if (isHidden()) return;
  if (!connectionConfig.hubUrl) { notice('未配置', '请填写 Hub 地址并保存。'); return; }
  if (deviceStatus.network === 'offline') {
    notice('连接失败', '设备未联网，已保留 Hub 配置；网络恢复后会重新连接。'); return;
  }
  if (inFlight) { refreshQueued = true; historyForceQueued = historyForceQueued || forceHistory; return; }
  if (forceHistory) historyRecovery.reset();
  clearTimeout(timer); inFlight = true; $('refresh').disabled = true;
  const epoch = sourceFence.capture();
  try {
    const stats = projectDashboardStats(await getJSON('/api/stats'));
    if (!sourceFence.accepts(epoch)) return;
    snapshot = stats; receivedAt = new Date().toISOString();
    recovery.reset();
    const historyCurrent = historyRevisionMatches(history, historyRevision, stats.historyRevision);
    // A fresh stats revision can verify retained history without downloading it
    // again. Keep its original read time; endpoint failures remain actionable.
    historyCached = !historyCurrent || Boolean(historyError);
    save(cacheKey, { stats, receivedAt });
    render(); notice('已连接', '用量已更新；额度为各账号采集快照。');
    // Older Hubs without revisions cannot establish cached history freshness.
    // History failures retain current stats and mark historical data as cached.
    if (historyRecovery.ready() && (forceHistory || !historyCurrent || historyError))
      await readHistory(stats, epoch);
    if (!sourceFence.accepts(epoch)) return;
    notice('已连接', historyError ? '用量已更新；历史读取失败，保留可用缓存或预览。' : '用量已更新；额度与近期会话为最新观测。');
  } catch (error) {
    if (!sourceFence.accepts(epoch)) return;
    if (error instanceof SyntaxError || /数据格式不正确/.test(error.message)) error.code = 'invalid_response';
    recovery.failed(error);
    handleHubFailure(error);
    const failure = requestFailure(error);
    historyCached = true; render();
    notice(failure.title, failure.detail + (snapshot ? ' 当前显示本机保留快照，不代表在线采集。' : ' 当前没有可显示的数据。'));
  } finally {
    inFlight = false; $('refresh').disabled = false;
    if (refreshQueued && !isHidden()) { const force = historyForceQueued; refreshQueued = false; historyForceQueued = false; refresh(force); }
    else { refreshQueued = false; historyForceQueued = false; schedule(); }
  }
}
$('refresh').addEventListener('click', retry);
$('interval').addEventListener('change', () => { try { localStorage.setItem(intervalKey, $('interval').value); } catch (_) { /* Optional storage. */ } showInterval(); schedule(); });
function resetSourceCaches() {
  [cacheKey, historyCacheKey].forEach(key => { try { localStorage.removeItem(key); } catch (_) { /* Optional storage. */ } });
  snapshot = null; receivedAt = null; history = null; historyReceivedAt = null; historyRevision = null; historyError = ''; historyCached = true;
  historyRecovery.reset();
  dashboard.clear(); render();
}
function applyConnection(next, message) {
  const changedSource = connectionConfig.hubUrl !== next.hubUrl;
  connectionConfig = next;
  if (!sourceFence.change(sourceKey(next))) sourceFence.invalidate();
  recovery.reset(); clearTimeout(timer); timer = null; clearTimeout(recoveryTimer); recoveryTimer = null;
  transport.cancel(); refreshQueued = false; historyForceQueued = false;
  if (changedSource) dashboard.setOrderSource(next.hubUrl);
  resetSourceCaches();
  try { localStorage.setItem(sourceStampKey, sourceKey(next)); } catch (_) { /* Optional storage. */ }
  if (next.hubUrl) { notice('正在连接', message); refresh(true); }
  else notice(['read_failed', 'key_unavailable'].includes(next.storageState) ? '连接记录不可读' : '未配置',
    ['read_failed', 'key_unavailable'].includes(next.storageState) ? '原连接记录已保留，请重试读取或重新配置。' : '请填写 Hub 地址并保存。');
}
settingsUI = createSettings({
  bridge: nativeBridge,
  getConfig: () => connectionConfig,
  getStatus: () => ({ label: $('connection').textContent, state: $('status-toggle').dataset.state,
    detail: detailMessage, receivedAt, historyReceivedAt, historyError, deviceStatus }),
  saveConnection(payload) {
    const next = saveNativeConnection(nativeBridge, payload);
    applyConnection(next, '配置已保存，正在读取数据。');
    return next;
  },
  reloadConnection() {
    const next = reloadNativeConnection(nativeBridge);
    applyConnection(next, '连接记录已重新读取，正在读取数据。');
    return next;
  },
  clearCache() {
    sourceFence.invalidate(); transport.cancel(); let removed = true;
    [cacheKey, historyCacheKey].forEach(key => { try { localStorage.removeItem(key); } catch (_) { removed = false; } });
    snapshot = null; receivedAt = null; history = null; historyReceivedAt = null; historyRevision = null; historyError = ''; historyCached = true;
    historyRecovery.reset();
    dashboard.clear();
    notice('当前快照已清除', removed ? '缓存已清除；刷新后重新读取。' : '界面已清空，部分缓存未清除。');
    return removed;
  },
  refresh: retry, repaint: reason => displayRefresh.request(reason),
  onConnected: () => document.querySelector('#navigation [data-page=limits]').click()
});
$('settings-open').addEventListener('click', () => settingsUI.open());
$('status-toggle').addEventListener('click', () => settingsUI.open('connection'));
document.querySelectorAll('[data-interval]').forEach(b => b.addEventListener('click', () => {
  $('interval').value = b.dataset.interval; $('interval').dispatchEvent(new Event('change'));
}));
document.querySelectorAll('[data-period]').forEach(b => b.addEventListener('click', () => {
  $('period').value = b.dataset.period;
  document.querySelectorAll('[data-period]').forEach(p => p.setAttribute('aria-pressed', String(p === b)));
  $('period').dispatchEvent(new Event('change'));
}));
function visibilityChanged() {
  clearTimeout(recoveryTimer); recoveryTimer = null;
  if (isHidden()) {
    clearTimeout(timer); sourceFence.invalidate(); transport.cancel();
    refreshQueued = false; historyForceQueued = false;
    displayRefresh.cancel(); cancelDashboardSwipe(); cancelSettingsSwipe(); settingsUI.cancelTests();
  } else {
    settingsUI.update();
    if (recovery.automaticAllowed) refresh();
  }
}
document.addEventListener('visibilitychange', visibilityChanged);
window.addEventListener('tokenmonitor:visibility', event => { nativeHidden = event.detail?.hidden === true; visibilityChanged(); });
window.tokenMonitorBack = () => settingsUI.opened ? settingsUI.back() : dashboard.back();
function sizeViewport() {
  const viewport = window.visualViewport;
  document.documentElement.style.setProperty('--app-height', Math.floor(viewport ? viewport.height : innerHeight) + 'px');
  settingsUI.resize();
}
sizeViewport();
window.addEventListener('resize', sizeViewport);
if (window.visualViewport) window.visualViewport.addEventListener('resize', sizeViewport);
// All dashboard navigation is paginated. Never pan the viewport or a nested list.
document.addEventListener('wheel', e => e.preventDefault(), { passive: false });
document.addEventListener('touchmove', e => e.preventDefault(), { passive: false });
const cancelDashboardSwipe = installPageSwipe($('dashboard'), delta => dashboard.turn(delta), () => $('settings').hidden && !isHidden());
const cancelSettingsSwipe = installPageSwipe(document.querySelector('.settings-body'), delta => settingsUI.turn(delta), () => settingsUI.opened && !isHidden());
document.addEventListener('keydown', e => {
  if (e.defaultPrevented) return;
  if (e.key === 'Escape') { if (window.tokenMonitorBack()) e.preventDefault(); return; }
  if (e.target.matches('input,textarea')) return;
  if (['PageDown', 'PageUp', 'ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) e.preventDefault();
  if (['PageDown', 'PageUp', 'ArrowLeft', 'ArrowRight'].includes(e.key)) {
    e.preventDefault(); const delta = ['PageUp', 'ArrowLeft'].includes(e.key) ? -1 : 1;
    if (settingsUI.opened) settingsUI.turn(delta); else $('page-' + (delta < 0 ? 'prev' : 'next')).click();
  }

});

try { const saved = localStorage.getItem(intervalKey); if (['0', '60', '300'].includes(saved)) $('interval').value = saved; } catch (_) { /* Optional storage. */ }
showInterval();
{
  try {
    const key = sourceKey(connectionConfig);
    if (localStorage.getItem(sourceStampKey) !== key) [cacheKey, historyCacheKey].forEach(k => localStorage.removeItem(k));
    localStorage.setItem(sourceStampKey, key);
  } catch (_) { /* In-memory state remains isolated. */ }
}
try { localStorage.removeItem('token-monitor-eink-session-archive-v1'); } catch (_) { /* Best-effort cleanup of a retired cache. */ }
readCache();
if (!connectionConfig.hubUrl) {
  const unreadable = ['read_failed', 'key_unavailable'].includes(connectionConfig.storageState);
  notice(unreadable ? '连接记录不可读' : '未配置', unreadable ? '原连接记录已保留，请重试读取或重新配置。' : '请填写 Hub 地址并保存。');
  settingsUI.open('connection');
}
else refresh();

// Initialization is complete even while the first Hub read is pending.
if (!window.TokenMonitorBoot || window.TokenMonitorBoot.ready()) {
  try { nativeBridge.uiReady?.(); } catch (_) { /* Older APKs have no readiness handshake. */ }
}
