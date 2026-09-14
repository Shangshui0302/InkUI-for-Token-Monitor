# 决策日志

## 2026-09-14：Poke3 无边框 APK

- Android WebView 壳移除原生 ActionBar，监控页默认以沉浸模式隐藏状态栏和导航栏；首次节点地址配置仍显示系统栏，避免妨碍输入。
- 网页“显示”设置仅在受限 Android bridge 存在时展示两个系统栏开关，选择写入 APK 私有偏好并在窗口重新获得焦点和冷启动时恢复；普通浏览器不显示这些选项。
- Poke3 实机完成默认沉浸、分别显示状态栏/导航栏、重新开启并冷启动恢复验证；Onyx 悬浮控件仍由系统管理。
- 使用图像模型重新生成统一的像素黑猫图标母版，PWA、页头以及 Android legacy/round/adaptive launcher 资源共用该视觉；兼容性审查同时修复直接 `aapt2` 资源编译与 Monitor PNG 静态路由缺口。

## 2026-09-11：PWA 视觉与节点设置重做

- PWA 保持原生 HTML/CSS/JS，四页改为总览、额度、活动、会话；总览额度/节点使用 3:2 栅格，响应式断点覆盖桌面、窄屏和 600×800 Eink。
- 头部使用 48px 原创坐猫 SVG；主题色保留七色 `accent/strong/soft` 选择并写入 localStorage，Eink 覆盖为黑/黑/白但不丢失选择。
- 活动页从 14 天窗口改为自然月周一开列日历，多账户选择、缺失日与 0 token 分离、选中日详情和 Eink 点密度均由现有 `dailyUsage` 渲染，不补造数据。
- 设置改为兼容 Chrome 85 的固定遮罩 modal，分显示/设备/高级 tab；设备页读取 `/api/v1/identity`、`/api/v1/discoveries` 和 peers，并仅用 control token 调用配对、改名、按需显示本机配对码。
- Spark bucket 仅在展示层按 `limitId=codex_bengalfox` 或 `limitName=GPT-5.3-Codex-Spark` 过滤，原始 state 不变；本轮未重启或构建内嵌 server，待统一集成后做浏览器与真实设备验收。

## 2026-09-10：部署收尾与验收证据边界

- 源码已安全部署到 Windows `D:\TemporaryWorkSpace\codex-monitor-UI-mesh`；Windows x64 release build 已成功产出 `codex-monitor-server.exe` 和 `codex-monitor-agent.exe`。
- Windows 真实程序受控启动后可在 500ms 内返回 localhost HTTP 200；已定位 `codex-cli 0.146.0`，未把账号或凭据写入长期记录。
- NixOS 真实 collector 已读到实际 sessions、account、300 分钟（五小时）/10080 分钟（一周）额度窗口和 daily usage buckets；字段验证不等同于 Poke3 实机可读性。
- Podman 双节点协议链路、定向 discovery 发送单测已通过；这不替代真实两台机器的最终验收。
- Poke3 尚未加入网络，未做 BOOX 实机安装、断线恢复或控制验收。
- 本轮 NixOS↔Windows 真实 mesh 因验收编排/启动路径问题尚未形成有效证据；这是验收证据缺口，不应解释为程序失败。
- Windows 实机的 HTTP `8788` 入站防火墙规则尚未配置；明日先用临时状态启动双方完成真实 mesh，再由管理员添加该规则，最后让 Poke3 加入。DHCP 地址不作为长期依赖，优先使用局域网 DNS/mDNS 主机名。

## 2026-09-10：额度兼容与 PWA 更新

- 兼容旧版单桶 `rateLimits` 缺少 `limitId` 的响应，保留额度窗口而不丢弃整桶；未登录账户在 PWA 额度页明确显示“需要登录”。
- PWA 按 300 分钟和 10080 分钟识别五小时/一周额度，字段顺序变化时不再盲标，仍兼容缺少时长的旧响应。
- 线程状态归一化补充 `inProgress/completed/failed`，避免 App Server 状态命名变化让会话页误显示为未知。
- Service Worker 缓存版本升至 `v9`，并缓存 Poke3 PWA 的高对比 SVG 图标；中断按钮发送前增加原生确认。
- manifest 当前保留 SVG 图标，不预先生成 PNG；Poke3 实机安装若不采纳该图标，再依据实机结果决定是否补 PNG。

