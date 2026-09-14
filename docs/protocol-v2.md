# Codex Monitor 通用协议（v2）

PC 节点之间使用换行分隔 JSON over TCP（单帧上限 512 KiB）；Poke3、ESP32 和其他设备只需要 HTTP/JSON，
不需要 Rust 运行时。所有时间字段为 Unix 秒，所有 ID 和 token 都是 UTF-8 字符串。

## 节点发现与配对

- UDP multicast：`239.255.77.77:37991`，TTL 为 1；公告包含 `networkId`、`hostId`、
  `publicKey`、TCP `port`、可选 HTTP `httpPort` 和能力列表。节点同时向
  `255.255.255.255:37991` 发送相同公告，
  作为部分 WLAN 禁止 multicast 时的轻量兜底。
- TCP peer：默认 `8787`。首条消息为 `AgentHello`，随后是 Ed25519 challenge-response。
- 配对码为区分大小写的 8 位 ASCII 字母数字串；随机生成时至少包含一个数字、一个大写字母和一个小写字母。格式校验只要求 8 位 ASCII 字母数字。
- 节点只接受同一 `networkId` 且已保存公钥的 peer；首次配对需要短期配对码。自动初次连接需两端配置相同配对码；`POST /api/v1/pair` 手动配对使用目标对端的配对码，两端本地配对码无需相同。
- 旧版本的 6 位 `CODEX_MONITOR_PAIR_CODE` 配置需更换为 8 位；配对码变更不会删除或重置已保存的对端公钥。
- 地址变化不影响身份。节点重新公告后按 `hostId` 重连，Poke3 不监听 multicast。

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
`GET /api/v1/pairing-code` 同样只接受控制 token，返回本节点当前握手配对码且使用
`Cache-Control: no-store`；它不会出现在只读 identity、state 或 discoveries 响应中。
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
