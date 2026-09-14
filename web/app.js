// Chrome 85 has no Element.replaceChildren; keep one compatibility boundary for all render roots.
if (typeof Element.prototype.replaceChildren !== "function") {
  Element.prototype.replaceChildren = function (...nodes) {
    this.textContent = "";
    nodes.forEach((node) => {
      this.appendChild(node instanceof Node ? node : document.createTextNode(String(node)));
    });
  };
}

const POLL_INTERVAL_MS = 15_000;
const REQUEST_TIMEOUT_MS = 8_000;
const PAGE_SIZE = 4;
const ACCOUNT_PAGE_SIZE = 2;
const EINK_DEVICE_PAGE_SIZE = 3;
const STATE_CACHE_DB = "codex-monitor";
const STATE_CACHE_STORE = "state";
const STATE_CACHE_KEY = "latest";
const ENDPOINT_STORAGE_KEY = "codex-monitor-endpoint";
const ENDPOINTS_STORAGE_KEY = "codex-monitor-endpoints";
const TOKEN_STORAGE_KEY = "codex-monitor-read-token";
const CONTROL_TOKEN_STORAGE_KEY = "codex-monitor-control-token";
const THEME_STORAGE_KEY = "codex-monitor-theme";
const ACCENT_STORAGE_KEY = "codex-monitor-accent";
const MAX_FAILOVER_ENDPOINTS = 4;

const ACCENT_PALETTES = {
  red: { label: "赤", accent: "#DC766D", strong: "#9D302B", soft: "#FCEAE7" },
  orange: { label: "橙", accent: "#E9A061", strong: "#985010", soft: "#FFF0E2" },
  yellow: { label: "黄", accent: "#E1C65A", strong: "#776000", soft: "#FFF8D9" },
  green: { label: "绿", accent: "#8ED8BD", strong: "#246A53", soft: "#E6F5ED" },
  cyan: { label: "青", accent: "#79CED2", strong: "#176970", soft: "#E4F5F5" },
  blue: { label: "蓝", accent: "#87AFE8", strong: "#315A9D", soft: "#EAF1FC" },
  purple: { label: "紫", accent: "#B49BDC", strong: "#674094", soft: "#F2EBFA" },
};

const refreshButton = document.querySelector("#refresh");
const settingsToggle = document.querySelector("#settings-toggle");
const settingsClose = document.querySelector("#settings-close");
const settingsBackdrop = document.querySelector("#settings-backdrop");
const settingsPanel = document.querySelector("#settings-panel");
const themeSelect = document.querySelector("#theme-select");
const accentOptions = document.querySelector("#accent-options");
const androidShellSettings = document.querySelector("#android-shell-settings");
const hideStatusBar = document.querySelector("#hide-status-bar");
const hideNavigationBar = document.querySelector("#hide-navigation-bar");
const androidNodeAddress = document.querySelector("#android-node-address");
const settingsTabs = Array.from(document.querySelectorAll("[data-settings-tab]"));
const settingsTabpanels = Array.from(document.querySelectorAll(".settings-tabpanel"));
const endpointForm = document.querySelector("#endpoint-form");
const endpointInput = document.querySelector("#endpoint-input");
const tokenInput = document.querySelector("#token-input");
const controlTokenInput = document.querySelector("#control-token-input");
const endpointReset = document.querySelector("#endpoint-reset");
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
const identityForm = document.querySelector("#identity-form");
const identityAliasInput = document.querySelector("#identity-alias-input");
const identityOs = document.querySelector("#identity-os");
const identityStatus = document.querySelector("#identity-status");
const pairingCodeToggle = document.querySelector("#pairing-code-toggle");
const pairingCodePanel = document.querySelector("#pairing-code-panel");
const pairingCodeValue = document.querySelector("#pairing-code-value");
const pairingCodeRefresh = document.querySelector("#pairing-code-refresh");
const pairingCodeHide = document.querySelector("#pairing-code-hide");
const discoveriesRoot = document.querySelector("#discoveries");
const discoveriesStatus = document.querySelector("#discoveries-status");
const discoveriesRefresh = document.querySelector("#discoveries-refresh");
const connectedDevicesRoot = document.querySelector("#connected-devices");
const devicePager = document.querySelector("#device-pager");

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

const CONNECTED_DEVICE_STATUS_LABELS = {
  ...HOST_STATUS_LABELS,
  paired: "已配对",
};

const DISCOVERY_STATUS_LABELS = {
  discovered: "已发现",
  pairing: "配对中",
  paired: "已配对",
  connected: "已连接",
  failed: "失败",
  incompatible: "网络不兼容",
  expired: "已过期",
  unknown: "未知",
};

let loading = false;
let nextRefresh = 0;
let currentView = "overview";
let accountPage = 0;
let hostPage = 0;
let sessionPage = 0;
let activityMonth = localMonthKey();
let activitySelection = localDateKey();
let activityAccountKey = "";
let latestState = null;
let stateCursor = null;
let showingCachedState = false;
let cachedStateSource = null;
let cachedAt = null;
let endpointBase = initialEndpoint();
let knownEndpoints = loadKnownEndpoints(endpointBase);
let endpointGeneration = 0;
let clientToken = readStorage(TOKEN_STORAGE_KEY) || "";
let controlToken = readStorage(CONTROL_TOKEN_STORAGE_KEY) || "";
let currentSettingsTab = "display";
let settingsFocusBeforeOpen = null;
let devicePage = 0;
let discoveryState = null;
let discoveryLoading = false;
let identityLoading = false;
let discoveryRequestGeneration = 0;
let identityRequestGeneration = 0;
let currentEndpointOnline = false;
let liveEndpointIdentity = null;
let pairingCodeRequestGeneration = 0;
const pendingPairNodeIds = new Set();

function readStorage(key) {
  try {
    return window.localStorage.getItem(key);
  } catch (_) {
    return null;
  }
}

function writeStorage(key, value) {
  try {
    if (value) window.localStorage.setItem(key, value);
    else window.localStorage.removeItem(key);
  } catch (_) {
    // A URL or token entered for this page still works for the current session.
  }
}

function normalizeTheme(value) {
  return value === "eink" ? "eink" : "normal";
}

function normalizeAccent(value) {
  return Object.prototype.hasOwnProperty.call(ACCENT_PALETTES, value) ? value : "green";
}

function applyTheme(value, persist = true) {
  const theme = normalizeTheme(value);
  document.documentElement.dataset.theme = theme;
  if (themeSelect) themeSelect.value = theme;
  const themeMeta = document.querySelector('meta[name="theme-color"]');
  if (themeMeta) themeMeta.content = theme === "eink" ? "#ffffff" : "#f5f3ed";
  if (persist) writeStorage(THEME_STORAGE_KEY, theme);
}

function applyAccent(value, persist = true) {
  const accent = normalizeAccent(value);
  document.documentElement.dataset.accent = accent;
  if (accentOptions) {
    Array.from(accentOptions.querySelectorAll("[data-accent]"))
      .forEach((button) => button.setAttribute("aria-checked", String(button.dataset.accent === accent)));
  }
  if (persist) writeStorage(ACCENT_STORAGE_KEY, accent);
}

function focusableIn(container) {
  return Array.from(container.querySelectorAll(
    'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
  )).filter((node) => !node.hidden && !node.closest("[hidden]") && node.getAttribute("aria-hidden") !== "true");
}

