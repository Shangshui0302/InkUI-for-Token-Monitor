import React, { FormEvent, ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

declare global {
  interface Window {
    __TAURI_INTERNALS__?: { invoke: Invoke };
  }
}

type Panel = "overview" | "quota" | "activity" | "threads";
const ACTIVITY_PAGE_SIZE = 100;
const ACTIVITY_MAX_ITEMS = 500;
const DETAIL_TURN_PAGE_SIZE = 20;
const DETAIL_MAX_TURNS = 100;
const DETAIL_ITEMS_PER_TURN = 30;
const PROJECT_MAX_ITEMS = 500;
const THREAD_PAGE_SIZE = 50;
const THREAD_MAX_ITEMS = 500;
const APPROVAL_PAGE_SIZE = 100;
const APPROVAL_MAX_PAGES = 20;
const APPROVAL_MAX_ITEMS = 2_000;
const APPROVAL_MAX_RENDERED = 500;
const NEARBY_MAX_ITEMS = 128;
const NEARBY_HINT_MAX_CHARS = 128;
const tokenCount = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 0 });
type DeviceState = {
  phase: "cold" | "unjoined" | "joined" | string;
  codexCapable?: boolean;
  groupName?: string;
  deviceName?: string;
  deviceId?: string;
  backgroundError?: string | null;
};
type NetworkState = { listening: boolean; port?: number; onlinePeers: string[]; peerStatuses?: Record<string, MemberStatus>; recoveryWarnings?: Record<string, string> };
type LocalSourceState = { capable: boolean; ready: boolean; mode?: string | null; error?: string | null };
type MemberStatus = "offline" | "connecting" | "online" | "authenticationFailed" | "incompatible";
type Member = { deviceId: string; deviceName?: string | null; codexCapable?: boolean | null; protocolVersion?: number | null; businessProtocol?: { major: number; minor: number } | null; capabilities?: string[]; local: boolean; online: boolean; status?: MemberStatus };
type MemberList = { items: Member[]; total: number; truncated: boolean };
type PendingJoinRequest = { requestId: string; deviceId: string; deviceName: string; codexCapable: boolean; protocolVersion: number };
type NearbyDevice = { deviceHint?: string | null; address: string };
type Project = { project_id: string; name: string; path: string; available_models?: string[] };
type ProjectPage = { items: Project[]; next?: number | null };
type BusinessResponse<T = unknown> = { result?: T; error?: unknown };
type CachedBusinessResponse<T> = { payload: BusinessResponse<T>; observedAt: number };
type ReadResult<T> = { value: T; cachedAt: number | null };
type ThreadSummary = { id: string; name?: string; preview?: string; status?: { type?: string; activeFlags?: string[] }; managed?: boolean };
type ThreadPage = { items?: ThreadSummary[] | null; next?: number | null };
type ThreadItem = {
  id?: string;
  type?: string;
  text?: string;
  content?: Array<{ type?: string; text?: string }>;
  summary?: string[];
  command?: string;
  aggregatedOutput?: string | null;
  status?: string;
};
type ThreadTurn = {
  id?: string;
  status?: string;
  itemsView?: string;
  items?: ThreadItem[];
};
type ThreadDetailResponse = {
  truncated?: boolean;
  incomplete?: boolean;
  next_before_turn_id?: string | null;
  thread?: {
    id?: string;
    preview?: string;
    status?: { type?: string } | string;
    turns?: ThreadTurn[];
  };
};
type Approval = {
  approval_id: string;
  description: string;
  thread: { device_id: string; thread_id: string };
  source_process_generation: number;
  status: string;
};
type ApprovalPage = { items?: Approval[] | null; next?: number | null; incomplete?: boolean };
type MonitoredApproval = {
  target: string;
  approval: Approval;
  cachedAt: number | null;
  unavailable: boolean;
};
type SourceSnapshot = {
  source_device_id?: string;
  source_mode?: "sharedDaemon" | "isolatedFallback" | string;
  process_generation?: number;
  observed_at_unix_seconds: number;
  quota?: {
    account_id: string;
    source_device_id: string;
    observed_at_unix_seconds: number;
    windows: Array<{
      name: string;
      remaining_percent?: number | null;
      resets_at_unix_seconds?: number | null;
    }>;
  } | null;
  thread_counts?: {
    running: number;
    attention: number;
    total: number;
    truncated: boolean;
    available: boolean;
  };
  raw_usage?: { dailyUsageBuckets?: Array<{ startDate: string; tokens: number }> };
  errors?: { quota?: string | null; usage?: string | null; threads?: string | null };
};
type GroupSourceSnapshot = { target: string; value: SourceSnapshot; cachedAt: number | null };
type GroupDailyUsage = {
  accountId: string;
  sourceDeviceId: string;
  startDate: string;
  tokens: number;
  cachedAt: number | null;
  sourceMode?: string;
};
type Activity = {
  activity_id: string;
  source_device_id: string;
  thread: { thread_id: string };
  kind: string;
  message?: string | null;
  occurred_at_unix_seconds: number;
};
type EventEnvelope = {
  source_device_id: string;
  process_generation: number;
  cursor: number;
  event: {
    ActivitySnapshot?: { activities: Activity[] };
    ApprovalChanged?: Approval;
  };
};

const projectActivities = (events: EventEnvelope[]) => {
  const activityById = new Map<string, Activity>();
  for (const envelope of events) {
    for (const activity of envelope.event.ActivitySnapshot?.activities || []) {
      if (activity.source_device_id !== envelope.source_device_id) continue;
      const key = `${activity.source_device_id}:${activity.activity_id}`;
      const known = activityById.get(key);
      if (!known || activity.occurred_at_unix_seconds >= known.occurred_at_unix_seconds) {
        activityById.set(key, activity);
      }
    }
  }
  return [...activityById.values()].sort(
    (left, right) => right.occurred_at_unix_seconds - left.occurred_at_unix_seconds,
  );
};

const sameActivities = (left: Activity[], right: Activity[]) =>
  left.length === right.length && left.every((activity, index) => {
    const other = right[index];
    return activity.activity_id === other.activity_id
      && activity.source_device_id === other.source_device_id
      && activity.occurred_at_unix_seconds === other.occurred_at_unix_seconds
      && activity.kind === other.kind
      && activity.message === other.message
      && activity.thread.thread_id === other.thread.thread_id;
  });

const eventIdentity = (event: EventEnvelope) =>
  `${event.source_device_id}:${event.process_generation}:${event.cursor}`;

const approvalEventKey = (events: EventEnvelope[]) => events
  .filter((event) => event.event.ApprovalChanged != null)
  .map(eventIdentity)
  .join(";");

const threadEventKey = (events: EventEnvelope[], target: string, threadId: string) => events
  .filter((event) => event.source_device_id === target && (event.event.ActivitySnapshot?.activities || [])
    .some((activity) => activity.thread.thread_id === threadId))
  .map(eventIdentity)
  .join(";");

const sourceActivityEventKey = (events: EventEnvelope[], target: string) => events
  .filter((event) => event.source_device_id === target && event.event.ActivitySnapshot != null)
  .map(eventIdentity)
  .join(";");

const OPERATION_CAPABILITY = {
  projectsList: "codex.business.operation.projects-list.v1",
  stateSubscribe: "codex.business.operation.state-subscribe.v1",
  eventsReplay: "codex.business.operation.events-replay.v1",
  threadsList: "codex.business.operation.threads-list.v1",
  threadsRead: "codex.business.operation.threads-read.v1",
  approvalsList: "codex.business.operation.approvals-list.v1",
} as const;
type BusinessOperation = keyof typeof OPERATION_CAPABILITY;
type ReadBusinessOperation = "projectsList" | "stateSubscribe" | "threadsList" | "threadsRead" | "approvalsList";
const VISIBLE_MONITOR_POLL_MS = 5_000;

const invoke = <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
  const call = window.__TAURI_INTERNALS__?.invoke;
  if (!call) return Promise.reject(new Error("浏览器预览：IPC 未启用"));
  return call<T>(command, args);
};

const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

const memberStatusLabel = (status?: MemberStatus) => ({
  offline: "离线",
  connecting: "连接中",
  online: "在线",
  authenticationFailed: "身份验证失败",
  incompatible: "协议不兼容",
}[status || "offline"]);

const threadStatusLabel = (status?: { type?: string; activeFlags?: string[] }) => {
  if (status?.activeFlags?.includes("waitingOnApproval")) return "等待审批";
  switch (status?.type) {
    case "active": return "运行中";
    case "idle": return "空闲";
    case "systemError": return "异常";
    case "notLoaded": return "历史记录 · 实时状态不可见";
    default: return "状态未知";
  }
};

class BusinessProtocolError extends Error {
  constructor(readonly payload: unknown) {
    super(protocolErrorKind(payload) === "SourceUnavailable" ? "来源服务不可用" : JSON.stringify(payload));
    this.name = "BusinessProtocolError";
  }
}

const protocolErrorKind = (payload: unknown) => {
  if (typeof payload === "string") return payload;
  if (payload && typeof payload === "object") return Object.keys(payload as Record<string, unknown>)[0] || "";
  return "";
};

const responseValue = <T,>(response: BusinessResponse<T>): T => {
  if (response.error) throw new BusinessProtocolError(response.error);
  return response.result as T;
};

const requestBusiness = <T,>(
  targetDeviceId: string,
  operation: string,
  params: Record<string, unknown>,
  requestId: string | null = null,
) =>
  invoke<BusinessResponse<T>>("p2_request", {
    targetDeviceId,
    operation,
    params,
    requestId,
  });

async function readBusiness<T>(
  targetDeviceId: string,
  operation: string,
  params: Record<string, unknown>,
  preferCache = false,
  allowOnline = true,
): Promise<ReadResult<T>> {
  if (preferCache) {
    const cached = await invoke<CachedBusinessResponse<T> | null>("p2_cached_request", {
      targetDeviceId,
      operation,
      params,
    }).catch(() => null);
    if (cached) return { value: responseValue(cached.payload), cachedAt: cached.observedAt };
  }
  if (!allowOnline) throw new Error("该来源当前没有可用缓存，等待监控能力协商完成。");
  try {
    return { value: responseValue(await requestBusiness<T>(targetDeviceId, operation, params)), cachedAt: null };
  } catch (onlineError) {
    if (onlineError instanceof BusinessProtocolError && protocolErrorKind(onlineError.payload) !== "SourceUnavailable") throw onlineError;
    const cached = await invoke<CachedBusinessResponse<T> | null>("p2_cached_request", {
      targetDeviceId,
      operation,
      params,
    }).catch(() => null);
    if (!cached) throw onlineError;
    return { value: responseValue(cached.payload), cachedAt: cached.observedAt };
  }
}

