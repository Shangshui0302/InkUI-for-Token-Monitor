import { normalizeConnectionAddress } from './connection.mjs';
import { createSettingsConnectionFlow } from './settings-connection.mjs';
import { absoluteTime } from './model.mjs';
import { networkLabel } from './device-status.mjs';

// Settings use the same fixed viewport as the dashboard. A page is a set of
// complete controls; no container scrolls, including when the IME is visible.
export function createSettings({ bridge, getConfig, getStatus, saveConnection, clearCache, refresh, repaint, onConnected = () => {} }) {
  const $ = id => document.getElementById(id);
  const root = $('settings'), body = root.querySelector('.settings-body');
  const pager = $('settings-pager'), fields = $('editor-fields'), actions = root.querySelector('.editor-actions');
  let view = 'connection', tab = 'connection', page = 0, pages = [], returnFocus = null;
  let initialSetup = false, removeSecret = false, feedback = '', editSteps = [], expanded = false;
  let confirmAction = null, confirmFocus = null, layingOut = false, savedMessage = '';
  let summarySignature = '', diagnosticSignature = '';
  const remember = new Map();
  const flow = createSettingsConnectionFlow({ bridge, getConfig, save: saveConnection, onState(state) {
    if (state.phase === 'saved') {
      savedMessage = state.message;
      const enterDashboard = initialSetup && state.verified === true;
      eraseDraft(); initialSetup = false;
      if (enterDashboard) { close(); onConnected(); }
      else { navigate('connection'); update(); }
    } else if (view === 'edit') {
      feedback = state.message;
      layout(state.phase !== 'idle' ? 'feedback' : undefined);
      repaint('content');
    }
  } });
  window.addEventListener('tokenmonitor:connection-test', event => flow.accept(event.detail));

  // Make each appearance/behavior control group independently pageable on a
  // short display, while retaining the two-column composition on a Poke3.
  for (const group of root.querySelectorAll('[data-display-group]')) {
    const wrapper = document.createElement('div'); wrapper.className = 'display-controls';
    const count = group.dataset.displayGroup === 'appearance' ? 2 : 3;
    [...group.children].slice(0, count).forEach(node => wrapper.append(node));
    group.prepend(wrapper);
  }

  function focusables() {
    const within = confirmAction ? $('settings-confirm') : root;
    return [...within.querySelectorAll('button,input,[tabindex]')].filter(node => !node.disabled && node.tabIndex >= 0 && node.getClientRects().length);
  }
  function focusVisible(preferred) {
    if (preferred?.getClientRects().length && !preferred.disabled && preferred.tabIndex >= 0) preferred.focus({ preventScroll: true });
    else $(root.hidden ? 'settings-open' : 'settings-close').focus({ preventScroll: true });
  }
  function dirty() {
    const saved = getConfig().hubUrl;
    return $('connection-url').value.trim() !== saved || Boolean($('connection-secret').value || removeSecret);
  }
  function eraseDraft() {
    $('connection-secret').value = ''; $('connection-secret').type = 'password';
    removeSecret = false; feedback = '';
  }
  function navigate(next) {
    if (view === 'edit' && next !== 'edit') flow.cancel();
    remember.set(view, page); view = next; page = remember.get(next) || 0;
    if (['connection', 'display', 'more'].includes(next)) tab = next;
    root.dataset.view = next;
    $('settings-title').textContent = next === 'edit' ? initialSetup ? '连接 Hub' : '编辑 Hub'
      : next === 'diagnostics' ? '连接诊断' : next === 'about' ? '关于 InkUI' : '设置';
    root.querySelectorAll('[data-setting]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.setting === tab)));
    root.querySelectorAll('[data-setting-pane]').forEach(p => { p.hidden = p.dataset.settingPane !== next; });
    layout(); focusVisible(); repaint('page');
  }
  function open(next = 'connection') {
    if (root.hidden) {
      returnFocus = document.activeElement;
      root.hidden = false; $('app').setAttribute('inert', ''); $('app').setAttribute('aria-hidden', 'true');
    }
    if (view === 'edit' && next !== 'edit' && dirty()) { confirmDiscard(() => navigate(next)); return; }
    if (next === 'connection' && !getConfig().hubUrl) edit();
    else navigate(next);
    update();
  }
  function close() {
    flow.cancel(); eraseDraft(); initialSetup = false;
    if (root.contains(document.activeElement)) document.activeElement.blur();
    root.hidden = true; $('app').removeAttribute('inert'); $('app').removeAttribute('aria-hidden');
    const target = returnFocus; returnFocus = null;
    focusVisible(target || $('settings-open')); repaint('page');
  }
  function confirm(title, message, accept, label, keep = '取消') {
    flow.cancel(); confirmFocus = document.activeElement;
    $('confirm-title').textContent = title; $('confirm-message').textContent = message;
    $('confirm-accept').textContent = label; $('confirm-keep').textContent = keep;
    confirmAction = accept; $('settings-confirm').hidden = false;
    $('confirm-keep').focus({ preventScroll: true }); repaint('page');
  }
  function dismissConfirm() {
    confirmAction = null; $('settings-confirm').hidden = true;
    focusVisible(confirmFocus); confirmFocus = null; repaint('page');
  }
  function confirmDiscard(accept) {
    confirm('放弃修改？', '当前输入尚未保存。', () => { eraseDraft(); accept(); }, '放弃修改', '继续编辑');
  }
  function back() {
    if (root.hidden) return false;
    if (confirmAction) { dismissConfirm(); return true; }
    if (document.activeElement?.matches('#settings input:not([type=hidden])')) { document.activeElement.blur(); return true; }
    if (view === 'edit') {
      if (!expanded && page > 0) { page = 0; layout(); repaint('page'); return true; }
      if (dirty()) confirmDiscard(exitEditor);
      else exitEditor();
    } else if (view === 'diagnostics') navigate('connection');
    else if (view === 'about') navigate('more');
    else close();
    return true;
  }
  function exitEditor() {
    flow.cancel(); eraseDraft();
    if (initialSetup) close(); else navigate('connection');
  }
  function edit() {
    initialSetup = !getConfig().hubUrl;
    savedMessage = ''; flow.cancel(); eraseDraft();
    const config = getConfig();
    $('connection-url').value = config.hubUrl;
    $('connection-url').placeholder = 'IP 或可解析的主机名';
    $('connection-address-label').textContent = 'Hub 地址';
    fields.querySelector('label[for=connection-secret]').textContent = 'Hub 密钥';
    remember.set('edit', 0); navigate('edit');
  }
  function editInfo() {
    const config = getConfig(), savedUrl = config.hubUrl;
    const has = config.hasHubSecret;
    const address = normalizeConnectionAddress($('connection-url').value);
    const same = !address.error && address.origin && address.origin === normalizeConnectionAddress(savedUrl).origin;
    const retained = has && same;
    const typed = Boolean($('connection-secret').value);
    $('secret-input-area').hidden = false;
    $('connection-secret-state').textContent = removeSecret ? '待移除' : typed ? '待保存' : retained ? '已保存' : has ? '需重新填写' : '待填写';
    $('secret-remove').hidden = !retained;
    $('secret-remove').textContent = removeSecret ? '撤销移除' : '移除密钥';
    $('secret-reveal').disabled = !typed;
    $('secret-reveal').textContent = $('connection-secret').type === 'password' ? '显示' : '隐藏';
    $('secret-reveal').setAttribute('aria-pressed', String($('connection-secret').type !== 'password'));
    $('connection-secret').placeholder = removeSecret ? '输入新密钥可撤销移除' : retained ? '留空保留，输入新密钥替换' : '粘贴 Hub 密钥';
    $('connection-secret-help').textContent = removeSecret ? '保存时移除已有密钥' : typed ? '连接成功后保存' : retained ? '原密钥不回显，可直接输入替换' : has ? '新地址不沿用原密钥' : '填写 Hub 提供的密钥';
    $('connection-cancel').textContent = initialSetup ? '稍后设置' : '取消';
    const hint = address.loopback ? '此地址指向墨水屏设备本身。电脑上的 Hub 请填写电脑的局域网 IP。'
      : address.defaultPort ? '将连接 ' + address.value
      : '';
    $('connection-address-hint').textContent = hint; $('connection-address-hint').hidden = !hint;
    const testing = flow.state.phase === 'testing';
    $('connection-save').textContent = testing ? '正在连接…' : initialSetup ? '连接并保存' : '测试并保存';
    $('connection-save').disabled = testing;
    $('connection-offline-save').hidden = !flow.state.canSaveOffline;
    return { hint };
  }

  function overflow(node) { return node.scrollHeight > node.clientHeight + 1 || node.scrollWidth > node.clientWidth + 1; }
  function blocksOverflow(container, units) {
    // CSS grid can shrink a row while a grandchild overflows it. Inspect the
    // complete visible blocks too, rather than trusting the outer scroll size.
    return overflow(container) || units.some(node => !node.hidden && node.getClientRects().length && overflow(node))
      || [...container.querySelectorAll('[data-display-group]')].some(node => !node.hidden && overflow(node));
  }
  function textPages(text, node) {
    const parts = []; let remaining = text;
    while (remaining) {
      let low = 1, high = remaining.length, fit = 1;
      while (low <= high) {
        const mid = Math.floor((low + high) / 2); node.textContent = remaining.slice(0, mid);
        if (!overflow(node)) { fit = mid; low = mid + 1; } else high = mid - 1;
      }
      parts.push(remaining.slice(0, fit)); remaining = remaining.slice(fit);
    }
    return parts.length ? parts : [''];
  }
  function editLayout(focusStep) {
    const info = editInfo();
    const nodes = [...fields.querySelectorAll('[data-editor-step]')];
    const actionNodes = [...actions.children].filter(node => node.id !== 'connection-offline-save' || flow.state.canSaveOffline);
    actions.hidden = false; actions.dataset.page = 'false'; actionNodes.forEach(n => { n.hidden = false; });
    fields.dataset.layout = 'expanded'; $('connection-steps').hidden = true;
    $('connection-editor').dataset.layout = 'expanded';
    nodes.forEach(n => { n.hidden = n.dataset.editorStep === 'feedback' && !feedback; });
    $('connection-feedback').textContent = feedback;
    $('connection-editor').dataset.tight = String(body.clientHeight < 180);
    expanded = body.clientWidth >= 270 && body.clientHeight >= 260 && !overflow(fields) && !overflow(actions);
    if (expanded) { pages = [null]; return; }
    fields.dataset.layout = 'steps'; $('connection-steps').hidden = false;
    $('connection-editor').dataset.layout = 'steps';
    // Full address guidance is in the pageable feedback step. It must not
    // push the input out of a keyboard-compressed address page.
    $('connection-address-hint').hidden = true;
    const pagedActions = body.clientHeight < 150 || body.clientWidth < 260;
    actions.dataset.page = String(pagedActions);
    if (pagedActions) actions.hidden = true;
    const available = fields.clientHeight;
    const feedbackNode = $('connection-feedback');
    feedbackNode.hidden = false; feedbackNode.style.height = Math.max(16, available) + 'px';
    const message = [feedback, info.hint].filter(Boolean).join('。');
    const chunks = message ? textPages(message, feedbackNode) : [];
    editSteps = [{ step: 'address' }, { step: 'secret' },
      ...chunks.map(text => ({ step: 'feedback', text })),
      ...(pagedActions ? actionNodes.map(node => ({ step: 'action', node })) : [])];
    if (focusStep) {
      const target = editSteps.findIndex(item => item.step === focusStep);
      if (target >= 0) page = target;
    } else if (document.activeElement === $('connection-url')) page = 0;
    else if (document.activeElement === $('connection-secret')) page = editSteps.findIndex(item => item.step === 'secret');
    page = Math.max(0, Math.min(page, editSteps.length - 1));
    const current = editSteps[page]; pages = editSteps;
    root.querySelectorAll('[data-editor-jump]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.editorJump === current.step)));
    nodes.forEach(node => { node.hidden = node.dataset.editorStep !== current.step; });
    feedbackNode.style.height = current.step === 'feedback' ? Math.max(16, available) + 'px' : '';
    if (current.step === 'feedback') feedbackNode.textContent = current.text;
    if (pagedActions) {
      fields.hidden = current.step === 'action'; actions.hidden = current.step !== 'action';
      actionNodes.forEach(node => { node.hidden = node !== current.node; });
    } else fields.hidden = false;
  }
  function blockLayout() {
    const pane = root.querySelector('[data-setting-pane="' + view + '"]');
    let units = [...pane.querySelectorAll('[data-settings-block]')];
    if (view === 'display') {
      const groups = [...pane.querySelectorAll('[data-display-group]')];
      groups.forEach(g => { g.hidden = false; [...g.children].forEach(n => { n.hidden = n.id === 'accent-options' && document.documentElement.dataset.theme !== 'color'; }); });
      pane.dataset.paged = 'false';
      if (!overflow(pane) && groups.every(g => !overflow(g))) { pages = [groups]; return; }
      pane.dataset.paged = 'true';
      units = groups.flatMap(g => [...g.children].filter(n => !n.hidden));
    }
    if (view === 'more') $('app-version').hidden = body.clientHeight < 210;
    units.forEach(node => { node.hidden = false; });
    const groups = []; let batch = [];
    // Measure complete controls in their real layout. Never infer pages from
    // a provider count, a screen model or a fixed number of text characters.
    for (const unit of units) {
      units.forEach(n => { n.hidden = !batch.includes(n) && n !== unit; });
      if (batch.length && blocksOverflow(pane, units)) { groups.push(batch); batch = []; }
      batch.push(unit);
    }
    if (batch.length) groups.push(batch);
    pages = groups.length ? groups : [[]]; page = Math.min(page, pages.length - 1);
    units.forEach(node => { node.hidden = !pages[page].includes(node); });
    if (view === 'display') pane.querySelectorAll('[data-display-group]').forEach(g => { g.hidden = ![...g.children].some(n => !n.hidden); });
  }
  function diagnosticLayout() {
    const status = getStatus(), config = getConfig();
    const rows = [['网络', networkLabel(status.deviceStatus)], ['Hub', status.label || '尚未连接'],
      ['地址', config.hubUrl || '未配置'], ['最近成功', status.receivedAt ? absoluteTime(status.receivedAt) : '尚无数据'],
      ['详情', status.detail || '尚无数据'], ['历史', status.historyError || (status.historyReceivedAt ? '已读取 · ' + absoluteTime(status.historyReceivedAt) : '尚未读取')]];
    const signature = JSON.stringify(rows) + ':' + body.clientWidth + ':' + body.clientHeight;
    const copy = $('diagnostic-copy');
    if (signature !== diagnosticSignature) {
      diagnosticSignature = signature; pages = [];
      const dl = document.createElement('dl'), dt = document.createElement('dt'), dd = document.createElement('dd'); dl.append(dt, dd); copy.replaceChildren(dl);
      for (const [label, text] of rows) {
        dt.textContent = label;
        dd.style.height = Math.max(16, copy.clientHeight - dt.offsetHeight - 12) + 'px';
        // One row per page on tiny screens, complete values across as many
        // pages as required. Desktop/Poke layouts pack the generated rows below.
        for (const part of textPages(String(text), dd)) pages.push({ label, text: part });
      }
      copy.replaceChildren();
      for (const row of pages) {
        const block = document.createElement('div'); block.dataset.settingsBlock = '';
        const table = document.createElement('dl'), title = document.createElement('dt'), value = document.createElement('dd');
        title.textContent = row.label; value.textContent = row.text; table.append(title, value); block.append(table); copy.append(block);
      }
    }
    const units = [...copy.children], groups = []; let batch = [];
    units.forEach(n => { n.hidden = false; });
    for (const unit of units) {
      units.forEach(n => { n.hidden = !batch.includes(n) && n !== unit; });
      if (batch.length && blocksOverflow(copy, units)) { groups.push(batch); batch = []; }
      batch.push(unit);
    }
    if (batch.length) groups.push(batch);
    pages = groups.length ? groups : [[]]; page = Math.min(page, pages.length - 1);
    units.forEach(node => { node.hidden = !pages[page].includes(node); });
  }
  function layout(focusStep) {
    if (root.hidden || layingOut) return;
    layingOut = true;
    // Reserve navigation before measuring to avoid keyboard/page-count cycles.
    pager.hidden = false;
    if (view === 'edit') { fields.hidden = false; $('connection-feedback').style.height = ''; editLayout(focusStep); }
    else if (view === 'diagnostics') diagnosticLayout();
    else blockLayout();
    page = Math.max(0, Math.min(page, pages.length - 1));
    $('settings-position').textContent = (view === 'edit' && !expanded ? ({ address:'地址', secret:'密钥', feedback:'结果', action:'操作' }[editSteps[page]?.step] || '') + ' · ' : '') + (page + 1) + ' / ' + pages.length;
    $('settings-prev').disabled = page === 0; $('settings-next').disabled = page >= pages.length - 1;
    pager.hidden = pages.length <= 1;
    layingOut = false;
  }
  function update() {
    const config = getConfig(), status = getStatus();
    const signature = JSON.stringify([config, status, savedMessage]);
    if (signature !== summarySignature) {
      summarySignature = signature;
      $('hub-summary-state').textContent = config.hubUrl ? status.label || '尚未连接' : '未配置';
      $('hub-summary-address').textContent = config.hubUrl || '添加电脑上的 Hub';
      $('hub-summary-time').textContent = status.receivedAt ? '最近成功 ' + absoluteTime(status.receivedAt) : '尚未读取';
      const error = status.state === 'error' && config.hubUrl ? status.detail : savedMessage;
      $('hub-summary-error').textContent = error; $('hub-summary-error').hidden = !error;
      $('hub-edit').textContent = config.hubUrl ? '编辑地址与密钥' : '添加 Hub';
      $('hub-retry').hidden = !config.hubUrl || status.state !== 'error';
    }
    layout();
  }
  function turn(delta) {
    if (root.hidden || confirmAction || pages.length < 2) return;
    if (root.contains(document.activeElement) && document.activeElement.matches('input')) document.activeElement.blur();
    page = Math.max(0, Math.min(pages.length - 1, page + delta)); layout(); repaint('page');
  }
  root.querySelectorAll('[data-setting]').forEach(button => button.addEventListener('click', () => open(button.dataset.setting)));
  let dismissInput = false;
  $('settings-close').addEventListener('pointerdown', () => {
    dismissInput = document.activeElement?.matches('#settings input:not([type=hidden])') === true;
    if (dismissInput) document.activeElement.blur();
  });
  $('settings-close').addEventListener('click', () => {
    if (dismissInput) { dismissInput = false; layout(); return; }
    back();
  });
  $('hub-summary').addEventListener('click', () => navigate('diagnostics'));
  $('hub-edit').addEventListener('click', edit);
  $('hub-retry').addEventListener('click', () => { savedMessage = ''; refresh(); });
  $('diagnostic-retry').addEventListener('click', () => { savedMessage = ''; refresh(); });
  $('about-open').addEventListener('click', () => navigate('about'));
  $('cache-open').addEventListener('click', () => confirm('清除缓存？', '清除用量、历史和会话快照。保留连接、配色和服务商顺序。', () => {
    const removed = clearCache(); savedMessage = removed ? '缓存已清除' : '界面已清空，部分缓存未清除';
    $('cache-open').querySelector('span:not(.row-arrow)').textContent = savedMessage + ' · 保留连接和偏好'; update();
  }, '清除缓存'));
  $('confirm-keep').addEventListener('click', dismissConfirm);
  $('confirm-accept').addEventListener('click', () => { const action = confirmAction; dismissConfirm(); action?.(); });
  $('settings-prev').addEventListener('click', () => turn(-1));
  $('settings-next').addEventListener('click', () => turn(1));
  $('connection-cancel').addEventListener('click', () => {
    if (dirty()) confirmDiscard(exitEditor); else exitEditor();
  });
  root.querySelectorAll('[data-editor-jump]').forEach(button => button.addEventListener('click', () => {
    if (document.activeElement?.matches('#settings input')) document.activeElement.blur();
    layout(button.dataset.editorJump); repaint('page');
  }));
  $('secret-remove').addEventListener('click', () => { flow.cancel(); removeSecret = !removeSecret; $('connection-secret').value = ''; layout('secret'); repaint('content'); });
  $('secret-reveal').addEventListener('click', () => { $('connection-secret').type = $('connection-secret').type === 'password' ? 'text' : 'password'; editInfo(); repaint('content'); });
  for (const id of ['connection-url', 'connection-secret']) $(id).addEventListener('input', () => { flow.cancel(); feedback = ''; if (id === 'connection-secret') removeSecret = false; layout(); });
  $('connection-url').addEventListener('keydown', event => {
    if (event.key !== 'Enter') return; event.preventDefault();
    layout('secret'); if (!$('secret-input-area').hidden) $('connection-secret').focus({ preventScroll: true }); else document.activeElement.blur();
  });
  $('connection-secret').addEventListener('keydown', event => {
    if (event.key !== 'Enter') return; event.preventDefault(); document.activeElement.blur(); layout();
  });
  $('connection-save').addEventListener('click', () => {
    if (document.activeElement?.matches('#settings input')) document.activeElement.blur();
    flow.submit({ source: 'hub', url: $('connection-url').value, secret: $('connection-secret').value, removeSecret });
  });
  $('connection-offline-save').addEventListener('click', () => flow.saveOffline());
  function capabilities() {
    let supported = false;
    try { supported = JSON.parse(bridge.getDisplayCapabilities?.() || '{}').supported === true; } catch (_) { /* System managed. */ }
    $('screen-mode').textContent = supported ? '自动 · 6 次翻页全刷' : '由系统管理';
    $('screen-clear').hidden = !supported; layout();
  }
  $('screen-clear').addEventListener('click', () => repaint('full'));
  window.addEventListener('tokenmonitor:display-capabilities', capabilities); capabilities();
  try {
    const info = JSON.parse(bridge.getAppInfo?.() || '{}');
    const version = typeof info.versionName === 'string' && /^[\w.+-]{1,40}$/.test(info.versionName) ? 'v' + info.versionName : '';
    const build = Number.isInteger(info.versionCode) && info.versionCode > 0 ? ' · 构建 ' + info.versionCode : '';
    const label = version ? version + build : '';
    $('app-version').textContent = label; $('about-version').textContent = label;
  } catch (_) { /* An older APK may omit metadata. */ }
  // Older WebViews ignore inert; enforce focus containment as well.
  document.addEventListener('focusin', event => {
    if (root.hidden) return;
    const scope = confirmAction ? $('settings-confirm') : root;
    if (!scope.contains(event.target)) (focusables()[0] || $('settings-close')).focus({ preventScroll: true });
  });
  document.addEventListener('keydown', event => {
    if (root.hidden || event.defaultPrevented) return;
    if (event.key === 'Tab') {
      const controls = focusables(), first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey ? document.activeElement === first : document.activeElement === last) {
        event.preventDefault(); (event.shiftKey ? last : first)?.focus({ preventScroll: true });
      }
    }
  });
  // Request a refresh only after a settings action actually changes the UI.
  root.addEventListener('click', event => {
    if (!event.target.closest('button') || event.target.closest('#screen-clear')) return;
    const before = root.outerHTML;
    queueMicrotask(() => { if (root.outerHTML !== before) repaint('page'); });
  }, true);
  return { open, back, update, resize: layout, turn, cancelTests: () => flow.cancel(), get opened() { return !root.hidden; } };
}