## 2026-09-10：真实 App Server 采集闭环

- 使用本机实际 `codex app-server` 验证：会话、ChatGPT `plus` 计划、5 小时/一周额度和 23 天 `dailyUsageBuckets` 均进入 `/api/v1/state`；首次启动的模型刷新可能让一次额度请求超时，后续采集恢复为 `ready`，不属于字段解析失败。
- collector 对单次请求故障按字段保留上一份可用会话/账户数据，同时把快照标记为 `degraded`；不会因一次额度或 usage 超时清空 Poke3 当前显示。
- App Server 进程整体短暂不可用时也保留上一份会话/账户 payload，只更新 sequence、时间和 `unavailable` 错误，随后由 collector 重启流程恢复。

## 2026-09-10：三平台部署入口

- 新增 `docs/deployment.md`，固定 NixOS module、Windows `D:\TemporaryWorkSpace` 脚本和 Poke3 PWA 的启动顺序、端口、配对码、token 与入口故障转移说明。
- Windows 增加基于用户 env 文件的登录启动 Scheduled Task；secret 不出现在任务命令行，ESP32 继续复用通用 HTTP/JSON 接口。
- 修正 NixOS 防火墙说明：新节点 mesh 需要入站 TCP `8787`，旧 Agent 才只需要对 Monitor 出站。

## 2026-09-10：NixOS 声明式运行入口

- flake 暴露 `nixosModules.codex-monitor`，以 systemd 服务运行同一对等节点；服务用户、HTTP 绑定、network ID、发现接口和防火墙开关可配置。
- NixOS module 与 Windows 启动脚本都允许显式指定 `CODEX_MONITOR_CODEX_BIN`，避免 systemd/Powershell PATH 与用户环境不一致。
- 配对码、只读 token、控制 token 通过 systemd `EnvironmentFile` 注入，避免把 secret 写入 Nix store。
- 不自动执行 `nixos-rebuild switch`；模块只提供声明式配置入口和运行边界。

## 2026-09-10：本机节点运行闭环

- 使用隔离身份/配对/状态路径和 loopback HTTP 端口启动节点；内嵌 PWA 返回 200，`/api/v1/state`、`/api/v1/identity`、`/api/v1/peers` 正常，未配置控制 token 时 `POST /api/v1/control` 返回 403。
- Codex collector 以不存在的测试路径启动，按预期显示 `unavailable`；未连接或中断真实 Codex 会话。跨主机和 Poke3 实机验收仍需用户侧部署。

## 2026-09-10：Poke3 入口故障转移

- 节点公告和对等 `AgentHello` 携带可选 HTTP 端口，已配对节点在 `/api/v1/state` 的 `peers[].httpEndpoint(s)` 暴露 IP/主机名入口候选。
- PWA 缓存最多四个已知入口；当前入口请求失败时按顺序尝试其他节点，并继续使用同一只读/控制 token。
- 主机名和 `.local` 入口标记为 best-effort，依赖局域网 DNS/LLMNR/mDNS；解析不可用时保留手工 IP 入口。
- 不做局域网 IP 扫描；若各节点 token 或 HTTP 绑定不同，仍需在 PWA 中手动配置对应入口。

## 2026-09-10：跨节点时间显示

- PWA 主机卡片和连接摘要优先显示来源 `lastSnapshotAt`，不显示各节点本地接收时间，避免同一快照在不同 PC 上出现人为时间差。

## 2026-09-10：控制请求防重放

- 空的 `CODEX_MONITOR_PAIR_CODE` 现在按未配置处理并生成随机码，避免误把空字符串当作首次配对凭据。
- 被动握手复用 `Identity::host_id_from_public_key` 做公钥/host id 校验，异常 Unicode 输入不会再触发字节切片 panic。
- 节点对已验签的控制请求按 `senderId + requestId` 做进程内幂等保护；有效期内重复请求只返回失败结果，不再次进入 Codex collector。
- 缓存最多保留 256 条并按 `expiresAt` 清理；控制请求本身仍限制为短期有效，暂不引入数据库级幂等。
- 对等控制路由带连接代次；同一节点短暂保留多条连接并始终选最新，最新连接失败时回退旧连接。
- Registry 的断开状态仅在该节点全部连接回收后更新，旧连接回收不会把仍在线节点短暂标记为离线。
- 路由发送在连接队列已关闭但清理尚未完成的窗口内也会按新到旧重试，并摘除失效代次。