const staleLabel = (cachedAt: number | null) => cachedAt == null
  ? ""
  : `（显示本机缓存；缓存于 ${new Date(cachedAt * 1000).toLocaleString()}）`;

const boundedText = (value: string, limit = 6000) =>
  value.length <= limit ? value : `${value.slice(0, limit)}\n…内容过长，已截断显示`;

const compactIdentifier = (value: string) =>
  value.length <= 28 ? value : `${value.slice(0, 14)}…${value.slice(-8)}`;

const threadItemPresentation = (item: ThreadItem) => {
  switch (item.type) {
    case "userMessage":
      return {
        label: "用户",
        text: (item.content || [])
          .map((part) => part.type === "text" ? part.text || "" : part.type ? `[${part.type}]` : "")
          .filter(Boolean)
          .join("\n"),
      };
    case "agentMessage": return { label: "Codex", text: item.text || "" };
    case "plan": return { label: "计划", text: item.text || "" };
    case "reasoning": return { label: "推理摘要", text: (item.summary || []).join("\n") };
    case "commandExecution":
      return {
        label: `命令${item.status ? ` · ${item.status}` : ""}`,
        text: [item.command, item.aggregatedOutput].filter(Boolean).join("\n\n"),
      };
    default: return { label: item.type || "未知条目", text: "" };
  }
};

function NodeRow({
  name,
  description,
  state,
  children,
}: {
  name: string;
  description: string;
  state: string;
  children?: ReactNode;
}) {
  return (
    <div className="node">
      <div>
        <strong>{name}</strong>
        <p>{description}</p>
      </div>
      <span>{state}</span>
      {children}
    </div>
  );
}

function DeviceGroup({ onState, observedNetwork, observedMembers }: { onState: (state: DeviceState) => void; observedNetwork: NetworkState; observedMembers: Member[] }) {
  const [state, setState] = useState<DeviceState>({ phase: "cold" });
  const [network, setNetwork] = useState<NetworkState>({ listening: false, onlinePeers: [] });
  const [members, setMembers] = useState<Member[]>([]);
  const [membersTruncated, setMembersTruncated] = useState(false);
  const [membersTotal, setMembersTotal] = useState(0);
  const [pendingJoin, setPendingJoin] = useState<PendingJoinRequest | null>(null);
  const [nearbyDevices, setNearbyDevices] = useState<NearbyDevice[]>([]);
  const [joinAddress, setJoinAddress] = useState("");
  const [invitation, setInvitation] = useState("");
  const [invitationPollingUntil, setInvitationPollingUntil] = useState(0);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState("");

  const applyState = useCallback(
    (next: DeviceState) => {
      invalidateGroupSnapshot();
      setState(next);
      onState(next);
      setError(next.backgroundError || "");
    },
    [onState],
  );

  const refreshNetwork = useCallback(async () => {
    if (state.phase !== "joined") return;
    try {
      const next = await invoke<NetworkState>("p1_network_state");
      setNetwork((current) => JSON.stringify(current) === JSON.stringify(next) ? current : next);
    } catch (caught) {
      setError(message(caught));
    }
  }, [state.phase]);

  const refreshMembers = useCallback(async () => {
    if (state.phase !== "joined") return;
    try {
      const next = await invoke<MemberList>("p1_members");
      setMembers((current) => JSON.stringify(current) === JSON.stringify(next.items) ? current : next.items);
      setMembersTruncated(next.truncated);
      setMembersTotal(next.total);
    } catch (caught) {
      setError(message(caught));
    }
  }, [state.phase]);

  const refreshPendingJoin = useCallback(async () => {
    if (state.phase !== "joined") return;
    try {
      const next = await invoke<PendingJoinRequest | null>("p1_pending_join");
      setPendingJoin((current) => current?.requestId === next?.requestId ? current : next);
    } catch (caught) {
      setError(message(caught));
    }
  }, [state.phase]);

  useEffect(() => {
    if (!window.__TAURI_INTERNALS__?.invoke) {
      applyState({ phase: "preview" });
      return;
    }
    invoke<DeviceState>("p1_initialize").then(applyState).catch((caught) => {
      setState({ phase: "failed" });
      setError(message(caught));
    });
  }, [applyState]);

  useEffect(() => {
    setNetwork((current) => JSON.stringify(current) === JSON.stringify(observedNetwork) ? current : observedNetwork);
  }, [observedNetwork]);

  useEffect(() => {
    setMembers((current) => JSON.stringify(current) === JSON.stringify(observedMembers) ? current : observedMembers);
  }, [observedMembers]);

  useEffect(() => {
    if (state.phase !== "joined" || invitationPollingUntil <= Date.now()) return;
    let polling = false;
    const poll = async () => {
      if (polling || document.visibilityState !== "visible") return;
      if (Date.now() >= invitationPollingUntil) {
        setInvitationPollingUntil(0);
        setInvitation("");
        setPendingJoin(null);
        return;
      }
      polling = true;
      try { await refreshPendingJoin(); }
      finally { polling = false; }
    };
    void poll();
    const pendingTimer = window.setInterval(() => void poll(), 1_000);
    return () => window.clearInterval(pendingTimer);
  }, [state.phase, invitationPollingUntil, refreshPendingJoin]);

  const submitCreate = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    setBusy("create");
    try {
      applyState(
        await invoke<DeviceState>("p1_create_group", {
          groupName: data.get("groupName"),
          deviceName: data.get("deviceName"),
        }),
      );
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy("");
    }
  };

  const submitJoin = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setBusy("join");
    setError("");
    setNotice("已验证加入码后还需邀请设备核对并接受本机身份，请保持页面打开。");
    try {
      const next = await invoke<DeviceState>("p1_join_group", {
        address: data.get("address"),
        shortCode: data.get("shortCode"),
        deviceName: data.get("deviceName"),
      });
      applyState(next);
      setNotice("邀请设备已接受，本机已加入设备组。");
    } catch (caught) {
      setNotice("");
      setError(message(caught));
    } finally {
      setBusy("");
    }
  };

  const discoverNearby = async () => {
    setBusy("discover");
    setError("");
    setNotice("正在局域网中搜索已打开邀请的设备…");
    try {
      const devices = await invoke<NearbyDevice[]>("p1_discover_nearby");
      const bounded = devices
        .filter((device) => device.address.length <= 80)
        .slice(0, NEARBY_MAX_ITEMS)
        .map((device) => ({ ...device, deviceHint: device.deviceHint?.slice(0, NEARBY_HINT_MAX_CHARS) }));
      setNearbyDevices(bounded);
      setNotice(bounded.length
        ? bounded.length >= NEARBY_MAX_ITEMS
          ? `附近设备结果已达到 ${NEARBY_MAX_ITEMS} 条显示上限；请选择目标设备，或直接输入地址。`
          : "请选择邀请设备，然后输入它屏幕上的八位加入码。"
        : "没有发现附近设备。请让已有成员先打开邀请，或继续手工输入地址。");
    } catch (caught) {
      setNotice("");
      setError(message(caught));
    } finally {
      setBusy("");
    }
  };

  const decideJoin = async (accept: boolean) => {
    if (!pendingJoin) return;
    setBusy("join-decision");
    setError("");
    try {
      await invoke("p1_decide_join", { requestId: pendingJoin.requestId, accept });
      setPendingJoin(null);
      setInvitation("");
      setInvitationPollingUntil(0);
      setNotice(accept ? "已接受新设备，正在写入并同步成员名册。" : "已拒绝新设备，本次加入码已作废。");
      if (accept) window.setTimeout(() => void refreshMembers(), 1000);
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy("");
    }
  };

  const createInvitation = async () => {
    setBusy("invite");
    setError("");
    try {
      const value = await invoke<{ offer?: { expires_at?: number }; addresses: string[]; port: number; shortCode: string }>("p1_create_invitation");
      const addresses = value.addresses.length
        ? value.addresses.join(" 或 ")
        : `本机局域网地址:${value.port}`;
      setInvitation(`地址：${addresses}　加入码：${value.shortCode}　10 分钟内有效`);
      setInvitationPollingUntil(value.offer?.expires_at ? value.offer.expires_at * 1_000 : Date.now() + 10 * 60 * 1_000);
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy("");
    }
  };

  const connectManual = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const address = new FormData(event.currentTarget).get("address");
    try {
      const peer = await invoke<string>("p1_connect_peer", { address });
      setError(`已认证连接 ${peer}`);
      await refreshNetwork();
    } catch (caught) {
      setError(message(caught));
    }
  };

  const revoke = async (deviceId: string) => {
    if (!window.confirm(`移除设备 ${deviceId}？该身份不能恢复，只能用新身份重新加入。`)) return;
    try {
      setError("");
      await invoke("p1_revoke_member", { deviceId });
      const next = await invoke<DeviceState>("p1_state");
      applyState(next);
      await Promise.all([refreshMembers(), refreshNetwork()]);
      setNotice("移除已在本机生效；其他设备将在重新连接并同步名册后执行撤销。");
    } catch (caught) {
      setError(message(caught));
    }
  };

  const renameDevice = async () => {
    const deviceName = window.prompt("新的本机名称", state.deviceName || "");
    if (deviceName === null) return;
    setBusy("rename");
    try {
      applyState(await invoke<DeviceState>("p1_rename_device", { deviceName }));
      await refreshMembers();
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy("");
    }
  };

  const exitGroup = async () => {
    if (!window.confirm("退出设备组会删除本机保存的成员关系、项目和监控缓存；设备身份密钥会保留。确定退出？")) return;
    setBusy("exit");
    try {
      applyState(await invoke<DeviceState>("p1_exit_group"));
      setMembers([]);
      setNetwork({ listening: false, onlinePeers: [] });
      setInvitation("");
      setInvitationPollingUntil(0);
      setNotice("");
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy("");
    }
  };

  const phase = state.phase === "joined" ? "已加入" : state.phase === "unjoined" ? "未加入" : state.phase === "preview" ? "浏览器预览" : state.phase === "failed" ? "初始化失败" : "初始化中";
  const description = state.phase === "joined"
    ? state.codexCapable
      ? "本机可提供 Codex 服务，组内成员可直连。"
      : "本机是监控端，不运行 Codex App Server。"
    : state.phase === "preview"
      ? "设备身份与设备组只在原生应用中初始化。"
      : state.phase === "failed"
        ? "系统安全存储或本地数据库不可用。"
        : "本机身份已安全创建。创建设备组，或稍后通过附近设备邀请加入。";

  return (
    <section className="card onboarding">
      <div className="card-heading"><h2>设备组</h2><span className="count">{phase}</span></div>
      <p>{description}</p>
      {state.phase === "unjoined" && <>
        <form onSubmit={submitCreate}>
          <label htmlFor="group-name">设备组名称</label>
          <input id="group-name" name="groupName" defaultValue="我的设备" required />
          <label htmlFor="device-name">本机名称</label>
          <input id="device-name" name="deviceName" placeholder="例如 NixOS 工作站或 Poke3" required />
          <button disabled={busy === "create"}>创建我的设备组</button>
        </form>
        <form onSubmit={submitJoin}>
          <h3>加入已有设备组</h3>
          <button type="button" disabled={busy === "discover"} onClick={() => void discoverNearby()}>
            {busy === "discover" ? "正在搜索附近设备…" : "搜索附近设备"}
          </button>
          {nearbyDevices.length > 0 && <div className="nearby-devices" role="list" aria-label="附近设备">
            {nearbyDevices.slice(0, NEARBY_MAX_ITEMS).map((device) => <button
              type="button"
              role="listitem"
              className={joinAddress === device.address ? "nearby-device selected" : "nearby-device"}
              key={device.address}
              onClick={() => setJoinAddress(device.address)}
            >
              <strong>{device.deviceHint || "附近的 Nekodex 设备"}</strong>
              <span>{device.address}</span>
            </button>)}
          </div>}
          <label htmlFor="join-address">邀请设备地址</label>
          <input id="join-address" name="address" inputMode="url" placeholder="例如 192.168.1.20:8788" value={joinAddress} onChange={(event) => setJoinAddress(event.currentTarget.value)} required />
          <label htmlFor="join-code">加入码</label>
          <input id="join-code" name="shortCode" inputMode="text" autoCapitalize="characters" autoComplete="one-time-code" minLength={8} maxLength={8} required />
          <label htmlFor="join-device-name">本机名称</label>
          <input id="join-device-name" name="deviceName" placeholder="例如 Poke3" required />
          <p className="warning">加入后，设备组成员可查看本机公布的额度、活动、会话和待审批状态，并可邀请或移除其他设备。仅加入你信任的个人设备组。</p>
          <button disabled={busy === "join"}>加入设备组</button>
        </form>
      </>}
      {state.phase === "joined" && <div id="joined-device">
        <dl>
          <div><dt>设备组</dt><dd>{state.groupName}</dd></div>
          <div><dt>本机</dt><dd>{state.deviceName}</dd></div>
          <div><dt>设备 ID</dt><dd>{state.deviceId}</dd></div>
          <div><dt>局域网服务</dt><dd>{network.listening ? `正在监听端口 ${network.port}` : "未启动"}</dd></div>
          <div><dt>已认证连接</dt><dd>{network.onlinePeers.length}</dd></div>
        </dl>
        <button type="button" onClick={() => void refreshNetwork()}>刷新连接状态</button>
        <button type="button" onClick={() => void refreshMembers()}>刷新成员</button>
        <button type="button" disabled={busy === "rename"} onClick={() => void renameDevice()}>修改本机名称</button>
        <button type="button" disabled={busy === "exit"} onClick={() => void exitGroup()}>退出设备组</button>
        <button type="button" disabled={busy === "invite"} onClick={() => void createInvitation()}>邀请新设备</button>
        {invitation && <p className="warning">收到新设备请求后，请核对设备名称和 ID。接受即授予监控数据查看及成员管理权限。</p>}
        {pendingJoin && <section className="pending-join">
          <h3>新设备请求加入</h3>
          <dl>
            <div><dt>设备名称</dt><dd>{pendingJoin.deviceName}</dd></div>
            <div><dt>设备 ID</dt><dd>{pendingJoin.deviceId}</dd></div>
            <div><dt>能力</dt><dd>{pendingJoin.codexCapable ? "Codex 来源电脑" : "监控端"} · 协议 {pendingJoin.protocolVersion}</dd></div>
          </dl>
          <p className="warning">请与新设备屏幕上的名称和设备 ID 核对。接受后，它将获得本组的监控数据查看及成员管理权限。</p>
          <div className="task-actions"><button type="button" disabled={busy === "join-decision"} onClick={() => void decideJoin(false)}>拒绝</button><button type="button" className="inverted" disabled={busy === "join-decision"} onClick={() => void decideJoin(true)}>核对无误，接受</button></div>
        </section>}
        {Object.entries(network.recoveryWarnings || {}).map(([deviceId, warning]) =>
          <output key={deviceId} className="warning">{deviceId} 的活动历史不完整：{warning}</output>
        )}
        <form className="business-form" onSubmit={connectManual}>
          <input name="address" placeholder="手工直连 IP:端口" required />
          <button>连接</button>
        </form>
        {invitation && <output id="invitation-output">{invitation}</output>}
        {membersTruncated && <p className="warning">成员列表不完整：已显示 {members.length} / {membersTotal} 个成员。</p>}
        <div className="empty-state">
          {members.length ? members.map((member) => <NodeRow
            key={member.deviceId}
            name={member.deviceName || member.deviceId}
            description={`${member.local ? "本机" : "已认证设备组成员"}${member.codexCapable === true ? " · Codex 来源" : member.codexCapable === false ? " · 监控端" : " · 能力未知"}${member.businessProtocol ? ` · 业务协议 ${member.businessProtocol.major}.${member.businessProtocol.minor}` : ""}`}
            state={memberStatusLabel(member.status || (member.online ? "online" : "offline"))}
          >
            {!member.local && <button type="button" onClick={() => void revoke(member.deviceId)}>移除</button>}
          </NodeRow>) : "尚未加载成员。"}
        </div>
      </div>}
      {error && <output id="device-error">{error}</output>}
      {notice && <output id="device-notice">{notice}</output>}
    </section>
  );
}

