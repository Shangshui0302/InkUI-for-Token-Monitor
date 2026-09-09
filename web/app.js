const POLL_INTERVAL_MS = 15_000;
const REQUEST_TIMEOUT_MS = 8_000;
const DAILY_PERIOD_DAYS = 14;
const PAGE_SIZE = 4;
const ACCOUNT_PAGE_SIZE = 2;

const refreshButton = document.querySelector("#refresh");
const connectionStatus = document.querySelector("#connection-status");
const lastUpdated = document.querySelector("#last-updated");
const revision = document.querySelector("#revision");
const accountCount = document.querySelector("#account-count");
const hostCount = document.querySelector("#host-count");
const navButtons = Array.from(document.querySelectorAll("[data-view]"));
const views = Array.from(document.querySelectorAll(".view"));
const overviewRoot = document.querySelector("#overview");
const accountsRoot = document.querySelector("#accounts");
const activityRoot = document.querySelector("#activity");
const hostsRoot = document.querySelector("#hosts");

const HOST_STATUS_LABELS = {
  connected: "在线",
  stale: "数据陈旧",
  offline: "离线",
  unknown: "未知",
};

const COLLECTOR_STATUS_LABELS = {
  ready: "采集正常",
  degraded: "部分可用",
  auth_required: "需要登录",
  unavailable: "采集不可用",
};

const SESSION_STATUS_LABELS = {
  active: "运行中",
  idle: "空闲",
  waiting: "等待中",
  error: "错误",
  unknown: "未知",
};

let loading = false;
let nextRefresh = 0;
let currentView = "overview";
let accountPage = 0;
let hostPage = 0;
let sessionPage = 0;
let activityPage = 0;
let latestState = null;

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined && text !== null) node.textContent = text;
  return node;
}

function statusLabel(labels, value) {
  return labels[value] || labels.unknown || value || "未知";
}

function statusClass(value) {
  return [
    "connected",
    "stale",
    "offline",
    "unknown",
    "ready",
    "degraded",
    "auth_required",
    "unavailable",
    "active",
    "idle",
    "waiting",
    "error",
  ].includes(value)
    ? value
    : "unknown";
}

function statusPill(value, labels) {
  return element("span", `status status-${statusClass(value)}`, statusLabel(labels, value));
}

function formatTime(seconds) {
  if (!Number.isFinite(Number(seconds)) || Number(seconds) <= 0) return "—";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(Number(seconds) * 1000));
}

function formatNumber(value) {
  if (!Number.isFinite(Number(value))) return "—";
  return new Intl.NumberFormat("zh-CN").format(Number(value));
}

function formatCompact(value) {
  if (!Number.isFinite(Number(value))) return "—";
  const number = Number(value);
  if (number >= 1_000_000_000) return `${(number / 1_000_000_000).toFixed(1)}B`;
  if (number >= 1_000_000) return `${(number / 1_000_000).toFixed(1)}M`;
  if (number >= 1_000) return `${(number / 1_000).toFixed(1)}K`;
  return String(number);
}

function formatPercent(value) {
  if (!Number.isFinite(Number(value))) return "—";
  return `${Math.round(Math.max(0, Math.min(100, Number(value))))}%`;
}

function resetCountdown(seconds) {
  if (!Number.isFinite(Number(seconds))) return "未知";
  const remaining = Number(seconds) - Math.floor(Date.now() / 1000);
  if (remaining <= 0) return "即将重置";
  const minutes = Math.ceil(remaining / 60);
  if (minutes >= 24 * 60) return `${Math.floor(minutes / (24 * 60))} 天后`;
  if (minutes >= 60) return `${Math.floor(minutes / 60)} 小时后`;
  return `${minutes} 分钟后`;
}

function setConnection(kind, title, detail) {
  connectionStatus.textContent = title;
  connectionStatus.className = `connection-title connection-${statusClass(kind)}`;
  lastUpdated.textContent = detail;
}

function renderWindow(parent, label, window) {
  if (!window) return;
  const block = element("div", "limit-window");
  const heading = element("div", "limit-heading");
  heading.append(element("strong", "limit-name", label));
  heading.append(element("span", "limit-value", formatPercent(window.usedPercent)));
  block.append(heading);

  const meter = element("div", "meter");
  const fill = element("span", "meter-fill");
  const percent = Number(window.usedPercent);
  fill.style.width = `${Number.isFinite(percent) ? Math.max(0, Math.min(100, percent)) : 0}%`;
  meter.append(fill);
  meter.setAttribute("role", "progressbar");
  meter.setAttribute("aria-valuemin", "0");
  meter.setAttribute("aria-valuemax", "100");
  if (Number.isFinite(percent)) meter.setAttribute("aria-valuenow", String(Math.round(percent)));
  block.append(meter);

  if (Number.isFinite(Number(window.resetsAt))) {
    block.append(
      element("p", "limit-details", `重置 ${formatTime(window.resetsAt)}（${resetCountdown(window.resetsAt)}）`),
    );
  }
  parent.append(block);
}

