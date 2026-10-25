import { normalizeStats, absoluteTime } from './model.mjs';
import { dashboardView } from './dashboard-model.mjs';
import { heatLevel } from './theme.mjs';
import { trendGeometry } from './charts.mjs';

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
// Each page is measured before pagination; long fields remain available in drill-downs.
export function createDashboard(host) {
  let data = null, context = {}, view = null;
  let page = 'limits', period = document.getElementById('period').value || 'today';
  let detailKind = 'usage', runningOnly = false, activityKind = 'trend';
  let drill = null, actions = [], pageCount = 1, currentIndex = 0;
  const positions = {};
  let resizeFrame = 0;
  const shade = index => 'var(--heat-' + Math.max(3, 15 - index * 3) + ')';
  function key() { return drill ? 'drill' : page + ':' + period + ':' + (page === 'details' ? detailKind : page === 'activity' ? activityKind : page === 'sessions' ? runningOnly : ''); }
  function actionButton(label, fn, className = 'dash-tab') {
    const b = el('button', label, className); b.type = 'button';
    b.dataset.dashAction = String(actions.push(fn) - 1); return b;
  }
  function open(title, pairs) { drill = { title, pairs }; positions.drill = 0; render(); }
  function row(label, value, secondary, fn, className = '') {
    const node = actionButton(undefined, fn || (() => open(label, [['名称', label], ['数值', value], ['说明', secondary || '—']])), 'dash-row ' + className);
    const top = el('span', undefined, 'dash-line');
    top.append(el('span', label, 'dash-name'), el('strong', value, 'dash-value')); node.append(top);
    if (secondary) node.append(el('span', secondary, 'dash-secondary'));
    return node;
  }
  function tabs(root, choices, selected, change) {
    const controls = el('div', undefined, 'dash-controls');
    choices.forEach(([id, label]) => {
      const b = actionButton(label, () => { change(id); render(); });
      b.setAttribute('aria-pressed', String(id === selected)); controls.append(b);
    }); root.append(controls);
  }
  function tone(value) { return !finite(value) ? 'normal' : value <= 10 ? 'danger' : value <= 25 ? 'warn' : 'ok'; }
  function ring(value) {
    const svg = svgEl('svg', { viewBox: '0 0 72 72', class: 'dash-ring', 'aria-hidden': 'true' });
    svg.append(svgEl('circle', { cx: 36, cy: 36, r: 28, fill: 'none', stroke: 'var(--soft-line)', 'stroke-width': 6 }));
    if (finite(value)) svg.append(svgEl('circle', { cx: 36, cy: 36, r: 28, fill: 'none', stroke: 'currentColor', 'stroke-width': 6, pathLength: 100, 'stroke-dasharray': Math.max(0, Math.min(100, value)) + ' 100', transform: 'rotate(-90 36 36)' }));
    return svg;
  }
  function providerFields(p, plan) {
    return [['供应商', p.name], ['套餐', plan || '未提供'], ['账号', p.account || '未提供'], ['状态', p.status + (p.stale ? ' · 已过期' : '')], ['来源', p.source || '未提供'], ['采集', absoluteTime(p.updatedAt)]];
  }
  function quotaItems(cards) {
    const items = [];
    normalizeStats(data.stats).forEach(p => {
      const raw = data.stats.limits.providers[p.key] || {};
      const plan = raw.plan || raw.planType || raw.subscriptionPlan || '';
      const fields = providerFields(p, plan), windows = p.windows.length ? p.windows : [null];
      const title = [p.name, plan, p.account].filter(Boolean).join(' · ');
      if (!cards) {
        windows.forEach(w => items.push(row(p.name + (w ? ' · ' + w.label : ''), w ? count(w.value) + w.unit : p.status, [p.account, p.stale ? '过期' : !p.ok ? p.status : '', w ? resetIn(w.resetsAt) : ''].filter(Boolean).join(' · '), () => open(title, w ? fields.concat([['窗口', w.label], [w.valueLabel, count(w.value) + w.unit], ['重置', absoluteTime(w.resetsAt)]]) : fields))));
        return;
      }
      for (let i = 0; i < windows.length; i += 2) {
        const card = el('section', undefined, 'dash-card dash-quota-card');
        const heading = actionButton(undefined, () => open(title, fields), 'dash-card-heading');
        heading.append(el('strong', p.name, 'dash-name'), el('span', [plan || p.account, p.stale ? '过期' : !p.ok ? p.status : '', windows.length > 2 ? (i / 2 + 1) + '/' + Math.ceil(windows.length / 2) : ''].filter(Boolean).join(' · '), 'dash-badge'));
        card.append(heading);
        const group = el('div', undefined, 'dash-quota-windows');
        windows.slice(i, i + 2).forEach(w => {
          if (!w) { group.append(actionButton(p.status, () => open(title, fields), 'dash-quota-empty')); return; }
          const percent = w.unit === '%' && w.valueLabel === '剩余额度' ? w.value : null;
          const node = actionButton(undefined, () => open(title, fields.concat([['窗口', w.label], [w.valueLabel, count(w.value) + w.unit], ['重置', absoluteTime(w.resetsAt)]])), 'dash-quota-window');
          node.dataset.tone = tone(percent);
          if (w.unit === '%') {
            const gauge = el('span', undefined, 'dash-gauge'); gauge.append(ring(percent));
            gauge.append(el('strong', count(w.value) + w.unit, 'dash-gauge-number')); node.append(gauge);
            node.append(el('span', w.label.replace(/(额度|窗口)$/g, '') || w.label, 'dash-window-label'));
          } else {
            node.classList.add('dash-balance');
            const symbol = { CNY: '¥', USD: '$', EUR: '€' }[w.unit];
            node.append(el('span', w.valueLabel || w.label, 'dash-window-label'), el('strong', symbol ? symbol + count(w.value) : count(w.value) + (w.unit ? ' ' + w.unit : ''), 'dash-balance-number'));
          }
          const note = resetIn(w.resetsAt) || (w.unit === '%' ? w.valueLabel === '剩余额度' ? '剩余可用' : w.valueLabel : '');
          if (note) node.append(el('span', note, 'dash-window-note'));
          group.append(node);
        });
        card.append(group);
        if (p.account && plan && p.account !== plan) card.append(el('div', p.account, 'dash-account'));
        items.push(card);
      }
    }); return items;
  }
  function distributionItem(item, kind, cards, index) {
    const fields = [['名称', item.name], ['tokens', count(item.tokens)], ['占比', percentage(item.share)], ['估算 USD', usd(item.costUsd)]];
    if (!cards) return row(item.name, compact(item.tokens), percentage(item.share), () => open(item.name, fields));
    const card = actionButton(undefined, () => open(item.name, fields), 'dash-card dash-distribution-card');
    card.append(el('span', item.name, 'dash-name'), el('strong', compact(item.tokens), 'dash-metric-number'));
    const foot = el('span', undefined, 'dash-card-foot');
    foot.append(el('span', percentage(item.share), 'dash-rank-share'), el('span', listUsd(item.costUsd), 'dash-secondary'));
    card.append(foot); card.style.setProperty('--item-shade', shade(index % 5)); card.dataset.kind = kind;
    return card;
  }
  function sessionItem(item, cards) {
    const title = known(item.title) ? item.title : known(item.project) ? item.project : '会话';
    const status = item.archived ? '已归档' : states[item.state];
    if (!cards) return row(title, compact(item.totalTokens), [item.model, status].filter(Boolean).join(' · '), () => open('会话用量', sessionPairs(item)));
    const card = actionButton(undefined, () => open('会话用量', sessionPairs(item)), 'dash-card dash-session-card');
    const heading = el('span', undefined, 'dash-line');
    heading.append(el('strong', title, 'dash-name'), el('span', status, 'dash-status')); card.append(heading);
    card.dataset.state = item.state;
    card.append(el('span', [known(item.project) && item.project !== title ? item.project : '', known(item.model) ? item.model : ''].filter(Boolean).join(' · '), 'dash-secondary'));
    const values = el('span', undefined, 'dash-card-foot');
    values.append(el('strong', compact(item.totalTokens), 'dash-session-number'), el('span', listUsd(item.costUsd), 'dash-secondary')); card.append(values);
    const contextRow = el('span', undefined, 'dash-context');
    if (finite(item.contextPercent)) {
      const ticks = el('span', undefined, 'dash-ticks'); ticks.setAttribute('aria-hidden', 'true');
      for (let i = 0; i < 12; i++) { const tick = el('span'); if (item.contextPercent > i * 100 / 12) tick.className = 'filled'; ticks.append(tick); }
      contextRow.append(ticks, el('span', '上下文 ' + count(item.contextPercent) + '%', 'dash-context-number'));
    } else contextRow.append(el('span', absoluteTime(item.lastUsedAt), 'dash-secondary'));
    card.append(contextRow);
    return card;
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
  function metric(label, value, note, fn, cards) {
    if (!cards) return row(label, value, note, fn);
    const card = actionButton(undefined, fn, 'dash-card dash-metric-card');
    card.append(el('span', label, 'dash-secondary'), el('strong', value, 'dash-metric-number'));
    if (note) card.append(el('span', note, 'dash-secondary')); return card;
  }
  function distributionHeader(surface, items, height) {
    const positive = items.filter(x => finite(x.tokens) && x.tokens > 0);
    const recorded = positive.reduce((sum, item) => sum + item.tokens, 0);
    const total = Math.max(recorded, finite(view.summary.totalTokens) ? view.summary.totalTokens : 0);
    if (!total) return;
    if (total > recorded) positive.push({ name: '未归属', tokens: total - recorded });
    const button = actionButton(undefined, () => open('分布数值', positive.map(x => [x.name, count(x.tokens) + ' tokens · ' + percentage(x.tokens / total)])), 'dash-distribution-header');
    button.style.height = Math.min(116, height * .32) + 'px';
    const svg = svgEl('svg', {viewBox:'0 0 100 100', 'aria-hidden':'true'});
    const grouped = positive.slice(0, 4);
    if (positive.length > 4) grouped.push({ name: '其余', tokens: positive.slice(4).reduce((sum, x) => sum + x.tokens, 0) });
    let offset = 0;
    grouped.forEach((item, i) => {
      const share = item.tokens / total * 100;
      svg.append(svgEl('circle', {cx:50,cy:50,r:36,fill:'none',stroke:shade(i),'stroke-width':15,pathLength:100,'stroke-dasharray':share+' '+(100-share),'stroke-dashoffset':-offset,transform:'rotate(-90 50 50)'})); offset += share;
    });
    const label = svgEl('text', {x:50,y:54,'text-anchor':'middle',fill:'var(--ink)','font-size':14,'font-weight':700}); label.textContent=compact(total);svg.append(label);
    button.append(svg);
    const legend = el('span',undefined,'dash-legend');
    grouped.slice(0,4).forEach((item,i)=>{const line=el('span',undefined,'dash-legend-line');const dot=el('i');dot.style.background=shade(i);line.append(dot,el('span',item.name,'dash-name'),el('strong',percentage(item.tokens/total)));legend.append(line);});
    button.append(legend);surface.append(button);
  }
  function render() {
    actions = [];
    const surface = el('div', undefined, 'dash-surface');
    const width = host.clientWidth, height = host.clientHeight;
    const font = height < 80 ? 8 : width < 280 || height < 150 ? 10 : width < 400 || height < 260 ? 12 : 14;
    const cards = width >= 220 && height >= 140;
    surface.style.setProperty('--dash-font', font + 'px');
    surface.style.setProperty('--dash-row-height', (font * 2.7 + 4) + 'px');
    let rows = [], chart = false, layout = 'rows';
    if (!data) rows = [row('暂无用量快照', '—', '请用刷新按钮读取')];
    else {
      view = dashboardView(data.stats, data.history, period, Date.now(), data.archive);
      if (drill) {
        const controls = el('div', undefined, 'dash-controls');
        controls.append(actionButton('返回', () => { drill = null; render(); }), el('strong', drill.title, 'dash-caption')); surface.append(controls);
        const chars = Math.max(2, Math.floor((width - 18) / font));
        drill.pairs.forEach(([label, value]) => {
          const text = Array.from(String(value == null ? '—' : value).replace(/\r?\n/g, ' ↵ ').replace(/\t/g, ' '));
          const chunks = Math.max(1, Math.ceil(text.length / chars));
          for (let i = 0; i < chunks; i++) {
            const node = el('div', undefined, 'dash-row dash-reading');
            node.append(el('span', label + (chunks > 1 ? ' ' + (i + 1) + '/' + chunks : ''), 'dash-secondary'), el('span', text.slice(i * chars, (i + 1) * chars).join(''), 'dash-full-value')); rows.push(node);
          }
        });
      } else if (page === 'limits') { rows = quotaItems(cards); layout = cards ? 'quota' : 'rows'; }
      else if (page === 'sessions') {
        tabs(surface, [['all', '全部 ' + view.sessions.length], ['running', '运行中 ' + view.sessions.filter(s => s.state === 'running').length]], runningOnly ? 'running' : 'all', id => { runningOnly = id === 'running'; });
        rows = (runningOnly ? view.sessions.filter(s => s.state === 'running') : view.sessions).map(item => sessionItem(item, cards)); layout = cards ? 'session' : 'rows';
        if (view.sessionsScope === 'local-archive' || view.sessionsIncomplete || view.sessionsUnavailable || context.archiveError) surface.append(actionButton(view.sessionsScope === 'local-archive' ? '本机累计 · 数据范围' : '数据范围', () => open('会话数据范围', sourcePairs()), 'dash-scope'));
      } else if (page === 'details') {
        tabs(surface, [['usage', '用量'], ['models', '模型'], ['clients', '工具'], ['projects', '项目'], ['devices', '设备'], ['source', '来源']], detailKind, id => { detailKind = id; });
        layout = cards ? 'metric' : 'rows';
        if (detailKind === 'usage') rows = summaryPairs().slice(1).filter(([,value],i)=>i<2||value!=='—').map(([label, value],i) => metric(label,label==='总 tokens'?compact(view.summary.totalTokens):label==='估算 USD'?(finite(view.summary.costUsd)?listUsd(view.summary.costUsd):'—')+(view.summary.costPartial?' · 部分':'' ):value,i===0?periods[period]:'',()=>open('用量明细',summaryPairs()),cards));
        else if (detailKind === 'source') rows = [metric('数据来源','更新时间','',()=>open('数据来源',sourcePairs()),cards),metric('历史汇总',compact(view.history.summary.totalTokens),'tokens',()=>open('历史汇总',historySummaryPairs()),cards)];
        else if (detailKind === 'devices') rows = view.devices.map(d => metric(d.name,compact(d.totalTokens),[known(d.platform)?d.platform:'',d.stale?'过期':'',listUsd(d.costUsd)].filter(Boolean).join(' · '),()=>open(d.name,[['设备',d.name],['平台',d.platform],['状态',d.stale?'已过期':'来源未标记过期'],['tokens',count(d.totalTokens)],['估算 USD',usd(d.costUsd)],['采集',absoluteTime(d.updatedAt)]]),cards));
        else {
          const items = view[detailKind];
          if (cards && height >= 240 && width >= 280) distributionHeader(surface,items,height);
          rows = items.map((item,index)=>distributionItem(item,detailKind,cards,index));
        }
      } else if (page === 'activity') {
        tabs(surface,[['heat','热力'],['trend','趋势'],['day','每日'],['month','每月']],activityKind,id=>{activityKind=id;});
        const hs=view.history.summary;
        if (view.history.available && height >= 170) {
          const summary=el('div',undefined,'dash-history-stats');
          [['活跃天数',count(hs.activeDays)],['单日峰值',compact(hs.peakDayTokens)],['连续活跃',count(hs.currentStreak)]].forEach(([label,value])=>summary.append(metric(label,value,'',()=>open('历史汇总',historySummaryPairs()),true)));surface.append(summary);
        }
        if (activityKind === 'day' || activityKind === 'month') rows=(activityKind==='day'?view.history.daily:view.history.monthly).slice().reverse().map(r=>row(r.date||r.month,compact(r.tokens),listUsd(r.cost)+(r.unpricedTokens>0?' · 部分':''),()=>open(r.date||r.month,historyPairs(r))));
        else chart=view.history.available;
        if (!view.history.available) rows=[row('暂无历史','—','查看数据来源',()=>open('数据来源',sourcePairs()))];
      }
    }
    const body=el('div',undefined,'dash-body'); surface.append(body);
    const previous=host.firstElementChild;if(previous)previous.remove();host.append(surface);
    const available=Math.max(1,body.clientHeight);
    const columns=layout==='rows'?1:width>=530?2:layout==='metric'&&width>=300?2:1;
    const rowHeight=font*2.7+4;
    const minimum=layout==='quota'?font*7.7+14:layout==='session'?font*5.2+8:layout==='metric'?font*4.6+12:rowHeight;
    const perColumn=Math.max(1,Math.floor((available+6)/(minimum+6))), size=columns*perColumn;
    if(chart)renderChart(body,width,available,font);
    else {
      if(!rows.length)rows.push(row('暂无记录','—',period==='allTime'?'可切换本月或查看来源':'当前时段没有数据'));
      pageCount=Math.max(1,Math.ceil(rows.length/size));currentIndex=Math.max(0,Math.min(positions[key()]||0,pageCount-1));positions[key()]=currentIndex;
      body.dataset.layout=layout;body.style.gridTemplateColumns='repeat('+columns+',minmax(0,1fr))';body.style.gridTemplateRows='repeat('+perColumn+',minmax(0,1fr))';
      if(available<rowHeight){body.classList.add('dash-cramped');body.style.fontSize=Math.max(1,Math.min(font,available/1.4))+'px';}
      rows.slice(currentIndex*size,(currentIndex+1)*size).forEach(item=>body.append(item));
    }
    if(previous&&previous.isEqualNode(surface)){surface.remove();host.append(previous);}updatePager();
  }
  function drawTrend(svg, daily, start, end, width, height, offset = 0, font = 12) {
    const group=svgEl('g',{transform:'translate(0 '+offset+')'});svg.append(group);
    const geometry=trendGeometry(daily,{start,end,width,height,inset:{left:6,right:6,top:Math.min(24,height*.2),bottom:Math.min(22,height*.2)}});
    const top=Math.min(24,height*.2), bottom=height-Math.min(22,height*.2);
    if(height>=60)for(let i=0;i<3;i++){const y=top+(bottom-top)*i/2;group.append(svgEl('line',{x1:6,x2:width-6,y1:y,y2:y,stroke:'var(--soft-line)','stroke-width':.6}));}
    geometry.segments.forEach((segment,i)=>{
      if (!segment.gap) group.append(svgEl('path',{d:segment.d+'L'+geometry.points[i+1].x+','+bottom+'L'+geometry.points[i].x+','+bottom+'Z',fill:'var(--heat-1)',stroke:'none'}));
    });
    if(geometry.line)group.append(svgEl('path',{d:geometry.line,class:'dash-trend-segment',fill:'none',stroke:'var(--chart-line)','stroke-width':2.4,'stroke-linecap':'round','stroke-linejoin':'round'}));
    geometry.points.forEach(point=>group.append(svgEl('circle',{cx:point.x,cy:point.y,r:geometry.points.length>1?1.7:3,fill:'var(--paper)',stroke:'var(--chart-line)','stroke-width':1.2})));
    if(height>=60){
      const label=(text,x,y,anchor='start')=>{const n=svgEl('text',{x,y,'text-anchor':anchor,'font-size':Math.max(7,font*.8),fill:'var(--muted)'});n.textContent=text;group.append(n);};
      label(geometry.points.length?'峰值 '+compact(geometry.max):'无已记录数据',6,Math.max(9,top-6));
      label(start.slice(5),6,height-3);label(end.slice(5),width-6,height-3,'end');
    }
    return geometry;
  }
  function renderChart(body,width,height,font) {
    body.classList.add('dash-chart');
    const daily=view.history.daily,last=new Date(dateKey(Date.now())+'T12:00:00');
    const first=activityKind==='heat'?new Date(last.getFullYear(),last.getMonth()-11,1,12):new Date(last);
    if(activityKind==='trend')first.setDate(first.getDate()-44);
    const desiredCell=Math.max(8,Math.min(12,(height-44)/7)),weeks=Math.max(1,Math.floor((width-24)/desiredCell));
    const size=activityKind==='heat'?Math.max(7,weeks*7-6):Math.max(7,Math.floor((width-20)/7));
    const days=Math.round((last-first)/86400000)+1;
    pageCount=Math.max(1,Math.ceil(days/size));currentIndex=Math.min(positions[key()]||0,pageCount-1);positions[key()]=currentIndex;
    const end=new Date(last);end.setDate(end.getDate()-currentIndex*size);
    const start=new Date(end);start.setDate(start.getDate()-size+1);if(start<first)start.setTime(first.getTime());
    const lookup=new Map(daily.map(r=>[r.date,r])),entries=[];
    for(let d=new Date(start);d<=end;d.setDate(d.getDate()+1))entries.push({date:dateKey(d),row:lookup.get(dateKey(d)),day:d.getDay(),month:d.getMonth()+1,monthStart:d.getDate()===1});
    const caption=actionButton((activityKind==='heat'?'12个月 · ':'平滑趋势 · ')+dateKey(start).slice(5)+'—'+dateKey(end).slice(5),()=>open('活动范围',[['范围',dateKey(first)+' — '+dateKey(last)],['当前分段',dateKey(start)+' — '+dateKey(end)],['来源',view.history.source==='full'?'完整历史':'预览；空白日期不代表无活动'],['曲线','实线按真实日期间距平滑连接已有记录；无记录日期没有补录数值，也不能据此确认采集失败或零用量。首尾无记录区域留白。每日／每月按钮可查看原始值。']]),'dash-chart-caption');body.append(caption);
    const chartHeight=Math.max(1,height-caption.offsetHeight-2),chartWidth=Math.max(1,width-4);
    const svg=svgEl('svg',{viewBox:'0 0 '+chartWidth+' '+chartHeight,role:'img','aria-label':activityKind==='heat'?'每日活动热力图与近期平滑趋势':'按真实日期间距绘制的平滑实线用量曲线'});svg.style.height=chartHeight+'px';
    if(activityKind==='trend')drawTrend(svg,daily,dateKey(start),dateKey(end),chartWidth,chartHeight,0,font);
    else {
      const max=Math.max(1,...daily.filter(r=>r.date>=dateKey(first)&&r.date<=dateKey(last)).map(r=>r.tokens||0));
      const labelHeight=Math.min(14,chartHeight/4),offset=start.getDay();
      const cell=Math.max(1,Math.min((chartHeight-labelHeight)/7,(chartWidth-18)/Math.ceil((offset+entries.length)/7)));
      let previousMonth=-1;
      entries.forEach((e,i)=>{
        const x=18+Math.floor((offset+i)/7)*cell,y=labelHeight+e.day*cell;
        if(e.month!==previousMonth&&((i===0&&new Date(e.date+'T12:00:00').getDate()<=15)||e.monthStart)){
          const label=svgEl('text',{x,y:Math.max(1,labelHeight-2),'font-size':Math.max(1,Math.min(font*.8,labelHeight-1)),fill:'var(--muted)'});label.textContent=e.month+'月';svg.append(label);previousMonth=e.month;
        }
        const observed=e.row&&finite(e.row.tokens);
        svg.append(svgEl('rect',{x,y,width:Math.max(1,cell-2),height:Math.max(1,cell-2),rx:Math.max(0,Math.min(1,(cell-2)/6)),fill:observed?'var(--heat-'+heatLevel(e.row.tokens,max)+')':'var(--paper)',stroke:'var(--soft-line)','stroke-width':observed ? .5 : .3}));
      });
      const usedHeight=labelHeight+cell*7;
      if(chartHeight-usedHeight>=80&&chartWidth>=280){const since=new Date(last);since.setDate(since.getDate()-44);drawTrend(svg,daily,dateKey(since),dateKey(last),chartWidth,chartHeight-usedHeight-12,usedHeight+12,font);}
    }
    body.append(svg);
  }
  function updatePager(){
    const prev=document.getElementById('page-prev'),next=document.getElementById('page-next'),position=document.getElementById('page-position');
    if(prev)prev.disabled=currentIndex===0;if(next)next.disabled=currentIndex+1>=pageCount;if(position)position.textContent=(currentIndex+1)+' / '+pageCount;
  }
  function turn(delta){positions[key()]=Math.max(0,Math.min(pageCount-1,currentIndex+delta));render();}
  host.addEventListener('click',event=>{const button=event.target.closest('[data-dash-action]');if(button&&host.contains(button)&&actions[Number(button.dataset.dashAction)])actions[Number(button.dataset.dashAction)]();});
  document.querySelectorAll('#navigation [data-page]').forEach(button=>button.addEventListener('click',()=>{
    page=button.dataset.page;drill=null;document.querySelectorAll('#navigation [data-page]').forEach(n=>{if(n.dataset.page===page)n.setAttribute('aria-current','page');else n.removeAttribute('aria-current');});render();
  }));
  document.getElementById('period').addEventListener('change',event=>{period=event.target.value;drill=null;render();});
  const prev=document.getElementById('page-prev'),next=document.getElementById('page-next');if(prev)prev.addEventListener('click',()=>turn(-1));if(next)next.addEventListener('click',()=>turn(1));
  const resize=()=>{if(resizeFrame)cancelAnimationFrame(resizeFrame);resizeFrame=requestAnimationFrame(()=>{resizeFrame=0;render();});};
  if(typeof ResizeObserver!=='undefined')new ResizeObserver(resize).observe(host);else window.addEventListener('resize',resize);
  return {update(stats,history,nextContext,archive=null){data=stats?{stats,history,archive}:null;context=nextContext||{};render();},clear(){data=null;drill=null;render();}};
}