function Overview({ device, network, members, membersTotal, membersTruncated, sources, sourceError, openQuota, onState }: { device: DeviceState | null; network: NetworkState; members: Member[]; membersTotal: number; membersTruncated: boolean; sources: GroupSourceSnapshot[]; sourceError: string; openQuota: () => void; onState: (state: DeviceState) => void }) {
  const joined = device?.phase === "joined";
  const accounts = newestAccountSnapshots(sources);
  const developerHosts = members.filter((member) => member.codexCapable).length || (joined && device.codexCapable ? 1 : 0);
  const quotaWindows = accounts.flatMap((source) => source.value.quota?.windows || []);
  const knownRemaining = quotaWindows.map((window) => window.remaining_percent).filter((value): value is number => value != null);
  const lowestRemaining = knownRemaining.length ? Math.min(...knownRemaining) : null;
  const availableCounts = sources.map((source) => source.value.thread_counts).filter((counts) => counts?.available);
  const cachedCountSources = sources.filter((source) => source.cachedAt != null && source.value.thread_counts?.available).length;
  const cachedSources = sources.filter((source) => source.cachedAt != null).length;
  const historyOnlySources = sources.filter((source) => source.value.source_mode !== "sharedDaemon").length;
  const running = availableCounts.reduce((total, counts) => total + (counts?.running || 0), 0);
  const attention = availableCounts.reduce((total, counts) => total + (counts?.attention || 0), 0);
  const countsIncomplete = availableCounts.length < developerHosts || availableCounts.some((counts) => counts?.truncated);
  const countLabel = (value: number) => availableCounts.length ? `${cachedCountSources ? "缓存 " : ""}${countsIncomplete ? "≥" : ""}${value}` : "—";
  return <>
    <DeviceGroup onState={onState} observedNetwork={network} observedMembers={members} />
    <section className="summary" aria-label="状态摘要">
      <article><strong>{developerHosts}</strong><span>开发主机</span></article>
      <article><strong>{accounts.length || "—"}</strong><span>账户</span></article>
      <article className="inverted"><strong>{countLabel(running)}</strong><span>运行中</span></article>
      <article><strong>{countLabel(attention)}</strong><span>需关注</span></article>
    </section>
    <section className="grid">
      <article className="card">
        <div className="card-heading"><h2>额度摘要</h2><button type="button" onClick={openQuota}>查看额度</button></div>
        <p className="label">{accounts.length ? `${accounts.length} 个账户的最新独立快照` : "等待来源电脑连接 Codex 服务"}</p><strong className="quota">{lowestRemaining == null ? "未知" : `${lowestRemaining}%`}</strong>
        <p>{lowestRemaining == null ? "尚无可识别的剩余额度。" : "显示各账户最新快照中的最低剩余值，不跨账户或采集时间相加。"}</p>
        {sourceError && <p className="warning">{sourceError}</p>}
        {cachedSources > 0 && <p>{cachedSources} 个来源当前显示缓存快照，不能视为实时状态。</p>}
        {historyOnlySources > 0 && <p>{historyOnlySources} 个来源处于历史回退模式，其会话实时状态和详细审批可能不可见。</p>}
      </article>
      <article className="card">
        <div className="card-heading"><h2>节点状态</h2><span className="count">{membersTruncated ? `≥${membersTotal}` : members.length || (joined ? 1 : 0)}</span></div>
        {membersTruncated && <p className="warning">成员列表不完整，仅显示前 {members.length} 个成员。</p>}
        {members.length ? members.map((member) => <NodeRow key={member.deviceId} name={member.deviceName || member.deviceId} description={member.codexCapable ? "Codex 来源电脑" : "监控端"} state={member.local ? "本机" : memberStatusLabel(member.status)} />) : joined ? <NodeRow name={device.deviceName || "本机"} description={device.codexCapable ? "Codex 来源电脑" : "监控端"} state="本机" /> : <div className="empty-state">加入设备组后显示本机与已认证连接。</div>}
      </article>
    </section>
  </>;
}

