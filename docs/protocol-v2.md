# Codex Monitor 通用协议（v2）

Android 10 节点协商 `peer-sync-p256-v1` 与 `state-snapshot-v1`。桌面端随后发送
`AndroidSnapshot`：`payloadJson` 保留被签名的精确 UTF-8 JSON，签名消息为
`codex-monitor/android-snapshot-v1:<networkId>:<senderId>:<payloadJson>`，并使用配对时已固定的
P-256 公钥验证。Android 因此不依赖平台 Ed25519，也不会接受未认证的状态快照。

节点之间使用换行分隔 JSON over TCP（单帧上限 512 KiB）。Linux、Windows 与 Android 都是完整节点；
ESP32 等只读显示器仍可只实现 HTTP/JSON。所有时间字段为 Unix 秒，所有 ID 和 token 都是 UTF-8 字符串。

## 节点发现与配对

发现流程参考 [LocalSend Protocol v2.1](https://github.com/localsend/protocol)：节点启动即公告稳定身份，
接收方记录来源地址并主动回连，身份指纹用于排除自发现和记忆设备。Codex Monitor 不复用其文件传输
协议，并额外要求一次性配对码和双方签名挑战，不能把未经认证的 UDP 地址直接视为可信节点。

- UDP multicast：`239.255.77.77:37991`，TTL 为 1；公告包含 `networkId`、`hostId`、
  `publicKey`、TCP `port`、可选 HTTP `httpPort` 和能力列表。节点同时向
  `255.255.255.255:37991` 发送相同公告，
  作为部分 WLAN 禁止 multicast 时的轻量兜底。
- TCP peer：默认 `8787`。首条 `AgentHello` 只含身份/能力元数据，不含配对码。新配对使用
  `codex-monitor/pair-transcript-v2`：按 initiator、acceptor 顺序，以 32 位大端长度前缀编码双方
  host ID、主公钥、配对公钥、network ID 和 acceptor 随机 challenge。challenge 与最终
  `PairAccepted` 均由接收方身份签名；发起方的 `PairResponse.codeProof` 和接收方的
  `PairAccepted.codeProof` 分别是以配对码为密钥、区分 `initiator`/`acceptor` 角色的 HMAC-SHA256。
  只有验证对端的签名及返回的 code proof 后，发起方才持久化 trust；未认证的 `PairAccepted` 必须拒绝。
- 配对码为区分大小写的 8 位 ASCII 字母数字串，生成时至少包含数字、大写字母和小写字母各一个。每次读取或刷新都创建新一代配对码，旧代立即失效。
- 未配对节点只进入发现列表。首次配对必须由用户输入目标节点当前配对码，但码不会进入 `AgentHello` 或其他网络字段；双方只交换绑定上述 transcript 的 HMAC proof。同一码最多完成一次配对，成功后立即消费。失败握手不会明文暴露配对码，但捕获到 proof 仍具有下述离线枚举边界。
- 已保存的 peer 按持久公钥重连，不需要配对码。刷新配对码不会清除信任记录。
- 地址变化不影响身份。节点重新公告后按 `hostId` 重连；Android 节点和桌面节点一样监听 multicast、回应发现并接受 TCP 回连。

安全边界：随机 challenge、角色标签和 transcript 绑定可阻止主动 MITM 替换任一身份、公钥或网络后仍完成配对；旧的未认证 `PairAccepted` 不再被接受。此方案不是 PAKE：旁观者可从一次捕获的 HMAC proof 离线枚举 8 位短码，因此应通过可信的本地 UI/通道安全传递短时配对码，并在发现泄露时刷新。代码从不明文上网，但 HMAC 不能隐藏低熵口令的离线猜测风险。

## 签名信封

```json
{
  "version": 2,
  "kind": "state_snapshot",
  "networkId": "home-lan",
  "senderId": "ed25519:0123456789abcdef",
  "sequence": 42,
  "sentAt": 1760000000,
  "payload": {},
  "signature": "<128 hex chars>"
}
```

签名是上述字段按固定 JSON 字段顺序序列化、排除 `signature` 后的 Ed25519 签名。接收方
必须先验证 `version`、`networkId`、发送者公钥和签名，再按来源 `sequence` 丢弃重复/乱序快照。
节点只对本机采集的 `sourceNodeId` 负责；视图用稳定排序合并来源，离线来源保留最后快照并标记
`offline` 或 `stale`。

## 客户端 HTTP profile

```text
GET /api/v1/identity
GET /api/v1/peers
GET /api/v1/discoveries
GET /api/v1/state
GET /api/v1/state?since=<cursor>
GET /api/v1/health
```

响应是 JSON；`/api/v1/state` 的 `cursor` 是不透明、可被 JavaScript 精确表示的整数，未变化时返回 `304`。只读 token
使用 `Authorization: Bearer <token>`，由 `CODEX_MONITOR_READ_TOKEN` 开启。控制 token 与之
分离，ESP32 等只读设备不应保存或发送控制 token。

`GET /api/v1/discoveries` 仅返回通过公告校验且未过期的局域网节点。`POST /api/v1/pair`
和 `PATCH /api/v1/identity` 必须使用控制 token；未配置控制 token 时返回 403。配对只接受
发现缓存中的 `nodeId` 和配对码，连接地址来自已验证公告，不接受 HTTP 传入地址。
`pairingCode` 必须是 8 位 ASCII 字母数字，否则返回 400；若对应节点的握手尚在进行，重复请求返回
409 `peer connection is already in progress`，只有已确认的 peer 连接才返回 `connected`。
`GET /api/v1/pairing-code` 同样只接受控制 token。每次成功请求都会轮换并返回一枚新的一次性配对码，响应使用
`Cache-Control: no-store`；旧码立即失效，且配对码不会出现在只读 identity、state 或 discoveries 响应中。
用户通过 `PATCH /api/v1/identity` 保存的昵称优先于 `CODEX_MONITOR_ALIAS`，因此服务重启后
仍保持 UI 改名；只有尚无持久化昵称时才采用环境变量。

节点的 `peers` 视图会在已配对连接上返回 `httpEndpoint`（当前 IP）和可选的
`httpEndpoints`（主机名/`.local` 候选）。PWA 可缓存这些地址，在当前入口断线时依次尝试其他
节点；主机名候选依赖局域网 DNS、LLMNR 或 mDNS，解析不可用时客户端应保留手工 IP 入口；
这不是 IP 扫描，且仍受节点 HTTP token 和局域网边界保护。

控制目前只有：

```json
{
  "action": "interrupt",
  "targetNodeId": "ed25519:...",
  "threadId": "thread-id",
  "turnId": "turn-id",
  "requestId": "request-id",
  "expiresAt": 1760000060
}
```

发送到 `POST /api/v1/control`，需要 `CODEX_MONITOR_CONTROL_TOKEN`。没有可靠 `turnId` 时
客户端不得伪造请求；协议不提供任意 shell、文件操作或 Codex 凭据传输。

## 嵌入式最小实现

ESP32 可以只实现 HTTPS/HTTP GET、JSON 解析和一个本地缓存：启动时请求 `/api/v1/state`，
之后携带 `since`，遇到 `304` 保留屏幕，遇到连接失败显示最后更新时间和 stale 状态。它不需要
加入 peer 配对，也不需要解析 TCP JSONL；若后续需要 MQTT/WebSocket，适配层应保持相同的
`NetworkView` 字段和 cursor 语义。
