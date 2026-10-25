import { absoluteTime } from './model.mjs';
import { projectDashboardStats, projectHistoryForCache, projectArchiveForCache } from './dashboard-model.mjs';
import { createDashboard } from './dashboard.js';
import { themeKey, accentKey, ACCENTS, normalizeTheme, normalizeAccent } from './theme.mjs';

const $ = id => document.getElementById(id);
const cacheKey = 'token-monitor-eink-snapshot-v1';
const historyCacheKey = 'token-monitor-eink-history-v1';
const archiveCacheKey = 'token-monitor-eink-session-archive-v1';
const intervalKey = 'token-monitor-eink-interval-v1';
const tokenKey = 'token-monitor-eink-view-token-v1';
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
let token = '';
try { token = sessionStorage.getItem(tokenKey) || ''; } catch (_) { /* Storage is optional. */ }
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
let collapseSettingsOnSuccess = false;
let cacheEpoch = 0;

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
  setText($('connection'), label); setText($('detail'), detailText);
  setText($('last-time'), receivedAt ? new Date(receivedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }) : '');
  setText($('last-success'), receivedAt ? absoluteTime(receivedAt) : '尚无数据');
}
function showInterval() {
  const seconds = Number($('interval').value);
  setText($('interval-current'), seconds === 0 ? '手动刷新' : seconds === 60 ? '每 60 秒' : '每 5 分钟');
  document.querySelectorAll('[data-interval]').forEach(b => b.setAttribute('aria-pressed', String(Number(b.dataset.interval) === seconds)));
}
function schedule() {
  clearTimeout(timer); const seconds = Number($('interval').value);
  if (seconds > 0 && !document.hidden) timer = setTimeout(refresh, seconds * 1000);
}
function validTime(value) { return typeof value === 'string' && Number.isFinite(Date.parse(value)); }
function save(key, value) { try { localStorage.setItem(key, JSON.stringify(value)); } catch (_) { /* Reading works without persistent storage. */ } }
function readCache() {
  try {
    const value = JSON.parse(localStorage.getItem(archiveCacheKey));
    if (value && validTime(value.receivedAt)) {
      archive = projectArchiveForCache(value.archive); archiveReceivedAt = value.receivedAt;
      save(archiveCacheKey, { archive, receivedAt: archiveReceivedAt });
    }
  } catch (_) { /* Old or invalid archive cache is ignored. */ }
  try {
    const value = JSON.parse(localStorage.getItem(historyCacheKey));
    if (value && validTime(value.receivedAt)) {
      history = projectHistoryForCache(value.history); historyReceivedAt = value.receivedAt;
      historyRevision = typeof value.revision === 'string' ? value.revision : null;
      save(historyCacheKey, { history, receivedAt: historyReceivedAt, revision: historyRevision });
    }
  } catch (_) { /* Old or invalid cache is ignored. */ }
  try {
    const value = JSON.parse(localStorage.getItem(cacheKey));
    if (value && validTime(value.receivedAt)) {
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
async function getJSON(path) {
  const controller = new AbortController(); const timeout = setTimeout(() => controller.abort(), 10000);
  try {
    const response = await fetch(path, { headers: token ? { Authorization: 'Bearer ' + token } : {}, signal: controller.signal, cache: 'no-store', credentials: 'omit' });
    if (!response.ok) {
      let message = '服务暂时不可用';
      try { const body = await response.json(); if (typeof body.message === 'string') message = body.message; } catch (_) { /* Keep fallback. */ }
      if (response.status === 401) { showSettings('token'); try { sessionStorage.removeItem(tokenKey); } catch (_) { /* Optional storage. */ } }
      const error = new Error(response.status === 401 ? '查看令牌无效，请重新填写。' : message);
      error.kind = response.status === 401 ? '认证失败' : response.status >= 500 ? '上游或服务错误' : '读取失败'; throw error;
    }
    return await response.json();
  } finally { clearTimeout(timeout); }
}
async function readHistory(stats, epoch) {
  try {
    const nextHistory = projectHistoryForCache(await getJSON('/api/history'));
    if (epoch !== cacheEpoch) return;
    history = nextHistory; historyReceivedAt = new Date().toISOString(); historyRevision = stats.historyRevision;
    historyError = ''; historyCached = false;
    save(historyCacheKey, { history, receivedAt: historyReceivedAt, revision: historyRevision });
  } catch (error) {
    if (epoch !== cacheEpoch) return;
    historyError = requestFailure(error).detail; historyCached = true;
  }
  render();
}
async function readArchive(epoch) {
  archiveChecked = true; archiveAttemptAt = Date.now();
  try {
    const nextArchive = projectArchiveForCache(await getJSON('/api/session-archive'));
    if (epoch !== cacheEpoch) return;
    archive = nextArchive; archiveReceivedAt = new Date().toISOString(); archiveError = ''; archiveCached = false;
    save(archiveCacheKey, { archive, receivedAt: archiveReceivedAt });
  } catch (error) {
    if (epoch !== cacheEpoch) return;
    archiveError = requestFailure(error).detail; archiveCached = true;
  }
  render();
}
async function refresh(forceArchive = false) {
  if (document.hidden) return;
  if (inFlight) { refreshQueued = true; archiveForceQueued = archiveForceQueued || forceArchive; return; }
  clearTimeout(timer); inFlight = true; $('refresh').disabled = true;
  const epoch = cacheEpoch;
  try {
    const stats = projectDashboardStats(await getJSON('/api/stats'));
    if (epoch !== cacheEpoch) return;
    snapshot = stats; receivedAt = new Date().toISOString();
    save(cacheKey, { stats, receivedAt });
    try { if (token) sessionStorage.setItem(tokenKey, token); else sessionStorage.removeItem(tokenKey); } catch (_) { /* Optional storage. */ }
    if (collapseSettingsOnSuccess) {
      if (!document.querySelector('[data-setting-pane="token"]').hidden) closeSettings();
      collapseSettingsOnSuccess = false;
    }
    render(); notice('已连接', '用量已更新；额度为各账号采集快照。');
    // Optional sources commit independently; neither failure discards stats.
    const optional = [];
    if (!history || historyRevision !== stats.historyRevision || historyError) optional.push(readHistory(stats, epoch));
    if (forceArchive || !archiveChecked || Date.now() - archiveAttemptAt >= 180000) optional.push(readArchive(epoch));
    await Promise.allSettled(optional);
    if (epoch !== cacheEpoch) return;
    const issues = [];
    if (historyError) issues.push('完整历史');
    if (archiveError) issues.push('本机归档');
    notice('已连接', issues.length ? '用量已更新；' + issues.join('、') + '读取失败，保留可用缓存或预览。' : '用量已更新；额度与近期会话为最新观测。');
  } catch (error) {
    if (epoch !== cacheEpoch) return;
    const failure = requestFailure(error);
    historyCached = true; archiveCached = true; render();
    notice(failure.title, failure.detail + (snapshot ? ' 当前显示本机保留快照，不代表在线采集。' : ' 当前没有可显示的数据。'));
  } finally {
    inFlight = false; $('refresh').disabled = false;
    if (refreshQueued && !document.hidden) { const force = archiveForceQueued; refreshQueued = false; archiveForceQueued = false; refresh(force); }
    else { refreshQueued = false; archiveForceQueued = false; schedule(); }
  }
}
$('auth-form').addEventListener('submit', event => {
  event.preventDefault(); collapseSettingsOnSuccess = true; token = $('token').value.trim(); $('token').value = ''; refresh(true);
});
$('refresh').addEventListener('click', () => refresh(true));
$('interval').addEventListener('change', () => { try { localStorage.setItem(intervalKey, $('interval').value); } catch (_) { /* Optional storage. */ } showInterval(); schedule(); });
$('clear-cache').addEventListener('click', () => {
  cacheEpoch++; let removed = true;
  [cacheKey, historyCacheKey, archiveCacheKey].forEach(key => { try { localStorage.removeItem(key); } catch (_) { removed = false; } });
  snapshot = null; receivedAt = null; history = null; historyReceivedAt = null; historyRevision = null; historyError = ''; historyCached = true; archive = null; archiveReceivedAt = null; archiveError = ''; archiveCached = true; archiveChecked = false; archiveAttemptAt = 0; dashboard.clear();
  notice('当前快照已清除', removed ? '用量、历史与本机会话归档缓存已清除；刷新后重新读取。' : '页面已清空，但浏览器未允许清除部分持久缓存，重新打开可能恢复旧数据。');
});
document.addEventListener('visibilitychange', () => { if (document.hidden) clearTimeout(timer); else refresh(); });

function showSettings(page = 'refresh') {
  $('settings').hidden = false;
  document.querySelectorAll('[data-setting]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.setting === page)));
  document.querySelectorAll('[data-setting-pane]').forEach(p => { p.hidden = p.dataset.settingPane !== page; });
}
function closeSettings() {
  if ($('settings').contains(document.activeElement)) document.activeElement.blur();
  $('settings').hidden = true;
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
function sizeViewport() {
  const viewport = window.visualViewport;
  document.documentElement.style.setProperty('--app-height', Math.floor(viewport ? viewport.height : innerHeight) + 'px');
}
sizeViewport();
window.addEventListener('resize', sizeViewport);
if (window.visualViewport) window.visualViewport.addEventListener('resize', sizeViewport);
// All dashboard navigation is paginated. Never pan the viewport or a nested list.
document.addEventListener('wheel', e => e.preventDefault(), { passive: false });
document.addEventListener('touchmove', e => e.preventDefault(), { passive: false });
document.addEventListener('keydown', e => {
  if (e.target.matches('input,textarea')) return;
  if (['PageDown', 'PageUp', 'ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) e.preventDefault();
  if ($('settings').hidden && ['PageDown', 'PageUp', 'ArrowLeft', 'ArrowRight'].includes(e.key)) {
    e.preventDefault(); $('page-' + (['PageUp', 'ArrowLeft'].includes(e.key) ? 'prev' : 'next')).click();
  }
  if (e.key === 'Escape') closeSettings();
});

try { const saved = localStorage.getItem(intervalKey); if (['0', '60', '300'].includes(saved)) $('interval').value = saved; } catch (_) { /* Optional storage. */ }
showInterval(); readCache(); refresh();