type TargetTools = {
  target: string;
  targets: Array<{ id: string; label: string }>;
  onlineTargets: string[];
  capabilitiesByTarget: Record<string, string[]>;
  localSourceState: LocalSourceState;
  supports: (target: string, operation: BusinessOperation) => boolean;
  canRequest: (target: string, operation: ReadBusinessOperation) => boolean;
  canRead: (target: string, operation: ReadBusinessOperation) => boolean;
  setTarget: (value: string) => void;
  refreshTargets: () => Promise<string>;
};

type GroupSourceFailure = { target: string; error: string };
type GroupSourceResult = { items: GroupSourceSnapshot[]; failures: GroupSourceFailure[] };
let sharedGroupSnapshot: { key: string; expiresAt: number; promise: Promise<GroupSourceResult> } | null = null;

function invalidateGroupSnapshot() { sharedGroupSnapshot = null; }

async function groupSourceSnapshots(tools: TargetTools, force = false): Promise<GroupSourceResult> {
  let targets = tools.targets.map((target) => target.id);
  if (!targets.length) {
    const selected = await tools.refreshTargets();
    targets = selected ? [selected] : [];
  }
  const key = targets.map((target) => `${target}:${tools.onlineTargets.includes(target) ? "online" : "offline"}:${[...(tools.capabilitiesByTarget[target] || [])].sort().join(",")}`).sort().join(";");
  if (!force && sharedGroupSnapshot?.key === key && sharedGroupSnapshot.expiresAt > Date.now()) return sharedGroupSnapshot.promise;
  const promise = (async () => {
    const unsupported = targets.filter((target) => !tools.canRead(target, "stateSubscribe"));
    const results = await Promise.all(targets.filter((target) => tools.canRead(target, "stateSubscribe")).map(async (target) => {
      try {
        const canRequest = tools.canRequest(target, "stateSubscribe");
        const read = await readBusiness<SourceSnapshot>(target, "stateSubscribe", { after: null }, !canRequest, canRequest);
        return { target, value: read.value, cachedAt: read.cachedAt } satisfies GroupSourceSnapshot;
      } catch (error) {
        return { target, error };
      }
    }));
    return {
      items: results.filter((result): result is GroupSourceSnapshot => "value" in result),
      failures: [
        ...unsupported.map((target) => ({ target, error: "该来源未协商状态快照监控能力" })),
        ...results.filter((result) => "error" in result).map((result) => ({
          target: result.target,
          error: message(result.error),
        })),
      ],
    };
  })();
  sharedGroupSnapshot = { key, expiresAt: Date.now() + 10_000, promise };
  return promise;
}

const preferSourceSnapshot = (candidate: GroupSourceSnapshot, known: GroupSourceSnapshot) => {
  const candidateOnline = candidate.cachedAt == null;
  const knownOnline = known.cachedAt == null;
  if (candidateOnline !== knownOnline) return candidateOnline;
  const candidateObserved = candidate.value.quota?.observed_at_unix_seconds ?? candidate.value.observed_at_unix_seconds;
  const knownObserved = known.value.quota?.observed_at_unix_seconds ?? known.value.observed_at_unix_seconds;
  if (candidateObserved !== knownObserved) {
    return candidateObserved > knownObserved;
  }
  const candidateLive = candidate.value.source_mode === "sharedDaemon";
  const knownLive = known.value.source_mode === "sharedDaemon";
  if (candidateLive !== knownLive) return candidateLive;
  const candidateId = candidate.value.source_device_id || candidate.target;
  const knownId = known.value.source_device_id || known.target;
  return candidateId.localeCompare(knownId) < 0;
};

const newestAccountSnapshots = (sources: GroupSourceSnapshot[]) => {
  const accounts = new Map<string, GroupSourceSnapshot>();
  for (const source of sources) {
    const accountId = source.value.quota?.account_id;
    if (!accountId) continue;
    const known = accounts.get(accountId);
    if (!known || preferSourceSnapshot(source, known)) accounts.set(accountId, source);
  }
  return [...accounts.values()];
};

function QuotaPanel({ sources, error, refresh }: { sources: GroupSourceSnapshot[]; error: string; refresh: () => Promise<void> }) {
  const accounts = newestAccountSnapshots(sources);
  const sourceErrors = sources.flatMap((source) => {
    const message = source.value.errors?.quota;
    if (!message) return [];
    return [{
      source: source.value.source_device_id || source.target,
      message,
      cachedAt: source.cachedAt,
    }];
  });
  return <section className="card placeholder-panel">
    <h2>额度</h2><button type="button" onClick={() => void refresh()}>刷新所有来源</button>
    <div className="empty-state">
      {error && <p>{error}</p>}
      {sourceErrors.map((failure) => <div className="warning failure-notice" key={`${failure.source}:${failure.message}`}>
        <strong>来源 {compactIdentifier(failure.source)} 暂时无法读取额度。</strong>
        <span>{failure.cachedAt != null ? "缓存快照中也没有可用额度，请稍后刷新。" : "设备连接仍然可用，请稍后刷新。"}</span>
        <details><summary>查看技术详情</summary><pre>{boundedText(failure.message, 512)}</pre></details>
      </div>)}
      {!accounts.length ? sourceErrors.length || error
        ? "当前来源已连接，但没有可展示的额度快照。请先处理上方错误后刷新。"
        : "尚无额度数据。接入来源电脑后会按账户去重，并显示采集时间。" : accounts.map((source) => {
        const quota = source.value.quota!;
        return <section className="quota-account" key={quota.account_id}>
          <h3>账户 {quota.account_id}</h3>
          {quota.windows.map((window) => <NodeRow key={`${quota.account_id}:${window.name}`} name={window.name} description={window.resets_at_unix_seconds ? `重置：${new Date(window.resets_at_unix_seconds * 1000).toLocaleString()}` : "重置时间未知"} state={window.remaining_percent == null ? "未知" : `剩余 ${window.remaining_percent}%`} />)}
          {!quota.windows.length && <p>账户已识别，但额度窗口不可用。</p>}
          <p>来源 {quota.source_device_id} · {source.value.source_mode === "sharedDaemon" ? "共享实时来源" : "历史回退来源"} · 采集于 {new Date(quota.observed_at_unix_seconds * 1000).toLocaleString()} {staleLabel(source.cachedAt)}</p>
        </section>;
      })}
    </div>
  </section>;
}

