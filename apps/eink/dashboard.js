import { normalizeStats, absoluteTime } from './model.mjs';
import { dashboardView } from './dashboard-model.mjs';

const finite = value => typeof value === 'number' && Number.isFinite(value);
const number = new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 2 });
const count = value => finite(value) ? number.format(value) : '—';
const compact = value => !finite(value) ? '—' : Math.abs(value) >= 1e9 ? (value / 1e9).toFixed(2) + 'B' : Math.abs(value) >= 1e6 ? (value / 1e6).toFixed(2) + 'M' : Math.abs(value) >= 1e4 ? (value / 1e3).toFixed(1) + 'K' : count(value);
const usd = value => finite(value) ? '$' + value.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 }) : '—';
const listUsd = value => finite(value) ? '$' + value.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : '';
const known = value => value && !['未知模型', '未知客户端', '未知平台', '未分类', '未命名会话'].includes(value);
function resetIn(value) {
  const at = Date.parse(value);
  if (!Number.isFinite(at)) return '';
  const minutes = Math.max(0, Math.ceil((at - Date.now()) / 60000));
  if (!minutes) return '待重置';
  if (minutes >= 1440) return Math.floor(minutes / 1440) + '天' + (Math.floor(minutes % 1440 / 60) ? Math.floor(minutes % 1440 / 60) + '时' : '') + '后';
  return minutes >= 60 ? Math.floor(minutes / 60) + '时' + (minutes % 60 ? minutes % 60 + '分' : '') + '后' : minutes + '分后';
}
const percentage = value => finite(value) ? (value * 100).toFixed(1) + '%' : '—';
const states = { running: '运行中', ended: '本轮结束', idle: '未运行' };
const periods = { today: '今日', month: '本月', allTime: '累计' };
const svgNS = 'http://www.w3.org/2000/svg';
export function el(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}
function svgEl(tag, attributes) {
  const node = document.createElementNS(svgNS, tag);
  Object.keys(attributes).forEach(key => node.setAttribute(key, attributes[key]));
  return node;
}
function dateKey(value) {
  const d = new Date(value);
  return d.getFullYear() + '-' + String(d.getMonth() + 1).padStart(2, '0') + '-' + String(d.getDate()).padStart(2, '0');
}
function sessionPairs(s) {
  return [['标题', s.title], ['项目', s.project], ['模型', s.model], ['工具', s.client], ['状态', s.archived ? '已归档' : states[s.state]], ['总 tokens', count(s.totalTokens)], ['估算 USD', usd(s.costUsd) + (s.costPartial ? ' · 部分费用' : '')], ['上下文已用', finite(s.contextPercent) ? count(s.contextPercent) + '%' : '—'], ['上下文 / 容量', count(s.contextTokens) + ' / ' + count(s.contextWindow)], ['输入', count(s.inputTokens)], ['输出', count(s.outputTokens)], ['缓存读取', count(s.cacheReadTokens)], ['缓存写入', count(s.cacheWriteTokens)], ['推理', count(s.reasoningTokens)], ['未定价 tokens', count(s.unpricedTokens)], ['消息', count(s.messageCount)], ['最近活动', absoluteTime(s.lastUsedAt)]];
}
function historyPairs(row) {
  return [['日期', row.date || row.month], ['tokens', count(row.tokens)], ['估算 USD', usd(row.cost) + (row.unpricedTokens > 0 ? ' · 部分费用' : '')], ['消息', count(row.messages)], ['活跃分钟', finite(row.activeTimeMs) ? count(row.activeTimeMs / 60000) : '—'], ['输入', count(row.inputTokens)], ['输出', count(row.outputTokens)], ['缓存读取', count(row.cacheReadTokens)], ['缓存写入', count(row.cacheWriteTokens)], ['推理', count(row.reasoningTokens)], ['未分类', count(row.unclassifiedTokens)], ['未定价', count(row.unpricedTokens)]];
}
// Every visible unit is a fixed-height row or a measured chart. Long values
// become numbered text segments in the same pager, rather than hidden overflow.
export function createDashboard(host) {
  let data = null, context = {}, view = null;
  let page = 'overview', period = document.getElementById('period').value || 'today';
  let detailKind = 'models', runningOnly = false, activityKind = 'heat';
  let drill = null, actions = [], pageCount = 1, currentIndex = 0;
  const positions = {};
  let resizeFrame = 0;
  function key() { return drill ? 'drill' : page + ':' + period + ':' + (page === 'details' ? detailKind : page === 'activity' ? activityKind : page === 'sessions' ? runningOnly : ''); }
  function actionButton(label, fn, className) {
    const b = el('button', label, className || 'dash-tab'); b.type = 'button';
    b.dataset.dashAction = String(actions.push(fn) - 1); return b;
  }
  function open(title, pairs) { drill = { title, pairs }; positions.drill = 0; render(); }
  function row(label, value, secondary, fn, className) {
    const node = actionButton(undefined, fn || (() => open(label, [['名称', label], ['数值', value], ['说明', secondary || '—']])), 'dash-row' + (className ? ' ' + className : ''));
    const top = el('span', undefined, 'dash-line');
    top.appendChild(el('span', label, 'dash-name')); top.appendChild(el('strong', value, 'dash-value')); node.appendChild(top);
    if (secondary) node.appendChild(el('span', secondary, 'dash-secondary'));
    return node;
  }
  function tabs(root, choices, selected, change) {
    const controls = el('div', undefined, 'dash-controls');
    choices.forEach(([id, label]) => { const b = actionButton(label, () => { change(id); render(); }); b.setAttribute('aria-pressed', String(id === selected)); controls.appendChild(b); }); root.appendChild(controls);
  }
  function providers() { return normalizeStats(data.stats); }
  function quotaRows() {
    const rows = [];
    providers().forEach(p => {
      const source = data.stats.limits.providers[p.key] || {};
      const plan = source.plan || source.planType || source.subscriptionPlan || '';
      const name = p.name + (plan ? ' · ' + plan : '') + (p.account ? ' · ' + p.account : '');
      const status = (p.stale ? '过期 · ' : '') + (!p.ok ? p.status + ' · ' : '');
      const fields = [['供应商', p.name], ['套餐', plan || '未提供'], ['账号', p.account || '未提供'], ['状态', p.status + (p.stale ? ' · 已过期' : '')], ['来源', p.source || '未提供'], ['采集', absoluteTime(p.updatedAt)]];
      if (!p.windows.length) rows.push(row(p.name, p.status, p.account || '', () => open(name, fields)));
      p.windows.forEach((w, i) => {
        const label = w.label.replace(/(额度|窗口)$/g, '') || w.label;
        const reset = resetIn(w.resetsAt);
        const secondary = [i === 0 ? [plan, p.account].filter(Boolean).join(' · ') : '', status.replace(/ · $/, ''), w.value !== null ? (w.valueLabel === '剩余额度' ? '剩余' : w.valueLabel) : '', reset].filter(Boolean).join(' · ');
        rows.push(row(p.name + ' ' + label, count(w.value) + w.unit, secondary, () => open(name, fields.concat([['窗口', w.label], [w.valueLabel, count(w.value) + w.unit], ['重置', absoluteTime(w.resetsAt)]]))));
      });
    });
    return rows;
  }
  function distributionRow(item) {
    return row(item.name, compact(item.tokens), [finite(item.share) ? percentage(item.share) : '', finite(item.costUsd) ? listUsd(item.costUsd) : ''].filter(Boolean).join(' · '), () => open(item.name, [['名称', item.name], ['tokens', count(item.tokens)], ['占比', percentage(item.share)], ['估算 USD', usd(item.costUsd)]]));
  }
  function sessionRow(item) {
    const label = known(item.project) ? item.project : known(item.title) ? item.title : '会话';
    const secondary = [known(item.model) ? item.model : '', finite(item.contextPercent) ? '已用 ' + count(item.contextPercent) + '%' : '', item.archived ? '已归档' : states[item.state]].filter(Boolean).join(' · ');
    return row(label, compact(item.totalTokens), secondary, () => open('会话用量', sessionPairs(item)), 'dash-session');
  }
  function summaryPairs() {
    const s = view.summary;
    return [['时段', periods[period]], ['总 tokens', count(s.totalTokens)], ['估算 USD', usd(s.costUsd) + (s.costPartial ? ' · 部分费用' : '')], ['运行中', count(view.runningSessions.length)], ['速度 tokens/s', count(s.tokensPerSecond)], ['输出 tokens/s', count(s.outputTokensPerSecond)], ['输入', count(s.inputTokens)], ['输出', count(s.outputTokens)], ['缓存读取', count(s.cacheReadTokens)], ['缓存写入', count(s.cacheWriteTokens)], ['推理', count(s.reasoningTokens)], ['未分类', count(s.unclassifiedTokens)], ['未定价', count(s.unpricedTokens)]];
  }
  function sourcePairs() {
    const h = view.history;
    return [['用量采集', absoluteTime(data.stats.updatedAt)], ['用量读取', absoluteTime(context.receivedAt)], ['历史', h.source === 'full' ? '完整历史' : h.source === 'preview' ? '近30日预览，空白日期不代表无活动' : '未提供'], ['历史读取', absoluteTime(context.historyReceivedAt) + (context.historyCached ? ' · 缓存' : '')], ['历史错误', context.historyError || '无'], ['会话范围', view.sessionsScope === 'local-archive' ? '本机累计留存，不代表跨设备完整历史' : 'Hub 已提供的会话'], ['会话完整性', view.sessionsUnavailable ? '上游未提供累计会话列表' : view.sessionsIncomplete ? '上游省略部分会话' : '未标记缺省'], ['项目完整性', view.projectsIncomplete ? '上游省略部分项目' : '未标记缺省'], ['归档采集', absoluteTime(view.archiveCapturedAt)], ['归档读取', absoluteTime(context.archiveReceivedAt) + (context.archiveCached ? ' · 缓存' : '')], ['归档错误', context.archiveError || (data.archive && !data.archive.available ? '本机累计归档不可用' : '无')], ['费用口径', '按已知模型价格估算 USD；未定价 tokens 不计入费用；— 表示未提供。额度余额保留原币种。'], ['运行状态', '按最新观测和最近活动推断；上下文百分比表示已用。归档会话仅匹配新鲜 Hub 观测时显示活动状态。']];
  }
  function historySummaryPairs() {
    const h = view.history.summary;
    return [['历史 tokens', count(h.totalTokens)], ['历史估算 USD', usd(h.totalCost)], ['活跃天数', count(h.activeDays)], ['单日峰值', count(h.peakDayTokens)], ['连续活跃', count(h.currentStreak)], ['最长连续', count(h.longestStreak)], ['消息', count(h.messages)], ['活跃分钟', finite(h.activeTimeMs) ? count(h.activeTimeMs / 60000) : '—'], ['常用模型', h.favoriteModel || '—']];
  }
  function render() {
    actions = [];
    const surface = el('div', undefined, 'dash-surface');
    const width = host.clientWidth, height = host.clientHeight;
    const font = height < 80 ? 8 : width < 280 || height < 150 ? 10 : width < 400 || height < 260 ? 12 : 14;
    surface.style.setProperty('--dash-font', font + 'px');
    surface.style.setProperty('--dash-row-height', (font * 2.7 + 4) + 'px');
    let rows = [], chart = false;
    if (!data) rows = [row('暂无用量快照', '—', '请用刷新按钮读取')];
    else {
      view = dashboardView(data.stats, data.history, period, Date.now(), data.archive);
      if (drill) {
        const controls = el('div', undefined, 'dash-controls');
        controls.appendChild(actionButton('返回', () => { drill = null; render(); })); controls.appendChild(el('strong', drill.title, 'dash-caption')); surface.appendChild(controls);
        // Segment by worst-case full-width glyphs, with no ellipsis in drill rows.
        const chars = Math.max(2, Math.floor((width - 18) / font));
        drill.pairs.forEach(([label, value]) => {
          const text = Array.from(String(value == null ? '—' : value).replace(/\r?\n/g, ' ↵ ').replace(/\t/g, ' '));
          const chunks = Math.max(1, Math.ceil(text.length / chars));
          for (let i = 0; i < chunks; i++) {
            const node = el('div', undefined, 'dash-row dash-reading');
            node.appendChild(el('span', label + (chunks > 1 ? ' ' + (i + 1) + '/' + chunks : ''), 'dash-secondary'));
            node.appendChild(el('span', text.slice(i * chars, (i + 1) * chars).join(''), 'dash-full-value')); rows.push(node);
          }
        });
      } else if (page === 'overview') {
        if (height >= 120) {
        const hero = actionButton(undefined, () => open(periods[period] + '用量', summaryPairs()), 'dash-hero');
        hero.appendChild(el('span', compact(view.summary.totalTokens), 'dash-total')); hero.appendChild(el('span', 'tokens' + (finite(view.summary.costUsd) ? ' · ' + listUsd(view.summary.costUsd) : '') + (view.summary.costPartial ? ' 部分' : '') + (finite(view.summary.costUsd) ? ' USD' : ''), 'dash-cost')); surface.appendChild(hero);
        const brief = el('div', undefined, 'dash-brief');
        brief.appendChild(el('span', '运行 ' + view.runningSessions.length + (finite(view.summary.tokensPerSecond) ? ' · ' + compact(view.summary.tokensPerSecond) + ' tok/s' : '')));
        const components = [['输入', view.summary.inputTokens], ['输出', view.summary.outputTokens]].filter(([, value]) => finite(value)).map(([label, value]) => label + ' ' + compact(value));
        if (components.length) brief.appendChild(el('span', components.join(' · '))); surface.appendChild(brief);
        }
        rows = quotaRows().concat(view.models.slice(0, 5).map(x => distributionRow(x)), view.recentSessions.map(x => sessionRow(x)));
        if (height < 120) rows.unshift(row(periods[period] + ' tokens', compact(view.summary.totalTokens), listUsd(view.summary.costUsd), () => open('汇总用量', summaryPairs())));
        if (!rows.length) rows.push(row('暂无明细', '—', '点击总量查看汇总'));
      } else if (page === 'limits') rows = quotaRows();
      else if (page === 'sessions') {
        tabs(surface, [['all', '全部 ' + view.sessions.length], ['running', '运行中']], runningOnly ? 'running' : 'all', id => { runningOnly = id === 'running'; });
        rows = (runningOnly ? view.sessions.filter(s => s.state === 'running') : view.sessions).map(x => sessionRow(x));
        if (view.sessionsScope === 'local-archive' || view.sessionsIncomplete || view.sessionsUnavailable || context.archiveError) surface.appendChild(actionButton(view.sessionsScope === 'local-archive' ? '本机累计 · 范围' : '数据范围', () => open('会话数据范围', sourcePairs()), 'dash-scope'));
      } else if (page === 'details') {
        tabs(surface, [['models', '模型'], ['clients', '工具'], ['projects', '项目'], ['devices', '设备'], ['source', '来源']], detailKind, id => { detailKind = id; });
        if (detailKind === 'source') rows = [row('来源与更新时间', '查看', '', () => open('数据来源', sourcePairs())), row('汇总用量', '查看', '', () => open('汇总用量', summaryPairs())), row('历史汇总', '查看', '', () => open('历史汇总', historySummaryPairs()))];
        else if (detailKind === 'devices') rows = view.devices.map(d => row(d.name, compact(d.totalTokens), [known(d.platform) ? d.platform : '', d.stale ? '过期' : '', listUsd(d.costUsd)].filter(Boolean).join(' · '), () => open(d.name, [['设备', d.name], ['平台', d.platform], ['状态', d.stale ? '已过期' : '来源未标记过期'], ['tokens', count(d.totalTokens)], ['估算 USD', usd(d.costUsd)], ['采集', absoluteTime(d.updatedAt)]])));
        else rows = view[detailKind].map(x => distributionRow(x));
      } else if (page === 'activity') {
        tabs(surface, [['heat', '热力'], ['trend', '趋势'], ['day', '每日'], ['month', '每月']], activityKind, id => { activityKind = id; });
        const hs = view.history.summary;
        if (view.history.available) surface.appendChild(actionButton([finite(hs.activeDays) ? '活跃 ' + count(hs.activeDays) + ' 天' : '', finite(hs.peakDayTokens) ? '峰值 ' + compact(hs.peakDayTokens) : ''].filter(Boolean).join(' · ') || '历史汇总', () => open('历史汇总', historySummaryPairs()), 'dash-activity-summary'));
        if (activityKind === 'day' || activityKind === 'month') rows = (activityKind === 'day' ? view.history.daily : view.history.monthly).slice().reverse().map(r => row(r.date || r.month, compact(r.tokens), listUsd(r.cost) + (r.unpricedTokens > 0 ? ' · 部分' : ''), () => open(r.date || r.month, historyPairs(r))));
        else chart = view.history.available;
        if (!view.history.available) rows = [row('暂无历史', '—', '来源页可查看读取状态', () => open('数据来源', sourcePairs()))];
      }
    }
    const body = el('div', undefined, 'dash-body'); surface.appendChild(body);
    // Attach briefly for actual available height; existing content is kept when
    // the rendered tree is identical, including event handlers via delegation.
    const previous = host.firstElementChild;
    if (previous) previous.remove();
    host.appendChild(surface);
    const available = Math.max(1, body.clientHeight);
    const rowHeight = font * 2.7 + 4;
    const columns = !drill && page === 'overview' && width >= 640 ? 2 : 1;
    const perColumn = Math.max(1, Math.floor(available / rowHeight));
    const size = columns * perColumn;
    if (chart) renderChart(body, width, available, font);
    else {
      if (!rows.length) rows.push(row('暂无记录', '—', period === 'allTime' ? '可切换本月或查看来源' : '当前时段没有数据'));
      pageCount = Math.max(1, Math.ceil(rows.length / size));
      currentIndex = Math.max(0, Math.min(positions[key()] || 0, pageCount - 1)); positions[key()] = currentIndex;
      body.style.gridTemplateColumns = 'repeat(' + columns + ',minmax(0,1fr))';
      body.style.gridTemplateRows = 'repeat(' + perColumn + ',minmax(0,1fr))';
      if (available < rowHeight) body.classList.add('dash-cramped');
      rows.slice(currentIndex * size, (currentIndex + 1) * size).forEach(r => body.appendChild(r));
    }
    if (previous && previous.isEqualNode(surface)) { surface.remove(); host.appendChild(previous); }
    updatePager();
  }
  function renderChart(body, width, height, font) {
    body.classList.add('dash-chart');
    const daily = view.history.daily;
    const last = new Date(dateKey(Date.now()) + 'T12:00:00');
    const first = activityKind === 'heat' ? new Date(last.getFullYear(), last.getMonth() - 11, 1, 12) : new Date(last);
    if (activityKind === 'trend') first.setDate(first.getDate() - 44);
    // Fixed windows preserve empty dates; the source detail explains preview gaps.
    const desiredCell = Math.max(8, Math.min(12, (height - 44) / 7));
    const weeks = Math.max(1, Math.floor((width - 24) / desiredCell));
    const size = activityKind === 'heat' ? Math.max(7, weeks * 7 - 6) : Math.max(2, Math.floor((width - 20) / 9));
    const days = Math.round((last - first) / 86400000) + 1;
    pageCount = Math.max(1, Math.ceil(days / size)); currentIndex = Math.min(positions[key()] || 0, pageCount - 1); positions[key()] = currentIndex;
    const end = new Date(last); end.setDate(end.getDate() - currentIndex * size);
    const start = new Date(end); start.setDate(start.getDate() - size + 1); if (start < first) start.setTime(first.getTime());
    const lookup = new Map(daily.map(r => [r.date, r])); const entries = [];
    for (let d = new Date(start); d <= end; d.setDate(d.getDate() + 1)) entries.push({ date: dateKey(d), row: lookup.get(dateKey(d)), day: d.getDay(), month: d.getMonth() + 1, monthStart: d.getDate() === 1 });
    const peak = daily.filter(r => r.date >= dateKey(first) && r.date <= dateKey(last)).reduce((best, r) => finite(r.tokens) && (!best || r.tokens > best.tokens) ? r : best, null);
    const captionLabel = (activityKind === 'heat' ? '12个月 · ' : '近45日 · ') + dateKey(start).slice(5) + '—' + dateKey(end).slice(5);
    const caption = actionButton(captionLabel, () => open(activityKind === 'heat' ? '12个月活动' : '45日趋势', [['范围', dateKey(first) + ' — ' + dateKey(last)], ['当前分段', dateKey(start) + ' — ' + dateKey(end)], ['峰值日期', peak ? peak.date : '未提供'], ['峰值 tokens', peak ? count(peak.tokens) : '未提供'], ['来源', view.history.source === 'full' ? '完整历史' : '预览；空白日期不代表无活动']]), 'dash-chart-caption'); body.appendChild(caption);
    const chartHeight = Math.max(1, height - caption.offsetHeight - 2), chartWidth = Math.max(1, width - 4);
    const svg = svgEl('svg', { viewBox: '0 0 ' + chartWidth + ' ' + chartHeight, role: 'img', 'aria-label': activityKind === 'heat' ? '最近十二个月的每日活动；每日按钮查看数值' : '最近四十五日 tokens 趋势；每日按钮查看数值' });
    svg.style.height = chartHeight + 'px';
    const max = Math.max(1, ...daily.filter(r => r.date >= dateKey(first) && r.date <= dateKey(last)).map(r => r.tokens || 0));
    if (activityKind === 'heat') {
      const labelHeight = Math.min(14, chartHeight / 4);
      const offset = start.getDay();
      const cell = Math.max(1, Math.min((chartHeight - labelHeight) / 7, (chartWidth - 18) / Math.ceil((offset + entries.length) / 7)));
      let previousMonth = -1;
      entries.forEach((e, i) => {
        const x = 18 + Math.floor((offset + i) / 7) * cell, y = labelHeight + e.day * cell;
        if (e.month !== previousMonth && ((i === 0 && new Date(e.date + 'T12:00:00').getDate() <= 15) || e.monthStart)) {
          const text = svgEl('text', { x, y: Math.max(1, labelHeight - 2), 'font-size': Math.max(1, Math.min(font * .8, labelHeight - 1)), fill: '#000' }); text.textContent = e.month + '月'; svg.appendChild(text); previousMonth = e.month;
        }
        const tokens = e.row && e.row.tokens || 0;
        svg.appendChild(svgEl('rect', { x, y, width: Math.max(1, cell - 2), height: Math.max(1, cell - 2), fill: tokens > 0 ? '#000' : '#fff', stroke: '#000', 'stroke-width': '.5' }));
        if (tokens > 0) svg.appendChild(svgEl('rect', { x: x + 1, y: y + 1, width: Math.max(0, cell - 4), height: Math.max(0, (cell - 4) * (1 - tokens / max)), fill: '#fff' }));
      });
      const usedHeight = labelHeight + cell * 7;
      if (chartHeight - usedHeight >= 72 && chartWidth >= 280) {
        const since = new Date(last); since.setDate(since.getDate() - 44);
        const recent = daily.filter(r => r.date >= dateKey(since) && r.date <= dateKey(last));
        const recentMax = Math.max(1, ...recent.map(r => r.tokens || 0));
        const label = svgEl('text', { x: 2, y: usedHeight + 14, 'font-size': font * .85, fill: '#000' });
        label.textContent = '近45日 · 峰值 ' + (recent.length ? compact(recentMax) : '—'); svg.appendChild(label);
        let points = [];
        const flush = () => { if (points.length) svg.appendChild(svgEl('polyline', { points: points.join(' '), fill: 'none', stroke: '#000', 'stroke-width': 1.5 })); points = []; };
        for (let i = 0; i < 45; i++) {
          const d = new Date(since); d.setDate(d.getDate() + i); const item = lookup.get(dateKey(d));
          if (!item || !finite(item.tokens)) { flush(); continue; }
          const x = 3 + i * (chartWidth - 6) / 44;
          const y = usedHeight + 23 + (chartHeight - usedHeight - 28) * (1 - item.tokens / recentMax);
          points.push(x + ',' + y); svg.appendChild(svgEl('circle', { cx: x, cy: y, r: 1.5, fill: '#000' }));
        }
        flush();
      }
    } else {
      const labelHeight = Math.min(14, chartHeight / 3);
      const peakLabel = svgEl('text', { x: 2, y: Math.max(1, labelHeight - 2), 'font-size': Math.max(1, Math.min(font * .8, labelHeight - 1)), fill: '#000' }); peakLabel.textContent = '45日峰值 ' + (peak ? compact(peak.tokens) + ' · ' + peak.date.slice(5) : '—'); svg.appendChild(peakLabel);
      let points = [];
      const flush = () => { if (points.length) svg.appendChild(svgEl('polyline', { points: points.join(' '), fill: 'none', stroke: '#000', 'stroke-width': 1.5 })); points = []; };
      entries.forEach((e, i) => { if (!e.row || !finite(e.row.tokens)) { flush(); return; } const x = 3 + i * (chartWidth - 6) / Math.max(1, entries.length - 1), y = labelHeight + 2 + Math.max(0, chartHeight - labelHeight - 5) * (1 - e.row.tokens / max); points.push(x + ',' + y); svg.appendChild(svgEl('circle', { cx: x, cy: y, r: 1.5, fill: '#000' })); }); flush();
    }
    body.appendChild(svg);
  }

  function updatePager() {
    const prev = document.getElementById('page-prev'), next = document.getElementById('page-next'), position = document.getElementById('page-position');
    if (prev) prev.disabled = currentIndex === 0;
    if (next) next.disabled = currentIndex + 1 >= pageCount;
    if (position) position.textContent = (currentIndex + 1) + ' / ' + pageCount;
  }
  function turn(delta) { positions[key()] = Math.max(0, Math.min(pageCount - 1, currentIndex + delta)); render(); }
  host.addEventListener('click', event => { const b = event.target.closest('[data-dash-action]'); if (b && host.contains(b) && actions[Number(b.dataset.dashAction)]) actions[Number(b.dataset.dashAction)](); });
  document.querySelectorAll('#navigation [data-page]').forEach(b => b.addEventListener('click', () => {
    page = b.dataset.page; drill = null;
    document.querySelectorAll('#navigation [data-page]').forEach(n => { if (n.dataset.page === page) n.setAttribute('aria-current', 'page'); else n.removeAttribute('aria-current'); }); render();
  }));
  document.getElementById('period').addEventListener('change', event => { period = event.target.value; drill = null; render(); });
  const prev = document.getElementById('page-prev'), next = document.getElementById('page-next');
  if (prev) prev.addEventListener('click', () => turn(-1)); if (next) next.addEventListener('click', () => turn(1));
  const resize = () => { if (resizeFrame) cancelAnimationFrame(resizeFrame); resizeFrame = requestAnimationFrame(() => { resizeFrame = 0; render(); }); };
  if (typeof ResizeObserver !== 'undefined') new ResizeObserver(resize).observe(host);
  else window.addEventListener('resize', resize);
  return {
    update(stats, history, nextContext, archive = null) { data = stats ? { stats, history, archive } : null; context = nextContext || {}; render(); },
    clear() { data = null; drill = null; render(); }
  };
}