## 2026-09-10：缓存网络隔离

- 节点启动加载状态缓存时校验 `sourceNodeId` 和当前 `networkId`；切换逻辑网络后丢弃旧网络或
  无网络标记的缓存快照，避免离线视图串网。

## 2026-09-10：PWA cursor 精度

- `state.cursor` 收敛到 JavaScript 安全整数范围；Poke3 的 `since` 请求不会因 `Number` 精度
  丢失而持续绕过 `304`。

## 2026-09-10：P6 受限 interrupt 控制

- `codex app-server` 的 `turn/interrupt` 已按已确认 schema 接入，活动/等待会话通过受限 `thread/read`
  尝试补齐当前 `turnId`，没有可靠 turn id 时不伪造；
- `POST /api/v1/control` 使用独立的 `CODEX_MONITOR_CONTROL_TOKEN`，请求经过本机路由器后可交给本机
  collector 或已连接 peer；peer 间使用签名 `control_request`/`control_result` envelope；
- 控制请求有短期过期时间、request id、目标 node id 和 capability 校验，未配置控制 token 时入口关闭；
- 当前只交付 interrupt；Poke3 会话页在存在可靠 `turnId` 时显示按钮，follow-up、stop、resume、rename/archive 暂不实现。
- 主动连接和被动接受连接的两端都注册 peer control route，因此任一节点都可以把目标会话请求
  发给另一台已配对节点；连接断开时挂起请求会返回可诊断失败。
- TCP 与 App Server 的 JSONL 读取都在读入阶段限制为 `max + 1` 字节，超长或无换行帧会被拒绝，
  不再让异常输入无限增长内存。
- 对等 JSONL 帧上限从 64 KiB 调整为 512 KiB，协议会话上限收紧为与 collector 一致的 25 条；
  高转义文本的最大会话快照也由协议测试保证可容纳，同时仍保持有界读取。

## 2026-09-10：跨平台运行入口

- `flake.nix` 暴露 `codex-monitor-node` package，构建同一套 Rust 节点和兼容 Agent；
- Windows 使用 `packaging/windows/build.ps1` 生成两个 `.exe`，`firewall.ps1` 只为 Private 网络
  开放 mesh、PWA 和 discovery 端口；
- `run-node.ps1` 统一设置 network ID、配对码、HTTP 绑定和可选 token，减少 Windows 启动时的手工配置；
- `docs/protocol-v2.md` 固化 JSON-first envelope 与 HTTP/JSON profile，嵌入式客户端不需要 Rust。

## 2026-09-10：发现兜底

- 节点继续使用独立 multicast，同时向 `255.255.255.255:37991` 发相同公告；用于部分 WLAN
  禁止 multicast 的场景，仍由 network ID、配对码和公钥校验决定是否加入网络。

## 2026-09-10：去中心化节点 P0-P5 迭代

- `codex-monitor-server` 迁移为可对等运行的 PC 节点：同一 `networkId` 下通过 multicast 发现，
  使用 Ed25519 配对和签名 `state_snapshot`，按来源 sequence 合并，旧 Agent 入口继续兼容；
- `/api/v1/identity`、`/api/v1/peers`、`/api/v1/state`、`/api/v1/health` 与 cursor/304 已加入，
  本地状态缓存只在重启后显示离线，不伪造实时状态；
- Poke3 PWA 增加 IndexedDB 最后快照、离线 stale 标记和手工节点地址切换；可选只读 API token
  通过 `CODEX_MONITOR_READ_TOKEN` 开启，中央节点仍只是可选入口，不承载唯一业务权威；
- 本轮只做定向编译、协议/注册表测试和 localhost HTTP smoke，真实 NixOS↔Windows、Poke3
  实机和 DHCP 重连验收留待部署时执行。

## 2026-09-09：初始化方案