function ActivityPanel({ tools, active }: { tools: TargetTools; active: boolean }) {
  const [buckets, setBuckets] = useState<GroupDailyUsage[] | null>(null);
  const [activities, setActivities] = useState<Activity[]>([]);
  const [sourceFilter, setSourceFilter] = useState("");
  const [dateFilter, setDateFilter] = useState("");
  const [activityLimit, setActivityLimit] = useState(ACTIVITY_PAGE_SIZE);
  const [error, setError] = useState("");
  const [usageUnavailable, setUsageUnavailable] = useState(false);
  const eventRevision = useRef<number | null>(null);
  const refreshSequence = useRef(0);
  const eventUpdateSequence = useRef(0);
  const sourceReadKey = tools.targets.map((target) => `${target.id}:${tools.onlineTargets.includes(target.id) ? "online" : "offline"}:${[...(tools.capabilitiesByTarget[target.id] || [])].sort().join(",")}`).sort().join(";");
  const refresh = useCallback(async (force = false) => {
    const sequence = ++refreshSequence.current;
    try {
      const [snapshots, revision] = await Promise.all([
        groupSourceSnapshots(tools, force),
        invoke<number>("p2_event_revision"),
      ]);
      const eventSequence = ++eventUpdateSequence.current;
      const events = await invoke<EventEnvelope[]>("p2_events");
      if (sequence !== refreshSequence.current) return;
      const newestUsage = new Map<string, GroupSourceSnapshot>();
      for (const source of snapshots.items) {
        if (!source.value.raw_usage) continue;
        const accountId = source.value.quota?.account_id;
        const key = accountId ? `account:${accountId}` : `source:${source.target}`;
        const known = newestUsage.get(key);
        if (!known || preferSourceSnapshot(source, known)) newestUsage.set(key, source);
      }
      setBuckets([...newestUsage.values()].flatMap((source) => {
        const accountId = source.value.quota?.account_id || `来源 ${source.target}`;
        const sourceDeviceId = source.value.quota?.source_device_id || source.value.source_device_id || source.target;
        return (source.value.raw_usage?.dailyUsageBuckets || []).map((bucket) => ({
          ...bucket,
          accountId,
          sourceDeviceId,
          cachedAt: source.cachedAt,
          sourceMode: source.value.source_mode,
        }));
      }).sort((left, right) => right.startDate.localeCompare(left.startDate) || left.accountId.localeCompare(right.accountId)));
      if (eventSequence === eventUpdateSequence.current) {
        eventRevision.current = revision;
        const nextActivities = projectActivities(events);
        setActivities((current) => sameActivities(current, nextActivities) ? current : nextActivities);
      }
      setActivityLimit(ACTIVITY_PAGE_SIZE);
      const cached = snapshots.items.filter((source) => source.cachedAt != null).length;
      const usageFailures = snapshots.items.flatMap((source) => {
        const failure = source.value.errors?.usage;
        return failure
          ? [`来源 ${source.value.source_device_id || source.target} 的日用量读取失败：${boundedText(failure, 512)}`]
          : [];
      });
      setUsageUnavailable(snapshots.failures.length > 0 || usageFailures.length > 0);
      const warnings = [
        ...snapshots.failures.slice(0, 5).map((failure) =>
          `来源 ${failure.target} 当前不可读：${boundedText(failure.error, 512)}`,
        ),
        snapshots.failures.length > 5 ? `另有 ${snapshots.failures.length - 5} 个来源当前不可读` : "",
        cached ? `${cached} 个来源当前显示本机缓存` : "",
        ...usageFailures.slice(0, 5),
        usageFailures.length > 5 ? `另有 ${usageFailures.length - 5} 个来源的日用量读取失败` : "",
        snapshots.items.some((source) => source.value.source_mode !== "sharedDaemon") ? "部分来源处于历史回退模式，实时会话状态可能不可见" : "",
      ].filter(Boolean);
      setError(warnings.join("；"));
    } catch (caught) {
      if (sequence === refreshSequence.current) {
        setUsageUnavailable(true);
        setError(message(caught));
      }
    }
  }, [tools.refreshTargets, sourceReadKey]);
  useEffect(() => {
    if (!active) return;
    if (document.visibilityState === "visible") void refresh();
    const onVisibility = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [active, refresh]);
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    let running = false;
    const pollEvents = async () => {
      if (running || document.visibilityState !== "visible") return;
      running = true;
      try {
        const revision = await invoke<number>("p2_event_revision");
        if (eventRevision.current === revision) return;
        const sequence = ++eventUpdateSequence.current;
        const events = await invoke<EventEnvelope[]>("p2_events");
        if (cancelled || sequence !== eventUpdateSequence.current) return;
        eventRevision.current = revision;
        const nextActivities = projectActivities(events);
        setActivities((current) => sameActivities(current, nextActivities) ? current : nextActivities);
      } catch {
        // Manual refresh reports failures; background event refresh stays quiet.
      } finally {
        running = false;
      }
    };
    const timer = window.setInterval(() => void pollEvents(), VISIBLE_MONITOR_POLL_MS);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [active]);
  const sources = [...new Set([
    ...activities.map((activity) => activity.source_device_id),
    ...(buckets || []).map((bucket) => bucket.sourceDeviceId),
  ])];
  const filtered = activities.filter((activity) => {
    if (sourceFilter && activity.source_device_id !== sourceFilter) return false;
    if (!dateFilter) return true;
    return new Date(activity.occurred_at_unix_seconds * 1000).toLocaleDateString("sv-SE") === dateFilter;
  });
  const filteredBuckets = (buckets || []).filter((bucket) => {
    if (sourceFilter && bucket.sourceDeviceId !== sourceFilter) return false;
    return !dateFilter || bucket.startDate.slice(0, 10) === dateFilter;
  });
  const visible = filtered.slice(0, Math.min(activityLimit, ACTIVITY_MAX_ITEMS));
  return <section className="card placeholder-panel">
    <h2>活动</h2><button type="button" onClick={() => void refresh(true)}>刷新活动与日用量</button>
    <div className="business-toolbar">
      <select aria-label="按来源筛选" value={sourceFilter} onChange={(event) => { setSourceFilter(event.target.value); setActivityLimit(ACTIVITY_PAGE_SIZE); }}><option value="">全部来源</option>{sources.map((source) => <option key={source} value={source}>{source}</option>)}</select>
      <input aria-label="按日期筛选" type="date" value={dateFilter} onChange={(event) => { setDateFilter(event.target.value); setActivityLimit(ACTIVITY_PAGE_SIZE); }} />
    </div>
    <div className="empty-state">
      {error && <p>{error}</p>}
      {buckets === null ? "来源接口无数据时会明确显示不可用。" : filteredBuckets.length ? filteredBuckets.map((bucket, index) => <NodeRow key={`${bucket.accountId}:${bucket.sourceDeviceId}:${bucket.startDate}:${index}`} name={bucket.startDate} description={`${bucket.accountId} · 来源 ${bucket.sourceDeviceId} · ${bucket.sourceMode === "sharedDaemon" ? "共享实时来源" : "历史回退来源"}${staleLabel(bucket.cachedAt)}`} state={`${tokenCount.format(bucket.tokens)} tokens`} />) : usageUnavailable ? "当前没有可展示的日用量；部分来源读取失败，无法确认是否确实为空。" : "当前筛选范围内没有日用量数据。"}
    </div>
    <div className="empty-state">
      {visible.length ? visible.map((activity) => <NodeRow key={`${activity.source_device_id}:${activity.activity_id}`} name={activity.message || activity.kind} description={`${activity.source_device_id} · ${activity.thread.thread_id} · ${new Date(activity.occurred_at_unix_seconds * 1000).toLocaleString()}`} state={activity.kind} />) : "尚无可显示的活动记录。"}
      <p className="thread-partial">活动列表最多同时显示 {ACTIVITY_MAX_ITEMS} 条；这是为墨水屏和跨设备监控设置的安全上限。{filtered.length > visible.length ? ` 当前已显示 ${visible.length}/${filtered.length} 条。` : " 已显示当前筛选范围内的全部记录。"}</p>
      {filtered.length > visible.length && visible.length < ACTIVITY_MAX_ITEMS && <button type="button" onClick={() => setActivityLimit((limit) => Math.min(limit + ACTIVITY_PAGE_SIZE, ACTIVITY_MAX_ITEMS))}>再显示 {Math.min(ACTIVITY_PAGE_SIZE, ACTIVITY_MAX_ITEMS - visible.length)} 条（剩余 {filtered.length - visible.length}）</button>}
      {filtered.length > visible.length && visible.length >= ACTIVITY_MAX_ITEMS && <p className="thread-partial">已达到活动显示上限；请使用来源或日期筛选以查看其他记录。</p>}
    </div>
  </section>;
}