function trapSettingsFocus(event) {
  if (!settingsBackdrop || settingsBackdrop.hidden || event.key !== "Tab") return;
  const focusable = focusableIn(settingsPanel);
  if (!focusable.length) {
    event.preventDefault();
    settingsPanel.focus();
    return;
  }
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

function setSettingsTab(tab) {
  if (!settingsPanel || !settingsTabs.length) return;
  const next = settingsTabs.some((item) => item.dataset.settingsTab === tab) ? tab : "display";
  const leavingDevice = currentSettingsTab === "device" && next !== "device";
  currentSettingsTab = next;
  if (leavingDevice) hideLocalPairingCode();
  settingsTabs.forEach((button) => {
    const active = button.dataset.settingsTab === next;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-selected", String(active));
    button.tabIndex = active ? 0 : -1;
  });
  settingsTabpanels.forEach((panel) => {
    const active = panel.id === `settings-${next}`;
    panel.classList.toggle("is-active", active);
    panel.hidden = !active;
  });
  if (next === "device") {
    renderDeviceSettings();
    loadDeviceIdentity();
    loadDiscoveries();
  }
}

function setSettingsOpen(open) {
  if (!settingsPanel || !settingsToggle || !settingsBackdrop) return;
  settingsBackdrop.hidden = !open;
  settingsToggle.setAttribute("aria-expanded", String(open));
  if (open) {
    settingsFocusBeforeOpen = document.activeElement;
    setSettingsTab(currentSettingsTab);
    window.setTimeout(() => {
      const first = focusableIn(settingsPanel)[0];
      (first || settingsPanel).focus();
    }, 0);
  } else {
    hideLocalPairingCode();
    if (settingsFocusBeforeOpen && settingsFocusBeforeOpen.isConnected) settingsFocusBeforeOpen.focus();
    settingsFocusBeforeOpen = null;
  }
}

function normalizeEndpoint(value) {
  try {
    const url = new URL(value || window.location.origin, window.location.origin);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    url.hash = "";
    url.search = "";
    return url.toString().replace(/\/$/, "");
  } catch (_) {
    return null;
  }
}

function initialEndpoint() {
  const queryEndpoint = new URLSearchParams(window.location.search).get("endpoint");
  const storedEndpoint = readStorage(ENDPOINT_STORAGE_KEY);
  return normalizeEndpoint(queryEndpoint) || normalizeEndpoint(storedEndpoint) || window.location.origin;
}

function loadKnownEndpoints(current) {
  let saved = [];
  try {
    const value = JSON.parse(readStorage(ENDPOINTS_STORAGE_KEY) || "[]");
    if (Array.isArray(value)) saved = value;
  } catch (_) {
    saved = [];
  }
  const values = [current, ...saved]
    .filter((value) => typeof value === "string" && value.trim())
    .map(normalizeEndpoint)
    .filter(Boolean);
  return [...new Set(values)].slice(0, MAX_FAILOVER_ENDPOINTS);
}

function rememberEndpoints(values) {
  const normalized = values
    .filter((value) => typeof value === "string" && value.trim())
    .map(normalizeEndpoint)
    .filter(Boolean);
  knownEndpoints = [...new Set([endpointBase, ...normalized])].slice(0, MAX_FAILOVER_ENDPOINTS);
  writeStorage(ENDPOINTS_STORAGE_KEY, JSON.stringify(knownEndpoints));
}

function rememberPeerEndpoints(state) {
  const peers = Array.isArray(state && state.peers) ? state.peers : [];
  const endpoints = peers.flatMap((peer) => {
    if (!peer) return [];
    return [peer.httpEndpoint, ...(Array.isArray(peer.httpEndpoints) ? peer.httpEndpoints : [])];
  });
  rememberEndpoints(endpoints);
}

function endpointCacheKey() {
  return endpointBase === window.location.origin ? STATE_CACHE_KEY : `${STATE_CACHE_KEY}:${endpointBase}`;
}

function endpointUrl(path) {
  return `${endpointBase}${path}`;
}

function readRequestHeaders(token) {
  const headers = { Accept: "application/json" };
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

function openStateCache() {
  if (!("indexedDB" in window)) return Promise.resolve(null);
  return new Promise((resolve, reject) => {
    const request = window.indexedDB.open(STATE_CACHE_DB, 1);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STATE_CACHE_STORE)) {
        request.result.createObjectStore(STATE_CACHE_STORE, { keyPath: "id" });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("IndexedDB unavailable"));
  });
}

function storeRequest(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("IndexedDB request failed"));
  });
}

async function readCachedState() {
  try {
    const database = await openStateCache();
    if (!database) return null;
    const transaction = database.transaction(STATE_CACHE_STORE, "readonly");
    const record = await storeRequest(transaction.objectStore(STATE_CACHE_STORE).get(endpointCacheKey()));
    database.close();
    return record && record.state ? record : null;
  } catch (_) {
    return null;
  }
}

async function writeCachedState(state, cacheKey = endpointCacheKey()) {
  try {
    const database = await openStateCache();
    if (!database) return;
    const transaction = database.transaction(STATE_CACHE_STORE, "readwrite");
    transaction.objectStore(STATE_CACHE_STORE).put({
      id: cacheKey,
      savedAt: Math.floor(Date.now() / 1000),
      state,
    });
    await new Promise((resolve, reject) => {
      transaction.oncomplete = resolve;
      transaction.onerror = () => reject(transaction.error || new Error("IndexedDB write failed"));
      transaction.onabort = () => reject(transaction.error || new Error("IndexedDB write aborted"));
    });
    database.close();
  } catch (_) {
    // The monitor remains usable when storage is disabled or full.
  }
}

function staleCachedState(state) {
  return {
    ...state,
    hosts: Array.isArray(state.hosts)
      ? state.hosts.map((host) => ({
          ...host,
          status: host.status === "connected" ? "stale" : host.status,
      }))
      : state.hosts,
    peers: Array.isArray(state.peers)
      ? state.peers.map((peer) => ({
          ...peer,
          status: peer.status === "connected" ? "stale" : peer.status,
        }))
      : state.peers,
  };
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined && text !== null) {
    if (className && className.split(/\s+/).includes("empty")) {
      node.append(renderPixelCat("sleeping"), document.createTextNode(text));
    } else node.textContent = text;
  }
  return node;
}

function renderAccentOptions() {
  if (!accentOptions) return;
  accentOptions.replaceChildren();
  Object.keys(ACCENT_PALETTES).forEach((name) => {
    const palette = ACCENT_PALETTES[name];
    const option = element("button", "accent-option", palette.label);
    option.type = "button";
    option.dataset.accent = name;
    option.setAttribute("role", "radio");
    option.setAttribute("aria-label", `${palette.label}主题色`);
    option.setAttribute("aria-checked", "false");
    option.style.setProperty("--option-accent", palette.accent);
    option.style.setProperty("--option-strong", palette.strong);
    option.style.setProperty("--option-soft", palette.soft);
    option.addEventListener("click", () => applyAccent(name));
    accentOptions.append(option);
  });
}

function initializeAndroidShellSettings() {
  const bridge = window.CodexMonitorAndroid;
  if (!bridge || !androidShellSettings || !hideStatusBar || !hideNavigationBar) return;
  try {
    hideStatusBar.checked = Boolean(bridge.getHideStatusBar());
    hideNavigationBar.checked = Boolean(bridge.getHideNavigationBar());
    androidShellSettings.hidden = false;
  } catch (_error) {
    return;
  }

  hideStatusBar.addEventListener("change", () => {
    try {
      bridge.setHideStatusBar(hideStatusBar.checked);
    } catch (_error) {
      hideStatusBar.checked = !hideStatusBar.checked;
    }
  });
  hideNavigationBar.addEventListener("change", () => {
    try {
      bridge.setHideNavigationBar(hideNavigationBar.checked);
    } catch (_error) {
      hideNavigationBar.checked = !hideNavigationBar.checked;
    }
  });
  if (androidNodeAddress) {
    androidNodeAddress.addEventListener("click", () => {
      try {
        bridge.showNodeAddressSettings();
      } catch (_error) {
        // The native shell may have been detached while this page was cached.
      }
    });
  }
}