- 中央 Monitor 定为 Windows 10 主机，原因是用户希望该设备承担中心角色；
- NixOS 主机运行远程 Agent；
- BOOX Poke3 首版使用 Android 浏览器/PWA，不先开发原生 App；
- 主机发现采用 LocalSend 风格 UDP multicast，但使用独立服务名、端口和消息格式；
- Agent 主动连接 Monitor，避免固定 IP 和日常 SSH；
- 主开发环境选择 NixOS 宿主机，通过项目 flake 提供 Rust 工具链；
- 首版不复制 `auth.json`、不暴露任意 shell、不自动审批；
- 许可证、是否需要 APK、Windows Service 形式仍为 `TBD`。

## 2026-09-09：开启 M1

- `main` 已从 bootstrap 分支快进到提交 `014b4f9`；M1 在独立分支 `codex/m1-discovery` 开发；
- M1 传输先采用换行分隔 JSON over TCP，保留后续切换 WebSocket 的 payload 兼容边界；
- Monitor 广播 `239.255.77.77:37991`，控制链路监听 TCP `8787`；公告每 5 秒刷新、20 秒过期；
- Agent 使用出站连接，首次用配对码，之后用双方持久化的公钥；Agent 会 pin Monitor 身份，heartbeat 超时后重新发现；
- 本机联调已验证发现、配对和 heartbeat；跨主机 DHCP/Windows 防火墙验证留在 M1 收尾。

## 2026-09-09：M1.1 组播接口选择

- 实测 Mihomo/TUN 会在两台主机上抢占 `224.0.0.0/4`，导致未指定接口的组播发现偏离 WLAN；
- `if-addrs` 枚举接口，自动跳过 loopback、点对点、虚拟/保留隧道地址，并优先真实私有 LAN；
- `socket2` 为 Monitor 设置 `IP_MULTICAST_IF`，Agent 在选定接口加入组播；
- `CODEX_MONITOR_DISCOVERY_INTERFACE` 支持按接口名或 IPv4 地址覆盖自动选择；
- Windows 本机发现/配对，以及 NixOS↔Windows 跨主机发现、首次配对和无配对码重连已通过；
- NixOS 防火墙通过声明式 UDP `37991` 放行，Windows 端规则限定为 Private 网络。

## 2026-09-09：M2 只读状态链

- 新增 `codex-monitor-codex-client`，通过本机 `codex app-server --listen stdio://` 读取
  thread、账户、额度窗口和 usage 摘要；请求只读且关闭 token refresh；
- `protocol` 增加 `StateSnapshot`、账户/额度/会话类型和 `state-snapshot-v1` capability；
- Agent 仅在 Monitor 公告协商该 capability 后上报快照，Monitor 按序列号丢弃旧快照；
- Monitor 增加内存 registry 和 loopback HTTP API：`/api/state`、`/api/hosts`、`/api/accounts`；
- 同一账户在多台主机出现时按本地生成的分组 key 合并，额度不相加；保留 host offline/stale
  和 collector unavailable 状态；
- M2 已通过 NixOS 本机纵向验证和 Windows/NixOS 双主机验收；两个主机的 collector 均能返回
  会话、计划和额度窗口，Monitor API 保留 stale/offline 状态并合并同一账户。

## 2026-09-09：M3 Poke3 PWA

- Poke3 首版采用原生 HTML/CSS/JS 和同端口静态资源，不引入 React/Vue 或 Node 构建链；
- 页面每 15 秒轮询 `/api/state`，保留手动刷新和断线/过期提示；SSE 延后到确有功耗或
  延迟需求时再加入；
- Service Worker 只缓存静态壳，`/api/*` 永不缓存，避免额度和会话显示旧数据；
- Monitor 默认继续 loopback，用户显式 LAN 绑定时需使用可信 Private 网络；M3 不开放写入或
  未认证控制接口。

## 2026-09-09：M3.1 小屏活动视图

- `account/usage/read` 的 `dailyUsageBuckets` 已进入受限 `AccountState`，只接受严格日期和
  非负 token，并限制最多 400 个 bucket；不使用日用量反推五小时/一周额度；
- Poke3 页面改为“总览 / 额度 / 活动 / 会话”按钮切换，账户、主机和会话使用分页；
- 额度窗口显示为“五小时限制”和“一周限制”，隐藏 `windowDurationMins` 文本；
- 活动页默认显示最近 14 天热力图，具体日期和 token 同时显示；App Server 未返回的历史不
  伪造，持久化历史留到后续阶段。
