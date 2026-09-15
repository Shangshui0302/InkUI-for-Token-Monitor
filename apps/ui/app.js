const root = document.documentElement;
const status = document.querySelector("#runtime-status");
const dots = document.querySelector(".dots");

for (let index = 0; index < 24; index += 1) {
  const dot = document.createElement("i");
  if (index < 13) dot.className = "filled";
  dots.append(dot);
}

document.querySelector("#theme-button").addEventListener("click", () => {
  root.dataset.theme = root.dataset.theme === "eink" ? "light" : "eink";
});

const navigation = document.querySelector("nav");
navigation.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-target]");
  if (!button) return;
  const target = button.dataset.target;
  for (const item of navigation.querySelectorAll("button[data-target]")) {
    const active = item === button;
    item.classList.toggle("active", active);
    if (active) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  }
  for (const panel of document.querySelectorAll("[data-panel]")) {
    panel.hidden = panel.dataset.panel !== target;
  }
});

const dialog = document.querySelector("#approval-dialog");
document.querySelector("#dialog-button").addEventListener("click", () => dialog.showModal());

document.querySelector("#websocket-button").addEventListener("click", () => {
  const websocketStatus = document.querySelector("#websocket-status");
  websocketStatus.textContent = "连接中…";
  const socket = new WebSocket("ws://127.0.0.1:18789");
  const timeout = setTimeout(() => {
    websocketStatus.textContent = "连接超时";
    socket.close();
  }, 3000);
  socket.addEventListener("open", () => socket.send("nekodex-p0"));
  socket.addEventListener("message", (event) => {
    clearTimeout(timeout);
    websocketStatus.textContent = event.data === "nekodex-p0" ? "WebSocket 正常" : "回显内容不匹配";
    socket.close();
  });
  socket.addEventListener("error", () => {
    clearTimeout(timeout);
    websocketStatus.textContent = "WebSocket 连接失败";
  });
});

document.querySelector("#appserver-button").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  const appserverStatus = document.querySelector("#appserver-status");
  const invoke = window.__TAURI_INTERNALS__?.invoke;
  if (!invoke) {
    appserverStatus.textContent = "浏览器预览不运行 App Server";
    return;
  }
  button.disabled = true;
  appserverStatus.textContent = "正在运行临时生命周期验证…";
  try {
    const result = await invoke("p0_appserver_lifecycle");
    const passed = result.approvalSeen && result.interruptAccepted && result.status === "interrupted";
    appserverStatus.textContent = passed
      ? "创建、发送、审批、停止已通过"
      : `验证结果不完整：${JSON.stringify(result)}`;
  } catch (error) {
    appserverStatus.textContent = String(error);
  } finally {
    button.disabled = false;
  }
});

async function checkRuntime() {
  try {
    const invoke = window.__TAURI_INTERNALS__?.invoke;
    if (!invoke) throw new Error("浏览器预览：IPC 未启用");
    const environment = await invoke("p0_environment");
    status.textContent = `${environment.platform} · 本地资源 · IPC 正常`;
    status.dataset.ok = "true";
  } catch (error) {
    status.textContent = error.message;
  }
}

checkRuntime();