function renderMiniCat() {
  return renderPixelCat("peeking");
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
    "discovered",
    "pairing",
    "paired",
    "failed",
    "incompatible",
    "expired",
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

function latestSnapshotTime(state) {
  const times = (Array.isArray(state && state.hosts) ? state.hosts : [])
    .map((host) => Number(host && host.lastSnapshotAt))
    .filter((value) => Number.isFinite(value) && value > 0);
  return times.length ? Math.max(...times) : Number(state && state.generatedAt) || 0;
}

function formatPercent(value) {
  if (!Number.isFinite(Number(value))) return "—";
  return `${Math.round(Math.max(0, Math.min(100, Number(value))))}%`;
}

function remainingPercent(usedPercent) {
  const used = Number(usedPercent);
  return Number.isFinite(used) ? 100 - Math.max(0, Math.min(100, used)) : Number.NaN;
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
  const remaining = remainingPercent(window.usedPercent);
  const block = element("div", "limit-window");
  const heading = element("div", "limit-heading");
  heading.append(element("strong", "limit-name", label));
  heading.append(element("span", "limit-value", formatPercent(remaining)));
  block.append(heading);

  const meter = element("div", "meter");
  const fill = element("span", "meter-fill");
  fill.style.width = `${Number.isFinite(remaining) ? remaining : 0}%`;
  meter.append(fill);
  const filledSegments = Number.isFinite(remaining) ? Math.round(remaining / 10) : 0;
  const segments = element("span", "meter-segments");
  for (let index = 0; index < 10; index += 1) {
    segments.append(element("span", `meter-segment${index < filledSegments ? " is-filled" : ""}`));
  }
  meter.append(segments);
  meter.setAttribute("role", "progressbar");
  meter.setAttribute("aria-valuemin", "0");
  meter.setAttribute("aria-valuemax", "100");
  if (Number.isFinite(remaining)) meter.setAttribute("aria-valuenow", String(Math.round(remaining)));
  block.append(meter);

  if (Number.isFinite(Number(window.resetsAt))) {
    block.append(
      element("p", "limit-details", `重置 ${formatTime(window.resetsAt)}（${resetCountdown(window.resetsAt)}）`),
    );
  }
  parent.append(block);
}

function quotaWindows(bucket) {
  const candidates = [bucket && bucket.primary, bucket && bucket.secondary].filter(Boolean);
  const fiveHour = candidates.find((window) => Number(window.windowDurationMins) === 5 * 60) || null;
  const week = candidates.find((window) => Number(window.windowDurationMins) === 7 * 24 * 60) || null;
  return { fiveHour, week };
}

function isSparkBucket(bucket) {
  return Boolean(
    bucket &&
      (bucket.limitId === "codex_bengalfox" || bucket.limitName === "GPT-5.3-Codex-Spark"),
  );
}

function visibleRateLimits(account) {
  return (Array.isArray(account && account.rateLimits) ? account.rateLimits : []).filter(
    (bucket) => !isSparkBucket(bucket),
  );
}

function renderRemainingDots(remaining) {
  const grid = element("span", "remaining-dots");
  grid.setAttribute("aria-hidden", "true");
  const amount = Number.isFinite(Number(remaining))
    ? Math.round(Math.max(0, Math.min(100, Number(remaining))) * 24 / 100)
    : 0;
  for (let index = 0; index < 24; index += 1) {
    grid.append(element("i", `remaining-dot${index < amount ? " is-filled" : ""}`));
  }
  return grid;
}

function renderOverviewQuota(view) {
  const account = view && view.account ? view.account : {};
  const root = element("div", "overview-quota");
  const limits = visibleRateLimits(account);
  if (!view || !account || !limits.length) {
    root.append(element("p", "empty compact", "暂无额度窗口"));
    return root;
  }

  const primary = limits.find((bucket) => bucket && bucket.limitId === "codex") || limits[0];
  const primaryWindows = quotaWindows(primary);
  const featuredWindow = primaryWindows.week || primaryWindows.fiveHour;
  if (featuredWindow) {
    const featured = element("div", "overview-quota-primary");
    const heading = element("div", "limit-heading overview-quota-heading");
    const remaining = remainingPercent(featuredWindow.usedPercent);
    heading.append(
      element(
        "p",
        "overview-quota-label",
        `${primary.limitName || primary.limitId || "codex"} · ${primaryWindows.week ? "一周限制" : "五小时限制"}`,
      ),
      element("strong", "overview-quota-value", formatPercent(remaining)),
    );
    featured.append(heading, renderRemainingDots(remaining));

    const meter = element("div", "meter");
    const fill = element("span", "meter-fill");
    fill.style.width = `${Number.isFinite(remaining) ? remaining : 0}%`;
    meter.append(fill);
    const filledSegments = Number.isFinite(remaining) ? Math.round(remaining / 10) : 0;
    const segments = element("span", "meter-segments");
    for (let index = 0; index < 10; index += 1) {
      segments.append(element("span", `meter-segment${index < filledSegments ? " is-filled" : ""}`));
    }
    meter.append(segments);
    meter.setAttribute("role", "progressbar");
    meter.setAttribute("aria-valuemin", "0");
    meter.setAttribute("aria-valuemax", "100");
    if (Number.isFinite(remaining)) meter.setAttribute("aria-valuenow", String(Math.round(remaining)));
    featured.append(meter);
    if (Number.isFinite(Number(featuredWindow.resetsAt))) {
      featured.append(
        element(
          "p",
          "limit-details",
          `重置 ${formatTime(featuredWindow.resetsAt)}（${resetCountdown(featuredWindow.resetsAt)}）`,
        ),
      );
    }
    root.append(featured);
  }

  limits.filter((bucket) => bucket !== primary).forEach((bucket) => {
    const windows = quotaWindows(bucket);
    const parts = [];
    if (windows.week) parts.push(`一周 ${formatPercent(remainingPercent(windows.week.usedPercent))}`);
    if (windows.fiveHour) parts.push(`五小时 ${formatPercent(remainingPercent(windows.fiveHour.usedPercent))}`);
    if (parts.length) {
      const row = element("div", "overview-quota-row");
      row.append(
        element("span", null, bucket.limitName || bucket.limitId || "其他额度"),
        element("strong", null, parts.join(" · ")),
      );
      root.append(row);
    }
  });
  if (!root.children.length) root.append(element("p", "empty compact", "暂无额度窗口"));
  return root;
}

function renderAccount(view, compact = false) {
  const account = view.account || {};
  const card = element("article", `card account-card${compact ? " compact-card" : ""}`);
  const heading = element("div", "card-heading");
  const title = element("div");
  title.append(
    element(
      "h3",
      "card-title",
      account.planType || (account.requiresOpenaiAuth ? "需要登录" : "计划未知"),
    ),
  );
  title.append(
    element("p", "meta account-secondary", account.authMode ? `登录方式：${account.authMode}` : "登录方式未知"),
  );
  heading.append(title);
  heading.append(element("span", "source-count", `${(view.sourceHosts || []).length} 台主机`));
  card.append(heading);

  if (!compact && Array.isArray(view.sourceHosts) && view.sourceHosts.length) {
    card.append(element("p", "meta account-secondary", `来源：${view.sourceHosts.join("、")}`));
  }

  const limits = visibleRateLimits(account);
  if (limits.length) {
    const limitsRoot = element("div", "limits");
    limits.forEach((bucket) => {
      const bucketRoot = element("div", "limit-bucket");
      bucketRoot.append(element("h4", "bucket-title", bucket.limitName || bucket.limitId || "额度"));
      const windows = quotaWindows(bucket);
      renderWindow(bucketRoot, "五小时限制", windows.fiveHour);
      renderWindow(bucketRoot, "一周限制", windows.week);
      if (!compact && bucket.credits) {
        const creditParts = [];
        if (bucket.credits.balance !== undefined && bucket.credits.balance !== null) {
          creditParts.push(`余额 ${bucket.credits.balance}`);
        }
        if (bucket.credits.unlimited === true) creditParts.push("无限");
        if (creditParts.length) bucketRoot.append(element("p", "credit account-secondary", `Credits：${creditParts.join(" · ")}`));
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
  if (usage.length) card.append(element("p", "meta usage account-secondary", usage.join(" · ")));
  card.append(element("p", "meta account-secondary", `更新于 ${formatTime(account.observedAt)}`));
  return card;
}

function makeRequestId() {
  if (globalThis.crypto && typeof globalThis.crypto.randomUUID === "function") {
    return globalThis.crypto.randomUUID();
  }
  return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

async function interruptSession(hostId, session, button) {
  if (!controlToken || !session.threadId || !session.turnId) return;
  if (!window.confirm("确认中断当前会话？")) return;
  button.disabled = true;
  button.textContent = "发送中…";
  try {
    const headers = {
      Accept: "application/json",
      "Content-Type": "application/json",
      Authorization: `Bearer ${controlToken}`,
    };
    const response = await fetch(endpointUrl("/api/v1/control"), {
      method: "POST",
      cache: "no-store",
      headers,
      body: JSON.stringify({
        action: "interrupt",
        targetNodeId: hostId,
        threadId: session.threadId,
        turnId: session.turnId,
        requestId: makeRequestId(),
        expiresAt: Math.floor(Date.now() / 1000) + 60,
      }),
    });
    const result = await response.json().catch(() => ({}));
    if (!response.ok || result.ok !== true) {
      throw new Error(result.message || result.error || `HTTP ${response.status}`);
    }
    button.textContent = "已请求中断";
    window.setTimeout(refresh, 500);
  } catch (error) {
    button.textContent = `失败：${error.message || "控制不可用"}`;
    window.setTimeout(() => {
      if (button.isConnected) {
        button.textContent = "中断";
        button.disabled = false;
      }
    }, 2_500);
  }
}

function renderSession(session, hostId) {
  const item = element("article", "session");
  const heading = element("div", "session-heading");
  const title = element("div", "session-title-lockup");
  title.append(element("span", "pixel-paw"), element("h4", "session-title", session.title || session.threadId || "未命名会话"));
  heading.append(title);
  heading.append(statusPill(session.status, SESSION_STATUS_LABELS));
  item.append(heading);
  if (session.lastOutputSummary) item.append(element("p", "session-preview", session.lastOutputSummary));
  if (Number.isFinite(Number(session.updatedAt))) {
    item.append(element("p", "meta session-updated", `更新 ${formatTime(session.updatedAt)}`));
  }
  const times = [];
  if (Number.isFinite(Number(session.createdAt))) times.push(`创建 ${formatTime(session.createdAt)}`);
  if (session.threadId) times.push(`线程 ${session.threadId}`);
  if (times.length) item.append(element("p", "meta session-secondary", times.join(" · ")));
  if (session.turnId && ["active", "waiting"].includes(session.status)) {
    const actions = element("div", "session-actions");
    if (controlToken) {
      const interrupt = element("button", "link-button", "中断");
      interrupt.type = "button";
      interrupt.addEventListener("click", () => interruptSession(hostId, session, interrupt));
      actions.append(interrupt);
    } else {
      actions.append(element("span", "meta", "设置控制 token 后可中断"));
    }
    item.append(actions);
  }
  return item;
}

function sessionPageSize() {
  if (document.documentElement.dataset.theme === "eink") return 2;
  return window.innerWidth >= 1024 ? 5 : 3;
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

function renderNodeStatus(host) {
  const item = element("article", "node-status-item");
  const copy = element("div", "node-status-copy");
  const sessions = Array.isArray(host.sessions) ? host.sessions : [];
  const active = sessions.filter((session) => session.status === "active").length;
  const collector = host.collectorStatus
    ? statusLabel(COLLECTOR_STATUS_LABELS, host.collectorStatus)
    : "尚未采集";
  const title = element("div", "node-status-title");
  title.append(renderPixelCat("face"), element("h3", "node-status-name", host.alias || host.hostId || "未知主机"));
  copy.append(title);
  copy.append(
    element(
      "p",
      "node-status-meta node-tech",
      `${collector} · ${active} 个运行中 · 快照 ${formatTime(host.lastSnapshotAt || host.lastSeen)}`,
    ),
  );
  item.append(copy, statusPill(host.status, HOST_STATUS_LABELS));
  return item;
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

  const columns = element("div", "overview-columns");
  const quota = element("section", "card overview-section overview-featured");
  const quotaHeading = element("div", "card-heading overview-section-heading");
  const quotaTitle = element("div", "overview-heading-title");
  quotaTitle.append(renderMiniCat(), element("h2", null, "额度摘要"));
  quotaHeading.append(quotaTitle);
  const openAccounts = element("button", "link-button", "查看额度");
  openAccounts.type = "button";
  openAccounts.addEventListener("click", () => switchView("accounts"));
  quotaHeading.append(openAccounts);
  quota.append(quotaHeading);
  quota.append(accounts.length ? renderOverviewQuota(accounts[0]) : element("p", "empty compact", "暂无账户数据"));

  const nodes = element("section", "card overview-section overview-nodes");
  const nodesHeading = element("div", "card-heading overview-section-heading");
  nodesHeading.append(element("h2", null, "节点状态"));
  nodesHeading.append(element("span", "count", String(hosts.length)));
  nodes.append(nodesHeading);
  const nodeList = element("div", "node-status-list");
  if (hosts.length) hosts.forEach((host) => nodeList.append(renderNodeStatus(host)));
  else nodeList.append(element("p", "empty compact", "暂无主机数据"));
  nodes.append(nodeList);
  columns.append(quota, nodes);
  overviewRoot.append(columns);
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

function localDateKey(date = new Date()) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function localMonthKey(date = new Date()) {
  return localDateKey(date).slice(0, 7);
}

function shiftDate(value, days) {
  const [year, month, day] = value.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  date.setUTCDate(date.getUTCDate() + days);
  return dateKey(date);
}

function daysInMonth(monthValue) {
  const [year, month] = monthValue.split("-").map(Number);
  return new Date(Date.UTC(year, month, 0)).getUTCDate();
}

function monthCalendar(monthValue) {
  const [year, month] = monthValue.split("-").map(Number);
  const firstDay = new Date(Date.UTC(year, month - 1, 1));
  const mondayOffset = (firstDay.getUTCDay() + 6) % 7;
  const count = daysInMonth(monthValue);
  const total = Math.ceil((mondayOffset + count) / 7) * 7;
  return Array.from({ length: total }, (_, index) => {
    const day = index - mondayOffset + 1;
    return day < 1 || day > count
      ? { date: null, outside: true }
      : { date: `${monthValue}-${String(day).padStart(2, "0")}`, outside: false };
  });
}

function calendarDays(monthValue) {
  return monthCalendar(monthValue);
}

function monthTitle(monthValue) {
  const [year, month] = monthValue.split("-").map(Number);
  return `${year}年${month}月`;
}

function shiftMonth(monthValue, amount) {
  const [year, month] = monthValue.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1 + amount, 1));
  return `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(2, "0")}`;
}

function clampDateToMonth(dateValue, monthValue) {
  const day = Number(String(dateValue || "").slice(8, 10));
  const safeDay = Math.min(Math.max(Number.isFinite(day) && day > 0 ? day : 1, 1), daysInMonth(monthValue));
  return `${monthValue}-${String(safeDay).padStart(2, "0")}`;
}

function dailyMap(account) {
  const map = new Map();
  (Array.isArray(account && account.dailyUsage) ? account.dailyUsage : []).forEach((bucket) => {
    if (bucket && typeof bucket.startDate === "string" && Number.isFinite(Number(bucket.tokens))) {
      map.set(bucket.startDate, Number(bucket.tokens));
    }
  });
  return map;
}

function dailyLevel(tokens, maximum, hasData = Number(tokens) > 0) {
  if (!hasData) return 0;
  if (!maximum) return 1;
  if (tokens >= maximum * 0.75) return 4;
  if (tokens >= maximum * 0.5) return 3;
  if (tokens >= maximum * 0.25) return 2;
  return 1;
}

function renderPixelCat(kind = "prone") {
  const sprite = kind === "face" ? "peeking" : kind;
  const supported = ["sitting", "peeking", "prone", "sleeping"];
  const span = document.createElement("span");
  span.className = `pixel-cat pixel-cat-${supported.includes(sprite) ? sprite : "prone"}`;
  span.setAttribute("aria-hidden", "true");
  return span;
}

function activityAccountId(view, index) {
  return String(view && view.accountKey ? view.accountKey : `account-${index}`);
}

function activityAccount(accounts) {
  if (!accounts.length) return null;
  const index = accounts.findIndex((view, position) => activityAccountId(view, position) === activityAccountKey);
  const selectedIndex = index >= 0 ? index : 0;
  activityAccountKey = activityAccountId(accounts[selectedIndex], selectedIndex);
  return accounts[selectedIndex] && accounts[selectedIndex].account
    ? accounts[selectedIndex].account
    : null;
}

function renderActivity(state) {
  activityRoot.replaceChildren();
  const accounts = Array.isArray(state.accounts) ? state.accounts : [];
  const account = activityAccount(accounts);
  if (!account) {
    activityRoot.append(element("p", "empty", "暂无每日用量数据"));
    return;
  }
  const usage = dailyMap(account);
  const maximum = Math.max(0, ...usage.values());
  activitySelection = clampDateToMonth(activitySelection, activityMonth);

  const heading = element("div", "activity-heading");
  const title = element("div", "activity-title");
  title.append(renderPixelCat("prone"), element("h2", null, "活动"));
  heading.append(title);
  const controls = element("div", "calendar-controls");
  const previous = element("button", "small-button", "上月");
  previous.type = "button";
  previous.addEventListener("click", () => {
    activityMonth = shiftMonth(activityMonth, -1);
    activitySelection = clampDateToMonth(activitySelection, activityMonth);
    renderActivity(latestState);
  });
  const next = element("button", "small-button", "下月");
  next.type = "button";
  next.addEventListener("click", () => {
    activityMonth = shiftMonth(activityMonth, 1);
    activitySelection = clampDateToMonth(activitySelection, activityMonth);
    renderActivity(latestState);
  });
  const today = element("button", "small-button", "今天");
  today.type = "button";
  today.addEventListener("click", () => {
    activityMonth = localMonthKey();
    activitySelection = localDateKey();
    renderActivity(latestState);
  });
  controls.append(previous, element("strong", "month-title", monthTitle(activityMonth)), next, today);
  heading.append(controls);
  activityRoot.append(heading);

  if (accounts.length > 1) {
    const accountPicker = element("label", "activity-account", "账户");
    const select = element("select");
    select.setAttribute("aria-label", "选择活动账户");
    accounts.forEach((view, index) => {
      const option = element("option", null, view.account && (view.account.planType || view.account.authMode)
        ? `${view.account.planType || view.account.authMode} · ${index + 1}`
        : `账户 ${index + 1}`);
      option.value = activityAccountId(view, index);
      select.append(option);
    });
    select.value = activityAccountKey;
    select.addEventListener("change", () => {
      activityAccountKey = select.value;
      activitySelection = clampDateToMonth(localDateKey(), activityMonth);
      renderActivity(latestState);
    });
    accountPicker.append(select);
    activityRoot.append(accountPicker);
  }

  const calendar = element("div", "calendar");
  const weekdays = element("div", "calendar-weekdays");
  ["一", "二", "三", "四", "五", "六", "日"].forEach((day) => weekdays.append(element("span", null, day)));
  calendar.append(weekdays);
  const heatmap = element("div", "calendar-grid");
  heatmap.setAttribute("role", "grid");
  calendarDays(activityMonth).forEach((entry) => {
    if (!entry.date) {
      const blank = element("span", "calendar-cell calendar-cell-empty");
      blank.setAttribute("aria-hidden", "true");
      heatmap.append(blank);
      return;
    }
    const hasData = usage.has(entry.date);
    const tokens = hasData ? usage.get(entry.date) : 0;
    const cell = element("button", `calendar-cell heat-${dailyLevel(tokens, maximum, hasData)}`);
    cell.type = "button";
    cell.setAttribute("role", "gridcell");
    cell.setAttribute("aria-label", `${entry.date}，${hasData ? `${formatNumber(tokens)} token` : "无数据"}`);
    cell.title = `${entry.date} · ${hasData ? `${formatNumber(tokens)} token` : "无数据"}`;
    if (entry.date === activitySelection) cell.classList.add("is-selected");
    if (entry.date === localDateKey()) cell.classList.add("is-today");
    cell.append(element("span", "calendar-date", entry.date.slice(8)));
    cell.append(element("span", "calendar-token", hasData ? `${formatCompact(tokens)} token` : "无数据"));
    cell.addEventListener("click", () => {
      activitySelection = entry.date;
      renderActivity(latestState);
    });
    heatmap.append(cell);
  });
  calendar.append(heatmap);
  activityRoot.append(calendar);

  const detail = element("section", "calendar-detail");
  detail.append(element("h3", null, activitySelection));
  detail.append(element("p", null, usage.has(activitySelection)
    ? `${formatNumber(usage.get(activitySelection))} token`
    : "无数据"));
  activityRoot.append(detail);
  activityRoot.append(
    element("p", "view-note", "缺失日期与 0 token 分开显示；日用量仅展示 Codex 返回的数据，不补造历史。"),
  );
}

function requestErrorMessage(error, fallback = "请求失败") {
  const status = Number(error && error.status);
  if (status === 401 || status === 403) return "需要管理权限；请打开高级设置解锁控制 token";
  if (status === 400) return "请求格式或配对码无效";
  if (status === 404) return "设备发现已过期或不存在";
  if (status === 409) return "网络不兼容或握手失败";
  if (status === 503) return "设备当前不可用";
  if (status === 504) return "设备响应超时";
  if (error && error.name === "AbortError") return "请求超时";
  return (error && error.message) || fallback;
}

function devicePageSize() {
  return document.documentElement.dataset.theme === "eink" && window.innerWidth < 768
    ? EINK_DEVICE_PAGE_SIZE
    : Number.POSITIVE_INFINITY;
}

function sliceDevicePage(items) {
  const size = devicePageSize();
  if (!Number.isFinite(size)) return items;
  return items.slice(devicePage * size, (devicePage + 1) * size);
}

function deviceIsExpired(device) {
  const expiresAt = Number(device && device.expiresAt);
  return Number.isFinite(expiresAt) && expiresAt > 0 && expiresAt <= Math.floor(Date.now() / 1000);
}

function deviceOs(device) {
  return device && device.os ? device.os : "系统未知";
}

function renderDeviceItem(device, nearby = false) {
  const item = element("article", "device-item");
  const heading = element("div", "device-heading");
  const name = device && (device.alias || device.nodeId) ? device.alias || device.nodeId : "未知设备";
  heading.append(element("h4", "device-name", name));
  const state = nearby ? device.pairingState || "discovered" : device.status || "unknown";
  heading.append(statusPill(state, nearby ? DISCOVERY_STATUS_LABELS : CONNECTED_DEVICE_STATUS_LABELS));
  item.append(heading);
  item.append(element("p", "meta device-meta", `${deviceOs(device)} · ${formatTime(device && (device.lastSeen || device.lastSnapshotAt))}`));

  if (!nearby) return item;

  const expired = deviceIsExpired(device);
  const incompatible = device && device.networkCompatible === false;
  if (incompatible) item.append(element("p", "device-warning", "网络不兼容，不可配对"));
  else if (expired) item.append(element("p", "device-warning", "发现已过期，请刷新"));
  else if (device && device.reason) item.append(element("p", "meta device-reason", device.reason));

  const pairingState = String(device && device.pairingState);
  const alreadyConnected = ["connected", "paired"].includes(pairingState);
  if (alreadyConnected) return item;

  const controls = element("div", "device-controls");
  const pairingCode = element("input", "pairing-code");
  pairingCode.type = "text";
  pairingCode.maxLength = 8;
  pairingCode.minLength = 8;
  pairingCode.pattern = "[A-Za-z0-9]{8}";
  pairingCode.autocomplete = "off";
  pairingCode.setAttribute("autocapitalize", "off");
  pairingCode.setAttribute("autocorrect", "off");
  pairingCode.spellcheck = false;
  pairingCode.placeholder = "8 位配对码";
  pairingCode.setAttribute("aria-label", `${name}配对码`);
  pairingCode.title = "恰好 8 位数字或大小写英文字母，区分大小写";
  const pair = element("button", "small-button", "配对");
  pair.type = "button";
  pair.disabled = Boolean(incompatible || expired || alreadyConnected || !device || !device.nodeId);
  if (pairingState === "paired") pair.textContent = "已配对";
  else if (pairingState === "connected") pair.textContent = "已连接";
  pair.addEventListener("click", () => pairDevice(device, pairingCode, pair));
  controls.append(pairingCode, pair);
  item.append(controls);
  return item;
}

function renderDevicePager(pageCount) {
  if (!devicePager) return;
  if (!Number.isFinite(pageCount) || pageCount <= 1) {
    devicePager.hidden = true;
    devicePager.replaceChildren();
    return;
  }
  devicePager.hidden = false;
  devicePager.replaceChildren();
  const previous = element("button", "pager-button", "上一页");
  previous.type = "button";
  previous.disabled = devicePage <= 0;
  previous.addEventListener("click", () => {
    devicePage -= 1;
    renderDeviceSettings();
  });
  const next = element("button", "pager-button", "下一页");
  next.type = "button";
  next.disabled = devicePage >= pageCount - 1;
  next.addEventListener("click", () => {
    devicePage += 1;
    renderDeviceSettings();
  });
  devicePager.append(previous, element("span", "pager-label", `设备 ${devicePage + 1} / ${pageCount}`), next);
}

function renderDeviceSettings() {
  if (!settingsDeviceVisible() || !latestState || pendingPairNodeIds.size || deviceListInteractionActive()) return;
  const currentIdentity = liveEndpointIdentity || (currentEndpointOnline ? latestState.identity : null);
  const identity = currentIdentity || {};
  if (identityAliasInput && document.activeElement !== identityAliasInput) {
    identityAliasInput.value = identity.alias || "";
  }
  if (identityOs) identityOs.textContent = identity.os || "系统未知";

  const peers = Array.isArray(latestState.peers) ? latestState.peers : [];
  const discovered = discoveryState && Array.isArray(discoveryState.devices) ? discoveryState.devices : [];
  const ownNodeId = currentIdentity && currentIdentity.nodeId;
  const otherPeers = otherPeerDevices(peers, ownNodeId);
  const otherDiscovered = otherPeerDevices(discovered, ownNodeId);
  const pageSize = devicePageSize();
  const pageCount = Number.isFinite(pageSize)
    ? Math.max(1, Math.ceil(Math.max(otherPeers.length, otherDiscovered.length) / pageSize))
    : 1;
  devicePage = Math.min(devicePage, pageCount - 1);
  if (connectedDevicesRoot) {
    connectedDevicesRoot.replaceChildren();
    const current = element("article", "device-item current-device");
    const heading = element("div", "device-heading");
    heading.append(
      element("h4", "device-name", identity.alias || (identity.nodeId ? identity.nodeId : "当前入口")),
      statusPill(currentEndpointOnline ? "connected" : "unknown", CONNECTED_DEVICE_STATUS_LABELS),
    );
    current.append(heading);
    current.append(element("p", "meta device-meta", identity.os || "当前节点信息尚未确认"));
    current.append(element("p", "meta device-endpoint", `当前连接地址：${endpointBase}`));
    if (!currentEndpointOnline) {
      current.append(element("p", "meta device-reason", "连接状态未确认；这里不会用离线缓存推断当前节点身份。"));
    }
    connectedDevicesRoot.append(current);
    const pagePeers = sliceDevicePage(otherPeers);
    if (pagePeers.length) pagePeers.forEach((peer) => connectedDevicesRoot.append(renderDeviceItem(peer)));
    else connectedDevicesRoot.append(element("p", "empty compact", "暂无其他已连接设备"));
  }
  if (discoveriesRoot) {
    discoveriesRoot.replaceChildren();
    if (!discoveryState) {
      discoveriesRoot.append(element("p", "empty compact", "正在读取发现…"));
    } else if (discoveryState.available === false) {
      discoveriesRoot.append(element("p", "empty compact", discoveryState.reason || "附近设备发现不可用"));
    } else {
      const pageDevices = sliceDevicePage(otherDiscovered);
      if (pageDevices.length) pageDevices.forEach((device) => discoveriesRoot.append(renderDeviceItem(device, true)));
      else discoveriesRoot.append(element("p", "empty compact", "当前没有发现其他可配对节点"));
    }
  }
  renderDevicePager(pageCount);
}

function otherPeerDevices(devices, ownNodeId) {
  const seen = new Set();
  return (Array.isArray(devices) ? devices : []).filter((device) => {
    if (!device || !device.nodeId || device.nodeId === ownNodeId || seen.has(device.nodeId)) return false;
    seen.add(device.nodeId);
    return true;
  });
}

function normalizePairingCode(value) {
  return String(value || "").trim();
}

function validPairingCode(value) {
  return /^[A-Za-z0-9]{8}$/.test(value);
}

function settingsDeviceVisible() {
  return currentSettingsTab === "device" && settingsBackdrop && !settingsBackdrop.hidden;
}

function deviceListInteractionActive() {
  const active = document.activeElement;
  return Boolean(
    active &&
      ((discoveriesRoot && discoveriesRoot.contains(active)) ||
        (connectedDevicesRoot && connectedDevicesRoot.contains(active))),
  );
}

async function apiRequest(path, options = {}) {
  const controller = new AbortController();
  const timeoutMs = Number.isFinite(Number(options.timeoutMs))
    ? Math.max(0, Number(options.timeoutMs))
    : REQUEST_TIMEOUT_MS;
  const timeout = window.setTimeout(() => controller.abort(), timeoutMs);
  const token = options.token === undefined ? clientToken : options.token;
  const headers = { ...readRequestHeaders(token) };
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  try {
    const response = await fetch(endpointUrl(path), {
      method: options.method || "GET",
      cache: "no-store",
      headers,
      signal: controller.signal,
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
    });
    const payload = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(payload.message || payload.error || `HTTP ${response.status}`);
      error.status = response.status;
      error.code = payload.code;
      throw error;
    }
    return payload;
  } finally {
    window.clearTimeout(timeout);
  }
}

async function loadDeviceIdentity() {
  if (identityLoading) return;
  const requestEndpointGeneration = endpointGeneration;
  const requestGeneration = ++identityRequestGeneration;
  identityLoading = true;
  try {
    const identity = await apiRequest("/api/v1/identity");
    if (requestEndpointGeneration !== endpointGeneration || requestGeneration !== identityRequestGeneration) return;
    if (identity && typeof identity === "object") {
      liveEndpointIdentity = identity;
      currentEndpointOnline = true;
      latestState = { ...(latestState || emptyState()), identity };
      renderDeviceSettings();
    }
  } catch (error) {
    if (requestEndpointGeneration === endpointGeneration && requestGeneration === identityRequestGeneration && identityStatus) {
      identityStatus.textContent = requestErrorMessage(error, "当前节点信息读取失败");
    }
  } finally {
    if (requestGeneration === identityRequestGeneration) identityLoading = false;
  }
}

function hideLocalPairingCode() {
  pairingCodeRequestGeneration += 1;
  if (pairingCodePanel) pairingCodePanel.hidden = true;
  if (pairingCodeValue) pairingCodeValue.textContent = "";
  if (pairingCodeToggle) {
    pairingCodeToggle.hidden = false;
    pairingCodeToggle.disabled = false;
    pairingCodeToggle.textContent = "生成新的配对码";
  }
  if (pairingCodeRefresh) pairingCodeRefresh.disabled = false;
}

async function showLocalPairingCode() {
  if (!controlToken) {
    if (identityStatus) identityStatus.textContent = "需要管理权限；请打开高级设置解锁控制 token";
    setSettingsTab("advanced");
    return;
  }
  if (!pairingCodeToggle) return;
  const requestGeneration = ++pairingCodeRequestGeneration;
  pairingCodeToggle.disabled = true;
  if (pairingCodeRefresh) pairingCodeRefresh.disabled = true;
  if (pairingCodeValue) pairingCodeValue.textContent = "生成中…";
  pairingCodeToggle.textContent = "生成中…";
  try {
    const result = await apiRequest("/api/v1/pairing-code", { token: controlToken });
    if (requestGeneration !== pairingCodeRequestGeneration || !settingsDeviceVisible()) return;
    if (!result || typeof result.pairingCode !== "string" || !result.pairingCode) {
      throw new Error("后端未返回配对码");
    }
    pairingCodeValue.textContent = result.pairingCode;
    pairingCodePanel.hidden = false;
    pairingCodeToggle.disabled = false;
    pairingCodeToggle.hidden = true;
    if (pairingCodeRefresh) pairingCodeRefresh.disabled = false;
  } catch (error) {
    if (requestGeneration !== pairingCodeRequestGeneration || !settingsDeviceVisible()) return;
    if (identityStatus) identityStatus.textContent = requestErrorMessage(error, "配对码读取失败");
    if (pairingCodeValue) pairingCodeValue.textContent = "";
    if (pairingCodePanel) pairingCodePanel.hidden = true;
    pairingCodeToggle.disabled = false;
    pairingCodeToggle.hidden = false;
    pairingCodeToggle.textContent = "生成新的配对码";
    if (pairingCodeRefresh) pairingCodeRefresh.disabled = false;
  }
}

async function loadDiscoveries() {
  if (discoveryLoading || !settingsDeviceVisible()) return;
  const requestEndpointGeneration = endpointGeneration;
  const requestGeneration = ++discoveryRequestGeneration;
  discoveryLoading = true;
  if (discoveriesStatus) discoveriesStatus.textContent = "正在读取发现…";
  try {
    const result = await apiRequest("/api/v1/discoveries");
    if (requestEndpointGeneration !== endpointGeneration || requestGeneration !== discoveryRequestGeneration) return;
    discoveryState = result && typeof result === "object" ? result : { available: false, reason: "响应格式无效", devices: [] };
    const count = Array.isArray(discoveryState.devices) ? discoveryState.devices.length : 0;
    if (discoveriesStatus) discoveriesStatus.textContent = discoveryState.available === false
      ? (discoveryState.reason || "附近设备发现不可用")
      : `已发现 ${count} 台其他电脑`;
    devicePage = 0;
    renderDeviceSettings();
  } catch (error) {
    if (requestEndpointGeneration !== endpointGeneration || requestGeneration !== discoveryRequestGeneration) return;
    discoveryState = { available: false, reason: requestErrorMessage(error, "附近设备读取失败"), devices: [] };
    if (discoveriesStatus) discoveriesStatus.textContent = discoveryState.reason;
    renderDeviceSettings();
  } finally {
    if (requestGeneration === discoveryRequestGeneration) discoveryLoading = false;
  }
}

async function pairDevice(device, pairingCodeInput, button) {
  if (!controlToken) {
    if (discoveriesStatus) discoveriesStatus.textContent = "需要管理权限；请打开高级设置解锁控制 token";
    setSettingsTab("advanced");
    return;
  }
  const pairingCode = normalizePairingCode(pairingCodeInput.value);
  if (!validPairingCode(pairingCode)) {
    pairingCodeInput.setCustomValidity("配对码必须恰好 8 位，使用数字及大小写英文字母，且区分大小写");
    pairingCodeInput.reportValidity();
    return;
  }
  pairingCodeInput.setCustomValidity("");
  button.disabled = true;
  button.textContent = "配对中…";
  pendingPairNodeIds.add(String(device.nodeId));
  try {
    const result = await apiRequest("/api/v1/pair", {
      method: "POST",
      token: controlToken,
      timeoutMs: 25_000,
      body: { nodeId: device.nodeId, pairingCode },
    });
    if (!result || result.nodeId !== device.nodeId || result.status !== "connected") {
      throw new Error("后端尚未确认设备已连接");
    }
    pendingPairNodeIds.delete(String(device.nodeId));
    button.textContent = "已确认连接";
    button.blur();
    await loadDiscoveries();
    await refresh();
  } catch (error) {
    pendingPairNodeIds.delete(String(device.nodeId));
    button.textContent = error && error.name === "AbortError"
      ? "响应超时，结果未确认"
      : requestErrorMessage(error, "配对失败");
    if (error && error.name === "AbortError") {
      button.blur();
      await refresh();
      await loadDiscoveries();
    }
    window.setTimeout(() => {
      if (button.isConnected) {
        button.textContent = "配对";
        button.disabled = false;
      }
    }, 2_500);
  }
}

function aliasByteLength(value) {
  if (typeof TextEncoder === "function") return new TextEncoder().encode(value).length;
  return unescape(encodeURIComponent(value)).length;
}

function validAlias(value) {
  const alias = String(value || "").trim();
  return alias.length > 0 && aliasByteLength(alias) <= 64 ? alias : null;
}

async function saveIdentity(event) {
  event.preventDefault();
  const alias = validAlias(identityAliasInput && identityAliasInput.value);
  if (!alias) {
    if (identityAliasInput) {
      identityAliasInput.setCustomValidity("昵称需为 1–64 字节");
      identityAliasInput.reportValidity();
    }
    return;
  }
  if (!controlToken) {
    if (identityStatus) identityStatus.textContent = "需要管理权限；请打开高级设置解锁控制 token";
    setSettingsTab("advanced");
    return;
  }
  identityAliasInput.setCustomValidity("");
  const button = identityForm.querySelector('button[type="submit"]');
  const requestEndpointGeneration = endpointGeneration;
  button.disabled = true;
  button.textContent = "保存中…";
  try {
    const result = await apiRequest("/api/v1/identity", {
      method: "PATCH",
      token: controlToken,
      body: { alias },
    });
    if (requestEndpointGeneration !== endpointGeneration) return;
    const identity = result && result.identity ? result.identity : result;
    if (!identity || identity.alias !== alias) throw new Error("后端未确认昵称");
    liveEndpointIdentity = { ...(liveEndpointIdentity || {}), ...identity };
    currentEndpointOnline = true;
    latestState = { ...(latestState || emptyState()), identity: { ...(latestState && latestState.identity), ...identity } };
    if (identityStatus) identityStatus.textContent = "已保存";
    renderDeviceSettings();
  } catch (error) {
    if (requestEndpointGeneration === endpointGeneration && identityStatus) identityStatus.textContent = requestErrorMessage(error, "保存失败");
  } finally {
    button.disabled = false;
    button.textContent = "保存昵称";
  }
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
  card.append(element("p", "meta host-secondary", `采集：${collector} · 运行中 ${activeCount} · 最近快照 ${formatTime(host.lastSnapshotAt || host.lastSeen)}`));
  if (host.error) card.append(element("p", "error", host.error));
  const totalPages = Math.max(1, Math.ceil(sessions.length / sessionPageSize()));
  const currentPage = Math.min(page, totalPages - 1);
  const list = element("div", "sessions");
  sessions
    .slice(currentPage * sessionPageSize(), (currentPage + 1) * sessionPageSize())
    .forEach((session) => list.append(renderSession(session, host.hostId)));
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
  stateCursor = Number.isFinite(Number(state.cursor))
    ? Number(state.cursor)
    : Number.isFinite(Number(state.revision))
      ? Number(state.revision)
      : null;
  const accounts = Array.isArray(state.accounts) ? state.accounts : [];
  const hosts = Array.isArray(state.hosts) ? state.hosts : [];
  accountCount.textContent = String(accounts.length);
  hostCount.textContent = String(hosts.length);
  revision.textContent = `revision ${state.revision ?? "—"}`;
  renderOverview(state);
  renderAccounts(state);
  renderActivity(state);
  renderHosts(state);
  if (settingsDeviceVisible()) renderDeviceSettings();
  switchView(currentView);
}

async function readStateFrom(base, cursor) {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const stateEndpoint = cursor === null
      ? `${base}/api/v1/state`
      : `${base}/api/v1/state?since=${encodeURIComponent(cursor)}`;
    const headers = readRequestHeaders(clientToken);
    let response = await fetch(stateEndpoint, {
      cache: "no-store",
      headers,
      signal: controller.signal,
    });
    if (response.status === 404) {
      // Keep a PWA built during the migration usable against an older node.
      response = await fetch(`${base}/api/state`, {
        cache: "no-store",
        headers,
        signal: controller.signal,
      });
    }
    if (response.status === 304) return { state: null, endpoint: base };
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return { state: await response.json(), endpoint: base };
  } finally {
    window.clearTimeout(timeout);
  }
}

async function readState() {
  let lastError = null;
  for (const candidate of knownEndpoints.slice(0, MAX_FAILOVER_ENDPOINTS)) {
    try {
      return await readStateFrom(candidate, candidate === endpointBase ? stateCursor : null);
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError || new Error("没有可用的 Monitor 节点");
}

async function refresh() {
  if (loading) return;
  const requestGeneration = endpointGeneration;
  loading = true;
  refreshButton.disabled = true;
  try {
    const result = await readState();
    if (requestGeneration !== endpointGeneration) return;
    const switchedEndpoint = result.endpoint !== endpointBase;
    if (switchedEndpoint) {
      hideLocalPairingCode();
      endpointBase = result.endpoint;
      endpointGeneration += 1;
      liveEndpointIdentity = null;
      currentEndpointOnline = false;
      discoveryState = null;
      identityRequestGeneration += 1;
      discoveryRequestGeneration += 1;
      identityLoading = false;
      discoveryLoading = false;
      stateCursor = null;
      writeStorage(ENDPOINT_STORAGE_KEY, endpointBase);
    }
    const state = result.state;
    if (state) {
      const stateCacheKey = endpointCacheKey();
      showingCachedState = false;
      cachedStateSource = null;
      cachedAt = null;
      currentEndpointOnline = true;
      if (state.identity && typeof state.identity === "object") liveEndpointIdentity = state.identity;
      rememberPeerEndpoints(state);
      render(state);
      await writeCachedState(state, stateCacheKey);
      if (switchedEndpoint && settingsDeviceVisible()) {
        loadDeviceIdentity();
        loadDiscoveries();
      }
    } else if (showingCachedState && cachedStateSource) {
      currentEndpointOnline = true;
      showingCachedState = false;
      render(cachedStateSource);
      cachedStateSource = null;
    }
    const current = latestState || {};
    const hostTotal = Array.isArray(current.hosts) ? current.hosts.length : 0;
    setConnection(
      "connected",
      "已连接",
      `${hostTotal} 台主机 · ${switchedEndpoint ? `已切换到 ${endpointBase}` : state ? `最近快照 ${formatTime(latestSnapshotTime(current))}` : "数据未变化"}`,
    );
  } catch (error) {
    if (requestGeneration !== endpointGeneration) return;
    currentEndpointOnline = false;
    if (latestState && !showingCachedState) {
      cachedAt = Number(latestState.generatedAt) || Math.floor(Date.now() / 1000);
      cachedStateSource = latestState;
      showingCachedState = true;
      render(staleCachedState(latestState));
    }
    const message = error && error.name === "AbortError" ? "请求超时" : error.message || "网络错误";
    setConnection(
      "error",
      showingCachedState ? "离线缓存" : "连接中断",
      `${showingCachedState ? "最后成功" : "状态可能过期"} · ${formatTime(currentCacheTime())} · ${message}`,
    );
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
window.addEventListener("resize", () => {
  if (latestState) {
    renderHosts(latestState);
    if (settingsDeviceVisible()) renderDeviceSettings();
  }
});
if (settingsToggle) settingsToggle.addEventListener("click", () => setSettingsOpen(settingsBackdrop.hidden));
if (settingsClose) settingsClose.addEventListener("click", () => setSettingsOpen(false));
if (settingsBackdrop) settingsBackdrop.addEventListener("click", (event) => {
  if (event.target === settingsBackdrop) setSettingsOpen(false);
});
if (settingsTabs.length) settingsTabs.forEach((button) => {
  button.addEventListener("click", () => setSettingsTab(button.dataset.settingsTab));
});
if (themeSelect) themeSelect.addEventListener("change", () => {
  applyTheme(themeSelect.value);
  if (latestState) render(latestState);
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && settingsBackdrop && !settingsBackdrop.hidden) {
    event.preventDefault();
    setSettingsOpen(false);
    return;
  }
  trapSettingsFocus(event);
});

function currentCacheTime() {
  return cachedAt || (latestState && latestState.generatedAt) || 0;
}

function emptyState() {
  return { apiVersion: 1, generatedAt: 0, revision: 0, cursor: null, hosts: [], accounts: [], peers: [] };
}

async function applyEndpoint(value) {
  const normalized = normalizeEndpoint(value);
  if (!normalized) {
    if (endpointInput) {
      endpointInput.setCustomValidity("请输入 http:// 或 https:// 地址");
      endpointInput.reportValidity();
    }
    return;
  }
  if (endpointInput) endpointInput.setCustomValidity("");
  endpointBase = normalized;
  rememberEndpoints([normalized]);
  endpointGeneration += 1;
  if (endpointInput) endpointInput.value = endpointBase;
  clientToken = tokenInput ? tokenInput.value.trim() : clientToken;
  controlToken = controlTokenInput ? controlTokenInput.value.trim() : controlToken;
  writeStorage(ENDPOINT_STORAGE_KEY, endpointBase);
  writeStorage(TOKEN_STORAGE_KEY, clientToken);
  writeStorage(CONTROL_TOKEN_STORAGE_KEY, controlToken);
  stateCursor = null;
  latestState = null;
  currentEndpointOnline = false;
  liveEndpointIdentity = null;
  discoveryState = null;
  identityRequestGeneration += 1;
  discoveryRequestGeneration += 1;
  identityLoading = false;
  discoveryLoading = false;
  cachedStateSource = null;
  cachedAt = null;
  showingCachedState = false;
  accountPage = 0;
  hostPage = 0;
  sessionPage = 0;
  activityMonth = localMonthKey();
  activitySelection = localDateKey();
  activityAccountKey = "";
  window.clearTimeout(nextRefresh);
  render(emptyState());
  setConnection("error", "切换节点", endpointBase);
  if (settingsDeviceVisible()) {
    loadDeviceIdentity();
    loadDiscoveries();
  }
  await bootstrap();
}

async function bootstrap() {
  const cached = await readCachedState();
  if (cached) {
    cachedAt = cached.savedAt;
    showingCachedState = true;
    cachedStateSource = cached.state;
    render(staleCachedState(cached.state));
    setConnection("error", "离线缓存", `最后成功 · ${formatTime(cachedAt)}`);
  }
  refresh();
}

if (endpointInput) endpointInput.value = endpointBase;
if (tokenInput) tokenInput.value = clientToken;
if (controlTokenInput) controlTokenInput.value = controlToken;
renderAccentOptions();
initializeAndroidShellSettings();
applyTheme(readStorage(THEME_STORAGE_KEY), false);
applyAccent(readStorage(ACCENT_STORAGE_KEY), false);
if (endpointForm && endpointInput) {
  endpointForm.addEventListener("submit", (event) => {
    event.preventDefault();
    applyEndpoint(endpointInput.value);
  });
}
if (endpointReset) endpointReset.addEventListener("click", () => applyEndpoint(window.location.origin));
if (identityForm) identityForm.addEventListener("submit", saveIdentity);
if (pairingCodeToggle) pairingCodeToggle.addEventListener("click", showLocalPairingCode);
if (pairingCodeRefresh) pairingCodeRefresh.addEventListener("click", showLocalPairingCode);
if (pairingCodeHide) pairingCodeHide.addEventListener("click", hideLocalPairingCode);
if (discoveriesRefresh) discoveriesRefresh.addEventListener("click", () => {
  discoveryState = null;
  devicePage = 0;
  loadDiscoveries();
});

bootstrap();

const offlineAvailabilityNote = document.getElementById("offline-availability-note");
if (offlineAvailabilityNote) offlineAvailabilityNote.hidden = window.isSecureContext === true;

if (window.isSecureContext && "serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(() => {
    // Offline shell is optional; the monitor remains usable without it.
  });
}