function ThreadsPanel({ tools, active }: { tools: TargetTools; active: boolean }) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectId, setProjectId] = useState("");
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [threadsLoading, setThreadsLoading] = useState(false);
  const threadRequestSequence = useRef(0);
  const [detail, setDetail] = useState<{ target: string; projectId: string; threadId: string; value: ThreadDetailResponse; cachedAt: number | null } | null>(null);
  const [detailTurnLimit, setDetailTurnLimit] = useState(DETAIL_TURN_PAGE_SIZE);
  const detailRequestSequence = useRef(0);
  const detailRef = useRef<typeof detail>(null);
  const approvalEventRevision = useRef<number | null>(null);
  const approvalEvents = useRef("");
  const threadListEvents = useRef("");
  const detailEvents = useRef("");
  const [approvals, setApprovals] = useState<MonitoredApproval[]>([]);
  const [approvalWarning, setApprovalWarning] = useState("");
  const [status, setStatus] = useState("请选择来源设备。");
  const [localDeviceId, setLocalDeviceId] = useState("");
  const [sourceCapable, setSourceCapable] = useState(false);
  const [sourceReady, setSourceReady] = useState(false);
  const [sourceMode, setSourceMode] = useState<string | null>(null);
  const [sourceActionBusy, setSourceActionBusy] = useState(false);
  const sourceTargetKey = tools.targets
    .map((target) => `${target.id}:${tools.onlineTargets.includes(target.id) ? "online" : "offline"}:${[...(tools.capabilitiesByTarget[target.id] || [])].sort().join(",")}`)
    .sort()
    .join(";");
  const selection = useRef({ target: tools.target, projectId });
  selection.current = { target: tools.target, projectId };

  useEffect(() => { detailRef.current = detail; }, [detail]);

  const refreshProjects = useCallback(async (target = tools.target) => {
    if (!target) { setStatus("请先选择来源设备。"); return; }
    try {
      const local = await invoke<DeviceState>("p1_state");
      setLocalDeviceId(local.deviceId || "");
      let items: Project[];
      let cachedAt: number | null = null;
      let incomplete = false;
      if (target === local.deviceId) {
        items = await invoke<Project[]>("p2_local_projects");
        incomplete = items.length > PROJECT_MAX_ITEMS;
        items = items.slice(0, PROJECT_MAX_ITEMS);
      } else {
        items = [];
        let after: number | null = null;
        const seen = new Set<number>();
        let pages = 0;
        do {
          if (++pages > 100) throw new Error("项目分页超过安全上限");
          const canRequest = tools.canRequest(target, "projectsList");
          const read = await readBusiness<ProjectPage>(target, "projectsList", { page: { after, limit: 100 } }, !canRequest, canRequest);
          const pageItems = Array.isArray(read.value.items) ? read.value.items : [];
          const remaining = Math.max(0, PROJECT_MAX_ITEMS - items.length);
          if (pageItems.length > remaining) incomplete = true;
          items.push(...pageItems.slice(0, remaining));
          if (read.cachedAt != null) cachedAt = cachedAt == null ? read.cachedAt : Math.min(cachedAt, read.cachedAt);
          after = read.value.next ?? null;
          if (items.length >= PROJECT_MAX_ITEMS && after != null) {
            incomplete = true;
            break;
          }
          if (after != null && seen.has(after)) throw new Error("来源返回了重复的项目分页游标");
          if (after != null) seen.add(after);
        } while (after != null);
      }
      if (selection.current.target !== target) return;
      setProjects(items);
      if (!items.some((project) => project.project_id === selection.current.projectId)) {
        selection.current = { ...selection.current, projectId: "" };
        setProjectId("");
        threadRequestSequence.current += 1;
        detailRequestSequence.current += 1;
        setThreads([]);
        setNextCursor(null);
        detailRef.current = null;
        setDetail(null);
      }
      setStatus(`已载入 ${items.length} 个项目${staleLabel(cachedAt)}${incomplete ? `；已达到 ${PROJECT_MAX_ITEMS} 个显示上限，结果可能不完整` : ""}。`);
    } catch (caught) { setStatus(message(caught)); }
  }, [tools.target, sourceTargetKey]);

  const loadThreads = useCallback(async (
    after: number | null,
    append: boolean,
    selectedProject = selection.current.projectId,
    target = selection.current.target,
  ) => {
    if (!target || !tools.canRead(target, "threadsList")) { setStatus("该来源不支持读取会话列表。"); return; }
    if (append && threads.length >= THREAD_MAX_ITEMS) {
      setNextCursor(null);
      setStatus(`会话列表已达到 ${THREAD_MAX_ITEMS} 条监控上限，未继续读取。`);
      return;
    }
    const sequence = ++threadRequestSequence.current;
    setThreadsLoading(true);
    try {
      const canRequest = tools.canRequest(target, "threadsList");
      const read = await readBusiness<ThreadPage>(target, "threadsList", { project_id: selectedProject || null, page: { after, limit: THREAD_PAGE_SIZE } }, !canRequest, canRequest);
      if (sequence !== threadRequestSequence.current || selection.current.target !== target || selection.current.projectId !== selectedProject) return;
      const items = Array.isArray(read.value.items) ? read.value.items : [];
      const deduplicated = [...new Map((append ? [...threads, ...items] : items).map((thread) => [thread.id, thread])).values()];
      setThreads((current) => {
        const combined = append ? [...current, ...items] : items;
        return [...new Map(combined.map((thread) => [thread.id, thread])).values()].slice(0, THREAD_MAX_ITEMS);
      });
      const reachedLimit = deduplicated.length >= THREAD_MAX_ITEMS;
      setNextCursor(reachedLimit ? null : read.value.next ?? null);
      setStatus(items.length ? `已载入 ${Math.min(items.length, THREAD_MAX_ITEMS)} 条会话${staleLabel(read.cachedAt)}${reachedLimit ? `；已达到 ${THREAD_MAX_ITEMS} 条监控上限` : ""}。` : append ? "没有更多会话。" : "暂无会话。");
    } catch (caught) { setStatus(message(caught)); }
    finally {
      if (sequence === threadRequestSequence.current) setThreadsLoading(false);
    }
  }, [threads, tools.canRead, sourceTargetKey]);

  const readThread = async (threadId: string, target = tools.target) => {
    if (!target || !tools.canRead(target, "threadsRead")) { setStatus("该来源不支持读取会话详情。"); return; }
    const selectedProject = selection.current.projectId;
    const sequence = ++detailRequestSequence.current;
    try {
      const canRequest = tools.canRequest(target, "threadsRead");
      const read = await readBusiness<ThreadDetailResponse>(target, "threadsRead", { thread_id: threadId, project_id: selectedProject || null, include_turns: true, before_turn_id: null, turn_limit: DETAIL_TURN_PAGE_SIZE }, !canRequest, canRequest);
      if (sequence !== detailRequestSequence.current || selection.current.target !== target || selection.current.projectId !== selectedProject) return;
      setDetailTurnLimit(DETAIL_TURN_PAGE_SIZE);
      setDetail({ target, projectId: selectedProject, threadId, value: read.value, cachedAt: read.cachedAt });
      setStatus(`${threadId}：已载入 ${read.value.thread?.turns?.length ?? 0} 个回合${staleLabel(read.cachedAt)}${read.value.truncated || read.value.incomplete ? "；内容可能不完整" : ""}。`);
    } catch (caught) { setStatus(message(caught)); }
  };

  const loadEarlierTurns = async () => {
    const current = detailRef.current;
    if (!current || !tools.canRead(current.target, "threadsRead")) return;
    const beforeTurnId = current.value.next_before_turn_id ?? null;
    const existingTurns = current.value.thread?.turns || [];
    if (!beforeTurnId || existingTurns.length >= DETAIL_MAX_TURNS) {
      setStatus(existingTurns.length >= DETAIL_MAX_TURNS
        ? `会话详情已达到 ${DETAIL_MAX_TURNS} 个回合监控上限，未继续读取。`
        : "已经没有更早的回合。");
      return;
    }
    const target = current.target;
    const selectedProject = current.projectId;
    const threadId = current.threadId;
    const sequence = ++detailRequestSequence.current;
    try {
      const canRequest = tools.canRequest(target, "threadsRead");
      const read = await readBusiness<ThreadDetailResponse>(target, "threadsRead", { thread_id: threadId, project_id: selectedProject || null, include_turns: true, before_turn_id: beforeTurnId, turn_limit: DETAIL_TURN_PAGE_SIZE }, !canRequest, canRequest);
      if (sequence !== detailRequestSequence.current || selection.current.target !== target || selection.current.projectId !== selectedProject || detailRef.current?.threadId !== threadId) return;
      const earlierTurns = Array.isArray(read.value.thread?.turns) ? read.value.thread.turns : [];
      const existingById = new Set(existingTurns.map((turn) => turn.id).filter((id): id is string => Boolean(id)));
      const freshEarlierTurns = earlierTurns.filter((turn) => !turn.id || !existingById.has(turn.id));
      const combinedTurns = [...freshEarlierTurns, ...existingTurns].slice(0, DETAIL_MAX_TURNS);
      const nextValue: ThreadDetailResponse = {
        ...read.value,
        truncated: current.value.truncated || read.value.truncated,
        incomplete: current.value.incomplete || read.value.incomplete,
        next_before_turn_id: combinedTurns.length >= DETAIL_MAX_TURNS ? null : (read.value.next_before_turn_id ?? null),
        thread: { ...(current.value.thread || {}), ...(read.value.thread || {}), turns: combinedTurns },
      };
      const cachedAt = current.cachedAt == null ? read.cachedAt : read.cachedAt == null ? current.cachedAt : Math.min(current.cachedAt, read.cachedAt);
      setDetail({ ...current, value: nextValue, cachedAt });
      setStatus(`${threadId}：已载入 ${combinedTurns.length} 个回合${staleLabel(cachedAt)}${nextValue.truncated || nextValue.incomplete ? "；内容可能不完整" : ""}${combinedTurns.length >= DETAIL_MAX_TURNS ? `；已达到 ${DETAIL_MAX_TURNS} 个回合监控上限` : ""}。`);
    } catch (caught) { setStatus(message(caught)); }
  };

  const refreshApprovals = useCallback(async (quiet = false) => {
    const targets = tools.targets.map((target) => target.id).filter((target) => tools.canRead(target, "approvalsList"));
    const results = await Promise.all(targets.map(async (target) => {
      try {
        const items: Approval[] = [];
        const seen = new Set<number>();
        let after: number | null = null;
        let cachedAt: number | null = null;
        let incomplete = false;
        for (let page = 0; page < APPROVAL_MAX_PAGES; page += 1) {
          const canRequest = tools.canRequest(target, "approvalsList");
          const read = await readBusiness<ApprovalPage>(target, "approvalsList", { thread: null, page: { after, limit: APPROVAL_PAGE_SIZE } }, !canRequest, canRequest);
          const pageItems = Array.isArray(read.value.items) ? read.value.items : [];
          items.push(...pageItems.slice(0, Math.max(0, APPROVAL_MAX_ITEMS - items.length)));
          if (read.cachedAt != null) {
            cachedAt = cachedAt == null ? read.cachedAt : Math.min(cachedAt, read.cachedAt);
          }
          incomplete ||= read.value.incomplete === true;
          const next = read.value.next ?? null;
          if (items.length >= APPROVAL_MAX_ITEMS && next != null) {
            incomplete = true;
            return { target, read: { value: { items, incomplete }, cachedAt } };
          }
          if (next == null) return { target, read: { value: { items, incomplete }, cachedAt } };
          if (seen.has(next)) throw new Error("来源返回了重复的待审批分页游标");
          seen.add(next);
          after = next;
        }
        return { target, read: { value: { items, incomplete: true }, cachedAt } };
      }
      catch (caught) { return { target, read: null, error: message(caught) }; }
    }));
    const merged = new Map<string, MonitoredApproval>();
    for (const result of results) {
      if (!result.read) continue;
      for (const approval of result.read.value.items.filter((item) => item.status === "Pending")) {
        if (merged.size >= APPROVAL_MAX_RENDERED) break;
        merged.set(`${result.target}:${approval.approval_id}`, {
          target: result.target,
          approval,
          cachedAt: result.read.cachedAt,
          unavailable: false,
        });
      }
    }
    const failedResults = results.filter((result) => !result.read);
    const failed = new Set(failedResults.map((result) => result.target));
    const incomplete = new Set(results.filter((result) => result.read?.value.incomplete).map((result) => result.target));
    setApprovals((current) => {
      for (const row of current) {
        if (failed.has(row.target)) merged.set(`${row.target}:${row.approval.approval_id}`, { ...row, unavailable: true });
      }
      const nextApprovals = [...merged.values()].slice(0, APPROVAL_MAX_RENDERED);
      return JSON.stringify(current) === JSON.stringify(nextApprovals) ? current : nextApprovals;
    });
    const renderTruncated = merged.size > APPROVAL_MAX_RENDERED;
    const failureDetails = failedResults.slice(0, 5).map((result) =>
      `来源 ${result.target} 的待审批读取失败：${boundedText("error" in result ? result.error : "未知错误", 512)}`,
    );
    const warnings = [
      ...failureDetails,
      failedResults.length > 5 ? `另有 ${failedResults.length - 5} 个来源的待审批状态当前不可读。` : "",
      failed.size ? "无法读取的来源不能确认待审批状态；保留的旧结果已明确标记。" : "",
      incomplete.size ? `${incomplete.size} 个来源的会话扫描达到安全上限，待审批结果可能不完整。` : "",
      renderTruncated ? `待审批显示已达到 ${APPROVAL_MAX_RENDERED} 条上限，结果可能不完整。` : "",
    ].filter(Boolean);
    setApprovalWarning(warnings.join(" "));
    if (!quiet) setStatus(`已刷新 ${results.filter((result) => result.read).length}/${targets.length} 个来源的待审批状态${failed.size || incomplete.size ? "，部分来源不可完整确认" : ""}。`);
  }, [sourceTargetKey, tools.canRead]);

  useEffect(() => {
    if (!active) return;
    void tools.refreshTargets();
    setSourceCapable(tools.localSourceState.capable);
    setSourceReady(tools.localSourceState.ready);
    setSourceMode(tools.localSourceState.mode || null);
    if (tools.localSourceState.error) setStatus(tools.localSourceState.error);
  }, [active, tools.localSourceState, tools.refreshTargets]);

  useEffect(() => {
    threadRequestSequence.current += 1;
    detailRequestSequence.current += 1;
    setThreadsLoading(false);
    setProjects([]); setProjectId(""); setThreads([]); setNextCursor(null); detailRef.current = null; setDetail(null); setDetailTurnLimit(DETAIL_TURN_PAGE_SIZE);
    if (active && tools.target) void refreshProjects(tools.target);
  }, [active, tools.target, sourceTargetKey]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    approvalEventRevision.current = null;
    void Promise.all([
      invoke<number>("p2_event_revision"),
      invoke<EventEnvelope[]>("p2_events"),
    ]).then(async ([revision, events]) => {
      if (cancelled) return;
      approvalEventRevision.current = revision;
      approvalEvents.current = approvalEventKey(events);
      threadListEvents.current = sourceActivityEventKey(events, selection.current.target);
      await refreshApprovals();
    }).catch(() => void refreshApprovals());
    const poll = async () => {
      if (document.visibilityState !== "visible") return;
      try {
        const revision = await invoke<number>("p2_event_revision");
        if (approvalEventRevision.current === revision) return;
        approvalEventRevision.current = revision;
        const events = await invoke<EventEnvelope[]>("p2_events");
        const nextApprovalEvents = approvalEventKey(events);
        if (nextApprovalEvents !== approvalEvents.current) {
          approvalEvents.current = nextApprovalEvents;
          await refreshApprovals(true);
        }
        const currentSelection = selection.current;
        const nextThreadListEvents = sourceActivityEventKey(events, currentSelection.target);
        if (nextThreadListEvents !== threadListEvents.current) {
          threadListEvents.current = nextThreadListEvents;
          await loadThreads(null, false, currentSelection.projectId, currentSelection.target);
        }
        const currentDetail = detailRef.current;
        if (currentDetail && tools.canRead(currentDetail.target, "threadsRead")) {
          const nextDetailEvents = threadEventKey(events, currentDetail.target, currentDetail.threadId);
          if (nextDetailEvents === detailEvents.current) return;
          detailEvents.current = nextDetailEvents;
          const sequence = ++detailRequestSequence.current;
          const canRequest = tools.canRequest(currentDetail.target, "threadsRead");
          const read = await readBusiness<ThreadDetailResponse>(currentDetail.target, "threadsRead", { thread_id: currentDetail.threadId, project_id: currentDetail.projectId || null, include_turns: true, before_turn_id: null, turn_limit: DETAIL_TURN_PAGE_SIZE }, !canRequest, canRequest);
          if (sequence === detailRequestSequence.current && selection.current.target === currentDetail.target && selection.current.projectId === currentDetail.projectId && detailRef.current?.target === currentDetail.target && detailRef.current.projectId === currentDetail.projectId && detailRef.current.threadId === currentDetail.threadId) {
            setDetail({ ...currentDetail, value: read.value, cachedAt: read.cachedAt });
          }
        }
      } catch {
        // Manual refresh remains the visible error path.
      }
    };
    const timer = window.setInterval(() => void poll(), VISIBLE_MONITOR_POLL_MS);
    const approvalTimer = window.setInterval(() => {
      if (document.visibilityState === "visible") void refreshApprovals(true);
    }, 15_000);
    return () => { cancelled = true; window.clearInterval(timer); window.clearInterval(approvalTimer); };
  }, [active, loadThreads, refreshApprovals]);

  const restartSource = async () => {
    if (sourceActionBusy) return;
    setSourceActionBusy(true);
    try {
      const source = await invoke<{ capable: boolean; ready: boolean; mode?: string | null; error?: string }>("p2_restart_source");
      invalidateGroupSnapshot();
      setSourceReady(source.ready); setSourceMode(source.mode || null); setStatus(source.ready ? "本机来源服务已重启。" : source.error || "来源服务不可用。");
      await tools.refreshTargets();
    } catch (caught) { setStatus(message(caught)); }
    finally { setSourceActionBusy(false); }
  };

  const startSharedSource = async () => {
    if (sourceActionBusy) return;
    setSourceActionBusy(true);
    try {
      const source = await invoke<{ capable: boolean; ready: boolean; mode?: string | null; error?: string }>("p2_start_shared_source");
      invalidateGroupSnapshot();
      setSourceReady(source.ready); setSourceMode(source.mode || null);
      setStatus(source.mode === "sharedDaemon" ? "共享 App Server daemon 已连接。" : source.error || "daemon 已启动，但来源仍处于历史回退模式。");
      await tools.refreshTargets();
    } catch (caught) { setStatus(message(caught)); }
    finally { setSourceActionBusy(false); }
  };

  const addProject = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault(); const form = event.currentTarget; const data = new FormData(form);
    try { await invoke("p2_add_local_project", { name: data.get("name"), path: data.get("path"), availableModels: [] }); invalidateGroupSnapshot(); form.reset(); await refreshProjects(); }
    catch (caught) { setStatus(message(caught)); }
  };

  const editProject = async (project: Project) => {
    const name = window.prompt("项目名称", project.name); if (name == null) return;
    const path = window.prompt("项目目录", project.path); if (path == null) return;
    try { await invoke("p2_update_local_project", { projectId: project.project_id, name, path, availableModels: [] }); invalidateGroupSnapshot(); await refreshProjects(); }
    catch (caught) { setStatus(message(caught)); }
  };

  const deleteProject = async (project: Project) => {
    if (!window.confirm(`删除本机项目“${project.name}”？这不会删除项目目录或历史会话。`)) return;
    try { await invoke("p2_delete_local_project", { projectId: project.project_id }); invalidateGroupSnapshot(); await refreshProjects(); }
    catch (caught) { setStatus(message(caught)); }
  };

  return <section className="card placeholder-panel">
    <h2>会话监控</h2>
    {sourceCapable && <p>本机来源：{sourceReady ? sourceMode === "sharedDaemon" ? "共享 App Server daemon（可观察其已加载会话）" : "独立只读回退（持久化历史可读，实时状态受限）" : "不可用"}</p>}
    <div className="business-toolbar">
      <button type="button" disabled={!tools.target} onClick={() => void refreshProjects()}>刷新项目</button>
      <button type="button" disabled={threadsLoading || !tools.canRead(tools.target, "threadsList")} onClick={() => void loadThreads(null, false)}>刷新会话</button>
      <button type="button" onClick={() => void refreshApprovals()}>刷新待审批</button>
      {sourceCapable && <button type="button" disabled={sourceActionBusy} onClick={() => void restartSource()}>{sourceReady ? "重启本机来源" : "启动本机来源"}</button>}
      {sourceCapable && sourceMode !== "sharedDaemon" && <button type="button" disabled={sourceActionBusy} onClick={() => void startSharedSource()}>启用共享实时来源</button>}
      <output>{status}</output>
    </div>
    <div className="empty-state">{projects.length ? projects.map((project) => <NodeRow key={project.project_id} name={project.name} description={project.path} state={project.project_id}><div className="task-actions"><button type="button" onClick={() => { threadRequestSequence.current += 1; detailRequestSequence.current += 1; setThreadsLoading(false); selection.current = { ...selection.current, projectId: project.project_id }; setProjectId(project.project_id); setThreads([]); setNextCursor(null); detailRef.current = null; setDetail(null); void loadThreads(null, false, project.project_id); }}>查看会话</button>{tools.target === localDeviceId && <><button type="button" onClick={() => void editProject(project)}>编辑</button><button type="button" onClick={() => void deleteProject(project)}>删除</button></>}</div></NodeRow>) : "该设备尚未配置项目。"}</div>
      {sourceCapable && tools.target === localDeviceId && <form className="business-form" onSubmit={addProject}><input name="name" placeholder="项目名称" required /><input name="path" placeholder="本机项目目录" required /><button>添加本地项目</button></form>}
    <div className="empty-state">
      {threads.length ? threads.map((thread) => <NodeRow key={thread.id} name={thread.name || thread.preview || "未命名会话"} description={thread.id} state={threadStatusLabel(thread.status)}><button type="button" onClick={() => void readThread(thread.id)}>查看详情</button></NodeRow>) : "暂无会话。"}
      {threads.length >= THREAD_MAX_ITEMS && <p className="thread-partial">会话列表已达到 {THREAD_MAX_ITEMS} 条监控显示上限；未继续读取更多会话，请缩小项目范围。</p>}
      {nextCursor != null && <button type="button" disabled={threadsLoading} onClick={() => void loadThreads(nextCursor, true)}>{threadsLoading ? "正在加载…" : "加载更多会话"}</button>}
    </div>
    {detail && (() => {
      const turns = detail.value.thread?.turns || [];
      const visibleTurns = turns.slice(0, Math.min(detailTurnLimit, DETAIL_MAX_TURNS));
      const hasEarlierTurns = detail.value.next_before_turn_id != null && turns.length < DETAIL_MAX_TURNS;
      return <section className="thread-detail" aria-live="polite"><div className="thread-detail-heading"><div><strong>{detail.value.thread?.preview || detail.threadId}</strong><small>{detail.threadId} · {turns.length} 个回合{staleLabel(detail.cachedAt)}</small></div><div className="task-actions"><button type="button" onClick={() => void readThread(detail.threadId, detail.target)}>刷新</button><button type="button" onClick={() => { detailRequestSequence.current += 1; detailRef.current = null; setDetail(null); }}>关闭</button></div></div>{(detail.value.truncated || detail.value.incomplete) && <p>该会话内容可能不完整，仅显示适合跨设备监控的内容。</p>}<p className="thread-partial">会话详情最多显示 {DETAIL_MAX_TURNS} 个回合，每回合最多显示 {DETAIL_ITEMS_PER_TURN} 项；这是为只读监控设置的安全上限。</p><div className="thread-turns">{visibleTurns.map((turn, turnIndex) => <article className="thread-turn" key={turn.id || turnIndex}><header>回合 {turnIndex + 1}<span>{turn.status || "状态未知"}</span></header>{(turn.items || []).slice(0, DETAIL_ITEMS_PER_TURN).map((item, itemIndex) => { const presentation = threadItemPresentation(item); return <div className={`thread-item thread-item-${item.type || "unknown"}`} key={item.id || itemIndex}><strong>{presentation.label}</strong>{presentation.text && <pre>{boundedText(presentation.text)}</pre>}</div>; })}</article>)}</div>{hasEarlierTurns && <button type="button" onClick={() => void loadEarlierTurns()}>加载更早回合（当前 {turns.length}/{DETAIL_MAX_TURNS}）</button>}{turns.length >= DETAIL_MAX_TURNS && <p className="thread-partial">已达到会话详情显示上限；未继续读取更早回合。</p>}{turns.length > visibleTurns.length && visibleTurns.length < DETAIL_MAX_TURNS && <button type="button" onClick={() => setDetailTurnLimit((limit) => Math.min(limit + DETAIL_TURN_PAGE_SIZE, DETAIL_MAX_TURNS))}>再显示 {Math.min(DETAIL_TURN_PAGE_SIZE, DETAIL_MAX_TURNS - visibleTurns.length)} 个回合（剩余 {turns.length - visibleTurns.length}）</button>}{turns.length > visibleTurns.length && visibleTurns.length >= DETAIL_MAX_TURNS && <p className="thread-partial">已达到会话详情显示上限；仅展示受限内容，请刷新或依靠后端截断提示。</p>}</section>;
    })()}
    <h3>待审批监控</h3>
    <p>仅显示所选来源的 Nekodex 连接实际收到的审批请求；其他独立 Codex 客户端的审批不会被误报为可见。</p>
    <div className="empty-state">
      {approvalWarning && <p>{approvalWarning}</p>}
      {approvals.length ? approvals.map(({ target, approval, cachedAt, unavailable }) => <NodeRow key={`${target}:${approval.approval_id}`} name={approval.description} description={`${target} · ${approval.thread.thread_id}`} state={unavailable ? "来源不可读，保留上次结果" : cachedAt == null ? "待在来源端处理" : "显示本机缓存"}><small>请在来源电脑的 Codex 客户端中处理。</small></NodeRow>) : approvalWarning ? "当前无法确认是否存在待审批事项。" : "暂无待审批事项。"}
    </div>
  </section>;
}

