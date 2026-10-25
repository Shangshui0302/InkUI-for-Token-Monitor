import { publicConnection, connectionPayload, sourceKey, createSourceFence, normalizeConnectionAddress, connectionTestMessage } from './connection.mjs';
import { nativeConnection, saveNativeConnection, createTransport, createConnectionTester } from './transport.mjs';
import { absoluteTime } from './model.mjs';
import { projectDashboardStats, projectHistoryForCache, projectArchiveForCache } from './dashboard-model.mjs';
import { createDashboard } from './dashboard.js';
import { themeKey, accentKey, ACCENTS, normalizeTheme, normalizeAccent } from './theme.mjs';
import { installPageSwipe } from './gestures.mjs';
import { createDisplayRefresh } from './display.mjs';
import { icon } from './icons.mjs';
import { normalizeDeviceStatus, networkLabel } from './device-status.mjs';

const $ = id => document.getElementById(id);
const cacheKey = 'token-monitor-eink-snapshot-v1';
const historyCacheKey = 'token-monitor-eink-history-v1';
const archiveCacheKey = 'token-monitor-eink-session-archive-v1';
const intervalKey = 'token-monitor-eink-interval-v1';
const nativeBridge = window.TokenMonitorAndroid;
if (!nativeBridge || typeof nativeBridge.getConnection !== 'function' || typeof nativeBridge.saveConnection !== 'function') {
  $('connection').textContent = '应用初始化失败';
  throw new Error('Android connection bridge unavailable');
}
let connectionConfig = publicConnection();
let nativeHidden = false;
let nativePendingVerification = false;
let connectionSource = 'hub', connectionPage = 0, connectionMessage = '', connectionMessagePages = [''];
let clearHubSecret = false, clearArchiveSecret = false;
let connectionSteps = ['address', 'secret', 'actions', 'message'];
let connectionExpanded = false;
let detailPage = 0, detailMessage = '正在连接', detailPages = ['正在连接'];
const sourceStampKey = 'token-monitor-eink-native-source-v1';
try { connectionConfig = nativeConnection(nativeBridge); } catch (_) { connectionMessage = '无法读取连接配置，请重新保存。'; }
const sourceFence = createSourceFence(sourceKey(connectionConfig));
const connectionTester = createConnectionTester(nativeBridge, result => {
  setConnectionMessage(connectionTestMessage(result), true);
});
window.addEventListener('tokenmonitor:connection-test', event => connectionTester.accept(event.detail));
function isHidden() { return document.hidden || nativeHidden; }
const displayRefresh = createDisplayRefresh(nativeBridge, isHidden);
document.querySelectorAll('[data-icon]').forEach(node => node.replaceChildren(icon(node.dataset.icon)));
$('hub-state-icon').replaceChildren(icon('link'));
let deviceStatus = normalizeDeviceStatus(), deviceSignature = '';
function showDeviceStatus(value) {
  const next = normalizeDeviceStatus(value), signature = JSON.stringify(next);
  if (signature === deviceSignature) return;
  const visibleChanged = next.time !== deviceStatus.time || next.batteryPercent !== deviceStatus.batteryPercent
    || next.charging !== deviceStatus.charging || next.network !== deviceStatus.network || next.fullscreen !== deviceStatus.fullscreen;
  deviceStatus = next; deviceSignature = signature;
  $('device-time').textContent = next.time;
  $('device-network').replaceChildren(icon(next.network));
  $('device-network').title = networkLabel(next); $('device-network').setAttribute('aria-label', networkLabel(next));
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
  document.documentElement.dataset.fullscreen = String(next.fullscreen);
  const toggle = $('fullscreen-toggle'), label = next.fullscreen ? '退出全屏' : '进入全屏';
  toggle.replaceChildren(icon(next.fullscreen ? 'collapse' : 'expand'));
  toggle.setAttribute('aria-pressed', String(next.fullscreen)); toggle.setAttribute('aria-label', label); toggle.title = label;
  toggle.disabled = typeof nativeBridge.setFullscreen !== 'function';
  if (visibleChanged && $('settings').hidden) displayRefresh.request('content');
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
  $('theme-caption').textContent = theme === 'color' ? ACCENTS.find(a => a.id === accent).label + ' · 16 阶' : '16 级灰度';
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
let inFlight = false;
let timer = null;
let snapshot = null;
let receivedAt = null;
let history = null;
let historyReceivedAt = null;
let historyRevision = null;
let historyError = '';
let historyCached = true;
let archive = null;
let archiveReceivedAt = null;
let archiveError = '';
let archiveCached = true;
let archiveChecked = false;
let archiveAttemptAt = 0;
let archiveForceQueued = false;
let refreshQueued = false;
const transport = createTransport();

function setText(node, text) {
  if (node.textContent !== text) {
    node.textContent = text;
    if (node.getClientRects().length) displayRefresh.request('content');
  }
}
function render() {
  dashboard.update(snapshot, history, { receivedAt, historyReceivedAt, historyError, historyCached, archiveReceivedAt, archiveError, archiveCached }, archive);
}
function notice(title, detail) {
  const healthy = title === '已连接' && !historyError && !archiveError;
  const label = healthy ? '在线' : title === '已连接' ? '部分缓存' : title === '本机缓存' ? '缓存' : title === '连接失败' ? '离线' : title === '当前快照已清除' ? '已清除' : title;
  const detailText = healthy ? '在线' : title === '已连接'
    ? [historyError ? '历史读取失败' : '', archiveError ? '归档读取失败' : ''].filter(Boolean).join(' · ')
    : title === '当前快照已清除' && detail.includes('未允许') ? '部分缓存未清除'
    : label + (snapshot && !['本机缓存', '当前快照已清除'].includes(title) ? ' · 保留缓存' : '');
  setText($('connection'), label); detailMessage = detail || detailText;
  const connectionState = healthy ? 'online' : ['本机缓存','已连接'].includes(title) ? 'cached' : 'error';
  if ($('status-toggle').dataset.state !== connectionState) {
    $('status-toggle').dataset.state = connectionState;
    $('hub-state-icon').replaceChildren(icon(connectionState === 'error' ? 'link-off' : 'link'));
    if ($('settings').hidden) displayRefresh.request('content');
  }
  $('status-toggle').title = 'Hub：' + label;
  $('status-toggle').setAttribute('aria-label', 'Hub：' + label + '，查看连接详情');
  setText($('last-time'), receivedAt ? new Date(receivedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }) : '');
  updateDetailPages(); renderConnection();
}
function showInterval() {
  const seconds = Number($('interval').value);
  setText($('interval-current'), seconds === 0 ? '手动刷新' : seconds === 60 ? '每 60 秒' : '每 5 分钟');
  document.querySelectorAll('[data-interval]').forEach(b => b.setAttribute('aria-pressed', String(Number(b.dataset.interval) === seconds)));
}
function schedule() {
  clearTimeout(timer); const seconds = Number($('interval').value);
  if (seconds > 0 && !isHidden()) timer = setTimeout(refresh, seconds * 1000);
}
function validTime(value) { return typeof value === 'string' && Number.isFinite(Date.parse(value)); }
function save(key, value) { try { localStorage.setItem(key, JSON.stringify({ ...value, _connectionSource: sourceKey(connectionConfig) })); } catch (_) { /* Reading works without persistent storage. */ } }
function readCache() {
  try {
    const value = JSON.parse(localStorage.getItem(archiveCacheKey));
    if (value && value._connectionSource === sourceKey(connectionConfig) && validTime(value.receivedAt)) {
      archive = projectArchiveForCache(value.archive); archiveReceivedAt = value.receivedAt;
      save(archiveCacheKey, { archive, receivedAt: archiveReceivedAt });
    }
  } catch (_) { /* Old or invalid archive cache is ignored. */ }
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
  if (error.status === 401 || error.code === 'hub_unauthorized' || error.code === 'hub_unconfigured') {
    connectionSource = 'hub'; connectionPage = error.status === 401 || error.code === 'hub_unauthorized' ? 1 : 0;
    if (error.status === 401 || error.code === 'hub_unauthorized') connectionMessage = 'Hub 认证失败，请更新密钥后保存并连接。';
    showSettings('native');
  }
}
async function readHistory(stats, epoch) {
  try {
    const nextHistory = projectHistoryForCache(await getJSON('/api/history'));
    if (!sourceFence.accepts(epoch)) return;
    history = nextHistory; historyReceivedAt = new Date().toISOString(); historyRevision = stats.historyRevision;
    historyError = ''; historyCached = false;
    save(historyCacheKey, { history, receivedAt: historyReceivedAt, revision: historyRevision });
  } catch (error) {
    if (!sourceFence.accepts(epoch)) return;
    historyError = requestFailure(error).detail; historyCached = true;
    if (error.status === 401 || error.code === 'hub_unauthorized') handleHubFailure(error);
  }
  render();
}
async function readArchive(epoch) {
  archiveChecked = true; archiveAttemptAt = Date.now();
  try {
    const nextArchive = projectArchiveForCache(await getJSON('/api/session-archive'));
    if (!sourceFence.accepts(epoch)) return;
    archive = nextArchive; archiveReceivedAt = new Date().toISOString(); archiveError = ''; archiveCached = false;
    save(archiveCacheKey, { archive, receivedAt: archiveReceivedAt });
  } catch (error) {
    if (!sourceFence.accepts(epoch)) return;
    archiveError = requestFailure(error).detail; archiveCached = true;
  }
  render();
}
async function refresh(forceArchive = false) {
  if (isHidden()) return;
  if (!connectionConfig.hubUrl) { notice('未配置', '请填写 Hub 地址并保存。'); return; }
  if (inFlight) { refreshQueued = true; archiveForceQueued = archiveForceQueued || forceArchive; return; }
  clearTimeout(timer); inFlight = true; $('refresh').disabled = true;
  const epoch = sourceFence.capture();
  try {
    const stats = projectDashboardStats(await getJSON('/api/stats'));
    if (!sourceFence.accepts(epoch)) return;
    snapshot = stats; receivedAt = new Date().toISOString();
    save(cacheKey, { stats, receivedAt });
    render(); notice('已连接', '用量已更新；额度为各账号采集快照。');
    // Optional sources commit independently; neither failure discards stats.
    const optional = [];
    // Manual refresh reloads every source. Older Hubs without revisions cannot
    // establish that a cached history is still current, so read it again too.
    if (forceArchive || !history || !stats.historyRevision || historyRevision !== stats.historyRevision || historyError) optional.push(readHistory(stats, epoch));
    if (forceArchive || !archiveChecked || Date.now() - archiveAttemptAt >= 180000) optional.push(readArchive(epoch));
    await Promise.allSettled(optional);
    if (!sourceFence.accepts(epoch)) return;
    if (nativePendingVerification) { nativePendingVerification = false; setConnectionMessage('Hub 连接成功。' + (archiveError ? '归档：' + archiveError : connectionConfig.archiveUrl ? '归档来源已响应。' : '归档来源未配置。')); }
    const issues = [];
    if (historyError) issues.push('完整历史');
    if (archiveError) issues.push('本机归档');
    notice('已连接', issues.length ? '用量已更新；' + issues.join('、') + '读取失败，保留可用缓存或预览。' : '用量已更新；额度与近期会话为最新观测。');
  } catch (error) {
    if (!sourceFence.accepts(epoch)) return;
    handleHubFailure(error);
    const failure = requestFailure(error);
    if (nativePendingVerification) { nativePendingVerification = false; setConnectionMessage(failure.detail); }
    historyCached = true; archiveCached = true; render();
    notice(failure.title, failure.detail + (snapshot ? ' 当前显示本机保留快照，不代表在线采集。' : ' 当前没有可显示的数据。'));
  } finally {
    inFlight = false; $('refresh').disabled = false;
    if (refreshQueued && !isHidden()) { const force = archiveForceQueued; refreshQueued = false; archiveForceQueued = false; refresh(force); }
    else { refreshQueued = false; archiveForceQueued = false; schedule(); }
  }
}
$('refresh').addEventListener('click', () => refresh(true));
$('interval').addEventListener('change', () => { try { localStorage.setItem(intervalKey, $('interval').value); } catch (_) { /* Optional storage. */ } showInterval(); schedule(); });
$('cache-open').addEventListener('click', () => {
  $('clear-cache').disabled = false;
  $('cache-message').textContent = '清除本机保存的用量、历史和会话快照；连接配置会保留。';
  showSettings('cache');
});
$('cache-cancel').addEventListener('click', () => showSettings('refresh'));
$('clear-cache').addEventListener('click', () => {
  sourceFence.invalidate(); transport.cancel(); let removed = true;
  [cacheKey, historyCacheKey, archiveCacheKey].forEach(key => { try { localStorage.removeItem(key); } catch (_) { removed = false; } });
  snapshot = null; receivedAt = null; history = null; historyReceivedAt = null; historyRevision = null; historyError = ''; historyCached = true; archive = null; archiveReceivedAt = null; archiveError = ''; archiveCached = true; archiveChecked = false; archiveAttemptAt = 0; dashboard.clear();
  $('clear-cache').disabled = true;
  $('cache-message').textContent = removed ? '缓存已清除。返回后可刷新数据。' : '界面已清空，部分缓存未能清除。';
  notice('当前快照已清除', removed ? '用量、历史与本机会话归档缓存已清除；刷新后重新读取。' : '界面已清空，但未允许清除部分持久缓存，重新打开可能恢复旧数据。');
});
function visibilityChanged() {
  if (isHidden()) { clearTimeout(timer); displayRefresh.cancel(); cancelDashboardSwipe(); cancelSettingsSwipe(); cancelConnectionTest(); }
  else { renderConnection(); refresh(); }
}
document.addEventListener('visibilitychange', visibilityChanged);
window.addEventListener('tokenmonitor:visibility', event => { nativeHidden = event.detail?.hidden === true; visibilityChanged(); });