function renderAccount(view, compact = false) {
  const account = view.account || {};
  const card = element("article", `card account-card${compact ? " compact-card" : ""}`);
  const heading = element("div", "card-heading");
  const title = element("div");
  title.append(element("h3", "card-title", account.planType || "计划未知"));
  title.append(
    element("p", "meta", account.authMode ? `登录方式：${account.authMode}` : "登录方式未知"),
  );
  heading.append(title);
  heading.append(element("span", "source-count", `${(view.sourceHosts || []).length} 台主机`));
  card.append(heading);

  if (!compact && Array.isArray(view.sourceHosts) && view.sourceHosts.length) {
    card.append(element("p", "meta", `来源：${view.sourceHosts.join("、")}`));
  }

  const limits = Array.isArray(account.rateLimits) ? account.rateLimits : [];
  if (limits.length) {
    const limitsRoot = element("div", "limits");
    limits.forEach((bucket) => {
      const bucketRoot = element("div", "limit-bucket");
      bucketRoot.append(element("h4", "bucket-title", bucket.limitName || bucket.limitId || "额度"));
      renderWindow(bucketRoot, "五小时限制", bucket.primary);
      renderWindow(bucketRoot, "一周限制", bucket.secondary);
      if (!compact && bucket.credits) {
        const creditParts = [];
        if (bucket.credits.balance !== undefined && bucket.credits.balance !== null) {
          creditParts.push(`余额 ${bucket.credits.balance}`);
        }
        if (bucket.credits.unlimited === true) creditParts.push("无限");
        if (creditParts.length) bucketRoot.append(element("p", "credit", `Credits：${creditParts.join(" · ")}`));
      }
      limitsRoot.append(bucketRoot);
    });
    card.append(limitsRoot);
  } else {
    card.append(element("p", "empty compact", "暂无额度窗口"));
  }

  const usage = [];
  if (Number.isFinite(Number(account.usageLifetimeTokens))) {
    usage.push(`累计 token ${formatNumber(account.usageLifetimeTokens)}`);
  }
  if (Number.isFinite(Number(account.usagePeakDailyTokens))) {
    usage.push(`日峰值 ${formatNumber(account.usagePeakDailyTokens)}`);
  }
  if (Number.isFinite(Number(account.resetCreditsAvailable))) {
    usage.push(`可用 reset ${formatNumber(account.resetCreditsAvailable)}`);
  }
  if (usage.length) card.append(element("p", "meta usage", usage.join(" · ")));
  card.append(element("p", "meta", `更新于 ${formatTime(account.observedAt)}`));
  return card;
}

function renderSession(session) {
  const item = element("article", "session");
  const heading = element("div", "session-heading");
  heading.append(element("h4", "session-title", session.title || session.threadId || "未命名会话"));
  heading.append(statusPill(session.status, SESSION_STATUS_LABELS));
  item.append(heading);
  if (session.lastOutputSummary) item.append(element("p", "session-preview", session.lastOutputSummary));
  const times = [];
  if (Number.isFinite(Number(session.updatedAt))) times.push(`更新 ${formatTime(session.updatedAt)}`);
  if (Number.isFinite(Number(session.createdAt))) times.push(`创建 ${formatTime(session.createdAt)}`);
  if (times.length) item.append(element("p", "meta", times.join(" · ")));
  return item;
}

function renderPager(parent, label, page, totalPages, onPrevious, onNext) {
  const pager = element("div", "pager");
  const previous = element("button", "pager-button", "上一页");
  previous.type = "button";
  previous.disabled = page <= 0;
  previous.addEventListener("click", onPrevious);
  const next = element("button", "pager-button", "下一页");
  next.type = "button";
  next.disabled = page >= totalPages - 1;
  next.addEventListener("click", onNext);
  pager.append(previous, element("span", "pager-label", `${label} ${page + 1} / ${Math.max(1, totalPages)}`), next);
  parent.append(pager);
}