function App() {
  const [theme, setTheme] = useState<"eink" | "light" | "dark">("eink");
  const [panel, setPanel] = useState<Panel>("overview");
  const [runtime, setRuntime] = useState("正在检查本地界面与 IPC…");
  const [runtimeOk, setRuntimeOk] = useState(false);
  const [device, setDevice] = useState<DeviceState | null>(null);
  const [network, setNetwork] = useState<NetworkState>({ listening: false, onlinePeers: [] });
  const [localSourceState, setLocalSourceState] = useState<LocalSourceState>({ capable: false, ready: false });
  const [members, setMembers] = useState<Member[]>([]);
  const [membersTotal, setMembersTotal] = useState(0);
  const [membersTruncated, setMembersTruncated] = useState(false);
  const [groupSources, setGroupSources] = useState<GroupSourceSnapshot[]>([]);
  const [groupSourceError, setGroupSourceError] = useState("");
  const [target, setTarget] = useState("");
  const targetRef = useRef("");
  const [targets, setTargets] = useState<Array<{ id: string; label: string }>>([]);
  const [onlineTargets, setOnlineTargets] = useState<string[]>([]);
  const [capabilitiesByTarget, setCapabilitiesByTarget] = useState<Record<string, string[]>>({});
  const targetStateKeyRef = useRef("");
  const targetRefreshSequence = useRef(0);
  const groupSourceRefreshSequence = useRef(0);

  useEffect(() => { document.documentElement.dataset.theme = theme; }, [theme]);
  useEffect(() => {
    invoke<{ platform: string }>("app_environment").then((environment) => {
      setRuntime(`${environment.platform} · 本地资源 · IPC 正常`); setRuntimeOk(true);
    }).catch((caught) => setRuntime(message(caught)));
  }, []);

  const refreshTargets = useCallback(async () => {
    const sequence = ++targetRefreshSequence.current;
    const [local, network, members, source] = await Promise.all([
      invoke<DeviceState>("p1_state"),
      invoke<NetworkState>("p1_network_state"),
      invoke<MemberList>("p1_members"),
      invoke<LocalSourceState>("p2_source_state"),
    ]);
    if (sequence !== targetRefreshSequence.current) return targetRef.current;
    const stateKey = JSON.stringify({ local, network, members, source });
    if (targetStateKeyRef.current === stateKey) return targetRef.current;
    targetStateKeyRef.current = stateKey;
    setDevice(local);
    setNetwork(network);
    setLocalSourceState(source);
    setMembers(members.items);
    setMembersTotal(members.total);
    setMembersTruncated(members.truncated);
    setCapabilitiesByTarget(Object.fromEntries(members.items.map((member) => [member.deviceId, member.capabilities || []])));
    const next: Array<{ id: string; label: string }> = [];
    if (local.phase === "joined" && local.codexCapable && local.deviceId) next.push({ id: local.deviceId, label: `${local.deviceName}（${source.ready ? source.mode === "sharedDaemon" ? "共享来源可用" : "历史来源可用" : "本机来源不可用"}）` });
    const online = new Set(network.onlinePeers || []);
    const nextOnline = [...online];
    if (local.phase === "joined" && local.codexCapable && local.deviceId && source.ready) nextOnline.push(local.deviceId);
    setOnlineTargets(nextOnline);
    for (const member of members.items) {
      if (member.local || member.codexCapable === false || next.some((item) => item.id === member.deviceId)) continue;
      const name = member.deviceName || member.deviceId;
      const status = network.peerStatuses?.[member.deviceId] || member.status || "offline";
      const suffix = status === "online"
        ? "设备在线，来源状态待请求"
        : status === "connecting"
          ? "正在连接"
          : status === "authenticationFailed"
            ? "身份验证失败"
            : status === "incompatible"
              ? "版本不兼容"
              : "设备离线，可尝试缓存";
      next.push({ id: member.deviceId, label: `${name}（${suffix}）` });
    }
    setTargets(next);
    const selected = next.some((item) => item.id === targetRef.current) ? targetRef.current : next[0]?.id || "";
    targetRef.current = selected;
    setTarget(selected);
    return selected;
  }, []);

  const selectTarget = useCallback((value: string) => {
    targetRef.current = value;
    setTarget(value);
  }, []);

  const handleDeviceState = useCallback((next: DeviceState) => {
    setDevice(next);
    void refreshTargets().catch(() => undefined);
  }, [refreshTargets]);

  useEffect(() => {
    if (runtimeOk) void refreshTargets().catch(() => undefined);
    if (!runtimeOk) return;
    let running = false;
    const poll = async () => {
      if (running || document.visibilityState !== "visible") return;
      running = true;
      try { await refreshTargets(); } catch { /* Visible actions report connection errors. */ }
      finally { running = false; }
    };
    const timer = window.setInterval(() => void poll(), 5_000);
    return () => window.clearInterval(timer);
  }, [runtimeOk, refreshTargets]);

  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState !== "visible" || !window.__TAURI_INTERNALS__?.invoke) return;
      void invoke<NetworkState>("p1_resume_revalidate")
        .catch((caught) => setRuntime(`恢复局域网连接失败：${message(caught)}`))
        .finally(() => void refreshTargets().catch(() => undefined));
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [refreshTargets]);

  const supports = useCallback((deviceId: string, operation: BusinessOperation) =>
    (capabilitiesByTarget[deviceId] || []).includes(OPERATION_CAPABILITY[operation]), [capabilitiesByTarget]);
  const canRequest = useCallback((deviceId: string, operation: ReadBusinessOperation) =>
    Boolean(deviceId) && onlineTargets.includes(deviceId) && supports(deviceId, operation), [onlineTargets, supports]);
  const canRead = useCallback((deviceId: string, _operation: ReadBusinessOperation) =>
    targets.some((target) => target.id === deviceId), [targets]);
  const tools: TargetTools = { target, targets, onlineTargets, capabilitiesByTarget, localSourceState, supports, canRequest, canRead, setTarget: selectTarget, refreshTargets };
  const sourceTargetKey = targets.map((source) => `${source.id}:${onlineTargets.includes(source.id) ? "online" : "offline"}:${[...(capabilitiesByTarget[source.id] || [])].sort().join(",")}`).sort().join(";");
  const refreshGroupSources = useCallback(async (force = false) => {
    const sequence = ++groupSourceRefreshSequence.current;
    try {
      const result = await groupSourceSnapshots(tools, force);
      if (sequence !== groupSourceRefreshSequence.current) return;
      setGroupSources(result.items);
      const partial = result.items.filter((source) => source.value.errors?.quota || source.value.errors?.threads).length;
      const warnings = [
        ...result.failures.slice(0, 5).map((failure) =>
          `来源 ${failure.target} 当前不可读：${boundedText(failure.error, 512)}`,
        ),
        result.failures.length > 5 ? `另有 ${result.failures.length - 5} 个来源当前不可读。` : "",
        partial ? `${partial} 个来源的额度或会话统计仅部分可用。` : "",
      ].filter(Boolean);
      setGroupSourceError(warnings.join(" "));
    } catch (caught) {
      if (sequence === groupSourceRefreshSequence.current) setGroupSourceError(message(caught));
    }
  }, [sourceTargetKey, refreshTargets]);

  useEffect(() => {
    if (panel !== "overview" && panel !== "quota") return;
    if (document.visibilityState === "visible") void refreshGroupSources();
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") void refreshGroupSources();
    }, 60_000);
    const onVisibility = () => {
      if (document.visibilityState === "visible") void refreshGroupSources();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [panel, refreshGroupSources]);

  return <main className="app-shell">
    <header><div><p className="eyebrow">跨设备监控台</p><h1>Nekodex</h1><p data-ok={runtimeOk || undefined}>{runtime}</p></div><button type="button" onClick={() => setTheme((value) => value === "eink" ? "light" : value === "light" ? "dark" : "eink")}>主题：{theme === "eink" ? "墨水" : theme === "light" ? "浅色" : "深色"}</button></header>
    <nav aria-label="监控台页面">{([ ["overview", "总览"], ["quota", "额度"], ["activity", "活动"], ["threads", "会话"] ] as Array<[Panel, string]>).map(([id, label]) => <button key={id} type="button" className={panel === id ? "active" : ""} aria-current={panel === id ? "page" : undefined} onClick={() => setPanel(id)}>{label}</button>)}</nav>
    <div hidden={panel !== "overview"}><Overview device={device} network={network} members={members} membersTotal={membersTotal} membersTruncated={membersTruncated} sources={groupSources} sourceError={groupSourceError} openQuota={() => setPanel("quota")} onState={handleDeviceState} /></div>
    <div hidden={panel !== "quota"}><QuotaPanel sources={groupSources} error={groupSourceError} refresh={() => refreshGroupSources(true)} /></div>
    <div hidden={panel !== "activity"}><ActivityPanel tools={tools} active={panel === "activity"} /></div>
    <div hidden={panel !== "threads"}><ThreadsPanel tools={tools} active={panel === "threads"} /></div>
  </main>;
}

createRoot(document.getElementById("root")!).render(<React.StrictMode><App /></React.StrictMode>);