let settingsReturnFocus = null;
function settingsFocusables() {
  return [...$('settings').querySelectorAll('button, input, select, textarea, a[href], [tabindex]')]
    .filter(node => !node.disabled && node.tabIndex >= 0 && node.getClientRects().length);
}
function showSettings(page = 'native') {
  const previous = document.querySelector('[data-setting-pane]:not([hidden])')?.dataset.settingPane;
  if (previous === 'native' && page !== 'native') cancelConnectionTest();
  const opening = $('settings').hidden;
  if (opening) settingsReturnFocus = document.activeElement;
  $('settings').hidden = false;
  document.querySelectorAll('[data-setting]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.setting === (page === 'connection' ? 'native' : page === 'cache' ? 'refresh' : page))));
  document.querySelectorAll('[data-setting-pane]').forEach(p => { p.hidden = p.dataset.settingPane !== page; });
  if (page === 'native') renderConnection();
  if (page === 'connection') updateDetailPages();
  if (page === 'screen') showDisplayCapabilities();
  // Keep input focus on ordinary updates; move it only when opening or hiding its pane.
  if (opening) $('settings-close').focus();
  else if (!$('settings').contains(document.activeElement) || !document.activeElement.getClientRects().length)
    (document.querySelector('[data-setting="' + page + '"]') || $('settings-close')).focus();
  $('app').setAttribute('inert', '');
  $('app').setAttribute('aria-hidden', 'true');
  displayRefresh.request('page');
}
function closeSettings() {
  if ($('settings').contains(document.activeElement)) document.activeElement.blur();
  $('settings').hidden = true;
  $('app').removeAttribute('inert');
  $('app').removeAttribute('aria-hidden');
  restoreConnectionInputs(); connectionPage = 0;
  const restore = settingsReturnFocus;
  settingsReturnFocus = null;
  if (restore?.isConnected && !restore.disabled && restore.tabIndex >= 0 && restore.getClientRects().length) restore.focus();
  else $('settings-open').focus();
  displayRefresh.request('page');
}
// Older Android WebViews may ignore inert, so contain keyboard and programmatic focus too.
document.addEventListener('focusin', event => {
  if (!$('settings').hidden && !$('settings').contains(event.target)) $('settings-close').focus();
});
function showDisplayCapabilities() {
  let supported = false;
  try { supported = JSON.parse(nativeBridge.getDisplayCapabilities?.() || '{}').supported === true; } catch (_) { /* System-managed display. */ }
  setText($('screen-mode'), supported ? '每 6 次翻页全刷' : '由系统管理刷新');
  $('screen-clear').disabled = !supported;
}
$('screen-clear').addEventListener('click', () => displayRefresh.request('full'));
window.addEventListener('tokenmonitor:display-capabilities', showDisplayCapabilities);
showDisplayCapabilities();
// Settings controls change a fixed page; never refresh the obscured dashboard.
$('settings').addEventListener('click', event => {
  if (!event.target.closest('button') || event.target.closest('#screen-clear')) return;
  const before = $('settings').outerHTML;
  queueMicrotask(() => { if ($('settings').outerHTML !== before) displayRefresh.request('page'); });
}, true);
$('settings-open').addEventListener('click', () => showSettings());
$('settings-close').addEventListener('click', closeSettings);
$('status-toggle').addEventListener('click', () => showSettings('native'));
$('connection-details-open').addEventListener('click', () => showSettings('connection'));
$('connection-edit').addEventListener('click', () => showSettings('native'));
document.querySelectorAll('[data-setting]').forEach(b => b.addEventListener('click', () => showSettings(b.dataset.setting)));
document.querySelectorAll('[data-interval]').forEach(b => b.addEventListener('click', () => { $('interval').value = b.dataset.interval; $('interval').dispatchEvent(new Event('change')); }));
document.querySelectorAll('[data-period]').forEach(b => b.addEventListener('click', () => {
  $('period').value = b.dataset.period;
  document.querySelectorAll('[data-period]').forEach(p => p.setAttribute('aria-pressed', String(p === b)));
  $('period').dispatchEvent(new Event('change'));
}));
// Fit long status text to fixed pages. No status or address is lost to clipping.
function splitText(text, node) {
  if (!node.clientWidth || !node.clientHeight) return [text];
  const chunks = []; let remaining = text;
  while (remaining) {
    let low = 1, high = remaining.length, fit = 1;
    while (low <= high) {
      const middle = Math.floor((low + high) / 2); node.textContent = remaining.slice(0, middle);
      if (node.scrollHeight <= node.clientHeight + 1 && node.scrollWidth <= node.clientWidth + 1) { fit = middle; low = middle + 1; }
      else high = middle - 1;
    }
    chunks.push(remaining.slice(0, fit)); remaining = remaining.slice(fit);
  }
  return chunks.length ? chunks : [''];
}
function updateDetailPages() {
  if (document.querySelector('[data-setting-pane="connection"]').hidden) return;
  $('last-success').hidden = true;
  const details = [receivedAt ? '最近成功：' + absoluteTime(receivedAt) : '尚无数据', detailMessage];
  if (historyError) details.push('历史：' + historyError);
  if (archiveError) details.push('归档：' + archiveError);
  details.push('Hub：' + (connectionConfig.hubUrl || '未配置'), '归档：' + (connectionConfig.archiveUrl || '来源未配置'));
  detailPages = splitText(details.join('。'), $('detail')); detailPage = Math.min(detailPage, detailPages.length - 1);
  $('detail').textContent = detailPages[detailPage]; $('detail-position').textContent = (detailPage + 1) + '/' + detailPages.length;
  $('detail-prev').disabled = detailPage === 0; $('detail-next').disabled = detailPage >= detailPages.length - 1;
  if (!$('settings').hidden) displayRefresh.request('content');
}
$('detail-prev').addEventListener('click', () => { detailPage = Math.max(0, detailPage - 1); updateDetailPages(); });
$('detail-next').addEventListener('click', () => { detailPage = Math.min(detailPages.length - 1, detailPage + 1); updateDetailPages(); });
function cancelConnectionTest() {
  if (!connectionTester.pending) return;
  connectionTester.cancel();
  connectionMessage = '测试已取消；可以重新测试当前输入。';
  if (!isHidden()) renderConnection();
}
function connectionIsDirty() {
  return $('hub-url').value.trim() !== connectionConfig.hubUrl || $('archive-url').value.trim() !== connectionConfig.archiveUrl
    || Boolean($('hub-secret').value || $('archive-secret').value || clearHubSecret || clearArchiveSecret);
}
function restoreConnectionInputs() {
  connectionTester.cancel(); nativePendingVerification = false;
  $('hub-url').value = connectionConfig.hubUrl; $('archive-url').value = connectionConfig.archiveUrl;
  $('hub-secret').value = ''; $('archive-secret').value = '';
  clearHubSecret = false; clearArchiveSecret = false; connectionMessage = '';
}
function currentConnectionPayload(testSource = '') {
  const hub = normalizeConnectionAddress(testSource === 'archive' ? connectionConfig.hubUrl : $('hub-url').value, 'hub');
  const archive = normalizeConnectionAddress(testSource === 'hub' ? connectionConfig.archiveUrl : $('archive-url').value, 'archive');
  if (hub.error || archive.error) throw new Error((hub.error ? 'Hub：' + hub.error : '归档：' + archive.error));
  return connectionPayload(hub.value, testSource === 'archive' ? '' : $('hub-secret').value, testSource === 'archive' ? false : clearHubSecret, archive.value, testSource === 'hub' ? '' : $('archive-secret').value, testSource === 'hub' ? false : clearArchiveSecret);
}
function setConnectionMessage(message, focus = false) {
  connectionMessage = message;
  // Saving or testing must never change the settings pane the reader chose.
  if (!document.querySelector('[data-setting-pane="native"]').hidden) {
    if (focus) connectionPage = connectionSteps.indexOf('message');
    renderConnection();
  }
}
function renderConnection() {
  if (document.querySelector('[data-setting-pane="native"]').hidden) return;
  const hub = connectionSource === 'hub', sourceName = hub ? 'Hub' : '归档';
  const urlInput = $(hub ? 'hub-url' : 'archive-url'), secretInput = $(hub ? 'hub-secret' : 'archive-secret');
  const normalized = normalizeConnectionAddress(urlInput.value, connectionSource);
  const clear = hub ? clearHubSecret : clearArchiveSecret, has = hub ? connectionConfig.hasHubSecret : connectionConfig.hasArchiveSecret;
  const savedUrl = hub ? connectionConfig.hubUrl : connectionConfig.archiveUrl;
  const secretName = hub ? '密钥' : '查看令牌';
  const canKeepSecret = has && !normalized.error && normalized.origin === normalizeConnectionAddress(savedUrl, connectionSource).origin;
  const willDisableSource = !normalized.error && !normalized.value && Boolean(savedUrl || has);
  const secretState = willDisableSource ? '保存时停用来源并清除' + secretName + '，无需填写' : clear ? '保存时清除' : secretInput.value ? '已输入替换值' : has ? canKeepSecret ? '已保存，留空保留' : '来源已变更，需重新输入' : '尚未保存';
  const dirty = connectionIsDirty(), testing = connectionTester.pending;
  const state = testing ? '测试中，请稍候…' : dirty ? '有未保存的修改' : '当前为已保存配置';
  const preview = normalized.error || (normalized.value ? '将连接：' + normalized.value + (normalized.defaultPort ? '（默认端口）' : '') : hub ? '填写电脑的局域网 IP；裸地址使用端口 17321。' : '归档可选，留空即可。');
  const localWarning = normalized.loopback ? 'localhost / 127.0.0.1 指向墨水屏设备本身。电脑服务请填写电脑的局域网 IP。' : '';
  const liveState = hub ? $('connection').textContent : archiveError ? '读取失败，保留缓存' : archiveReceivedAt ? archiveCached ? '本机缓存' : '已响应' : savedUrl ? '尚未读取' : '未配置';
  const summary = sourceName + '连接：' + liveState + '。已保存来源：' + (savedUrl || '未配置') + '。' + secretName + '：' + secretState + '。' + state + '。';
  document.querySelectorAll('[data-connection-source]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.connectionSource === connectionSource)));
  $('hub-url').hidden = !hub; $('archive-url').hidden = hub; $('hub-secret').hidden = !hub; $('archive-secret').hidden = hub;
  $('connection-address-label').htmlFor = urlInput.id;
  $('connection-address-label').textContent = sourceName + ' 地址' + (dirty ? ' · 未保存' : '');
  $('connection-secret-label').htmlFor = secretInput.id;
  $('connection-secret-label').textContent = sourceName + ' ' + secretName;
  secretInput.placeholder = willDisableSource ? '保存时停用，无需填写' : clear ? '保存时清除' : canKeepSecret ? '留空保留已有值' : has ? '来源已变更，请重新输入' : '填写' + secretName;
  $('connection-saved').textContent = liveState + ' · 已保存：' + (savedUrl || '未配置') + ' · ' + secretName + (has ? '已保存' : '未保存');
  $('connection-address-preview').textContent = [preview, localWarning].filter(Boolean).join('。');
  $('connection-secret-state').textContent = secretState;
  $('connection-secret-clear').textContent = clear ? '撤销清除' : '清除' + secretName;
  $('connection-secret-clear').disabled = testing || (!has && !secretInput.value && !clear);
  $('connection-edit-state').textContent = state;
  $('connection-test').textContent = testing ? '测试中…' : '测试当前输入';
  $('connection-test').disabled = testing;
  $('connection-save').disabled = testing;
  $('connection-cancel').textContent = testing ? '取消测试 / 修改' : '取消修改';
  $('connection-cancel').disabled = !dirty && !testing && !connectionMessage;

  const pager = $('native-connection').querySelector('.setting-pager');
  // Measure with navigation reserved, so keyboard resizing cannot oscillate modes.
  pager.hidden = false;
  const pages = $('connection-pages'), height = pages.clientHeight, width = pages.clientWidth;
  const wasExpanded = connectionExpanded;
  const oldStep = connectionSteps[connectionPage] || 'message';
  connectionExpanded = (height >= 250 && width >= 460) || (height >= 390 && width >= 260);
  const splitActions = !connectionExpanded && (height < 100 || width < 220);
  const nextSteps = ['address', 'secret', ...(!connectionExpanded && height < 100 ? ['clear'] : []), ...(splitActions ? ['test', 'save', 'cancel'] : ['actions']), 'message'];
  if (nextSteps.join() !== connectionSteps.join()) {
    connectionPage = nextSteps.indexOf(oldStep);
    if (connectionPage < 0) connectionPage = nextSteps.indexOf('test');
    if (connectionPage < 0) connectionPage = nextSteps.indexOf('actions');
  }
  connectionSteps = nextSteps;
  connectionPage = Math.max(0, connectionPage);
  const messageStart = connectionSteps.indexOf('message');
  // The keyboard can collapse the full form while a field keeps focus. Keep
  // that field's step visible; later resize events then retain the same step.
  if (wasExpanded && !connectionExpanded) {
    if (document.activeElement === urlInput) connectionPage = 0;
    else if (document.activeElement === secretInput) connectionPage = 1;
  }
  pages.dataset.layout = connectionExpanded ? 'expanded' : 'compact';
  pages.dataset.columns = connectionExpanded && width >= 460 ? '2' : '1';
  pages.dataset.tight = String(height < 100);
  const step = connectionSteps[Math.min(messageStart, connectionPage)];
  pages.dataset.step = step;
  const actionStep = ['test', 'save', 'cancel', 'actions'].includes(step);
  document.querySelectorAll('[data-connection-page]').forEach(pane => {
    pane.hidden = !connectionExpanded && pane.dataset.connectionPage !== (actionStep ? 'actions' : step === 'clear' ? 'secret' : step);
  });
  ['test', 'save', 'cancel'].forEach(action => { $('connection-' + action).hidden = !connectionExpanded && splitActions && step !== action; });
  const messageNode = $('connection-message');
  if (connectionExpanded) $('connection-address-preview').textContent = preview;
  const previewNode = $('connection-address-preview');
  const previewOverflows = previewNode.scrollHeight > previewNode.clientHeight + 1 || previewNode.scrollWidth > previewNode.clientWidth + 1;
  const feedback = connectionExpanded
    ? [connectionMessage || '先测试当前输入，确认后保存。测试不会修改已保存配置。', localWarning, previewOverflows ? preview : ''].filter(Boolean).join('\n')
    : [connectionMessage, summary, preview, localWarning].filter(Boolean).join('\n');
  connectionMessagePages = splitText(feedback, messageNode);
  // Expanded forms keep the fields visible and use the pager for long feedback.
  if (connectionExpanded) connectionPage = Math.max(messageStart, Math.min(connectionPage, messageStart + connectionMessagePages.length - 1));
  else connectionPage = Math.min(connectionPage, messageStart + connectionMessagePages.length - 1);
  if (connectionExpanded || connectionPage >= messageStart) messageNode.textContent = connectionMessagePages[Math.max(0, connectionPage - messageStart)];
  const labels = { address: '地址', secret: '密钥', clear: '密钥状态', actions: '操作', test: '测试', save: '保存', cancel: '取消' };
  const total = connectionExpanded ? connectionMessagePages.length : messageStart + connectionMessagePages.length;
  const position = connectionExpanded ? connectionPage - messageStart + 1 : connectionPage + 1;
  $('connection-position').textContent = (connectionExpanded ? '结果 ' : labels[step] ? labels[step] + ' ' : '结果 ') + position + '/' + total;
  $('connection-prev').disabled = connectionExpanded ? connectionPage <= messageStart : connectionPage === 0;
  $('connection-next').disabled = position >= total;
  $('connection-next').textContent = connectionExpanded || connectionPage >= messageStart ? '下一页' : '下一步';
  pager.hidden = connectionExpanded && connectionMessagePages.length === 1;
  if (!$('settings').hidden) displayRefresh.request('content');
}
function resetSourceCaches() {
  [cacheKey, historyCacheKey, archiveCacheKey].forEach(key => { try { localStorage.removeItem(key); } catch (_) { /* Optional storage. */ } });
  snapshot = null; receivedAt = null; history = null; historyReceivedAt = null; historyRevision = null; historyError = ''; historyCached = true;
  archive = null; archiveReceivedAt = null; archiveError = ''; archiveCached = true; archiveChecked = false; archiveAttemptAt = 0;
  dashboard.clear(); render();
}
{
  document.querySelectorAll('[data-connection-source]').forEach(button => button.addEventListener('click', () => {
    cancelConnectionTest(); connectionSource = button.dataset.connectionSource; connectionPage = 0; connectionMessage = ''; renderConnection();
  }));
  $('connection-prev').addEventListener('click', () => { connectionPage = Math.max(0, connectionPage - 1); renderConnection(); });
  $('connection-next').addEventListener('click', () => { connectionPage++; renderConnection(); });
  $('connection-secret-clear').addEventListener('click', () => {
    cancelConnectionTest(); nativePendingVerification = false; connectionMessage = '';
    if (connectionSource === 'hub') { clearHubSecret = !clearHubSecret; $('hub-secret').value = ''; }
    else { clearArchiveSecret = !clearArchiveSecret; $('archive-secret').value = ''; }
    renderConnection();
  });
  ['hub-url', 'archive-url', 'hub-secret', 'archive-secret'].forEach(id => $(id).addEventListener('input', () => {
    cancelConnectionTest(); nativePendingVerification = false; connectionMessage = '';
    if (id === 'hub-secret') clearHubSecret = false;
    if (id === 'archive-secret') clearArchiveSecret = false;
    renderConnection();
  }));
  ['hub-url', 'archive-url'].forEach(id => $(id).addEventListener('keydown', event => {
    if (event.key !== 'Enter') return;
    event.preventDefault(); connectionPage = 1; renderConnection(); $(id === 'hub-url' ? 'hub-secret' : 'archive-secret').focus();
  }));
  ['hub-secret', 'archive-secret'].forEach(id => $(id).addEventListener('keydown', event => {
    if (event.key !== 'Enter') return;
    event.preventDefault(); $(id).blur(); connectionPage = Math.max(2, connectionSteps.indexOf('test')); renderConnection(); $('connection-test').focus();
  }));
  $('connection-cancel').addEventListener('click', () => {
    restoreConnectionInputs(); connectionPage = 0; renderConnection();
  });
  $('connection-save').addEventListener('click', () => {
    if (connectionTester.pending) return;
    try {
      const next = saveNativeConnection(nativeBridge, currentConnectionPayload());
      connectionConfig = next;
      if (!sourceFence.change(sourceKey(connectionConfig))) sourceFence.invalidate();
      transport.cancel(); resetSourceCaches();
      try { localStorage.setItem(sourceStampKey, sourceKey(connectionConfig)); } catch (_) { /* Optional storage. */ }
      restoreConnectionInputs();
      nativePendingVerification = Boolean(connectionConfig.hubUrl);
      setConnectionMessage(connectionConfig.hubUrl ? '已保存，正在读取数据…' : '已保存。Hub 来源未配置。', true);
      refresh(true);
    } catch (error) { setConnectionMessage(error.message || '连接配置未保存，请重试。', true); }
  });
  $('connection-test').addEventListener('click', () => {
    if (connectionTester.pending) return;
    if (typeof nativeBridge.testConnection !== 'function') {
      setConnectionMessage('此应用版本不能测试未保存的输入。请更新应用，或明确使用「保存并连接」。', true); return;
    }
    try {
      const payload = currentConnectionPayload(connectionSource); payload.source = connectionSource;
      const address = normalizeConnectionAddress(connectionSource === 'hub' ? $('hub-url').value : $('archive-url').value, connectionSource);
      if (!address.value) { setConnectionMessage('请先填写' + (connectionSource === 'hub' ? ' Hub' : '归档') + '地址。', true); return; }
      if (document.activeElement?.matches('#native-connection input')) document.activeElement.blur();
      connectionMessage = '正在测试当前输入；测试不会保存配置。';
      connectionTester.start(payload, address.origin);
      connectionPage = connectionSteps.indexOf('message'); renderConnection();
    } catch (error) { setConnectionMessage(error.message || '无法开始测试，请检查输入。', true); }
  });
}
window.tokenMonitorBack = () => {
  if (!$('settings').hidden) {
    if (document.activeElement?.matches('#native-connection input')) { document.activeElement.blur(); return true; }
    if (!document.querySelector('[data-setting-pane="native"]').hidden && !connectionExpanded && connectionPage > 0) { connectionPage = 0; renderConnection(); return true; }
    closeSettings(); return true;
  }
  return dashboard.back();
};
function sizeViewport() {
  const viewport = window.visualViewport;
  document.documentElement.style.setProperty('--app-height', Math.floor(viewport ? viewport.height : innerHeight) + 'px');
  if (!$('settings').hidden) { updateDetailPages(); renderConnection(); }
}
sizeViewport();
window.addEventListener('resize', sizeViewport);
if (window.visualViewport) window.visualViewport.addEventListener('resize', sizeViewport);
// All dashboard navigation is paginated. Never pan the viewport or a nested list.
document.addEventListener('wheel', e => e.preventDefault(), { passive: false });
document.addEventListener('touchmove', e => e.preventDefault(), { passive: false });
const cancelDashboardSwipe = installPageSwipe($('dashboard'), delta => dashboard.turn(delta), () => $('settings').hidden && !isHidden());
const cancelSettingsSwipe = installPageSwipe(document.querySelector('.settings-body'), delta => {
  const pane = document.querySelector('[data-setting-pane]:not([hidden])')?.dataset.settingPane;
  const prefix = pane === 'connection' ? 'detail' : pane === 'native' ? 'connection' : null;
  if (prefix) $(prefix + (delta > 0 ? '-next' : '-prev')).click();
}, () => !$('settings').hidden && !isHidden());
document.addEventListener('keydown', e => {
  if (e.defaultPrevented) return;
  if (e.key === 'Escape') { if (window.tokenMonitorBack()) e.preventDefault(); return; }
  if (e.key === 'Tab' && !$('settings').hidden) {
    const controls = settingsFocusables(), first = controls[0], last = controls[controls.length - 1];
    if (!$('settings').contains(document.activeElement) || (e.shiftKey ? document.activeElement === first : document.activeElement === last)) {
      e.preventDefault(); (e.shiftKey ? last : first)?.focus();
    }
    return;
  }
  if (e.target.matches('input,textarea')) return;
  if (['PageDown', 'PageUp', 'ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) e.preventDefault();
  if ($('settings').hidden && ['PageDown', 'PageUp', 'ArrowLeft', 'ArrowRight'].includes(e.key)) {
    e.preventDefault(); $('page-' + (['PageUp', 'ArrowLeft'].includes(e.key) ? 'prev' : 'next')).click();
  }

});

try { const saved = localStorage.getItem(intervalKey); if (['0', '60', '300'].includes(saved)) $('interval').value = saved; } catch (_) { /* Optional storage. */ }
showInterval();
{
  $('hub-url').value = connectionConfig.hubUrl; $('archive-url').value = connectionConfig.archiveUrl;
  try {
    const key = sourceKey(connectionConfig);
    if (localStorage.getItem(sourceStampKey) !== key) [cacheKey, historyCacheKey, archiveCacheKey].forEach(k => localStorage.removeItem(k));
    localStorage.setItem(sourceStampKey, key);
  } catch (_) { /* In-memory state remains isolated. */ }
}
readCache();
if (!connectionConfig.hubUrl) { notice('未配置', '请填写 Hub 地址并保存。'); showSettings('native'); }
else refresh();