function renderOverview(state) {
  overviewRoot.replaceChildren();
  const hosts = Array.isArray(state.hosts) ? state.hosts : [];
  const accounts = Array.isArray(state.accounts) ? state.accounts : [];
  const active = hosts.reduce(
    (total, host) => total + (Array.isArray(host.sessions) ? host.sessions.filter((s) => s.status === "active").length : 0),
    0,
  );
  const waiting = hosts.reduce(
    (total, host) => total + (Array.isArray(host.sessions) ? host.sessions.filter((s) => s.status === "waiting").length : 0),
    0,
  );
  const errors = hosts.filter((host) => host.status === "offline" || host.status === "stale" || host.error).length;

  const summary = element("section", "summary-grid");
  [
    ["开发主机", hosts.length],
    ["账户", accounts.length],
    ["运行中", active],
    ["需关注", errors + waiting],
  ].forEach(([label, value]) => {
    const card = element("article", "summary-card");
    card.append(element("strong", "summary-value", formatNumber(value)));
    card.append(element("span", "summary-label", label));
    summary.append(card);
  });
  overviewRoot.append(summary);

  overviewRoot.append(
    element("p", "view-note", "选择下方页面查看额度、活动热力图或会话。数据每 15 秒自动刷新。"),
  );
  if (accounts.length) {
    const heading = element("div", "section-heading");
    heading.append(element("h2", null, "当前额度"));
    const openAccounts = element("button", "link-button", "打开额度");
    openAccounts.type = "button";
    openAccounts.addEventListener("click", () => switchView("accounts"));
    heading.append(openAccounts);
    overviewRoot.append(heading, renderAccount(accounts[0], true));
  } else {
    overviewRoot.append(element("p", "empty", "暂无账户数据"));
  }
}

function renderAccounts(state) {
  const accounts = Array.isArray(state.accounts) ? state.accounts : [];
  accountsRoot.replaceChildren();
  if (!accounts.length) {
    accountsRoot.append(element("p", "empty", "暂无账户数据"));
    return;
  }
  const totalPages = Math.max(1, Math.ceil(accounts.length / ACCOUNT_PAGE_SIZE));
  accountPage = Math.min(accountPage, totalPages - 1);
  const start = accountPage * ACCOUNT_PAGE_SIZE;
  accounts.slice(start, start + ACCOUNT_PAGE_SIZE).forEach((account) => accountsRoot.append(renderAccount(account)));
  renderPager(accountsRoot, "账户", accountPage, totalPages, () => {
    accountPage -= 1;
    renderAccounts(latestState);
  }, () => {
    accountPage += 1;
    renderAccounts(latestState);
  });
}

function dateKey(date) {
  return date.toISOString().slice(0, 10);
}

function shiftDate(value, days) {
  const [year, month, day] = value.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  date.setUTCDate(date.getUTCDate() + days);
  return dateKey(date);
}

function dailyMap(account) {
  const map = new Map();
  (Array.isArray(account && account.dailyUsage) ? account.dailyUsage : []).forEach((bucket) => {
    if (typeof bucket.startDate === "string" && Number.isFinite(Number(bucket.tokens))) {
      map.set(bucket.startDate, Number(bucket.tokens));
    }
  });
  return map;
}

function dailyLevel(tokens, maximum) {
  if (!tokens || !maximum) return 0;
  if (tokens >= maximum * 0.75) return 4;
  if (tokens >= maximum * 0.5) return 3;
  if (tokens >= maximum * 0.25) return 2;
  return 1;
}

function renderActivity(state) {
  activityRoot.replaceChildren();
  const accounts = Array.isArray(state.accounts) ? state.accounts : [];
  const account = accounts[0] && accounts[0].account;
  if (!account) {
    activityRoot.append(element("p", "empty", "暂无每日用量数据"));
    return;
  }
  const usage = dailyMap(account);
  const dates = Array.from(usage.keys()).sort();
  const latest = dates[dates.length - 1] || dateKey(new Date());
  const end = shiftDate(latest, activityPage * DAILY_PERIOD_DAYS);
  const start = shiftDate(end, -(DAILY_PERIOD_DAYS - 1));
  const maximum = Math.max(0, ...usage.values());

  const heading = element("div", "section-heading");
  heading.append(element("h2", null, "每日活动"));
  heading.append(element("span", "meta", dates.length ? `${dates.length} 天可用` : "暂无记录"));
  activityRoot.append(heading);

  const controls = element("div", "period-controls");
  const previous = element("button", "pager-button", "更早 14 天");
  previous.type = "button";
  previous.addEventListener("click", () => {
    activityPage -= 1;
    renderActivity(latestState);
  });
  const next = element("button", "pager-button", "更新 14 天");
  next.type = "button";
  next.disabled = activityPage >= 0;
  next.addEventListener("click", () => {
    activityPage += 1;
    renderActivity(latestState);
  });
  controls.append(previous, element("span", "period-label", `${start} — ${end}`), next);
  activityRoot.append(controls);

  const heatmap = element("div", "heatmap");
  heatmap.setAttribute("role", "list");
  for (let index = 0; index < DAILY_PERIOD_DAYS; index += 1) {
    const day = shiftDate(start, index);
    const tokens = usage.get(day) || 0;
    const cell = element("div", `heat-cell heat-${dailyLevel(tokens, maximum)}`);
    cell.setAttribute("role", "listitem");
    cell.setAttribute("aria-label", `${day}，${formatNumber(tokens)} token`);
    cell.title = `${day} · ${formatNumber(tokens)} token`;
    cell.append(element("span", "heat-date", day.slice(5).replace("-", "/")));
    cell.append(element("span", "heat-value", formatCompact(tokens)));
    heatmap.append(cell);
  }
  activityRoot.append(heatmap);
  activityRoot.append(
    element("p", "view-note", "每格显示日期与 token；当前数据由 Codex 返回的可用日 bucket 组成，暂不补造历史记录。"),
  );
}

