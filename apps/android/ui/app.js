import { publicConnection, connectionPayload, sourceKey, createSourceFence } from './connection.mjs';
import { nativeConnection, saveNativeConnection, createTransport } from './transport.mjs';
import { absoluteTime } from './model.mjs';
import { projectDashboardStats, projectHistoryForCache, projectArchiveForCache } from './dashboard-model.mjs';
import { createDashboard } from './dashboard.js';
import { themeKey, accentKey, ACCENTS, normalizeTheme, normalizeAccent } from './theme.mjs';

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
let detailPage = 0, detailMessage = '正在连接', detailPages = ['正在连接'];
const sourceStampKey = 'token-monitor-eink-native-source-v1';
try { connectionConfig = nativeConnection(nativeBridge); } catch (_) { connectionMessage = '无法读取连接配置，请重新保存。'; }
const sourceFence = createSourceFence(sourceKey(connectionConfig));
function isHidden() { return document.hidden || nativeHidden; }

function showTheme(value, accentValue) {
  const theme = normalizeTheme(value), accent = normalizeAccent(accentValue);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.accent = accent;
  document.querySelectorAll('[data-theme-choice]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.themeChoice === theme)));
  document.querySelectorAll('[data-accent-choice]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.accentChoice === accent)));
  $('accent-options').hidden = theme !== 'color';
  $('theme-caption').textContent = theme === 'color' ? ACCENTS.find(a => a.id === accent).label + ' · 16 阶' : '16 级灰度';
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
const dashboard = createDashboard($('dashboard'));
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

function setText(node, text) { if (node.textContent !== text) node.textContent = text; }
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
  setText($('last-time'), receivedAt ? new Date(receivedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }) : '');
  updateDetailPages();
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
    if (!history || historyRevision !== stats.historyRevision || historyError) optional.push(readHistory(stats, epoch));
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
$('clear-cache').addEventListener('click', () => {
  sourceFence.invalidate(); transport.cancel(); let removed = true;
  [cacheKey, historyCacheKey, archiveCacheKey].forEach(key => { try { localStorage.removeItem(key); } catch (_) { removed = false; } });
  snapshot = null; receivedAt = null; history = null; historyReceivedAt = null; historyRevision = null; historyError = ''; historyCached = true; archive = null; archiveReceivedAt = null; archiveError = ''; archiveCached = true; archiveChecked = false; archiveAttemptAt = 0; dashboard.clear();
  notice('当前快照已清除', removed ? '用量、历史与本机会话归档缓存已清除；刷新后重新读取。' : '界面已清空，但未允许清除部分持久缓存，重新打开可能恢复旧数据。');
});
function visibilityChanged() { if (isHidden()) clearTimeout(timer); else refresh(); }
document.addEventListener('visibilitychange', visibilityChanged);
window.addEventListener('tokenmonitor:visibility', event => { nativeHidden = event.detail?.hidden === true; visibilityChanged(); });

function showSettings(page = 'refresh') {
  $('settings').hidden = false;
  document.querySelectorAll('[data-setting]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.setting === page)));
  document.querySelectorAll('[data-setting-pane]').forEach(p => { p.hidden = p.dataset.settingPane !== page; });
  if (page === 'native') renderConnection();
  if (page === 'connection') updateDetailPages();
}
function closeSettings() {
  if ($('settings').contains(document.activeElement)) document.activeElement.blur();
  $('settings').hidden = true;
  $('hub-secret').value = ''; $('archive-secret').value = ''; clearHubSecret = false; clearArchiveSecret = false;
}
$('settings-open').addEventListener('click', () => showSettings());
$('settings-close').addEventListener('click', closeSettings);
$('status-toggle').addEventListener('click', () => showSettings('connection'));
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
}
$('detail-prev').addEventListener('click', () => { detailPage = Math.max(0, detailPage - 1); updateDetailPages(); });
$('detail-next').addEventListener('click', () => { detailPage = Math.min(detailPages.length - 1, detailPage + 1); updateDetailPages(); });
function setConnectionMessage(message, focus = false) {
  connectionMessage = message;
  // Saving or refreshing must never change the settings page the reader chose.
  if (!document.querySelector('[data-setting-pane="native"]').hidden) {
    if (focus) connectionPage = 3;
    renderConnection();
  }
}
function renderConnection() {
  if (document.querySelector('[data-setting-pane="native"]').hidden) return;
  const hub = connectionSource === 'hub';
  document.querySelectorAll('[data-connection-source]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.connectionSource === connectionSource)));
  $('hub-url').hidden = !hub; $('archive-url').hidden = hub;
  $('hub-secret').hidden = !hub; $('archive-secret').hidden = hub;
  const clear = hub ? clearHubSecret : clearArchiveSecret, has = hub ? connectionConfig.hasHubSecret : connectionConfig.hasArchiveSecret;
  $(hub ? 'hub-secret' : 'archive-secret').placeholder = clear ? '保存时清除密钥' : has ? '已保存，留空保留' : hub ? 'Hub 密钥' : '归档查看令牌';
  $('connection-secret-clear').textContent = clear ? '保留' : '清除';
  const names = ['address', 'secret', 'actions', 'message'];
  document.querySelectorAll('[data-connection-page]').forEach(p => { p.hidden = p.dataset.connectionPage !== names[Math.min(3, connectionPage)]; });
  const summary = hub ? 'Hub：' + (connectionConfig.hubUrl || '未配置') + '。密钥：' + (connectionConfig.hasHubSecret ? '已保存' : '未配置') : '归档：' + (connectionConfig.archiveUrl || '来源未配置') + '。查看令牌：' + (connectionConfig.hasArchiveSecret ? '已保存' : '未配置');
  connectionMessagePages = connectionPage >= 3 ? splitText(connectionMessage || summary, $('connection-message')) : [''];
  if (connectionPage >= 3) {
    connectionPage = Math.min(connectionPage, connectionMessagePages.length + 2);
    $('connection-message').textContent = connectionMessagePages[connectionPage - 3];
  }
  $('connection-position').textContent = connectionPage < 3 ? ['地址', '密钥', '连接'][connectionPage] : (connectionPage - 2) + '/' + connectionMessagePages.length;
  $('connection-prev').disabled = connectionPage === 0;
  $('connection-next').disabled = connectionPage >= 3 && connectionPage >= connectionMessagePages.length + 2;
}
function resetSourceCaches() {
  [cacheKey, historyCacheKey, archiveCacheKey].forEach(key => { try { localStorage.removeItem(key); } catch (_) { /* Optional storage. */ } });
  snapshot = null; receivedAt = null; history = null; historyReceivedAt = null; historyRevision = null; historyError = ''; historyCached = true;
  archive = null; archiveReceivedAt = null; archiveError = ''; archiveCached = true; archiveChecked = false; archiveAttemptAt = 0;
  dashboard.clear(); render();
}
{
  document.querySelectorAll('[data-connection-source]').forEach(b => b.addEventListener('click', () => { connectionSource = b.dataset.connectionSource; connectionPage = 0; connectionMessage = ''; renderConnection(); }));
  $('connection-prev').addEventListener('click', () => { connectionPage = Math.max(0, connectionPage - 1); renderConnection(); });
  $('connection-next').addEventListener('click', () => { connectionPage++; renderConnection(); });
  $('connection-secret-clear').addEventListener('click', () => {
    if (connectionSource === 'hub') { clearHubSecret = !clearHubSecret; $('hub-secret').value = ''; }
    else { clearArchiveSecret = !clearArchiveSecret; $('archive-secret').value = ''; }
    renderConnection();
  });
  ['hub-secret', 'archive-secret'].forEach(id => $(id).addEventListener('input', () => { if (id === 'hub-secret') clearHubSecret = false; else clearArchiveSecret = false; }));
  $('connection-save').addEventListener('click', () => {
    try {
      connectionConfig = saveNativeConnection(nativeBridge, connectionPayload($('hub-url').value, $('hub-secret').value, clearHubSecret, $('archive-url').value, $('archive-secret').value, clearArchiveSecret));
      if (!sourceFence.change(sourceKey(connectionConfig))) sourceFence.invalidate();
      transport.cancel(); resetSourceCaches();
      try { localStorage.setItem(sourceStampKey, sourceKey(connectionConfig)); } catch (_) { /* Optional storage. */ }
      $('hub-url').value = connectionConfig.hubUrl; $('archive-url').value = connectionConfig.archiveUrl;
      clearHubSecret = false; clearArchiveSecret = false;
      nativePendingVerification = Boolean(connectionConfig.hubUrl);
      setConnectionMessage(connectionConfig.hubUrl ? '已保存，正在连接…' : '已保存。Hub 来源未配置。', true);
      refresh(true);
    } catch (error) { setConnectionMessage(error.message || '连接配置未保存', true); }
    finally { $('hub-secret').value = ''; $('archive-secret').value = ''; }
  });
  $('connection-test').addEventListener('click', () => {
    nativePendingVerification = Boolean(connectionConfig.hubUrl);
    setConnectionMessage(connectionConfig.hubUrl ? '正在连接…' : 'Hub 来源未配置。', true); refresh(true);
  });
}
window.tokenMonitorBack = () => {
  if (!$('settings').hidden) {
    if (document.activeElement?.matches('#native-connection input')) { document.activeElement.blur(); return true; }
    if (!document.querySelector('[data-setting-pane="native"]').hidden && connectionPage > 0) { connectionPage = 0; renderConnection(); return true; }
    closeSettings(); return true;
  }
  const back = Array.from($('dashboard').querySelectorAll('button')).find(button => button.textContent.trim() === '返回');
  if (back) { back.click(); return true; }
  return false;
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
document.addEventListener('keydown', e => {
  if (e.key === 'Escape') { if (window.tokenMonitorBack()) e.preventDefault(); return; }
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