function renderHost(host, page) {
  const card = element("article", "card host-card");
  const heading = element("div", "card-heading");
  heading.append(element("h2", "card-title", host.alias || host.hostId || "未知主机"));
  heading.append(statusPill(host.status, HOST_STATUS_LABELS));
  card.append(heading);
  const sessions = Array.isArray(host.sessions) ? host.sessions : [];
  const activeCount = sessions.filter((session) => session.status === "active").length;
  const collector = host.collectorStatus
    ? statusLabel(COLLECTOR_STATUS_LABELS, host.collectorStatus)
    : "尚未采集";
  card.append(element("p", "meta", `采集：${collector} · 运行中 ${activeCount} · 最近收到 ${formatTime(host.lastSeen)}`));
  if (host.error) card.append(element("p", "error", host.error));
  const totalPages = Math.max(1, Math.ceil(sessions.length / PAGE_SIZE));
  const currentPage = Math.min(page, totalPages - 1);
  const list = element("div", "sessions");
  sessions.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE).forEach((session) => list.append(renderSession(session)));
  card.append(list.children.length ? list : element("p", "empty compact", host.status === "unknown" ? "等待首次快照" : "暂无会话"));
  renderPager(card, "会话", currentPage, totalPages, () => {
    sessionPage -= 1;
    renderHosts(latestState);
  }, () => {
    sessionPage += 1;
    renderHosts(latestState);
  });
  return card;
}

function renderHosts(state) {
  const hosts = Array.isArray(state.hosts) ? state.hosts : [];
  hostsRoot.replaceChildren();
  if (!hosts.length) {
    hostsRoot.append(element("p", "empty", "暂无主机数据"));
    return;
  }
  hostPage = Math.min(hostPage, hosts.length - 1);
  sessionPage = Math.max(0, sessionPage);
  hostsRoot.append(renderHost(hosts[hostPage], sessionPage));
  renderPager(hostsRoot, "主机", hostPage, hosts.length, () => {
    hostPage -= 1;
    sessionPage = 0;
    renderHosts(latestState);
  }, () => {
    hostPage += 1;
    sessionPage = 0;
    renderHosts(latestState);
  });
}

function switchView(view) {
  if (!views.some((item) => item.id === `view-${view}`)) return;
  currentView = view;
  navButtons.forEach((button) => {
    const active = button.dataset.view === view;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-selected", String(active));
  });
  views.forEach((item) => {
    item.classList.toggle("is-active", item.id === `view-${view}`);
    item.hidden = item.id !== `view-${view}`;
  });
}

function render(state) {
  latestState = state;
  const accounts = Array.isArray(state.accounts) ? state.accounts : [];
  const hosts = Array.isArray(state.hosts) ? state.hosts : [];
  accountCount.textContent = String(accounts.length);
  hostCount.textContent = String(hosts.length);
  revision.textContent = `revision ${state.revision ?? "—"}`;
  renderOverview(state);
  renderAccounts(state);
  renderActivity(state);
  renderHosts(state);
  switchView(currentView);
}

async function readState() {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const response = await fetch("/api/state", {
      cache: "no-store",
      headers: { Accept: "application/json" },
      signal: controller.signal,
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.json();
  } finally {
    window.clearTimeout(timeout);
  }
}

async function refresh() {
  if (loading) return;
  loading = true;
  refreshButton.disabled = true;
  try {
    const state = await readState();
    render(state);
    const hostTotal = Array.isArray(state.hosts) ? state.hosts.length : 0;
    setConnection("connected", "已连接", `${hostTotal} 台主机 · 更新于 ${formatTime(state.generatedAt)}`);
  } catch (error) {
    const message = error && error.name === "AbortError" ? "请求超时" : error.message || "网络错误";
    setConnection("error", "连接中断", `状态可能过期 · ${message}`);
  } finally {
    loading = false;
    refreshButton.disabled = false;
    nextRefresh = window.setTimeout(refresh, POLL_INTERVAL_MS);
  }
}

navButtons.forEach((button) => button.addEventListener("click", () => switchView(button.dataset.view)));
refreshButton.addEventListener("click", () => {
  window.clearTimeout(nextRefresh);
  refresh();
});
window.addEventListener("online", refresh);
refresh();

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(() => {
    // Offline shell is optional; the monitor remains usable without it.
  });
}
